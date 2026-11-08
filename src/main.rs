//! WinTool — Windows 7 / 8 / 10 / 11 üçün menyulu təmir aləti.
//! Bölmələr: şəbəkə təmiri, sistem təmizliyi, sistem təmiri.

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
        println!(" 1. Sebeke temiri   (internet yoxdur, DNS, Wi-Fi ...)");
        println!(" 2. Sistem temizliyi (temp, update cache, zibil qutusu)");
        println!(" 3. Sistem temiri   (SFC, DISM, Windows Update, printer ...)");
        println!(" 0. Cixis");
        match prompt("\nSecim: ").as_str() {
            "1" => net::menu(),
            "2" => clean::menu(),
            "3" => repair::menu(),
            "0" | "q" => break,
            _ => {}
        }
    }
}
