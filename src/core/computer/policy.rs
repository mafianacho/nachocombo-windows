//! Argument rules of the Computer Use tools that do not touch Windows (macOS `ComputerUsePolicies.swift`, Windows
//! rules). The shell calls them before it touches the system; a violation becomes a tool error and nothing runs.
//!
//! Windows differs from macOS where the platform does:
//! - Keys are virtual-key codes; `cmd` is the Windows key, `fn` does not exist (Windows shortcuts use `ctrl`).
//! - Paths are `C:\…` (either slash); UNC, device (`\\?\`, `\\.\`), alternate data streams (`a.txt:x`), device names
//!   (`NUL`, `COM1` …), short 8.3 names (`PROGRA~1`) and trailing dots or spaces are refused, because each is a second
//!   spelling of a path the checks below would otherwise not recognise. The shell checks the real path again after it
//!   resolved links.
//! - Files: no overwriting. `file_write` makes a new file or appends; an existing file is never replaced (no deleting
//!   actions on Windows).
//! - Processes: TERM, INT and HUP ask the windows to close (Windows has no signals); KILL ends the process.
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyError(pub String);

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

fn err<T>(m: impl Into<String>) -> Result<T, PolicyError> { Err(PolicyError(m.into())) }

// MARK: keyboard

pub struct KeyboardMap;

impl KeyboardMap {
    pub const VK_SHIFT: u16 = 0x10;
    pub const VK_CONTROL: u16 = 0x11;
    pub const VK_MENU: u16 = 0x12;
    pub const VK_LWIN: u16 = 0x5B;

    /// Windows virtual-key code of a key name (macOS names, plus `insert`).
    pub fn key_code(name: &str) -> Option<u16> {
        let n = name.trim().to_lowercase();
        let c = |ch: char| Some(ch as u16);
        match n.as_str() {
            s if s.len() == 1 && s.as_bytes()[0].is_ascii_lowercase() => c(s.as_bytes()[0].to_ascii_uppercase() as char),
            s if s.len() == 1 && s.as_bytes()[0].is_ascii_digit() => c(s.as_bytes()[0] as char),
            "=" => Some(0xBB), "-" => Some(0xBD), "]" => Some(0xDD), "[" => Some(0xDB), "'" => Some(0xDE), ";" => Some(0xBA),
            "\\" => Some(0xDC), "," => Some(0xBC), "/" => Some(0xBF), "." => Some(0xBE), "`" => Some(0xC0),
            "return" | "enter" => Some(0x0D), "tab" => Some(0x09), "space" => Some(0x20), "delete" | "backspace" => Some(0x08),
            "escape" | "esc" => Some(0x1B), "forwarddelete" => Some(0x2E), "home" => Some(0x24), "end" => Some(0x23),
            "pageup" => Some(0x21), "pagedown" => Some(0x22), "insert" => Some(0x2D),
            "left" | "arrowleft" => Some(0x25), "up" | "arrowup" => Some(0x26), "right" | "arrowright" => Some(0x27), "down" | "arrowdown" => Some(0x28),
            s if s.starts_with('f') => s[1..].parse::<u16>().ok().filter(|k| (1..=12).contains(k)).map(|k| 0x6F + k),
            _ => None,
        }
    }

    /// Keys SendInput has to mark as extended (`KEYEVENTF_EXTENDEDKEY`), or Windows reads the number pad's key.
    pub fn is_extended(vk: u16) -> bool { matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B) }

    /// Modifier names → virtual keys, pressed in this order (ctrl, alt, shift, win) and released backwards.
    pub fn modifiers(names: &[String]) -> Result<Vec<u16>, PolicyError> {
        let mut out = vec![];
        for raw in names {
            let vk = match raw.trim().to_lowercase().as_str() {
                "ctrl" | "control" => Self::VK_CONTROL,
                "alt" | "option" | "opt" => Self::VK_MENU,
                "shift" => Self::VK_SHIFT,
                "win" | "windows" | "cmd" | "command" | "meta" | "super" => Self::VK_LWIN,
                "fn" | "function" => return err("fn has no key on Windows"),
                _ => return err(format!("Unknown modifier key: {}", cut(raw, 20))),
            };
            if !out.contains(&vk) { out.push(vk); }
        }
        let order = [Self::VK_CONTROL, Self::VK_MENU, Self::VK_SHIFT, Self::VK_LWIN];
        out.sort_by_key(|v| order.iter().position(|o| o == v));
        Ok(out)
    }

    /// Combinations Windows keeps for itself or that SendInput cannot send: refused rather than half sent.
    pub fn check_combination(vk: u16, mods: &[u16]) -> Result<(), PolicyError> {
        let has = |m| mods.contains(&m);
        if vk == 0x2E && has(Self::VK_CONTROL) && has(Self::VK_MENU) { return err("Ctrl+Alt+Delete cannot be sent"); }
        if vk == 0x4C && has(Self::VK_LWIN) { return err("Win+L (lock) is not sent"); }
        Ok(())
    }
}

