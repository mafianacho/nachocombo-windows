//! Exponential backoff for `/ws/chat` and `/ws/connector` reconnects. Jitter is passed in so tests are deterministic.
//! Port of `PetReconnect.swift`.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReconnectPolicy {
    /// Seconds.
    pub base_delay: f64,
    pub max_delay: f64,
    pub max_attempts: u32,
}

impl Default for ReconnectPolicy {
    fn default() -> Self { Self { base_delay: 1.0, max_delay: 30.0, max_attempts: 8 } }
}

impl ReconnectPolicy {
    /// `attempt` starts at 1. `jitter` in 0...1 scales the delay into [50%, 100%]. Seconds.
    pub fn delay(&self, attempt: u32, jitter: f64) -> f64 {
        let a = attempt.clamp(1, 30);
        let raw = self.max_delay.min(self.base_delay * 2f64.powi(a as i32 - 1));
        let j = if jitter.is_nan() { 1.0 } else { jitter.clamp(0.0, 1.0) };
        raw * (0.5 + 0.5 * j)
    }

    pub fn should_retry(&self, attempt: u32, failure: &TransportFailure) -> bool {
        if matches!(failure.http_status, Some(401) | Some(403)) { return false; } // auth problem: retrying cannot help
        attempt <= self.max_attempts
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportFailure {
    pub http_status: Option<u16>,
    pub message: String,
}

impl TransportFailure {
    pub fn new(http_status: Option<u16>, message: impl Into<String>) -> Self { Self { http_status, message: message.into() } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_caps_and_jitters() {
        let p = ReconnectPolicy::default();
        assert_eq!(p.delay(1, 1.0), 1.0);
        assert_eq!(p.delay(2, 1.0), 2.0);
        assert_eq!(p.delay(3, 1.0), 4.0);
        assert_eq!(p.delay(10, 1.0), 30.0);
        assert_eq!(p.delay(100, 1.0), 30.0);
        assert_eq!(p.delay(0, 1.0), 1.0);
        assert_eq!(p.delay(3, 0.0), 2.0);
        assert_eq!(p.delay(3, 5.0), 4.0);
    }

    #[test]
    fn auth_failures_are_not_retried() {
        let p = ReconnectPolicy::default();
        assert!(!p.should_retry(1, &TransportFailure::new(Some(401), "x")));
        assert!(!p.should_retry(1, &TransportFailure::new(Some(403), "x")));
        assert!(p.should_retry(8, &TransportFailure::new(None, "x")));
        assert!(!p.should_retry(9, &TransportFailure::new(Some(500), "x")));
    }
}
