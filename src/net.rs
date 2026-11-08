//! Manual network repair: DNS, IP, Winsock/TCP-IP reset, proxy, DNS servers, Wi-Fi profiles, diagnostics.

use crate::safety;
use crate::ui::{self, Kind};
use crate::util::*;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::Duration;

const ITEMS: [&str; 7] = [
    "FULL network reset (everything, then restart)",
    "Flush DNS cache",
    "Renew IP address (release / renew)",
    "Connection test - where is the problem?",
    "Change DNS servers (Cloudflare / Google / auto)",
    "Reset proxy settings",
    "Wi-Fi profiles (view / delete)",
];

pub fn menu() {
    while let Some(i) = ui::menu("Network repair (manual)", &ITEMS, "Back", false) {
        ui::header(ITEMS[i]);
        match i {
            0 => full_repair(),
            1 => {
                cmd("Flushing DNS cache", "ipconfig /flushdns");
            }
            2 => renew_ip(),
            3 => diagnose(),
            4 => change_dns(),
            5 => {
                safety::checkpoint();
                reset_proxy();
            }
            _ => wifi(),
        }
        pause();
    }
}

fn renew_ip() {
    cmd("Releasing IP (internet drops for a few seconds)", "ipconfig /release");
    cmd("Requesting a new IP", "ipconfig /renew");
}

fn reset_proxy() {
    cmd("Resetting system (WinHTTP) proxy", "netsh winhttp reset proxy");
    cmd(
        "Disabling user proxy",
        r#"reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings" /v ProxyEnable /t REG_DWORD /d 0 /f"#,
    );
}

fn full_repair() {
    println!("\nThis resets network settings to defaults:");
    println!("DNS cache, IP, Winsock, TCP/IP stack and proxy. A restart is required afterwards.");
    if !confirm("Continue?") {
        return;
    }
    safety::checkpoint();
    cmd("Flushing DNS cache", "ipconfig /flushdns");
    renew_ip();
    cmd("Re-registering DNS", "ipconfig /registerdns");
    cmd("Resetting Winsock catalog", "netsh winsock reset");
    cmd("Resetting TCP/IPv4 stack", "netsh int ip reset");
    cmd("Resetting TCP/IPv6 stack", "netsh int ipv6 reset");
    reset_proxy();
    println!("\nDone. Restart the computer to apply the changes.");
    if confirm("Restart now?") {
        cmd("Restarting in 10 seconds", "shutdown /r /t 10");
    }
}

/// Default gateway from the `route print` table (language-independent).
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

fn mark(ok: bool) -> Kind {
    if ok { Kind::Ok } else { Kind::Fail }
}

fn diagnose() {
    let (stack, gw, gw_ok, net, dns) = ui::spin("Testing", || {
        let stack = cmd_quiet("ping -n 1 -w 1000 127.0.0.1");
        let gw = default_gateway();
        let gw_ok = gw.as_deref().map(|g| cmd_quiet(&format!("ping -n 2 -w 1500 {g}"))).unwrap_or(false);
        let net = tcp_ok("1.1.1.1:443") || tcp_ok("8.8.8.8:443");
        let dns = ("www.google.com", 443).to_socket_addrs().map(|mut i| i.next().is_some()).unwrap_or(false);
        (stack, gw, gw_ok, net, dns)
    });
    println!();
    ui::line(mark(stack), "TCP/IP stack (127.0.0.1)");
    match &gw {
        Some(g) => ui::line(mark(gw_ok), &format!("Modem / router ({g})")),
        None => ui::line(Kind::Fail, "Modem / router - no default gateway"),
    }
    ui::line(mark(net), "Internet (direct IP connection)");
    ui::line(mark(dns), "DNS (name -> IP resolution)");

    println!("\nVerdict:");
    if !stack {
        println!(" TCP/IP stack is broken -> option 1 (FULL reset) + restart.");
    } else if gw.is_none() {
        println!(" The adapter is not connected to a network (no IP / gateway).");
        println!(" Check cable / Wi-Fi, then option 3 (renew IP). If that fails: driver - use the automatic repair.");
    } else if !gw_ok && !net {
        println!(" Cannot reach the router. Power-cycle it, check the cable, then option 3 (renew IP).");
    } else if !net {
        println!(" Router works but no internet -> problem at the provider / router.");
        println!(" If you use a proxy / VPN, also try option 6 (reset proxy).");
    } else if !dns {
        println!(" Internet works, DNS does not -> option 5 (set DNS to 1.1.1.1) and option 2 (flush DNS).");
    } else {
        println!(" Everything looks fine. If a site still fails: browser / proxy / antivirus.");
    }
}

fn ask_interface() -> Option<String> {
    run("Network adapters", "netsh", &["interface", "show", "interface"]);
    let name = prompt("\nType the adapter name (last column, e.g. Wi-Fi / Ethernet), empty = cancel: ");
    if name.is_empty() || name.contains('"') {
        return None;
    }
    Some(name)
}

fn change_dns() {
    let Some(ifc) = ask_interface() else { return };
    println!("\n 1. Cloudflare (1.1.1.1 / 1.0.0.1) - fastest");
    println!(" 2. Google     (8.8.8.8 / 8.8.4.4)");
    println!(" 3. Automatic  (DHCP, from the router)");
    let (a, b) = match prompt("Choice: ").as_str() {
        "1" => ("1.1.1.1", "1.0.0.1"),
        "2" => ("8.8.8.8", "8.8.4.4"),
        "3" => {
            cmd("Setting DNS to automatic", &format!(r#"netsh interface ip set dns name="{ifc}" source=dhcp"#));
            cmd("Flushing DNS cache", "ipconfig /flushdns");
            return;
        }
        _ => return,
    };
    safety::checkpoint();
    cmd(&format!("Primary DNS: {a}"), &format!(r#"netsh interface ip set dns name="{ifc}" source=static addr={a}"#));
    cmd(&format!("Secondary DNS: {b}"), &format!(r#"netsh interface ip add dns name="{ifc}" addr={b} index=2"#));
    cmd("Flushing DNS cache", "ipconfig /flushdns");
}

fn wifi() {
    run("Saved Wi-Fi profiles", "netsh", &["wlan", "show", "profiles"]);
    let name = prompt("\nType a profile name to delete (empty = back): ");
    if name.is_empty() || name.contains('"') {
        return;
    }
    if confirm(&format!("Delete '{name}'?")) {
        cmd("Deleting profile", &format!(r#"netsh wlan delete profile name="{name}""#));
    }
}
