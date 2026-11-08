//! Şəbəkə təmiri: DNS, IP, Winsock/TCP-IP reset, proxy, DNS server, Wi-Fi profilləri, diaqnostika.

use crate::util::*;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::Duration;

pub fn menu() {
    loop {
        header("Sebeke temiri");
        println!(" 1. TAM sebeke temiri (hamisi birden, sonra restart)");
        println!(" 2. DNS cache temizle");
        println!(" 3. IP unvanini yenile (release / renew)");
        println!(" 4. Internet testi - problem haradadir?");
        println!(" 5. DNS serverini deyis (Cloudflare / Google / avtomatik)");
        println!(" 6. Proxy ayarlarini sifirla");
        println!(" 7. Wi-Fi profilleri (bax / sil)");
        println!(" 0. Geri");
        match prompt("\nSecim: ").as_str() {
            "1" => full_repair(),
            "2" => {
                cmd("DNS cache temizlenir", "ipconfig /flushdns");
            }
            "3" => renew_ip(),
            "4" => diagnose(),
            "5" => change_dns(),
            "6" => reset_proxy(),
            "7" => wifi(),
            "0" => return,
            _ => continue,
        }
        pause();
    }
}

fn renew_ip() {
    cmd("IP buraxilir (internet bir neche saniye kesilecek)", "ipconfig /release");
    cmd("Yeni IP alinir", "ipconfig /renew");
}

fn reset_proxy() {
    cmd("Sistem (WinHTTP) proxy sifirlanir", "netsh winhttp reset proxy");
    cmd(
        "Istifadeci proxy-si sondurulur",
        r#"reg add "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings" /v ProxyEnable /t REG_DWORD /d 0 /f"#,
    );
}

fn full_repair() {
    println!("\nBu emeliyyat sebeke ayarlarini standarta qaytarir:");
    println!("DNS cache, IP, Winsock, TCP/IP stack ve proxy. Sonda restart lazimdir.");
    if !confirm("Davam edilsin?") {
        return;
    }
    cmd("DNS cache temizlenir", "ipconfig /flushdns");
    renew_ip();
    cmd("DNS qeydiyyati yenilenir", "ipconfig /registerdns");
    cmd("Winsock katalogu sifirlanir", "netsh winsock reset");
    cmd("TCP/IPv4 stack sifirlanir", "netsh int ip reset");
    cmd("TCP/IPv6 stack sifirlanir", "netsh int ipv6 reset");
    reset_proxy();
    println!("\nHazirdir. Deyisikliklerin tetbiqi ucun komputer yeniden basladilmalidir.");
    if confirm("Indi restart edilsin?") {
        cmd("10 saniyeye restart", "shutdown /r /t 10");
    }
}

/// `route print` cədvəlindən default gateway — dildən asılı deyil.
fn default_gateway() -> Option<String> {
    let out = Command::new("route").args(["print", "0.0.0.0"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().find_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        (t.len() >= 3 && t[0] == "0.0.0.0" && t[1] == "0.0.0.0" && t[2].contains('.'))
            .then(|| t[2].to_string())
    })
}

fn tcp_ok(addr: &str) -> bool {
    addr.parse::<SocketAddr>()
        .map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(4)).is_ok())
        .unwrap_or(false)
}

fn diagnose() {
    println!("\nTest edilir...\n");

    let stack = cmd_quiet("ping -n 1 -w 1000 127.0.0.1");
    println!(" [{}] TCP/IP stack (127.0.0.1)", mark(stack));

    let gw = default_gateway();
    let gw_ok = gw.as_deref().map(|g| cmd_quiet(&format!("ping -n 2 -w 1500 {g}"))).unwrap_or(false);
    match &gw {
        Some(g) => println!(" [{}] Modem / router ({g})", mark(gw_ok)),
        None => println!(" [{}] Modem / router - default gateway yoxdur", mark(false)),
    }

    let net = tcp_ok("1.1.1.1:443") || tcp_ok("8.8.8.8:443");
    println!(" [{}] Internet (IP ile qosulma)", mark(net));

    let dns = ("www.google.com", 443).to_socket_addrs().map(|mut i| i.next().is_some()).unwrap_or(false);
    println!(" [{}] DNS (ad -> IP cevrilmesi)", mark(dns));

    println!("\nNetice:");
    if !stack {
        println!(" TCP/IP stack xarabdir -> menyuda 1 (TAM temir) + restart.");
    } else if gw.is_none() {
        println!(" Adapter sebekeye qosulmayib (IP/gateway yoxdur).");
        println!(" Kabel / Wi-Fi qosulmasini yoxla, sonra 3 (IP yenile). Olmazsa: driver.");
    } else if !gw_ok && !net {
        println!(" Modeme catmir. Modemi sondur-yandir, kabeli yoxla, sonra 3 (IP yenile).");
    } else if !net {
        println!(" Modem isleyir, amma internet yoxdur -> problem provayderde / modemde.");
        println!(" Proxy/VPN varsa 6 (proxy sifirla) da yoxla.");
    } else if !dns {
        println!(" Internet var, DNS islemir -> 5 (DNS-i 1.1.1.1 et) ve 2 (DNS cache).");
    } else {
        println!(" Her sey qaydasindadir. Sayt acilmirsa: brauzer / proxy / antivirus.");
    }
}

fn mark(ok: bool) -> &'static str {
    if ok { "OK" } else { "XX" }
}

fn ask_interface() -> Option<String> {
    run("Sebeke adapterleri", "netsh", &["interface", "show", "interface"]);
    let name = prompt("\nAdapterin adini yaz (sonuncu sutun, mes. Wi-Fi / Ethernet), bos = legv: ");
    if name.is_empty() || name.contains('"') {
        return None;
    }
    Some(name)
}

fn change_dns() {
    let Some(ifc) = ask_interface() else { return };
    println!("\n 1. Cloudflare (1.1.1.1 / 1.0.0.1) - en suretli");
    println!(" 2. Google     (8.8.8.8 / 8.8.4.4)");
    println!(" 3. Avtomatik  (DHCP, modemden)");
    let (a, b) = match prompt("Secim: ").as_str() {
        "1" => ("1.1.1.1", "1.0.0.1"),
        "2" => ("8.8.8.8", "8.8.4.4"),
        "3" => {
            cmd("DNS avtomatik edilir", &format!(r#"netsh interface ip set dns name="{ifc}" source=dhcp"#));
            cmd("DNS cache temizlenir", "ipconfig /flushdns");
            return;
        }
        _ => return,
    };
    cmd(
        &format!("Esas DNS: {a}"),
        &format!(r#"netsh interface ip set dns name="{ifc}" source=static addr={a}"#),
    );
    cmd(
        &format!("Ehtiyat DNS: {b}"),
        &format!(r#"netsh interface ip add dns name="{ifc}" addr={b} index=2"#),
    );
    cmd("DNS cache temizlenir", "ipconfig /flushdns");
}

fn wifi() {
    run("Saxlanmis Wi-Fi profilleri", "netsh", &["wlan", "show", "profiles"]);
    let name = prompt("\nSilmek ucun profil adini yaz (bos = geri): ");
    if name.is_empty() || name.contains('"') {
        return;
    }
    if confirm(&format!("'{name}' silinsin?")) {
        cmd("Profil silinir", &format!(r#"netsh wlan delete profile name="{name}""#));
    }
}
