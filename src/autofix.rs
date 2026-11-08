//! Automatic network diagnosis and repair.
//!
//! Order: internet test → adapters/drivers → link (cable / Wi-Fi) → IP/DHCP → gateway →
//! proxy/Winsock → DNS → captive portal / clock → report.
//! Re-tests after every fix and stops as soon as the internet works.
//! All checks are language-independent (numeric WMI codes, Win32 API, TCP/HTTP tests).

use crate::drivers::{self, ProblemDevice};
use crate::online;
use crate::safety;
use crate::ui::{self, Kind};
use crate::util::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- report

struct Report {
    lines: Vec<String>,
    fixed: Vec<String>,
    manual: Vec<Vec<String>>,
    advised: Vec<String>,
    restart_needed: bool,
}

impl Report {
    fn new() -> Self {
        Self { lines: vec![], fixed: vec![], manual: vec![], advised: vec![], restart_needed: false }
    }
    /// Colored line on screen, plain line in the report file.
    fn put(&mut self, kind: Kind, s: impl Into<String>) {
        let s = s.into();
        ui::line(kind, &s);
        self.lines.push(ui::plain(kind, &s));
    }
    fn log(&mut self, s: impl Into<String>) {
        let s = s.into();
        let kind = if s.trim_start().starts_with("PROBLEM") { Kind::Warn } else { Kind::Plain };
        self.put(kind, s);
    }
    fn step(&mut self, s: &str) {
        ui::step(s);
        self.lines.push(String::new());
        self.lines.push(s.to_string());
    }
    /// Report file only (not printed).
    fn note(&mut self, s: impl Into<String>) {
        self.lines.push(s.into());
    }
    fn fix(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.put(Kind::Ok, s.clone());
        self.fixed.push(s);
    }
    fn manual(&mut self, s: impl Into<String>) {
        self.manual.push(vec![s.into()]);
    }
    /// Download advice for a device, once per hardware ID.
    fn advise(&mut self, d: &ProblemDevice) {
        if self.advised.iter().any(|h| h.eq_ignore_ascii_case(&d.hwid)) {
            return;
        }
        self.advised.push(d.hwid.clone());
        let mut block = drivers::advice(d);
        if !d.likely_network && d.code == 28 && d.hwid.to_uppercase().starts_with("PCI") {
            block.insert(0, "Unknown device without a driver (often the network card):".into());
        }
        self.manual.push(block);
    }
}

// ---------------------------------------------------------------- adapters (WMI)

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

/// Physical network adapters (virtual, Bluetooth, VPN adapters are filtered out).
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

