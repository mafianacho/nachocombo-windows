//! The socket of the NachoCombo MCP server: Streamable HTTP, one JSON answer per request (`Connection: close`, no SSE,
//! no sessions), as on macOS. Standard library sockets only, so the same code runs in the Windows app and in the Linux
//! tests (a real client round trip).
//!
//! - Binds to **127.0.0.1 only** (no LAN mode on Windows): other computers cannot reach the port at all, and a peer
//!   that is somehow not loopback is dropped before a byte is read (the service refuses it again with 403).
//! - At most 16 connections at a time; a connection that has not sent a whole request within 15 seconds is dropped.
//! - [`McpServer::stop`] closes the listening socket, ends every open connection and joins the accept thread (the
//!   OFF contract: switching the server off leaves no socket and no thread behind).
use super::http::{HttpParse, HttpParser, HttpResponse};
use super::policy::NetworkPolicy;
use super::service::McpService;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct McpServer {
    addr: SocketAddr,
    stopping: Arc<AtomicBool>,
    service: Arc<McpService>,
    open: Arc<Mutex<HashMap<u64, TcpStream>>>,
    accept: Option<JoinHandle<()>>,
}

impl McpServer {
    pub const MAX_CONNECTIONS: usize = 16;
    pub const REQUEST_SECONDS: u64 = 15;

