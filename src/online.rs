//! Online driver install: Windows Update first, Snappy Driver Installer Origin (SDIO)
//! for the devices Windows Update has nothing for.
//!
//! Policy — system stability first:
//! - only devices with no driver or a broken one are targeted by default;
//! - Windows Update optional updates for working devices are offered separately (default: no);
//! - SDIO runs in `select missing` mode only, so it never replaces a working driver
//!   (64-bit Windows refuses unsigned kernel drivers on its own);
//! - SDIO and its driver packs live in %ProgramData%\WinTool\SDIO on the repaired PC and are
//!   deleted after a successful run; after a failed run they are kept and reported.

use crate::drivers::{self, ProblemDevice};
use crate::safety;
use crate::ui::{self, Kind};
use crate::util::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const SDIO_PAGE: &str = "https://www.glenn.delahoy.com/snappy-driver-installer-origin/";
const SDIO_DL: &str = "https://www.glenn.delahoy.com/downloads/sdio/";
const SDIO_FALLBACK_ZIP: &str = "SDIO_2.0.6.889.zip";
const SDIO_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) WinTool";

pub type Log<'a> = &'a mut dyn FnMut(Kind, String);

#[derive(Default)]
pub struct Outcome {
    /// Devices that still have no working driver.
    pub remaining: Vec<ProblemDevice>,
    pub reboot: bool,
    /// SDIO folder kept after a failed run.
    pub kept_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------- Windows Update

struct WuUpdate {
    id: String,
    title: String,
    hwid: String,
    date: String,
    size: u64,
}

const WU_SEARCH: &str = r#"try {
  $s = New-Object -ComObject Microsoft.Update.Session
  $q = $s.CreateUpdateSearcher()
  $q.ServerSelection = 2
  $q.Online = $true
  $r = $q.Search("IsInstalled=0 and Type='Driver' and IsHidden=0")
  foreach ($u in $r.Updates) {
    $d = ''; try { $d = $u.DriverVerDate.ToString('yyyy-MM-dd') } catch {}
    "U|{0}|{1}|{2}|{3}|{4}" -f $u.Identity.UpdateID, ($u.Title -replace '\|','/'), $u.DriverHardwareID, $d, $u.MaxDownloadSize
  }
  'DONE'
} catch { 'ERR|' + $_.Exception.Message }"#;

fn wu_search() -> Result<Vec<WuUpdate>, String> {
    cmd_quiet("net start wuauserv");
    let out = ps(WU_SEARCH);
    let mut ups = vec![];
    let mut done = false;
    for l in out.lines().map(str::trim) {
        if let Some(e) = l.strip_prefix("ERR|") {
            return Err(e.to_string());
        }
        if l == "DONE" {
            done = true;
        }
        if let Some(rest) = l.strip_prefix("U|") {
            let f: Vec<&str> = rest.splitn(5, '|').collect();
            if f.len() == 5 {
                ups.push(WuUpdate {
                    id: f[0].into(),
                    title: f[1].into(),
                    hwid: f[2].into(),
                    date: f[3].into(),
                    size: f[4].parse().unwrap_or(0),
                });
            }
        }
    }
    if done { Ok(ups) } else { Err("no response from the Windows Update agent".into()) }
}

