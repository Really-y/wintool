//! Avtomatik şəbəkə diaqnostikası və təmiri.
//!
//! Ardıcıllıq: internet testi → adapter/driver → bağlantı (kabel / Wi-Fi) →
//! IP/DHCP → gateway → proxy/Winsock → DNS → captive portal / saat → hesabat.
//! Hər düzəlişdən sonra yenidən test edir; problem həll olunanda dayanır.
//! Bütün yoxlamalar dildən asılı deyil (WMI rəqəmsal kodları, Win32 API, TCP testləri).

use crate::util::*;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const NET_CLASS: &str = "{4d36e972-e325-11ce-bfc1-08002be10318}";

// ---------------------------------------------------------------- hesabat

struct Report {
    lines: Vec<String>,
    fixed: Vec<String>,
    manual: Vec<String>,
    restart_needed: bool,
}

impl Report {
    fn new() -> Self {
        Self { lines: vec![], fixed: vec![], manual: vec![], restart_needed: false }
    }
    fn log(&mut self, s: impl Into<String>) {
        let s = s.into();
        println!("{s}");
        self.lines.push(s);
    }
    fn fix(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.log(format!("   -> {s}"));
        self.fixed.push(s);
    }
    fn manual(&mut self, s: impl Into<String>) {
        self.manual.push(s.into());
    }
}

// ---------------------------------------------------------------- PowerShell / WMI

/// PowerShell skriptini -EncodedCommand ilə işlədir (dırnaq problemi olmur, PS 2.0+ / Win7 işləyir).
fn ps(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
        .arg(base64(&utf16))
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn base64(data: &[u8]) -> String {
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

#[derive(Clone)]
struct Adapter {
    name: String,
    conn_id: String,
    device_id: String,
    pnp_id: String,
    enabled: bool,
    err: u32,
    status: u32,
}

impl Adapter {
    fn wireless(&self) -> bool {
        let n = format!("{} {}", self.name, self.conn_id).to_lowercase();
        ["wireless", "wi-fi", "wifi", "wlan", "802.11"].iter().any(|k| n.contains(k))
    }
    fn label(&self) -> String {
        if self.conn_id.is_empty() { self.name.clone() } else { format!("{} [{}]", self.conn_id, self.name) }
    }
    fn connected(&self) -> bool {
        self.status == 2 || self.status == 9
    }
}

/// Fiziki şəbəkə adapterləri (virtual, Bluetooth, VPN çıxarılır).
fn adapters() -> Vec<Adapter> {
    let out = ps(r#"$ErrorActionPreference='SilentlyContinue'
Get-WmiObject Win32_NetworkAdapter -Filter "PhysicalAdapter=True" | ForEach-Object {
 "{0}|{1}|{2}|{3}|{4}|{5}|{6}" -f $_.Name,$_.NetConnectionID,$_.DeviceID,$_.PNPDeviceID,$_.NetEnabled,$_.ConfigManagerErrorCode,$_.NetConnectionStatus }"#);
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split('|').collect();
            (f.len() == 7).then(|| Adapter {
                name: f[0].to_string(),
                conn_id: f[1].to_string(),
                device_id: f[2].to_string(),
                pnp_id: f[3].to_string(),
                enabled: f[4].eq_ignore_ascii_case("true"),
                err: f[5].parse().unwrap_or(0),
                status: f[6].parse().unwrap_or(0),
            })
        })
        .filter(|a| {
            let n = a.name.to_lowercase();
            let id = a.pnp_id.to_uppercase();
            !id.starts_with("ROOT\\")
                && !id.starts_with("SWD\\")
                && !["bluetooth", "virtual", "vmware", "virtualbox", "hyper-v", "tap-", "vpn", "miniport", "loopback", "tunnel"]
                    .iter()
                    .any(|k| n.contains(k))
        })
        .collect()
}

struct ProblemDevice {
    name: String,
    hwid: String,
    code: u32,
    likely_network: bool,
}

/// Device Manager-də xətası olan və şəbəkə ilə əlaqəli ola biləcək cihazlar.
fn problem_devices() -> Vec<ProblemDevice> {
    let out = ps(r#"$ErrorActionPreference='SilentlyContinue'
Get-WmiObject Win32_PnPEntity | Where-Object { $_.ConfigManagerErrorCode -ne 0 } | ForEach-Object {
 "{0}|{1}|{2}|{3}" -f $_.Name,$_.DeviceID,$_.ClassGuid,$_.ConfigManagerErrorCode }"#);
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split('|').collect();
            if f.len() != 4 {
                return None;
            }
            let name = f[0].to_lowercase();
            let id = f[1].to_uppercase();
            let code: u32 = f[3].parse().unwrap_or(0);
            let net_class = f[2].eq_ignore_ascii_case(NET_CLASS);
            let net_name = ["network", "ethernet", "wireless", "wi-fi", "wifi", "wlan", "802.11", "lan "]
                .iter()
                .any(|k| name.contains(k));
            // Driversiz naməlum PCI/USB cihazı — tez-tez şəbəkə kartı olur.
            let unknown_bus = code == 28 && f[2].is_empty() && (id.starts_with("PCI\\") || id.starts_with("USB\\"));
            if net_name && name.contains("bluetooth") {
                return None;
            }
            (net_class || net_name || unknown_bus).then(|| ProblemDevice {
                name: f[0].to_string(),
                hwid: f[1].to_string(),
                code,
                likely_network: net_class || net_name,
            })
        })
        .collect()
}

