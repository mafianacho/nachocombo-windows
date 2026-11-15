//! Plain-TCP WebSocket client (std `TcpStream`) for the dev CLI and the tests: `ws://` on this computer only (the mock
//! server, a local nacho-server), no TLS. The shipped app talks to the server through WinHTTP instead. A reader thread
//! per connection delivers the events. Port of `PetSocketTransport.swift` (there Linux-only on Glibc sockets); the
//! upgrade answer's `Sec-WebSocket-Accept` is checked here too.
use super::chat_client::{TransportEvent, TransportHandler, WebSocketTransport};
use super::crypto;
use super::reconnect::TransportFailure;
use super::settings::{ServerEndpoint, URLParts};
use super::ws_framing::{WSDecoder, WSEncoder, WSMessage};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Default)]
struct Inner {
    /// The socket for writes and for `close` (set once TCP is up).
    stream: Mutex<Option<TcpStream>>,
    /// The upgrade succeeded: frames may be written.
    open: AtomicBool,
    state: Mutex<ReportState>,
    handler: Mutex<Option<TransportHandler>>,
}

#[derive(Default)]
struct ReportState {
    closed_by_caller: bool,
    reported: bool,
}

/// One connection attempt (the chat client makes a new one per attempt).
#[derive(Default)]
pub struct SocketTransport {
    inner: Arc<Inner>,
}

impl SocketTransport {
    pub fn new() -> Self { Self::default() }

    /// The `Sec-WebSocket-Accept` a server must answer for `key` (RFC 6455 §4.2.2).
    pub fn accept_value(key: &str) -> String { crypto::base64(&crypto::sha1(format!("{key}{WS_GUID}").as_bytes())) }

    /// `ws://host[:port]/path` on loopback → (host for the socket, Host header, port, path); else the failure to report.
    pub fn target(url: &str) -> Result<(String, String, u16, String), TransportFailure> {
        let lower = url.trim().to_lowercase();
        if lower.starts_with("wss://") {
            return Err(TransportFailure::new(None, "SocketTransport는 TLS(wss://)를 지원하지 않아요. 개발용 loopback ws:// 만 돼요"));
        }
        let refuse = || TransportFailure::new(None, "SocketTransport는 loopback ws:// 만 지원해요 (개발·CI 전용)");
        let Some((parts, cred, _)) = URLParts::split(url.trim()) else { return Err(refuse()) };
        if parts.scheme != "ws" || cred || !ServerEndpoint::is_loopback(&parts.host) { return Err(refuse()); }
        let port = parts.port.unwrap_or(80);
        let sock_host = parts.host.trim_start_matches('[').trim_end_matches(']').to_string();
        let path = if parts.path.is_empty() { "/".to_string() } else { parts.path.clone() };
        Ok((sock_host, format!("{}:{port}", parts.host), port, path))
    }

    /// The upgrade request (the bearer is left out when it could break the header).
    pub fn handshake_request(path: &str, host_header: &str, key: &str, bearer: Option<&str>) -> String {
        let mut req = format!("GET {path} HTTP/1.1\r\nHost: {host_header}\r\n");
        req += &format!("Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n");
        if let Some(b) = bearer.filter(|b| !b.is_empty() && !b.contains(['\r', '\n'])) { req += &format!("Authorization: Bearer {b}\r\n"); }
        req + "\r\n"
    }

    /// Checks the upgrade answer head (without the blank line): 101 and the right accept value.
    pub fn check_handshake(head: &str, key: &str) -> Result<(), TransportFailure> {
        let mut lines = head.split("\r\n");
        let status: Option<u16> = lines.next().and_then(|l| l.split(' ').nth(1)).and_then(|s| s.parse().ok());
        if status != Some(101) { return Err(TransportFailure::new(status, format!("HTTP {}", status.unwrap_or(0)))); }
        let accept = lines.filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept")).map(|(_, v)| v.trim());
        if accept != Some(Self::accept_value(key).as_str()) {
            return Err(TransportFailure::new(Some(101), "서버의 WebSocket 응답이 맞지 않아요 (Sec-WebSocket-Accept)"));
        }
        Ok(())
    }
}

/// Mask keys and the handshake key: not secrets (masking only keeps proxies from caching), so std's random hasher keys
/// are enough.
fn random_bytes<const N: usize>() -> [u8; N] {
    use std::hash::{BuildHasher, Hasher};
    let mut out = [0u8; N];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_usize(i);
        let b = h.finish().to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
    out
}