/// Downloads and installs the given update IDs. Returns ((title, result code) per update, reboot needed).
fn wu_install(ids: &[String]) -> Result<(Vec<(String, u32)>, bool), String> {
    let list = ids.iter().map(|i| ps_quote(i)).collect::<Vec<_>>().join(",");
    let script = format!(
        r#"try {{
  $ids = @({list})
  $s = New-Object -ComObject Microsoft.Update.Session
  $q = $s.CreateUpdateSearcher(); $q.ServerSelection = 2; $q.Online = $true
  $r = $q.Search("IsInstalled=0 and Type='Driver' and IsHidden=0")
  $c = New-Object -ComObject Microsoft.Update.UpdateColl
  foreach ($u in $r.Updates) {{ if ($ids -contains $u.Identity.UpdateID) {{ if (-not $u.EulaAccepted) {{ $u.AcceptEula() }}; [void]$c.Add($u) }} }}
  if ($c.Count -eq 0) {{ 'ERR|the updates are no longer offered'; exit }}
  $d = $s.CreateUpdateDownloader(); $d.Updates = $c; [void]$d.Download()
  $i = $s.CreateUpdateInstaller(); $i.Updates = $c; $res = $i.Install()
  for ($k = 0; $k -lt $c.Count; $k++) {{ "R|{{0}}|{{1}}" -f ($c.Item($k).Title -replace '\|','/'), $res.GetUpdateResult($k).ResultCode }}
  "REBOOT|{{0}}" -f $res.RebootRequired
}} catch {{ 'ERR|' + $_.Exception.Message }}"#
    );
    let out = ps(&script);
    let mut results = vec![];
    let mut reboot = None;
    for l in out.lines().map(str::trim) {
        if let Some(e) = l.strip_prefix("ERR|") {
            return Err(e.to_string());
        }
        if let Some(rest) = l.strip_prefix("R|") {
            if let Some((t, c)) = rest.rsplit_once('|') {
                results.push((t.to_string(), c.parse().unwrap_or(0)));
            }
        }
        if let Some(v) = l.strip_prefix("REBOOT|") {
            reboot = Some(v.eq_ignore_ascii_case("true"));
        }
    }
    match reboot {
        Some(r) => Ok((results, r)),
        None => Err("no response from the Windows Update installer".into()),
    }
}

/// Does a Windows Update driver hardware ID belong to this device?
fn same_device(dev_hwid: &str, upd_hwid: &str) -> bool {
    if upd_hwid.trim().is_empty() {
        return false;
    }
    let d = dev_hwid.to_uppercase();
    let u = upd_hwid.to_uppercase();
    d.starts_with(&u) || (drivers::ven_dev(&u).is_some() && drivers::short_hwid(&d) == drivers::short_hwid(&u))
}

// ---------------------------------------------------------------- SDIO

fn sdio_dir() -> PathBuf {
    env_path("ProgramData", r"C:\ProgramData").join(r"WinTool\SDIO")
}

fn os_is_64bit() -> bool {
    std::env::var_os("PROCESSOR_ARCHITEW6432").is_some()
        || std::env::var("PROCESSOR_ARCHITECTURE").map(|a| a.eq_ignore_ascii_case("AMD64")).unwrap_or(false)
}

