//! System cleanup: temp files (every user), Windows Update and Delivery Optimization caches,
//! error reports, thumbnail cache, recycle bins of all fixed drives.

use crate::report::Report;
use crate::ui;
use crate::util::*;
use std::fs;
use std::path::{Path, PathBuf};

enum Kind {
    /// Everything inside the folder(s).
    Dir,
    /// Files in the folder(s) matching prefix/suffix.
    Matching(&'static str, &'static str),
    /// `X:\$Recycle.Bin`: empties every per-user bin but keeps the bins themselves
    /// (and their desktop.ini), so Windows does not have to rebuild them.
    RecycleBin,
}

struct Target {
    name: &'static str,
    paths: Vec<PathBuf>,
    kind: Kind,
    /// Services that must be stopped while cleaning (restarted only if they were running).
    services: &'static [&'static str],
}

fn targets() -> Vec<Target> {
    let win = windir();
    let local = env_path("LOCALAPPDATA", r"C:\Users\Default\AppData\Local");
    let progdata = env_path("ProgramData", r"C:\ProgramData");
    let one = |p: PathBuf| vec![p];
    vec![
        Target { name: "Temp files (all users)", paths: user_temps(), kind: Kind::Dir, services: &[] },
        Target { name: "Windows Temp", paths: one(win.join("Temp")), kind: Kind::Dir, services: &[] },
        Target {
            name: "Windows Update downloads",
            paths: one(win.join(r"SoftwareDistribution\Download")),
            kind: Kind::Dir,
            services: &["wuauserv", "bits"],
        },
        Target {
            name: "Delivery Optimization cache",
            paths: one(win.join(r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache")),
            kind: Kind::Dir,
            services: &["DoSvc"],
        },
        Target {
            name: "Error reports",
            paths: vec![
                progdata.join(r"Microsoft\Windows\WER\ReportArchive"),
                progdata.join(r"Microsoft\Windows\WER\ReportQueue"),
            ],
            kind: Kind::Dir,
            services: &[],
        },
        Target {
            name: "Thumbnail cache",
            paths: one(local.join(r"Microsoft\Windows\Explorer")),
            kind: Kind::Matching("thumbcache_", ".db"),
            services: &[],
        },
        Target { name: "Recycle Bin (all drives, all users)", paths: recycle_bins(), kind: Kind::RecycleBin, services: &[] },
    ]
}

/// %TEMP% plus `<profile>\AppData\Local\Temp` of every user profile: WinTool runs as an admin,
/// which may be a different account than the person using the PC.
fn user_temps() -> Vec<PathBuf> {
    let mut v = vec![env_path("TEMP", r"C:\Windows\Temp")];
    let users = PathBuf::from(format!(r"{}\Users", env_path("SystemDrive", "C:").display()));
    if let Ok(rd) = fs::read_dir(&users) {
        for e in rd.flatten() {
            // "All Users", "Default User" ... are junctions.
            if e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) {
                let t = e.path().join(r"AppData\Local\Temp");
                // A Temp that is a link could point anywhere: never empty its target.
                if fs::symlink_metadata(&t).map(|m| m.is_dir() && !m.file_type().is_symlink()).unwrap_or(false) {
                    v.push(t);
                }
            }
        }
    }
    // The same folder twice (different spelling of the path) would be counted twice.
    let mut seen: Vec<String> = vec![];
    v.retain(|p| {
        let key = p.to_string_lossy().trim_end_matches('\\').to_lowercase();
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    v
}

#[link(name = "kernel32")]
extern "system" {
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(root: *const u16) -> u32;
}

const DRIVE_FIXED: u32 = 3;

/// `$Recycle.Bin` of every fixed drive. Only fixed drives are probed: touching an empty card
/// reader or DVD drive can pop up "There is no disk in the drive".
fn recycle_bins() -> Vec<PathBuf> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|root| {
            let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            unsafe { GetDriveTypeW(w.as_ptr()) == DRIVE_FIXED }
        })
        .map(|root| PathBuf::from(root).join("$Recycle.Bin"))
        .filter(|p| p.is_dir())
        .collect()
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

fn matching_size(dir: &Path, p: &str, s: &str) -> u64 {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_lowercase();
                    n.starts_with(p) && n.ends_with(s)
                })
                .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0)
}