fn code_meaning(code: u32) -> &'static str {
    match code {
        1 | 18 => "driver duzgun qurulmayib",
        3 => "driver zedelenib ve ya yaddas catmir",
        10 => "cihaz ise dusmur (driver xetasi)",
        12 => "resurs konflikti",
        14 => "restart lazimdir",
        19 | 39 => "driver zedelenib / registry xetasi",
        21 | 24 | 45 => "cihaz tapilmir (qosulmayib ve ya xarab)",
        22 => "cihaz Device Manager-de sondurulub",
        28 => "DRIVER QURASDIRILMAYIB",
        31 => "driver yuklenmir",
        37 | 52 => "driver yuklenmir (imza / uygunsuzluq)",
        43 => "cihaz xeta verib dayandi (driver ve ya hardware)",
        _ => "namelum xeta",
    }
}

/// `PCI\VEN_10EC&DEV_8168&...` → ("10EC", "8168"); USB üçün VID/PID.
fn ven_dev(hwid: &str) -> Option<(String, String)> {
    let u = hwid.to_uppercase();
    let grab = |key: &str| u.find(key).map(|i| u[i + key.len()..].chars().take(4).collect::<String>());
    grab("VEN_").zip(grab("DEV_")).or_else(|| grab("VID_").zip(grab("PID_")))
}

fn vendor(ven: &str) -> &'static str {
    match ven {
        "10EC" | "0BDA" => "Realtek",
        "8086" | "8087" => "Intel",
        "168C" | "17CB" | "0CF3" => "Qualcomm Atheros",
        "1969" => "Qualcomm Atheros / Killer",
        "14E4" | "0A5C" => "Broadcom",
        "14C3" | "0E8D" => "MediaTek",
        "1814" | "148F" => "Ralink / MediaTek",
        "11AB" | "1B4B" => "Marvell",
        "10B7" => "3Com",
        "0846" => "Netgear",
        "2357" => "TP-Link",
        "0B95" => "ASIX",
        _ => "",
    }
}

// ---------------------------------------------------------------- düzəlişlər

#[link(name = "cfgmgr32")]
extern "system" {
    fn CM_Locate_DevNodeW(pdnDevInst: *mut u32, pDeviceID: *const u16, ulFlags: u32) -> u32;
    fn CM_Reenumerate_DevNode(dnDevInst: u32, ulFlags: u32) -> u32;
}

