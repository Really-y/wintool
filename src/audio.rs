//! No sound — automatic diagnosis and repair.
//!
//! Order: sound devices & drivers → Windows Audio services → playback devices (enable,
//! unmute, volume) → test tone the user confirms → other outputs (HDMI etc.) → restart the
//! audio stack → reinstall the sound driver → remaining manual steps.
//! Checks are language-independent: WMI codes, the Core Audio API (COM through
//! PowerShell) and winmm for the test tone.

use crate::drivers::{self, ProblemDevice};
use crate::online;
use crate::report::Report;
use crate::safety;
use crate::ui::{self, Kind};
use crate::util::*;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

const MEDIA_CLASS: &str = "{4d36e96c-e325-11ce-bfc1-08002be10318}";

// ---------------------------------------------------------------- sound devices (WMI)

struct SoundDev {
    name: String,
    id: String,
    class: String,
    code: u32,
}

impl SoundDev {
    fn physical(&self) -> bool {
        let u = self.id.to_uppercase();
        !u.starts_with("ROOT\\") && !u.starts_with("SWD\\")
    }
    /// Sound device proper (codec / USB audio), not the controller it hangs off.
    fn media(&self) -> bool {
        self.class.eq_ignore_ascii_case(MEDIA_CLASS) || drivers::is_audio_hwid(&self.id)
    }
    fn as_problem(&self) -> ProblemDevice {
        ProblemDevice { name: self.name.clone(), hwid: self.id.clone(), code: self.code, likely_network: false }
    }
}

/// Physical sound devices and audio controllers, including driverless ones
/// (HD Audio codecs, PCI class 04xx multimedia audio).
fn sound_devices() -> Vec<SoundDev> {
    let out = ps(r#"$ErrorActionPreference='SilentlyContinue'
$media = '{4d36e96c-e325-11ce-bfc1-08002be10318}'
$endpoint = '{c166523c-fe0c-4a94-a586-f1a80cfbbf3e}'
Get-WmiObject Win32_PnPEntity | Where-Object {
  $_.ClassGuid -ne $endpoint -and (
    $_.ClassGuid -eq $media -or $_.PNPDeviceID -like 'HDAUDIO\FUNC_01*' -or
    (($_.CompatibleID -join ' ') -match 'PCI\\CC_040[13]') -or $_.Name -match 'audio' )
} | ForEach-Object { "{0}|{1}|{2}|{3}" -f $_.Name,$_.PNPDeviceID,$_.ClassGuid,$_.ConfigManagerErrorCode }"#);
    out.lines()
        .filter_map(|l| {
            let mut f = l.trim().rsplitn(4, '|');
            let code = f.next()?.parse().unwrap_or(0);
            let class = f.next()?.to_string();
            let id = f.next()?.to_string();
            let name = f.next()?.to_string();
            Some(SoundDev { name: if name.is_empty() { "Unknown audio device".into() } else { name }, id, class, code })
        })
        .filter(SoundDev::physical)
        .collect()
}

/// Sound devices that still have a Device Manager error (14 = restart pending, not broken).
fn broken_sound() -> Vec<SoundDev> {
    sound_devices().into_iter().filter(|d| d.code != 0 && d.code != 14).collect()
}

// ---------------------------------------------------------------- services

/// Start order: MMCSS (Windows 7/8 only) → AudioEndpointBuilder → Audiosrv.
const SERVICES: [&str; 3] = ["MMCSS", "AudioEndpointBuilder", "Audiosrv"];

struct Svc {
    name: String,
    state: String,
    start: String,
}

fn audio_services() -> Vec<Svc> {
    let out = ps(r#"$ErrorActionPreference='SilentlyContinue'
Get-WmiObject Win32_Service | Where-Object { @('MMCSS','AudioEndpointBuilder','Audiosrv') -contains $_.Name } |
 ForEach-Object { "{0}|{1}|{2}" -f $_.Name,$_.State,$_.StartMode }"#);
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.trim().split('|').collect();
            (f.len() == 3).then(|| Svc { name: f[0].into(), state: f[1].into(), start: f[2].into() })
        })
        .collect()
}