fn cut(s: &str, n: usize) -> String { s.chars().take(n).collect() }

// MARK: input, screenshots, shell

pub struct InputPolicy;

impl InputPolicy {
    pub const MAX_TEXT: usize = 4000;
    pub const MAX_COORDINATE: f64 = 100_000.0;
    /// Pixels per wheel notch for `mouse_scroll` (Windows scrolls in notches of 120 units).
    pub const PIXELS_PER_NOTCH: i64 = 40;
    pub const MAX_SCROLL: i64 = 100_000;

    pub fn point(x: f64, y: f64) -> Result<(i32, i32), PolicyError> {
        if !x.is_finite() || !y.is_finite() || x.abs() > Self::MAX_COORDINATE || y.abs() > Self::MAX_COORDINATE { return err("Invalid coordinates"); }
        Ok((x.round() as i32, y.round() as i32))
    }

    pub fn text(t: &str) -> Result<(), PolicyError> {
        if t.is_empty() { return err("The text to type is empty"); }
        if t.chars().count() > Self::MAX_TEXT { return err(format!("Text can be at most {} characters", Self::MAX_TEXT)); }
        Ok(())
    }

    pub fn click_count(c: Option<i64>) -> Result<u32, PolicyError> {
        let v = c.unwrap_or(1);
        if !(1..=3).contains(&v) { return err("clickCount must be 1 to 3"); }
        Ok(v as u32)
    }

    /// Pixels → wheel units (120 per notch of about 40 pixels; smaller amounts are sent as part of a notch).
    pub fn wheel_units(pixels: i64) -> Result<i32, PolicyError> {
        if pixels.abs() > Self::MAX_SCROLL { return err(format!("A scroll can be at most {} pixels", Self::MAX_SCROLL)); }
        Ok((pixels * 120 / Self::PIXELS_PER_NOTCH) as i32)
    }
}

pub struct ScreenshotPolicy;

impl ScreenshotPolicy {
    pub const DEFAULT_MAX_WIDTH: i64 = 1568;
    pub const MIN_WIDTH: i64 = 64;
    pub const MAX_WIDTH: i64 = 4096;
    /// A region larger than this many pixels is refused (a 16K canvas would take seconds and hundreds of MB).
    pub const MAX_REGION_PIXELS: i64 = 8192 * 8192;

    pub fn clamp_max_width(requested: Option<i64>) -> i64 { requested.map_or(Self::DEFAULT_MAX_WIDTH, |w| w.clamp(Self::MIN_WIDTH, Self::MAX_WIDTH)) }

    /// The size an image of `w`×`h` is scaled to (never up).
    pub fn scaled(w: i64, h: i64, max_width: i64) -> (i64, i64) {
        if w <= max_width || w <= 0 { return (w, h); }
        (max_width, ((h as f64) * (max_width as f64) / (w as f64)).round().max(1.0) as i64)
    }

    pub fn region(x: f64, y: f64, w: f64, h: f64) -> Result<(i32, i32, i32, i32), PolicyError> {
        let (x, y) = InputPolicy::point(x, y)?;
        if !w.is_finite() || !h.is_finite() || w < 1.0 || h < 1.0 { return err("width and height must be at least 1"); }
        let (w, h) = (w.round() as i64, h.round() as i64);
        if w * h > Self::MAX_REGION_PIXELS { return err("The region is too large"); }
        Ok((x, y, w as i32, h as i32))
    }
}

pub struct ShellPolicy;

impl ShellPolicy {
    pub const DEFAULT_TIMEOUT: i64 = 30;
    pub const MAX_TIMEOUT: i64 = 120;
    pub const MAX_OUTPUT: usize = 65_536;
    /// The command line PowerShell gets is base64 UTF-16 (`-EncodedCommand`), so keep well under Windows' 32 767.
    pub const MAX_COMMAND: usize = 8000;