/// Device Manager → "Scan for hardware changes" (bütün ağac, sinxron).
fn rescan_devices() -> bool {
    unsafe {
        let mut root = 0u32;
        if CM_Locate_DevNodeW(&mut root, std::ptr::null(), 0) != 0 {
            return false;
        }
        CM_Reenumerate_DevNode(root, 1) == 0
    }
}

fn wmi_adapter_call(a: &Adapter, method: &str) {
    if a.device_id.chars().all(|c| c.is_ascii_digit()) && !a.device_id.is_empty() {
        ps(&format!(
            "$a = Get-WmiObject Win32_NetworkAdapter -Filter \"DeviceID='{}'\"; if ($a) {{ [void]$a.{method}() }}",
            a.device_id
        ));
    }
}

fn enable_adapter(a: &Adapter) {
    if !a.conn_id.is_empty() && !a.conn_id.contains('"') {
        cmd_quiet(&format!(r#"netsh interface set interface name="{}" admin=enabled"#, a.conn_id));
    }
    wmi_adapter_call(a, "Enable");
}

fn restart_adapter(a: &Adapter) {
    wmi_adapter_call(a, "Disable");
    sleep(Duration::from_secs(3));
    enable_adapter(a);
}

/// `.exe`-nin yanındakı `Drivers\` qovluğunda olan bütün .inf driverləri quraşdırır.
fn install_local_drivers(r: &mut Report) -> usize {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("Drivers"))) else {
        return 0;
    };
    let mut infs = vec![];
    collect_inf(&dir, &mut infs, 0);
    if infs.is_empty() {
        return 0;
    }
    r.log(format!("   Drivers qovlugunda {} driver tapildi, qurasdirilir...", infs.len()));
    let mut ok = 0;
    for inf in &infs {
        // `pnputil -i -a` həm Win7, həm Win10/11-də işləyir.
        let s = Command::new("pnputil").arg("-i").arg("-a").arg(inf).output();
        if s.map(|o| o.status.success()).unwrap_or(false) {
            ok += 1;
        }
    }
    ok
}

fn collect_inf(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 6 {
        return;
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_inf(&p, out, depth + 1);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("inf")) {
                out.push(p);
            }
        }
    }
}

/// Saxlanmış Wi-Fi profillərinə növbə ilə qoşulmağa çalışır.
fn wifi_connect_saved(r: &mut Report, a: &Adapter) -> bool {
    let out = Command::new("netsh").args(["wlan", "show", "profiles"]).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    // Başlıq xəttindən ("-----") sonrakı "etiket : ad" sətirləri — etiket dilə görə dəyişir, ad yox.
    let profiles: Vec<String> = text
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("---"))
        .filter_map(|l| l.split_once(" : ").map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty() && !v.contains('"'))
        .collect();
    if profiles.is_empty() {
        r.log("   Saxlanmis Wi-Fi sebekesi yoxdur.");
        return false;
    }
    for p in profiles {
        r.log(format!("   Wi-Fi-a qosulmaga calisir: {p}"));
        let ifc = if a.conn_id.is_empty() || a.conn_id.contains('"') { String::new() } else { format!(r#" interface="{}""#, a.conn_id) };
        cmd_quiet(&format!(r#"netsh wlan connect name="{p}"{ifc}"#));
        for _ in 0..6 {
            sleep(Duration::from_secs(2));
            if adapters().iter().any(|x| x.device_id == a.device_id && x.connected()) {
                r.fix(format!("Wi-Fi-a qosuldu: {p}"));
                return true;
            }
        }
    }
    false
}

const INET_SETTINGS: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

fn reg_value(name: &str) -> Option<String> {
    let out = Command::new("reg").args(["query", INET_SETTINGS, "/v", name]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "    ProxyServer    REG_SZ    host:port" — dəyər REG_* tipindən sonra gəlir.
    text.lines().find_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        let i = t.iter().position(|x| x.starts_with("REG_"))?;
        (t.first() == Some(&name) && i + 1 < t.len()).then(|| t[i + 1..].join(" "))
    })
}

/// Aktiv istifadəçi proxy-si (`host:port`), yoxdursa None.
fn user_proxy() -> Option<String> {
    if reg_value("ProxyEnable")? != "0x1" {
        return None;
    }
    let raw = reg_value("ProxyServer")?;
    // "http=host:port;https=host:port" və ya sadəcə "host:port"
    let first = raw.split(';').find(|s| s.starts_with("https=")).or_else(|| raw.split(';').next())?;
    let hp = first.split_once('=').map(|(_, v)| v).unwrap_or(first).trim();
    let hp = hp.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/');
    (!hp.is_empty()).then(|| if hp.contains(':') { hp.to_string() } else { format!("{hp}:80") })
}

fn proxy_reachable(hostport: &str) -> bool {
    hostport
        .to_socket_addrs()
        .ok()
        .and_then(|mut i| i.next())
        .map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(4)).is_ok())
        .unwrap_or(false)
}

