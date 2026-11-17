//! WinTool — menu-driven repair tool for Windows 7 / 8 / 10 / 11.
//! Sections: FULL CHECK (finds and fixes everything, unattended), automatic network repair,
//! automatic sound repair, virus scan, manual network repair, cleanup, system repair.

mod art;
mod audio;
mod autofix;
mod catalog;
mod clean;
mod drivers;
mod fullcheck;
mod net;
mod online;
mod repair;
mod report;
mod safety;
mod scan;
mod ui;
mod util;

const ITEMS: [&str; 7] = [
    "FULL CHECK - find & fix everything",
    "NO INTERNET - auto diagnose & repair",
    "NO SOUND - auto diagnose & repair",
    "VIRUS SCAN - WinTool + Defender",
    "Manual network repair (DNS, IP, Wi-Fi)",
    "System cleanup (temp, update cache ...)",
    "System repair (SFC, DISM, drivers ...)",
];

fn main() {
    util::safe_cwd();
    let _ = std::process::Command::new(util::sys("cmd")).args(["/C", "title WinTool"]).status();
    while let Some(i) = ui::menu("Main menu", &ITEMS, "Exit", true) {
        match i {
            0 => {
                fullcheck::run();
                util::pause();
            }
            1 => {
                autofix::run();
                util::pause();
            }
            2 => {
                audio::run();
                util::pause();
            }
            3 => scan::menu(),
            4 => net::menu(),
            5 => clean::menu(),
            _ => repair::menu(),
        }
    }
    ui::goodbye();
}
