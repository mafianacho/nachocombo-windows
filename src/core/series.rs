//! History for the charts: one sample every 2 s for an hour, then coarser rings up to 24 h, in memory only.
pub const RANGES: [u32; 4] = [600, 3600, 10800, 86400];
pub fn range_name(r: u32) -> &'static str { match r { 600 => "10 min", 3600 => "1 h", 10800 => "3 h", _ => "24 h" } }

#[derive(Clone, Debug)]
struct Ring { step: u32, cap: usize, data: Vec<f64>, head: usize, acc: f64, n: u32 }

impl Ring {
    fn new(step: u32, cap: usize) -> Self { Ring { step, cap, data: Vec::new(), head: 0, acc: 0.0, n: 0 } }
    fn push(&mut self, v: f64) {
        self.acc += v;
        self.n += 1;
        if self.n < self.step { return; }
        let avg = self.acc / self.n as f64;
        (self.acc, self.n) = (0.0, 0);
        if self.data.len() < self.cap { self.data.push(avg); } else { self.data[self.head] = avg; self.head = (self.head + 1) % self.cap; }
    }
    fn last(&self, count: usize) -> Vec<f64> {
        let len = self.data.len();
        let take = count.min(len);
        (len - take..len).map(|i| self.data[(self.head + i) % len]).collect()
    }
}

#[derive(Clone, Debug)]
pub struct Series { fine: Ring, coarse: Ring }

impl Default for Series { fn default() -> Self { Series::new() } }

impl Series {
    /// Fine: 2 s × 1800 = 1 h. Coarse: 60 s × 1440 = 24 h.
    pub fn new() -> Self { Series { fine: Ring::new(1, 1800), coarse: Ring::new(30, 1440) } }
    pub fn push(&mut self, v: f64) { self.fine.push(v); self.coarse.push(v); }
    pub fn values(&self, range: u32) -> Vec<f64> {
        if range <= 3600 { self.fine.last((range / 2) as usize) } else { self.coarse.last((range / 60) as usize) }
    }
    pub fn latest(&self) -> Option<f64> { self.fine.last(1).first().copied() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ranges_and_wrap() {
        let mut s = Series::new();
        for i in 0..2000 { s.push(i as f64); }
        assert_eq!(s.values(600).len(), 300);
        assert_eq!(*s.values(600).last().unwrap(), 1999.0);
        assert_eq!(s.values(3600).len(), 1800);
        assert_eq!(s.values(3600)[0], 200.0);
        assert_eq!(s.values(86400).len(), 66);
        assert_eq!(s.values(86400)[0], 14.5);
        assert_eq!(s.latest(), Some(1999.0));
        assert_eq!(Series::new().values(600), Vec::<f64>::new());
    }
}
