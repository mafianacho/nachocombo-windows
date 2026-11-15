//! JSON-RPC 2.0 for the MCP endpoint (port of `ComputerUseJSON.swift`): single messages and batches, notifications
//! and client replies are acknowledged without an answer.
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// MCP "resource not found".
    pub const RESOURCE_NOT_FOUND: i64 = -32002;

    pub fn new(code: i64, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
    pub fn method_not_found(method: &str) -> Self { Self::new(Self::METHOD_NOT_FOUND, format!("Method not found: {}", cut(method, 64))) }
    pub fn invalid_params(message: impl Into<String>) -> Self { Self::new(Self::INVALID_PARAMS, message) }
}

/// At most `n` characters (names echoed back in errors).
pub fn cut(s: &str, n: usize) -> String { s.chars().take(n).collect() }

#[derive(Clone, Debug, PartialEq)]
pub enum RpcInbound {
    Request { id: Value, method: String, params: Option<Value> },
    Notification { method: String, params: Option<Value> },
    /// A reply to something the server asked. The server never asks, so it is acknowledged and ignored.
    Response,
    Invalid { id: Option<Value>, error: RpcError },
}

#[derive(Clone, Debug, PartialEq)]
pub enum RpcParsed {
    Single(RpcInbound),
    Batch(Vec<RpcInbound>),
    Invalid { id: Option<Value>, error: RpcError },
}

pub struct JsonRpc;

impl JsonRpc {
    pub fn parse(data: &[u8]) -> RpcParsed {
        let Ok(value) = serde_json::from_slice::<Value>(data) else {
            return RpcParsed::Invalid { id: None, error: RpcError::new(RpcError::PARSE_ERROR, "Parse error") };
        };
        match value {
            Value::Array(items) if items.is_empty() => RpcParsed::Invalid { id: None, error: RpcError::new(RpcError::INVALID_REQUEST, "Empty batch") },
            Value::Array(items) => RpcParsed::Batch(items.iter().map(Self::inbound).collect()),
            v => match Self::inbound(&v) {
                RpcInbound::Invalid { id, error } => RpcParsed::Invalid { id, error },
                m => RpcParsed::Single(m),
            },
        }
    }

    fn inbound(value: &Value) -> RpcInbound {
        let bad = |id: Option<Value>| RpcInbound::Invalid { id, error: RpcError::new(RpcError::INVALID_REQUEST, "Invalid Request") };
        let Value::Object(o) = value else { return bad(None) };
        // The id is echoed in errors only when it is a string, a number or null.
        let id = match o.get("id") {
            None => None,
            Some(v @ (Value::String(_) | Value::Number(_) | Value::Null)) => Some(v.clone()),
            Some(_) => return bad(None),
        };
        if o.get("jsonrpc") != Some(&Value::String("2.0".into())) { return bad(id); }
        if o.get("method").is_none() && (o.contains_key("result") || o.contains_key("error")) { return RpcInbound::Response; }
        let Some(Value::String(method)) = o.get("method") else { return bad(id) };
        let params = o.get("params").cloned();
        match id {
            Some(id) if !id.is_null() => RpcInbound::Request { id, method: method.clone(), params },
            _ => RpcInbound::Notification { method: method.clone(), params },
        }
    }

    pub fn result(id: &Value, result: Value) -> Value { json!({"jsonrpc": "2.0", "id": id, "result": result}) }

    pub fn error(id: Option<&Value>, error: &RpcError) -> Value {
        json!({"jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "error": {"code": error.code, "message": error.message}})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_requests_notifications_and_replies() {
        assert_eq!(JsonRpc::parse(br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
            RpcParsed::Single(RpcInbound::Request { id: json!(1), method: "ping".into(), params: None }));
        assert_eq!(JsonRpc::parse(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            RpcParsed::Single(RpcInbound::Notification { method: "notifications/initialized".into(), params: None }));
        assert_eq!(JsonRpc::parse(br#"{"jsonrpc":"2.0","id":null,"method":"x"}"#),
            RpcParsed::Single(RpcInbound::Notification { method: "x".into(), params: None }));
        assert_eq!(JsonRpc::parse(br#"{"jsonrpc":"2.0","id":"a","result":{}}"#), RpcParsed::Single(RpcInbound::Response));
        let RpcParsed::Batch(b) = JsonRpc::parse(br#"[{"jsonrpc":"2.0","id":"a","method":"ping"},{"bad":1}]"#) else { panic!() };
        assert_eq!(b.len(), 2);
        assert!(matches!(b[1], RpcInbound::Invalid { .. }));
    }

    #[test]
    fn refuses_broken_messages_with_the_standard_codes() {
        let code = |p: RpcParsed| match p { RpcParsed::Invalid { error, .. } => error.code, other => panic!("{other:?}") };
        assert_eq!(code(JsonRpc::parse(b"{not json")), RpcError::PARSE_ERROR);
        assert_eq!(code(JsonRpc::parse(b"[]")), RpcError::INVALID_REQUEST);
        assert_eq!(code(JsonRpc::parse(br#"{"id":1,"method":"ping"}"#)), RpcError::INVALID_REQUEST);
        assert_eq!(code(JsonRpc::parse(br#"{"jsonrpc":"2.0","id":{"x":1},"method":"ping"}"#)), RpcError::INVALID_REQUEST);
        assert_eq!(code(JsonRpc::parse(br#"{"jsonrpc":"2.0","id":2,"method":5}"#)), RpcError::INVALID_REQUEST);
        // The id comes back when it is a string or a number.
        let RpcParsed::Invalid { id, .. } = JsonRpc::parse(br#"{"jsonrpc":"1.0","id":"q","method":"ping"}"#) else { panic!() };
        assert_eq!(id, Some(json!("q")));
    }

    #[test]
    fn encodes_results_and_errors() {
        assert_eq!(JsonRpc::result(&json!(3), json!({})), json!({"jsonrpc":"2.0","id":3,"result":{}}));
        assert_eq!(JsonRpc::error(None, &RpcError::method_not_found("nope")), json!({"jsonrpc":"2.0","id":null,"error":{"code":-32601,"message":"Method not found: nope"}}));
    }
}
