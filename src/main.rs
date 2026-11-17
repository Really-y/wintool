//! WinTool — menu-driven repair tool for Windows 7 / 8 / 10 / 11.
//! Sections: automatic network repair, automatic sound repair, manual network repair,
//! cleanup, system repair.

mod art;
mod audio;
mod autofix;
mod catalog;
mod clean;
mod drivers;
mod net;
mod online;
mod repair;
mod report;
mod safety;
mod ui;
mod util;

const ITEMS: [&str; 5] = [
    "NO INTERNET - auto diagnose & repair",
    "NO SOUND - auto diagnose & repair",
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
                autofix::run();
                util::pause();
            }
            1 => {
                audio::run();
                util::pause();
            }
            2 => net::menu(),
            3 => clean::menu(),
            _ => repair::menu(),
        }
    }
    ui::goodbye();
}
