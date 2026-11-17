//! App installer: popular free programs in one go.
//!
//! Windows 10 / 11: `winget` (Microsoft's package manager - always the current version from
//! the publisher). Without winget (Windows 7 / 8, LTSC) or when winget fails: the official
//! direct download, installed silently. Every downloaded installer must carry a VALID
//! digital signature before it is started (WinTool runs as admin).

use crate::report::Report;
use crate::scan::signatures;
use crate::ui::{self, Kind};
use crate::util::*;
use crate::{drivers, online};
use regex_lite::Regex;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where a direct download comes from.
#[derive(Clone, Copy)]
enum Src {
    Url(&'static str),
    /// Page + regex of the installer link (relative to `base`); the highest version wins.
    Scrape { page: &'static str, re: &'static str, base: &'static str },
}

#[derive(Clone, Copy)]
enum Setup {
    /// .exe with silent switches.
    Exe(&'static str),
    Msi,
    /// DirectX June 2010: unpack, then DXSETUP /silent.
    DirectX,
}

#[derive(Clone, Copy)]
struct Direct {
    x64: Src,
    x86: Option<Src>,
    setup: Setup,
    /// Also for Windows 7 / 8.
    win7: bool,
    /// Windows 7 / 8 need another (older, still supported) build.
    legacy: Option<(Src, Src)>,
}

struct App {
    name: &'static str,
    group: &'static str,
    winget: &'static str,
    /// winget source "msstore" (Microsoft Store apps).
    store: bool,
    direct: Option<Direct>,
    basic: bool,
}

const fn exe(x64: Src, x86: Option<Src>, args: &'static str, win7: bool) -> Option<Direct> {
    Some(Direct { x64, x86, setup: Setup::Exe(args), win7, legacy: None })
}

const APPS: &[App] = &[
    App {
        name: "Google Chrome",
        group: "Browsers",
        winget: "Google.Chrome",
        store: false,
        direct: Some(Direct {
            x64: Src::Url("https://dl.google.com/chrome/install/GoogleChromeStandaloneEnterprise64.msi"),
            x86: Some(Src::Url("https://dl.google.com/chrome/install/GoogleChromeStandaloneEnterprise.msi")),
            setup: Setup::Msi,
            win7: false,
            legacy: None,
        }),
        basic: true,
    },
    App {
        name: "Mozilla Firefox",
        group: "Browsers",
        winget: "Mozilla.Firefox",
        store: false,
        direct: Some(Direct {
            x64: Src::Url("https://download.mozilla.org/?product=firefox-latest-ssl&os=win64&lang=en-US"),
            x86: Some(Src::Url("https://download.mozilla.org/?product=firefox-latest-ssl&os=win&lang=en-US")),
            setup: Setup::Exe("/S"),
            win7: true,
            // Firefox ESR 115: the last line that runs on Windows 7 / 8.
            legacy: Some((
                Src::Url("https://download.mozilla.org/?product=firefox-esr115-latest-ssl&os=win64&lang=en-US"),
                Src::Url("https://download.mozilla.org/?product=firefox-esr115-latest-ssl&os=win&lang=en-US"),
            )),
        }),
        basic: false,
    },
    App { name: "Microsoft Edge", group: "Browsers", winget: "Microsoft.Edge", store: false, direct: None, basic: false },
    App {
        name: "7-Zip (archives: zip, rar, 7z)",
        group: "Tools",
        winget: "7zip.7zip",
        store: false,
        direct: exe(
            Src::Scrape { page: "https://www.7-zip.org/download.html", re: r"a/7z\d+-x64\.exe", base: "https://www.7-zip.org/" },
            Some(Src::Scrape { page: "https://www.7-zip.org/download.html", re: r"a/7z\d+\.exe", base: "https://www.7-zip.org/" }),
            "/S",
            true,
        ),
        basic: true,
    },
    App {
        name: "VLC media player",
        group: "Tools",
        winget: "VideoLAN.VLC",
        store: false,
        direct: exe(
            Src::Scrape { page: "https://get.videolan.org/vlc/last/win64/", re: r"vlc-[\d.]+-win64\.exe", base: "https://get.videolan.org/vlc/last/win64/" },
            Some(Src::Scrape { page: "https://get.videolan.org/vlc/last/win32/", re: r"vlc-[\d.]+-win32\.exe", base: "https://get.videolan.org/vlc/last/win32/" }),
            "/L=1033 /S",
            true,
        ),
        basic: true,
    },
    App { name: "Adobe Acrobat Reader (PDF)", group: "Tools", winget: "Adobe.Acrobat.Reader.64-bit", store: false, direct: None, basic: false },
    App { name: "Notepad++", group: "Tools", winget: "Notepad++.Notepad++", store: false, direct: None, basic: false },
    App { name: "LibreOffice (Word / Excel files)", group: "Tools", winget: "TheDocumentFoundation.LibreOffice", store: false, direct: None, basic: false },
    App { name: "K-Lite Codec Pack (all video formats)", group: "Tools", winget: "CodecGuide.K-LiteCodecPack.Standard", store: false, direct: None, basic: false },
    App { name: "Everything (instant file search)", group: "Tools", winget: "voidtools.Everything", store: false, direct: None, basic: false },
    App { name: "qBittorrent", group: "Tools", winget: "qBittorrent.qBittorrent", store: false, direct: None, basic: false },
    App { name: "OBS Studio (screen recording)", group: "Tools", winget: "OBSProject.OBSStudio", store: false, direct: None, basic: false },
    App { name: "CPU-Z (hardware info)", group: "Tools", winget: "CPUID.CPU-Z", store: false, direct: None, basic: false },
    App { name: "CrystalDiskInfo (disk health)", group: "Tools", winget: "CrystalDewWorld.CrystalDiskInfo", store: false, direct: None, basic: false },
    App {
        name: "Telegram Desktop",
        group: "Messengers & remote help",
        winget: "Telegram.TelegramDesktop",
        store: false,
        direct: exe(Src::Url("https://telegram.org/dl/desktop/win64"), Some(Src::Url("https://telegram.org/dl/desktop/win")), "/VERYSILENT /NORESTART", false),
        basic: false,
    },
    App { name: "WhatsApp", group: "Messengers & remote help", winget: "9NKSQGP7F2NH", store: true, direct: None, basic: false },
    App {
        name: "Zoom",
        group: "Messengers & remote help",
        winget: "Zoom.Zoom",
        store: false,
        direct: Some(Direct {
            x64: Src::Url("https://zoom.us/client/latest/ZoomInstallerFull.msi?archType=x64"),
            x86: Some(Src::Url("https://zoom.us/client/latest/ZoomInstallerFull.msi")),
            setup: Setup::Msi,
            win7: false,
            legacy: None,
        }),
        basic: false,
    },
    App { name: "Discord", group: "Messengers & remote help", winget: "Discord.Discord", store: false, direct: None, basic: false },
    App {
        name: "AnyDesk (remote help)",
        group: "Messengers & remote help",
        winget: "AnyDesk.AnyDesk",
        store: false,
        direct: exe(
            Src::Url("https://download.anydesk.com/AnyDesk.exe"),
            None,
            r#"--install "C:\Program Files (x86)\AnyDesk" --start-with-win --create-shortcuts --create-desktop-icon --silent"#,
            false,
        ),
        basic: false,
    },
    App {
        name: "TeamViewer (remote help)",
        group: "Messengers & remote help",
        winget: "TeamViewer.TeamViewer",
        store: false,
        direct: exe(
            Src::Url("https://download.teamviewer.com/download/TeamViewer_Setup_x64.exe"),
            Some(Src::Url("https://download.teamviewer.com/download/TeamViewer_Setup.exe")),
            "/S",
            false,
        ),
        basic: false,
    },
    App {
        name: "Steam (games)",
        group: "Games & runtimes",
        winget: "Valve.Steam",
        store: false,
        direct: exe(Src::Url("https://cdn.cloudflare.steamstatic.com/client/installer/SteamSetup.exe"), None, "/S", false),
        basic: false,
    },
    App {
        name: "Visual C++ 2015-2022 runtime (64-bit)",
        group: "Games & runtimes",
        winget: "Microsoft.VCRedist.2015+.x64",
        store: false,
        direct: exe(Src::Url("https://aka.ms/vs/17/release/vc_redist.x64.exe"), None, "/install /quiet /norestart", true),
        basic: true,
    },
    App {
        name: "Visual C++ 2015-2022 runtime (32-bit)",
        group: "Games & runtimes",
        winget: "Microsoft.VCRedist.2015+.x86",
        store: false,
        direct: exe(
            Src::Url("https://aka.ms/vs/17/release/vc_redist.x86.exe"),
            Some(Src::Url("https://aka.ms/vs/17/release/vc_redist.x86.exe")),
            "/install /quiet /norestart",
            true,
        ),
        basic: true,
    },
    App {
        name: ".NET Framework 4.8",
        group: "Games & runtimes",
        winget: "Microsoft.DotNet.Framework.DeveloperPack_4",
        store: false,
        direct: exe(
            Src::Url("https://go.microsoft.com/fwlink/?linkid=2088631"),
            Some(Src::Url("https://go.microsoft.com/fwlink/?linkid=2088631")),
            "/q /norestart",
            true,
        ),
        basic: true,
    },
    App {
        name: "DirectX 9 runtime (older games)",
        group: "Games & runtimes",
        winget: "Microsoft.DirectX",
        store: false,
        direct: Some(Direct {
            x64: Src::Url("https://download.microsoft.com/download/8/4/A/84A35BF1-DAFE-4AE8-82AF-AD2AE20B6B14/directx_Jun2010_redist.exe"),
            x86: Some(Src::Url("https://download.microsoft.com/download/8/4/A/84A35BF1-DAFE-4AE8-82AF-AD2AE20B6B14/directx_Jun2010_redist.exe")),
            setup: Setup::DirectX,
            win7: true,
            legacy: None,
        }),
        basic: true,
    },
    App { name: "Microsoft PowerToys", group: "Games & runtimes", winget: "Microsoft.PowerToys", store: false, direct: None, basic: false },
];

// ---------------------------------------------------------------- environment

/// winget.exe of the account WinTool runs as (an App Execution Alias), if it works.
fn winget() -> Option<PathBuf> {
    if win_version() < (10, 0, 17763) {
        return None;
    }
    let p = env_path("LOCALAPPDATA", r"C:\Users\Default\AppData\Local").join(r"Microsoft\WindowsApps\winget.exe");
    let ok = Command::new(&p).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    ok.then_some(p)
}

/// Windows 7 / 8.x.
fn legacy_os() -> bool {
    win_version() < (10, 0, 0)
}

/// Can this app be installed here (winget available / direct download for this Windows)?
fn available(a: &App, has_winget: bool) -> bool {
    has_winget || a.direct.is_some_and(|d| !legacy_os() || d.win7)
}

// ---------------------------------------------------------------- pure helpers

/// Numbers in a version-ish string, for "highest version wins".
fn version_key(s: &str) -> Vec<u64> {
    s.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect()
}

/// Highest-version match of `re` in `html`.
fn best_link(html: &str, re: &str) -> Option<String> {
    let re = Regex::new(re).ok()?;
    re.find_iter(html).map(|m| m.as_str().to_string()).max_by_key(|m| version_key(m))
}

/// "1 3 5", "1,3-5", "b" (basic set) → app indexes (0-based), only available ones.
fn parse_choice(input: &str, avail: &[bool], basic: &[bool]) -> Vec<usize> {
    let mut out: Vec<usize> = vec![];
    for part in input.split([' ', ',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        if part.eq_ignore_ascii_case("b") {
            out.extend((0..avail.len()).filter(|&i| basic[i] && avail[i]));
            continue;
        }
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
            for n in a.min(b)..=a.max(b) {
                if n >= 1 && n <= avail.len() && avail[n - 1] {
                    out.push(n - 1);
                }
            }
        }
    }
    let mut seen = vec![];
    out.retain(|i| {
        if seen.contains(i) {
            false
        } else {
            seen.push(*i);
            true
        }
    });
    out
}

/// winget exit codes that mean "nothing to do - it is already installed".
fn winget_already(code: i32) -> bool {
    matches!(code as u32, 0x8A15002B | 0x8A150061 | 0x8A15010C)
}

// ---------------------------------------------------------------- install

fn work_dir() -> PathBuf {
    std::env::temp_dir().join("WinTool-apps")
}

fn resolve(src: Src) -> Option<String> {
    match src {
        Src::Url(u) => Some(u.to_string()),
        Src::Scrape { page, re, base } => {
            let f = work_dir().join("page.html");
            if !online::download(page, &f) {
                return None;
            }
            let html = fs::read_to_string(&f).unwrap_or_default();
            let _ = fs::remove_file(&f);
            best_link(&html, re).map(|l| format!("{base}{l}"))
        }
    }
}

fn run_ok(c: &mut Command) -> Result<(), String> {
    match c.status() {
        // 3010 / 1641: installed, restart needed.
        Ok(s) if s.success() || matches!(s.code(), Some(3010) | Some(1641)) => Ok(()),
        Ok(s) => Err(format!("installer exit code {}", s.code().unwrap_or(-1))),
        Err(e) => Err(e.to_string()),
    }
}

fn install_direct(a: &App, d: &Direct) -> Result<(), String> {
    let (s64, s86) = match d.legacy {
        Some((l64, l86)) if legacy_os() => (l64, Some(l86)),
        _ => (d.x64, d.x86),
    };
    let src = if drivers::os_is_64bit() { s64 } else { s86.ok_or("no 32-bit version")? };
    let _ = fs::create_dir_all(work_dir());
    let url = resolve(src).ok_or("download page not reachable")?;
    let ext = if matches!(d.setup, Setup::Msi) { "msi" } else { "exe" };
    let file = work_dir().join(format!("{}.{ext}", a.winget.replace(['.', '+'], "_")));
    if !online::download(&url, &file) {
        return Err("download failed".into());
    }
    // Admin rights: only a validly signed installer is started.
    let path = file.to_string_lossy().to_string();
    let sig = signatures(std::slice::from_ref(&path));
    match sig.get(&path.to_lowercase()).map(|(s, _)| s.as_str()) {
        Some("Valid") => {}
        other => {
            let _ = fs::remove_file(&file);
            return Err(format!("installer signature is {} - not started", other.unwrap_or("unknown")));
        }
    }
    let result = match d.setup {
        Setup::Exe(args) => run_ok(Command::new(&file).raw_arg(args)),
        Setup::Msi => run_ok(Command::new(sys("msiexec")).arg("/i").arg(&file).args(["/qn", "/norestart"])),
        Setup::DirectX => {
            let dir = work_dir().join("directx");
            let _ = fs::create_dir_all(&dir);
            run_ok(Command::new(&file).arg("/Q").raw_arg(format!("/T:\"{}\"", dir.display())))
                .and_then(|_| run_ok(Command::new(dir.join("DXSETUP.exe")).arg("/silent")))
        }
    };
    let _ = fs::remove_file(&file);
    result
}

/// Ok(true) = it was already installed.
fn install_winget(wg: &Path, a: &App) -> Result<bool, String> {
    let mut c = Command::new(wg);
    c.args(["install", "--id", a.winget, "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements"]);
    c.args(["--source", if a.store { "msstore" } else { "winget" }]);
    match c.status() {
        Ok(s) if s.success() => Ok(false),
        Ok(s) if s.code().is_some_and(winget_already) => Ok(true),
        Ok(s) => Err(format!("winget error 0x{:08X}", s.code().unwrap_or(-1) as u32)),
        Err(e) => Err(e.to_string()),
    }
}

pub fn menu() {
    let wg = ui::spin("Checking winget", winget);
    let avail: Vec<bool> = APPS.iter().map(|a| available(a, wg.is_some())).collect();
    let basic: Vec<bool> = APPS.iter().map(|a| a.basic).collect();
    println!();
    match &wg {
        Some(_) => ui::info("winget found: programs come straight from their publishers, always the newest version."),
        None => ui::info("No winget: official direct downloads are used (only programs that support this Windows)."),
    }
    let mut group = "";
    for (i, a) in APPS.iter().enumerate() {
        if a.group != group {
            group = a.group;
            println!("\n  {group}");
        }
        let mark = if a.basic { "*" } else { " " };
        if avail[i] {
            println!("   {:>2}.{mark} {}", i + 1, a.name);
        } else {
            println!("   --   {}  (not available for this Windows)", a.name);
        }
    }
    println!("\n  * = basic set (b)");
    let input = prompt("\n  Numbers to install (e.g. 1 4 5, 2-6, b = basic set, Enter = back): ");
    let chosen = parse_choice(&input, &avail, &basic);
    if chosen.is_empty() {
        return;
    }
    ui::header("App installer");
    if !internet_ok() {
        ui::fail("No internet connection - run NO INTERNET repair first.");
        pause();
        return;
    }
    let mut r = Report::new();
    r.note(format!("WinTool {} - app installer report  (by {}, {})", ui::VERSION, ui::AUTHOR, ui::SITE));
    r.put(Kind::Plain, format!("  Date:   {}", now_str()));
    r.put(Kind::Plain, format!("  System: {}", version_str()));
    let total = chosen.len();
    let mut ok = 0;
    for (n, &i) in chosen.iter().enumerate() {
        let a = &APPS[i];
        r.step(&format!("[{}/{total}] {}", n + 1, a.name));
        let direct = |a: &App, d: &Direct| ui::spin("Downloading and installing", || install_direct(a, d)).map(|_| "installed (official download)");
        let result = match wg.as_ref().map(|w| ui::spin("Installing with winget", || install_winget(w, a))) {
            Some(Ok(true)) => Ok("already installed"),
            Some(Ok(false)) => Ok("installed (winget)"),
            Some(Err(e)) => match &a.direct {
                Some(d) => {
                    r.put(Kind::Warn, format!("   winget failed ({e}) - trying the official download"));
                    direct(a, d)
                }
                None => Err(e),
            },
            None => match &a.direct {
                Some(d) => direct(a, d),
                None => Err("needs winget".to_string()),
            },
        };
        match result {
            Ok(how) => {
                r.fix(format!("{}: {how}", a.name));
                ok += 1;
            }
            Err(e) => {
                r.put(Kind::Fail, format!("{}: NOT installed - {e}", a.name));
                r.manual(format!("{}: install it from the publisher's website.", a.name));
            }
        }
    }
    let _ = fs::remove_dir_all(work_dir());
    ui::result(ok == total, &format!("{ok} of {total} program(s) installed."));
    r.print_steps();
    r.save("WinTool-apps-report.txt");
    pause();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices() {
        let avail = [true, true, false, true, true];
        let basic = [true, false, true, true, false];
        assert_eq!(parse_choice("1 2", &avail, &basic), vec![0, 1]);
        assert_eq!(parse_choice("2-5", &avail, &basic), vec![1, 3, 4]);
        assert_eq!(parse_choice("b, 5, 1", &avail, &basic), vec![0, 3, 4]);
        assert_eq!(parse_choice("9 x 0", &avail, &basic), Vec::<usize>::new());
        assert_eq!(parse_choice("", &avail, &basic), Vec::<usize>::new());
    }

    #[test]
    fn links() {
        let page = r#"<a href="a/7z2301-x64.exe">x</a> <a href="a/7z2501-x64.exe">y</a> <a href="a/7z1900-x64.exe">z</a> <a href="a/7z2501.exe">w</a>"#;
        assert_eq!(best_link(page, r"a/7z\d+-x64\.exe").as_deref(), Some("a/7z2501-x64.exe"));
        assert_eq!(best_link(page, r"a/7z\d+\.exe").as_deref(), Some("a/7z2501.exe"));
        let vlc = r#"<a href="vlc-3.0.24-win64.exe">a</a> <a href="vlc-3.0.24-win64.exe.sha256">b</a>"#;
        assert_eq!(best_link(vlc, r"vlc-[\d.]+-win64\.exe").as_deref(), Some("vlc-3.0.24-win64.exe"));
        assert!(winget_already(0x8A15002Bu32 as i32));
        assert!(!winget_already(1));
    }

    #[test]
    fn catalog_is_sane() {
        for a in APPS {
            assert!(!a.winget.is_empty() && !a.name.is_empty(), "{}", a.name);
            if a.basic {
                assert!(a.direct.is_some(), "basic app {} needs a direct download", a.name);
            }
        }
    }
}
