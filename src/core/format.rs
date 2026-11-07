pub fn bytes(v: f64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut x, mut i) = (v.max(0.0), 0);
    while x >= 1024.0 && i < 4 { x /= 1024.0; i += 1; }
    if i == 0 || x >= 100.0 { format!("{:.0} {}", x, U[i]) } else { format!("{:.1} {}", x, U[i]) }
}
pub fn rate(v: f64) -> String { format!("{}/s", bytes(v)) }
pub fn percent(v: f64) -> String { format!("{:.0}%", v) }
pub fn uptime(s: u64) -> String {
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
}
pub fn minutes(m: u32) -> String { if m >= 60 { format!("{}:{:02}", m / 60, m % 60) } else { format!("{m} min") } }
/// What fits in a small tray icon: "87", "1.2M".
pub fn compact_rate(v: f64) -> String {
    let (x, u) = if v >= 1e9 { (v / 1e9, "G") } else if v >= 1e6 { (v / 1e6, "M") } else if v >= 1e3 { (v / 1e3, "K") } else { (v, "") };
    if x >= 10.0 || u.is_empty() { format!("{x:.0}{u}") } else { format!("{x:.1}{u}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn formats() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1536.0), "1.5 KB");
        assert_eq!(bytes(150.0 * 1024.0), "150 KB");
        assert_eq!(rate(1048576.0), "1.0 MB/s");
        assert_eq!(uptime(90061), "1d 1h");
        assert_eq!(uptime(3700), "1h 1m");
        assert_eq!(minutes(95), "1:35");
        assert_eq!(compact_rate(1_234_000.0), "1.2M");
        assert_eq!(compact_rate(87.0), "87");
    }
}