    pub fn timeout(requested: Option<i64>) -> u64 { requested.map_or(Self::DEFAULT_TIMEOUT, |t| t.clamp(1, Self::MAX_TIMEOUT)) as u64 }

    pub fn command(c: &str) -> Result<(), PolicyError> {
        if c.trim().is_empty() { return err("The command is empty"); }
        if c.chars().count() > Self::MAX_COMMAND { return err(format!("A command can be at most {} characters", Self::MAX_COMMAND)); }
        if c.contains('\0') { return err("The command contains a NUL character"); }
        Ok(())
    }

    /// `-EncodedCommand` takes base64 of the UTF-16LE text.
    pub fn encoded(c: &str) -> String {
        let bytes: Vec<u8> = c.encode_utf16().flat_map(u16::to_le_bytes).collect();
        crate::core::pet::crypto::base64(&bytes)
    }

    /// Keeps the first `MAX_OUTPUT` bytes on a character boundary.
    pub fn cut_output(bytes: &[u8]) -> (String, bool) {
        let s = String::from_utf8_lossy(bytes);
        if s.len() <= Self::MAX_OUTPUT { return (s.into_owned(), false); }
        let mut end = Self::MAX_OUTPUT;
        while !s.is_char_boundary(end) { end -= 1; }
        (s[..end].to_string(), true)
    }
}

// MARK: open

pub struct OpenPolicy;

impl OpenPolicy {
    /// Kinds of file whose default handler runs code, installs something or follows a link somewhere else.
    pub const BLOCKED_EXTENSIONS: [&'static str; 52] = [
        "exe", "com", "bat", "cmd", "ps1", "psm1", "psd1", "ps1xml", "psc1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "wsc",
        "msi", "msp", "mst", "msc", "cpl", "scr", "pif", "lnk", "url", "hta", "jar", "reg", "inf", "sct", "application",
        "appref-ms", "gadget", "settingcontent-ms", "appx", "appxbundle", "msix", "msixbundle", "py", "pyw", "pl", "rb", "sh",
        "iso", "img", "vhd", "vhdx", "dll", "chm", "library-ms", "search-ms", "scf",
    ];

    pub fn url(text: &str) -> Result<String, PolicyError> {
        let t = text.trim();
        if t.is_empty() || t.len() > 2048 || t.chars().any(|c| c.is_whitespace() || c.is_control()) { return err("Not a valid URL"); }
        let Some((scheme, rest)) = t.split_once(':') else { return err("Not a valid URL") };
        let scheme = scheme.to_ascii_lowercase();
        if !["http", "https", "mailto"].contains(&scheme.as_str()) { return err("Only http, https and mailto URLs can be opened"); }
        if scheme != "mailto" {
            let Some(after) = rest.strip_prefix("//") else { return err("The URL has no host") };
            let host = after.split(['/', '?', '#']).next().unwrap_or("");
            let host = host.rsplit('@').next().unwrap_or("");
            if host.is_empty() || host.starts_with(':') { return err("The URL has no host"); }
        } else if rest.is_empty() { return err("Not a valid URL"); }
        Ok(t.to_string())
    }

    pub fn file(path: &str) -> Result<String, PolicyError> {
        let p = WinPath::normalize(path)?;
        let ext = WinPath::extension(&p);
        if Self::BLOCKED_EXTENSIONS.contains(&ext.as_str()) { return err(format!("Programs, scripts, installers and shortcuts cannot be opened with this tool (.{ext})")); }
        Ok(p)
    }
}

// MARK: Windows paths

pub struct WinPath;

impl WinPath {
    const DEVICE_NAMES: [&'static str; 22] = ["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
        "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"];

    /// `C:\a\b` with `\`, "." and ".." resolved, or an error for every other spelling.
    pub fn normalize(path: &str) -> Result<String, PolicyError> {
        let bad = || err("An absolute path like C:\\Users\\name\\file.txt is required");
        if path.chars().any(|c| c.is_control()) || path.len() > 1024 { return bad(); }
        let p = path.replace('/', "\\");
        let b = p.as_bytes();
        if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' { return bad(); }
        let drive = (b[0] as char).to_ascii_uppercase();
        let mut parts: Vec<&str> = vec![];
        for c in p[3..].split('\\') {
            if c.is_empty() || c == "." { continue; }
            if c == ".." { parts.pop(); continue; }
            if c.contains(':') { return err("Alternate data streams (name:stream) are not allowed"); }
            if c.contains(['*', '?', '"', '<', '>', '|']) { return err("The path contains characters Windows does not allow"); }
            if c.ends_with('.') || c.ends_with(' ') { return err("A path part cannot end with a dot or a space"); }
            let stem = c.split('.').next().unwrap_or("").to_ascii_lowercase();
            if Self::DEVICE_NAMES.contains(&stem.as_str()) { return err("Device names (NUL, CON, COM1 …) are not allowed"); }
            if Self::is_short_name(c) { return err("Short 8.3 names (like PROGRA~1) are not allowed; use the full name"); }
            parts.push(c);
        }
        Ok(format!("{drive}:\\{}", parts.join("\\")))
    }

