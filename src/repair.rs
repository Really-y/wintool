//! Sistem təmiri: SFC, DISM, Windows Update reset, Print Spooler, ikon cache, disk yoxlaması.

use crate::util::*;
use std::fs;
use std::path::Path;

pub fn menu() {
    loop {
        header("Sistem temiri");
        println!(" 1. Sistem fayllarini yoxla ve berpa et (SFC)");
        println!(" 2. Windows image berpasi (DISM RestoreHealth) - Win8+");
        println!(" 3. TAM temir (DISM + SFC)");
        println!(" 4. Windows Update-i sifirla (update ilisib qalanda)");
        println!(" 5. Printer novbesini sifirla (Print Spooler)");
        println!(" 6. Ikon ve thumbnail cache-i yenile (ikonlar qarisanda)");
        println!(" 7. Diski yoxla (chkdsk)");
        println!(" 0. Geri");
        match prompt("\nSecim: ").as_str() {
            "1" => sfc(),
            "2" => dism(),
            "3" => {
                dism();
                sfc();
            }
            "4" => reset_windows_update(),
            "5" => reset_spooler(),
            "6" => rebuild_icon_cache(),
            "7" => check_disk(),
            "0" => return,
            _ => continue,
        }
        pause();
    }
}

fn sfc() {
    println!("\nSFC butun sistem fayllarini yoxlayir (10-30 deqiqe). Pencereni baglama.");
    run("sfc /scannow", "sfc", &["/scannow"]);
}

fn dism() {
    if is_win7_or_older() {
        println!("\nWindows 7-de DISM /RestoreHealth yoxdur.");
        println!("Evezine Microsoft-un 'System Update Readiness Tool' (KB947821) istifade olunur.");
        println!("SFC (1) Win7-de normal isleyir.");
        return;
    }
    println!("\nDISM Windows image-ni Windows Update-den berpa edir (internet lazimdir, 10-30 deq).");
    run("DISM RestoreHealth", "Dism.exe", &["/Online", "/Cleanup-Image", "/RestoreHealth"]);
}

/// Qovluğu `<ad>.old`-a çevirir; köhnə `.old` varsa əvvəl silir.
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
    println!("\nUpdate servisleri dayandirilir, cache qovluqlari yenilenir, servisler yeniden baslayir.");
    if !confirm("Davam edilsin?") {
        return;
    }
    let services = ["wuauserv", "cryptSvc", "bits", "msiserver"];
    for s in services {
        println!("  servis dayandirilir: {s}");
        cmd_quiet(&format!("net stop {s} /y"));
    }
    let win = windir();
    for (name, p) in [
        ("SoftwareDistribution", win.join("SoftwareDistribution")),
        ("catroot2", win.join(r"System32\catroot2")),
    ] {
        let ok = rename_to_old(&p);
        println!("  {name:<22} {}", if ok { "[OK] yenilendi" } else { "[XETA] istifadededir" });
    }
    for s in services {
        println!("  servis baslayir: {s}");
        cmd_quiet(&format!("net start {s}"));
    }
    println!("\nHazirdir. Indi Windows Update-de yeniden 'Check for updates' et.");
}

fn reset_spooler() {
    cmd("Print Spooler dayandirilir", "net stop spooler /y");
    let (f, k) = clear_dir(&windir().join(r"System32\spool\PRINTERS"));
    println!("\n  Ilisib qalan cap isleri silindi ({}){}", human(f), if k > 0 { ", bezileri otuldu" } else { "" });
    cmd("Print Spooler baslayir", "net start spooler");
}

fn rebuild_icon_cache() {
    println!("\nMasaustu ve taskbar bir neche saniye yox olacaq (Explorer yeniden baslayir).");
    if !confirm("Davam edilsin?") {
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
    freed += remove_matching(&exp, "iconcache_", ".db");
    freed += remove_matching(&exp, "thumbcache_", ".db");
    println!("  Cache silindi: {}", human(freed));
    let _ = std::process::Command::new("explorer.exe").spawn();
    println!("  Explorer yeniden basladi. Tam netice ucun bir defe sign out / restart et.");
}

fn check_disk() {
    let drive = env_path("SystemDrive", "C:").display().to_string();
    if is_win7_or_older() {
        run(&format!("{drive} yoxlanir (yalniz oxuma)"), "chkdsk", &[&drive]);
    } else {
        run(&format!("{drive} onlayn yoxlanir"), "chkdsk", &[&drive, "/scan"]);
    }
    println!("\nXeta tapilibsa, duzeltme restart zamani edilmelidir.");
    if confirm("Novbeti restart-da duzeltme planlansin (chkdsk /f)?") {
        cmd("Planlanir", &format!("echo Y| chkdsk {drive} /f"));
    }
}
