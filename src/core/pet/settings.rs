//! Nacho Pet settings: which nacho-server and login server, where the pet and its chat sit, what Nacho may do, and the
//! server URL validation (`ServerEndpoint`, `IssuerURL`) without a URL crate — credentials, query and fragment refused,
//! plain http only for this computer. Port of `PetSettings.swift`.
use super::auth::OIDCConfig;
use super::chat_layout::{PetChatLayout, PetChatSize};
use super::selection::PetSelectionBarSettings;
use serde::{Deserialize, Serialize};

/// Which nacho-server (and login realm) the pet talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PetServerMode {
    /// The production server and realm (default).
    #[default]
    Production,
    /// A nacho-server running on this computer (127.0.0.1:8787, login on localhost:18180).
    Local,
    /// The test-only mock server (`tools/mock-server`): chat, uploads, connector and a stand-in login on one port.
    Mock,
    /// Typed in by the user (validated).
    Custom,
}

impl PetServerMode {
    pub const ALL: [PetServerMode; 4] = [Self::Production, Self::Local, Self::Mock, Self::Custom];
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Production => "Nacho (nacho.nachotown.com)",
            Self::Local => "Local nacho-server",
            Self::Mock => "Mock server (development)",
            Self::Custom => "Custom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PetPosition {
    pub x: f64,
    pub y: f64,
}

/// Saved as `pet.json` next to the app's other settings. Every field falls back on its own, so a file from an older or
/// newer version still loads (serde `default` per field; a field of the wrong type falls back too, see [`Self::from_json`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PetSettings {
    pub server_mode: PetServerMode,
    #[serde(rename = "customServerURL")]
    pub custom_server_url: String,
    #[serde(rename = "customIssuerURL")]
    pub custom_issuer_url: String,
    /// Where the user dragged the pet (screen coordinates of its window's top-left corner on Windows). None = bottom right.
    pub pet_position: Option<PetPosition>,
    /// The chat bubble's size after the user dragged its edges. None = the design's default.
    pub chat_size: Option<PetChatSize>,
    /// Nacho may arrange windows and switch gadgets on or off when asked in the chat (connector tools).
    pub allow_combo_control: bool,
    /// The selection bar (part of Nacho Pet, not a gadget). Off by default.
    pub selection_bar: PetSelectionBarSettings,
    /// After the user spoke (hold the pet to talk), read the first sentence of Nacho's answer aloud.
    pub speak_voice_replies: bool,
}

impl Default for PetSettings {
    fn default() -> Self {
        Self {
            server_mode: PetServerMode::Production,
            custom_server_url: String::new(),
            custom_issuer_url: String::new(),
            pet_position: None,
            chat_size: None,
            allow_combo_control: true,
            selection_bar: PetSelectionBarSettings::default(),
            speak_voice_replies: true,
        }
    }
}

impl PetSettings {
    pub const PRODUCTION_SERVER_URL: &'static str = "https://nacho.nachotown.com";
    pub const PRODUCTION_ISSUER_URL: &'static str = "https://auth.nachotown.com/realms/nachotown";
    pub const LOCAL_SERVER_URL: &'static str = "http://127.0.0.1:8787";
    pub const LOCAL_ISSUER_URL: &'static str = "http://localhost:18180/realms/nachotown";
    pub const MOCK_SERVER_URL: &'static str = "http://127.0.0.1:8787";
    pub const MOCK_ISSUER_URL: &'static str = "http://127.0.0.1:8787/realms/nachotown";

