//! REST read models for the connector/tool screen. Port of `PetConnectors.swift`. Contracts: nacho-server
//! `GET /api/v1/connections` and `GET /api/v1/tools`. Decoded tolerantly: unknown fields are ignored, missing optional
//! fields are None.
use super::http::{PetHTTPClient, PetHTTPRequest};
use super::json::{JsonExt, JsonObject};
use super::settings::{ServerEndpoint, URLParts};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionRow {
    /// Connection id (uuid); the source when the server sends none.
    pub id: String,
    pub source: String,
    pub label: String,
    /// connected | needs_reauth | disconnected | error | <unknown>
    pub status: String,
    pub needs_attention: Option<String>,
    pub account_label: Option<String>,
    pub scope_summary: String,
    /// The service switch (`PATCH /connections/{id}` `enabled`). Absent = on.
    pub enabled: bool,
    /// Row version the server wants back as `expected_version` when the switch or the scope changes.
    pub version: i64,
    /// `granted_scope.level`: `read` | `read_write`, when the server says what is granted.
    pub scope_level: Option<String>,
    /// Optional server-provided management link (http/https only, see [`SafeLink`]).
    pub link: Option<String>,
}

impl ConnectionRow {
    pub fn status_text(&self) -> &'static str {
        match self.needs_attention.as_deref() {
            Some("needs_reauth") => return "다시 로그인이 필요해요",
            Some("degraded") => return "일부 기능이 불안정해요",
            Some("expiring") => return "곧 만료돼요",
            _ => {}
        }
        match self.status.as_str() {
            "connected" => "연결됨",
            "needs_reauth" => "다시 로그인이 필요해요",
            "disconnected" => "연결 끊김",
            "error" => "오류",
            _ => "알 수 없음",
        }
    }
    pub fn is_healthy(&self) -> bool { self.status == "connected" && self.needs_attention.is_none() }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRow {
    pub id: String,
    pub name: String,
    pub description: String,
    /// builtin | http | mcp | connector | connection
    pub kind: String,
    pub source: Option<String>,
    pub enabled: bool,
    /// The user's own switch (`user_enabled`). Absent (an older server) = on.
    pub user_enabled: bool,
    /// A default tool Nacho needs in every chat (`locked`). Absent = false.
    pub locked: bool,
}