impl Inner {
    fn report(&self, ev: TransportEvent) {
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(ev, TransportEvent::Closed(_)) {
                if s.reported { return; }
                s.reported = true;
            } else if s.closed_by_caller {
                return;
            }
        }
        let h = self.handler.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(h) = h { h(ev); }
    }

    fn closed_by_caller(&self) -> bool { self.state.lock().unwrap_or_else(|e| e.into_inner()).closed_by_caller }

    fn write_all(&self, data: &[u8]) {
        let g = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut s) = g.as_ref() { let _ = s.write_all(data); }
    }

    fn lost(&self) { self.report(TransportEvent::Closed(Some(TransportFailure::new(None, "연결이 끊겼어요")))); }

    fn run(self: Arc<Self>, url: String, bearer: Option<String>) {
        let (host, host_header, port, path) = match SocketTransport::target(&url) {
            Ok(t) => t,
            Err(f) => return self.report(TransportEvent::Closed(Some(f))),
        };
        let Ok(addrs) = (host.as_str(), port).to_socket_addrs() else {
            return self.report(TransportEvent::Closed(Some(TransportFailure::new(None, "주소를 찾지 못했어요"))));
        };
        let Some(stream) = addrs.into_iter().find_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(5)).ok()) else {
            return self.report(TransportEvent::Closed(Some(TransportFailure::new(None, "서버에 연결하지 못했어요"))));
        };
        let _ = stream.set_nodelay(true);
        let Ok(mut reader) = stream.try_clone() else { return self.lost() };
        *self.stream.lock().unwrap_or_else(|e| e.into_inner()) = Some(stream);
        if self.closed_by_caller() {
            if let Some(s) = self.stream.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { let _ = s.shutdown(Shutdown::Both); }
            return;
        }

        let key = crypto::base64(&random_bytes::<16>());
        self.write_all(SocketTransport::handshake_request(&path, &host_header, &key, bearer.as_deref()).as_bytes());

        let mut head: Vec<u8> = vec![];
        let mut buf = vec![0u8; 16384];
        let extra = loop {
            let n = match reader.read(&mut buf) { Ok(0) | Err(_) => return self.lost(), Ok(n) => n };
            head.extend_from_slice(&buf[..n]);
            if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                let extra = head[i + 4..].to_vec();
                head.truncate(i);
                break extra;
            }
            if head.len() > 16384 {
                return self.report(TransportEvent::Closed(Some(TransportFailure::new(None, "응답 헤더가 너무 커요"))));
            }
        };
        if let Err(f) = SocketTransport::check_handshake(&String::from_utf8_lossy(&head), &key) {
            return self.report(TransportEvent::Closed(Some(f)));
        }
        self.open.store(true, Ordering::SeqCst);
        self.report(TransportEvent::Opened);

        let mut decoder = WSDecoder::new();
        let mut pending = extra;
        loop {
            match decoder.feed(&pending) {
                Ok(msgs) => {
                    for m in msgs {
                        match m {
                            WSMessage::Text(t) => self.report(TransportEvent::Text(t)),
                            WSMessage::Ping(d) => self.write_all(&WSEncoder::pong(&d, random_bytes())),
                            WSMessage::Close(_) => {
                                self.write_all(&WSEncoder::close(random_bytes())); // echo, then done
                                self.report(TransportEvent::Closed(None));
                                return;
                            }
                            WSMessage::Pong(_) | WSMessage::Binary(_) => {}
                        }
                    }
                }
                Err(e) => return self.report(TransportEvent::Closed(Some(TransportFailure::new(None, format!("프로토콜 오류: {e}"))))),
            }
            let n = match reader.read(&mut buf) { Ok(0) | Err(_) => return self.lost(), Ok(n) => n };
            pending = buf[..n].to_vec();
        }
    }
}

impl WebSocketTransport for SocketTransport {
    fn connect(&self, url: &str, bearer: Option<&str>, handler: TransportHandler) {
        *self.inner.handler.lock().unwrap_or_else(|e| e.into_inner()) = Some(handler);
        let inner = self.inner.clone();
        let (url, bearer) = (url.to_string(), bearer.map(str::to_string));
        std::thread::spawn(move || inner.run(url, bearer));
    }

    fn send(&self, text: &str) {
        if self.inner.open.load(Ordering::SeqCst) { self.inner.write_all(&WSEncoder::text(text, random_bytes())); }
    }

    /// Caller initiated: no `Closed` event follows.
    fn close(&self) {
        {
            let mut s = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            s.closed_by_caller = true;
            s.reported = true;
        }
        if self.inner.open.load(Ordering::SeqCst) { self.inner.write_all(&WSEncoder::close(random_bytes())); }
        if let Some(s) = self.inner.stream.lock().unwrap_or_else(|e| e.into_inner()).as_ref() { let _ = s.shutdown(Shutdown::Both); }
    }
}