    /// Lenient load: each field is read on its own; a missing or broken field keeps its default.
    pub fn from_json(text: &str) -> Option<PetSettings> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let o = v.as_object()?;
        let mut s = PetSettings::default();
        fn field<T: serde::de::DeserializeOwned>(o: &serde_json::Map<String, serde_json::Value>, k: &str) -> Option<T> {
            o.get(k).and_then(|v| serde_json::from_value(v.clone()).ok())
        }
        if let Some(x) = field(o, "serverMode") { s.server_mode = x; }
        if let Some(x) = field(o, "customServerURL") { s.custom_server_url = x; }
        if let Some(x) = field(o, "customIssuerURL") { s.custom_issuer_url = x; }
        s.pet_position = field(o, "petPosition");
        s.chat_size = field(o, "chatSize");
        if let Some(x) = field(o, "allowComboControl") { s.allow_combo_control = x; }
        if let Some(x) = field(o, "selectionBar") { s.selection_bar = x; }
        if let Some(x) = field(o, "speakVoiceReplies") { s.speak_voice_replies = x; }
        Some(s)
    }

    /// Pretty JSON, keys sorted.
    pub fn to_json(&self) -> String {
        let v = serde_json::to_value(self).unwrap_or_default();
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
    }

    /// Resolved server for the current mode, or None when the custom URL is empty/invalid.
    pub fn endpoint(&self) -> Option<ServerEndpoint> {
        let raw = match self.server_mode {
            PetServerMode::Production => Self::PRODUCTION_SERVER_URL,
            PetServerMode::Local => Self::LOCAL_SERVER_URL,
            PetServerMode::Mock => Self::MOCK_SERVER_URL,
            PetServerMode::Custom => &self.custom_server_url,
        };
        ServerEndpoint::parse(raw).ok()
    }

    /// Resolved login configuration for the current mode, or None when the custom issuer is empty/invalid.
    pub fn oidc(&self) -> Option<OIDCConfig> {
        let raw = match self.server_mode {
            PetServerMode::Production => Self::PRODUCTION_ISSUER_URL,
            PetServerMode::Local => Self::LOCAL_ISSUER_URL,
            PetServerMode::Mock => Self::MOCK_ISSUER_URL,
            PetServerMode::Custom => &self.custom_issuer_url,
        };
        IssuerURL::parse(raw).ok().map(OIDCConfig::new)
    }

    /// Checks what the user typed. Returns the first problem, or None when the settings can be saved.
    pub fn validation_problem(&self) -> Option<String> {
        if self.server_mode != PetServerMode::Custom { return None; }
        if let Err(e) = ServerEndpoint::parse(&self.custom_server_url) { return Some(format!("서버 주소: {}", e.korean_message())); }
        if let Err(e) = IssuerURL::parse(&self.custom_issuer_url) { return Some(format!("로그인 주소: {}", e.korean_message())); }
        None
    }

    /// A key that changes when the account world changes (other server or realm): tokens of one must not be used on
    /// another, so the token store is keyed by it.
    pub fn account_scope(&self) -> String {
        format!(
            "{}|{}",
            self.endpoint().map(|e| e.base).unwrap_or_else(|| "-".into()),
            self.oidc().map(|o| o.issuer).unwrap_or_else(|| "-".into())
        )
    }

    /// A hand-edited file must not smuggle in a bad URL or a position that is not a number.
    pub fn sanitized(s: PetSettings) -> PetSettings {
        let mut out = s;
        if out.server_mode == PetServerMode::Custom && out.validation_problem().is_some() {
            out.server_mode = PetServerMode::Production;
            out.custom_server_url.clear();
            out.custom_issuer_url.clear();
        }
        if let Some(p) = out.pet_position {
            if !(p.x.is_finite() && p.y.is_finite()) || p.x.abs() > 100_000.0 || p.y.abs() > 100_000.0 { out.pet_position = None; }
        }
        // Not a number or absurd: back to the default. Too small: the smallest usable size. The screen limit is applied
        // when the chat is shown (it depends on the screen the pet is on).
        if let Some(c) = out.chat_size {
            if !(c.width.is_finite() && c.height.is_finite()) || c.width > 100_000.0 || c.height > 100_000.0 {
                out.chat_size = None;
            } else {
                out.chat_size = Some(PetChatLayout::clamp(c, None));
            }
        }
        out.selection_bar = out.selection_bar.sanitized();
        out
    }
}

