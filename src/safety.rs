//! System Restore point — offered once per session, before the first system change.

use crate::ui;
use crate::util::ps;
use std::sync::atomic::{AtomicBool, Ordering};

static ASKED: AtomicBool = AtomicBool::new(false);

/// Windows 8+ silently refuses a second restore point within 24 h; the frequency value
/// is set to 0 for the duration of the call and restored afterwards.
const SCRIPT: &str = r#"$k = 'HKLM:\Software\Microsoft\Windows NT\CurrentVersion\SystemRestore'
$n = 'SystemRestorePointCreationFrequency'
$old = (Get-ItemProperty -Path $k -Name $n -ErrorAction SilentlyContinue).$n
try {
  New-ItemProperty -Path $k -Name $n -Value 0 -PropertyType DWord -Force -ErrorAction Stop | Out-Null
  Checkpoint-Computer -Description 'WinTool' -RestorePointType 'MODIFY_SETTINGS' -ErrorAction Stop
  'OK'
} catch {
  'ERR|' + $_.Exception.Message
} finally {
  if ($old -eq $null) { Remove-ItemProperty -Path $k -Name $n -ErrorAction SilentlyContinue }
  else { New-ItemProperty -Path $k -Name $n -Value $old -PropertyType DWord -Force | Out-Null }
}"#;

/// Asks (once per session, default yes) and creates a restore point.
pub fn checkpoint() {
    if ASKED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !ui::ask("Create a System Restore point before making changes?", true) {
        ui::info("Restore point skipped.");
        return;
    }
    let out = ui::spin("Creating restore point", || ps(SCRIPT));
    let last = out.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
    if last == "OK" {
        ui::ok("Restore point 'WinTool' created.");
    } else {
        let msg = last.strip_prefix("ERR|").unwrap_or("no response from PowerShell");
        ui::warn(&format!("Restore point not created: {msg}"));
        ui::info("System Protection may be off (System Properties -> System Protection). Continuing.");
    }
}