    /// `NAME~1.EXT`: a tilde followed by digits in the name part.
    fn is_short_name(c: &str) -> bool {
        let name = c.split('.').next().unwrap_or("");
        name.find('~').is_some_and(|i| name[i + 1..].chars().next().is_some_and(|d| d.is_ascii_digit()))
    }

    pub fn extension(p: &str) -> String {
        let last = p.rsplit('\\').next().unwrap_or("");
        match last.rfind('.') { Some(i) if i > 0 => last[i + 1..].to_ascii_lowercase(), _ => String::new() }
    }

    /// `root` or something under it (case-insensitive, as NTFS by default).
    pub fn inside(path: &str, root: &str) -> bool {
        let (p, r) = (path.to_lowercase(), root.trim_end_matches('\\').to_lowercase());
        p == r || p.starts_with(&format!("{r}\\"))
    }
}

// MARK: files

/// Where `file_read` / `file_write` may go. Built by the shell from the user's known folders.
#[derive(Clone, Debug)]
pub struct FilePolicy {
    /// Never read nor written.
    pub never: Vec<String>,
    /// Writing only under these.
    pub writable: Vec<String>,
    /// Never written even when under a writable root.
    pub no_write: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode { Create, Append }

impl FilePolicy {
    pub const MAX_READ: usize = 262_144;
    pub const MAX_WRITE: usize = 1_048_576;

    /// Windows' folders: `home` (%USERPROFILE%), `roaming` (%APPDATA%), `local` (%LOCALAPPDATA%), `temp` (%TEMP%),
    /// `documents` (the Documents known folder) and `app` (Nacho Combo's own folder).
    pub fn windows(home: &str, roaming: &str, local: &str, temp: &str, documents: &str, app: &str) -> Self {
        let j = |a: &str, b: &str| format!("{}\\{b}", a.trim_end_matches('\\'));
        let never = vec![
            j(home, ".ssh"), j(home, ".gnupg"), j(home, ".aws"), j(home, ".azure"), j(home, ".kube"), j(home, ".docker"),
            j(roaming, "Microsoft\\Credentials"), j(local, "Microsoft\\Credentials"), j(roaming, "Microsoft\\Protect"),
            j(roaming, "Microsoft\\Crypto"), j(roaming, "Microsoft\\SystemCertificates"), j(local, "Microsoft\\Vault"),
            j(local, "Google\\Chrome\\User Data"), j(local, "Microsoft\\Edge\\User Data"), j(roaming, "Mozilla\\Firefox"),
            app.trim_end_matches('\\').to_string(),
        ];
        let no_write = vec![
            j(roaming, "Microsoft\\Windows\\Start Menu"), j(documents, "WindowsPowerShell"), j(documents, "PowerShell"),
            j(local, "Microsoft\\WindowsApps"), j(roaming, "Microsoft\\Windows\\Recent"),
        ];
        Self { never, writable: vec![home.trim_end_matches('\\').to_string(), temp.trim_end_matches('\\').to_string()], no_write }
    }

    /// The normalised path, or why it is refused (call again with the real path after links are resolved).
    pub fn check(&self, path: &str, write: bool) -> Result<String, PolicyError> {
        let p = WinPath::normalize(path)?;
        if self.never.iter().any(|r| WinPath::inside(&p, r)) { return err("This location is protected"); }
        if !write { return Ok(p); }
        if !self.writable.iter().any(|r| WinPath::inside(&p, r)) { return err("Writing is only allowed under the user's folder and the temp folder"); }
        if self.no_write.iter().any(|r| WinPath::inside(&p, r)) { return err("Cannot write to startup, PowerShell profile or app alias folders"); }
        Ok(p)
    }