#[cfg(test)]
mod tests {
    //! Not in the Swift tests (the Linux transport had none): a loopback server thread speaks the other side.
    use super::*;
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn events() -> (TransportHandler, mpsc::Receiver<TransportEvent>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |ev| { let _ = tx.lock().unwrap().send(ev); }), rx)
    }

    fn next(rx: &mpsc::Receiver<TransportEvent>) -> TransportEvent { rx.recv_timeout(Duration::from_secs(5)).expect("event") }

    /// Reads the upgrade request; answers with `status` (and the right accept value when `good_accept`).
    fn accept(s: &mut TcpStream, status: u16, good_accept: bool) -> String {
        let mut head = vec![];
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            s.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        let req = String::from_utf8(head).unwrap();
        let key = req.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ")).unwrap().trim().to_string();
        let acc = if good_accept { SocketTransport::accept_value(&key) } else { "nope".into() };
        let reason = if status == 101 { "Switching Protocols" } else { "Unauthorized" };
        write!(s, "HTTP/1.1 {status} {reason}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nsec-websocket-accept: {acc}\r\n\r\n").unwrap();
        req
    }

    /// One client frame (masked) → (opcode, payload).
    fn read_frame(s: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).unwrap();
        assert_eq!(h[1] & 0x80, 0x80, "client frames are masked");
        let mut len = (h[1] & 0x7f) as usize;
        if len == 126 {
            let mut l = [0u8; 2];
            s.read_exact(&mut l).unwrap();
            len = u16::from_be_bytes(l) as usize;
        }
        let mut key = [0u8; 4];
        s.read_exact(&mut key).unwrap();
        let mut p = vec![0u8; len];
        s.read_exact(&mut p).unwrap();
        (h[0] & 0x0f, p.iter().enumerate().map(|(i, b)| b ^ key[i % 4]).collect())
    }

    #[test]
    fn refuses_tls_and_non_loopback_urls() {
        let (h, rx) = events();
        SocketTransport::new().connect("wss://127.0.0.1:8443/ws/chat", None, h);
        let TransportEvent::Closed(Some(f)) = next(&rx) else { panic!("closed") };
        assert!(f.message.contains("wss://"));
        let (h, rx) = events();
        SocketTransport::new().connect("ws://example.com/ws/chat", None, h);
        assert!(matches!(next(&rx), TransportEvent::Closed(Some(_))));
        assert_eq!(SocketTransport::target("ws://[::1]:8787/ws/chat").unwrap(), ("::1".into(), "[::1]:8787".into(), 8787, "/ws/chat".into()));
        assert_eq!(SocketTransport::target("ws://localhost").unwrap().3, "/");
    }

    #[test]
    fn handshake_request_and_accept_check() {
        // RFC 6455 §1.3 sample key.
        assert_eq!(SocketTransport::accept_value("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        let r = SocketTransport::handshake_request("/ws/chat", "127.0.0.1:8787", "k", Some("t\r\nX: y"));
        assert!(!r.contains("Authorization"));
        assert!(SocketTransport::handshake_request("/", "h:1", "k", Some("tok")).contains("Authorization: Bearer tok\r\n"));
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        assert!(SocketTransport::check_handshake("HTTP/1.1 101 OK\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", key).is_ok());
        assert_eq!(SocketTransport::check_handshake("HTTP/1.1 401 Unauthorized", key).unwrap_err().http_status, Some(401));
        assert!(SocketTransport::check_handshake("HTTP/1.1 101 OK\r\nSec-WebSocket-Accept: x", key).is_err());
    }

    #[test]
    fn round_trip_with_a_loopback_server() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let req = accept(&mut s, 101, true);
            assert!(req.starts_with("GET /ws/chat HTTP/1.1\r\n"));
            assert!(req.contains("Authorization: Bearer tok\r\n"));
            s.write_all(&[0x81, 5]).unwrap(); // text "hello", split over two writes
            s.flush().unwrap();
            std::thread::sleep(Duration::from_millis(20));
            s.write_all(b"hello").unwrap();
            s.write_all(&[0x89, 1, 7]).unwrap(); // ping → pong
            // the pong (reader thread) and the text (test thread) may come in either order
            let mut got = vec![read_frame(&mut s), read_frame(&mut s)];
            got.sort();
            assert_eq!(got, vec![(0x1, br#"{"type":"ping"}"#.to_vec()), (0xA, vec![7])]);
            s.write_all(&[0x88, 2, 0x03, 0xe8]).unwrap(); // close
            assert_eq!(read_frame(&mut s).0, 0x8, "close is echoed");
        });
        let t = SocketTransport::new();
        let (h, rx) = events();
        t.connect(&format!("ws://127.0.0.1:{port}/ws/chat"), Some("tok"), h);
        assert_eq!(next(&rx), TransportEvent::Opened);
        assert_eq!(next(&rx), TransportEvent::Text("hello".into()));
        t.send(r#"{"type":"ping"}"#);
        assert_eq!(next(&rx), TransportEvent::Closed(None));
        server.join().unwrap();
    }

    #[test]
    fn http_status_and_bad_accept_fail_the_connection() {
        for (status, good, want) in [(401u16, true, Some(401u16)), (101, false, Some(101))] {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                let (mut s, _) = l.accept().unwrap();
                accept(&mut s, status, good);
            });
            let (h, rx) = events();
            SocketTransport::new().connect(&format!("ws://localhost:{port}/ws/chat"), None, h);
            let TransportEvent::Closed(Some(f)) = next(&rx) else { panic!("closed with a failure") };
            assert_eq!(f.http_status, want);
            server.join().unwrap();
        }
    }

    #[test]
    fn close_by_caller_reports_nothing() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            accept(&mut s, 101, true);
            assert_eq!(read_frame(&mut s).0, 0x8);
        });
        let t = SocketTransport::new();
        let (h, rx) = events();
        t.connect(&format!("ws://127.0.0.1:{port}/"), None, h);
        assert_eq!(next(&rx), TransportEvent::Opened);
        t.close();
        server.join().unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "no Closed after close()");
    }
}
