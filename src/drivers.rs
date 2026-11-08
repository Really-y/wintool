//! Driver detection, offline install from the `Drivers` folder, and download advice.
//!
//! Flow: find devices with Device Manager error codes → rescan → unpack .cab/.zip and
//! install every .inf from `Drivers\` (next to WinTool.exe) → if a device still has no
//! driver, tell the user exactly which driver is needed and where to download it.

use crate::util::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

pub const NET_CLASS: &str = "{4d36e972-e325-11ce-bfc1-08002be10318}";

pub struct ProblemDevice {
    pub name: String,
    pub hwid: String,
    pub code: u32,
    pub likely_network: bool,
}

/// Devices with a Device Manager error. `network_only` keeps network-related ones
/// (Net class, network-sounding names, or driverless unknown PCI/USB devices).
pub fn problem_devices(network_only: bool) -> Vec<ProblemDevice> {
    let out = ps(r#"$ErrorActionPreference='SilentlyContinue'
Get-WmiObject Win32_PnPEntity | Where-Object { $_.ConfigManagerErrorCode -ne 0 } | ForEach-Object {
 "{0}|{1}|{2}|{3}" -f $_.Name,$_.DeviceID,$_.ClassGuid,$_.ConfigManagerErrorCode }"#);
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split('|').collect();
            if f.len() != 4 {
                return None;
            }
            let lname = f[0].to_lowercase();
            let id = f[1].to_uppercase();
            let code: u32 = f[3].parse().unwrap_or(0);
            let net_class = f[2].eq_ignore_ascii_case(NET_CLASS);
            let net_name = ["network", "ethernet", "wireless", "wi-fi", "wifi", "wlan", "802.11", "lan "]
                .iter()
                .any(|k| lname.contains(k))
                && !lname.contains("bluetooth");
            // A driverless unknown PCI/USB device is very often the network card.
            let unknown_bus = code == 28 && f[2].is_empty() && (id.starts_with("PCI\\") || id.starts_with("USB\\"));
            let keep = !network_only || net_class || net_name || unknown_bus;
            keep.then(|| ProblemDevice {
                name: if f[0].is_empty() { "Unknown device".into() } else { f[0].to_string() },
                hwid: f[1].to_string(),
                code,
                likely_network: net_class || net_name,
            })
        })
        .collect()
}

pub fn code_meaning(code: u32) -> &'static str {
    match code {
        1 | 18 => "driver is not configured correctly",
        3 => "driver is corrupted or the system is low on memory",
        10 => "device cannot start (driver error)",
        12 => "resource conflict",
        14 => "restart required",
        19 | 39 => "driver is corrupted / registry error",
        21 | 24 | 45 => "device not present (disconnected or faulty)",
        22 => "device is disabled in Device Manager",
        28 => "NO DRIVER INSTALLED",
        31 => "driver failed to load",
        37 | 52 => "driver failed to load (signature / incompatible)",
        43 => "device reported a problem and was stopped (driver or hardware)",
        _ => "unknown error",
    }
}

/// `PCI\VEN_10EC&DEV_8168&...` → ("10EC", "8168"); USB uses VID/PID.
pub fn ven_dev(hwid: &str) -> Option<(String, String)> {
    let u = hwid.to_uppercase();
    let grab = |key: &str| u.find(key).map(|i| u[i + key.len()..].chars().take(4).collect::<String>());
    grab("VEN_").zip(grab("DEV_")).or_else(|| grab("VID_").zip(grab("PID_")))
}

/// Short hardware ID suitable for searching, e.g. `PCI\VEN_10EC&DEV_8168`.
pub fn short_hwid(hwid: &str) -> String {
    let u = hwid.to_uppercase();
    let bus = u.split('\\').next().unwrap_or("");
    match ven_dev(&u) {
        Some((v, d)) if u.contains("VID_") => format!("{bus}\\VID_{v}&PID_{d}"),
        Some((v, d)) => format!("{bus}\\VEN_{v}&DEV_{d}"),
        None => u.split('\\').take(2).collect::<Vec<_>>().join("\\"),
    }
}

