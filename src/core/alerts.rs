//! Alerts start after a condition held for a few samples (2 s apart) and end only after it has been calm for a while.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind { Cpu, Memory, Disk, Temperature, Battery }

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub cpu: Option<f64>,
    /// 1 normal, 2 high, 4 critical.
    pub pressure: u8,
    pub disk_used: Option<f64>,
    pub temperature: Option<f64>,
    pub battery_percent: Option<f64>,
    pub on_ac: bool,
}

pub fn pressure_level(memory_load: f64) -> u8 { if memory_load >= 95.0 { 4 } else if memory_load >= 88.0 { 2 } else { 1 } }

#[derive(Default)]
pub struct Alerts {
    pub cpu_threshold: f64,
    pub active: BTreeSet<Kind>,
    streaks: BTreeMap<Kind, u32>,
    clear: BTreeMap<Kind, u32>,
    last: Input,
}

impl Alerts {
    pub fn new(cpu_threshold: f64) -> Self { Alerts { cpu_threshold, ..Default::default() } }

    fn track(&mut self, k: Kind, over: bool, reset: bool, samples: u32, clear: u32, started: &mut Vec<Kind>) {
        if over {
            let s = self.streaks.entry(k).or_default();
            *s += 1;
            let hit = *s >= samples;
            self.clear.insert(k, 0);
            if hit && self.active.insert(k) { started.push(k); }
        } else {
            self.streaks.insert(k, 0);
            let c = self.clear.entry(k).or_default();
            *c = if reset { *c + 1 } else { 0 };
            if *c >= clear { self.active.remove(&k); }
        }
    }

    /// Feeds one sample; returns the alerts that started with it.
    pub fn update(&mut self, i: Input) -> Vec<Kind> {
        self.last = i;
        let mut st = vec![];
        let (t, cpu) = (self.cpu_threshold, i.cpu.unwrap_or(0.0));
        self.track(Kind::Cpu, t > 0.0 && cpu >= t, t <= 0.0 || cpu < t - 5.0, 15, if t <= 0.0 { 1 } else { 3 }, &mut st);
        self.track(Kind::Memory, i.pressure >= 2, i.pressure <= 1, 5, 15, &mut st);
        let d = i.disk_used.unwrap_or(0.0);
        self.track(Kind::Disk, d >= 90.0, d < 88.0, 1, 1, &mut st);
        let temp = i.temperature.unwrap_or(0.0);
        self.track(Kind::Temperature, temp >= 95.0, temp < 90.0, 10, 5, &mut st);
        let low = i.battery_percent.is_some_and(|p| !i.on_ac && p <= 10.0);
        let ok = i.battery_percent.is_none_or(|p| i.on_ac || p > 12.0);
        self.track(Kind::Battery, low, ok, 1, 1, &mut st);
        st
    }

    pub fn message(&self, k: Kind) -> String {
        let l = &self.last;
        match k {
            Kind::Cpu => format!("CPU has been above {:.0}% for 30 seconds ({:.0}% now)", self.cpu_threshold, l.cpu.unwrap_or(0.0)),
            Kind::Memory => if l.pressure >= 4 { "Memory is almost full".into() } else { "Memory is running low".into() },
            Kind::Disk => format!("The system disk is {:.0}% full", l.disk_used.unwrap_or(0.0)),
            Kind::Temperature => format!("A sensor is at {:.0} °C", l.temperature.unwrap_or(0.0)),
            Kind::Battery => format!("Battery at {:.0}%; connect power soon", l.battery_percent.unwrap_or(0.0)),
        }
    }
    pub fn messages(&self) -> Vec<String> { self.active.iter().map(|k| self.message(*k)).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn cpu_needs_30_seconds_and_does_not_flap() {
        let mut a = Alerts::new(90.0);
        let hi = Input { cpu: Some(95.0), pressure: 1, ..Default::default() };
        for _ in 0..14 { assert!(a.update(hi).is_empty()); }
        assert_eq!(a.update(hi), vec![Kind::Cpu]);
        assert!(a.update(hi).is_empty());
        // Just under the threshold is not calm.
        a.update(Input { cpu: Some(87.0), pressure: 1, ..Default::default() });
        assert!(a.active.contains(&Kind::Cpu));
        for _ in 0..3 { a.update(Input { cpu: Some(10.0), pressure: 1, ..Default::default() }); }
        assert!(a.active.is_empty());
    }
    #[test] fn memory_clears_after_30_seconds() {
        let mut a = Alerts::new(0.0);
        for _ in 0..5 { a.update(Input { pressure: 2, ..Default::default() }); }
        assert!(a.active.contains(&Kind::Memory));
        for _ in 0..14 { a.update(Input { pressure: 1, ..Default::default() }); }
        assert!(a.active.contains(&Kind::Memory));
        a.update(Input { pressure: 1, ..Default::default() });
        assert!(a.active.is_empty());
    }
    #[test] fn battery_only_when_unplugged() {
        let mut a = Alerts::new(0.0);
        assert!(a.update(Input { pressure: 1, battery_percent: Some(5.0), on_ac: true, ..Default::default() }).is_empty());
        assert_eq!(a.update(Input { pressure: 1, battery_percent: Some(5.0), ..Default::default() }), vec![Kind::Battery]);
        a.update(Input { pressure: 1, battery_percent: Some(11.0), ..Default::default() });
        assert!(a.active.contains(&Kind::Battery));
        a.update(Input { pressure: 1, battery_percent: Some(11.0), on_ac: true, ..Default::default() });
        assert!(a.active.is_empty());
    }
    #[test] fn disk_and_pressure_levels() {
        let mut a = Alerts::new(0.0);
        assert_eq!(a.update(Input { pressure: 1, disk_used: Some(91.0), ..Default::default() }), vec![Kind::Disk]);
        assert_eq!((pressure_level(50.0), pressure_level(90.0), pressure_level(97.0)), (1, 2, 4));
    }
}