fn reset_proxy_settings() {
    cmd_quiet("netsh winhttp reset proxy");
    cmd_quiet(&format!(r#"reg add "{INET_SETTINGS}" /v ProxyEnable /t REG_DWORD /d 0 /f"#));
}

fn set_dns(r: &mut Report, a: &Adapter) {
    if a.conn_id.is_empty() || a.conn_id.contains('"') {
        return;
    }
    let ok = cmd_quiet(&format!(r#"netsh interface ip set dns name="{}" source=static addr=1.1.1.1"#, a.conn_id));
    cmd_quiet(&format!(r#"netsh interface ip add dns name="{}" addr=8.8.8.8 index=2"#, a.conn_id));
    cmd_quiet("ipconfig /flushdns");
    if ok {
        r.fix(format!("DNS server 1.1.1.1 / 8.8.8.8 edildi ({})", a.conn_id));
    }
}

// ---------------------------------------------------------------- testlər

#[derive(PartialEq)]
enum Http {
    Ok,
    Portal,
    Fail,
}

struct Probe {
    gw: Option<String>,
    gw_ok: bool,
    apipa: bool,
    tcp: bool,
    dns: bool,
    http: Http,
    server_time: Option<i64>,
}

impl Probe {
    fn ok(&self) -> bool {
        self.tcp && self.dns && self.http == Http::Ok
    }
}

fn default_gateway() -> Option<String> {
    let out = Command::new("route").args(["print", "0.0.0.0"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().find_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        (t.len() >= 3 && t[0] == "0.0.0.0" && t[1] == "0.0.0.0" && t[2].contains('.')).then(|| t[2].to_string())
    })
}

fn tcp_ok(addr: &str) -> bool {
    addr.parse::<SocketAddr>()
        .map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(4)).is_ok())
        .unwrap_or(false)
}

/// Microsoft-un öz internet testi: real internet / captive portal ayırd edir, server saatını verir.
fn http_probe() -> (Http, Option<i64>) {
    let Some(addr) = ("www.msftconnecttest.com", 80).to_socket_addrs().ok().and_then(|mut i| i.next()) else {
        return (Http::Fail, None);
    };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_secs(5)) else {
        return (Http::Fail, None);
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(6)));
    let _ = s.write_all(b"GET /connecttest.txt HTTP/1.1\r\nHost: www.msftconnecttest.com\r\nConnection: close\r\n\r\n");
    let mut buf = Vec::new();
    let _ = Read::take(&mut s, 65536).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let date = text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case("date").then(|| parse_http_date(v.trim())).flatten()
    });
    if text.contains("Microsoft Connect Test") {
        (Http::Ok, date)
    } else if text.starts_with("HTTP/") {
        (Http::Portal, date)
    } else {
        (Http::Fail, None)
    }
}

