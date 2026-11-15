//! Brute-force guard and request limits (port of `ComputerUseLimits.swift`). Times are Unix seconds from an injected
//! clock, so the tests move time by hand.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub type Clock = Arc<dyn Fn() -> f64 + Send + Sync>;

pub fn system_clock() -> Clock { Arc::new(crate::core::pet::clock::now) }

/// Recent failure times per client, and when each locked-out client may try again.
type FailureTables = (HashMap<String, Vec<f64>>, HashMap<String, f64>);
/// Recent request times per key, and when the table was last purged.
type HitTable = (HashMap<String, Vec<f64>>, Option<f64>);

/// After `max_failures` bad keys within `window` seconds the client is locked out for `lockout` seconds; while locked
/// even a correct key is refused (fail closed). The client is the peer address; on this PC every caller is loopback,
/// so a tunnel process on the same PC shares one lockout (accepted, as on macOS).
pub struct AuthFailureLimiter {
    max_failures: usize,
    window: f64,
    lockout: f64,
    now: Clock,
    state: Mutex<FailureTables>,
}

impl AuthFailureLimiter {
    pub fn new(now: Clock) -> Self { Self::with(5, 60.0, 60.0, now) }

    pub fn with(max_failures: usize, window: f64, lockout: f64, now: Clock) -> Self {
        Self { max_failures, window, lockout, now, state: Mutex::new((HashMap::new(), HashMap::new())) }
    }

    /// Seconds until the client may try again, or None when it is not locked out.
    pub fn retry_after(&self, client: &str) -> Option<f64> {
        let mut s = self.state.lock().unwrap();
        let until = *s.1.get(client)?;
        let remaining = until - (self.now)();
        if remaining > 0.0 { return Some(remaining); }
        s.1.remove(client);
        s.0.remove(client);
        None
    }

    pub fn record_failure(&self, client: &str) {
        let mut s = self.state.lock().unwrap();
        let t = (self.now)();
        let mut recent: Vec<f64> = s.0.get(client).cloned().unwrap_or_default().into_iter().filter(|x| t - x < self.window).collect();
        recent.push(t);
        let locked = recent.len() >= self.max_failures;
        s.0.insert(client.to_string(), recent);
        if locked { s.1.insert(client.to_string(), t + self.lockout); }
        // Never let an attacker grow the table without bound.
        if s.0.len() > 1024 {
            let (failures, locked) = &mut *s;
            failures.retain(|k, _| locked.contains_key(k));
        }
    }

    pub fn record_success(&self, client: &str) { self.state.lock().unwrap().0.remove(client); }
}

/// Sliding-window request limit per key (an API key id or a client address). The table is bounded.
pub struct RequestRateLimiter {
    limit: usize,
    window: f64,
    now: Clock,
    state: Mutex<HitTable>,
}

impl RequestRateLimiter {
    pub const MAX_TRACKED_KEYS: usize = 1024;

    pub fn new(now: Clock) -> Self { Self::with(240, 60.0, now) }

    pub fn with(limit: usize, window: f64, now: Clock) -> Self { Self { limit, window, now, state: Mutex::new((HashMap::new(), None)) } }

    pub fn retry_after_seconds(&self) -> u64 { (self.window.ceil() as u64).max(1) }

    pub fn tracked_keys(&self) -> usize { self.state.lock().unwrap().0.len() }

    pub fn allow(&self, key: &str) -> bool {
        let mut s = self.state.lock().unwrap();
        let t = (self.now)();
        self.purge_if_needed(&mut s, t);
        let mut recent: Vec<f64> = s.0.get(key).cloned().unwrap_or_default().into_iter().filter(|x| t - x < self.window).collect();
        if recent.len() >= self.limit { s.0.insert(key.to_string(), recent); return false; }
        recent.push(t);
        s.0.insert(key.to_string(), recent);
        true
    }

    /// Drops expired entries once per window, and whenever the table is full. If it is still full, the least recently
    /// active keys go first, so inventing keys cannot grow the table without bound.
    fn purge_if_needed(&self, s: &mut HitTable, t: f64) {
        let due = s.1.is_some_and(|last| t - last >= self.window);
        if !due && s.0.len() < Self::MAX_TRACKED_KEYS {
            if s.1.is_none() { s.1 = Some(t); }
            return;
        }
        s.1 = Some(t);
        let window = self.window;
        s.0.retain(|_, hits| hits.iter().any(|x| t - x < window));
        if s.0.len() >= Self::MAX_TRACKED_KEYS {
            let mut oldest: Vec<(String, f64)> = s.0.iter().map(|(k, v)| (k.clone(), v.last().copied().unwrap_or(f64::MIN))).collect();
            oldest.sort_by(|a, b| a.1.total_cmp(&b.1));
            let drop = s.0.len() - Self::MAX_TRACKED_KEYS + 1;
            for (k, _) in oldest.into_iter().take(drop) { s.0.remove(&k); }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A clock the test moves by hand.
    #[derive(Clone)]
    pub struct FakeClock(pub Arc<AtomicU64>);
    impl FakeClock {
        pub fn new(t: f64) -> Self { Self(Arc::new(AtomicU64::new(t.to_bits()))) }
        pub fn advance(&self, s: f64) { let t = f64::from_bits(self.0.load(Ordering::SeqCst)) + s; self.0.store(t.to_bits(), Ordering::SeqCst); }
        pub fn clock(&self) -> Clock { let a = self.0.clone(); Arc::new(move || f64::from_bits(a.load(Ordering::SeqCst))) }
    }

    #[test]
    fn five_failures_lock_the_client_out_for_a_minute() {
        let c = FakeClock::new(1000.0);
        let l = AuthFailureLimiter::new(c.clock());
        for _ in 0..4 { l.record_failure("127.0.0.1"); }
        assert_eq!(l.retry_after("127.0.0.1"), None);
        l.record_failure("127.0.0.1");
        assert_eq!(l.retry_after("127.0.0.1"), Some(60.0));
        assert_eq!(l.retry_after("::1"), None, "per client");
        c.advance(59.0);
        assert!(l.retry_after("127.0.0.1").is_some());
        c.advance(1.5);
        assert_eq!(l.retry_after("127.0.0.1"), None);
    }

    #[test]
    fn failures_outside_the_window_and_a_success_reset_the_count() {
        let c = FakeClock::new(0.0);
        let l = AuthFailureLimiter::new(c.clock());
        for _ in 0..4 { l.record_failure("a"); }
        c.advance(61.0);
        l.record_failure("a");
        assert_eq!(l.retry_after("a"), None);
        for _ in 0..3 { l.record_failure("a"); }
        l.record_success("a");
        l.record_failure("a");
        assert_eq!(l.retry_after("a"), None);
    }

    #[test]
    fn rate_limit_is_a_sliding_window() {
        let c = FakeClock::new(0.0);
        let r = RequestRateLimiter::with(3, 60.0, c.clock());
        assert!(r.allow("k") && r.allow("k") && r.allow("k"));
        assert!(!r.allow("k"));
        assert!(r.allow("other"));
        c.advance(60.0);
        assert!(r.allow("k"));
        assert_eq!(r.retry_after_seconds(), 60);
    }

    #[test]
    fn the_table_stays_bounded() {
        let c = FakeClock::new(0.0);
        let r = RequestRateLimiter::new(c.clock());
        for i in 0..3000 { c.advance(0.001); assert!(r.allow(&format!("k{i}"))); }
        assert!(r.tracked_keys() <= RequestRateLimiter::MAX_TRACKED_KEYS);
    }
}
