//! Terminal UI: colors, boxes, banner art, arrow-key menus, status badges, spinner.
//!
//! Built on crossterm, which drives the legacy Windows 7 console through the Win32
//! console API and modern consoles through VT sequences. Box drawing uses Unicode on
//! Windows 8+ and plain ASCII on Windows 7 (raster fonts).

use crate::art;
use crate::util;
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, queue};
use std::io::{stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const AUTHOR: &str = "Yusif Ezizov | AZEDEV";
pub const SITE: &str = "azedev.com";
const BOX_W: usize = 72;

/// Truecolor art when the console understands VT sequences (Windows 10+);
/// the 16-color art on the Windows 7 legacy console.
fn truecolor() -> bool {
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| !util::is_win7_or_older() && crossterm::ansi_support::supports_ansi())
}

/// Unicode box drawing on Win8+, ASCII on the Win7 legacy console.
fn fancy() -> bool {
    static F: OnceLock<bool> = OnceLock::new();
    *F.get_or_init(|| !util::is_win7_or_older())
}

struct Glyphs {
    tl: char,
    tr: char,
    bl: char,
    br: char,
    h: char,
    v: char,
}

fn glyphs() -> Glyphs {
    if fancy() {
        Glyphs { tl: '┌', tr: '┐', bl: '└', br: '┘', h: '─', v: '│' }
    } else {
        Glyphs { tl: '+', tr: '+', bl: '+', br: '+', h: '-', v: '|' }
    }
}

pub fn clear() {
    let _ = execute!(stdout(), ResetColor, Clear(ClearType::All), MoveTo(0, 0));
}

// ---------------------------------------------------------------- status lines

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Ok,
    Fail,
    Warn,
    Info,
    Plain,
}

fn badge(kind: Kind) -> (&'static str, Color) {
    match kind {
        Kind::Ok => ("[ OK ]", Color::Green),
        Kind::Fail => ("[FAIL]", Color::Red),
        Kind::Warn => ("[ !! ]", Color::Yellow),
        Kind::Info => ("[ .. ]", Color::Cyan),
        Kind::Plain => ("", Color::Reset),
    }
}

/// Plain-text form for report files.
pub fn plain(kind: Kind, text: &str) -> String {
    match badge(kind).0 {
        "" => text.to_string(),
        b => format!("  {b} {text}"),
    }
}

/// Prints a colored status line.
pub fn line(kind: Kind, text: &str) {
    let (b, c) = badge(kind);
    let mut out = stdout();
    if b.is_empty() {
        let _ = queue!(out, Print(text), Print("\n"));
    } else {
        let _ = queue!(out, Print("  "), SetForegroundColor(c), Print(b), ResetColor, Print(" "), Print(text), Print("\n"));
    }
    let _ = out.flush();
}

pub fn ok(t: &str) {
    line(Kind::Ok, t)
}
pub fn fail(t: &str) {
    line(Kind::Fail, t)
}
pub fn warn(t: &str) {
    line(Kind::Warn, t)
}
pub fn info(t: &str) {
    line(Kind::Info, t)
}

/// Highlighted step title, e.g. "[2/6] Checking drivers...".
pub fn step(t: &str) {
    let mut out = stdout();
    let _ = queue!(out, Print("\n"), SetForegroundColor(Color::Cyan), SetAttribute(Attribute::Bold), Print(t), SetAttribute(Attribute::Reset), ResetColor, Print("\n"));
    let _ = out.flush();
}

/// Big colored result banner.
pub fn result(success: bool, t: &str) {
    let g = glyphs();
    let c = if success { Color::Green } else { Color::Yellow };
    let bar: String = std::iter::repeat_n(g.h, BOX_W).collect();
    let mut out = stdout();
    let _ = queue!(out, Print("\n"), SetForegroundColor(c), Print(format!("{}{bar}{}\n", g.tl, g.tr)));
    let _ = queue!(out, Print(format!("{} {:<w$}{}\n", g.v, t, g.v, w = BOX_W - 1)));
    let _ = queue!(out, Print(format!("{}{bar}{}\n", g.bl, g.br)), ResetColor);
    let _ = out.flush();
}

