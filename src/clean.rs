//! Sistem təmizliyi: temp, Windows Update cache, xəta hesabatları, thumbnail cache, zibil qutusu.

use crate::util::*;
use std::path::PathBuf;

enum Kind {
    /// Qovluğun bütün içi.
    Dir,
    /// Qovluqda prefix/suffix-ə uyğun fayllar.
    Matching(&'static str, &'static str),
}

struct Target {
    name: &'static str,
    path: PathBuf,
    kind: Kind,
    /// Təmizlikdən əvvəl dayandırılmalı servislər.
    services: &'static [&'static str],
}

fn targets() -> Vec<Target> {
    let win = windir();
    let local = env_path("LOCALAPPDATA", r"C:\Users\Default\AppData\Local");
    let progdata = env_path("ProgramData", r"C:\ProgramData");
    let sysdrive = env_path("SystemDrive", "C:");
    vec![
        Target { name: "Istifadeci Temp", path: env_path("TEMP", r"C:\Windows\Temp"), kind: Kind::Dir, services: &[] },
        Target { name: "Windows Temp", path: win.join("Temp"), kind: Kind::Dir, services: &[] },
        Target {
            name: "Windows Update yuklemeleri",
            path: win.join(r"SoftwareDistribution\Download"),
            kind: Kind::Dir,
            services: &["wuauserv", "bits"],
        },
        Target {
            name: "Xeta hesabatlari (WER)",
            path: progdata.join(r"Microsoft\Windows\WER\ReportArchive"),
            kind: Kind::Dir,
            services: &[],
        },
        Target {
            name: "Xeta hesabatlari novbesi",
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
            name: "Zibil qutusu (butun istifadeciler)",
            path: PathBuf::from(format!(r"{}\$Recycle.Bin", sysdrive.display())),
            kind: Kind::Dir,
            services: &[],
        },
    ]
}

pub fn menu() {
    loop {
        header("Sistem temizliyi");
        println!(" 1. Tez temizlik (temp, update cache, zibil qutusu ...)");
        println!(" 2. Windows komponent temizliyi (WinSxS - kohne update-ler)");
        println!(" 3. Windows Disk Cleanup (cleanmgr) ac");
        println!(" 0. Geri");
        match prompt("\nSecim: ").as_str() {
            "1" => quick_clean(),
            "2" => component_cleanup(),
            "3" => {
                run("Disk Cleanup acilir", "cleanmgr", &[]);
            }
            "0" => return,
            _ => continue,
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
    println!("\nSkan edilir...\n");
    let sizes: Vec<u64> = ts.iter().map(size_of).collect();
    for (t, s) in ts.iter().zip(&sizes) {
        println!("  {:<38} {:>10}", t.name, human(*s));
    }
    let total: u64 = sizes.iter().sum();
    println!("  {:-<49}", "");
    println!("  {:<38} {:>10}", "CEMI", human(total));

    if total == 0 {
        println!("\nTemizlenecek bir sey yoxdur.");
        return;
    }
    if !confirm("\nHamisi silinsin?") {
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
        println!("  {:<38} {:>10} bosaldi", t.name, human(f));
        freed += f;
        skipped += k;
    }
    println!("\nCemi bosaldi: {}", human(freed));
    if skipped > 0 {
        println!("({skipped} element istifadede oldugu ucun otuldu - bu normaldir.)");
    }
}

fn component_cleanup() {
    if is_win7_or_older() {
        println!("\nWindows 7-de bu Disk Cleanup vasitesile edilir:");
        println!("acilan pencerede 'Clean up system files' -> 'Windows Update Cleanup' sec.");
        run("Disk Cleanup acilir", "cleanmgr", &[]);
        return;
    }
    println!("\nKohne update fayllari silinir (5-20 deqiqe ceke biler).");
    if confirm("Davam edilsin?") {
        run(
            "DISM komponent temizliyi",
            "Dism.exe",
            &["/Online", "/Cleanup-Image", "/StartComponentCleanup"],
        );
    }
}
