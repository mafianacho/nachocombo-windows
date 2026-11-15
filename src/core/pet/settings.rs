//! Nacho Pet settings, part 1: server URL validation (`ServerEndpoint`, `IssuerURL`) without a URL crate — credentials,
//! query and fragment refused, plain http only for this computer. Port of `PetSettings.swift`.
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
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) { return Err(ServerURLError::InvalidScheme); }
        let Some((scheme, _)) = s.split_once("://") else { return Err(ServerURLError::InvalidScheme) };
        let scheme = scheme.to_lowercase();
        if scheme != "http" && scheme != "https" { return Err(ServerURLError::InvalidScheme); }
        let Some((c, cred, query)) = URLParts::split(s) else { return Err(ServerURLError::NoHost) };
        if c.host.is_empty() || c.host == "[]" { return Err(ServerURLError::NoHost); }
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
        assert_eq!(IssuerURL::parse("https://auth.example/realms/a b"), Err(InvalidScheme));
        assert_eq!(IssuerURL::parse("https://auth.example/realm/x"), Err(BadPath));
        assert_eq!(IssuerURL::parse("https://auth.example/realms/.."), Err(BadPath));
        assert_eq!(IssuerURL::parse("http://evil.example/realms/x"), Err(PlainHTTPNotLoopback));
    }
}