/// Screen header for an action: clears the screen and draws a title box.
pub fn header(title: &str) {
    clear();
    let g = glyphs();
    let bar: String = std::iter::repeat_n(g.h, BOX_W).collect();
    let status = status_line();
    let mut out = stdout();
    let _ = queue!(out, SetForegroundColor(Color::Cyan), Print(format!("{}{bar}{}\n", g.tl, g.tr)));
    let _ = queue!(out, Print(g.v), SetForegroundColor(Color::White), SetAttribute(Attribute::Bold));
    let _ = queue!(out, Print(format!(" {:<w$}", format!("WinTool  |  {title}"), w = BOX_W - 1)), SetAttribute(Attribute::Reset));
    let _ = queue!(out, SetForegroundColor(Color::Cyan), Print(format!("{}\n", g.v)));
    let sig = format!("by {AUTHOR}");
    let gap = (BOX_W - 1).saturating_sub(status.len() + sig.len() + 1);
    let _ = queue!(out, Print(g.v), SetForegroundColor(Color::DarkGrey), Print(format!(" {status}{}", " ".repeat(gap))));
    let _ = queue!(out, SetForegroundColor(Color::Green), Print(&sig), Print(" "));
    let _ = queue!(out, SetForegroundColor(Color::Cyan), Print(format!("{}\n{}{bar}{}\n", g.v, g.bl, g.br)), ResetColor);
    let _ = out.flush();
}

/// Shown when WinTool exits.
pub fn goodbye() {
    clear();
    let mut out = stdout();
    let _ = queue!(out, Print("\n  WinTool v"), Print(VERSION), Print("  -  "));
    let _ = queue!(out, SetForegroundColor(Color::Green), SetAttribute(Attribute::Bold), Print(AUTHOR), SetAttribute(Attribute::Reset));
    let _ = queue!(out, SetForegroundColor(Color::DarkGrey), Print(format!("  -  {SITE}\n\n")), ResetColor);
    let _ = out.flush();
}

fn status_line() -> String {
    format!("{}  |  admin: {}", util::version_str(), if util::is_admin() { "yes" } else { "NO" })
}

// ---------------------------------------------------------------- banner art

fn palette(idx: u8) -> Color {
    match idx {
        0 => Color::Black,
        1 => Color::DarkBlue,
        2 => Color::DarkGreen,
        3 => Color::DarkCyan,
        4 => Color::DarkRed,
        5 => Color::DarkMagenta,
        6 => Color::DarkYellow,
        7 => Color::Grey,
        8 => Color::DarkGrey,
        9 => Color::Blue,
        10 => Color::Green,
        11 => Color::Cyan,
        12 => Color::Red,
        13 => Color::Magenta,
        14 => Color::Yellow,
        _ => Color::White,
    }
}

/// Color of the art pixel at (x, y), or None for transparent.
fn art_px(y: usize, x: usize) -> Option<Color> {
    if truecolor() {
        let hex = art::RGB_PIXELS.get(y)?.get(x * 6..x * 6 + 6)?;
        let c = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Color::Rgb { r: c(0)?, g: c(2)?, b: c(4)? })
    } else {
        let d = (*art::PIXELS.get(y)?.as_bytes().get(x)? as char).to_digit(16)?;
        Some(palette(d as u8))
    }
}

fn art_width() -> usize {
    if truecolor() { art::RGB_WIDTH } else { art::WIDTH }
}

pub fn art_rows() -> usize {
    let px = if truecolor() { art::RGB_PIXELS.len() } else { art::PIXELS.len() };
    px.div_ceil(2)
}

/// Draws one text row of the banner (two pixel rows) at the cursor position.
fn art_row(out: &mut impl Write, row: usize) {
    for x in 0..art_width() {
        let _ = match (art_px(row * 2, x), art_px(row * 2 + 1, x)) {
            (None, None) => queue!(out, ResetColor, Print(' ')),
            (Some(t), None) => queue!(out, ResetColor, SetForegroundColor(t), Print('▀')),
            (None, Some(b)) => queue!(out, ResetColor, SetForegroundColor(b), Print('▄')),
            (Some(t), Some(b)) => queue!(out, SetForegroundColor(t), SetBackgroundColor(b), Print('▀')),
        };
    }
    let _ = queue!(out, ResetColor);
}

// ---------------------------------------------------------------- menus

enum Row {
    Title(String),
    Dim(String),
    Warn(String),
    Item { key: char, label: String, selected: bool },
    Blank,
}

fn draw_row(out: &mut impl Write, r: &Row) {
    let _ = match r {
        Row::Title(t) => queue!(out, SetForegroundColor(Color::White), SetAttribute(Attribute::Bold), Print(t), SetAttribute(Attribute::Reset), ResetColor),
        Row::Dim(t) => queue!(out, SetForegroundColor(Color::DarkGrey), Print(t), ResetColor),
        Row::Warn(t) => queue!(out, SetForegroundColor(Color::Yellow), Print(t), ResetColor),
        Row::Item { key, label, selected: true } => queue!(
            out,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Cyan),
            Print(format!(" {key}. {label} ")),
            ResetColor
        ),
        Row::Item { key, label, selected: false } => {
            queue!(out, SetForegroundColor(Color::Cyan), Print(format!(" {key}.")), ResetColor, Print(format!(" {label} ")))
        }
        Row::Blank => Ok(()),
    };
}