    /// Opens `127.0.0.1:<port>` (port 0 = any free port, for tests).
    pub fn start(service: Arc<McpService>, port: u16) -> Result<McpServer, String> {
        let listener = TcpListener::bind((NetworkPolicy::BIND_ADDRESS, port)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse { format!("Port {port} is already in use by another program.") } else { format!("The MCP server could not open port {port}: {e}") }
        })?;
        let addr = listener.local_addr().map_err(|e| e.to_string())?;
        let stopping = Arc::new(AtomicBool::new(false));
        let open: Arc<Mutex<HashMap<u64, TcpStream>>> = Arc::new(Mutex::new(HashMap::new()));
        let (st, sv, op) = (stopping.clone(), service.clone(), open.clone());
        let accept = std::thread::Builder::new().name("nachocombo-mcp".into()).spawn(move || Self::accept_loop(listener, st, sv, op)).map_err(|e| e.to_string())?;
        Ok(McpServer { addr, stopping, service, open, accept: Some(accept) })
    }

    pub fn local_addr(&self) -> SocketAddr { self.addr }

    pub fn url(&self) -> String { format!("http://{}{}", self.addr, McpService::ENDPOINT) }

    pub fn open_connections(&self) -> usize { self.open.lock().unwrap().len() }

    fn accept_loop(listener: TcpListener, stopping: Arc<AtomicBool>, service: Arc<McpService>, open: Arc<Mutex<HashMap<u64, TcpStream>>>) {
        let next = AtomicU64::new(1);
        for stream in listener.incoming() {
            if stopping.load(Ordering::SeqCst) { break; }
            let Ok(stream) = stream else { continue };
            let Ok(peer) = stream.peer_addr() else { continue };
            // Not loopback: dropped before reading anything.
            if !NetworkPolicy::is_loopback(&peer.ip().to_string()) { let _ = stream.shutdown(Shutdown::Both); continue; }
            let id = next.fetch_add(1, Ordering::SeqCst);
            {
                let mut o = open.lock().unwrap();
                if o.len() >= Self::MAX_CONNECTIONS {
                    drop(o);
                    let mut s = stream;
                    let _ = s.write_all(&HttpResponse::error(503, "too many connections").serialized());
                    let _ = s.shutdown(Shutdown::Both);
                    continue;
                }
                match stream.try_clone() { Ok(c) => { o.insert(id, c); } Err(_) => continue }
            }
            let (sv, op) = (service.clone(), open.clone());
            let spawned = std::thread::Builder::new().name("nachocombo-mcp-conn".into()).spawn(move || {
                Self::serve(stream, &peer.ip().to_string(), &sv);
                op.lock().unwrap().remove(&id);
            });
            if spawned.is_err() { open.lock().unwrap().remove(&id); }
        }
        // Closing: the listener drops here, which closes the port.
    }

    fn serve(mut stream: TcpStream, peer: &str, service: &McpService) {
        let deadline = Instant::now() + Duration::from_secs(Self::REQUEST_SECONDS);
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        let response = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { break None; }
            let _ = stream.set_read_timeout(Some(left));
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break None,
                Ok(n) => {
                    buffer.extend_from_slice(&chunk[..n]);
                    match HttpParser::parse(&buffer) {
                        HttpParse::NeedMore => continue,
                        HttpParse::Invalid(r) => break Some(r),
                        HttpParse::Request(req, _) => break Some(service.handle(&req, peer)),
                    }
                }
            }
        };
        if let Some(r) = response {
            let _ = stream.set_write_timeout(Some(Duration::from_secs(Self::REQUEST_SECONDS)));
            let _ = stream.write_all(&r.serialized());
            let _ = stream.flush();
        }
        let _ = stream.shutdown(Shutdown::Both);
    }

    /// Closes the port and every open connection, refuses what is still being handled (503) and joins the accept thread.
    pub fn stop(&mut self) {
        if self.stopping.swap(true, Ordering::SeqCst) { return; }
        self.service.stop();
        // Wake the blocking accept so the loop sees `stopping` and drops the listener.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
        if let Some(h) = self.accept.take() { let _ = h.join(); }
        for (_, s) in self.open.lock().unwrap().drain() { let _ = s.shutdown(Shutdown::Both); }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) { self.stop(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp::service::tests::rig;
    use crate::core::mcp::service::McpApproval;
    use serde_json::{json, Value};
    use std::io::BufRead;

    /// A minimal MCP client over a real socket (what an MCP client sends: POST, Bearer, JSON).
    fn post(addr: SocketAddr, token: &str, body: &Value) -> (u16, Value) {
        let mut s = TcpStream::connect(addr).unwrap();
        let data = serde_json::to_vec(body).unwrap();
        let head = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n", addr.port(), data.len());
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(&data).unwrap();
        let mut reader = std::io::BufReader::new(s);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        let code: u16 = status.split(' ').nth(1).unwrap().parse().unwrap();
        let mut rest = String::new();
        reader.read_to_string(&mut rest).unwrap();
        let body = rest.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        (code, if body.is_empty() { Value::Null } else { serde_json::from_str(body).unwrap() })
    }

    #[test]
    fn a_client_round_trip_over_the_socket() {
        let r = rig(Some(McpApproval::Allowed));
        r.enable("clipboard");
        let crate::core::mcp::service::tests::Rig { service, token, .. } = r;
        let mut server = McpServer::start(Arc::new(service), 0).unwrap_or_else(|e| panic!("{e}"));
        let addr = server.local_addr();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        let (code, init) = post(addr, &token, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}));
        assert_eq!(code, 200);
        assert_eq!(init["result"]["serverInfo"]["name"], "NachoCombo MCP");
        assert_eq!(post(addr, &token, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).0, 202);
        let (_, list) = post(addr, &token, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["clipboard_list", "clipboard_get_item", "clipboard_copy_again"]);
        let (_, call) = post(addr, &token, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "clipboard_get_item", "arguments": {"index": 1}}}));
        assert_eq!(call["result"]["isError"], false, "{call}");
        assert_eq!(post(addr, "nccu_wrong", &json!({"jsonrpc": "2.0", "id": 4, "method": "ping"})).0, 401);
        server.stop();
        assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err(), "the port is closed after stop");
        assert_eq!(server.open_connections(), 0);
    }

    #[test]
    fn only_loopback_is_bound() {
        let r = rig(None);
        let server = McpServer::start(Arc::new(r.service), 0).unwrap();
        let port = server.local_addr().port();
        assert!(server.url().starts_with("http://127.0.0.1:"));
        // Any other address of this machine does not answer on the port.
        if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if sock.connect("192.0.2.1:9").is_ok() {
                if let Ok(local) = sock.local_addr() {
                    if !local.ip().is_loopback() && !local.ip().is_unspecified() {
                        assert!(TcpStream::connect_timeout(&SocketAddr::new(local.ip(), port), Duration::from_millis(500)).is_err(), "{} answered", local.ip());
                    }
                }
            }
        }
        assert!(TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(500)).is_ok());
    }

    #[test]
    fn a_busy_port_is_reported() {
        let blocker = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = blocker.local_addr().unwrap().port();
        let r = rig(None);
        let e = McpServer::start(Arc::new(r.service), port).err().unwrap();
        assert!(e.contains(&port.to_string()), "{e}");
    }

    #[test]
    fn a_slow_or_broken_request_gets_no_hang() {
        let r = rig(None);
        let server = McpServer::start(Arc::new(r.service), 0).unwrap();
        let mut s = TcpStream::connect(server.local_addr()).unwrap();
        s.write_all(b"POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 411"), "{out}");
    }
}