/// Loads and saves `pet.json`. A corrupt file is moved aside (`pet.json.corrupt`) and defaults are returned. Values read
/// from disk are re-validated.
pub struct PetSettingsStore {
    pub path: std::path::PathBuf,
}

impl PetSettingsStore {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self { Self { path: path.into() } }

    pub fn load(&self) -> PetSettings {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return PetSettings::default() };
        if let Some(s) = PetSettings::from_json(&text) { return PetSettings::sanitized(s); }
        let mut aside = self.path.clone().into_os_string();
        aside.push(".corrupt");
        let _ = std::fs::remove_file(&aside);
        let _ = std::fs::rename(&self.path, &aside);
        PetSettings::default()
    }

    /// Written to a temporary file first, then moved over the old one.
    pub fn save(&self, s: &PetSettings) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir)?; }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        std::fs::write(&tmp, s.to_json())?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerURLError { Empty, TooLong, InvalidScheme, NoHost, HasCredentials, HasQueryOrFragment, PlainHTTPNotLoopback, BadPath }

impl ServerURLError {
    pub fn korean_message(&self) -> &'static str {
        match self {
            Self::Empty => "주소를 입력해 주세요. 예: https://example.com",
            Self::TooLong => "주소가 너무 길어요. (최대 200자)",
            Self::InvalidScheme => "주소는 http:// 또는 https:// 로 시작해야 해요.",
            Self::NoHost => "주소에 서버 이름이 없어요. 예: https://example.com",
            Self::HasCredentials => "주소에 아이디/비밀번호를 넣을 수 없어요.",
            Self::HasQueryOrFragment => "주소 끝에 ? 나 # 를 붙일 수 없어요.",
            Self::PlainHTTPNotLoopback => "이 컴퓨터(127.0.0.1, localhost) 밖의 서버는 https:// 만 쓸 수 있어요.",
            Self::BadPath => "로그인 주소는 https://<서버>/realms/<이름> 모양이어야 해요.",
        }
    }
}

/// Validated server base URL: `http(s)://host[:port]` without a trailing slash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerEndpoint {
    pub base: String,
}

impl ServerEndpoint {
    pub fn web_socket_url(&self) -> String { self.web_socket("/ws/chat") }
    pub fn connector_url(&self) -> String { self.web_socket("/ws/connector") }

    fn web_socket(&self, path: &str) -> String {
        let rest = self.base.strip_prefix("https://").map(|r| format!("wss://{r}")).or_else(|| self.base.strip_prefix("http://").map(|r| format!("ws://{r}")));
        format!("{}{path}", rest.unwrap_or_else(|| self.base.clone()))
    }

    /// `base + path` (path starts with `/`; a query may follow).
    pub fn rest(&self, path: &str) -> String { format!("{}{path}", self.base) }

    pub fn is_loopback(host: &str) -> bool {
        matches!(host.to_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]")
    }

    pub fn parse(raw: &str) -> Result<ServerEndpoint, ServerURLError> {
        let c = URLCheck::components(raw)?;
        Ok(ServerEndpoint { base: c.origin() })
    }
}

/// Login realm issuer, e.g. `https://auth.nachotown.com/realms/nachotown`.
pub struct IssuerURL;

impl IssuerURL {
    pub fn parse(raw: &str) -> Result<String, ServerURLError> {
        let c = URLCheck::components(raw)?;
        let path = c.path.trim_end_matches('/');
        let parts: Vec<&str> = path.split('/').collect();
        // "", "realms", "<name>" — or a reverse-proxy prefix in front ("", "auth", "realms", "<name>").
        let ok = (3..=5).contains(&parts.len())
            && parts[0].is_empty()
            && parts[parts.len() - 2] == "realms"
            && parts[1..].iter().all(|p| URLCheck::is_path_segment(p));
        if !ok { return Err(ServerURLError::BadPath); }
        Ok(format!("{}{path}", c.origin()))
    }
}

