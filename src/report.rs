//! Repair report shared by the automatic network and sound repairs: colored lines on
//! screen, a plain copy for the report file, applied fixes and remaining manual steps.

use crate::drivers::{self, ProblemDevice};
use crate::ui::{self, Kind};

pub struct Report {
    pub lines: Vec<String>,
    pub fixed: Vec<String>,
    pub manual: Vec<Vec<String>>,
    advised: Vec<String>,
    /// Hardware problems software cannot fix: red on screen, at the very top of the report file.
    pub alerts: Vec<String>,
    pub restart_needed: bool,
}

impl Report {
    pub fn new() -> Self {
        Self { lines: vec![], fixed: vec![], manual: vec![], advised: vec![], alerts: vec![], restart_needed: false }
    }
    /// Colored line on screen, plain line in the report file.
    pub fn put(&mut self, kind: Kind, s: impl Into<String>) {
        let s = s.into();
        ui::line(kind, &s);
        self.lines.push(ui::plain(kind, &s));
    }
    /// Plain line; lines starting with "PROBLEM" are highlighted.
    pub fn log(&mut self, s: impl Into<String>) {
        let s = s.into();
        let kind = if s.trim_start().starts_with("PROBLEM") { Kind::Warn } else { Kind::Plain };
        self.put(kind, s);
    }
    pub fn step(&mut self, s: &str) {
        ui::step(s);
        self.lines.push(String::new());
        self.lines.push(s.to_string());
    }
    /// Report file only (not printed).
    pub fn note(&mut self, s: impl Into<String>) {
        self.lines.push(s.into());
    }
    pub fn fix(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.put(Kind::Ok, s.clone());
        self.fixed.push(s);
    }
    /// Hardware warning (shown in red now, repeated first in the summary and the report file).
    pub fn alert(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.put(Kind::Fail, format!("HARDWARE: {s}"));
        self.alerts.push(s);
    }
    pub fn manual(&mut self, s: impl Into<String>) {
        self.manual.push(vec![s.into()]);
    }
    pub fn manual_block(&mut self, block: Vec<String>) {
        self.manual.push(block);
    }
    /// Download advice for a device, once per hardware ID; `intro` goes above the block.
    pub fn advise(&mut self, d: &ProblemDevice, intro: Option<&str>) {
        if self.advised.iter().any(|h| h.eq_ignore_ascii_case(&d.hwid)) {
            return;
        }
        self.advised.push(d.hwid.clone());
        let mut block = drivers::advice(d);
        if let Some(t) = intro {
            block.insert(0, t.to_string());
        }
        self.manual.push(block);
    }

    /// Prints (and records) the applied fixes and the numbered manual steps.
    pub fn print_steps(&mut self) {
        let alerts = self.alerts.clone();
        if !alerts.is_empty() {
            self.put(Kind::Fail, "\n  !!! HARDWARE WARNINGS - software cannot fix these !!!");
            for a in alerts {
                self.put(Kind::Fail, format!("    ! {a}"));
            }
        }
        let fixed = self.fixed.clone();
        if fixed.is_empty() {
            self.put(Kind::Plain, "\n  Fixes applied: none");
        } else {
            self.put(Kind::Plain, "\n  Fixes applied:");
            for f in fixed {
                self.put(Kind::Plain, format!("    + {f}"));
            }
        }
        let manual = self.manual.clone();
        if !manual.is_empty() {
            self.put(Kind::Plain, "\n  What you need to do:");
            for (i, block) in manual.iter().enumerate() {
                self.put(Kind::Plain, "");
                for (j, line) in block.iter().enumerate() {
                    self.put(Kind::Plain, if j == 0 { format!("  {}. {line}", i + 1) } else { format!("     {line}") });
                }
            }
        }
    }

    /// Saves the report to the Desktop and next to WinTool.exe.
    pub fn save(&self, name: &str) {
        let mut out = vec![];
        if !self.alerts.is_empty() {
            out.push("!!! HARDWARE WARNINGS - software cannot fix these !!!".to_string());
            out.extend(self.alerts.iter().map(|a| format!("  ! {a}")));
            out.push(String::new());
        }
        out.extend(self.lines.iter().cloned());
        for f in drivers::save_text(name, &out) {
            ui::ok(&format!("Report saved: {}", f.display()));
        }
    }
}