impl ToolRow {
    pub fn kind_text(&self) -> &str {
        match self.kind.as_str() {
            "builtin" => "기본",
            "http" => "HTTP",
            "mcp" => "MCP",
            "connector" => "커넥터",
            "connection" => "서비스 연결",
            k => k,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRow {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectorOverview {
    pub connections: Vec<ConnectionRow>,
    pub tools: Vec<ToolRow>,
    pub capabilities: Vec<CapabilityRow>,
}

pub struct SafeLink;

impl SafeLink {
    /// Only http(s) links with a host may be opened in the browser (no file:, javascript:, custom schemes, no
    /// credentials; plain http only for this computer).
    pub fn openable(raw: Option<&str>) -> Option<String> {
        let s = raw?.trim();
        if s.is_empty() || s.chars().count() > 2000 || s.chars().any(|c| c.is_whitespace() || c.is_control()) { return None; }
        let (p, cred, _) = URLParts::split(s)?;
        if !(p.scheme == "https" || p.scheme == "http") || p.host.is_empty() || p.host == "[]" || cred { return None; }
        if p.scheme == "http" && !ServerEndpoint::is_loopback(&p.host) { return None; }
        Some(s.to_string())
    }
}

/// A JSON list whose every element is an object (anything else in it makes the whole list unreadable, as in the
/// macOS `as? [[String: Any]]`).
fn objects(v: Option<&Value>) -> Option<Vec<JsonObject>> {
    v?.as_array()?.iter().map(|x| x.as_object().cloned()).collect()
}

pub struct ConnectorDecoder;

impl ConnectorDecoder {
    pub fn connections(data: &[u8]) -> Option<Vec<ConnectionRow>> {
        let v: Value = serde_json::from_slice(data).ok()?;
        Some(objects(Some(&v))?.into_iter().filter_map(|o| {
            let source = o.str("source")?;
            Some(ConnectionRow {
                id: o.str("id").unwrap_or_else(|| source.clone()),
                label: o.str("label").unwrap_or_else(|| source.clone()),
                status: o.str("status").unwrap_or_else(|| "unknown".into()),
                needs_attention: o.str("needs_attention"),
                account_label: o.str("account_label"),
                scope_summary: o.str("scope_summary").unwrap_or_default(),
                enabled: o.bool("enabled").unwrap_or(true),
                version: o.get("version").and_then(Value::as_i64).unwrap_or(0),
                scope_level: o.obj("granted_scope").and_then(|g| g.str("level")),
                link: SafeLink::openable(o.str("link").or_else(|| o.str("manage_url")).as_deref()),
                source,
            })
        }).collect())
    }

    pub fn tools(data: &[u8]) -> Option<(Vec<ToolRow>, Vec<CapabilityRow>)> {
        let v: Value = serde_json::from_slice(data).ok()?;
        let o = v.as_object()?;
        let tools = objects(o.get("tools")).unwrap_or_default().into_iter().filter_map(|t| {
            let name = t.str("name")?;
            Some(ToolRow {
                id: t.str("id").unwrap_or_else(|| name.clone()),
                description: t.str("search_description").or_else(|| t.str("operation_description")).unwrap_or_default(),
                kind: t.obj("config").and_then(|c| c.str("kind")).unwrap_or_else(|| "builtin".into()),
                source: t.str("source"),
                enabled: t.bool("enabled").unwrap_or(true),
                user_enabled: t.bool("user_enabled").unwrap_or(true),
                locked: t.bool("locked").unwrap_or(false),
                name,
            })
        }).collect();
        let caps = objects(o.get("connector_capabilities")).unwrap_or_default().into_iter().filter_map(|c| {
            Some(CapabilityRow { name: c.str("name")?, description: c.str("description").unwrap_or_default() })
        }).collect();
        Some((tools, caps))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectorError { Unauthorized, Server(u16), Network(String), BadResponse }

impl ConnectorError {
    pub fn korean_message(&self) -> String {
        match self {
            Self::Unauthorized => "로그인이 필요해요.".into(),
            Self::Server(code) => format!("서버가 오류를 돌려줬어요. (HTTP {code})"),
            Self::Network(_) => "서버에 연결하지 못했어요. 서버 주소를 확인해 주세요.".into(),
            Self::BadResponse => "서버 응답을 읽지 못했어요.".into(),
        }
    }
}

type ToolsAndCaps = (Vec<ToolRow>, Vec<CapabilityRow>);
pub type OverviewCompletion = Box<dyn FnOnce(Result<ConnectorOverview, ConnectorError>) + Send>;

#[derive(Default)]
struct Pending {
    connections: Option<Result<Vec<ConnectionRow>, ConnectorError>>,
    tools: Option<Result<ToolsAndCaps, ConnectorError>>,
    completion: Option<OverviewCompletion>,
}

/// Reads connections + tools. Both calls must succeed; a partial list would hide a broken connection.
pub struct ConnectorsAPI {
    endpoint: ServerEndpoint,
    fetcher: Arc<dyn PetHTTPClient>,
    token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
}

impl ConnectorsAPI {
    pub fn new(endpoint: ServerEndpoint, fetcher: Arc<dyn PetHTTPClient>, token: Arc<dyn Fn() -> Option<String> + Send + Sync>) -> Self {
        Self { endpoint, fetcher, token }
    }

    fn status<T>(result: super::http::HTTPResult, decode: fn(&[u8]) -> Option<T>) -> Result<T, ConnectorError> {
        let resp = result.map_err(|e| ConnectorError::Network(e.0))?;
        if resp.status == 401 || resp.status == 403 { return Err(ConnectorError::Unauthorized); }
        if !resp.is_success() { return Err(ConnectorError::Server(resp.status)); }
        decode(&resp.body).ok_or(ConnectorError::BadResponse)
    }

    /// `completion` runs once, on the thread of whichever answer came last.
    pub fn load(&self, completion: OverviewCompletion) {
        let pending = Arc::new(Mutex::new(Pending { completion: Some(completion), ..Default::default() }));
        fn finish(p: &Mutex<Pending>) {
            let (c, t, done) = {
                let mut g = p.lock().unwrap();
                if g.connections.is_none() || g.tools.is_none() { return; }
                (g.connections.take().unwrap(), g.tools.take().unwrap(), g.completion.take())
            };
            let Some(done) = done else { return };
            done(match (c, t) {
                (Ok(c), Ok((tools, capabilities))) => Ok(ConnectorOverview { connections: c, tools, capabilities }),
                (Err(e), _) | (_, Err(e)) => Err(e),
            })
        }
        let bearer = (self.token)();
        let p = pending.clone();
        self.fetcher.send(PetHTTPRequest::get(self.endpoint.rest("/api/v1/connections"), bearer.as_deref()), Box::new(move |r| {
            p.lock().unwrap().connections = Some(Self::status(r, ConnectorDecoder::connections));
            finish(&p);
        }));
        let p = pending;
        self.fetcher.send(PetHTTPRequest::get(self.endpoint.rest("/api/v1/tools"), bearer.as_deref()), Box::new(move |r| {
            p.lock().unwrap().tools = Some(Self::status(r, ConnectorDecoder::tools));
            finish(&p);
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::pet::auth::testing::PetStubHTTP;
    use std::sync::mpsc;

    #[test]
    fn safe_link_only_allows_web_links() {
        assert!(SafeLink::openable(Some("https://example.com/a")).is_some());
        assert!(SafeLink::openable(Some("http://127.0.0.1:8787/x")).is_some());
        assert!(SafeLink::openable(Some("http://example.com")).is_none());
        assert!(SafeLink::openable(Some("file:///etc/passwd")).is_none());
        assert!(SafeLink::openable(Some("javascript:alert(1)")).is_none());
        assert!(SafeLink::openable(Some("nacho://connections")).is_none());
        assert!(SafeLink::openable(Some("https://u:p@example.com")).is_none());
        assert!(SafeLink::openable(Some("")).is_none());
        assert!(SafeLink::openable(None).is_none());
    }

    #[test]
    fn decode_connections() {
        let json = r#"[{"id":"i1","source":"google_calendar","label":"Google 캘린더","status":"connected","scope_summary":"읽기만","link":"https://x.com/m"},
             {"source":"naver_mail","status":"needs_reauth","needs_attention":"needs_reauth"},
             {"label":"no source"}]"#;
        let rows = ConnectorDecoder::connections(json.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Google 캘린더");
        assert!(rows[0].is_healthy());
        assert_eq!(rows[0].link.as_deref(), Some("https://x.com/m"));
        assert_eq!(rows[1].label, "naver_mail");
        assert_eq!(rows[1].status_text(), "다시 로그인이 필요해요");
        assert!(!rows[1].is_healthy());
        assert_eq!(ConnectorDecoder::connections(br#"{"a":1}"#), None);
    }

    #[test]
    fn decode_tools() {
        let json = r#"{"tools":[{"id":"t1","name":"search_web","search_description":"웹 검색","config":{"kind":"builtin"},"enabled":true},
                      {"name":"mail_search","config":{"kind":"connection"},"source":"naver_mail","enabled":false},{"x":1}],
             "connector_capabilities":[{"name":"browser_open","description":"열기"}]}"#;
        let (tools, caps) = ConnectorDecoder::tools(json.as_bytes()).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].kind_text(), "기본");
        assert_eq!(tools[1].kind_text(), "서비스 연결");
        assert!(!tools[1].enabled);
        assert_eq!(caps, vec![CapabilityRow { name: "browser_open".into(), description: "열기".into() }]);
        assert_eq!(ConnectorDecoder::tools(b"[]"), None);
    }

    fn load(f: &Arc<PetStubHTTP>) -> Result<ConnectorOverview, ConnectorError> {
        let ep = ServerEndpoint::parse("http://127.0.0.1:8787").unwrap();
        let (tx, rx) = mpsc::channel();
        ConnectorsAPI::new(ep, f.clone(), Arc::new(|| Some("mock-t".into()))).load(Box::new(move |r| tx.send(r).unwrap()));
        rx.recv().unwrap()
    }

    #[test]
    fn load_success_and_bearer() {
        let f = PetStubHTTP::new();
        f.on("GET", "/api/v1/connections", 200, r#"[{"source":"s","label":"L","status":"connected"}]"#);
        f.on("GET", "/api/v1/tools", 200, r#"{"tools":[],"connector_capabilities":[]}"#);
        assert_eq!(load(&f).map(|o| o.connections.len()), Ok(1));
        let r = f.requests();
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|r| r.header("Authorization") == Some("Bearer mock-t")));
    }

    #[test]
    fn load_error_mapping() {
        let f = PetStubHTTP::new();
        f.on("GET", "/api/v1/connections", 401, "");
        f.on("GET", "/api/v1/tools", 200, r#"{"tools":[]}"#);
        assert_eq!(load(&f), Err(ConnectorError::Unauthorized));
        f.on("GET", "/api/v1/connections", 500, "");
        assert_eq!(load(&f), Err(ConnectorError::Server(500)));
        f.on("GET", "/api/v1/connections", 200, "garbage");
        assert_eq!(load(&f), Err(ConnectorError::BadResponse));
        f.fail_network.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(load(&f), Err(ConnectorError::Network(_))));
    }
}
