//! System repair: SFC, DISM, Windows Update reset, Print Spooler, icon cache, disk check, drivers.

use crate::drivers;
use crate::safety;
use crate::ui;
use crate::util::*;
use std::fs;
use std::path::Path;

const ITEMS: [&str; 8] = [
    "Check and repair system files (SFC)",
    "Repair the Windows image (DISM) - Win8+",
    "FULL repair (DISM + SFC)",
    "Reset Windows Update (stuck updates)",
    "Reset the print queue (Print Spooler)",
    "Rebuild icon & thumbnail cache",
    "Check disk (chkdsk)",
    "Find & install missing drivers (all devices)",
];

pub fn menu() {
    while let Some(i) = ui::menu("System repair", &ITEMS, "Back", false) {
        ui::header(ITEMS[i]);
        match i {
            0 => sfc(),
            1 => dism(),
            2 => {
                dism();
                sfc();
            }
            3 => reset_windows_update(),
            4 => reset_spooler(),
            5 => rebuild_icon_cache(),
            6 => check_disk(),
            _ => drivers::check_all(),
        }
        pause();
    }
}

fn sfc() {
    println!("\nSFC checks every system file (10-30 minutes). Do not close the window.");
    run("sfc /scannow", "sfc", &["/scannow"]);
}

fn dism() {
    if is_win7_or_older() {
        println!("\nWindows 7 has no DISM /RestoreHealth.");
        println!("Use Microsoft's 'System Update Readiness Tool' (KB947821) instead.");
        println!("SFC (option 1) works normally on Windows 7.");
        return;
    }
    println!("\nDISM repairs the Windows image from Windows Update (needs internet, 10-30 min).");
    run("DISM RestoreHealth", "Dism.exe", &["/Online", "/Cleanup-Image", "/RestoreHealth"]);
}

/// Renames a folder to `<name>.old`, deleting a previous `.old` first.
fn rename_to_old(p: &Path) -> bool {
    if !p.exists() {
        return true;
    }
    let old = p.with_extension("old");
    if old.exists() {
        let _ = fs::remove_dir_all(&old);
    }
    fs::rename(p, &old).is_ok()
}

fn reset_windows_update() {
    println!("\nStops the update services, renews their cache folders and starts them again.");
    if !confirm("Continue?") {
        return;
    }
    safety::checkpoint();
    let services = ["wuauserv", "cryptSvc", "bits", "msiserver"];
    for s in services {
        println!("  stopping service: {s}");
        cmd_quiet(&format!("net stop {s} /y"));
    }
    let win = windir();
    for (name, p) in [
        ("SoftwareDistribution", win.join("SoftwareDistribution")),
        ("catroot2", win.join(r"System32\catroot2")),
    ] {
        let ok = rename_to_old(&p);
        println!("  {name:<22} {}", if ok { "[OK] renewed" } else { "[ERROR] in use" });
    }
    for s in services {
        println!("  starting service: {s}");
        cmd_quiet(&format!("net start {s}"));
    }
    println!("\nDone. Now click 'Check for updates' in Windows Update again.");
}

fn reset_spooler() {
    cmd("Stopping Print Spooler", "net stop spooler /y");
    let (f, k) = clear_dir(&windir().join(r"System32\spool\PRINTERS"));
    println!("\n  Stuck print jobs removed ({}){}", human(f), if k > 0 { ", some skipped" } else { "" });
    cmd("Starting Print Spooler", "net start spooler");
}

fn rebuild_icon_cache() {
    println!("\nThe desktop and taskbar disappear for a few seconds (Explorer restarts).");
    if !confirm("Continue?") {
        return;
    }
    cmd_quiet("taskkill /f /im explorer.exe");
    std::thread::sleep(std::time::Duration::from_secs(2));
    let local = env_path("LOCALAPPDATA", r"C:\Users\Default\AppData\Local");
    let icon_db = local.join("IconCache.db");
    let mut freed = fs::metadata(&icon_db).map(|m| m.len()).unwrap_or(0);
    if fs::remove_file(&icon_db).is_err() {
        freed = 0;
    }
    let exp = local.join(r"Microsoft\Windows\Explorer");
    freed += remove_matching(&exp, "iconcache_", ".db").0;
    freed += remove_matching(&exp, "thumbcache_", ".db").0;
    println!("  Cache removed: {}", human(freed));
    if restart_explorer() {
        println!("  Explorer restarted. Sign out / restart once for the full effect.");
    } else {
        println!("  Explorer did not come back: press Ctrl+Shift+Esc > File > Run new task > explorer.");
    }
}

fn explorer_running() -> bool {
    std::process::Command::new(sys("tasklist"))
        .args(["/FI", "IMAGENAME eq explorer.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase().contains("explorer.exe"))
        .unwrap_or(false)
}

/// Brings the shell back WITHOUT admin rights. Starting explorer.exe from this elevated
/// process would make the whole desktop (and everything opened from it) run as admin.
/// Windows normally restarts the shell itself as the signed-in user; if it doesn't,
/// explorer is started at the Basic User trust level.
fn restart_explorer() -> bool {
    let wait = |secs: u64| {
        (0..secs * 2).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(500));
            explorer_running()
        })
    };
    if wait(5) {
        return true;
    }
    let _ = std::process::Command::new(sys("runas"))
        .arg("/trustlevel:0x20000")
        .arg(windir().join("explorer.exe"))
        .status();
    wait(10)
}

fn check_disk() {
    let drive = env_path("SystemDrive", "C:").display().to_string();
    if is_win7_or_older() {
        run(&format!("Checking {drive} (read-only)"), "chkdsk", &[&drive]);
    } else {
        run(&format!("Online check of {drive}"), "chkdsk", &[&drive, "/scan"]);
    }
    println!("\nIf errors were found, they are fixed during a restart.");
    if confirm("Schedule a repair (chkdsk /f) for the next restart?") {
        cmd("Scheduling", &format!("echo Y| chkdsk {drive} /f"));
    }
}