// ---------------------------------------------------------------- Core Audio (COM via PowerShell)

/// C# 2.0 (PowerShell 2.0 on Windows 7 compiles with it): Core Audio endpoint list,
/// default-device volume / mute, and the IPolicyConfig calls the Sound control panel
/// itself uses to enable a device and make it the default (Windows 7 – 11).
const CORE_AUDIO_CS: &str = r#"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace WinToolAudio
{
    [StructLayout(LayoutKind.Sequential)]
    public struct PropKey { public Guid fmt; public int pid; }

    [StructLayout(LayoutKind.Explicit)]
    public struct PropVar
    {
        [FieldOffset(0)] public short vt;
        [FieldOffset(8)] public IntPtr p;
        [FieldOffset(16)] public long pad;
    }

    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore
    {
        [PreserveSig] int GetCount(out int c);
        [PreserveSig] int GetAt(int i, out PropKey k);
        [PreserveSig] int GetValue(ref PropKey k, out PropVar v);
    }

    [ComImport, Guid("5CDF2C82-841E-4546-9722-0CF74078229A"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IAudioEndpointVolume
    {
        void R0(); void R1(); void R2(); void R3();
        [PreserveSig] int SetMasterVolumeLevelScalar(float level, ref Guid ctx);
        void R5();
        [PreserveSig] int GetMasterVolumeLevelScalar(out float level);
        void R7(); void R8(); void R9(); void R10();
        [PreserveSig] int SetMute([MarshalAs(UnmanagedType.Bool)] bool mute, ref Guid ctx);
        [PreserveSig] int GetMute([MarshalAs(UnmanagedType.Bool)] out bool mute);
    }

    [ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDevice
    {
        [PreserveSig] int Activate(ref Guid iid, int ctx, IntPtr prm, out IAudioEndpointVolume vol);
        [PreserveSig] int OpenPropertyStore(int access, out IPropertyStore store);
        [PreserveSig] int GetId([MarshalAs(UnmanagedType.LPWStr)] out string id);
        [PreserveSig] int GetState(out int state);
    }

    [ComImport, Guid("0BD7A1BE-7A1A-44DB-8397-CC5392387B5E"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDeviceCollection
    {
        [PreserveSig] int GetCount(out int n);
        [PreserveSig] int Item(int i, out IMMDevice dev);
    }

    [ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IMMDeviceEnumerator
    {
        [PreserveSig] int EnumAudioEndpoints(int flow, int mask, out IMMDeviceCollection devs);
        [PreserveSig] int GetDefaultAudioEndpoint(int flow, int role, out IMMDevice dev);
    }

    [ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")]
    class MMDeviceEnumerator { }

    [ComImport, Guid("F8679F50-850A-41CF-9C72-430F290290C8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPolicyConfig
    {
        void R0(); void R1(); void R2(); void R3(); void R4(); void R5(); void R6(); void R7(); void R8(); void R9();
        [PreserveSig] int SetDefaultEndpoint([MarshalAs(UnmanagedType.LPWStr)] string id, int role);
        [PreserveSig] int SetEndpointVisibility([MarshalAs(UnmanagedType.LPWStr)] string id, int visible);
    }

    [ComImport, Guid("870AF99C-171D-4F9E-AF0D-E63DF40C2BC9")]
    class PolicyConfigClient { }

    public static class Audio
    {
        static IMMDeviceEnumerator Enumerator() { return (IMMDeviceEnumerator)(new MMDeviceEnumerator()); }

        static string Id(IMMDevice d)
        {
            string id;
            return d.GetId(out id) == 0 && id != null ? id : "";
        }

        static string Name(IMMDevice d)
        {
            IPropertyStore s;
            if (d.OpenPropertyStore(0, out s) != 0 || s == null) return "";
            PropKey k = new PropKey();
            k.fmt = new Guid("A45C254E-DF1C-4EFD-8020-67D146A850E0");
            k.pid = 14;
            PropVar v;
            if (s.GetValue(ref k, out v) != 0 || v.vt != 31 || v.p == IntPtr.Zero) return "";
            return Marshal.PtrToStringUni(v.p);
        }

        static IMMDevice Default(IMMDeviceEnumerator e)
        {
            IMMDevice d;
            return e.GetDefaultAudioEndpoint(0, 0, out d) == 0 ? d : null;
        }

        static IAudioEndpointVolume Volume(IMMDevice d)
        {
            Guid iid = typeof(IAudioEndpointVolume).GUID;
            IAudioEndpointVolume v;
            return d.Activate(ref iid, 23, IntPtr.Zero, out v) == 0 ? v : null;
        }

        // E|state|isDefault|id|name per playback endpoint, then V|volume%|muted for the default one.
        public static string[] Scan()
        {
            List<string> o = new List<string>();
            IMMDeviceEnumerator e = Enumerator();
            IMMDevice def = Default(e);
            string defId = def == null ? "" : Id(def);
            IMMDeviceCollection c;
            int n = 0;
            if (e.EnumAudioEndpoints(0, 11, out c) == 0 && c != null) c.GetCount(out n);
            for (int i = 0; i < n; i++)
            {
                IMMDevice d;
                if (c.Item(i, out d) != 0 || d == null) continue;
                int st;
                d.GetState(out st);
                string id = Id(d);
                o.Add("E|" + st + "|" + (id == defId ? "1" : "0") + "|" + id + "|" + Name(d).Replace("|", "/"));
            }
            IAudioEndpointVolume v = def == null ? null : Volume(def);
            if (v != null)
            {
                float lvl;
                bool muted;
                v.GetMasterVolumeLevelScalar(out lvl);
                v.GetMute(out muted);
                o.Add("V|" + (int)Math.Round(lvl * 100) + "|" + (muted ? "1" : "0"));
            }
            o.Add("DONE");
            return o.ToArray();
        }

        // Unmutes the default output; raises its volume to target% when below min%.
        // OK|wasMuted|raised
        public static string Unmute(int min, int target)
        {
            IMMDevice def = Default(Enumerator());
            IAudioEndpointVolume v = def == null ? null : Volume(def);
            if (v == null) return "ERR|no default playback device";
            Guid g = Guid.Empty;
            float lvl;
            bool muted;
            v.GetMasterVolumeLevelScalar(out lvl);
            v.GetMute(out muted);
            if (muted) v.SetMute(false, ref g);
            bool raise = lvl * 100 < min;
            if (raise) v.SetMasterVolumeLevelScalar(target / 100f, ref g);
            return "OK|" + (muted ? "1" : "0") + "|" + (raise ? "1" : "0");
        }

        public static string SetDefault(string id)
        {
            IPolicyConfig p = (IPolicyConfig)(new PolicyConfigClient());
            int hr = 0;
            for (int role = 0; role < 3; role++)
            {
                int h = p.SetDefaultEndpoint(id, role);
                if (h != 0) hr = h;
            }
            return hr == 0 ? "OK" : "ERR|0x" + hr.ToString("X8");
        }

        public static string Enable(string id)
        {
            IPolicyConfig p = (IPolicyConfig)(new PolicyConfigClient());
            int hr = p.SetEndpointVisibility(id, 1);
            return hr == 0 ? "OK" : "ERR|0x" + hr.ToString("X8");
        }
    }
}
"#;

/// Compiles the Core Audio helper and runs one call, e.g. `[WinToolAudio.Audio]::Scan()`.
/// Output is UTF-8 so device names in any language survive.
fn core_audio(call: &str) -> String {
    let script = format!(
        "$ErrorActionPreference = 'Stop'\n\
         try {{ [Console]::OutputEncoding = New-Object Text.UTF8Encoding $false }} catch {{}}\n\
         try {{\nAdd-Type -TypeDefinition @'\n{CORE_AUDIO_CS}\n'@\n{call}\n}} catch {{ 'ERR|' + $_.Exception.Message }}"
    );
    ps(&script)
}

/// "OK…" → Ok(rest), "ERR|msg" or nothing → Err(msg).
fn core_result(out: &str) -> Result<String, String> {
    let last = out.lines().map(|l| l.trim().trim_start_matches('\u{feff}')).rfind(|l| !l.is_empty()).unwrap_or("");
    if let Some(e) = last.strip_prefix("ERR|") {
        Err(e.to_string())
    } else if let Some(rest) = last.strip_prefix("OK") {
        Ok(rest.trim_start_matches('|').to_string())
    } else {
        Err("no response from PowerShell".into())
    }
}

#[derive(Clone)]
struct Endpoint {
    state: u32,
    default: bool,
    id: String,
    name: String,
}

struct Mixer {
    volume: u32,
    muted: bool,
}

fn state_text(s: u32) -> &'static str {
    match s {
        1 => "active",
        2 => "DISABLED",
        4 => "not present",
        8 => "UNPLUGGED (nothing in the jack)",
        _ => "unknown",
    }
}

fn parse_scan(out: &str) -> Result<(Vec<Endpoint>, Option<Mixer>), String> {
    let mut eps = vec![];
    let mut mixer = None;
    let mut done = false;
    for l in out.lines().map(|l| l.trim().trim_start_matches('\u{feff}')) {
        if let Some(e) = l.strip_prefix("ERR|") {
            return Err(e.to_string());
        }
        if let Some(rest) = l.strip_prefix("E|") {
            let f: Vec<&str> = rest.splitn(4, '|').collect();
            if f.len() == 4 {
                let name = if f[3].trim().is_empty() { "Unnamed output".to_string() } else { f[3].trim().to_string() };
                eps.push(Endpoint { state: f[0].parse().unwrap_or(0), default: f[1] == "1", id: f[2].to_string(), name });
            }
        } else if let Some(rest) = l.strip_prefix("V|") {
            if let Some((v, m)) = rest.split_once('|') {
                mixer = Some(Mixer { volume: v.parse().unwrap_or(0), muted: m == "1" });
            }
        } else if l == "DONE" {
            done = true;
        }
    }
    if done { Ok((eps, mixer)) } else { Err("no response from PowerShell".into()) }
}

fn scan_endpoints() -> Result<(Vec<Endpoint>, Option<Mixer>), String> {
    parse_scan(&core_audio("[WinToolAudio.Audio]::Scan()"))
}

fn set_default(id: &str) -> bool {
    core_result(&core_audio(&format!("[WinToolAudio.Audio]::SetDefault({})", ps_quote(id)))).is_ok()
}

// ---------------------------------------------------------------- test tone (winmm)

#[link(name = "winmm")]
extern "system" {
    fn PlaySoundW(psz_sound: *const u16, hmod: *mut std::ffi::c_void, fdw_sound: u32) -> i32;
}

/// Three rising notes (C5 E5 G5), 16-bit mono PCM WAV in memory.
fn tone_wav() -> Vec<u8> {
    const RATE: u32 = 44100;
    let mut pcm: Vec<u8> = vec![];
    for (freq, secs) in [(523.25f32, 0.3f32), (659.25, 0.3), (783.99, 0.6)] {
        let n = (RATE as f32 * secs) as usize;
        let fade = RATE as f32 * 0.02;
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            let env = (i.min(n - i) as f32 / fade).min(1.0);
            let s = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.45 * env;
            pcm.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
        }
    }
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(&pcm);
    w
}

/// Plays the tone on the current default output. False = Windows could not play it at all.
fn play_tone() -> bool {
    const SND_NODEFAULT: u32 = 0x2;
    const SND_MEMORY: u32 = 0x4;
    let wav = tone_wav();
    unsafe { PlaySoundW(wav.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_NODEFAULT) != 0 }
}

/// Plays the tone and asks the user. Only an explicit y / n counts; anything else replays.
fn heard(r: &mut Report, output: &str) -> bool {
    loop {
        r.put(Kind::Info, format!("Playing a test tone on: {output}"));
        if !play_tone() {
            r.put(Kind::Fail, "Windows could not play the test tone on this output.");
            return false;
        }
        let a = prompt("\n  ? Did you hear the test tone?  y = yes, n = no, r = play again: ").to_lowercase();
        match a.as_str() {
            "y" | "yes" => {
                r.note("  User heard the test tone.");
                return true;
            }
            "n" | "no" => {
                r.note("  User did NOT hear the test tone.");
                return false;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- main flow

pub fn run() {
    ui::header("No sound - AUTOMATIC diagnose & repair");
    let mut r = Report::new();
    r.note(format!("WinTool {} - sound report  (by {}, {})", ui::VERSION, ui::AUTHOR, ui::SITE));
    r.put(Kind::Plain, format!("  Date:   {}", now_str()));
    r.put(Kind::Plain, format!("  System: {}", version_str()));

    r.step("[1/5] Sound devices and drivers");
    devices_stage(&mut r);

    r.step("[2/5] Windows Audio services");
    services_stage(&mut r);

    r.step("[3/5] Playback devices, mute and volume");
    let active = endpoints_stage(&mut r);

    r.step("[4/5] Test tone");
    let works = test_stage(&mut r, &active);

    r.step("[5/5] Result");
    if !works {
        r.manual_block(vec![
            "Check the hardware side:".into(),
            "- speakers: power on, volume knob up, plug in the GREEN (rear) jack - or try headphones".into(),
            "- monitor / TV over HDMI: its own volume; or pick your speakers in Sound settings -> Playback".into(),
            "- Bluetooth headset: remove it in Bluetooth settings and pair it again".into(),
            "- only one app is silent: open the Volume Mixer (sndvol) and unmute that app".into(),
            "- still nothing: get the sound driver from the PC maker's support page, put it into the".into(),
            "  'Drivers' folder next to WinTool.exe and run this repair again".into(),
        ]);
    }
    let (ok, text) = if works && r.manual.is_empty() {
        (true, "RESULT: sound works.")
    } else if works {
        (true, "RESULT: sound works - see the remaining steps below.")
    } else if r.restart_needed {
        (false, "RESULT: repairs applied - RESTART REQUIRED, then run WinTool again.")
    } else {
        (false, "RESULT: not fixed automatically - follow the steps below.")
    };
    ui::result(ok, text);
    r.note("");
    r.note(text);
    if r.restart_needed && works {
        r.put(Kind::Warn, "Restart the computer to finish the driver installation.");
    }
    r.print_steps();
    r.save("WinTool-sound-report.txt");

    if !works && ui::ask("Open the Sound control panel now?", false) {
        let _ = Command::new(sys("control")).arg("mmsys.cpl").spawn();
    }
    if r.restart_needed && ui::ask("Restart now?", false) {
        cmd("Restarting in 10 seconds", "shutdown /r /t 10");
    }
}

fn devices_stage(r: &mut Report) {
    let devs = ui::spin("Scanning sound devices", sound_devices);
    if devs.is_empty() {
        r.put(Kind::Warn, "Windows sees no sound hardware at all.");
        r.manual(
            "No sound hardware is visible. In BIOS / UEFI enable 'HD Audio' / 'Onboard audio'; \
             a USB headset or sound card: try another USB port. Monitor speakers (HDMI) need the graphics driver.",
        );
    }
    for d in &devs {
        if d.code == 0 {
            r.log(format!("   {} - OK", d.name));
        } else {
            r.log(format!("   PROBLEM: {} - code {}: {}", d.name, d.code, drivers::code_meaning(d.code)));
        }
    }
    if devs.iter().any(|d| d.code == 14) {
        r.restart_needed = true;
    }
    let broken: Vec<&SoundDev> = devs.iter().filter(|d| d.code != 0 && d.code != 14).collect();
    if broken.is_empty() {
        return;
    }
    safety::checkpoint();

    // Disabled → enable; loaded but failing → restart; no driver → driver steps below.
    for d in &broken {
        match d.code {
            22 => {
                if drivers::enable_device(&d.id) {
                    r.fix(format!("Sound device enabled: {}", d.name));
                }
            }
            1 | 28 => {}
            _ => {
                if drivers::restart_device(&d.id) {
                    r.fix(format!("Sound device restarted: {}", d.name));
                }
            }
        }
    }
    let still = ui::spin("Scanning for hardware changes", || {
        drivers::rescan_devices();
        sleep(Duration::from_secs(3));
        broken_sound()
    });
    if still.is_empty() {
        return;
    }

    // Offline: the 'Drivers' folder next to WinTool.exe.
    let mut logs = vec![];
    let (found, ok) = drivers::install_from_folder(&mut |s| logs.push(s));
    for l in logs {
        r.log(l);
    }
    if ok > 0 {
        r.fix(format!("{ok} driver(s) installed from the 'Drivers' folder"));
        r.restart_needed = true;
    } else if found == 0 {
        r.log("   'Drivers' folder is empty - nothing to install offline.");
    }
    let still = ui::spin("Scanning for hardware changes", || {
        drivers::rescan_devices();
        sleep(Duration::from_secs(5));
        broken_sound()
    });
    if still.is_empty() {
        return;
    }

    // Online: Windows Update → Microsoft Update Catalog → Snappy Driver Installer.
    if ui::spin("Checking the internet connection", internet_ok) {
        let outcome = online::run(&mut |k, s| r.put(k, s), false);
        r.restart_needed |= outcome.reboot;
        if let Some(dir) = outcome.kept_dir {
            r.manual(format!(
                "Snappy Driver Installer files were kept for a retry in {} - delete with: rmdir /s /q \"{}\"",
                dir.display(),
                dir.display()
            ));
        }
    } else {
        r.put(Kind::Warn, "No internet - online driver search skipped (main menu -> 1 fixes the network).");
    }
    for d in ui::spin("Checking sound devices", broken_sound) {
        r.advise(&d.as_problem(), Some("Sound device without a working driver:"));
    }
}

fn services_stage(r: &mut Report) {
    let svcs = ui::spin("Checking services", audio_services);
    if !svcs.iter().any(|s| s.name.eq_ignore_ascii_case("Audiosrv")) {
        r.put(Kind::Fail, "The Windows Audio service (Audiosrv) is missing.");
        r.manual(
            "The Windows Audio service is missing - system files are damaged. \
             Run main menu -> System repair -> FULL repair (DISM + SFC), then restart.",
        );
        return;
    }
    if svcs.iter().any(|s| !s.start.eq_ignore_ascii_case("Auto")) {
        safety::checkpoint();
    }
    for name in SERVICES {
        let Some(s) = svcs.iter().find(|s| s.name.eq_ignore_ascii_case(name)) else { continue };
        if !s.start.eq_ignore_ascii_case("Auto") && cmd_quiet(&format!("sc config {name} start= auto")) {
            r.fix(format!("{name}: startup type set to Automatic (was {})", s.start));
        }
        if s.state.eq_ignore_ascii_case("Running") {
            r.log(format!("   {name} - running"));
        } else if cmd_quiet(&format!("net start {name}")) {
            r.fix(format!("{name} service started (was {})", s.state));
        } else {
            r.put(Kind::Fail, format!("{name} service could not be started"));
        }
    }
}

/// Lists playback devices, enables disabled ones, unmutes / raises the volume of the
/// default one. Returns the active playback devices.
fn endpoints_stage(r: &mut Report) -> Vec<Endpoint> {
    let (mut eps, mut mixer) = match ui::spin("Reading playback devices", scan_endpoints) {
        Ok(x) => x,
        Err(e) => {
            r.put(Kind::Warn, format!("Could not read the playback devices: {e}"));
            return vec![];
        }
    };
    if eps.is_empty() {
        r.log("   No playback devices.");
    }
    for e in &eps {
        r.log(format!("   {}{} - {}", if e.default { "[default] " } else { "" }, e.name, state_text(e.state)));
    }

    let disabled: Vec<Endpoint> = eps.iter().filter(|e| e.state == 2).cloned().collect();
    if !disabled.is_empty() {
        for e in &disabled {
            match core_result(&core_audio(&format!("[WinToolAudio.Audio]::Enable({})", ps_quote(&e.id)))) {
                Ok(_) => r.fix(format!("Playback device enabled: {}", e.name)),
                Err(m) => r.put(Kind::Fail, format!("Could not enable {}: {m}", e.name)),
            }
        }
        if let Ok((e2, m2)) = ui::spin("Reading playback devices", scan_endpoints) {
            eps = e2;
            mixer = m2;
        }
    }

    let active: Vec<Endpoint> = eps.iter().filter(|e| e.state == 1).cloned().collect();
    if active.is_empty() {
        let unplugged: Vec<&str> = eps.iter().filter(|e| e.state == 8).map(|e| e.name.as_str()).collect();
        if !unplugged.is_empty() {
            r.manual(format!(
                "Nothing is plugged in ({}). Plug the speakers / headphones into the GREEN jack (rear panel) \
                 or the headphone jack; front-panel jacks only work if the case cable is connected to the motherboard.",
                unplugged.join(", ")
            ));
        } else {
            r.manual(
                "Windows has no working playback device - the sound driver is missing or broken (see the driver steps). \
                 For monitor / TV speakers over HDMI, install the graphics driver.",
            );
        }
        return active;
    }

    if let Some(m) = mixer {
        r.log(format!("   Volume: {}%{}", m.volume, if m.muted { "  (MUTED)" } else { "" }));
        if m.muted || m.volume < 10 {
            match core_result(&core_audio("[WinToolAudio.Audio]::Unmute(10, 50)")) {
                Ok(flags) => {
                    let f: Vec<&str> = flags.split('|').collect();
                    if f.first() == Some(&"1") {
                        r.fix("Sound was muted - unmuted");
                    }
                    if f.get(1) == Some(&"1") {
                        r.fix(format!("Volume was {}% - raised to 50%", m.volume));
                    }
                }
                Err(e) => r.put(Kind::Fail, format!("Could not change mute / volume: {e}")),
            }
        }
    }
    active
}


/// Stops the audio services, restarts every working sound device, starts the services.
fn restart_audio_stack() -> usize {
    cmd_quiet("net stop Audiosrv /y");
    cmd_quiet("net stop AudioEndpointBuilder /y");
    let n = sound_devices().iter().filter(|d| d.media() && d.code == 0).filter(|d| drivers::restart_device(&d.id)).count();
    for s in SERVICES {
        cmd_quiet(&format!("net start {s}"));
    }
    sleep(Duration::from_secs(3));
    n
}

/// Device Manager → "Uninstall device" + "Scan for hardware changes": Windows installs
/// the best driver from its driver store again. Windows 10 2004+ (`pnputil /remove-device`).
fn reinstall_sound_devices() -> usize {
    cmd_quiet("net stop Audiosrv /y");
    let pnp = sys("pnputil");
    let n = sound_devices()
        .iter()
        .filter(|d| d.media())
        .filter(|d| Command::new(&pnp).arg("/remove-device").arg(&d.id).output().map(|o| o.status.success()).unwrap_or(false))
        .count();
    drivers::rescan_devices();
    sleep(Duration::from_secs(8));
    for s in SERVICES {
        cmd_quiet(&format!("net start {s}"));
    }
    sleep(Duration::from_secs(3));
    n
}

fn test_stage(r: &mut Report, active: &[Endpoint]) -> bool {
    if active.is_empty() {
        r.put(Kind::Fail, "No working playback device - test tone skipped.");
        return false;
    }
    let default = active.iter().find(|e| e.default).unwrap_or(&active[0]);
    if heard(r, &default.name) {
        return true;
    }

    // a) The sound may be going to another output (monitor over HDMI, a headset ...).
    let others: Vec<&Endpoint> = active.iter().filter(|e| e.id != default.id).collect();
    if !others.is_empty() {
        r.log("   Trying the other playback devices (sound may be going to the monitor / HDMI)...");
        for e in &others {
            if !set_default(&e.id) {
                r.put(Kind::Fail, format!("Could not switch to {}", e.name));
                continue;
            }
            if heard(r, &e.name) {
                r.fix(format!("Default playback device switched to: {}", e.name));
                return true;
            }
        }
        if set_default(&default.id) {
            r.log(format!("   Default playback device restored: {}", default.name));
        }
    }

    // b) Restart the audio services and the sound devices.
    let n = ui::spin("Restarting the audio services and sound devices", restart_audio_stack);
    r.fix(format!("Windows Audio services restarted, {n} sound device(s) restarted"));
    if heard(r, &default.name) {
        return true;
    }

    // c) Reinstall the sound driver from the driver store.
    let (ma, _, build) = win_version();
    if ma >= 10 && build >= 19041 {
        if ui::ask("Reinstall the sound driver (the device is removed and detected again)?", true) {
            safety::checkpoint();
            let n = ui::spin("Reinstalling the sound driver", reinstall_sound_devices);
            if n > 0 {
                r.fix(format!("Sound driver reinstalled on {n} device(s)"));
            } else {
                r.put(Kind::Fail, "The sound driver could not be reinstalled.");
            }
            let now = ui::spin("Reading playback devices", scan_endpoints).map(|(e, _)| e).unwrap_or_default();
            match now.iter().find(|e| e.default && e.state == 1) {
                Some(e) => {
                    if heard(r, &e.name) {
                        return true;
                    }
                }
                None => r.put(Kind::Fail, "No playback device after the reinstall - a restart may bring it back."),
            }
            r.restart_needed = true;
        }
    } else {
        r.manual(
            "Reinstall the sound driver: Device Manager -> 'Sound, video and game controllers' -> right-click the \
             sound device -> Uninstall -> Action -> Scan for hardware changes.",
        );
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_consistent() {
        let w = tone_wav();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        let riff = u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize;
        let data = u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize;
        assert_eq!(riff + 8, w.len());
        assert_eq!(data + 44, w.len());
    }

    #[test]
    fn parses_core_audio_scan() {
        let out = "\u{feff}E|1|1|{0.0.0.00000000}.{aaa}|Speakers (Realtek(R) Audio)\r\n\
                   E|8|0|{0.0.0.00000000}.{bbb}|Headphones\r\nE|2|0|{0.0.0.00000000}.{ccc}|\r\nV|7|1\r\nDONE\r\n";
        let (eps, mixer) = parse_scan(out).unwrap();
        assert_eq!(eps.len(), 3);
        assert!(eps[0].default && eps[0].state == 1);
        assert_eq!(eps[0].name, "Speakers (Realtek(R) Audio)");
        assert_eq!(eps[2].name, "Unnamed output");
        let m = mixer.unwrap();
        assert_eq!((m.volume, m.muted), (7, true));
        assert!(parse_scan("ERR|boom").is_err());
        assert!(parse_scan("").is_err());
    }

    #[test]
    fn core_results() {
        assert_eq!(core_result("noise\nOK|1|0\n"), Ok("1|0".to_string()));
        assert_eq!(core_result("OK\n"), Ok(String::new()));
        assert_eq!(core_result("ERR|0x80070490"), Err("0x80070490".to_string()));
        assert!(core_result("").is_err());
    }
}