    /// Never overwrite: a new file, or appending to one that exists.
    pub fn write_mode(exists: bool, append: bool) -> Result<WriteMode, PolicyError> {
        match (exists, append) {
            (false, _) => Ok(WriteMode::Create),
            (true, true) => Ok(WriteMode::Append),
            (true, false) => err("The file exists. Windows never overwrites a file with this tool; use append=true or a new name"),
        }
    }

    pub fn content(c: &str) -> Result<(), PolicyError> {
        if c.len() > Self::MAX_WRITE { return err(format!("Content can be at most {} bytes", Self::MAX_WRITE)); }
        Ok(())
    }
}

// MARK: processes

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillMode {
    /// Ask the windows to close (WM_CLOSE): the app may ask about unsaved work.
    Close,
    /// TerminateProcess.
    End,
}

pub struct ProcessPolicy;

impl ProcessPolicy {
    /// Never ended (Windows would refuse most anyway, or the PC would stop).
    pub const PROTECTED: [&'static str; 10] = ["system", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe",
        "svchost.exe", "dwm.exe", "msmpeng.exe"];

    pub fn kill(pid: i64, signal: Option<&str>, own_pid: u32) -> Result<KillMode, PolicyError> {
        if pid <= 4 || pid > u32::MAX as i64 || pid == own_pid as i64 { return err("This process cannot be ended"); }
        let s = signal.map(|s| s.trim().to_ascii_uppercase());
        match s.as_deref().map(|s| s.strip_prefix("SIG").unwrap_or(s)) {
            None | Some("TERM") | Some("INT") | Some("HUP") => Ok(KillMode::Close),
            Some("KILL") => Ok(KillMode::End),
            _ => err("Allowed signals are TERM, KILL, INT and HUP"),
        }
    }