pub fn vendor(ven: &str) -> &'static str {
    match ven {
        "10EC" | "0BDA" => "Realtek",
        "8086" | "8087" => "Intel",
        "168C" | "17CB" | "0CF3" => "Qualcomm Atheros",
        "1969" => "Qualcomm Atheros / Killer",
        "14E4" | "0A5C" => "Broadcom",
        "14C3" | "0E8D" => "MediaTek",
        "1814" | "148F" => "Ralink / MediaTek",
        "11AB" | "1B4B" => "Marvell",
        "10B7" => "3Com",
        "0846" => "Netgear",
        "2357" => "TP-Link",
        "0B95" => "ASIX",
        _ => "",
    }
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Human-readable block: what is missing and where to download it.
pub fn advice(d: &ProblemDevice) -> Vec<String> {
    let id = short_hwid(&d.hwid);
    let (ven, _) = ven_dev(&d.hwid).unwrap_or_default();
    let brand = vendor(&ven);
    let mut v = vec![
        format!("Device:      {}", d.name),
        format!("Problem:     code {} - {}", d.code, code_meaning(d.code)),
        format!("Hardware ID: {id}"),
    ];
    if !brand.is_empty() {
        v.push(format!("Chip vendor: {brand}"));
    }
    v.push("Where to download the driver (in this order):".into());
    v.push("  1) Your laptop / motherboard maker's support page (search your exact model) - most reliable".into());
    match brand {
        "Realtek" if !d.hwid.to_uppercase().starts_with("USB") => {
            v.push("  2) Realtek LAN drivers: https://www.realtek.com/Download/List?cate_id=584".into())
        }
        "Intel" => v.push("  2) Intel Driver & Support Assistant: https://www.intel.com/content/www/us/en/support/detect.html".into()),
        _ => {}
    }
    let n = if v.last().is_some_and(|l| l.starts_with("  2)")) { 3 } else { 2 };
    v.push(format!(
        "  {n}) Microsoft Update Catalog (official, any vendor): https://www.catalog.update.microsoft.com/Search.aspx?q={}",
        url_encode(&id)
    ));
    v.push(format!(
        "     Pick the newest entry for your Windows ({}{}) and download the .cab file.",
        if is_win7_or_older() { "Windows 7" } else { "Windows 10/11" },
        if cfg!(target_pointer_width = "64") { ", x64" } else { ", x86" }
    ));
    v.push("Then: put the .cab / .zip / .inf into the 'Drivers' folder next to WinTool.exe".into());
    v.push("      and run the automatic repair again - WinTool unpacks and installs it.".into());
    v.push("      (.exe installers: run them directly, or unpack with 7-Zip into 'Drivers'.)".into());
    v
}

// ---------------------------------------------------------------- actions

#[link(name = "cfgmgr32")]
extern "system" {
    fn CM_Locate_DevNodeW(pdnDevInst: *mut u32, pDeviceID: *const u16, ulFlags: u32) -> u32;
    fn CM_Reenumerate_DevNode(dnDevInst: u32, ulFlags: u32) -> u32;
}

/// Device Manager → "Scan for hardware changes" (whole tree, synchronous).
pub fn rescan_devices() -> bool {
    unsafe {
        let mut root = 0u32;
        if CM_Locate_DevNodeW(&mut root, std::ptr::null(), 0) != 0 {
            return false;
        }
        CM_Reenumerate_DevNode(root, 1) == 0
    }
}

pub fn drivers_dir() -> Option<PathBuf> {
    exe_dir().map(|d| d.join("Drivers"))
}

/// Unpacks .cab / .zip in `Drivers\` into `Drivers\_extracted\<name>` (once).
fn unpack_archives(dir: &Path, log: &mut dyn FnMut(String)) {
    let out_root = dir.join("_extracted");
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !p.is_file() || !(ext == "cab" || ext == "zip") {
            continue;
        }
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let out = out_root.join(&stem);
        if out.is_dir() || fs::create_dir_all(&out).is_err() {
            continue;
        }
        log(format!("   Unpacking {}", p.file_name().unwrap_or_default().to_string_lossy()));
        let ok = if ext == "cab" {
            // `expand` ships with every Windows version since XP.
            Command::new("expand").arg("-F:*").arg(&p).arg(&out).output().map(|o| o.status.success()).unwrap_or(false)
        } else {
            // Shell.Application works on Win7 (PS 2.0 has no Expand-Archive).
            ps(&format!(
                "$s=New-Object -ComObject Shell.Application; $s.Namespace({}).CopyHere($s.Namespace({}).Items(), 0x14)",
                ps_quote(&out.to_string_lossy()),
                ps_quote(&p.to_string_lossy())
            ));
            out.read_dir().map(|mut i| i.next().is_some()).unwrap_or(false)
        };
        if !ok {
            log("   (unpack failed - try 7-Zip)".into());
            let _ = fs::remove_dir_all(&out);
        }
    }
}