/// "Sun, 08 Nov 2026 13:55:00 GMT" → unix saniyə.
fn parse_http_date(s: &str) -> Option<i64> {
    let p: Vec<&str> = s.split_whitespace().collect();
    if p.len() < 5 {
        return None;
    }
    let d: i64 = p[1].parse().ok()?;
    let m = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|x| x.eq_ignore_ascii_case(p[2]))? as i64
        + 1;
    let y: i64 = p[3].parse().ok()?;
    let t: Vec<i64> = p[4].split(':').filter_map(|x| x.parse().ok()).collect();
    if t.len() != 3 {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86400 + t[0] * 3600 + t[1] * 60 + t[2])
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn has_apipa() -> bool {
    let out = Command::new("ipconfig").output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).contains("169.254.")).unwrap_or(false)
}

fn probe() -> Probe {
    let gw = default_gateway();
    let gw_ok = gw.as_deref().map(|g| cmd_quiet(&format!("ping -n 2 -w 1500 {g}"))).unwrap_or(false);
    let tcp = tcp_ok("1.1.1.1:443") || tcp_ok("8.8.8.8:443") || tcp_ok("1.1.1.1:80");
    let dns = ("www.microsoft.com", 80).to_socket_addrs().map(|mut i| i.next().is_some()).unwrap_or(false);
    let (http, server_time) = if dns { http_probe() } else { (Http::Fail, None) };
    Probe { gw, gw_ok, apipa: has_apipa(), tcp, dns, http, server_time }
}

fn show(r: &mut Report, p: &Probe) {
    let m = |b: bool| if b { "OK" } else { "XX" };
    match &p.gw {
        Some(g) => r.log(format!("   [{}] Modem / router ({g})", m(p.gw_ok))),
        None => r.log("   [XX] Modem / router - IP / gateway yoxdur"),
    }
    r.log(format!("   [{}] Internet (TCP)", m(p.tcp)));
    r.log(format!("   [{}] DNS", m(p.dns)));
    let h = match p.http {
        Http::Ok => "OK",
        Http::Portal => "!!",
        Http::Fail => "XX",
    };
    r.log(format!("   [{h}] Web (HTTP)"));
}

fn probe_after(secs: u64) -> Probe {
    sleep(Duration::from_secs(secs));
    probe()
}