fn wmi_adapter_call(a: &Adapter, method: &str) {
    if !a.device_id.is_empty() && a.device_id.chars().all(|c| c.is_ascii_digit()) {
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

/// Tries saved Wi-Fi profiles one by one.
fn wifi_connect_saved(r: &mut Report, a: &Adapter) -> bool {
    let out = Command::new("netsh").args(["wlan", "show", "profiles"]).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    // "label : name" lines after the first "-----" header line; the label is localized, the name is not.
    let profiles: Vec<String> = text
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("---"))
        .filter_map(|l| l.split_once(" : ").map(|(_, v)| v.trim().to_string()))
        .filter(|v| !v.is_empty() && !v.contains('"'))
        .collect();
    if profiles.is_empty() {
        r.log("   No saved Wi-Fi networks.");
        return false;
    }
    for p in profiles {
        r.log(format!("   Trying to connect to Wi-Fi: {p}"));
        let ifc = if a.conn_id.is_empty() || a.conn_id.contains('"') {
            String::new()
        } else {
            format!(r#" interface="{}""#, a.conn_id)
        };
        cmd_quiet(&format!(r#"netsh wlan connect name="{p}"{ifc}"#));
        for _ in 0..6 {
            sleep(Duration::from_secs(2));
            if adapters().iter().any(|x| x.device_id == a.device_id && x.connected()) {
                r.fix(format!("Connected to Wi-Fi: {p}"));
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------- proxy / DNS

const INET_SETTINGS: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

fn reg_value(name: &str) -> Option<String> {
    let out = Command::new("reg").args(["query", INET_SETTINGS, "/v", name]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "    ProxyServer    REG_SZ    host:port" — the value follows the REG_* type.
    text.lines().find_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        let i = t.iter().position(|x| x.starts_with("REG_"))?;
        (t.first() == Some(&name) && i + 1 < t.len()).then(|| t[i + 1..].join(" "))
    })
}

/// Active user proxy as `host:port`, or None.
fn user_proxy() -> Option<String> {
    if reg_value("ProxyEnable")? != "0x1" {
        return None;
    }
    let raw = reg_value("ProxyServer")?;
    // "http=host:port;https=host:port" or just "host:port"
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
        r.fix(format!("DNS servers set to 1.1.1.1 / 8.8.8.8 ({})", a.conn_id));
    }
}

// ---------------------------------------------------------------- tests

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

/// Microsoft's own connectivity test: tells real internet from a captive portal and returns server time.
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

/// "Sun, 08 Nov 2026 13:55:00 GMT" → unix seconds.
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

fn utc_string(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let secs = unix.rem_euclid(86400);
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

fn probe_after(secs: u64) -> Probe {
    sleep(Duration::from_secs(secs));
    probe()
}

fn show(r: &mut Report, p: &Probe) {
    let k = |b: bool| if b { Kind::Ok } else { Kind::Fail };
    match &p.gw {
        Some(g) => r.put(k(p.gw_ok), format!("Modem / router ({g})")),
        None => r.put(Kind::Fail, "Modem / router - no IP / gateway"),
    }
    r.put(k(p.tcp), "Internet (TCP)");
    r.put(k(p.dns), "DNS");
    let h = match p.http {
        Http::Ok => Kind::Ok,
        Http::Portal => Kind::Warn,
        Http::Fail => Kind::Fail,
    };
    r.put(h, "Web (HTTP)");
}

/// Current DNS / proxy settings, written to the report before anything is changed.
fn record_settings(r: &mut Report) {
    r.note("");
    r.note("--- Settings before repair (to restore manually if needed) ---");
    r.note(format!("User proxy: ProxyEnable={} ProxyServer={}", reg_value("ProxyEnable").unwrap_or_default(), reg_value("ProxyServer").unwrap_or_default()));
    for (title, args) in [("WinHTTP proxy", &["winhttp", "show", "proxy"][..]), ("DNS servers", &["interface", "ip", "show", "dns"][..])] {
        let out = Command::new("netsh").args(args).output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        r.note(format!("{title}:"));
        for l in text.lines().filter(|l| !l.trim().is_empty()) {
            r.note(format!("  {}", l.trim_end()));
        }
    }
    r.note("--- end of previous settings ---");
}

fn now_str() -> String {
    let out = Command::new("cmd").args(["/C", "echo %date% %time%"]).output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

// ---------------------------------------------------------------- main flow

pub fn run() {
    ui::header("No internet - AUTOMATIC diagnose & repair");
    let mut r = Report::new();
    r.note(format!("WinTool {} - network report", ui::VERSION));
    r.put(Kind::Plain, format!("  Date:   {}", now_str()));
    r.put(Kind::Plain, format!("  System: {}", version_str()));

    r.step("[1/6] Testing the connection");
    let mut p = ui::spin("Testing", probe);
    show(&mut r, &p);

    if !p.ok() {
        safety::checkpoint();
        record_settings(&mut r);

        r.step("[2/6] Network adapters and drivers");
        driver_stage(&mut r);

        r.step("[3/6] Link (cable / Wi-Fi)");
        link_stage(&mut r);

        r.step("[4/6] IP, DNS and internet access");
        p = network_stage(&mut r);
    } else {
        r.put(Kind::Ok, "Internet works - no network repair needed.");
    }

    r.step("[5/6] Captive portal and clock");
    final_stage(&mut r, &p);
    let p = ui::spin("Re-testing", probe);
    show(&mut r, &p);

    r.step("[6/6] Other drivers (Windows Update / Snappy Driver Installer)");
    if p.ok() {
        let outcome = online::run(&mut |k, s| r.put(k, s), false);
        r.restart_needed |= outcome.reboot;
        for d in &outcome.remaining {
            r.advise(d);
        }
        if let Some(dir) = outcome.kept_dir {
            r.manual(format!("Snappy Driver Installer files were kept for a retry in {} - delete with: rmdir /s /q \"{}\"", dir.display(), dir.display()));
        }
    } else {
        r.put(Kind::Info, "Skipped - needs a working internet connection.");
    }

    summary(&mut r, &p);
    for f in drivers::save_text("WinTool-network-report.txt", &r.lines) {
        ui::ok(&format!("Report saved: {}", f.display()));
    }

    if r.restart_needed && ui::ask("Restart now?", false) {
        cmd("Restarting in 10 seconds", "shutdown /r /t 10");
    }
}

fn driver_stage(r: &mut Report) {
    let ads = adapters();
    let probs = drivers::problem_devices(true);

    for a in &ads {
        let st = match a.status {
            2 | 9 => "connected",
            7 => "cable unplugged / Wi-Fi not connected",
            4 => "hardware not present",
            5 => "disabled",
            6 => "hardware malfunction",
            0 if !a.enabled => "disabled",
            _ => "not connected",
        };
        let err = if a.err != 0 { format!(", error code {}", a.err) } else { String::new() };
        r.log(format!("   {} - {st}{err}", a.label()));
    }
    for d in &probs {
        r.log(format!("   PROBLEM: {} - code {}: {}", d.name, d.code, drivers::code_meaning(d.code)));
    }

    let driver_issue = ads.is_empty() || ads.iter().any(|a| a.err != 0 && a.err != 22) || !probs.is_empty();
    if driver_issue {
        if ads.is_empty() {
            r.log("   No physical network adapter found.");
        }
        r.log("   Scanning for hardware changes...");
        if drivers::rescan_devices() {
            r.fix("Hardware rescanned");
        }
        sleep(Duration::from_secs(5));

        // Step 1: drivers from the USB 'Drivers' folder.
        let mut logs = vec![];
        let (found, ok) = drivers::install_from_folder(&mut |s| logs.push(s));
        for l in logs {
            r.log(l);
        }
        if found == 0 {
            r.log("   'Drivers' folder is empty - nothing to install offline.");
        } else if ok > 0 {
            r.fix(format!("{ok} driver(s) installed from the 'Drivers' folder"));
            drivers::rescan_devices();
            sleep(Duration::from_secs(8));
        }

        for a in adapters().iter().filter(|a| a.err != 0 && a.err != 22 && a.err != 28) {
            r.log(format!("   Restarting adapter: {}", a.label()));
            restart_adapter(a);
            r.fix(format!("Adapter restarted: {}", a.label()));
        }
        sleep(Duration::from_secs(5));
    }

    // Enable disabled adapters.
    for a in adapters().iter().filter(|a| a.err == 22 || a.status == 5 || (!a.enabled && a.status == 0)) {
        r.log(format!("   Enabling disabled adapter: {}", a.label()));
        enable_adapter(a);
        r.fix(format!("Adapter enabled: {}", a.label()));
    }

    // Step 2: still no driver → tell exactly what to download and where.
    sleep(Duration::from_secs(3));
    let still: Vec<ProblemDevice> = drivers::problem_devices(true).into_iter().filter(|d| d.code != 22).collect();
    if !still.is_empty() {
        r.log("   The right driver is not available offline - see the download instructions below.");
    }
    for d in &still {
        r.advise(d);
    }
    if adapters().is_empty() && still.is_empty() {
        r.manual(
            "No network adapter is visible at all. On a laptop check the Wi-Fi key / Fn+F2 / airplane mode; \
             in BIOS enable 'Onboard LAN / WLAN'. For a USB Wi-Fi adapter, try another USB port.",
        );
    }
}

fn link_stage(r: &mut Report) {
    let ads = adapters();
    if ads.iter().any(|a| a.connected()) {
        for a in ads.iter().filter(|a| a.connected()) {
            r.log(format!("   Connected: {}", a.label()));
        }
        return;
    }
    for a in ads.iter().filter(|a| a.wireless() && a.err == 0) {
        r.log(format!("   Wi-Fi not connected: {}", a.label()));
        if wifi_connect_saved(r, a) {
            return;
        }
    }
    for a in ads.iter().filter(|a| a.err == 0) {
        if a.wireless() {
            r.manual(format!(
                "Wi-Fi ({}) is not connected to any network. Pick your network from the Wi-Fi list and enter the password. \
                 Empty list: check airplane mode / Wi-Fi key / Fn+F2.",
                a.label()
            ));
        } else if a.status == 7 {
            r.manual(format!(
                "Ethernet cable is unplugged ({}). Check both ends, try another port on the router; the port light must be on.",
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

    // No IP from DHCP
    if !p.tcp && (p.gw.is_none() || p.apipa) {
        r.log("   No IP address (DHCP) - checking the DHCP service and renewing IP...");
        cmd_quiet("sc config Dhcp start= auto");
        cmd_quiet("net start Dhcp");
        cmd_quiet("ipconfig /release");
        cmd_quiet("ipconfig /renew");
        r.fix("DHCP service checked, IP renewed");
        p = probe_after(5);
        if !p.tcp && (p.gw.is_none() || p.apipa) {
            for a in &active {
                restart_adapter(a);
                r.fix(format!("Adapter restarted: {}", a.label()));
            }
            p = probe_after(10);
        }
    }

    // Gateway does not answer
    if !p.tcp && p.gw.is_some() && !p.gw_ok {
        r.log("   Router does not respond - renewing IP...");
        cmd_quiet("ipconfig /release");
        cmd_quiet("ipconfig /renew");
        r.fix("IP renewed");
        p = probe_after(5);
    }

    // Router OK, no internet → proxy / Winsock / TCP-IP
    if !p.tcp && p.gw_ok {
        r.log("   Router works but no internet - checking proxy and network stack...");
        match user_proxy() {
            Some(px) if proxy_reachable(&px) => {
                // Working corporate proxy: direct access may be blocked on purpose - leave it alone.
                r.log(format!("   A working proxy is configured ({px}) - this network goes online through it; left unchanged."));
                r.manual(format!(
                    "This network reaches the internet through a proxy ({px}). If the browser still fails, \
                     ask IT to check the proxy settings / your password."
                ));
                show(r, &p);
                return p;
            }
            Some(px) => {
                r.log(format!("   Proxy ({px}) does not respond - leftover from an old VPN / malware; removing."));
                reset_proxy_settings();
                r.fix(format!("Dead proxy ({px}) disabled"));
            }
            None => {
                reset_proxy_settings();
                r.fix("System proxy reset");
            }
        }
        p = probe_after(2);
        if !p.tcp {
            cmd_quiet("netsh winsock reset");
            cmd_quiet("netsh int ip reset");
            r.fix("Winsock and TCP/IP stack reset (restart required)");
            r.restart_needed = true;
        }
    }

    // Internet OK, DNS broken
    if p.tcp && !p.dns {
        r.log("   Internet works, DNS does not - repairing DNS...");
        cmd_quiet("ipconfig /flushdns");
        cmd_quiet("net stop Dnscache");
        cmd_quiet("net start Dnscache");
        r.fix("DNS cache flushed, DNS client service restarted");
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
            "The router works but there is no internet behind it. Power-cycle the router for 30 seconds; \
             if that does not help, the problem is on the provider side (unpaid bill, line outage) - call your ISP.",
        );
    }
    p
}

fn final_stage(r: &mut Report, p: &Probe) {
    if p.http == Http::Portal {
        r.log("   This network requires a login page (hotel / office / cafe Wi-Fi).");
        let _ = Command::new("cmd").args(["/C", "start", "", "http://www.msftconnecttest.com/redirect"]).status();
        r.manual("Log in / accept the terms on the page that opened in the browser.");
    }
    // A wrong clock breaks every HTTPS site.
    let server = p.server_time.or_else(|| http_probe().1);
    if let Some(srv) = server {
        let local = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let diff = (local - srv).abs();
        if diff > 300 {
            r.log(format!("   The clock is off by {} minutes - HTTPS sites fail because of this.", diff / 60));
            let utc = utc_string(srv);
            ps(&format!(
                "Set-Date -Date ([DateTime]::SpecifyKind([DateTime]::ParseExact('{utc}','yyyy-MM-dd HH:mm:ss',$null),'Utc')).ToLocalTime() | Out-Null"
            ));
            cmd_quiet("net start w32time");
            cmd_quiet("w32tm /resync /rediscover");
            r.fix("Clock corrected from the internet");
        } else {
            r.log("   Clock is correct.");
        }
    }
}

fn summary(r: &mut Report, p: &Probe) {
    let (ok, text) = if p.ok() && r.manual.is_empty() {
        (true, "RESULT: FIXED - the internet works.".to_string())
    } else if p.ok() {
        (true, "RESULT: the internet works - see the remaining steps below.".to_string())
    } else if r.restart_needed {
        (false, "RESULT: repairs applied - RESTART REQUIRED, then run WinTool again.".to_string())
    } else {
        (false, "RESULT: not fixed automatically - follow the steps below.".to_string())
    };
    ui::result(ok, &text);
    r.note("");
    r.note(text);
    if r.restart_needed && p.ok() {
        r.put(Kind::Warn, "Restart the computer to finish the driver installation.");
    }
    let fixed = r.fixed.clone();
    if fixed.is_empty() {
        r.put(Kind::Plain, "\n  Fixes applied: none");
    } else {
        r.put(Kind::Plain, "\n  Fixes applied:");
        for f in fixed {
            r.put(Kind::Plain, format!("    + {f}"));
        }
    }
    let manual = r.manual.clone();
    if !manual.is_empty() {
        r.put(Kind::Plain, "\n  What you need to do:");
        for (i, block) in manual.iter().enumerate() {
            r.put(Kind::Plain, "");
            for (j, line) in block.iter().enumerate() {
                r.put(Kind::Plain, if j == 0 { format!("  {}. {line}", i + 1) } else { format!("     {line}") });
            }
        }
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
}
