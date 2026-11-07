//! Device Control: sound output and volume, microphone mute and external monitor brightness (DDC/CI).
use crate::app::{app, State, WM_DEVICES};
use crate::core::layout::Rect;
use crate::core::settings::ModuleId;
use crate::gfx::*;
use crate::tray::{icon_from_pixels, icon_size};
use windows::core::*;
use windows::Win32::Devices::Display::*;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const UID_MIC: u32 = 31;

/// A program that plays sound, with its own volume (the Volume Mixer's rows).
#[derive(Clone, Debug)]
pub struct Session { pub pid: u32, pub name: String, pub volume: f32, pub muted: bool }

#[derive(Clone, Debug, PartialEq)]
pub struct Device { pub id: String, pub name: String, pub default: bool }

#[derive(Clone, Debug)]
pub struct Monitor { pub name: String, pub handle: isize, pub brightness: u32, pub min: u32, pub max: u32 }

#[derive(Default)]
pub struct Devices {
    enumerator: Option<IMMDeviceEnumerator>,
    client: Option<IMMNotificationClient>,
    pub outputs: Vec<Device>,
    pub inputs: Vec<Device>,
    pub volume: f32,
    pub muted: bool,
    pub mic_muted: bool,
    pub monitors: Vec<Monitor>,
    pub sessions: Vec<Session>,
    pub theme_switches: u32,
    pub notice: Option<String>,
    pub switches: u32,
}

impl Devices {
    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::json!({ "outputs": self.outputs.len(), "inputs": self.inputs.len(), "volume": self.volume, "micMuted": self.mic_muted, "monitors": self.monitors.iter().map(|m| (m.name.clone(), m.brightness)).collect::<Vec<_>>(), "switches": self.switches, "listening": self.client.is_some(), "sessions": self.sessions.iter().map(|s| (s.name.clone(), s.volume)).collect::<Vec<_>>(), "themeSwitches": self.theme_switches })
    }
}

/// The interface Windows' own Sound settings use to choose the default device. Undocumented but unchanged since
/// Windows 7; only the one method used here is declared, after placeholders for the ten before it.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn _0(&self) -> HRESULT; fn _1(&self) -> HRESULT; fn _2(&self) -> HRESULT; fn _3(&self) -> HRESULT; fn _4(&self) -> HRESULT;
    fn _5(&self) -> HRESULT; fn _6(&self) -> HRESULT; fn _7(&self) -> HRESULT; fn _8(&self) -> HRESULT; fn _9(&self) -> HRESULT;
    fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
}
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[implement(IMMNotificationClient)]
struct Watcher(isize);

#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for Watcher_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> Result<()> { self.post(1) }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> Result<()> { self.post(1) }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> Result<()> { self.post(0) }
    fn OnDefaultDeviceChanged(&self, _: EDataFlow, _: ERole, _: &PCWSTR) -> Result<()> { self.post(0) }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> Result<()> { Ok(()) }
}

impl Watcher_Impl {
    /// Called on an audio thread: the work happens on the UI thread. `1` means a device appeared.
    fn post(&self, appeared: usize) -> Result<()> {
        unsafe { let _ = PostMessageW(Some(HWND(self.0 as *mut _)), WM_DEVICES, WPARAM(appeared), LPARAM(0)); }
        Ok(())
    }
}

fn enumerator() -> Option<IMMDeviceEnumerator> { app().devices.enumerator.clone() }

unsafe fn list(e: &IMMDeviceEnumerator, flow: EDataFlow) -> Vec<Device> {
    let default = e.GetDefaultAudioEndpoint(flow, eMultimedia).ok().and_then(|d| d.GetId().ok()).map(|p| p.to_string().unwrap_or_default());
    let mut out = vec![];
    let Ok(coll) = e.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else { return out };
    for i in 0..coll.GetCount().unwrap_or(0) {
        let Ok(d) = coll.Item(i) else { continue };
        let id = d.GetId().ok().and_then(|p| { let s = p.to_string().ok(); CoTaskMemFree(Some(p.0 as *const _)); s }).unwrap_or_default();
        let name = d.OpenPropertyStore(STGM_READ).ok().and_then(|s| s.GetValue(&PKEY_Device_FriendlyName).ok()).map(|v| v.to_string()).unwrap_or_else(|| "Audio device".into());
        out.push(Device { default: Some(&id) == default.as_ref(), id, name });
    }
    out
}

