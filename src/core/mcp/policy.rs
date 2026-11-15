//! Who may talk to the MCP server and under which Host name (the local-only half of
//! `ComputerUseNetworkPolicy.swift`). Windows has no LAN mode: the socket binds to 127.0.0.1 only, and the service
//! still refuses any peer that is not loopback (403 before the key check) and any Host / Origin that is not this PC
//! (DNS rebinding, cross-site browser calls). Everything fails closed: text that does not parse strictly is refused.
use std::net::IpAddr;
use std::str::FromStr;

pub struct NetworkPolicy;

impl NetworkPolicy {
    /// The only address the server binds to.
    pub const BIND_ADDRESS: &'static str = "127.0.0.1";

    fn parse(raw: &str) -> Option<IpAddr> {
        let t = raw.trim();
        let t = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(t);
        // A zone id ("fe80::1%3") is not loopback anyway; drop it before parsing.
        let t = match t.split_once('%') { Some((a, z)) if a.contains(':') && !z.is_empty() => a, Some(_) => return None, None => t };
        let ip = IpAddr::from_str(t).ok()?;
        Some(ip.to_canonical())
    }

    /// 127/8 and ::1 (IPv4-mapped IPv6 judged by the IPv4 inside). Anything that does not parse is not loopback.
    pub fn is_loopback(client: &str) -> bool { Self::parse(client).is_some_and(|ip| ip.is_loopback()) }

    /// One spelling per address, so a client cannot dodge a lockout by changing the notation. Text that does not parse
    /// is lower-cased and cut to 64 characters so a hostile value cannot grow the limiter tables.
    pub fn normalize_client(raw: &str) -> String {
        match Self::parse(raw) { Some(ip) => ip.to_string(), None => raw.trim().to_lowercase().chars().take(64).collect() }
    }

    /// `localhost`, `127.0.0.1` or `[::1]`, each with an optional port. Userinfo, paths, bare IPv6, an empty port,
    /// other loopback spellings and every other name are refused.
    pub fn is_host_allowed(host: &str) -> bool {
        let v = host.trim();
        let valid_port = |p: &str| (1..=5).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit());
        let name = if let Some(rest) = v.strip_prefix('[') {
            let Some((inside, after)) = rest.split_once(']') else { return false };
            if !after.is_empty() && !after.strip_prefix(':').is_some_and(valid_port) { return false; }
            return inside == "::1";
        } else {
            let mut parts = v.split(':');
            let name = parts.next().unwrap_or("");
            match (parts.next(), parts.next()) {
                (None, _) => name,
                (Some(p), None) if valid_port(p) => name,
                _ => return false,
            }
        };
        let name = name.to_ascii_lowercase();
        let name = name.strip_suffix('.').unwrap_or(&name);
        name == "localhost" || name == "127.0.0.1"
    }

    /// An Origin header (browsers send one on cross-site calls): its authority must pass the Host rule. `null` and
    /// anything without a scheme are refused.
    pub fn is_origin_allowed(origin: &str) -> bool {
        let Some((_, rest)) = origin.split_once("://") else { return false };
        let authority = rest.split('/').next().unwrap_or("");
        Self::is_host_allowed(authority)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_peers() {
        for ok in ["127.0.0.1", "127.8.9.10", "::1", "[::1]", "::ffff:127.0.0.1"] { assert!(NetworkPolicy::is_loopback(ok), "{ok}"); }
        for no in ["192.168.0.5", "10.0.0.1", "100.64.0.1", "8.8.8.8", "::ffff:192.168.0.5", "fe80::1%3", "unknown", "", "0127.0.0.1", "127.1", "127.0.0.1:80"] {
            assert!(!NetworkPolicy::is_loopback(no), "{no}");
        }
    }

    #[test]
    fn one_spelling_per_address() {
        assert_eq!(NetworkPolicy::normalize_client("::ffff:127.0.0.1"), "127.0.0.1");
        assert_eq!(NetworkPolicy::normalize_client("[::1]"), "::1");
        assert_eq!(NetworkPolicy::normalize_client("0:0:0:0:0:0:0:1"), "::1");
        assert_eq!(NetworkPolicy::normalize_client(&"X".repeat(100)).len(), 64);
    }

    #[test]
    fn host_and_origin_must_be_this_pc() {
        for ok in ["localhost", "localhost:47653", "LOCALHOST.", "127.0.0.1", "127.0.0.1:47653", "[::1]", "[::1]:47653"] {
            assert!(NetworkPolicy::is_host_allowed(ok), "{ok}");
        }
        for no in ["evil.com", "127.0.0.1.nip.io", "192.168.0.5", "::1", "[::2]", "a@localhost", "localhost/x", "localhost:", "localhost:123456",
            "127.0.0.2", "0.0.0.0", "[::1]x", ""] {
            assert!(!NetworkPolicy::is_host_allowed(no), "{no}");
        }
        assert!(NetworkPolicy::is_origin_allowed("http://localhost:3000"));
        assert!(NetworkPolicy::is_origin_allowed("vscode-webview://127.0.0.1/x"));
        assert!(!NetworkPolicy::is_origin_allowed("null"));
        assert!(!NetworkPolicy::is_origin_allowed("https://evil.example"));
    }
}