fn now_str() -> String {
    let out = Command::new("cmd").args(["/C", "echo %date% %time%"]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

// ---------------------------------------------------------------- əsas axın

pub fn run() {
    header("Internet islemir - AVTOMATIK tap ve hell et");
    let mut r = Report::new();
    r.log(format!("Tarix:  {}", now_str()));
    r.log(format!("Sistem: {}", version_str()));

    // 1) İlkin test
    r.log("\n[1/5] Internet yoxlanilir...");
    let mut p = probe();
    show(&mut r, &p);

    if !p.ok() {
        // 2) Adapter və driver
        r.log("\n[2/5] Sebeke adapterleri ve driverler yoxlanilir...");
        driver_stage(&mut r);

        // 3) Fiziki bağlantı
        r.log("\n[3/5] Baglanti (kabel / Wi-Fi) yoxlanilir...");
        link_stage(&mut r);

        // 4) IP / DHCP / gateway / DNS
        r.log("\n[4/5] IP, DNS ve internet temir edilir...");
        p = network_stage(&mut r);
    } else {
        r.log("\nInternet isleyir - temire ehtiyac yoxdur. Elave yoxlamalar edilir...");
    }

    // 5) Captive portal və saat
    r.log("\n[5/5] Son yoxlamalar...");
    final_stage(&mut r, &p);
    let p = probe();
    show(&mut r, &p);

    summary(&mut r, &p);
    save_report(&mut r);

    if r.restart_needed && !p.ok() && confirm("\nIndi restart edilsin?") {
        cmd("10 saniyeye restart", "shutdown /r /t 10");
    }
}

fn driver_stage(r: &mut Report) {
    let mut ads = adapters();
    let probs = problem_devices();

    for a in &ads {
        let st = match a.status {
            2 | 9 => "qosulub",
            7 => "kabel / Wi-Fi qosulmayib",
            4 => "cihaz tapilmir",
            5 => "sondurulub",
            6 => "hardware xetasi",
            0 if !a.enabled => "sondurulub",
            _ => "qosulmayib",
        };
        r.log(format!("   {} - {st}{}", a.label(), if a.err != 0 { format!(", xeta kodu {}", a.err) } else { String::new() }));
    }
    for d in &probs {
        let vd = ven_dev(&d.hwid).map(|(v, dv)| format!("  [{} VEN {v} DEV {dv}]", vendor(&v))).unwrap_or_default();
        r.log(format!("   PROBLEM: {} - kod {}: {}{vd}", d.name, d.code, code_meaning(d.code)));
    }

    let driver_issue = ads.is_empty() || ads.iter().any(|a| a.err != 0 && a.err != 22) || !probs.is_empty();
    if driver_issue {
        if ads.is_empty() {
            r.log("   Hec bir fiziki sebeke adapteri tapilmadi.");
        }
        r.log("   Cihazlar yeniden axtarilir (scan for hardware changes)...");
        if rescan_devices() {
            r.fix("Cihazlar yeniden skan edildi");
        }
        sleep(Duration::from_secs(5));

        let n = install_local_drivers(r);
        if n > 0 {
            r.fix(format!("{n} driver Drivers qovlugundan qurasdirildi"));
            rescan_devices();
            sleep(Duration::from_secs(8));
        }

        for a in adapters().iter().filter(|a| a.err != 0 && a.err != 22 && a.err != 28) {
            r.log(format!("   Adapter yeniden basladilir: {}", a.label()));
            restart_adapter(a);
            r.fix(format!("Adapter yeniden basladildi: {}", a.label()));
        }
        sleep(Duration::from_secs(5));
    }

    // Söndürülmüş adapterləri yandır
    ads = adapters();
    for a in ads.iter().filter(|a| a.err == 22 || a.status == 5 || (!a.enabled && a.status == 0)) {
        r.log(format!("   Sondurulmus adapter yandirilir: {}", a.label()));
        enable_adapter(a);
        r.fix(format!("Adapter yandirildi: {}", a.label()));
    }

    // Hələ də driversiz qalanlar → əl ilə görüləcək iş
    sleep(Duration::from_secs(3));
    let still: Vec<ProblemDevice> = problem_devices().into_iter().filter(|d| d.code != 22).collect();
    let no_adapter = adapters().is_empty();
    for d in &still {
        let (v, dv) = ven_dev(&d.hwid).unwrap_or_default();
        let brand = vendor(&v);
        let brand_sp = if brand.is_empty() { String::new() } else { format!("{brand} ") };
        let what = if d.likely_network { "sebeke cihazi" } else { "namelum cihaz (sebeke karti ola biler)" };
        r.manual(format!(
            "{what} '{}' - {}. Driver lazimdir: {brand_sp}VEN_{v}&DEV_{dv}. Driveri {brand_sp}istehsalcinin \
             saytindan endir, WinTool.exe-nin yanindaki 'Drivers' qovluguna qoy ve avtomatik temiri yeniden islet \
             (ve ya Windows Update ucun basqa yolla internet ver: telefonla USB tethering).",
            d.name,
            code_meaning(d.code),
        ));
    }
    if no_adapter && still.is_empty() {
        r.manual(
            "Sebeke adapteri umumiyyetle gorunmur. Noutbukda Wi-Fi duymesini / Fn+F2 / airplane mode-u yoxla, \
             BIOS-da 'Onboard LAN / WLAN' aktiv olsun. USB Wi-Fi adapterdirse, basqa USB portuna tax."
                .to_string(),
        );
    }
}

fn link_stage(r: &mut Report) {
    let ads = adapters();
    if ads.iter().any(|a| a.connected()) {
        for a in ads.iter().filter(|a| a.connected()) {
            r.log(format!("   Qosulub: {}", a.label()));
        }
        return;
    }
    for a in ads.iter().filter(|a| a.wireless() && a.err == 0) {
        r.log(format!("   Wi-Fi qosulmayib: {}", a.label()));
        if wifi_connect_saved(r, a) {
            return;
        }
    }
    for a in ads.iter().filter(|a| a.err == 0) {
        if a.wireless() {
            r.manual(format!(
                "Wi-Fi ({}) hec bir sebekeye qosulmayib. Wi-Fi siyahisindan sebekeni sec ve parolu yaz. \
                 Siyahi bosdursa: airplane mode / Wi-Fi duymesi / Fn+F2.",
                a.label()
            ));
        } else if a.status == 7 {
            r.manual(format!(
                "Ethernet kabeli qosulmayib ({}). Kabeli her iki terefden yoxla, modemde basqa porta tax, \
                 portun isigi yanmalidir.",
                a.label()
            ));
        }
    }
}

fn network_stage(r: &mut Report) -> Probe {
    let mut p = probe_after(3);
    show(r, &p);
    if p.ok() {
        return p;
    }
    let active: Vec<Adapter> = adapters().into_iter().filter(|a| a.connected()).collect();

    // IP alınmayıb (DHCP)
    if !p.tcp && (p.gw.is_none() || p.apipa) {
        r.log("   IP alinmayib (DHCP) - DHCP servisi ve IP yenilenir...");
        cmd_quiet("sc config Dhcp start= auto");
        cmd_quiet("net start Dhcp");
        cmd_quiet("ipconfig /release");
        cmd_quiet("ipconfig /renew");
        r.fix("DHCP servisi yoxlanildi, IP yenilendi");
        p = probe_after(5);
        if !p.tcp && (p.gw.is_none() || p.apipa) {
            for a in &active {
                restart_adapter(a);
                r.fix(format!("Adapter yeniden basladildi: {}", a.label()));
            }
            p = probe_after(10);
        }
    }

    // Gateway cavab vermir
    if !p.tcp && p.gw.is_some() && !p.gw_ok {
        r.log("   Modem cavab vermir - IP yenilenir...");
        cmd_quiet("ipconfig /release");
        cmd_quiet("ipconfig /renew");
        r.fix("IP yenilendi");
        p = probe_after(5);
    }

    // Modem var, internet yoxdur → proxy / Winsock / TCP-IP
    if !p.tcp && p.gw_ok {
        r.log("   Modem isleyir, internet yoxdur - proxy ve sebeke stack yoxlanilir...");
        match user_proxy() {
            Some(px) if proxy_reachable(&px) => {
                // İşləyən korporativ proxy — birbaşa çıxış bağlı ola bilər, toxunmuruq.
                r.log(format!("   Isleyen proxy var ({px}) - bu sebeke internete proxy ile cixir, toxunulmadi."));
                r.manual(format!(
                    "Bu sebeke internete proxy ({px}) vasitesile cixir. Brauzer islemirse, IT sobesinden \
                     proxy ayarlarini ve sifreni yoxlat."
                ));
                show(r, &p);
                return p;
            }
            Some(px) => {
                r.log(format!("   Proxy ({px}) cavab vermir - kohne VPN / virus qaligi ola biler, sifirlanir."));
                reset_proxy_settings();
                r.fix(format!("Islemeyen proxy ({px}) sondurudu"));
            }
            None => {
                reset_proxy_settings();
                r.fix("Sistem proxy-si sifirlandi");
            }
        }
        p = probe_after(2);
        if !p.tcp {
            cmd_quiet("netsh winsock reset");
            cmd_quiet("netsh int ip reset");
            r.fix("Winsock ve TCP/IP stack sifirlandi (restart lazimdir)");
            r.restart_needed = true;
        }
    }

    // İnternet var, DNS yoxdur
    if p.tcp && !p.dns {
        r.log("   Internet var, DNS islemir - DNS temir edilir...");
        cmd_quiet("ipconfig /flushdns");
        cmd_quiet("net stop Dnscache");
        cmd_quiet("net start Dnscache");
        r.fix("DNS cache temizlendi, DNS servisi yeniden basladi");
        p = probe_after(2);
        if !p.dns {
            for a in &active {
                set_dns(r, a);
            }
            p = probe_after(3);
        }
    }
    show(r, &p);

    if !p.ok() && p.gw_ok && !p.tcp && !r.restart_needed {
        r.manual(
            "Modem isleyir, amma internete cixis yoxdur. Modemi 30 saniye sondur-yandir; \
             olmazsa problem provayderdedir (borc, xett qezasi) - provayderle elaqe saxla."
                .to_string(),
        );
    }
    p
}

fn final_stage(r: &mut Report, p: &Probe) {
    if p.http == Http::Portal {
        r.log("   Bu sebeke giris sehifesi teleb edir (hotel / ofis / kafe Wi-Fi).");
        let _ = Command::new("cmd").args(["/C", "start", "", "http://www.msftconnecttest.com/redirect"]).status();
        r.manual("Brauzerde acilan giris sehifesinde login ol / sertleri qebul et.".to_string());
    }
    // Saat yoxlaması: səhv saat HTTPS saytlarını açılmaz edir.
    let server = p.server_time.or_else(|| http_probe().1);
    if let Some(srv) = server {
        let local = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let diff = (local - srv).abs();
        if diff > 300 {
            r.log(format!("   Komputerin saati {} deqiqe sehvdir - saytlar bu sebebden acilmaya biler.", diff / 60));
            let utc = utc_string(srv);
            ps(&format!(
                "Set-Date -Date ([DateTime]::SpecifyKind([DateTime]::ParseExact('{utc}','yyyy-MM-dd HH:mm:ss',$null),'Utc')).ToLocalTime() | Out-Null"
            ));
            cmd_quiet("net start w32time");
            cmd_quiet("w32tm /resync /rediscover");
            r.fix("Saat internetden duzeldildi");
        } else {
            r.log("   Saat duzgundur.");
        }
    }
}

fn utc_string(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let secs = unix.rem_euclid(86400);
    // civil_from_days
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

fn summary(r: &mut Report, p: &Probe) {
    r.log("\n==================================================");
    if p.ok() {
        r.log("  NETICE: HELL OLUNDU - internet isleyir.");
    } else if r.restart_needed {
        r.log("  NETICE: temir edildi, RESTART LAZIMDIR. Restartdan sonra yeniden yoxla.");
    } else {
        r.log("  NETICE: avtomatik hell olunmadi - asagidaki addimlari et.");
    }
    r.log("==================================================");
    let fixed = r.fixed.clone();
    if fixed.is_empty() {
        r.log("Edilen duzelisler: yoxdur");
    } else {
        r.log("Edilen duzelisler:");
        for f in fixed {
            r.log(format!("  + {f}"));
        }
    }
    let manual = r.manual.clone();
    if !manual.is_empty() && !p.ok() {
        r.log("\nSenin etmeli olduqlarin:");
        for (i, m) in manual.iter().enumerate() {
            r.log(format!("  {}. {m}", i + 1));
        }
    }
}

fn save_report(r: &mut Report) {
    let desktop = env_path("USERPROFILE", r"C:\Users\Public").join("Desktop");
    let file = if desktop.is_dir() { desktop } else { std::env::temp_dir() }.join("WinTool-sebeke-hesabati.txt");
    let body = r.lines.join("\r\n") + "\r\n";
    match fs::write(&file, body) {
        Ok(_) => r.log(format!("\nHesabat saxlanildi: {}", file.display())),
        Err(e) => r.log(format!("\nHesabat saxlanila bilmedi: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_date_roundtrip() {
        let t = parse_http_date("Sun, 08 Nov 2026 13:55:07 GMT").unwrap();
        assert_eq!(utc_string(t), "2026-11-08 13:55:07");
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
    }

    #[test]
    fn base64_matches_reference() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn ven_dev_parses_pci_and_usb() {
        assert_eq!(ven_dev(r"PCI\VEN_10EC&DEV_8168&SUBSYS_1"), Some(("10EC".into(), "8168".into())));
        assert_eq!(ven_dev(r"USB\VID_0BDA&PID_B812\123"), Some(("0BDA".into(), "B812".into())));
        assert_eq!(ven_dev("ROOT\\NET\\0000"), None);
    }
}