/// The parts of an absolute URL the pet cares about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct URLParts {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

impl URLParts {
    pub fn origin(&self) -> String {
        match self.port {
            Some(p) => format!("{}://{}:{p}", self.scheme, self.host),
            None => format!("{}://{}", self.scheme, self.host),
        }
    }

    /// Splits `scheme://[userinfo@]host[:port][/path][?query][#fragment]`; also returns userinfo/query/fragment presence.
    pub fn split(s: &str) -> Option<(URLParts, bool, bool)> {
        let (scheme, rest) = s.split_once("://")?;
        if scheme.is_empty() || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b)) { return None; }
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        let (cred, hostport) = match authority.rfind('@') { Some(i) => (true, &authority[i + 1..]), None => (false, authority) };
        let (host, port) = if let Some(h) = hostport.strip_prefix('[') {
            let close = h.find(']')?;
            let after = &h[close + 1..];
            let port = match after { "" => None, p => Some(p.strip_prefix(':')?) };
            (format!("[{}]", &h[..close]), port)
        } else {
            match hostport.rsplit_once(':') { Some((h, p)) => (h.to_string(), Some(p)), None => (hostport.to_string(), None) }
        };
        let port = match port {
            None | Some("") => None,
            Some(p) => {
                if !p.bytes().all(|b| b.is_ascii_digit()) || p.len() > 5 { return None; }
                let n: u32 = p.parse().ok()?;
                if !(1..=65535).contains(&n) { return None; }
                Some(n as u16)
            }
        };
        let qf = tail.find(['?', '#']);
        let path = &tail[..qf.unwrap_or(tail.len())];
        Some((URLParts { scheme: scheme.to_lowercase(), host: host.to_lowercase(), port, path: path.to_string() }, cred, qf.is_some()))
    }
}

pub struct URLCheck;

