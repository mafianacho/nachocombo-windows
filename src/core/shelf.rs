//! Shelf items, undo and shake detection.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind { File, Image, Link(String), Text(String) }

#[derive(Clone, Debug, PartialEq)]
pub struct Item { pub path: PathBuf, pub kind: Kind, pub title: String, pub own_copy: bool }

#[derive(Default)]
pub struct Shelf { pub items: Vec<Item>, removed: Vec<(usize, Vec<Item>)>, pub selected: Option<usize> }

impl Shelf {
    /// A path already on the shelf is not added twice.
    pub fn add(&mut self, item: Item) -> bool {
        if !item.own_copy && self.items.iter().any(|i| i.path == item.path) { return false; }
        self.items.push(item);
        true
    }
    pub fn remove(&mut self, i: usize) {
        if i >= self.items.len() { return; }
        let it = self.items.remove(i);
        self.removed.push((i, vec![it]));
        self.selected = if self.items.is_empty() { None } else { Some(i.min(self.items.len() - 1)) };
    }
    pub fn clear(&mut self) {
        if self.items.is_empty() { return; }
        self.removed.push((0, std::mem::take(&mut self.items)));
        self.selected = None;
    }
    pub fn undo(&mut self) -> bool {
        let Some((at, items)) = self.removed.pop() else { return false };
        let at = at.min(self.items.len());
        self.items.splice(at..at, items);
        true
    }
    /// Own copies, including removed ones undo could still bring back, for deleting on stop.
    pub fn all_own_copies(&self) -> Vec<PathBuf> {
        self.items.iter().chain(self.removed.iter().flat_map(|r| r.1.iter())).filter(|i| i.own_copy).map(|i| i.path.clone()).collect()
    }
}

/// Counts direction reversals of the pointer: a shake is 3 of them, each at least `min_travel` long, within `window` ms.
pub struct Shake { last_x: i32, dir: i32, travel: i32, flips: Vec<u32>, pub min_travel: i32, pub window: u32 }

impl Shake {
    pub fn new(min_travel: i32) -> Self { Shake { last_x: 0, dir: 0, travel: 0, flips: vec![], min_travel, window: 700 } }
    pub fn reset(&mut self) { self.dir = 0; self.travel = 0; self.flips.clear(); }
    pub fn feed(&mut self, x: i32, t: u32) -> bool {
        let dx = x - self.last_x;
        self.last_x = x;
        if dx == 0 { return false; }
        let d = dx.signum();
        if d == self.dir { self.travel += dx.abs(); return false; }
        if self.travel >= self.min_travel { self.flips.push(t); }
        self.dir = d;
        self.travel = dx.abs();
        let w = self.window;
        self.flips.retain(|f| t.wrapping_sub(*f) <= w);
        if self.flips.len() >= 3 { self.flips.clear(); return true; }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(p: &str, own: bool) -> Item { Item { path: p.into(), kind: Kind::File, title: p.into(), own_copy: own } }
    #[test] fn add_remove_undo() {
        let mut s = Shelf::default();
        assert!(s.add(item("a", false)));
        assert!(!s.add(item("a", false)));
        s.add(item("b", true));
        s.add(item("c", false));
        s.remove(1);
        assert_eq!(s.selected, Some(1));
        s.clear();
        assert!(s.items.is_empty());
        assert_eq!(s.all_own_copies(), vec![PathBuf::from("b")]);
        assert!(s.undo());
        assert_eq!(s.items.len(), 2);
        assert!(s.undo());
        assert_eq!(s.items[1].path, PathBuf::from("b"));
        assert!(!s.undo());
    }
    #[test] fn shake_needs_three_quick_long_reversals() {
        let mut k = Shake::new(40);
        let mut t = 0;
        let mut go = |k: &mut Shake, xs: &[i32], step: u32| { let mut hit = false; for x in xs { t += step; hit |= k.feed(*x, t); } hit };
        assert!(!go(&mut k, &[0, 5, 0, 5, 0, 5, 0, 5], 20)); // too small
        assert!(go(&mut k, &[50, 100, 50, 0, 50, 100, 50], 20));
        k.reset();
        assert!(!go(&mut k, &[100, 0, 100, 0, 100], 1000)); // too slow
    }
}
