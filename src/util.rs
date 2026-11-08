//! Ortaq köməkçilər: əmr işlətmə, giriş/çıxış, Windows versiyası, qovluq təmizliyi.
//! Konsol mətni ASCII-dir (ə→e, ş→s ...) — Win7 konsolu UTF-8-i düzgün göstərmir.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Proqramı arqumentlərlə işə salır, çıxışını birbaşa konsola verir.
pub fn run(desc: &str, prog: &str, args: &[&str]) -> bool {
    println!("\n>> {desc}");
    report(Command::new(prog).args(args).status())
}

/// `cmd /C <line>` — sətir olduğu kimi ötürülür (Rust əlavə dırnaq qoymur).
pub fn cmd(desc: &str, line: &str) -> bool {
    println!("\n>> {desc}");
    report(Command::new("cmd").arg("/C").raw_arg(line).status())
}

/// Səssiz işlədir (servis dayandırma və s. üçün) — yalnız uğuru qaytarır.
pub fn cmd_quiet(line: &str) -> bool {
    Command::new("cmd")
        .arg("/C")
        .raw_arg(line)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn report(r: io::Result<std::process::ExitStatus>) -> bool {
    match r {
        Ok(s) if s.success() => {
            println!("   [OK]");
            true
        }
        Ok(s) => {
            println!("   [XETA] cixis kodu: {}", s.code().unwrap_or(-1));
            false
        }
        Err(e) => {
            println!("   [XETA] {e}");
            false
        }
    }
}

pub fn prompt(msg: &str) -> String {
    print!("{msg}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);
    s.trim().to_string()
}

pub fn confirm(msg: &str) -> bool {
    matches!(prompt(&format!("{msg} [b/x]: ")).to_lowercase().as_str(), "b" | "y" | "beli" | "yes")
}

pub fn pause() {
    prompt("\nDavam etmek ucun Enter...");
}

pub fn header(title: &str) {
    let _ = Command::new("cmd").args(["/C", "cls"]).status();
    println!("==================================================");
    println!("  WinTool  |  {title}");
    println!("  {}  |  Admin: {}", version_str(), if is_admin() { "beli" } else { "XEYR" });
    println!("==================================================");
}

/// `ver` çıxışından (məs. "Microsoft Windows [Version 6.1.7601]") major/minor/build.
/// Dil fərqindən asılı olmamaq üçün yalnız mötərizə içindəki rəqəmlərə baxır.
pub fn win_version() -> (u32, u32, u32) {
    let out = Command::new("cmd").args(["/C", "ver"]).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let inner = text.split('[').nth(1).unwrap_or("");
    let nums: Vec<u32> = inner
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    (
        nums.first().copied().unwrap_or(0),
        nums.get(1).copied().unwrap_or(0),
        nums.get(2).copied().unwrap_or(0),
    )
}

/// Windows 7 və ya daha köhnə (NT 6.1 və aşağı).
pub fn is_win7_or_older() -> bool {
    let (ma, mi, _) = win_version();
    ma < 6 || (ma == 6 && mi <= 1)
}

pub fn version_str() -> String {
    let (ma, mi, b) = win_version();
    let name = match (ma, mi) {
        (10, _) if b >= 22000 => "Windows 11",
        (10, _) => "Windows 10",
        (6, 3) => "Windows 8.1",
        (6, 2) => "Windows 8",
        (6, 1) => "Windows 7",
        (6, 0) => "Windows Vista",
        _ => "Windows",
    };
    format!("{name} ({ma}.{mi}.{b})")
}

/// `net session` yalnız admin altında uğurlu olur.
pub fn is_admin() -> bool {
    cmd_quiet("net session")
}

pub fn env_path(var: &str, fallback: &str) -> PathBuf {
    env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(fallback))
}

pub fn windir() -> PathBuf {
    env_path("SystemRoot", r"C:\Windows")
}

/// Qovluğun ümumi ölçüsü (bayt), oxuna bilməyən yerlər ötürülür.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_symlink() => 0,
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            _ => e.metadata().map(|m| m.len()).unwrap_or(0),
        })
        .sum()
}

/// Qovluğun İÇİNİ silir (qovluğun özü qalır). İstifadədə olan fayllar ötürülür.
/// Qaytarır: (boşalan bayt, silinə bilməyən element sayı).
pub fn clear_dir(p: &Path) -> (u64, u32) {
    let before = dir_size(p);
    let mut failed = 0;
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            let is_dir = e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false);
            let ok = if is_dir { fs::remove_dir_all(&path).is_ok() } else { fs::remove_file(&path).is_ok() };
            if !ok {
                failed += 1;
            }
        }
    }
    (before.saturating_sub(dir_size(p)), failed)
}

/// Qovluqda adı `prefix` ilə başlayıb `suffix` ilə bitən faylları silir.
pub fn remove_matching(dir: &Path, prefix: &str, suffix: &str) -> u64 {
    let mut freed = 0;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.starts_with(prefix) && name.ends_with(suffix) {
                let len = e.metadata().map(|m| m.len()).unwrap_or(0);
                if fs::remove_file(e.path()).is_ok() {
                    freed += len;
                }
            }
        }
    }
    freed
}

pub fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}
