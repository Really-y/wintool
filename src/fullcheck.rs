//! FULL CHECK: one unattended run that finds and fixes everything WinTool knows about.
//!
//! 1. one PowerShell probe collects the facts (disks, event logs, policies, hijacks,
//!    services, Defender, firewall ...), PS 2.0 compatible, numeric / non-localized values;
//! 2. Rust decides what is wrong (pure functions below, unit-tested);
//! 3. fixes run without questions (`ui::set_unattended`): a restore point first, every
//!    changed file backed up, everything written to WinTool-full-report.txt;
//! 4. hardware problems (failing disk, RAM / CPU errors, worn battery) cannot be fixed by
//!    software: red warnings, first in the report.

use crate::drivers::{self, ProblemDevice};
use crate::report::Report;
use crate::ui::{self, Kind};
use crate::util::*;
use crate::{audio, autofix, clean, online, safety};
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- probe

const PROBE: &str = r#"$ErrorActionPreference = 'SilentlyContinue'
try { [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false } catch {}
function O { $f = @(); for ($i = 1; $i -lt $args.Count; $i++) { $f += (([string]$args[$i]) -replace '[\r\n|]', ' ') }; Write-Output ([string]$args[0] + '|' + ($f -join '|')) }

# --- disks
Get-WmiObject -Namespace root\wmi -Class MSStorageDriver_FailurePredictStatus | ForEach-Object { O 'SMART' $_.InstanceName $_.PredictFailure }
if (Get-Command Get-PhysicalDisk -ErrorAction SilentlyContinue) { Get-PhysicalDisk | ForEach-Object { O 'PDISK' $_.FriendlyName $_.HealthStatus $_.MediaType } }
Get-WmiObject Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object { O 'VOL' $_.DeviceID $_.FreeSpace $_.Size }
Get-WmiObject Win32_Volume | Where-Object { $_.DirtyBitSet -and $_.DriveLetter } | ForEach-Object { O 'DIRTY' $_.DriveLetter }

# --- event logs (30 days)
$since = (Get-Date).AddDays(-30)
$ev = @(Get-WinEvent -FilterHashtable @{ LogName = 'System'; Id = @(7,11,51,52,55,98,129,153,41,1001,17,18,19,20,46,47); StartTime = $since } -ErrorAction SilentlyContinue)
$ev | Group-Object Id, ProviderName | ForEach-Object { O 'EVC' ($_.Group[0].Id) ($_.Group[0].ProviderName) ($_.Count) }
$ev | Where-Object { $_.Id -eq 1001 -and ($_.ProviderName -like '*BugCheck*' -or $_.ProviderName -like '*SystemErrorReporting*') } | ForEach-Object { O 'BSOD' ($_.TimeCreated.ToString('yyyy-MM-dd')) ($_.Properties[0].Value) }
$app = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Id = @(1000,1511,1515); StartTime = $since } -ErrorAction SilentlyContinue)
$app | Where-Object { $_.Id -ne 1000 } | Group-Object Id, ProviderName | ForEach-Object { O 'EVC' ($_.Group[0].Id) ($_.Group[0].ProviderName) ($_.Count) }
$wk = (Get-Date).AddDays(-7)
$app | Where-Object { $_.Id -eq 1000 -and $_.ProviderName -eq 'Application Error' -and $_.TimeCreated -gt $wk } | Group-Object { [string]$_.Properties[0].Value } | Sort-Object Count -Descending | Select-Object -First 5 | ForEach-Object { O 'APPCRASH' ($_.Name) ($_.Count) }

# --- battery
$full = Get-WmiObject -Namespace root\wmi -Class BatteryFullChargedCapacity | Select-Object -First 1
$des = Get-WmiObject -Namespace root\wmi -Class BatteryStaticData | Select-Object -First 1
if ($full -and $des -and $des.DesignedCapacity -gt 0) { O 'BATT' $full.FullChargedCapacity $des.DesignedCapacity }