unsafe fn endpoint_volume(flow: EDataFlow) -> Option<IAudioEndpointVolume> {
    let d = enumerator()?.GetDefaultAudioEndpoint(flow, eMultimedia).ok()?;
    d.Activate(CLSCTX_ALL, None).ok()
}

pub fn start() -> std::result::Result<(), String> {
    let a = app();
    unsafe {
        let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("Windows audio is not available: {e}"))?;
        let w: IMMNotificationClient = Watcher(a.main.0 as isize).into();
        let _ = e.RegisterEndpointNotificationCallback(&w);
        a.devices.enumerator = Some(e);
        a.devices.client = Some(w);
    }
    refresh();
    read_monitors();
    unsafe { SetTimer(Some(a.main), crate::app::TIMER_THEME, 60_000, None); }
    theme_tick();
    Ok(())
}

pub fn stop() {
    let a = app();
    unsafe { if let (Some(e), Some(c)) = (&a.devices.enumerator, &a.devices.client) { let _ = e.UnregisterEndpointNotificationCallback(c); } }
    release_monitors();
    unsafe { let _ = KillTimer(Some(a.main), crate::app::TIMER_THEME); }
    a.tray.remove(UID_MIC);
    a.devices = Devices::default();
}

pub fn refresh() {
    let a = app();
    let Some(e) = enumerator() else { return };
    unsafe {
        a.devices.outputs = list(&e, eRender);
        a.devices.inputs = list(&e, eCapture);
        if let Some(v) = endpoint_volume(eRender) {
            a.devices.volume = v.GetMasterVolumeLevelScalar().unwrap_or(0.0);
            a.devices.muted = v.GetMute().map(|b| b.as_bool()).unwrap_or(false);
        }
        a.devices.mic_muted = endpoint_volume(eCapture).and_then(|v| v.GetMute().ok()).is_some_and(|b| b.as_bool());
    }
    a.devices.sessions = sessions();
    draw_mic();
    a.changed();
}

/// A device appeared: the preferred one takes over.
pub fn on_change(appeared: bool) {
    let a = app();
    if a.state(ModuleId::DeviceControl) != State::Running { return; }
    refresh();
    if !appeared { return; }
    let o = a.settings.options.clone();
    if let Some(p) = o.preferred_output { if a.devices.outputs.iter().any(|d| d.id == p && !d.default) { set_default(&p); } }
    if let Some(p) = o.preferred_input { if a.devices.inputs.iter().any(|d| d.id == p && !d.default) { set_default(&p); } }
}

pub fn set_default(id: &str) {
    let a = app();
    unsafe {
        let policy: Result<IPolicyConfig> = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL);
        let w = HSTRING::from(id);
        let ok = policy.is_ok_and(|p| [eConsole, eMultimedia, eCommunications].iter().all(|r| p.SetDefaultEndpoint(PCWSTR(w.as_ptr()), *r).is_ok()));
        a.devices.notice = (!ok).then(|| "Windows refused to change the default device. Choose it in Settings ▸ System ▸ Sound.".to_string());
        if ok { a.devices.switches += 1; }
    }
    refresh();
}

pub fn next_output() {
    let d = &app().devices;
    if d.outputs.len() < 2 { return; }
    let i = d.outputs.iter().position(|x| x.default).unwrap_or(0);
    let next = d.outputs[(i + 1) % d.outputs.len()].clone();
    set_default(&next.id);
    app().tray.balloon("Sound Output", &next.name);
}