    pub fn protected_name(exe: &str) -> bool { Self::PROTECTED.contains(&exe.to_ascii_lowercase().as_str()) || exe.eq_ignore_ascii_case("NachoCombo.exe") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn keys_map_to_virtual_keys() {
        assert_eq!(KeyboardMap::key_code("c"), Some(0x43));
        assert_eq!(KeyboardMap::key_code(" A "), Some(0x41));
        assert_eq!(KeyboardMap::key_code("7"), Some(0x37));
        assert_eq!(KeyboardMap::key_code("return"), Some(0x0D));
        assert_eq!(KeyboardMap::key_code("delete"), Some(0x08), "macOS delete is backspace");
        assert_eq!(KeyboardMap::key_code("forwarddelete"), Some(0x2E));
        assert_eq!(KeyboardMap::key_code("arrowup"), Some(0x26));
        assert_eq!(KeyboardMap::key_code("f12"), Some(0x7B));
        assert_eq!(KeyboardMap::key_code("f13"), None);
        assert_eq!(KeyboardMap::key_code("."), Some(0xBE));
        assert_eq!(KeyboardMap::key_code("capslock"), None);
        assert!(KeyboardMap::is_extended(0x25) && KeyboardMap::is_extended(0x2E) && !KeyboardMap::is_extended(0x43));
    }

    #[test]
    fn modifiers_are_ordered_and_fn_is_refused() {
        assert_eq!(KeyboardMap::modifiers(&s(&["shift", "cmd", "ctrl", "alt"])), Ok(vec![0x11, 0x12, 0x10, 0x5B]));
        assert_eq!(KeyboardMap::modifiers(&s(&["Control", "control"])), Ok(vec![0x11]));
        assert_eq!(KeyboardMap::modifiers(&s(&["option"])), Ok(vec![0x12]));
        assert!(KeyboardMap::modifiers(&s(&["fn"])).unwrap_err().0.contains("fn"));
        assert!(KeyboardMap::modifiers(&s(&["hyper"])).is_err());
        assert!(KeyboardMap::check_combination(0x2E, &[0x11, 0x12]).is_err());
        assert!(KeyboardMap::check_combination(0x4C, &[0x5B]).is_err());
        assert!(KeyboardMap::check_combination(0x43, &[0x11]).is_ok());
    }

    #[test]
    fn input_limits() {
        assert_eq!(InputPolicy::point(10.4, -3.6), Ok((10, -4)));
        assert!(InputPolicy::point(f64::NAN, 0.0).is_err());
        assert!(InputPolicy::point(100_001.0, 0.0).is_err());
        assert!(InputPolicy::text("").is_err());
        assert!(InputPolicy::text(&"가".repeat(4000)).is_ok());
        assert!(InputPolicy::text(&"가".repeat(4001)).is_err());
        assert_eq!(InputPolicy::click_count(None), Ok(1));
        assert!(InputPolicy::click_count(Some(4)).is_err() && InputPolicy::click_count(Some(0)).is_err());
        assert_eq!(InputPolicy::wheel_units(40), Ok(120));
        assert_eq!(InputPolicy::wheel_units(-5), Ok(-15), "part of a notch");
        assert_eq!(InputPolicy::wheel_units(0), Ok(0));
        assert!(InputPolicy::wheel_units(100_001).is_err());
    }

    #[test]
    fn screenshot_and_shell_limits_match_macos() {
        assert_eq!(ScreenshotPolicy::clamp_max_width(None), 1568);
        assert_eq!(ScreenshotPolicy::clamp_max_width(Some(10)), 64);
        assert_eq!(ScreenshotPolicy::clamp_max_width(Some(99_999)), 4096);
        assert_eq!(ScreenshotPolicy::scaled(3136, 1764, 1568), (1568, 882));
        assert_eq!(ScreenshotPolicy::scaled(800, 600, 1568), (800, 600), "never scaled up");
        assert!(ScreenshotPolicy::region(0.0, 0.0, 0.0, 10.0).is_err());
        assert!(ScreenshotPolicy::region(0.0, 0.0, 9000.0, 9000.0).is_err());
        assert_eq!(ScreenshotPolicy::region(1.0, 2.0, 30.0, 40.0), Ok((1, 2, 30, 40)));
        assert_eq!(ShellPolicy::timeout(None), 30);
        assert_eq!(ShellPolicy::timeout(Some(0)), 1);
        assert_eq!(ShellPolicy::timeout(Some(999)), 120);
        assert!(ShellPolicy::command("  ").is_err());
        assert!(ShellPolicy::command(&"a".repeat(8001)).is_err());
        assert_eq!(ShellPolicy::encoded("dir"), "ZABpAHIA");
        let (out, cut) = ShellPolicy::cut_output(&vec![b'a'; 70_000]);
        assert!(cut && out.len() == 65_536);
        let mut v = vec![b'a'; 65_535];
        v.extend("가".as_bytes());
        let (out, cut) = ShellPolicy::cut_output(&v);
        assert!(cut && out.len() == 65_535, "cut on a character boundary");
    }

    #[test]
    fn urls() {
        assert!(OpenPolicy::url("https://example.com/a?b=c").is_ok());
        assert!(OpenPolicy::url("HTTP://example.com").is_ok());
        assert!(OpenPolicy::url("mailto:someone@example.com").is_ok());
        for bad in ["file:///C:/Windows/System32/calc.exe", "javascript:alert(1)", "ms-settings:privacy", "https://", "https:///path",
                    "https://exa mple.com", "", "search-ms:query=x", "\\\\server\\share", "https://user@:80/"] {
            assert!(OpenPolicy::url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn paths_have_one_spelling() {
        assert_eq!(WinPath::normalize("c:/Users/me/./a/../b.txt"), Ok("C:\\Users\\me\\b.txt".into()));
        assert_eq!(WinPath::normalize("C:\\..\\..\\Windows"), Ok("C:\\Windows".into()));
        for bad in ["relative\\a.txt", "\\\\server\\share\\a", "\\\\?\\C:\\a", "\\\\.\\PhysicalDrive0", "C:a.txt", "C:\\a.txt:secret",
                    "C:\\Users\\me\\NUL", "C:\\Users\\me\\com1.txt", "C:\\PROGRA~1\\x", "C:\\Users\\me\\a.txt.", "C:\\Users\\me\\a ", "C:\\a\\b*"] {
            assert!(WinPath::normalize(bad).is_err(), "{bad}");
        }
        assert!(WinPath::normalize("C:\\Users\\me\\my~file.txt").is_ok(), "a tilde without digits is a normal name");
        assert_eq!(WinPath::extension("C:\\a\\b.PS1"), "ps1");
        assert_eq!(WinPath::extension("C:\\a\\.profile"), "");
        assert!(WinPath::inside("C:\\USERS\\Me\\x", "c:\\users\\me"));
        assert!(!WinPath::inside("C:\\Users\\Mean", "C:\\Users\\Me"));
    }

    #[test]
    fn open_file_refuses_programs_and_shortcuts() {
        assert!(OpenPolicy::file("C:\\Users\\me\\notes.txt").is_ok());
        assert!(OpenPolicy::file("C:\\Users\\me\\photo.PNG").is_ok());
        for bad in ["C:\\Users\\me\\setup.exe", "C:\\Users\\me\\run.BAT", "C:\\Users\\me\\x.ps1", "C:\\Users\\me\\link.lnk", "C:\\Users\\me\\web.url",
                    "C:\\Users\\me\\app.hta", "C:\\Users\\me\\i.msi", "C:\\Users\\me\\d.iso", "C:\\Users\\me\\x.settingcontent-ms"] {
            assert!(OpenPolicy::file(bad).is_err(), "{bad}");
        }
    }

    fn policy() -> FilePolicy {
        FilePolicy::windows("C:\\Users\\me", "C:\\Users\\me\\AppData\\Roaming", "C:\\Users\\me\\AppData\\Local", "C:\\Users\\me\\AppData\\Local\\Temp",
                            "C:\\Users\\me\\Documents", "C:\\Users\\me\\AppData\\Roaming\\NachoCombo")
    }

    #[test]
    fn files_keep_secrets_and_startup_out() {
        let p = policy();
        assert!(p.check("C:\\Users\\me\\notes.txt", false).is_ok());
        assert!(p.check("C:\\Windows\\win.ini", false).is_ok(), "reading outside the user folder is allowed (as on macOS)");
        for never in ["C:\\Users\\me\\.ssh\\id_ed25519", "c:\\users\\ME\\.aws\\credentials", "C:\\Users\\me\\AppData\\Roaming\\NachoCombo\\computer-use-keys.json",
                      "C:\\Users\\me\\AppData\\Roaming\\Microsoft\\Protect\\x", "C:\\Users\\me\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Login Data",
                      "C:\\Users\\me\\x\\..\\.ssh\\config"] {
            assert_eq!(p.check(never, false), Err(PolicyError("This location is protected".into())), "{never}");
            assert!(p.check(never, true).is_err(), "{never}");
        }
        assert!(p.check("C:\\Users\\me\\out.txt", true).is_ok());
        assert!(p.check("C:\\Users\\me\\AppData\\Local\\Temp\\a.txt", true).is_ok());
        for no in ["C:\\Windows\\Temp\\a.txt", "D:\\a.txt", "C:\\Users\\other\\a.txt"] { assert!(p.check(no, true).is_err(), "{no}"); }
        for startup in ["C:\\Users\\me\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\x.txt",
                        "C:\\Users\\me\\Documents\\WindowsPowerShell\\Microsoft.PowerShell_profile.ps1", "C:\\Users\\me\\Documents\\PowerShell\\profile.ps1",
                        "C:\\Users\\me\\AppData\\Local\\Microsoft\\WindowsApps\\python.exe"] {
            assert!(p.check(startup, true).is_err(), "{startup}");
            assert!(p.check(startup, false).is_ok(), "{startup}: reading is fine");
        }
    }

    #[test]
    fn files_are_never_overwritten() {
        assert_eq!(FilePolicy::write_mode(false, false), Ok(WriteMode::Create));
        assert_eq!(FilePolicy::write_mode(false, true), Ok(WriteMode::Create));
        assert_eq!(FilePolicy::write_mode(true, true), Ok(WriteMode::Append));
        assert!(FilePolicy::write_mode(true, false).is_err());
        assert!(FilePolicy::content(&"a".repeat(1_048_577)).is_err());
    }

    #[test]
    fn processes() {
        assert_eq!(ProcessPolicy::kill(1234, None, 99), Ok(KillMode::Close));
        assert_eq!(ProcessPolicy::kill(1234, Some("sigkill"), 99), Ok(KillMode::End));
        assert_eq!(ProcessPolicy::kill(1234, Some("HUP"), 99), Ok(KillMode::Close));
        assert!(ProcessPolicy::kill(4, None, 99).is_err(), "System");
        assert!(ProcessPolicy::kill(0, None, 99).is_err());
        assert!(ProcessPolicy::kill(99, None, 99).is_err(), "never Nacho Combo itself");
        assert!(ProcessPolicy::kill(1234, Some("STOP"), 99).is_err());
        assert!(ProcessPolicy::protected_name("LSASS.EXE") && ProcessPolicy::protected_name("NachoCombo.exe") && !ProcessPolicy::protected_name("notepad.exe"));
    }
}