# --- pending restart
if (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending') { O 'REBOOT' 'a Windows component update' }
if (Test-Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired') { O 'REBOOT' 'Windows Update' }

# --- services
$svc = @(SVCLIST)
Get-WmiObject Win32_Service | Where-Object { $svc -contains $_.Name } | ForEach-Object { O 'SVC' $_.Name $_.StartMode $_.State }

# --- registry: policies, hijacks, startup
$users = @(Get-ChildItem 'Registry::HKEY_USERS' | Where-Object { $_.PSChildName -match '^S-1-5-21-[0-9-]+$' } | ForEach-Object { 'HKEY_USERS\' + $_.PSChildName })
$all = @('HKEY_LOCAL_MACHINE') + $users
$pol = @{
 'Software\Microsoft\Windows\CurrentVersion\Policies\System' = 'DisableTaskMgr,DisableRegistryTools,DisableChangePassword,DisableLockWorkstation';
 'Software\Policies\Microsoft\Windows\System' = 'DisableCMD';
 'Software\Microsoft\Windows\CurrentVersion\Policies\Explorer' = 'NoRun,NoControlPanel,NoFolderOptions,NoViewOnDrive,NoDrives,NoViewContextMenu,NoFind,NoClose,NoSetFolders'
}
foreach ($h in $all) { foreach ($k in $pol.Keys) {
 $p = 'Registry::' + $h + '\' + $k; $i = Get-ItemProperty -LiteralPath $p
 if ($i) { foreach ($n in $pol[$k].Split(',')) { $v = $i.$n; if ($v -ne $null -and [string]$v -ne '0') { O 'POL' $p $n $v } } } } }
$lm = @{
 'SOFTWARE\Policies\Microsoft\Windows Defender' = 'DisableAntiSpyware,DisableAntiVirus';
 'SOFTWARE\Policies\Microsoft\Windows Defender\Real-Time Protection' = 'DisableRealtimeMonitoring,DisableBehaviorMonitoring,DisableOnAccessProtection,DisableScanOnRealtimeEnable,DisableIOAVProtection';
 'SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate' = 'DisableWindowsUpdateAccess'
}
foreach ($k in $lm.Keys) {
 $p = 'Registry::HKEY_LOCAL_MACHINE\' + $k; $i = Get-ItemProperty -LiteralPath $p
 if ($i) { foreach ($n in $lm[$k].Split(',')) { $v = $i.$n; if ($v -ne $null -and [string]$v -ne '0') { O 'POL' $p $n $v } } } }
$s = Get-ItemProperty -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
if ($s -and $s.EnableLUA -ne $null) { O 'UAC' $s.EnableLUA }
foreach ($b in @('SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options', 'SOFTWARE\Wow6432Node\Microsoft\Windows NT\CurrentVersion\Image File Execution Options')) {
 Get-ChildItem -LiteralPath ('Registry::HKEY_LOCAL_MACHINE\' + $b) | ForEach-Object {
  $d = (Get-ItemProperty -LiteralPath $_.PSPath).Debugger; if ($d) { O 'IFEO' ('Registry::' + $_.Name) ($_.PSChildName) $d } } }
$wl = 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
$w = Get-ItemProperty -LiteralPath $wl
if ($w) { O 'WINLOGON' $wl 'Shell' $w.Shell; O 'WINLOGON' $wl 'Userinit' $w.Userinit }
foreach ($h in $users) { $p = 'Registry::' + $h + '\Software\Microsoft\Windows NT\CurrentVersion\Winlogon'; $v = (Get-ItemProperty -LiteralPath $p).Shell; if ($v) { O 'WINLOGON' $p 'Shell' $v } }
foreach ($b in @('SOFTWARE\Microsoft\Windows NT\CurrentVersion\Windows', 'SOFTWARE\Wow6432Node\Microsoft\Windows NT\CurrentVersion\Windows')) {
 $p = 'Registry::HKEY_LOCAL_MACHINE\' + $b; $i = Get-ItemProperty -LiteralPath $p
 if ($i -and $i.AppInit_DLLs -and $i.LoadAppInit_DLLs -eq 1) { O 'APPINIT' $p $i.AppInit_DLLs } }
$runs = @('Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Run', 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Wow6432Node\Microsoft\Windows\CurrentVersion\Run')
foreach ($h in $users) { $runs += 'Registry::' + $h + '\Software\Microsoft\Windows\CurrentVersion\Run' }
$skip = @('PSPath', 'PSParentPath', 'PSChildName', 'PSDrive', 'PSProvider')
foreach ($p in $runs) { $i = Get-ItemProperty -LiteralPath $p; if ($i) { $i.PSObject.Properties | Where-Object { $skip -notcontains $_.Name } | ForEach-Object { O 'RUN' $p ($_.Name) ($_.Value) } } }
foreach ($h in $users) {
 $p = 'Registry::' + $h + '\Software\Microsoft\Windows\CurrentVersion\Internet Settings'; $i = Get-ItemProperty -LiteralPath $p
 if ($i) { if ($i.ProxyEnable -eq 1 -and $i.ProxyServer) { O 'PROXY' $p $i.ProxyServer }; if ($i.AutoConfigURL) { O 'PAC' $p $i.AutoConfigURL } } }
$b = (Get-ItemProperty -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Internet Settings\Connections').WinHttpSettings
if ($b -and $b.Length -gt 16) { $n = [BitConverter]::ToInt32($b, 12); if ($n -gt 0 -and (16 + $n) -le $b.Length) { O 'WINHTTP' ([Text.Encoding]::ASCII.GetString($b, 16, $n)) } }

# --- security products, Defender, firewall
Get-WmiObject -Namespace root\SecurityCenter2 -Class AntiVirusProduct | ForEach-Object { O 'AV' $_.displayName $_.productState }
Get-WmiObject -Namespace root\SecurityCenter2 -Class FirewallProduct | ForEach-Object { O 'FWP' $_.displayName $_.productState }
if (Get-Command Get-MpComputerStatus -ErrorAction SilentlyContinue) {
 $m = Get-MpComputerStatus; if ($m) { O 'DEF' $m.AntivirusEnabled $m.RealTimeProtectionEnabled $m.AntivirusSignatureAge }
 $m = Get-MpPreference
 if ($m) { foreach ($x in @($m.ExclusionPath)) { if ($x) { O 'DEFEX' 'Path' $x } }; foreach ($x in @($m.ExclusionProcess)) { if ($x) { O 'DEFEX' 'Process' $x } }; foreach ($x in @($m.ExclusionExtension)) { if ($x) { O 'DEFEX' 'Extension' $x } } } }
foreach ($f in @('DomainProfile', 'StandardProfile', 'PublicProfile')) {
 $v = (Get-ItemProperty -LiteralPath ('Registry::HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\' + $f)).EnableFirewall
 if ($v -ne $null) { O 'FW' $f $v } }

# --- memory / page file
$cs = Get-WmiObject Win32_ComputerSystem
$pu = @(Get-WmiObject Win32_PageFileUsage).Count
O 'PAGE' ($cs.AutomaticManagedPagefile) $pu ([math]::Round($cs.TotalPhysicalMemory / 1MB))
O 'DONE'
"#;

/// Services checked by FULL CHECK: (name, start type to restore when disabled). Protected
/// services (WinDefend, gpsvc, WaaSMedicSvc ...) are left out - their start type cannot be changed.
const SERVICES: &[(&str, &str)] = &[
    ("Audiosrv", "auto"),
    ("AudioEndpointBuilder", "auto"),
    ("BITS", "demand"),
    ("CryptSvc", "auto"),
    ("Dhcp", "auto"),
    ("Dnscache", "auto"),
    ("EventLog", "auto"),
    ("EventSystem", "auto"),
    ("FontCache", "auto"),
    ("LanmanServer", "auto"),
    ("LanmanWorkstation", "auto"),
    ("MpsSvc", "auto"),
    ("Netman", "demand"),
    ("netprofm", "demand"),
    ("NlaSvc", "auto"),
    ("nsi", "auto"),
    ("Power", "auto"),
    ("ProfSvc", "auto"),
    ("RpcEptMapper", "auto"),
    ("RpcSs", "auto"),
    ("Schedule", "auto"),
    ("SENS", "auto"),
    ("ShellHWDetection", "auto"),
    ("Spooler", "auto"),
    ("Themes", "auto"),
    ("TrustedInstaller", "demand"),
    ("UsoSvc", "demand"),
    ("W32Time", "demand"),
    ("Winmgmt", "auto"),
    ("WlanSvc", "auto"),
    ("WSearch", "auto"),
    ("wuauserv", "demand"),
];

/// Probe output: `KEY|field|field...` rows.
struct Facts {
    rows: Vec<(String, Vec<String>)>,
}

impl Facts {
    fn parse(out: &str) -> Self {
        let rows = out
            .lines()
            .map(|l| l.trim().trim_start_matches('\u{feff}'))
            .filter_map(|l| {
                let mut f = l.split('|');
                let key = f.next()?.trim();
                (!key.is_empty() && key.chars().all(|c| c.is_ascii_uppercase())).then(|| (key.to_string(), f.map(|s| s.trim().to_string()).collect()))
            })
            .collect();
        Self { rows }
    }
    fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a [String]> + 'a {
        self.rows.iter().filter(move |(k, _)| k == key).map(|(_, v)| v.as_slice())
    }
    fn complete(&self) -> bool {
        self.rows.iter().any(|(k, _)| k == "DONE")
    }
}

fn field(v: &[String], i: usize) -> &str {
    v.get(i).map(String::as_str).unwrap_or("")
}

fn num(v: &[String], i: usize) -> u64 {
    field(v, i).parse().unwrap_or(0)
}

// ---------------------------------------------------------------- pure analysis

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hw {
    None,
    Ram,
    Disk,
    Cpu,
}

/// Bugcheck code → (name, likely cause, hardware suspect).
fn bugcheck(code: u32) -> (&'static str, &'static str, Hw) {
    match code {
        0x0A => ("IRQL_NOT_LESS_OR_EQUAL", "a driver (often network / Wi-Fi / antivirus) or RAM", Hw::Ram),
        0x19 => ("BAD_POOL_HEADER", "a driver or RAM", Hw::Ram),
        0x1A => ("MEMORY_MANAGEMENT", "RAM", Hw::Ram),
        0x1E => ("KMODE_EXCEPTION_NOT_HANDLED", "a driver", Hw::None),
        0x24 => ("NTFS_FILE_SYSTEM", "disk / file system damage", Hw::Disk),
        0x3B => ("SYSTEM_SERVICE_EXCEPTION", "a driver (often graphics) or damaged system files", Hw::None),
        0x4E => ("PFN_LIST_CORRUPT", "RAM or a driver", Hw::Ram),
        0x50 => ("PAGE_FAULT_IN_NONPAGED_AREA", "RAM, a driver or antivirus", Hw::Ram),
        0x77 => ("KERNEL_STACK_INPAGE_ERROR", "a failing disk / cable or RAM", Hw::Disk),
        0x7A => ("KERNEL_DATA_INPAGE_ERROR", "a failing disk / cable or RAM", Hw::Disk),
        0x7E => ("SYSTEM_THREAD_EXCEPTION_NOT_HANDLED", "a driver", Hw::None),
        0x7F => ("UNEXPECTED_KERNEL_MODE_TRAP", "hardware (RAM, CPU overheating) or a driver", Hw::Ram),
        0x9F => ("DRIVER_POWER_STATE_FAILURE", "a driver during sleep / shutdown", Hw::None),
        0xC2 => ("BAD_POOL_CALLER", "a driver", Hw::None),
        0xD1 => ("DRIVER_IRQL_NOT_LESS_OR_EQUAL", "a driver (network / Wi-Fi / USB most often)", Hw::None),
        0xEF => ("CRITICAL_PROCESS_DIED", "damaged system files or a failing disk", Hw::None),
        0xF4 => ("CRITICAL_OBJECT_TERMINATION", "a failing disk or its cable", Hw::Disk),
        0x116 => ("VIDEO_TDR_FAILURE", "the graphics driver or graphics card", Hw::None),
        0x124 => ("WHEA_UNCORRECTABLE_ERROR", "hardware: CPU, RAM, overheating or overclocking", Hw::Cpu),
        0x133 => ("DPC_WATCHDOG_VIOLATION", "a storage / SSD driver or SSD firmware", Hw::None),
        0x139 => ("KERNEL_SECURITY_CHECK_FAILURE", "a driver or damaged system files", Hw::None),
        0xC000021A => ("STATUS_SYSTEM_PROCESS_TERMINATED", "damaged system files", Hw::None),
        _ => ("", "a driver or hardware", Hw::None),
    }
}

/// First `0x...` number in a bugcheck event text: "0x0000001a (0x0000000000041792, ...)".
fn parse_bugcheck(text: &str) -> Option<u32> {
    let i = text.to_lowercase().find("0x")?;
    let hex: String = text[i + 2..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    u64::from_str_radix(&hex, 16).ok().map(|v| v as u32)
}

/// Domains that malware blocks or redirects in `hosts` to stop updates and antivirus.
const PROTECTED_DOMAINS: &[&str] = &[
    "windowsupdate.com",
    "windowsupdate.microsoft.com",
    "update.microsoft.com",
    "download.microsoft.com",
    "definitionupdates.microsoft.com",
    "kaspersky.com",
    "kaspersky-labs.com",
    "eset.com",
    "avast.com",
    "avg.com",
    "bitdefender.com",
    "malwarebytes.com",
    "mcafee.com",
    "norton.com",
    "symantec.com",
    "drweb.com",
    "sophos.com",
    "trendmicro.com",
    "avira.com",
    "f-secure.com",
    "virustotal.com",
];

enum Target {
    Blocked,
    Lan,
    Public,
}

fn classify_ip(ip: &str) -> Option<Target> {
    use std::net::IpAddr;
    match ip.parse::<IpAddr>().ok()? {
        IpAddr::V4(v) if v.is_loopback() || v.is_unspecified() => Some(Target::Blocked),
        IpAddr::V4(v) if v.is_private() || v.is_link_local() => Some(Target::Lan),
        IpAddr::V6(v) if v.is_loopback() || v.is_unspecified() => Some(Target::Blocked),
        IpAddr::V6(v) if (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80 => Some(Target::Lan),
        _ => Some(Target::Public),
    }
}

/// Indexes of `hosts` lines that look like a hijack: a name sent to a PUBLIC address (phishing /
/// ad injection), or update / antivirus sites blocked or redirected. Ad-block lists
/// (0.0.0.0 / 127.0.0.1 for other names) and LAN entries (home servers, development) are left alone.
fn hosts_hijacks(text: &str) -> Vec<usize> {
    let protected = |h: &str| {
        let h = h.to_lowercase();
        PROTECTED_DOMAINS.iter().any(|d| h == *d || h.ends_with(&format!(".{d}")))
    };
    text.lines()
        .enumerate()
        .filter(|(_, l)| {
            let l = l.split('#').next().unwrap_or("");
            let mut t = l.split_whitespace();
            let (Some(ip), names) = (t.next(), t.collect::<Vec<_>>()) else { return false };
            if names.is_empty() || names.iter().all(|n| n.eq_ignore_ascii_case("localhost")) {
                return false;
            }
            match classify_ip(ip) {
                Some(Target::Public) => true,
                Some(_) => names.iter().any(|n| protected(n)),
                None => false,
            }
        })
        .map(|(i, _)| i)
        .collect()
}

/// `%VAR%` expansion with the current environment (unknown variables stay as they are).
pub(crate) fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) if b > 0 && !after[..b].contains(' ') => {
                let name = &after[..b];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => out.push_str(&rest[a..a + b + 2]),
                }
                rest = &after[b + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The file a startup command needs: the program, or for rundll32 the DLL.
/// None when it cannot be told (a bare name found through PATH, odd syntax).
pub(crate) fn startup_file(cmd: &str) -> Option<String> {
    let c = expand_env(cmd.trim());
    let lower = c.to_lowercase();
    let (exe, rest) = if let Some(q) = c.strip_prefix('"') {
        let end = q.find('"')?;
        (q[..end].to_string(), q[end + 1..].to_string())
    } else if let Some(i) = lower.find(".exe") {
        (c[..i + 4].to_string(), c[i + 4..].to_string())
    } else {
        let mut t = c.splitn(2, ' ');
        (t.next()?.to_string(), t.next().unwrap_or("").to_string())
    };
    let name = exe.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
    if name == "rundll32.exe" || name == "rundll32" {
        let dll = rest.trim().trim_start_matches('"');
        let dll = dll.split([',', '"']).next().unwrap_or("").trim();
        return dll.contains('\\').then(|| dll.to_string());
    }
    exe.contains('\\').then_some(exe)
}

/// A Debugger value that is not malware: Process Explorer replacing Task Manager, Visual Studio.
fn ifeo_legit(debugger: &str) -> bool {
    let d = debugger.to_lowercase();
    ["procexp", "vsjitdebugger", "systemexplorer"].iter().any(|x| d.contains(x))
}

/// Winlogon Userinit must be only `...\system32\userinit.exe` (trailing comma allowed).
fn userinit_ok(v: &str) -> bool {
    let parts: Vec<&str> = v.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    parts.len() == 1 && parts[0].to_lowercase().ends_with("\\system32\\userinit.exe")
}

/// Defender exclusions malware adds to hide: whole drives, Windows, Temp, script hosts, .exe ...
fn bad_exclusion(kind: &str, value: &str, windir: &str) -> bool {
    let v = value.trim().trim_end_matches('\\').to_lowercase();
    match kind {
        "Path" => {
            let drive_root = v.len() <= 2 && v.ends_with(':');
            drive_root
                || v == windir.to_lowercase().trim_end_matches('\\')
                || v.ends_with("\\windows\\system32")
                || v.ends_with("\\appdata\\local\\temp")
                || v.ends_with("\\appdata\\roaming")
                || v.ends_with(":\\users")
                || v.ends_with(":\\programdata")
                || v.contains("%temp%")
        }
        "Process" => {
            let name = v.rsplit('\\').next().unwrap_or(&v);
            ["powershell.exe", "pwsh.exe", "cmd.exe", "rundll32.exe", "regsvr32.exe", "mshta.exe", "wscript.exe", "cscript.exe", "explorer.exe", "svchost.exe"]
                .contains(&name)
        }
        "Extension" => ["exe", "dll", "ps1", "bat", "cmd", "vbs", "js", "scr", "com"].contains(&v.trim_start_matches('.')),
        _ => false,
    }
}

/// Security Center productState: bit 12 set = real-time protection on.
fn product_enabled(state: &str) -> bool {
    state.trim().parse::<u32>().map(|s| s & 0x1000 != 0).unwrap_or(false)
}

/// "host:port" of the first proxy in a proxy string ("http=h:p;https=h:p" or "h:p").
fn proxy_hostport(raw: &str) -> Option<String> {
    let first = raw.split(';').find(|s| s.starts_with("https=")).or_else(|| raw.split(';').next())?;
    let hp = first.split_once('=').map(|(_, v)| v).unwrap_or(first).trim();
    let hp = hp.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
    (!hp.is_empty()).then(|| if hp.contains(':') { hp.to_string() } else { format!("{hp}:80") })
}

/// "host:port" of an http(s) URL.
fn url_hostport(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?;
    let port = if scheme.eq_ignore_ascii_case("https") { 443 } else { 80 };
    (!host.is_empty()).then(|| if host.contains(':') { host.to_string() } else { format!("{host}:{port}") })
}

pub(crate) fn reachable(hostport: &str) -> bool {
    hostport
        .to_socket_addrs()
        .ok()
        .and_then(|mut i| i.next())
        .map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(4)).is_ok())
        .unwrap_or(false)
}

// ---------------------------------------------------------------- fix helpers

/// Runs a PowerShell fix; true when it ran without an error.
pub(crate) fn ps_ok(script: &str) -> bool {
    ps(&format!("$ErrorActionPreference = 'Stop'\ntry {{\n{script}\n'OK' }} catch {{ 'ERR|' + $_.Exception.Message }}"))
        .lines()
        .any(|l| l.trim() == "OK")
}

pub(crate) fn reg_delete(path: &str, name: &str) -> bool {
    ps_ok(&format!("Remove-ItemProperty -LiteralPath {} -Name {}", ps_quote(path), ps_quote(name)))
}

/// `value` is a PowerShell expression: a number, or an already quoted string.
fn reg_set(path: &str, name: &str, value: &str) -> bool {
    ps_ok(&format!("Set-ItemProperty -LiteralPath {} -Name {} -Value {value}", ps_quote(path), ps_quote(name)))
}

fn short_key(path: &str) -> String {
    path.trim_start_matches("Registry::")
        .replace("HKEY_LOCAL_MACHINE", "HKLM")
        .split('\\')
        .map(|p| if p.starts_with("S-1-5-21-") { "<user>" } else { p })
        .collect::<Vec<_>>()
        .join("\\")
}

/// A path as this process sees it: a 32-bit WinTool must look into System32 through Sysnative.
pub(crate) fn native_path(p: &str) -> PathBuf {
    let sys32 = format!("{}\\system32\\", windir().to_string_lossy().to_lowercase());
    if std::env::var_os("PROCESSOR_ARCHITEW6432").is_some() && p.to_lowercase().starts_with(&sys32) {
        return windir().join("Sysnative").join(&p[sys32.len()..]);
    }
    PathBuf::from(p)
}

pub(crate) fn program_files() -> PathBuf {
    // ProgramW6432 = the real (64-bit) Program Files, also for a 32-bit WinTool.
    std::env::var_os("ProgramW6432")
        .or_else(|| std::env::var_os("ProgramFiles"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
}

// ---------------------------------------------------------------- state

#[derive(Default)]
struct State {
    /// Signs of malware (hijacked settings) that were repaired.
    suspicious: u32,
    memory_suspect: bool,
    low_space: bool,
    internet: bool,
    third_party_av: Option<String>,
    third_party_fw: bool,
}

// ---------------------------------------------------------------- run

pub fn run() {
    ui::header("FULL CHECK - find & fix everything");
    println!("  Checks disks, crashes, hidden / malware settings, services, drivers, internet,");
    println!("  sound, Defender, junk files and system files - and fixes what it finds.");
    println!("  No questions are asked; this takes 20-60 minutes. You can leave the PC.");
    ui::set_unattended(true);
    let mut r = Report::new();
    r.note(format!("WinTool {} - FULL CHECK report  (by {}, {})", ui::VERSION, ui::AUTHOR, ui::SITE));
    r.put(Kind::Plain, format!("  Date:   {}", now_str()));
    r.put(Kind::Plain, format!("  System: {}", version_str()));
    let mut st = State::default();

    r.step("[1/11] Restore point");
    safety::checkpoint();

    r.step("[2/11] Collecting information");
    let list = SERVICES.iter().map(|(n, _)| format!("'{n}'")).collect::<Vec<_>>().join(",");
    let script = PROBE.replace("SVCLIST", &list);
    let facts = Facts::parse(&ui::spin("Reading disks, event logs, registry and services (up to a minute)", || ps(&script)));
    if facts.complete() {
        r.put(Kind::Ok, format!("{} facts collected", facts.rows.len()));
    } else {
        r.put(Kind::Warn, "PowerShell answered only partly - some checks are skipped.");
    }
    if let Some(p) = facts.all("PAGE").next() {
        r.put(Kind::Plain, format!("  RAM:    {:.1} GB", num(p, 2) as f64 / 1024.0));
    }

    r.step("[3/11] Disks, crashes and hardware");
    hardware_stage(&mut r, &facts, &mut st);

    r.step("[4/11] Hidden problems and malware settings");
    security_stage(&mut r, &facts, &mut st);

    r.step("[5/11] Windows services and settings");
    services_stage(&mut r, &facts);

    r.step("[6/11] Internet");
    st.internet = autofix::repair_into(&mut r);

    r.step("[7/11] Devices and drivers");
    devices_stage(&mut r, &st);

    r.step("[8/11] Sound");
    audio::quick(&mut r);

    r.step("[9/11] Virus scan (WinTool scanner + Windows Defender)");
    defender_stage(&mut r, &facts, &mut st);

    r.step("[10/11] Junk files");
    let freed = clean::auto(&mut r, st.low_space);
    r.log(format!("   Freed in total: {}", human(freed)));

    r.step("[11/11] System files (DISM + SFC, the longest part)");
    system_files_stage(&mut r, &st);

    finish(r, &st);
}

fn finish(mut r: Report, st: &State) {
    ui::set_unattended(false);
    let (ok, text) = if !r.alerts.is_empty() {
        (false, "RESULT: software problems fixed - but there are HARDWARE WARNINGS (see the top).")
    } else if r.restart_needed {
        (true, "RESULT: repairs applied - RESTART the computer to finish them.")
    } else if r.manual.is_empty() {
        (true, "RESULT: everything checked - no problems left.")
    } else {
        (true, "RESULT: repairs applied - see the remaining steps below.")
    };
    ui::result(ok, text);
    r.note("");
    r.note(text);
    r.print_steps();
    r.save("WinTool-full-report.txt");

    if st.memory_suspect && ui::ask("The RAM may be faulty. Run Windows Memory Diagnostic (the PC restarts and tests the RAM)?", true) {
        let _ = Command::new(sys("mdsched")).spawn();
        return;
    }
    if r.restart_needed && ui::ask("Restart now?", false) {
        cmd("Restarting in 10 seconds", "shutdown /r /t 10");
    }
}

// ---------------------------------------------------------------- stages

fn hardware_stage(r: &mut Report, f: &Facts, st: &mut State) {
    let mut found = false;
    // Disks
    for v in f.all("SMART") {
        if field(v, 1).eq_ignore_ascii_case("true") {
            r.alert("A disk reports it is FAILING (SMART). Copy your files to another disk NOW and replace it.");
            found = true;
        }
    }
    for v in f.all("PDISK") {
        let bad = match field(v, 1) {
            "1" | "Warning" => Some("WARNING"),
            "2" | "Unhealthy" => Some("UNHEALTHY"),
            _ => None,
        };
        if let Some(b) = bad {
            r.alert(format!("Disk \"{}\" health: {b}. Back up your files NOW; the disk may fail soon.", field(v, 0)));
            found = true;
        }
    }
    let sysdrive = env_path("SystemDrive", "C:").to_string_lossy().to_uppercase();
    for v in f.all("VOL") {
        let (free, size) = (num(v, 1), num(v, 2));
        if size == 0 {
            continue;
        }
        let pct = free * 100 / size;
        let dev = field(v, 0).to_uppercase();
        if dev == sysdrive && (pct < 10 || free < 10 * 1024 * 1024 * 1024) {
            st.low_space = true;
            r.put(Kind::Warn, format!("{dev} is almost full: {} free ({pct}%) - Windows slows down and updates fail.", human(free)));
            found = true;
        } else if pct < 5 {
            r.put(Kind::Warn, format!("{dev} is almost full: {} free ({pct}%).", human(free)));
        }
    }
    for v in f.all("DIRTY") {
        r.put(Kind::Warn, format!("Drive {} is marked as damaged - Windows repairs it at the next restart.", field(v, 0)));
        r.restart_needed = true;
        found = true;
    }

    // Event log counts
    let count = |ids: &[u64], prov: &str| -> u64 {
        f.all("EVC").filter(|v| ids.contains(&num(v, 0)) && field(v, 1).to_lowercase().contains(prov)).map(|v| num(v, 2)).sum()
    };
    let disk_err = count(&[7, 11, 51, 52, 153], "disk");
    if disk_err > 0 {
        r.alert(format!(
            "{disk_err} disk read/write error(s) in the last 30 days (bad sectors or a loose cable). Back up your files, check the disk cable or replace the disk."
        ));
        found = true;
    }
    let ntfs = count(&[55, 98], "ntfs");
    if ntfs > 0 {
        r.put(Kind::Warn, format!("{ntfs} file system error(s) in the last 30 days."));
        if cmd_quiet(&format!("echo Y| chkdsk {sysdrive} /f")) {
            r.fix(format!("Disk repair (chkdsk /f) scheduled for {sysdrive} at the next restart"));
            r.restart_needed = true;
        }
        found = true;
    }
    let resets = count(&[129], "");
    if resets >= 5 {
        r.put(Kind::Warn, format!("{resets} storage controller resets (event 129) - the PC freezes for a moment."));
        r.manual("Storage controller resets: update the chipset / storage (SATA, NVMe) driver from the PC maker and the SSD firmware from the SSD maker.");
        found = true;
    }
    let power = count(&[41], "kernel-power");
    if power >= 2 {
        r.alert(format!(
            "The PC switched off unexpectedly {power} times in 30 days (power loss, crash or overheating). Check the power supply / battery / charger, clean the fans and dust."
        ));
        found = true;
    }
    let whea_fatal = count(&[18, 20, 46], "whea");
    let whea_fixed = count(&[17, 19, 47], "whea");
    if whea_fatal > 0 {
        r.alert(format!(
            "{whea_fatal} fatal hardware error(s) (WHEA: CPU / RAM / motherboard). Remove overclocking (BIOS: load defaults), check cooling, test the RAM."
        ));
        st.memory_suspect = true;
        found = true;
    } else if whea_fixed >= 10 {
        r.alert(format!("{whea_fixed} corrected hardware errors (WHEA) - hardware is starting to fail, overheating or overclocked."));
        st.memory_suspect |= count(&[47], "whea") > 0;
        found = true;
    }
    if count(&[1511, 1515], "profile") > 0 {
        r.put(Kind::Warn, "Windows signed a user in with a TEMPORARY profile (the user profile is damaged).");
        r.manual("A user profile is damaged (temporary profile). Create a new user account and copy the files from C:\\Users\\<old name>.");
        found = true;
    }

    // Blue screens
    let mut codes: Vec<(u32, u32, String)> = vec![];
    for v in f.all("BSOD") {
        let Some(c) = parse_bugcheck(field(v, 1)) else { continue };
        match codes.iter_mut().find(|x| x.0 == c) {
            Some(x) => x.1 += 1,
            None => codes.push((c, 1, field(v, 0).to_string())),
        }
    }
    for (code, n, last) in &codes {
        let (name, cause, hw) = bugcheck(*code);
        let text = format!("Blue screen 0x{code:X} {name} x{n} (last {last}) - usually {cause}.");
        match hw {
            Hw::None => {
                r.put(Kind::Warn, text);
                r.manual(format!("Blue screen 0x{code:X}: drivers are updated in this check; if it comes back, update {cause} from the PC maker's site."));
            }
            Hw::Ram => {
                r.alert(text);
                st.memory_suspect = true;
            }
            Hw::Disk | Hw::Cpu => r.alert(text),
        }
        found = true;
    }
    let dumps = fs::read_dir(windir().join("Minidump")).map(|d| d.count()).unwrap_or(0);
    if !codes.is_empty() && dumps > 0 {
        r.log(format!("   {dumps} crash dump(s) in {}\\Minidump (for a technician: WinDbg / BlueScreenView).", windir().display()));
    }

    // Battery
    if let Some(v) = f.all("BATT").next() {
        let (full, design) = (num(v, 0), num(v, 1));
        if design > 0 && full > 0 {
            let pct = full * 100 / design;
            if pct < 50 {
                r.alert(format!("Battery is worn out: holds only {pct}% of its original charge. Replace the battery."));
                found = true;
            } else if pct < 70 {
                r.put(Kind::Warn, format!("Battery holds {pct}% of its original charge (worn)."));
            } else {
                r.log(format!("   Battery: {pct}% of original capacity - OK"));
            }
        }
    }

    // App crashes
    for v in f.all("APPCRASH") {
        let n = num(v, 1);
        if n >= 3 {
            r.put(Kind::Warn, format!("{} crashed {n} times this week.", field(v, 0)));
            r.manual(format!("{} keeps crashing ({n} times this week): update or reinstall it.", field(v, 0)));
            found = true;
        }
    }
    if !found {
        r.put(Kind::Ok, "No disk, crash or hardware problems found.");
    }
}

fn security_stage(r: &mut Report, f: &Facts, st: &mut State) {
    let before = r.fixed.len();
    st.third_party_av = f
        .all("AV")
        .find(|v| !field(v, 0).to_lowercase().contains("defender") && product_enabled(field(v, 1)))
        .map(|v| field(v, 0).to_string());
    st.third_party_fw = f.all("FWP").any(|v| product_enabled(field(v, 1)));
    if let Some(av) = &st.third_party_av {
        r.log(format!("   Antivirus: {av} (Defender settings are left to it)"));
    }

    // Policies: Task Manager / Registry Editor / Run ... blocked, Defender / Windows Update blocked
    for v in f.all("POL") {
        let (path, name) = (field(v, 0), field(v, 1));
        let defender = path.contains("Windows Defender");
        if defender && st.third_party_av.is_some() {
            continue;
        }
        let what = match name {
            "DisableTaskMgr" => "Task Manager was blocked",
            "DisableRegistryTools" => "Registry Editor was blocked",
            "DisableCMD" => "Command Prompt was blocked",
            "NoRun" => "the Run box was removed",
            "NoControlPanel" => "Control Panel was blocked",
            "NoFolderOptions" => "Folder Options were removed (hides hidden files)",
            "NoDrives" | "NoViewOnDrive" => "drives were hidden in Explorer",
            "DisableWindowsUpdateAccess" => "Windows Update was blocked",
            _ if defender => "Windows Defender protection was turned off by a policy",
            _ => "a restriction policy was set",
        };
        if reg_delete(path, name) {
            r.fix(format!("Unblocked: {what} ({name} in {})", short_key(path)));
            if name != "DisableWindowsUpdateAccess" {
                st.suspicious += 1;
            }
        } else {
            r.put(Kind::Fail, format!("Could not remove {name} in {}", short_key(path)));
        }
    }

    if f.all("UAC").next().is_some_and(|v| field(v, 0) == "0") {
        let p = r"Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
        if reg_set(p, "EnableLUA", "1") {
            r.fix("User Account Control (UAC) was off - turned back on");
            r.restart_needed = true;
            st.suspicious += 1;
        }
    }

    // Programs hijacked through Image File Execution Options
    for v in f.all("IFEO") {
        let (path, exe, dbg) = (field(v, 0), field(v, 1), field(v, 2));
        if !ifeo_legit(dbg) && reg_delete(path, "Debugger") {
            r.fix(format!("{exe} was hijacked (it started \"{dbg}\" instead) - repaired"));
            st.suspicious += 1;
        }
    }

    // Winlogon shell / userinit
    for v in f.all("WINLOGON") {
        let (path, name, val) = (field(v, 0), field(v, 1), field(v, 2));
        let machine = path.contains("HKEY_LOCAL_MACHINE");
        match (machine, name) {
            (true, "Shell") if !val.eq_ignore_ascii_case("explorer.exe") => {
                if reg_set(path, "Shell", "'explorer.exe'") {
                    r.fix(format!("Windows shell was \"{val}\" - set back to explorer.exe"));
                    r.restart_needed = true;
                    st.suspicious += 1;
                }
            }
            (true, "Userinit") if !userinit_ok(val) => {
                let good = format!("{}\\system32\\userinit.exe,", windir().display());
                if reg_set(path, "Userinit", &ps_quote(&good)) {
                    r.fix(format!("Sign-in program was \"{val}\" - set back to userinit.exe"));
                    r.restart_needed = true;
                    st.suspicious += 1;
                }
            }
            (false, "Shell") => {
                if reg_delete(path, "Shell") {
                    r.fix(format!("A per-user shell \"{val}\" replaced the desktop - removed"));
                    r.restart_needed = true;
                    st.suspicious += 1;
                }
            }
            _ => {}
        }
    }

    for v in f.all("APPINIT") {
        if reg_set(field(v, 0), "LoadAppInit_DLLs", "0") {
            r.fix(format!("DLL injected into every program (AppInit_DLLs: {}) - disabled", field(v, 1)));
            st.suspicious += 1;
        }
    }

    hosts_check(r, st);

    // Proxies
    for v in f.all("PROXY") {
        let (path, server) = (field(v, 0), field(v, 1));
        match proxy_hostport(server) {
            Some(hp) if !reachable(&hp) => {
                if reg_set(path, "ProxyEnable", "0") {
                    r.fix(format!("Dead proxy {hp} switched off (it blocked the internet)"));
                }
            }
            Some(hp) => r.log(format!("   Proxy {hp} is set and works - kept")),
            None => {}
        }
    }
    for v in f.all("PAC") {
        let (path, url) = (field(v, 0), field(v, 1));
        let dead = match url_hostport(url) {
            Some(hp) => !reachable(&hp),
            None => {
                url.to_lowercase().starts_with("file:")
                    && !Path::new(url.trim_start_matches("file:///").trim_start_matches("file://")).exists()
            }
        };
        if dead {
            if reg_delete(path, "AutoConfigURL") {
                r.fix(format!("Dead proxy script {url} removed"));
            }
        } else {
            r.log(format!("   Proxy script {url} is set - kept"));
        }
    }
    if let Some(hp) = f.all("WINHTTP").next().and_then(|v| proxy_hostport(field(v, 0))) {
        if !reachable(&hp) && cmd_quiet("netsh winhttp reset proxy") {
            r.fix(format!("Dead system proxy {hp} removed (it blocked Windows Update)"));
        }
    }

    // Defender exclusions that hide malware
    if st.third_party_av.is_none() {
        let win = windir().to_string_lossy().to_string();
        for v in f.all("DEFEX") {
            let (kind, val) = (field(v, 0), field(v, 1));
            if bad_exclusion(kind, val, &win) && ps_ok(&format!("Remove-MpPreference -Exclusion{kind} {}", ps_quote(val))) {
                r.fix(format!("Defender exclusion \"{val}\" removed (it hid that from the antivirus)"));
                st.suspicious += 1;
            }
        }
    }

    // Firewall
    let off: Vec<&str> = f.all("FW").filter(|v| field(v, 1) == "0").map(|v| field(v, 0)).collect();
    if !off.is_empty() {
        if st.third_party_fw {
            r.log("   Windows Firewall is off, another firewall is active - kept");
        } else if cmd_quiet("netsh advfirewall set allprofiles state on") {
            r.fix(format!("Windows Firewall was off ({}) - turned on", off.join(", ")));
        }
    }

    // Startup entries pointing to deleted programs
    let mut total = 0;
    for v in f.all("RUN") {
        total += 1;
        let (path, name, cmdline) = (field(v, 0), field(v, 1), field(v, 2));
        let Some(file) = startup_file(cmdline) else { continue };
        if !native_path(&file).exists() && reg_delete(path, name) {
            r.fix(format!("Broken startup entry \"{name}\" removed (its program {file} no longer exists)"));
        }
    }
    if total > 25 {
        r.put(Kind::Warn, format!("{total} programs start with Windows - startup is slow."));
        r.manual("Many programs start with Windows: Task Manager -> Startup -> disable the ones you do not need.");
    }

    if r.fixed.len() == before {
        r.put(Kind::Ok, "No hidden problems or hijacked settings found.");
    } else if st.suspicious > 0 {
        r.put(Kind::Warn, format!("{} sign(s) of malware were repaired - an antivirus scan follows.", st.suspicious));
    }
}

fn hosts_check(r: &mut Report, st: &mut State) {
    let hosts = native_path(&format!("{}\\System32\\drivers\\etc\\hosts", windir().display()));
    let Ok(text) = fs::read_to_string(&hosts) else { return };
    let bad = hosts_hijacks(&text);
    if bad.is_empty() {
        return;
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let backup = hosts.with_file_name(format!("hosts.wintool-{stamp}.bak"));
    if fs::copy(&hosts, &backup).is_err() {
        r.put(Kind::Fail, "The hosts file looks hijacked, but no backup could be made - left unchanged.");
        return;
    }
    let fixed: Vec<String> = text
        .lines()
        .enumerate()
        .map(|(i, l)| if bad.contains(&i) { format!("# [disabled by WinTool] {l}") } else { l.to_string() })
        .collect();
    if let Ok(m) = fs::metadata(&hosts) {
        let mut p = m.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = fs::set_permissions(&hosts, p);
    }
    if fs::write(&hosts, fixed.join("\r\n") + "\r\n").is_ok() {
        for i in &bad {
            r.log(format!("   hosts: {}", text.lines().nth(*i).unwrap_or("").trim()));
        }
        r.fix(format!("hosts file: {} hijacked line(s) disabled (backup: {})", bad.len(), backup.display()));
        cmd_quiet("ipconfig /flushdns");
        st.suspicious += 1;
    } else {
        r.put(Kind::Fail, "The hosts file looks hijacked but could not be changed (in use / protected).");
    }
}

fn services_stage(r: &mut Report, f: &Facts) {
    let before = r.fixed.len();
    for (name, start) in SERVICES {
        let Some(v) = f.all("SVC").find(|v| field(v, 0).eq_ignore_ascii_case(name)) else { continue };
        let (mode, state) = (field(v, 1), field(v, 2));
        let disabled = mode.eq_ignore_ascii_case("Disabled");
        if disabled {
            if cmd_quiet(&format!("sc config {name} start= {start}")) {
                r.fix(format!("Service {name} was disabled - set to {}", if *start == "auto" { "Automatic" } else { "Manual" }));
            } else {
                r.put(Kind::Fail, format!("Service {name} is disabled and could not be changed"));
                continue;
            }
        }
        let should_run = *start == "auto" && (disabled || mode.eq_ignore_ascii_case("Auto"));
        if should_run && !state.eq_ignore_ascii_case("Running") {
            if cmd_quiet(&format!("net start {name}")) {
                r.fix(format!("Service {name} was stopped - started"));
            } else {
                r.put(Kind::Warn, format!("Service {name} could not be started"));
            }
        }
    }

    if let Some(v) = f.all("PAGE").next() {
        if field(v, 0).eq_ignore_ascii_case("false")
            && num(v, 1) == 0
            && ps_ok("$c = Get-WmiObject Win32_ComputerSystem -EnableAllPrivileges; $c.AutomaticManagedPagefile = $true; $null = $c.Put()")
        {
            r.fix("Page file was switched off (programs crash with 'out of memory') - set to automatic");
            r.restart_needed = true;
        }
    }
    for v in f.all("REBOOT") {
        r.put(Kind::Warn, format!("A restart is pending for {}.", field(v, 0)));
        r.restart_needed = true;
    }
    if r.fixed.len() == before {
        r.put(Kind::Ok, "Windows services and settings are OK.");
    }
}

fn devices_stage(r: &mut Report, st: &State) {
    let probs = ui::spin("Scanning devices", || drivers::problem_devices(false));
    if probs.is_empty() {
        r.put(Kind::Ok, "All devices work.");
        return;
    }
    let mut touched = false;
    for d in &probs {
        r.log(format!("PROBLEM {}  (code {}: {})", d.name, d.code, drivers::code_meaning(d.code)));
        match d.code {
            22 => touched |= drivers::enable_device(&d.hwid),
            1 | 28 => {} // no driver - installed below
            _ => touched |= drivers::restart_device(&d.hwid),
        }
    }
    let left: Vec<ProblemDevice> = if touched {
        ui::spin("Checking devices", || {
            drivers::rescan_devices();
            std::thread::sleep(Duration::from_secs(3));
            drivers::problem_devices(false)
        })
    } else {
        probs.iter().map(|d| ProblemDevice { name: d.name.clone(), hwid: d.hwid.clone(), code: d.code, likely_network: d.likely_network }).collect()
    };
    for d in &probs {
        if !left.iter().any(|l| l.hwid.eq_ignore_ascii_case(&d.hwid)) {
            r.fix(format!("{}: works again", d.name));
        }
    }
    if left.is_empty() {
        return;
    }
    if drivers::drivers_dir().is_some() {
        let (ok, _) = drivers::install_from_folder(&mut |s| r.log(s));
        if ok > 0 {
            r.restart_needed = true;
        }
    }
    if st.internet {
        let outcome = online::run(&mut |k, s| r.put(k, s), false);
        r.restart_needed |= outcome.reboot;
        for d in &outcome.remaining {
            r.advise(d, None);
        }
        if let Some(dir) = outcome.kept_dir {
            r.manual(format!("Snappy Driver Installer files were kept for a retry in {} - delete with: rmdir /s /q \"{}\"", dir.display(), dir.display()));
        }
    } else {
        for d in &left {
            r.advise(d, None);
        }
    }
}

fn defender_stage(r: &mut Report, f: &Facts, st: &mut State) {
    // WinTool's own scanner runs on every PC, also next to another antivirus.
    st.suspicious += crate::scan::run_into(r, st.internet);
    if let Some(av) = &st.third_party_av {
        r.put(Kind::Info, format!("{av} protects this PC - run a full scan with it from time to time."));
        if st.suspicious > 0 {
            r.manual(format!("Signs of malware were found: run a FULL scan with {av} now."));
        }
        return;
    }
    if let Some(v) = f.all("DEF").next() {
        if field(v, 1).eq_ignore_ascii_case("false") {
            cmd_quiet("net start WinDefend");
            if ps_ok("Set-MpPreference -DisableRealtimeMonitoring $false") {
                r.fix("Defender real-time protection was off - turned on");
            } else {
                r.put(Kind::Warn, "Defender real-time protection is off and could not be turned on.");
                r.manual("Turn on Windows Security -> Virus & threat protection -> Real-time protection.");
            }
        }
        let age = num(v, 2);
        if age > 2 && !st.internet {
            r.put(Kind::Warn, format!("Defender virus definitions are {age} days old (no internet to update them)."));
        }
    }
    defender_scan(r, st.internet);
}

/// Updates Defender's virus definitions (online) and runs its quick scan.
pub(crate) fn defender_scan(r: &mut Report, internet: bool) {
    let mpcmd = program_files().join(r"Windows Defender\MpCmdRun.exe");
    if !mpcmd.is_file() {
        r.put(Kind::Warn, "Windows Defender is not available on this PC.");
        r.manual("No antivirus besides WinTool's scanner: install a free antivirus that supports this Windows version.");
        return;
    }
    if internet {
        let ok = ui::spin("Updating Defender virus definitions", || {
            Command::new(&mpcmd).arg("-SignatureUpdate").output().map(|o| o.status.success()).unwrap_or(false)
        });
        if ok {
            r.fix("Defender virus definitions updated");
        } else {
            r.put(Kind::Warn, "Defender virus definitions could not be updated.");
        }
    }
    let code = ui::spin("Defender quick scan (5-15 minutes)", || {
        Command::new(&mpcmd).args(["-Scan", "-ScanType", "1"]).output().ok().and_then(|o| o.status.code())
    });
    match code {
        Some(0) => r.put(Kind::Ok, "Defender quick scan: no threats found."),
        Some(2) => {
            r.put(Kind::Warn, "Defender quick scan FOUND THREATS - Defender removes / quarantines them.");
            r.manual("Defender found malware: Windows Security -> Virus & threat protection -> Protection history, then run a FULL scan.");
        }
        _ => r.put(Kind::Warn, "Defender quick scan could not run (another antivirus may have turned Defender off)."),
    }
}

fn system_files_stage(r: &mut Report, st: &State) {
    if !is_win7_or_older() {
        if st.internet {
            if crate::util::run("DISM RestoreHealth (repairs the Windows image, 10-30 min)", "Dism.exe", &["/Online", "/Cleanup-Image", "/RestoreHealth"]) {
                r.log("   DISM: Windows image checked / repaired");
            } else {
                r.put(Kind::Warn, "DISM could not repair the Windows image.");
                r.manual("DISM failed: run main menu -> System repair -> DISM again later, or do a repair install of Windows (keeps files).");
            }
        } else {
            r.log("   DISM skipped - needs internet.");
        }
    }
    if crate::util::run("SFC /scannow (checks every system file, 10-30 min)", "sfc", &["/scannow"]) {
        r.log("   SFC finished - system files checked / repaired.");
    } else {
        r.put(Kind::Warn, "SFC found problems it could not repair (see above) - restart and run the FULL CHECK again.");
        r.restart_needed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_rows() {
        let f = Facts::parse("\u{feff}SMART|SCSI\\Disk&Ven_1|False\nVOL|C:|1000|5000\nnoise line\nDONE|\n");
        assert!(f.complete());
        assert_eq!(f.all("VOL").next().unwrap(), &["C:".to_string(), "1000".into(), "5000".into()]);
        assert_eq!(f.all("SMART").count(), 1);
    }

    #[test]
    fn bugchecks() {
        assert_eq!(parse_bugcheck("0x0000001a (0x0000000000041792, 0x0)"), Some(0x1A));
        assert_eq!(parse_bugcheck("The bugcheck was: 0x00000124 (0x0, ...)"), Some(0x124));
        assert_eq!(parse_bugcheck("nothing"), None);
        assert_eq!(bugcheck(0x1A).2, Hw::Ram);
        assert_eq!(bugcheck(0x7A).2, Hw::Disk);
        assert_eq!(bugcheck(0xD1).2, Hw::None);
    }

    #[test]
    fn hosts() {
        let text = "# comment\n127.0.0.1 localhost\n::1 localhost\n0.0.0.0 ads.example.com\n\
                    192.168.1.5 nas.home\n203.0.113.9 www.paypal.com\n127.0.0.1 update.microsoft.com\n\
                    0.0.0.0 vortex.data.microsoft.com\n10.0.0.2 www.kaspersky.com # redirect\n";
        assert_eq!(hosts_hijacks(text), vec![5, 6, 8]);
    }

    #[test]
    fn startup_files() {
        assert_eq!(startup_file(r#""C:\Program Files\App\app.exe" /min"#).as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(startup_file(r"C:\Program Files\App\app.exe -tray").as_deref(), Some(r"C:\Program Files\App\app.exe"));
        assert_eq!(startup_file(r#"rundll32.exe "C:\Users\x\AppData\evil.dll",Start"#).as_deref(), Some(r"C:\Users\x\AppData\evil.dll"));
        assert_eq!(startup_file("ctfmon.exe"), None);
        assert_eq!(startup_file(""), None);
    }

    #[test]
    fn env_expansion() {
        std::env::set_var("WT_TEST_DIR", r"C:\T");
        assert_eq!(expand_env(r"%WT_TEST_DIR%\a.exe"), r"C:\T\a.exe");
        assert_eq!(expand_env("100% sure %NOPE_XYZ%"), "100% sure %NOPE_XYZ%");
    }

    #[test]
    fn hijack_rules() {
        assert!(userinit_ok(r"C:\Windows\system32\userinit.exe,"));
        assert!(!userinit_ok(r"C:\Windows\system32\userinit.exe,C:\evil.exe"));
        assert!(ifeo_legit(r"C:\Tools\procexp64.exe"));
        assert!(!ifeo_legit(r"C:\evil.exe"));
        assert!(bad_exclusion("Path", r"C:\", r"C:\Windows"));
        assert!(bad_exclusion("Path", r"C:\Windows", r"C:\Windows"));
        assert!(!bad_exclusion("Path", r"D:\Games\Steam", r"C:\Windows"));
        assert!(bad_exclusion("Process", r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe", r"C:\Windows"));
        assert!(bad_exclusion("Extension", ".exe", r"C:\Windows"));
        assert!(product_enabled("397568")); // 0x061100
        assert!(!product_enabled("393472")); // 0x060100
    }

    #[test]
    fn proxies() {
        assert_eq!(proxy_hostport("http=1.2.3.4:8080;https=5.6.7.8:3128").as_deref(), Some("5.6.7.8:3128"));
        assert_eq!(proxy_hostport("proxy.local").as_deref(), Some("proxy.local:80"));
        assert_eq!(url_hostport("http://wpad.corp/wpad.dat").as_deref(), Some("wpad.corp:80"));
        assert_eq!(url_hostport("https://x.y:8443/p.pac").as_deref(), Some("x.y:8443"));
        assert_eq!(url_hostport("file:///C:/p.pac"), None);
    }
}
