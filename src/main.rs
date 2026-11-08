//! WinTool — menu-driven repair tool for Windows 7 / 8 / 10 / 11.
//! Sections: automatic network repair, manual network repair, cleanup, system repair.

mod art;
mod autofix;
mod clean;
mod drivers;
mod net;
mod online;
mod repair;
mod safety;
mod ui;
mod util;

const ITEMS: [&str; 4] = [
    "NO INTERNET - auto diagnose & repair",
    "Manual network repair (DNS, IP, Wi-Fi)",
    "System cleanup (temp, update cache ...)",
    "System repair (SFC, DISM, drivers ...)",
];

fn main() {
    let _ = std::process::Command::new("cmd").args(["/C", "title WinTool"]).status();
    while let Some(i) = ui::menu("Main menu", &ITEMS, "Exit", true) {
        match i {
            0 => {
                autofix::run();
                util::pause();
            }
            1 => net::menu(),
            2 => clean::menu(),
            _ => repair::menu(),
        }
    }
    ui::goodbye();
}