impl URLCheck {
    pub fn components(raw: &str) -> Result<URLParts, ServerURLError> {
        let s = raw.trim();
        if s.is_empty() { return Err(ServerURLError::Empty); }
        if s.chars().count() > 200 { return Err(ServerURLError::TooLong); }
        let Some((scheme, _)) = s.split_once("://") else { return Err(ServerURLError::InvalidScheme) };
        let scheme = scheme.to_lowercase();
        if scheme != "http" && scheme != "https" { return Err(ServerURLError::InvalidScheme); }
        let Some((c, cred, query)) = URLParts::split(s) else { return Err(ServerURLError::NoHost) };
        if c.host.is_empty() || c.host == "[]" || c.host.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
            return Err(ServerURLError::NoHost);
        }
        if cred { return Err(ServerURLError::HasCredentials); }
        if query { return Err(ServerURLError::HasQueryOrFragment); }
        if scheme == "http" && !ServerEndpoint::is_loopback(&c.host) { return Err(ServerURLError::PlainHTTPNotLoopback); }
        Ok(c)
    }

    pub fn is_path_segment(s: &str) -> bool {
        !s.is_empty() && s.len() <= 64 && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ServerURLError::*;

    #[test]
    fn server_urls_are_validated() {
        let e = ServerEndpoint::parse("  HTTPS://Nacho.Example:8443/ ").unwrap();
        assert_eq!(e.base, "https://nacho.example:8443");
        assert_eq!(e.web_socket_url(), "wss://nacho.example:8443/ws/chat");
        assert_eq!(e.connector_url(), "wss://nacho.example:8443/ws/connector");
        assert_eq!(e.rest("/api/v1/me"), "https://nacho.example:8443/api/v1/me");
        assert_eq!(ServerEndpoint::parse("http://127.0.0.1:8787").unwrap().web_socket_url(), "ws://127.0.0.1:8787/ws/chat");
        assert!(ServerEndpoint::parse("http://[::1]:8787").is_ok());
        assert!(ServerEndpoint::parse("http://localhost").is_ok());
        assert_eq!(ServerEndpoint::parse(""), Err(Empty));
        assert_eq!(ServerEndpoint::parse(&format!("https://{}.com", "a".repeat(200))), Err(TooLong));
        assert_eq!(ServerEndpoint::parse("ftp://x.com"), Err(InvalidScheme));
        assert_eq!(ServerEndpoint::parse("nacho.example"), Err(InvalidScheme));
        assert_eq!(ServerEndpoint::parse("https://"), Err(NoHost));
        assert_eq!(ServerEndpoint::parse("https://u:p@x.com"), Err(HasCredentials));
        assert_eq!(ServerEndpoint::parse("https://x.com/?a=1"), Err(HasQueryOrFragment));
        assert_eq!(ServerEndpoint::parse("https://x.com#f"), Err(HasQueryOrFragment));
        assert_eq!(ServerEndpoint::parse("http://192.168.0.2"), Err(PlainHTTPNotLoopback));
        assert_eq!(ServerEndpoint::parse("https://x.com:99999"), Err(NoHost));
        assert_eq!(ServerEndpoint::parse("https://x.com:0"), Err(NoHost));
    }

    #[test]
    fn issuer_urls() {
        assert_eq!(IssuerURL::parse("https://auth.example/realms/nachotown/").unwrap(), "https://auth.example/realms/nachotown");
        assert_eq!(IssuerURL::parse("https://auth.example/auth/realms/n_1").unwrap(), "https://auth.example/auth/realms/n_1");
        assert_eq!(IssuerURL::parse("https://auth.example/"), Err(BadPath));
        assert_eq!(IssuerURL::parse("https://auth.example/realms/a b"), Err(BadPath));
        assert_eq!(ServerEndpoint::parse("https://na cho.example"), Err(NoHost));
        assert_eq!(IssuerURL::parse("https://auth.example/realm/x"), Err(BadPath));
        assert_eq!(IssuerURL::parse("https://auth.example/realms/.."), Err(BadPath));
        assert_eq!(IssuerURL::parse("http://evil.example/realms/x"), Err(PlainHTTPNotLoopback));
    }

    #[test]
    fn modes_resolve_and_scope_tokens() {
        let mut s = PetSettings::default();
        assert_eq!(s.endpoint().unwrap().base, PetSettings::PRODUCTION_SERVER_URL);
        assert_eq!(s.oidc().unwrap().issuer, PetSettings::PRODUCTION_ISSUER_URL);
        let prod = s.account_scope();
        s.server_mode = PetServerMode::Mock;
        assert_ne!(s.account_scope(), prod);
        s.server_mode = PetServerMode::Custom;
        assert!(s.endpoint().is_none());
        assert_eq!(s.validation_problem().as_deref(), Some("서버 주소: 주소를 입력해 주세요. 예: https://example.com"));
        s.custom_server_url = "https://n.example".into();
        s.custom_issuer_url = "https://a.example/realms/r".into();
        assert_eq!(s.validation_problem(), None);
        assert_eq!(s.account_scope(), "https://n.example|https://a.example/realms/r");
    }

    #[test]
    fn settings_file_is_lenient_and_sanitized() {
        let s = PetSettings::from_json(r#"{"serverMode":"custom","customServerURL":"http://evil.example","allowComboControl":"yes","speakVoiceReplies":false,"petPosition":{"x":1e9,"y":2},"future":1}"#).unwrap();
        assert!(s.allow_combo_control, "wrong type falls back to the default");
        assert!(!s.speak_voice_replies);
        let clean = PetSettings::sanitized(s);
        assert_eq!(clean.server_mode, PetServerMode::Production);
        assert!(clean.custom_server_url.is_empty());
        assert!(clean.pet_position.is_none());
        let d = PetSettings { pet_position: Some(PetPosition { x: 10.0, y: 20.0 }), ..Default::default() };
        assert_eq!(PetSettings::from_json(&d.to_json()).unwrap(), d);
        assert!(PetSettings::from_json("[1]").is_none());
    }

    #[test]
    fn store_moves_a_corrupt_file_aside() {
        let dir = std::env::temp_dir().join(format!("nacho-pet-settings-{}", super::super::media::PetIDs::new_uuid()));
        let store = PetSettingsStore::new(dir.join("pet.json"));
        assert_eq!(store.load(), PetSettings::default());
        let s = PetSettings { speak_voice_replies: false, ..Default::default() };
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
        std::fs::write(dir.join("pet.json"), "{not json").unwrap();
        assert_eq!(store.load(), PetSettings::default());
        assert!(dir.join("pet.json.corrupt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Ported from PetSettingsAndConnectorsTests / PetChatLayoutTests / PetVoiceChoiceTests (the PetSettings parts).
    fn temp_dir() -> std::path::PathBuf { std::env::temp_dir().join(format!("nacho-pet-settings-{}", super::super::media::PetIDs::new_uuid())) }

    #[test]
    fn defaults_are_positionless_production() {
        let s = PetSettings::default();
        assert_eq!(s.server_mode, PetServerMode::Production);
        assert_eq!(s.endpoint().unwrap().base, "https://nacho.nachotown.com");
        assert_eq!(s.oidc().unwrap().issuer, "https://auth.nachotown.com/realms/nachotown");
        assert!(s.pet_position.is_none());
        assert!(s.allow_combo_control);
        assert!(s.validation_problem().is_none());
    }

    #[test]
    fn modes_resolve_to_their_servers() {
        let mut s = PetSettings { server_mode: PetServerMode::Local, ..Default::default() };
        assert_eq!(s.endpoint().unwrap().base, "http://127.0.0.1:8787");
        assert_eq!(s.oidc().unwrap().issuer, "http://localhost:18180/realms/nachotown");
        s.server_mode = PetServerMode::Mock;
        assert_eq!(s.oidc().unwrap().issuer, "http://127.0.0.1:8787/realms/nachotown");
        s.server_mode = PetServerMode::Custom;
        assert!(s.endpoint().is_none());
        assert!(s.validation_problem().is_some());
        s.custom_server_url = "https://nacho.example.com".into();
        s.custom_issuer_url = "https://id.example.com/auth/realms/nachotown/".into();
        assert!(s.validation_problem().is_none());
        assert_eq!(s.oidc().unwrap().issuer, "https://id.example.com/auth/realms/nachotown");
        assert_ne!(s.account_scope(), PetSettings::default().account_scope(), "tokens of one server never go to another");
    }

    #[test]
    fn issuer_validation() {
        assert_eq!(IssuerURL::parse("https://x.com"), Err(BadPath));
        assert_eq!(IssuerURL::parse("https://x.com/realms/"), Err(BadPath));
        assert_eq!(IssuerURL::parse("https://x.com/realms/a/../b"), Err(BadPath));
        assert_eq!(IssuerURL::parse("https://x.com/realms/na cho"), Err(BadPath));
        assert_eq!(IssuerURL::parse("http://x.com/realms/a"), Err(PlainHTTPNotLoopback));
        assert_eq!(IssuerURL::parse("https://x.com/realms/a?x=1"), Err(HasQueryOrFragment));
        assert_eq!(IssuerURL::parse("https://X.com/realms/nachotown").unwrap(), "https://x.com/realms/nachotown");
    }

    #[test]
    fn round_trip_and_persisted_position() {
        let dir = temp_dir();
        let store = PetSettingsStore::new(dir.join("pet.json"));
        let s = PetSettings {
            server_mode: PetServerMode::Custom,
            custom_server_url: "https://nacho.example.com".into(),
            custom_issuer_url: "https://id.example.com/realms/nachotown".into(),
            pet_position: Some(PetPosition { x: 120.5, y: 300.0 }),
            allow_combo_control: false,
            ..Default::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
        assert_eq!(PetSettingsStore::new(dir.join("none.json")).load(), PetSettings::default(), "missing file gives defaults");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hand_edited_bad_values_are_sanitized() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pet.json");
        std::fs::write(&path, r#"{"serverMode":"custom","customServerURL":"http://evil.example.com","customIssuerURL":"https://e.com/realms/x","petPosition":{"x":1e300,"y":2},"future":1}"#).unwrap();
        let s = PetSettingsStore::new(&path).load();
        assert_eq!(s.server_mode, PetServerMode::Production);
        assert_eq!(s.custom_server_url, "");
        assert!(s.pet_position.is_none());
        std::fs::write(&path, r#"{"serverMode":"moon","allowComboControl":false}"#).unwrap();
        let t = PetSettingsStore::new(&path).load();
        assert_eq!(t.server_mode, PetServerMode::Production);
        assert!(!t.allow_combo_control);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_chat_size_survives_a_relaunch() {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pet.json");
        std::fs::write(&path, r#"{"serverMode":"production","chatSize":{"width":520,"height":640}}"#).unwrap();
        let first = PetSettingsStore::new(&path);
        first.save(&first.load()).unwrap(); // the app saves its settings on the next change
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["chatSize"]["width"], 520.0, "the chat size was dropped on load or save");
        assert_eq!(v["chatSize"]["height"], 640.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_size_setting_round_trip_and_sanitize() {
        let dir = temp_dir();
        let path = dir.join("pet.json");
        let store = PetSettingsStore::new(&path);
        let mut s = PetSettings::default();
        assert!(s.chat_size.is_none());
        s.chat_size = Some(PetChatSize { width: 480.0, height: 600.0 });
        store.save(&s).unwrap();
        assert_eq!(PetSettingsStore::new(&path).load().chat_size, Some(PetChatSize { width: 480.0, height: 600.0 }));
        // A hand-edited file cannot make it tiny, absurd or not a number; old files without the key load as default.
        s.chat_size = Some(PetChatSize { width: 5.0, height: 5.0 });
        assert_eq!(PetSettings::sanitized(s.clone()).chat_size, Some(PetChatLayout::MINIMUM_SIZE));
        s.chat_size = Some(PetChatSize { width: 1e9, height: 600.0 });
        assert!(PetSettings::sanitized(s.clone()).chat_size.is_none());
        s.chat_size = Some(PetChatSize { width: f64::INFINITY, height: 600.0 });
        assert!(PetSettings::sanitized(s).chat_size.is_none());
        std::fs::write(&path, r#"{"serverMode":"production","chatSize":"big"}"#).unwrap();
        let loaded = PetSettingsStore::new(&path).load();
        assert!(loaded.chat_size.is_none());
        assert_eq!(loaded.server_mode, PetServerMode::Production);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn speak_setting_is_on_by_default_and_survives_old_files() {
        assert!(PetSettings::default().speak_voice_replies);
        let old = PetSettings::from_json(r#"{"serverMode":"production","allowComboControl":true}"#).unwrap();
        assert!(old.speak_voice_replies);
        let s = PetSettings { speak_voice_replies: false, ..Default::default() };
        assert!(!PetSettings::from_json(&s.to_json()).unwrap().speak_voice_replies);
    }

    #[test]
    fn selection_bar_settings_live_under_selection_bar() {
        let s = PetSettings::from_json(r#"{"selectionBar":{"enabled":true,"order":["ask","nope"],"delay":"moon"}}"#).unwrap();
        assert!(s.selection_bar.enabled);
        let back = PetSettings::from_json(&s.to_json()).unwrap();
        assert_eq!(back.selection_bar, s.selection_bar);
        assert!(PetSettings::from_json(r#"{"speakVoiceReplies":false}"#).unwrap().selection_bar == PetSelectionBarSettings::default());
    }
}