fn collect_inf(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 8 {
        return;
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_inf(&p, out, depth + 1);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("inf")) {
                out.push(p);
            }
        }
    }
}

/// Installs every driver found in `Drivers\` (after unpacking archives).
/// Windows itself picks the right one for each device by hardware ID.
/// Returns (drivers found, drivers accepted by pnputil).
pub fn install_from_folder(log: &mut dyn FnMut(String)) -> (usize, usize) {
    let Some(dir) = drivers_dir().filter(|d| d.is_dir()) else {
        return (0, 0);
    };
    unpack_archives(&dir, log);
    let mut infs = vec![];
    collect_inf(&dir, &mut infs, 0);
    if infs.is_empty() {
        return (0, 0);
    }
    log(format!("   'Drivers' folder: {} driver(s) found, installing...", infs.len()));
    let mut ok = 0;
    for inf in &infs {
        // `pnputil -i -a` works on Windows 7 and on 10/11 (legacy syntax still supported).
        if Command::new("pnputil").arg("-i").arg("-a").arg(inf).output().map(|o| o.status.success()).unwrap_or(false) {
            ok += 1;
        }
    }
    (infs.len(), ok)
}

// ---------------------------------------------------------------- menu item

/// "Find missing drivers" — all devices, not only network.
pub fn check_all() {
    println!("\nScanning devices...");
    let mut probs = problem_devices(false);
    if probs.is_empty() {
        println!("\nAll devices have working drivers.");
        return;
    }
    println!("\n{} device(s) with problems found. Trying to fix...", probs.len());
    if rescan_devices() {
        println!("   Hardware rescan done.");
    }
    let (found, ok) = install_from_folder(&mut |s| println!("{s}"));
    if found > 0 {
        println!("   {ok} of {found} driver(s) added from the 'Drivers' folder.");
        rescan_devices();
    }
    sleep(Duration::from_secs(5));
    probs = problem_devices(false);
    if probs.is_empty() {
        println!("\nFIXED - all devices now have working drivers.");
        return;
    }
    let mut lines = vec![format!("WinTool - missing drivers ({})", version_str()), String::new()];
    for d in &probs {
        lines.extend(advice(d));
        lines.push(String::new());
    }
    println!();
    for l in &lines {
        println!("{l}");
    }
    for p in save_text("WinTool-missing-drivers.txt", &lines) {
        println!("Saved: {}", p.display());
    }
}

/// Saves a text report to the Desktop and next to WinTool.exe (the USB stick),
/// so the download links can be opened on another computer.
pub fn save_text(name: &str, lines: &[String]) -> Vec<PathBuf> {
    let body = lines.join("\r\n") + "\r\n";
    let desktop = env_path("USERPROFILE", r"C:\Users\Public").join("Desktop");
    let mut saved = vec![];
    for dir in [Some(desktop), exe_dir()].into_iter().flatten() {
        let f = dir.join(name);
        if dir.is_dir() && fs::write(&f, &body).is_ok() {
            saved.push(f);
        }
    }
    saved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ven_dev_parses_pci_and_usb() {
        assert_eq!(ven_dev(r"PCI\VEN_10EC&DEV_8168&SUBSYS_1"), Some(("10EC".into(), "8168".into())));
        assert_eq!(ven_dev(r"USB\VID_0BDA&PID_B812\123"), Some(("0BDA".into(), "B812".into())));
        assert_eq!(ven_dev("ROOT\\NET\\0000"), None);
    }

    #[test]
    fn short_hwid_and_url() {
        assert_eq!(short_hwid(r"PCI\VEN_10EC&DEV_8168&SUBSYS_86771043&REV_15\4&1"), r"PCI\VEN_10EC&DEV_8168");
        assert_eq!(short_hwid(r"USB\VID_0BDA&PID_B812\00e04c"), r"USB\VID_0BDA&PID_B812");
        assert_eq!(url_encode(r"PCI\VEN_10EC&DEV_8168"), "PCI%5CVEN_10EC%26DEV_8168");
    }
}