pub fn set_volume(v: f32) {
    unsafe { if let Some(e) = endpoint_volume(eRender) { let _ = e.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null()); } }
    app().devices.volume = v.clamp(0.0, 1.0);
}

/// Mutes or unmutes every microphone, so a call app that picked another one is muted too.
pub fn toggle_mic() {
    let a = app();
    if a.state(ModuleId::DeviceControl) != State::Running { return; }
    let mute = !a.devices.mic_muted;
    unsafe {
        if let Some(e) = enumerator() {
            if let Ok(coll) = e.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) {
                for i in 0..coll.GetCount().unwrap_or(0) {
                    if let Ok(d) = coll.Item(i) {
                        if let Ok(v) = d.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) { let _ = v.SetMute(mute, std::ptr::null()); }
                    }
                }
            }
        }
    }
    refresh();
    a.tray.balloon("Microphone", if mute { "Muted everywhere" } else { "On again" });
}

fn draw_mic() {
    let a = app();
    if !a.devices.mic_muted { a.tray.remove(UID_MIC); return; }
    let n = icon_size();
    let s = n as f64 / 16.0;
    let red = rgb(0xE5484D);
    let px = render_offscreen(n as u32, n as u32, 96.0, None, |c| {
        c.round(Rect::new(5.5 * s, 1.5 * s, 5.0 * s, 8.5 * s), 2.5 * s, red);
        c.arc(8.0 * s, 7.5 * s, 4.5 * s, 0.0, red, 1.2 * s);
        c.line(3.5 * s, 7.5 * s, 3.5 * s, 8.0 * s, red, 1.2 * s);
        c.line(12.5 * s, 7.5 * s, 12.5 * s, 8.0 * s, red, 1.2 * s);
        c.line(8.0 * s, 12.0 * s, 8.0 * s, 14.5 * s, red, 1.2 * s);
        c.line(2.0 * s, 14.0 * s, 14.0 * s, 2.0 * s, red, 1.6 * s);
    }).unwrap_or_default();
    a.tray.set(UID_MIC, icon_from_pixels(&px, n, n), "Microphone muted: click to unmute", true);
}

// ---------- volume per app ----------

unsafe fn session_volumes() -> Vec<(u32, ISimpleAudioVolume)> {
    let mut out = vec![];
    let Some(d) = enumerator().and_then(|e| e.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()) else { return out };
    let Ok(mgr) = d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { return out };
    let Ok(list) = mgr.GetSessionEnumerator() else { return out };
    for i in 0..list.GetCount().unwrap_or(0) {
        let Ok(c) = list.GetSession(i) else { continue };
        let Ok(c2) = c.cast::<IAudioSessionControl2>() else { continue };
        // Expired sessions belong to programs that closed; system sounds have their own row in Windows' mixer.
        if c.GetState().is_ok_and(|s| s == AudioSessionStateExpired) || c2.IsSystemSoundsSession() == S_OK { continue; }
        let pid = c2.GetProcessId().unwrap_or(0);
        if let Ok(v) = c.cast::<ISimpleAudioVolume>() { out.push((pid, v)); }
    }
    out
}

pub fn sessions() -> Vec<Session> {
    let mut out: Vec<Session> = vec![];
    unsafe {
        for (pid, v) in session_volumes() {
            let name = crate::monitor_data::process_name(pid).unwrap_or_else(|| format!("Process {pid}"));
            // One row per program: a browser plays through several sessions.
            if out.iter().any(|s| s.name == name) { continue; }
            out.push(Session { pid, name, volume: v.GetMasterVolume().unwrap_or(1.0), muted: v.GetMute().is_ok_and(|m| m.as_bool()) });
        }
    }
    out
}

