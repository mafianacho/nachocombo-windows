//! Lenient JSON reading shared by the pet modules (the Swift `Dictionary<String, Any>` helpers of
//! `PetServerModels.swift`): a wrong type reads as "missing", so a server that adds or changes a field never breaks the pet.
use serde_json::{Map, Value};

pub type JsonObject = Map<String, Value>;

pub trait JsonExt {
    fn str(&self, k: &str) -> Option<String>;
    fn int(&self, k: &str) -> Option<i64>;
    fn bool(&self, k: &str) -> Option<bool>;
    fn obj(&self, k: &str) -> Option<JsonObject>;
    fn objs(&self, k: &str) -> Vec<JsonObject>;
    fn strs(&self, k: &str) -> Vec<String>;
}

impl JsonExt for JsonObject {
    fn str(&self, k: &str) -> Option<String> { self.get(k).and_then(Value::as_str).map(str::to_string) }
    /// Whole numbers, also when sent as a float (`3.0`); not NaN or beyond ±9e15.
    fn int(&self, k: &str) -> Option<i64> {
        let v = self.get(k)?;
        if let Some(n) = v.as_i64() { return Some(n); }
        let d = v.as_f64()?;
        if d.is_finite() && d.abs() < 9e15 { Some(d as i64) } else { None }
    }
    fn bool(&self, k: &str) -> Option<bool> { self.get(k).and_then(Value::as_bool) }
    fn obj(&self, k: &str) -> Option<JsonObject> { self.get(k).and_then(Value::as_object).cloned() }
    fn objs(&self, k: &str) -> Vec<JsonObject> {
        self.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_object().cloned()).collect()).unwrap_or_default()
    }
    fn strs(&self, k: &str) -> Vec<String> {
        self.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }
}

/// Parses bytes as a JSON object (nil for anything else).
pub fn object(data: &[u8]) -> Option<JsonObject> {
    match serde_json::from_slice::<Value>(data).ok()? { Value::Object(o) => Some(o), _ => None }
}

/// Parses text as a JSON object.
pub fn object_str(text: &str) -> Option<JsonObject> { object(text.as_bytes()) }

/// Compact JSON with sorted keys (serde_json's map is ordered by key), so the same value always gives the same bytes.
pub fn encode(v: &Value) -> String { serde_json::to_string(v).unwrap_or_else(|_| "{}".into()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lenient_reads() {
        let o = object_str(r#"{"a":"x","n":3.0,"big":1e300,"b":true,"o":{"k":1},"l":[{"k":1},2,{"k":2}],"s":["a",1,"b"]}"#).unwrap();
        assert_eq!(o.str("a").as_deref(), Some("x"));
        assert_eq!(o.str("n"), None);
        assert_eq!(o.int("n"), Some(3));
        assert_eq!(o.int("big"), None);
        assert_eq!(o.bool("b"), Some(true));
        assert_eq!(o.obj("o").unwrap().int("k"), Some(1));
        assert_eq!(o.objs("l").len(), 2);
        assert_eq!(o.strs("s"), vec!["a", "b"]);
        assert!(object(b"[1]").is_none());
        assert_eq!(encode(&serde_json::json!({"b":1,"a":2})), r#"{"a":2,"b":1}"#);
    }
}