fn draw_menu(title: &str, items: &[&str], back: &str, sel: usize, with_art: bool) {
    let mut rows = vec![Row::Title(format!("WinTool v{VERSION}  |  {title}"))];
    if with_art {
        rows.push(Row::Dim(format!("by {AUTHOR}  -  {SITE}")));
    }
    rows.push(Row::Dim(status_line()));
    if !util::is_admin() {
        rows.push(Row::Warn("Not admin! Right-click -> Run as admin".into()));
    }
    rows.push(Row::Blank);
    for (i, it) in items.iter().enumerate() {
        rows.push(Row::Item { key: char::from_digit(i as u32 + 1, 10).unwrap_or('?'), label: it.to_string(), selected: i == sel });
    }
    rows.push(Row::Item { key: '0', label: back.to_string(), selected: false });
    rows.push(Row::Blank);
    let arrows = if fancy() { "↑/↓" } else { "Up/Down" };
    rows.push(Row::Dim(format!("{arrows} move  Enter ok  1-{} keys  Esc {back}", items.len().min(9))));

    let mut out = stdout();
    let _ = queue!(out, ResetColor, Clear(ClearType::All));
    if with_art {
        let total = art_rows().max(rows.len());
        let top_pad = art_rows().saturating_sub(rows.len()) / 2;
        for y in 0..total {
            let _ = queue!(out, MoveTo(0, y as u16));
            if y < art_rows() {
                art_row(&mut out, y);
            }
            if y >= top_pad {
                if let Some(r) = rows.get(y - top_pad) {
                    let _ = queue!(out, MoveTo(art_width() as u16 + 3, y as u16));
                    draw_row(&mut out, r);
                }
            }
        }
        let _ = queue!(out, MoveTo(0, total as u16));
    } else {
        for (y, r) in rows.iter().enumerate() {
            let _ = queue!(out, MoveTo(1, y as u16 + 1));
            draw_row(&mut out, r);
        }
    }
    let _ = out.flush();
}

/// Arrow-key menu. Returns the chosen item index, or None for Esc / 0 (back / exit).
/// Number keys select an item immediately.
pub fn menu(title: &str, items: &[&str], back: &str, with_art: bool) -> Option<usize> {
    let mut sel = 0usize;
    let raw = terminal::enable_raw_mode().is_ok();
    let choice = loop {
        draw_menu(title, items, back, sel, with_art);
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Up => sel = if sel == 0 { items.len() - 1 } else { sel - 1 },
                KeyCode::Down | KeyCode::Tab => sel = (sel + 1) % items.len(),
                KeyCode::Home => sel = 0,
                KeyCode::End => sel = items.len() - 1,
                KeyCode::Enter => break Some(sel),
                KeyCode::Esc | KeyCode::Char('0') | KeyCode::Char('q') => break None,
                KeyCode::Char(c) => {
                    if let Some(n) = c.to_digit(10).map(|n| n as usize) {
                        if (1..=items.len()).contains(&n) {
                            break Some(n - 1);
                        }
                    }
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break None,
        }
    };
    if raw {
        let _ = terminal::disable_raw_mode();
    }
    clear();
    choice
}

// ---------------------------------------------------------------- spinner

/// Runs `work` on a worker thread while showing a spinner with elapsed time.
/// `work` must not print to the console.
pub fn spin<T: Send>(msg: &str, work: impl FnOnce() -> T + Send) -> T {
    const FRAMES: [char; 4] = ['|', '/', '-', '\\'];
    let done = AtomicBool::new(false);
    let start = Instant::now();
    std::thread::scope(|s| {
        let handle = s.spawn(|| {
            let r = work();
            done.store(true, Ordering::SeqCst);
            r
        });
        let mut i = 0;
        let mut out = stdout();
        while !done.load(Ordering::SeqCst) {
            let secs = start.elapsed().as_secs();
            let _ = queue!(
                out,
                Print("\r  "),
                SetForegroundColor(Color::Cyan),
                Print(FRAMES[i % FRAMES.len()]),
                ResetColor,
                Print(format!(" {msg} ({}:{:02})  ", secs / 60, secs % 60))
            );
            let _ = out.flush();
            std::thread::sleep(Duration::from_millis(120));
            i += 1;
        }
        let blank = " ".repeat(msg.len() + 20);
        let _ = queue!(out, Print(format!("\r{blank}\r")));
        let _ = out.flush();
        match handle.join() {
            Ok(r) => r,
            Err(e) => std::panic::resume_unwind(e),
        }
    })
}

// ---------------------------------------------------------------- prompts

/// Yes/no question with a default answer (Enter = default).
pub fn ask(question: &str, default_yes: bool) -> bool {
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    let mut out = stdout();
    let _ = queue!(out, SetForegroundColor(Color::Yellow), Print(format!("\n  ? {question} {hint}: ")), ResetColor);
    let _ = out.flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    match s.trim().to_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
}
