//! Shared helpers: running commands, console I/O, Windows version, folder cleanup, PowerShell.

use crate::ui;
use std::env;
use std::sync::OnceLock;
use std::fs;
use std::io::{self, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Runs a program with arguments; its output goes straight to the console.
pub fn run(desc: &str, prog: &str, args: &[&str]) -> bool {
    ui::info(desc);
    report(Command::new(prog).args(args).status())
}

/// `cmd /C <line>` — the line is passed verbatim (Rust adds no extra quoting).
pub fn cmd(desc: &str, line: &str) -> bool {
    ui::info(desc);
    report(Command::new("cmd").arg("/C").raw_arg(line).status())
}

/// Runs silently (service stop/start etc.) and only reports success.
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
            ui::ok("done");
            true
        }
        Ok(s) => {
            ui::fail(&format!("exit code {}", s.code().unwrap_or(-1)));
            false
        }
        Err(e) => {
            ui::fail(&e.to_string());
            false
        }
    }
}

/// Unpacks a .zip into `out`: `tar` (Windows 10 1803+), else Shell.Application (Windows 7),
/// waiting until every top-level item has been copied.
pub fn unzip(zip: &Path, out: &Path) -> bool {
    let _ = fs::create_dir_all(out);
    let has_items = || out.read_dir().map(|mut i| i.next().is_some()).unwrap_or(false);
    let tar = windir().join(r"System32\tar.exe");
    if tar.is_file() && Command::new(&tar).arg("-xf").arg(zip).arg("-C").arg(out).output().map(|o| o.status.success()).unwrap_or(false) {
        return has_items();
    }
    ps(&format!(
        "$s = New-Object -ComObject Shell.Application; $src = $s.Namespace({}); $dst = $s.Namespace({})
         $n = $src.Items().Count; $dst.CopyHere($src.Items(), 0x14)
         $t = 0; while (($dst.Items().Count -lt $n) -and ($t -lt 600)) {{ Start-Sleep -Milliseconds 500; $t++ }}
         Start-Sleep -Seconds 2",
        ps_quote(&zip.to_string_lossy()),
        ps_quote(&out.to_string_lossy())
    ));
    has_items()
}

/// Quick reachability test (direct TCP to public DNS anycast + name resolution).
pub fn internet_ok() -> bool {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    let tcp = ["1.1.1.1:443", "8.8.8.8:443"].iter().any(|a| {
        a.parse::<SocketAddr>()
            .map(|s| TcpStream::connect_timeout(&s, std::time::Duration::from_secs(4)).is_ok())
            .unwrap_or(false)
    });
    tcp && ("www.microsoft.com", 80).to_socket_addrs().map(|mut i| i.next().is_some()).unwrap_or(false)
}

/// Runs a PowerShell script via -EncodedCommand (no quoting issues; works with PS 2.0 on Win7).
pub fn ps(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
        .arg(base64(&utf16))
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Single-quoted PowerShell string literal.
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

pub fn prompt(msg: &str) -> String {
    print!("{msg}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    let _ = io::stdin().read_line(&mut s);
    s.trim().to_string()
}

/// Yes/no question, default "no" (used before destructive actions).
pub fn confirm(msg: &str) -> bool {
    ui::ask(msg, false)
}

pub fn pause() {
    prompt("\n  Press Enter to continue...");
}

/// Windows version (major, minor, build), detected once.
pub fn win_version() -> (u32, u32, u32) {
    static V: OnceLock<(u32, u32, u32)> = OnceLock::new();
    *V.get_or_init(detect_version)
}

/// Parses `ver` output (e.g. "Microsoft Windows [Version 6.1.7601]") into major/minor/build.
/// Only the digits inside the brackets are used, so the display language does not matter.
fn detect_version() -> (u32, u32, u32) {
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

/// Windows 7 or older (NT 6.1 and below).
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

/// `net session` only succeeds when elevated. Detected once.
pub fn is_admin() -> bool {
    static A: OnceLock<bool> = OnceLock::new();
    *A.get_or_init(|| cmd_quiet("net session"))
}

pub fn env_path(var: &str, fallback: &str) -> PathBuf {
    env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(fallback))
}

pub fn windir() -> PathBuf {
    env_path("SystemRoot", r"C:\Windows")
}

/// Folder that contains WinTool.exe (e.g. the USB stick).
pub fn exe_dir() -> Option<PathBuf> {
    env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Total size of a folder in bytes; unreadable entries are skipped.
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

/// Deletes the CONTENTS of a folder (the folder itself stays). Files in use are skipped.
/// Returns (bytes freed, number of entries that could not be removed).
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

/// Deletes files in `dir` whose lowercase name starts with `prefix` and ends with `suffix`.
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