/// Sets every session of the program with this name.
pub fn set_app_volume(name: &str, volume: Option<f32>, mute: Option<bool>) {
    unsafe {
        for (pid, v) in session_volumes() {
            if crate::monitor_data::process_name(pid).as_deref() != Some(name) { continue; }
            if let Some(x) = volume { let _ = v.SetMasterVolume(x.clamp(0.0, 1.0), std::ptr::null()); }
            if let Some(m) = mute { let _ = v.SetMute(m, std::ptr::null()); }
        }
    }
    let a = app();
    for s in a.devices.sessions.iter_mut().filter(|s| s.name == name) {
        if let Some(x) = volume { s.volume = x.clamp(0.0, 1.0); }
        if let Some(m) = mute { s.muted = m; }
    }
}

// ---------- light and dark mode ----------

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

/// Writes both switches Windows' Settings ▸ Personalization ▸ Colors writes, then tells running apps.
pub fn set_light_mode(light: bool) {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut k = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PERSONALIZE, None, KEY_SET_VALUE, &mut k).is_err() { return; }
        let v = (light as u32).to_le_bytes();
        let _ = RegSetValueExW(k, w!("AppsUseLightTheme"), None, REG_DWORD, Some(&v));
        let _ = RegSetValueExW(k, w!("SystemUsesLightTheme"), None, REG_DWORD, Some(&v));
        let _ = RegCloseKey(k);
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("ImmersiveColorSet").as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
    }
    app().devices.theme_switches += 1;
    crate::tray::refresh_theme();
}

pub fn toggle_theme() { set_light_mode(crate::window::is_dark()); }

/// Called every minute while the gadget runs: switches only when the scheduled mode differs from the current one.
pub fn theme_tick() {
    let a = app();
    let o = &a.settings.options;
    if !o.theme_schedule || a.state(ModuleId::DeviceControl) != State::Running { return; }
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let now = t.wHour as i32 * 60 + t.wMinute as i32;
    let want_light = crate::core::settings::light_due(now, o.light_minute, o.dark_minute);
    if want_light == crate::window::is_dark() { set_light_mode(want_light); }
}

// ---------- displays ----------

fn release_monitors() {
    for m in app().devices.monitors.drain(..) {
        unsafe { let _ = DestroyPhysicalMonitor(HANDLE(m.handle as *mut _)); }
    }
}

pub fn read_monitors() {
    release_monitors();
    let mut out = vec![];
    for (h, _, _) in crate::snap::monitors() {
        unsafe {
            let mut n = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(h, &mut n).is_err() || n == 0 { continue; }
            let mut arr = vec![PHYSICAL_MONITOR::default(); n as usize];
            if GetPhysicalMonitorsFromHMONITOR(h, &mut arr).is_err() { continue; }
            for pm in arr {
                let (mut lo, mut cur, mut hi) = (0, 0, 0);
                // Monitors without DDC/CI (and laptop screens) refuse: they are left out.
                if GetMonitorBrightness(pm.hPhysicalMonitor, &mut lo, &mut cur, &mut hi) != 0 && hi > lo {
                    let desc = pm.szPhysicalMonitorDescription; let name = String::from_utf16_lossy(&desc[..desc.iter().position(|c| *c == 0).unwrap_or(0)]);
                    out.push(Monitor { name: if name.is_empty() { "Display".into() } else { name }, handle: pm.hPhysicalMonitor.0 as isize, brightness: cur, min: lo, max: hi });
                } else { let _ = DestroyPhysicalMonitor(pm.hPhysicalMonitor); }
            }
        }
    }
    app().devices.monitors = out;
}

pub fn set_brightness(i: usize, v: u32) {
    let Some(m) = app().devices.monitors.get_mut(i) else { return };
    let v = v.clamp(m.min, m.max);
    unsafe { let _ = SetMonitorBrightness(HANDLE(m.handle as *mut _), v); }
    m.brightness = v;
}

pub fn step_all(up: bool) {
    let n = app().devices.monitors.len();
    for i in 0..n {
        let m = &app().devices.monitors[i];
        let step = ((m.max - m.min) / 10).max(1);
        let v = if up { m.brightness + step } else { m.brightness.saturating_sub(step) };
        set_brightness(i, v);
    }
    app().changed();
}

#[allow(dead_code)]
fn _unused() -> Option<HMONITOR> { None }
