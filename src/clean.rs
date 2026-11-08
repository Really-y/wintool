//! System cleanup: temp files, Windows Update cache, error reports, thumbnail cache, recycle bin.

use crate::ui;
use crate::util::*;
use std::path::PathBuf;

enum Kind {
    /// Everything inside the folder.
    Dir,
    /// Files in the folder matching prefix/suffix.
    Matching(&'static str, &'static str),
}

struct Target {
    name: &'static str,
    path: PathBuf,
    kind: Kind,
    /// Services that must be stopped while cleaning.
    services: &'static [&'static str],
}

fn targets() -> Vec<Target> {
    let win = windir();
    let local = env_path("LOCALAPPDATA", r"C:\Users\Default\AppData\Local");
    let progdata = env_path("ProgramData", r"C:\ProgramData");
    let sysdrive = env_path("SystemDrive", "C:");
    vec![
        Target { name: "User Temp", path: env_path("TEMP", r"C:\Windows\Temp"), kind: Kind::Dir, services: &[] },
        Target { name: "Windows Temp", path: win.join("Temp"), kind: Kind::Dir, services: &[] },
        Target {
            name: "Windows Update downloads",
            path: win.join(r"SoftwareDistribution\Download"),
            kind: Kind::Dir,
            services: &["wuauserv", "bits"],
        },
        Target {
            name: "Error reports (WER archive)",
            path: progdata.join(r"Microsoft\Windows\WER\ReportArchive"),
            kind: Kind::Dir,
            services: &[],
        },
        Target {
            name: "Error reports (WER queue)",
            path: progdata.join(r"Microsoft\Windows\WER\ReportQueue"),
            kind: Kind::Dir,
            services: &[],
        },
        Target {
            name: "Thumbnail cache",
            path: local.join(r"Microsoft\Windows\Explorer"),
            kind: Kind::Matching("thumbcache_", ".db"),
            services: &[],
        },
        Target {
            name: "Recycle Bin (all users)",
            path: PathBuf::from(format!(r"{}\$Recycle.Bin", sysdrive.display())),
            kind: Kind::Dir,
            services: &[],
        },
    ]
}

const ITEMS: [&str; 3] = [
    "Quick cleanup (temp, update cache, recycle bin ...)",
    "Windows component cleanup (WinSxS - old updates)",
    "Open Windows Disk Cleanup (cleanmgr)",
];

pub fn menu() {
    while let Some(i) = ui::menu("System cleanup", &ITEMS, "Back", false) {
        ui::header(ITEMS[i]);
        match i {
            0 => quick_clean(),
            1 => component_cleanup(),
            _ => {
                run("Opening Disk Cleanup", "cleanmgr", &[]);
            }
        }
        pause();
    }
}

fn size_of(t: &Target) -> u64 {
    match t.kind {
        Kind::Dir => dir_size(&t.path),
        Kind::Matching(p, s) => std::fs::read_dir(&t.path)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| {
                        let n = e.file_name().to_string_lossy().to_lowercase();
                        n.starts_with(p) && n.ends_with(s)
                    })
                    .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0),
    }
}

fn quick_clean() {
    let ts = targets();
    let sizes: Vec<u64> = ui::spin("Scanning", || ts.iter().map(size_of).collect());
    println!();
    for (t, s) in ts.iter().zip(&sizes) {
        println!("  {:<38} {:>10}", t.name, human(*s));
    }
    let total: u64 = sizes.iter().sum();
    println!("  {:-<49}", "");
    println!("  {:<38} {:>10}", "TOTAL", human(total));

    if total == 0 {
        println!("\nNothing to clean.");
        return;
    }
    if !confirm("\nDelete all of it?") {
        return;
    }

    let mut freed = 0;
    let mut skipped = 0;
    for t in &ts {
        for s in t.services {
            cmd_quiet(&format!("net stop {s} /y"));
        }
        let (f, k) = match t.kind {
            Kind::Dir => clear_dir(&t.path),
            Kind::Matching(p, s) => (remove_matching(&t.path, p, s), 0),
        };
        for s in t.services {
            cmd_quiet(&format!("net start {s}"));
        }
        ui::ok(&format!("{:<38} {:>10} freed", t.name, human(f)));
        freed += f;
        skipped += k;
    }
    ui::result(true, &format!("Total freed: {}", human(freed)));
    if skipped > 0 {
        println!("({skipped} item(s) skipped because they are in use - this is normal.)");
    }
}

fn component_cleanup() {
    if is_win7_or_older() {
        println!("\nOn Windows 7 this is done with Disk Cleanup:");
        println!("in the window click 'Clean up system files' -> tick 'Windows Update Cleanup'.");
        run("Opening Disk Cleanup", "cleanmgr", &[]);
        return;
    }
    println!("\nRemoving superseded update files (can take 5-20 minutes).");
    if confirm("Continue?") {
        run("DISM component cleanup", "Dism.exe", &["/Online", "/Cleanup-Image", "/StartComponentCleanup"]);
    }
}