/// Newest SDIO_<arch>R<nnn>.exe in `dir` (XP builds excluded).
fn find_sdio_exe(dir: &Path) -> Option<PathBuf> {
    let prefix = if os_is_64bit() { "sdio_x64_r" } else { "sdio_r" };
    let mut best: Option<(u32, PathBuf)> = None;
    for e in fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if let Some(rest) = name.strip_prefix(prefix).and_then(|r| r.strip_suffix(".exe")) {
            if let Ok(rev) = rest.parse::<u32>() {
                if best.as_ref().is_none_or(|(b, _)| rev > *b) {
                    best = Some((rev, e.path()));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Downloads `url` to `dest`: curl.exe (Win10+), then PowerShell (TLS 1.2), then BITS.
fn download(url: &str, dest: &Path) -> bool {
    let ok = |p: &Path| fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false);
    let _ = fs::remove_file(dest);
    let curl = windir().join(r"System32\curl.exe");
    if curl.is_file() {
        let _ = Command::new(&curl).args(["-fsSL", "--retry", "2", "-A", UA, "-o"]).arg(dest).arg(url).output();
        if ok(dest) {
            return true;
        }
    }
    ps(&format!(
        "try {{ [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor 3072 }} catch {{}}
         try {{ $w = New-Object Net.WebClient; $w.Headers.Add('User-Agent', {}); $w.DownloadFile({}, {}) }} catch {{}}",
        ps_quote(UA),
        ps_quote(url),
        ps_quote(&dest.to_string_lossy())
    ));
    if ok(dest) {
        return true;
    }
    let _ = Command::new("bitsadmin")
        .args(["/transfer", "WinToolSDIO", "/download", "/priority", "FOREGROUND", url])
        .arg(dest)
        .output();
    ok(dest)
}

/// Latest SDIO zip name from the official page, or the known fallback.
fn latest_sdio_zip(tmp: &Path) -> String {
    let page = tmp.join("sdio-page.html");
    let mut best = (vec![0u32], SDIO_FALLBACK_ZIP.to_string());
    if download(SDIO_PAGE, &page) {
        let html = fs::read_to_string(&page).unwrap_or_default();
        for part in html.split("SDIO_").skip(1) {
            let Some(ver) = part.split(".zip").next() else { continue };
            let nums: Option<Vec<u32>> = ver.split('.').map(|n| n.parse().ok()).collect();
            if let Some(nums) = nums.filter(|n| n.len() == 4) {
                if nums > best.0 {
                    best = (nums, format!("SDIO_{ver}.zip"));
                }
            }
        }
        let _ = fs::remove_file(&page);
    }
    best.1
}

fn copy_dir(src: &Path, dst: &Path) -> bool {
    if fs::create_dir_all(dst).is_err() {
        return false;
    }
    let Ok(rd) = fs::read_dir(src) else { return false };
    rd.flatten().all(|e| {
        let to = dst.join(e.file_name());
        if e.path().is_dir() { copy_dir(&e.path(), &to) } else { fs::copy(e.path(), &to).is_ok() }
    })
}

/// Makes SDIO available in %ProgramData%\WinTool\SDIO. Silent (runs under a spinner).
/// Sources: already there → `SDIO` folder next to WinTool.exe → download from the official site.
fn prepare_sdio(base: &Path) -> Result<PathBuf, String> {
    if let Some(exe) = find_sdio_exe(base) {
        return Ok(exe);
    }
    fs::create_dir_all(base).map_err(|e| format!("cannot create {}: {e}", base.display()))?;
    if let Some(local) = exe_dir().map(|d| d.join("SDIO")).filter(|d| find_sdio_exe(d).is_some()) {
        if copy_dir(&local, base) {
            if let Some(exe) = find_sdio_exe(base) {
                return Ok(exe);
            }
        }
    }
    let zip_name = latest_sdio_zip(base);
    let zip = base.join(&zip_name);
    if !download(&format!("{SDIO_DL}{zip_name}"), &zip) {
        return Err(format!(
            "could not download {zip_name} (old TLS on Windows 7?). Put the extracted SDIO folder next to WinTool.exe as 'SDIO' and retry."
        ));
    }
    let unpacked = unzip(&zip, base);
    let _ = fs::remove_file(&zip);
    if !unpacked {
        return Err("could not unpack the SDIO archive".into());
    }
    find_sdio_exe(base).ok_or_else(|| "SDIO executable not found in the archive".into())
}

enum SdioRun {
    Done,
    Failed,
    Timeout,
}

fn run_sdio(exe: &Path, base: &Path) -> SdioRun {
    let b = base.display();
    let done = base.join("wintool.done");
    let failed = base.join("wintool.fail");
    let _ = fs::remove_file(&done);
    let _ = fs::remove_file(&failed);
    let script = format!(
        "verbose 384\r\nlogging on\r\nlogdir {b}\\logs\r\ndrpdir {b}\\drivers\r\nindexdir {b}\\indexes\r\nextractdir {b}\\extract\r\n\
         enableinstall on\r\ninit\r\ncheckupdates\r\nonerror goto :fail\r\nget indexes\r\nonerror goto :fail\r\n\
         init\r\ncheckupdates\r\nonerror goto :fail\r\nselect missing\r\ninstall\r\nonerror goto :fail\r\n\
         cmd echo ok> \"{}\"\r\nend\r\n:fail\r\ncmd echo fail> \"{}\"\r\nend\r\n",
        done.display(),
        failed.display()
    );
    let script_path = base.join("wintool-script.txt");
    if fs::write(&script_path, script).is_err() {
        return SdioRun::Failed;
    }
    // SDIO logs to {base}\logs; keep its console output away from WinTool's spinner.
    let Ok(mut child) = Command::new(exe)
        .arg(format!("-script:{}", script_path.display()))
        .current_dir(base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return SdioRun::Failed;
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > SDIO_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return SdioRun::Timeout;
            }
            Ok(None) => sleep(Duration::from_secs(2)),
            Err(_) => return SdioRun::Failed,
        }
    }
    if done.is_file() { SdioRun::Done } else { SdioRun::Failed }
}

fn still_broken(hwids: &[String]) -> Vec<String> {
    let now = drivers::problem_devices(false);
    hwids.iter().filter(|h| now.iter().any(|d| d.hwid.eq_ignore_ascii_case(h))).cloned().collect()
}

fn sdio_flow(log: Log, targets: &[String], out: &mut Outcome) {
    let base = sdio_dir();
    let exe = match ui::spin("Preparing Snappy Driver Installer", || prepare_sdio(&base)) {
        Ok(e) => e,
        Err(msg) => {
            log(Kind::Fail, format!("Snappy Driver Installer: {msg}"));
            return;
        }
    };
    log(Kind::Info, "Snappy Driver Installer downloads the matching driver packs (torrent) and installs them.".into());
    log(Kind::Info, "This can take 5-30 minutes; a separate SDIO console window may open.".into());
    let run = ui::spin("Snappy Driver Installer working", || run_sdio(&exe, &base));
    match run {
        SdioRun::Done => log(Kind::Ok, "Snappy Driver Installer finished.".into()),
        SdioRun::Failed => log(Kind::Fail, "Snappy Driver Installer reported an error (no internet / torrent blocked?).".into()),
        SdioRun::Timeout => log(Kind::Fail, "Snappy Driver Installer timed out after 30 min (torrent blocked by a firewall?).".into()),
    }
    let left = ui::spin("Checking devices", || {
        drivers::rescan_devices();
        sleep(Duration::from_secs(5));
        still_broken(targets)
    });
    let fixed = targets.len() - left.len();
    if fixed > 0 {
        log(Kind::Ok, format!("{fixed} device(s) got a driver from Snappy Driver Installer."));
        out.reboot = true;
    }
    if left.is_empty() && matches!(run, SdioRun::Done) {
        let _ = fs::remove_dir_all(&base);
        if let Some(parent) = base.parent() {
            let _ = fs::remove_dir(parent); // only succeeds when empty
        }
        log(Kind::Ok, "Temporary SDIO files removed.".into());
    } else if base.is_dir() {
        log(Kind::Warn, format!("SDIO files kept for a retry: {}", base.display()));
        log(Kind::Plain, format!("       delete later with:  rmdir /s /q \"{}\"", base.display()));
        out.kept_dir = Some(base);
    }
}

// ---------------------------------------------------------------- main flow

fn result_text(code: u32) -> (&'static str, Kind) {
    match code {
        2 => ("installed", Kind::Ok),
        3 => ("installed with errors", Kind::Warn),
        4 => ("FAILED", Kind::Fail),
        5 => ("aborted", Kind::Fail),
        _ => ("not installed", Kind::Fail),
    }
}

/// Finds and installs missing drivers online. `explicit` = started from the menu
/// (searches Windows Update even when every device works, to offer optional updates).
pub fn run(log: Log, explicit: bool) -> Outcome {
    let mut out = Outcome::default();
    let probs: Vec<ProblemDevice> =
        ui::spin("Scanning devices", || drivers::problem_devices(false)).into_iter().filter(|d| d.code != 22).collect();
    if probs.is_empty() {
        log(Kind::Ok, "All devices have working drivers.".into());
        if !explicit {
            return out;
        }
    } else {
        log(Kind::Info, format!("{} device(s) without a working driver:", probs.len()));
        for d in &probs {
            log(Kind::Plain, format!("       - {}  (code {}: {})", d.name, d.code, drivers::code_meaning(d.code)));
        }
    }

    // 1) Windows Update
    let (mut for_problems, mut optional) = (vec![], vec![]);
    match ui::spin("Searching Windows Update for drivers (can take a few minutes)", wu_search) {
        Ok(ups) => {
            for u in ups {
                if probs.iter().any(|d| same_device(&d.hwid, &u.hwid)) {
                    for_problems.push(u);
                } else {
                    optional.push(u);
                }
            }
        }
        Err(e) => log(Kind::Warn, format!("Windows Update search failed: {e}")),
    }

    // 2) Devices Windows Update has nothing for → SDIO (no driver at all only).
    let sdio_targets: Vec<&ProblemDevice> = probs
        .iter()
        .filter(|d| matches!(d.code, 1 | 28) && !for_problems.iter().any(|u| same_device(&d.hwid, &u.hwid)))
        .collect();

    if for_problems.is_empty() && sdio_targets.is_empty() && optional.is_empty() {
        if !probs.is_empty() {
            log(Kind::Warn, "No online driver found for the problem devices.".into());
        }
        out.remaining = probs;
        return out;
    }

    log(Kind::Plain, String::new());
    if !for_problems.is_empty() {
        log(Kind::Info, format!("Windows Update has {} driver(s) for the problem devices:", for_problems.len()));
        for u in &for_problems {
            log(Kind::Plain, format!("       - {}  ({}, {})", u.title, u.date, human(u.size)));
        }
    }
    if !sdio_targets.is_empty() {
        log(Kind::Info, format!("Snappy Driver Installer will look for drivers for {} device(s):", sdio_targets.len()));
        for d in &sdio_targets {
            log(Kind::Plain, format!("       - {}  [{}]", d.name, drivers::short_hwid(&d.hwid)));
        }
    }
    if !optional.is_empty() {
        log(Kind::Info, format!("{} optional driver update(s) for working devices (not selected).", optional.len()));
    }

    let main = (!for_problems.is_empty() || !sdio_targets.is_empty()) && ui::ask("Install the drivers listed above?", true);
    let opt = !optional.is_empty() && {
        for u in &optional {
            log(Kind::Plain, format!("       - {}  ({}, {})", u.title, u.date, human(u.size)));
        }
        ui::ask(&format!("Also install the {} optional update(s) for working devices?", optional.len()), false)
    };
    if !main && !opt {
        log(Kind::Info, "Driver installation skipped.".into());
        out.remaining = probs;
        return out;
    }
    safety::checkpoint();

    // Install from Windows Update
    let mut ids: Vec<String> = vec![];
    if main {
        ids.extend(for_problems.iter().map(|u| u.id.clone()));
    }
    if opt {
        ids.extend(optional.iter().map(|u| u.id.clone()));
    }
    if !ids.is_empty() {
        match ui::spin("Downloading and installing from Windows Update", || wu_install(&ids)) {
            Ok((results, reboot)) => {
                for (title, code) in results {
                    let (txt, kind) = result_text(code);
                    log(kind, format!("{title} - {txt}"));
                }
                out.reboot |= reboot;
            }
            Err(e) => log(Kind::Fail, format!("Windows Update install failed: {e}")),
        }
    }

    // Snappy Driver Installer for what is still missing
    if main && !sdio_targets.is_empty() {
        let targets: Vec<String> = sdio_targets.iter().map(|d| d.hwid.clone()).collect();
        let left = ui::spin("Checking devices", || {
            drivers::rescan_devices();
            sleep(Duration::from_secs(5));
            still_broken(&targets)
        });
        if left.is_empty() {
            log(Kind::Ok, "All target devices are working now - Snappy Driver Installer not needed.".into());
        } else {
            sdio_flow(log, &left, &mut out);
        }
    }

    out.remaining =
        ui::spin("Final device check", || drivers::problem_devices(false)).into_iter().filter(|d| d.code != 22).collect();
    if out.remaining.iter().any(|d| d.code == 14) {
        out.reboot = true;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_update_hwid_to_device() {
        let dev = r"PCI\VEN_10EC&DEV_8168&SUBSYS_86771043&REV_15\4&1A2B3C&0&00E4";
        assert!(same_device(dev, r"PCI\VEN_10EC&DEV_8168&SUBSYS_86771043"));
        assert!(same_device(dev, r"PCI\VEN_10EC&DEV_8168"));
        assert!(!same_device(dev, r"PCI\VEN_8086&DEV_1502"));
        assert!(!same_device(dev, ""));
    }
}
