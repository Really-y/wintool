//! Microsoft Update Catalog: the official, Microsoft-signed driver packages behind Windows
//! Update, searchable by hardware ID. Used for problem devices Windows Update itself
//! offers nothing for.
//!
//! The catalog search is fuzzy, so every candidate is checked on its details page:
//! classification (drivers only, never firmware), Windows version, architecture and the
//! package's supported hardware IDs must match the device. Packages are downloaded as
//! .cab to the user's %TEMP%, unpacked with `expand` and installed with pnputil; the next
//! candidate is tried only if the device is still broken.

use crate::drivers::{self, ProblemDevice};
use crate::online;
use crate::util::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

const BASE: &str = "https://www.catalog.update.microsoft.com/";
/// Bigger packages (graphics suites) are left to Windows Update / the vendor.
const MAX_SIZE: u64 = 400 * 1024 * 1024;
/// Details pages checked per search, packages kept per device.
const MAX_DETAILS: usize = 8;
const MAX_PKGS: usize = 3;

pub struct Package {
    pub id: String,
    pub title: String,
    /// (year, month, day)
    pub date: (u32, u32, u32),
    pub size: u64,
}

impl Package {
    pub fn date_str(&self) -> String {
        let (y, m, d) = self.date;
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// Device Manager codes where a different driver can help.
pub fn wanted(code: u32) -> bool {
    matches!(code, 1 | 10 | 18 | 19 | 28 | 31 | 32 | 37 | 39 | 40 | 41 | 43 | 48 | 52)
}

// ---------------------------------------------------------------- HTML parsing

fn decode(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// Visible text of an HTML fragment, whitespace collapsed.
fn text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    decode(&out.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn is_guid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

struct Row {
    id: String,
    title: String,
    products: String,
    class: String,
    date: (u32, u32, u32),
    size: u64,
}

/// Result rows of a Search.aspx page. Each row is `<tr id="{guid}_R{n}">` with cells
/// `{guid}_C1_R{n}` (title) … `_C5_R{n}` and a hidden `{guid}_originalSize` span.
fn parse_search(html: &str) -> Vec<Row> {
    html.split("<tr id=\"")
        .skip(1)
        .filter_map(|seg| {
            let id = seg.get(..36).filter(|g| is_guid(g) && seg[36..].starts_with("_R"))?;
            let cell = |c: u32| -> String {
                let key = format!("{id}_C{c}_R");
                let Some(i) = seg.find(&key) else { return String::new() };
                let rest = &seg[i..];
                let body = rest.find('>').map(|j| &rest[j + 1..]).unwrap_or("");
                text(body.split("</td>").next().unwrap_or(""))
            };
            let size = seg
                .find("_originalSize\">")
                .and_then(|i| seg[i + 15..].split('<').next())
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            let d: Vec<u32> = cell(4).split('/').filter_map(|x| x.trim().parse().ok()).collect();
            let date = if d.len() == 3 { (d[2], d[0], d[1]) } else { (0, 0, 0) };
            Some(Row { id: id.to_string(), title: cell(1), products: cell(2), class: cell(3), date, size })
        })
        .collect()
}

/// Lowest build a catalog product line applies to, for Windows 10/11 product lines.
fn min_build(product: &str) -> Option<u32> {
    let p = product.to_lowercase();
    let win10 = p.starts_with("10");
    if !(win10 || p.starts_with("11") || p.starts_with("- client")) {
        return None;
    }
    // (keyword, Windows 10 build, Windows 11 build). Order matters: "fall creators" before "creators".
    let table: &[(&str, u32, u32)] = &[
        ("1507", 10240, 10240),
        ("1511", 10586, 10586),
        ("anniversary", 14393, 14393),
        ("1607", 14393, 14393),
        ("fall creators", 16299, 16299),
        ("creators", 15063, 15063),
        ("1703", 15063, 15063),
        ("1709", 16299, 16299),
        ("april 2018", 17134, 17134),
        ("1803", 17134, 17134),
        ("october 2018", 17763, 17763),
        ("1809", 17763, 17763),
        ("may 2019", 18362, 18362),
        ("1903", 18362, 18362),
        ("1909", 18363, 18363),
        ("vibranium", 19041, 19041),
        ("2004", 19041, 19041),
        ("20h2", 19042, 19042),
        ("21h1", 19043, 19043),
        ("cobalt", 22000, 22000),
        ("21h2", 19044, 22000),
        ("nickel", 22621, 22621),
        ("22h2", 19045, 22621),
        ("23h2", 22631, 22631),
        ("germanium", 26100, 26100),
        ("24h2", 26100, 26100),
        ("25h2", 26200, 26200),
    ];
    let found = table.iter().find(|(k, _, _)| p.contains(k)).map(|&(_, w10, w11)| if win10 { w10 } else { w11 });
    Some(found.unwrap_or(if win10 { 10240 } else { 22000 }))
}

/// Does the product column cover this Windows version (major, minor, build)?
fn os_matches(products: &str, (ma, mi, build): (u32, u32, u32)) -> bool {
    products.split("Windows").skip(1).map(str::trim_start).any(|p| {
        let low = p.to_lowercase();
        if low.contains("server") {
            return false;
        }
        match (ma, mi) {
            (10, _) => min_build(p).is_some_and(|min| build >= min),
            (6, 3) => low.starts_with('8'),
            (6, 2) => low.starts_with('8') && !low.starts_with("8.1"),
            (6, 1) => low.starts_with('7') || low.starts_with("vista"),
            _ => false,
        }
    })
}

/// Device hardware ID (instance ID without the instance part) is covered by `supported`
/// when it starts with it at a `&` boundary: `PCI\VEN_10EC&DEV_8168` covers
/// `PCI\VEN_10EC&DEV_8168&SUBSYS_…`, but not `PCI\VEN_10EC&DEV_81681`.
fn hwid_covered(device: &str, supported: &str) -> bool {
    let d = device.to_uppercase();
    let s = supported.trim().to_uppercase();
    !s.is_empty() && d.starts_with(&s) && (d.len() == s.len() || d[s.len()..].starts_with('&'))
}

/// Architecture and supported hardware IDs from a ScopedViewInline.aspx page.
fn parse_details(html: &str) -> (String, Vec<String>) {
    let arch = html
        .find("labelArchitecture_Separator")
        .and_then(|i| html[i..].find("</span>").map(|j| &html[i + j + 7..]))
        .map(|rest| text(rest.split('<').next().unwrap_or("")))
        .unwrap_or_default();
    let hwids = html
        .find("id=\"driverhwIDs\"")
        .map(|i| {
            let box_html = &html[i..];
            let end = box_html.find("boxTitleDetails").unwrap_or(box_html.len());
            box_html[..end]
                .split('<')
                .filter_map(|frag| frag.split_once('>').map(|(_, t)| decode(t.trim())))
                .filter(|t| t.contains('\\'))
                .collect()
        })
        .unwrap_or_default();
    (arch, hwids)
}

fn arch_ok(arch: &str) -> bool {
    let a = arch.to_uppercase();
    if a.is_empty() || a == "N/A" {
        return true;
    }
    if drivers::os_is_64bit() { a.contains("AMD64") || a.contains("X64") } else { a.contains("X86") }
}

// ---------------------------------------------------------------- network

/// In the elevated user's own %TEMP% (not writable by other users).
fn work_dir() -> PathBuf {
    std::env::temp_dir().join("WinTool-catalog")
}

fn get(url: &str) -> Option<String> {
    let f = work_dir().join("page.html");
    let _ = fs::create_dir_all(work_dir());
    let ok = online::download(url, &f);
    let body = fs::read(&f).ok().map(|b| String::from_utf8_lossy(&b).into_owned());
    let _ = fs::remove_file(&f);
    body.filter(|_| ok)
}

/// The instance ID minus the per-instance tail: `PCI\VEN_..&DEV_..&SUBSYS_..&REV_..`.
fn device_hwid(instance_id: &str) -> String {
    instance_id.splitn(3, '\\').take(2).collect::<Vec<_>>().join("\\")
}

/// Search queries, most specific first: without the revision, then vendor/device only.
fn queries(instance_id: &str) -> Vec<String> {
    let full = device_hwid(instance_id).to_uppercase();
    let no_rev = full.split("&REV_").next().unwrap_or(&full).to_string();
    let short = drivers::short_hwid(instance_id);
    let mut q = vec![no_rev];
    if !q.contains(&short) {
        q.push(short);
    }
    q
}

/// Matching packages for a device, newest first (at most MAX_PKGS). Silent; network heavy.
pub fn find(instance_id: &str) -> Vec<Package> {
    let up = instance_id.to_uppercase();
    if up.starts_with("ROOT\\") || up.starts_with("SWD\\") || !up.contains('\\') {
        return vec![];
    }
    let hw = device_hwid(instance_id);
    let ver = win_version();
    let mut seen: Vec<String> = vec![];
    let mut found: Vec<Package> = vec![];
    for q in queries(instance_id) {
        let Some(html) = get(&format!("{BASE}Search.aspx?q={}", drivers::url_encode(&q))) else { continue };
        let mut rows: Vec<Row> = parse_search(&html)
            .into_iter()
            .filter(|r| r.class.starts_with("Drivers") && r.size <= MAX_SIZE && os_matches(&r.products, ver))
            .filter(|r| !seen.contains(&r.id))
            .collect();
        rows.sort_by(|a, b| b.date.cmp(&a.date));
        for r in rows.into_iter().take(MAX_DETAILS) {
            seen.push(r.id.clone());
            let Some(page) = get(&format!("{BASE}ScopedViewInline.aspx?updateid={}", r.id)) else { continue };
            let (arch, hwids) = parse_details(&page);
            if arch_ok(&arch) && hwids.iter().any(|s| hwid_covered(&hw, s)) {
                found.push(Package { id: r.id, title: r.title, date: r.date, size: r.size });
                if found.len() >= MAX_PKGS {
                    return found;
                }
            }
        }
        if !found.is_empty() {
            break;
        }
    }
    found
}

/// Direct .cab URL of a package (DownloadDialog.aspx).
fn cab_url(id: &str) -> Option<String> {
    let json = format!(r#"[{{"size":0,"languages":"","uidInfo":"{id}","updateID":"{id}"}}]"#);
    let f = work_dir().join("dialog.html");
    let _ = fs::create_dir_all(work_dir());
    let body = format!("updateIDs={}", drivers::url_encode(&json));
    let ok = online::download_post(&format!("{BASE}DownloadDialog.aspx"), &body, &f);
    let html = fs::read_to_string(&f).unwrap_or_default();
    let _ = fs::remove_file(&f);
    if !ok {
        return None;
    }
    let i = html.find(".url = '")?;
    let url = html[i + 8..].split('\'').next()?;
    url.starts_with("https://").then(|| url.to_string())
}

/// Downloads, unpacks and installs one package. Returns true when pnputil accepted a driver.
fn install(pkg: &Package, dir: &Path) -> bool {
    let Some(url) = cab_url(&pkg.id) else { return false };
    let cab = dir.join(format!("{}.cab", pkg.id));
    let out = dir.join(&pkg.id);
    if !online::download(&url, &cab) || fs::create_dir_all(&out).is_err() {
        return false;
    }
    let unpacked = Command::new(sys("expand")).arg("-F:*").arg(&cab).arg(&out).output().map(|o| o.status.success()).unwrap_or(false);
    let _ = fs::remove_file(&cab);
    let mut infs = vec![];
    drivers::collect_inf(&out, &mut infs, 0);
    unpacked && drivers::install_infs(&infs) > 0
}

/// Tries the packages in order until the device works. Returns the title that fixed it.
/// Silent (runs under a spinner).
pub fn install_first_working(instance_id: &str, pkgs: &[Package]) -> Option<String> {
    let dir = work_dir();
    let _ = fs::create_dir_all(&dir);
    let mut fixed = None;
    for p in pkgs {
        if install(p, &dir) {
            drivers::rescan_devices();
            sleep(Duration::from_secs(4));
            if online::still_broken(&[instance_id.to_string()]).is_empty() {
                fixed = Some(p.title.clone());
                break;
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
    fixed
}

/// Problem devices the catalog should be searched for (`skip`: already covered elsewhere).
pub fn targets<'a>(probs: &'a [ProblemDevice], skip: impl Fn(&ProblemDevice) -> bool) -> Vec<&'a ProblemDevice> {
    probs.iter().filter(|d| wanted(d.code) && !skip(d)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = r#"<tr id="headerRow"><td>x</td></tr><tr id="227f2cf5-6758-4977-9c35-0359d9e1acd2_R1" style="">
<td class="a" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_C0_R1"></td><td class="a" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_C1_R1">
<a id='227f2cf5-6758-4977-9c35-0359d9e1acd2_link' onclick='goToDetails("x");'>
Realtek Semiconductor Corp. - MEDIA - 6.0.9876.1
</a>
</td><td class="a" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_C2_R1">
Windows 10,  Vibranium and later,  Servicing Drivers, Windows 10 S,  Vibranium and later,  Servicing Drivers
</td><td class="a" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_C3_R1">
Drivers (Sound)
</td><td class="a" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_C4_R1">
8/18/2025
</td><td class="a" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_C5_R1">
6.0.9876.1
</td><td class="a" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_C6_R1">
<span id="227f2cf5-6758-4977-9c35-0359d9e1acd2_size">12.3 MB</span>
<span class="noDisplay" id="227f2cf5-6758-4977-9c35-0359d9e1acd2_originalSize">12854346</span>
</td></tr>"#;

    #[test]
    fn parses_search_rows() {
        let rows = parse_search(ROW);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.id, "227f2cf5-6758-4977-9c35-0359d9e1acd2");
        assert_eq!(r.title, "Realtek Semiconductor Corp. - MEDIA - 6.0.9876.1");
        assert_eq!(r.class, "Drivers (Sound)");
        assert_eq!(r.date, (2025, 8, 18));
        assert_eq!(r.size, 12854346);
        assert!(os_matches(&r.products, (10, 0, 19045)));
        assert!(!os_matches(&r.products, (10, 0, 18363)));
        assert!(!os_matches(&r.products, (6, 1, 7601)));
    }

    #[test]
    fn product_lines() {
        let w11 = "Windows - Client, version 21H2 and later, Servicing Drivers";
        assert!(os_matches(w11, (10, 0, 22631)));
        assert!(!os_matches(w11, (10, 0, 19045)));
        assert!(os_matches("Windows 10, version 21H2 and later, Servicing Drivers", (10, 0, 19044)));
        assert!(os_matches("Windows 7, Windows 8, Windows 8.1 and later drivers", (6, 1, 7601)));
        assert!(!os_matches("Windows Server 2016 and Later Servicing Drivers", (10, 0, 19045)));
        assert!(os_matches("Windows 10 Anniversary Update and Later Servicing Drivers", (10, 0, 17763)));
        assert!(!os_matches("Windows 10 Fall Creators Update and Later Servicing Drivers", (10, 0, 15063)));
    }

    #[test]
    fn details_and_hwids() {
        let html = r#"<span id="ScopedViewHandler_labelArchitecture_Separator" class="labelTitle">Architecture:</span>
            AMD64
        </div><span id="ScopedViewHandler_labelSupportedHWIDs" class="boxTitleDetails">Supported Hardware IDs</span>
        <div class="tabDriverPackageContentBox" id="driverhwIDs" TABINDEX="1">
            <div style="padding-bottom: 0.3em;">
                hdaudio\func_01&amp;ven_10ec&amp;dev_0887
            </div>
        </div><span class="boxTitleDetails">Next</span>"#;
        let (arch, ids) = parse_details(html);
        assert_eq!(arch, "AMD64");
        assert_eq!(ids, vec![r"hdaudio\func_01&ven_10ec&dev_0887".to_string()]);
        let dev = device_hwid(r"HDAUDIO\FUNC_01&VEN_10EC&DEV_0887&SUBSYS_1043892A&REV_1003\4&2&0&0001");
        assert!(hwid_covered(&dev, &ids[0]));
        assert!(!hwid_covered(&dev, r"hdaudio\func_01&ven_10ec&dev_088"));
        assert!(!hwid_covered(&dev, r"hdaudio\func_01&ven_10ec&dev_0897"));
    }

    #[test]
    fn search_queries() {
        assert_eq!(
            queries(r"PCI\VEN_10EC&DEV_8168&SUBSYS_86771043&REV_15\4&1A2B3C&0&00E4"),
            vec![r"PCI\VEN_10EC&DEV_8168&SUBSYS_86771043".to_string(), r"PCI\VEN_10EC&DEV_8168".to_string()]
        );
    }
}
