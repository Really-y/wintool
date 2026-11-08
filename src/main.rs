//! WinTool — Windows 7 / 8 / 10 / 11 üçün menyulu təmir aləti.
//! Bölmələr: şəbəkə təmiri, sistem təmizliyi, sistem təmiri.

mod autofix;
mod clean;
mod net;
mod repair;
mod util;

use util::*;

fn main() {
    let _ = std::process::Command::new("cmd").args(["/C", "title WinTool"]).status();
    loop {
        header("Ana menyu");
        if !is_admin() {
            println!("\n  DIQQET: admin huququ yoxdur. Sag klik -> 'Run as administrator'.");
        }
        println!();
        println!(" 1. INTERNET ISLEMIR - avtomatik tap ve hell et");
        println!(" 2. Sebeke temiri, el ile (DNS, IP, Wi-Fi ...)");
        println!(" 3. Sistem temizliyi (temp, update cache, zibil qutusu)");
        println!(" 4. Sistem temiri   (SFC, DISM, Windows Update, printer ...)");
        println!(" 0. Cixis");
        match prompt("\nSecim: ").as_str() {
            "1" => {
                autofix::run();
                pause();
            }
            "2" => net::menu(),
            "3" => clean::menu(),
            "4" => repair::menu(),
            "0" | "q" => break,
            _ => {}
        }
    }
}
