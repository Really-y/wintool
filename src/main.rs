//! WinTool — menu-driven repair tool for Windows 7 / 8 / 10 / 11.
//! Sections: automatic network repair, manual network repair, cleanup, system repair.

mod autofix;
mod clean;
mod drivers;
mod net;
mod repair;
mod util;

use util::*;

fn main() {
    let _ = std::process::Command::new("cmd").args(["/C", "title WinTool"]).status();
    loop {
        header("Main menu");
        if !is_admin() {
            println!("\n  WARNING: not running as administrator. Right-click -> 'Run as administrator'.");
        }
        println!();
        println!(" 1. NO INTERNET - automatic diagnose & repair");
        println!(" 2. Network repair, manual (DNS, IP, Wi-Fi ...)");
        println!(" 3. System cleanup (temp, update cache, recycle bin)");
        println!(" 4. System repair (SFC, DISM, Windows Update, drivers ...)");
        println!(" 0. Exit");
        match prompt("\nChoice: ").as_str() {
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