fn size_of(t: &Target) -> u64 {
    t.paths
        .iter()
        .map(|path| match t.kind {
            Kind::Dir | Kind::RecycleBin => dir_size(path),
            Kind::Matching(p, s) => matching_size(path, p, s),
        })
        .sum()
}

/// Which of `services` are running now (Get-Service status names are not localized).
fn running(services: &[&str]) -> Vec<String> {
    if services.is_empty() {
        return vec![];
    }
    let list = services.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",");
    ps(&format!(
        "Get-Service {list} -ErrorAction SilentlyContinue | Where-Object {{ $_.Status -eq 'Running' }} | ForEach-Object {{ $_.Name }}"
    ))
    .lines()
    .map(|l| l.trim().to_string())
    .filter(|l| !l.is_empty())
    .collect()
}

fn empty_bins(root: &Path) -> (u64, u32) {
    let mut r = (0, 0);
    for bin in fs::read_dir(root).into_iter().flatten().flatten() {
        if bin.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false) {
            let (f, k) = clear_dir_keep(&bin.path(), &["desktop.ini"]);
            r = (r.0 + f, r.1 + k);
        }
    }
    r
}

/// Cleans one target. Returns (bytes freed, items skipped because they are in use).
fn clean(t: &Target) -> (u64, u32) {
    let was_running = running(t.services);
    for s in t.services {
        cmd_quiet(&format!("net stop {s} /y"));
    }
    let (mut freed, mut skipped) = (0, 0);
    for path in &t.paths {
        let (f, k) = match t.kind {
            Kind::Dir => clear_dir(path),
            Kind::Matching(p, s) => remove_matching(path, p, s),
            Kind::RecycleBin => empty_bins(path),
        };
        freed += f;
        skipped += k;
    }
    // Only services that were running before: a service the user had stopped stays stopped.
    for s in &was_running {
        cmd_quiet(&format!("net start {s}"));
    }
    (freed, skipped)
}

/// Cleanup part of FULL CHECK, without questions. The Recycle Bin is emptied only when the
/// system drive is low on space - it may hold files the user still wants.
pub fn auto(r: &mut Report, include_recycle_bin: bool) -> u64 {
    let ts = ui::spin("Looking for user profiles and drives", targets);
    let mut freed = 0;
    for t in ts.iter().filter(|t| include_recycle_bin || !matches!(t.kind, Kind::RecycleBin)) {
        if size_of(t) == 0 {
            continue;
        }
        let (f, k) = ui::spin(&format!("Cleaning: {}", t.name), || clean(t));
        let skipped = if k > 0 { format!(" ({k} in use, skipped)") } else { String::new() };
        if f > 0 {
            r.fix(format!("{}: {} freed{skipped}", t.name, human(f)));
        } else {
            r.log(format!("   {}: nothing removed{skipped}", t.name));
        }
        freed += f;
    }
    freed
}

fn quick_clean() {
    let ts = ui::spin("Looking for user profiles and drives", targets);
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
    for (t, size) in ts.iter().zip(&sizes) {
        if *size == 0 {
            continue;
        }
        let (f, k) = ui::spin(&format!("Cleaning: {}", t.name), || clean(t));
        let line = format!("{:<38} {:>10} freed", t.name, human(f));
        if k > 0 {
            ui::warn(&format!("{line}  ({k} in use, skipped)"));
        } else {
            ui::ok(&line);
        }
        freed += f;
        skipped += k;
    }
    ui::result(true, &format!("Total freed: {}", human(freed)));
    if skipped > 0 {
        println!("({skipped} item(s) are in use by running programs and were left alone - this is normal.");
        println!(" Close your programs or restart the PC, then run the cleanup again to remove them too.)");
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
