//! The Widget Board (the macOS "bento"): one window with a tile per gadget. Ported from `BentoLayout`, `BentoContent` and
//! `BentoBuilder`. The layout (order, size, hidden), the packing into a grid and what each tile says are all plain data,
//! so they are tested without Windows. The board never switches a gadget on or off by being edited.
//!
//! Text is English here; the window passes it through `i18n::t` where it draws it.
use super::settings::ModuleId;
use serde::{Deserialize, Deserializer, Serialize};

// ---------- layout ----------

/// How big a tile is: one cell, two cells side by side, or two by two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BentoSize { Small, Wide, Large }

impl BentoSize {
    pub const ALL: [BentoSize; 3] = [BentoSize::Small, BentoSize::Wide, BentoSize::Large];
    pub fn columns(self) -> usize { if self == BentoSize::Small { 1 } else { 2 } }
    pub fn rows(self) -> usize { if self == BentoSize::Large { 2 } else { 1 } }
    pub fn name(self) -> &'static str { match self { BentoSize::Small => "Small", BentoSize::Wide => "Wide", BentoSize::Large => "Large" } }
}

/// A value from a newer version (or a broken one) becomes the default instead of losing the whole file.
fn lenient<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

impl Default for BentoSize { fn default() -> Self { BentoSize::Small } }

/// One tile, in board order. `id` is a gadget's key (a string, so a file from a newer version with more gadgets decodes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BentoItem {
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub size: BentoSize,
    #[serde(default, deserialize_with = "lenient")]
    pub hidden: bool,
}

impl BentoItem {
    pub fn new(id: ModuleId, size: BentoSize) -> BentoItem { BentoItem { id: id.key().into(), size, hidden: false } }
    pub fn module(&self) -> Option<ModuleId> { board_module(&self.id) }
}

/// The gadgets that get a tile: all of them except the board itself.
pub fn board_modules() -> Vec<ModuleId> { ModuleId::ALL.into_iter().filter(|m| *m != ModuleId::WidgetBoard).collect() }
fn board_module(key: &str) -> Option<ModuleId> { ModuleId::from_key(key).filter(|m| *m != ModuleId::WidgetBoard) }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BentoLayout {
    pub version: u32,
    pub items: Vec<BentoItem>,
}

impl Default for BentoLayout { fn default() -> Self { BentoLayout { version: BentoLayout::VERSION, items: vec![] } } }

impl BentoLayout {
    pub const VERSION: u32 = 1;

    /// The board order of the design.
    pub const DESIGN_ORDER: [ModuleId; 13] = [ModuleId::SystemMonitor, ModuleId::WindowSnap, ModuleId::KeepAwake, ModuleId::Clipboard,
        ModuleId::CaptureFloat, ModuleId::FocusDim, ModuleId::DeviceControl, ModuleId::CalendarClock, ModuleId::DiskMap,
        ModuleId::FileShelf, ModuleId::TrayOrganizer, ModuleId::AppSwitch, ModuleId::MouseTune];

    /// The size each gadget starts with (what its tile was designed for).
    pub fn default_size(id: ModuleId) -> BentoSize {
        match id {
            ModuleId::SystemMonitor | ModuleId::Clipboard | ModuleId::DeviceControl | ModuleId::FileShelf => BentoSize::Wide,
            _ => BentoSize::Small,
        }
    }

    /// The board for someone who has none yet: the gadgets that are on first, then the rest, each group in the design's
    /// order. Nothing is hidden.
    pub fn defaults(is_on: impl Fn(ModuleId) -> bool) -> BentoLayout {
        let order: Vec<ModuleId> = Self::DESIGN_ORDER.into_iter().chain(board_modules().into_iter().filter(|m| !Self::DESIGN_ORDER.contains(m))).collect();
        let on = order.iter().copied().filter(|m| is_on(*m));
        let off = order.iter().copied().filter(|m| !is_on(*m));
        BentoLayout { version: Self::VERSION, items: on.chain(off).map(|m| BentoItem::new(m, Self::default_size(m))).collect() }
    }

    /// Every gadget exactly once: unknown ids and duplicates go, gadgets missing from the file (a version that added one)
    /// are appended with their default size.
    pub fn normalized(&self) -> BentoLayout {
        let mut seen = std::collections::BTreeSet::new();
        let mut items = vec![];
        for i in &self.items {
            if i.module().is_some() && seen.insert(i.id.clone()) { items.push(i.clone()); }
        }
        for m in Self::DESIGN_ORDER.into_iter().chain(board_modules()) {
            if seen.insert(m.key().to_string()) { items.push(BentoItem::new(m, Self::default_size(m))); }
        }
        BentoLayout { version: Self::VERSION, items }
    }

    pub fn visible(&self) -> Vec<&BentoItem> { self.items.iter().filter(|i| !i.hidden).collect() }
    pub fn hidden_modules(&self) -> Vec<ModuleId> { self.items.iter().filter(|i| i.hidden).filter_map(|i| i.module()).collect() }
    pub fn item(&self, id: ModuleId) -> Option<&BentoItem> { self.items.iter().find(|i| i.id == id.key()) }
    fn index(&self, id: ModuleId) -> Option<usize> { self.items.iter().position(|i| i.id == id.key()) }

    pub fn set_size(&mut self, id: ModuleId, size: BentoSize) {
        if let Some(i) = self.index(id) { self.items[i].size = size; }
    }

    pub fn set_hidden(&mut self, id: ModuleId, hidden: bool) {
        let Some(i) = self.index(id) else { return };
        self.items[i].hidden = hidden;
        // A tile that comes back goes to the end of the board, where "+ Add Widget" was.
        if !hidden { let item = self.items.remove(i); self.items.push(item); }
    }

    /// Moves `id` so it sits just before `target` in board order (None: to the end).
    pub fn move_before(&mut self, id: ModuleId, target: Option<ModuleId>) {
        if Some(id) == target { return }
        let Some(from) = self.index(id) else { return };
        let item = self.items.remove(from);
        match target.and_then(|t| self.index(t)) { Some(to) => self.items.insert(to, item), None => self.items.push(item) }
    }

    /// Drag and drop in edit mode: dropped on a tile further along it lands after it, on one before it lands before it
    /// (the dropped tile takes the target's place either way).
    pub fn drop_onto(&mut self, id: ModuleId, target: ModuleId) {
        if id == target { return }
        let (Some(from), Some(to)) = (self.index(id), self.index(target)) else { return };
        let item = self.items.remove(from);
        let at = self.items.iter().position(|i| i.id == target.key()).unwrap_or(to);
        self.items.insert(if from < to { at + 1 } else { at }, item);
    }

    /// Keyboard move in edit mode: one place earlier (-1) or later (+1) among the visible tiles.
    pub fn step(&mut self, id: ModuleId, delta: i32) {
        let shown: Vec<ModuleId> = self.visible().iter().filter_map(|i| i.module()).collect();
        let Some(i) = shown.iter().position(|m| *m == id) else { return };
        let j = i as i32 + delta;
        if j < 0 || j as usize >= shown.len() { return }
        let j = j as usize;
        if delta < 0 { self.move_before(id, Some(shown[j])) } else { self.move_before(id, shown.get(j + 1).copied()) }
    }
}

/// Where a tile lands on a grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement { pub id: String, pub column: usize, pub row: usize, pub columns: usize, pub rows: usize }

/// Fills the grid row by row in board order: each tile takes the first free place it fits in (later small tiles fill
/// gaps that bigger ones left). A tile wider than the grid is narrowed to it. `extra` ids are one cell each, after all.
pub fn pack(items: &[BentoItem], columns: usize, extra: &[&str]) -> (Vec<Placement>, usize) {
    let cols = columns.max(1);
    let mut taken: Vec<Vec<bool>> = vec![];
    let free = |taken: &Vec<Vec<bool>>, r: usize, c: usize, w: usize, h: usize| {
        c + w <= cols && (r..r + h).all(|y| taken.get(y).is_none_or(|row| (c..c + w).all(|x| !row[x])))
    };
    let all: Vec<(String, usize, usize)> = items.iter().map(|i| (i.id.clone(), i.size.columns().min(cols), i.size.rows()))
        .chain(extra.iter().map(|e| (e.to_string(), 1, 1))).collect();
    let mut out = vec![];
    for (id, w, h) in all {
        let mut r = 0;
        'search: loop {
            for c in 0..cols {
                if free(&taken, r, c, w, h) {
                    while taken.len() < r + h { taken.push(vec![false; cols]); }
                    for row in taken.iter_mut().skip(r).take(h) { for cell in row.iter_mut().skip(c).take(w) { *cell = true; } }
                    out.push(Placement { id, column: c, row: r, columns: w, rows: h });
                    break 'search;
                }
            }
            r += 1;
        }
    }
    (out, taken.len())
}

/// Columns that fit `width` when a cell must be at least `min_cell` wide: 6 on a wide window, 4, then 2.
pub fn columns_for_width(width: f64, min_cell: f64, gap: f64) -> usize {
    [6usize, 4].into_iter().find(|n| *n as f64 * min_cell + (*n as f64 - 1.0) * gap <= width).unwrap_or(2)
}

/// The name in a tile's header: the gadget's name, shortened where the cell is narrow ("Calendar", "Tray").
pub fn tile_name(id: ModuleId, size: BentoSize) -> &'static str {
    if size != BentoSize::Small { return id.name(); }
    match id {
        ModuleId::CalendarClock => "Calendar", ModuleId::CaptureFloat => "Capture", ModuleId::Clipboard => "Clipboard", ModuleId::DeviceControl => "Devices",
        ModuleId::TrayOrganizer => "Tray", ModuleId::SystemMonitor => "Monitor", ModuleId::FileShelf => "Shelf", _ => id.name(),
    }
}

/// What each size of a tile shows, for the edit panel's three previews.
pub fn size_text(id: ModuleId, size: BentoSize) -> &'static str {
    use BentoSize::*;
    use ModuleId::*;
    match (id, size) {
        (SystemMonitor, Small) => "Two rings: CPU and memory",
        (SystemMonitor, _) => "CPU, memory and disk rings and the network speed",
        (Clipboard, Small) => "The latest copy",
        (Clipboard, Wide) => "The latest 3 copies",
        (Clipboard, Large) => "The latest 8 copies",
        (FileShelf, Small) => "One shelf item and a drop spot",
        (FileShelf, _) => "Shelf items and a drop spot",
        (DeviceControl, Small) => "Volume",
        (DeviceControl, _) => "Volume, display brightness and the output device",
        (CalendarClock, Small) => "Weekday and date",
        (CalendarClock, _) => "Weekday, date and the world clocks",
        (DiskMap, Small) => "A usage bar",
        (DiskMap, _) => "A usage bar and the share of each folder (after a scan)",
        (KeepAwake, _) => "A ring with the time left",
        (WindowSnap, _) => "Snap shapes and a shortcut",
        (CaptureFloat, _) => "How many floats are open and the shortcut",
        (FocusDim, _) => "How strongly it dims",
        (TrayOrganizer, _) => "Whether the icons are folded",
        (AppSwitch, _) => "The switch shortcut",
        (MouseTune, _) => "Scroll speed and direction",
        (WidgetBoard, _) => "",
    }
}

// ---------- content ----------

#[derive(Clone, Debug, PartialEq)]
pub struct BentoRing { pub label: String, pub fraction: Option<f64>, pub tone: Tone }
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone { Green, Blue, Orange }
impl BentoRing {
    /// "37%", or "—" when unknown.
    pub fn text(&self) -> String { self.fraction.map_or("—".into(), |f| format!("{}%", (f * 100.0).round() as i64)) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BentoLine { pub label: String, pub value: String }
impl BentoLine { pub fn new(l: impl Into<String>, v: impl Into<String>) -> Self { BentoLine { label: l.into(), value: v.into() } } }

/// The filled part of a snap picture in unit coordinates (x, y from the top left, width, height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapShape { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind { Text, Link, Image, File }

#[derive(Clone, Debug, PartialEq)]
pub struct BentoRow { pub kind: RowKind, pub text: String, pub detail: Option<String> }

#[derive(Clone, Debug, PartialEq)]
pub struct BentoSlider { pub label: String, pub fraction: Option<f64>, pub value: String }

#[derive(Clone, Debug, PartialEq)]
pub struct BentoSegment { pub label: String, pub fraction: f64 }

/// What one tile's body shows, built from what the gadget really has right now (never sample values). A gadget that
/// cannot give the numbers its tile was drawn for gets `Basic`: one line saying what it does.
#[derive(Clone, Debug, PartialEq)]
pub enum BentoBody {
    Basic { line: String },
    Gauges { rings: Vec<BentoRing>, side: Vec<BentoLine> },
    Layouts { shapes: Vec<SnapShape>, caption: Option<String> },
    /// A countdown: fraction left (None: no end), big text, caption.
    Timer { fraction: Option<f64>, big: String, caption: String },
    /// Recent items, newest first; `empty` says what to show when there are none.
    List { rows: Vec<BentoRow>, empty: String },
    Stack { count: usize, caption: String },
    Level { fraction: f64, low: String, high: String, caption: String },
    Sliders { sliders: Vec<BentoSlider>, rows: Vec<BentoLine> },
    Calendar { weekday: String, day: String, lines: Vec<BentoLine>, note: Option<String> },
    Usage { big: String, caption: String, segments: Vec<BentoSegment> },
    Slots { rows: Vec<BentoRow>, drop: String },
    Strip { caption: String },
}

/// The dot of a tile's header. The words (`text`) always go with it: in the tooltip and as the state line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BentoDot { Off, On, Error }
impl BentoDot {
    pub fn text(self) -> &'static str { match self { BentoDot::Off => "Off", BentoDot::On => "On", BentoDot::Error => "On · error" } }
}

/// Number and time text of the tiles.
pub mod fmt {
    /// Decimal units like Explorer: 1_200_000 → "1.2MB/s", 84_000 → "84KB/s", 0 → "0B/s".
    pub fn rate(bytes_per_second: f64) -> String { size(bytes_per_second) + "/s" }

    /// Bytes in 1000 steps, one decimal below 10 ("1.2MB", "84KB", "612GB", "1TB").
    pub fn size(bytes: f64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB", "PB"];
        let (mut v, mut i) = (bytes.max(0.0), 0);
        while v >= 999.5 && i < units.len() - 1 { v /= 1000.0; i += 1; }
        if i == 0 { return format!("{}B", v as i64); }
        if v < 10.0 {
            let r = (v * 10.0).round() / 10.0;
            return if r == r.round() { format!("{}{}", r as i64, units[i]) } else { format!("{r:.1}{}", units[i]) };
        }
        format!("{}{}", v.round() as i64, units[i])
    }

    /// Seconds left → "1:14" (h:mm) from an hour on, else "14 min", "45 sec".
    pub fn remaining(seconds: f64) -> String {
        let s = seconds.max(0.0).ceil() as i64;
        if s >= 3600 { format!("{}:{:02}", s / 3600, (s % 3600) / 60) } else if s >= 60 { format!("{} min", (s + 59) / 60) } else { format!("{s} sec") }
    }

    /// A duration in words: "2 h", "1 h 30 min", "45 min".
    pub fn duration(seconds: f64) -> String {
        let m = (seconds / 60.0).round().max(0.0) as i64;
        if m >= 60 { if m % 60 == 0 { format!("{} h", m / 60) } else { format!("{} h {} min", m / 60, m % 60) } } else { format!("{m} min") }
    }

    /// How long ago: "just now", "3 min", "2 h", "yesterday", "5 d".
    pub fn age(seconds: f64) -> String {
        let s = seconds.max(0.0);
        if s < 60.0 { "just now".into() } else if s < 3600.0 { format!("{} min", (s / 60.0) as i64) } else if s < 86400.0 { format!("{} h", (s / 3600.0) as i64) }
        else if s < 2.0 * 86400.0 { "yesterday".into() } else { format!("{} d", (s / 86400.0) as i64) }
    }

    /// 0…1 → "64".
    pub fn percent_number(fraction: f64) -> String { format!("{}", (fraction.clamp(0.0, 1.0) * 100.0).round() as i64) }

    /// The weekday's name, 0 = Sunday.
    pub fn weekday(sunday_first: usize) -> &'static str {
        ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"][sunday_first.min(6)]
    }

    /// The sentences of a caption joined with " · ": each is translated on its own.
    pub fn sentences(line: &str) -> Vec<&str> { line.split(" · ").collect() }

    /// One line for a list row: newlines and runs of spaces folded, at most `limit` characters.
    pub fn one_line(text: &str, limit: usize) -> String {
        let folded = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if folded.chars().count() > limit { folded.chars().take(limit).collect::<String>() + "…" } else { folded }
    }
}

fn clamp(v: f64) -> f64 { v.clamp(0.0, 1.0) }

/// The basic tile's line for a gadget that is off or has nothing to show.
pub fn off_line(id: ModuleId) -> &'static str {
    match id {
        ModuleId::SystemMonitor => "Turn on to see CPU, memory, disk and network",
        ModuleId::WindowSnap => "Turn on to snap windows to halves, quarters and thirds",
        ModuleId::KeepAwake => "Turn on to keep the PC from sleeping",
        ModuleId::Clipboard => "Turn on to remember what you copy",
        ModuleId::CaptureFloat => "Turn on to float screenshots above your windows",
        ModuleId::FocusDim => "Turn on to dim the other windows",
        ModuleId::DeviceControl => "Turn on for sound devices and volume",
        ModuleId::CalendarClock => "Turn on for the date, a calendar and world clocks",
        ModuleId::DiskMap => "See what fills the disk",
        ModuleId::FileShelf => "Turn on to park files for a moment",
        ModuleId::TrayOrganizer => "Turn on to fold away tray icons",
        ModuleId::AppSwitch => "Switch between windows with one shortcut",
        ModuleId::MouseTune => "Set the wheel direction and pointer of a mouse",
        ModuleId::WidgetBoard => "",
    }
}

/// What a gadget's tile shows from the values handed in. The app reads the gadgets; these functions only shape plain
/// values, so the rules (what shows when, what falls back to the basic tile) are tested here.
pub mod build {
    use super::*;

    fn basic(id: ModuleId) -> BentoBody { BentoBody::Basic { line: off_line(id).into() } }

    /// CPU and memory in percent (0…100), the system disk as a fraction, the network in bytes per second. None = not
    /// measured yet.
    pub fn monitor(running: bool, cpu: Option<f64>, memory: Option<f64>, disk: Option<f64>, down: Option<f64>, up: Option<f64>) -> BentoBody {
        if !running { return basic(ModuleId::SystemMonitor); }
        let ring = |l: &str, f: Option<f64>, tone| BentoRing { label: l.into(), fraction: f.map(clamp), tone };
        BentoBody::Gauges {
            rings: vec![ring("CPU", cpu.map(|c| c / 100.0), Tone::Green), ring("Memory", memory.map(|c| c / 100.0), Tone::Blue), ring("Disk", disk, Tone::Orange)],
            side: vec![BentoLine::new("Network", ""), BentoLine::new("↓", down.map_or("—".into(), fmt::rate)), BentoLine::new("↑", up.map_or("—".into(), fmt::rate))],
        }
    }

    /// The four snap positions the tile draws, and the shortcut of "left half" with how many more snap shortcuts there are.
    pub fn window_snap(running: bool, left_label: Option<&str>, other_shortcuts: usize) -> BentoBody {
        let s = |x, y, w, h| SnapShape { x, y, w, h };
        let shapes = vec![s(0.0, 0.0, 0.5, 1.0), s(0.5, 0.0, 0.5, 1.0), s(0.0, 0.0, 0.5, 0.5), s(0.0, 0.0, 1.0, 1.0)];
        let caption = if !running { Some(off_line(ModuleId::WindowSnap).to_string()) } else {
            match (left_label, other_shortcuts) {
                (Some(l), n) if n > 0 => Some(format!("{l} and {n} more")),
                (Some(l), _) => Some(l.to_string()),
                (None, n) if n > 0 => Some(format!("Shortcuts: {n}")),
                _ => None,
            }
        };
        BentoBody::Layouts { shapes, caption }
    }

    /// A timed hold (`until`, `held_since` as seconds since 1970), an open-ended one (`manual`), awake because of a rule
    /// (`reason`), or asleep allowed.
    pub fn keep_awake(running: bool, holds: bool, manual: bool, until: Option<f64>, held_since: Option<f64>, reason: Option<&str>, preset_minutes: i32, now: f64) -> BentoBody {
        if !running { return basic(ModuleId::KeepAwake); }
        if holds {
            if let Some(end) = until {
                let left = end - now;
                let (mut fraction, mut caption) = (None, "left".to_string());
                if let Some(start) = held_since.filter(|s| end > *s) {
                    fraction = Some(clamp(left / (end - start)));
                    caption = format!("left · {}", fmt::duration(end - start)); // two sentences: `fmt::sentences` splits them to be translated apart
                }
                return BentoBody::Timer { fraction, big: fmt::remaining(left), caption };
            }
            return BentoBody::Timer { fraction: None, big: "On".into(), caption: if manual { "Until you turn it off".into() } else { reason.unwrap_or("Staying awake").into() } };
        }
        let preset = if preset_minutes > 0 { format!("Click to stay awake for {}", fmt::duration(preset_minutes as f64 * 60.0)) } else { "Click to stay awake until you turn it off".into() };
        BentoBody::Timer { fraction: Some(0.0), big: "Off".into(), caption: preset }
    }

    /// The latest items, newest first, as the history lists them (its title line, never more). Images show only their
    /// kind. At most `limit` (the large tile shows 8). `items` are (kind, title, seconds since 1970 when known: the history keeps no copy times, so the app passes None and no age is shown).
    pub fn clipboard(running: bool, items: &[(RowKind, String, Option<f64>)], now: f64, limit: usize) -> BentoBody {
        if !running { return basic(ModuleId::Clipboard); }
        let rows = items.iter().take(limit).map(|(kind, title, date)| BentoRow {
            kind: *kind,
            text: if *kind == RowKind::Image { "Image".into() } else { fmt::one_line(title, 80) },
            detail: date.map(|d| fmt::age(now - d)),
        }).collect();
        BentoBody::List { rows, empty: "Nothing copied yet".into() }
    }

    pub fn capture(running: bool, floats: usize, shortcut: Option<&str>) -> BentoBody {
        if !running { return basic(ModuleId::CaptureFloat); }
        let mut caption = if floats == 0 { "No floats open".to_string() } else { format!("Floats open: {floats}") };
        if let Some(s) = shortcut { caption += &format!(" · {s}"); }
        BentoBody::Stack { count: floats, caption }
    }

    /// Strength 0.1…0.9 from the settings (also when off).
    pub fn focus_dim(running: bool, intensity: f64, paused: bool) -> BentoBody {
        let f = clamp((intensity - 0.1) / (0.9 - 0.1));
        let caption = if !running { off_line(ModuleId::FocusDim).to_string() } else if paused { "Paused".into() } else { format!("Dims the other windows by {}%", fmt::percent_number(intensity)) };
        BentoBody::Level { fraction: f, low: "Light".into(), high: "Strong".into(), caption }
    }

    /// Output volume 0…1 (None: no device), external display brightness 0…100 (None: no display takes it), the output's name.
    pub fn devices(running: bool, volume: Option<f64>, brightness: Option<i32>, output: Option<&str>) -> BentoBody {
        if !running { return basic(ModuleId::DeviceControl); }
        let mut sliders = vec![BentoSlider { label: "Volume".into(), fraction: volume.map(clamp), value: volume.map_or("—".into(), fmt::percent_number) }];
        if let Some(b) = brightness { sliders.push(BentoSlider { label: "Brightness".into(), fraction: Some(clamp(b as f64 / 100.0)), value: b.to_string() }); }
        BentoBody::Sliders { sliders, rows: vec![BentoLine::new(output.unwrap_or("No output device"), "Output")] }
    }

    /// Today from the clock (`weekday` 0 = Sunday), and the first world clocks as (city, time) lines. Windows lets no
    /// app read the user's calendar events, so the tile never claims to show an event.
    pub fn calendar(running: bool, weekday: usize, day: u32, worlds: &[(String, String)]) -> BentoBody {
        let lines: Vec<BentoLine> = if running { worlds.iter().take(2).map(|(c, t)| BentoLine::new(c.clone(), t.clone())).collect() } else { vec![] };
        let note = if !running { Some(off_line(ModuleId::CalendarClock).to_string()) } else if lines.is_empty() { Some("Click for the calendar".to_string()) } else { None };
        BentoBody::Calendar { weekday: fmt::weekday(weekday).into(), day: day.to_string(), lines, note }
    }

    /// The system disk (works while the gadget is off) and, when a scan is there, its top-level folders.
    pub fn disk(used: Option<u64>, total: Option<u64>, folders: &[(String, u64)]) -> BentoBody {
        let (Some(u), Some(t)) = (used, total) else { return basic(ModuleId::DiskMap) };
        if t == 0 { return basic(ModuleId::DiskMap); }
        let (u, t) = (u as f64, t as f64);
        let mut segments = vec![];
        if !folders.is_empty() {
            for (name, size) in folders.iter().take(3).filter(|f| f.1 > 0) { segments.push(BentoSegment { label: name.clone(), fraction: *size as f64 / t }); }
            let shown: f64 = segments.iter().map(|s| s.fraction).sum();
            let rest = u / t - shown;
            if rest > 0.005 { segments.push(BentoSegment { label: "Other".into(), fraction: rest }); }
        } else {
            segments.push(BentoSegment { label: "Used".into(), fraction: u / t });
        }
        BentoBody::Usage { big: fmt::size(u), caption: format!("/ {} used", fmt::size(t)), segments }
    }

    pub fn shelf(running: bool, items: &[(RowKind, String)]) -> BentoBody {
        if !running { return basic(ModuleId::FileShelf); }
        BentoBody::Slots { rows: items.iter().take(8).map(|(k, t)| BentoRow { kind: *k, text: fmt::one_line(t, 40), detail: None }).collect(), drop: "Drop files here".into() }
    }

    /// Windows lists no tray icons to count, so the tile says only folded or not.
    pub fn tray(running: bool, collapsed: bool, failed: bool) -> BentoBody {
        if !running { return basic(ModuleId::TrayOrganizer); }
        if failed { return BentoBody::Strip { caption: "Windows would not fold the icons".into() }; }
        BentoBody::Strip { caption: if collapsed { "Icons are folded away".into() } else { "All icons are in view".into() } }
    }

    /// App Switch has no recent-programs list to show, so it stays a basic tile: its switch shortcut and app shortcut count.
    pub fn app_switch(running: bool, switcher: Option<&str>, app_shortcuts: usize) -> BentoBody {
        if !running { return basic(ModuleId::AppSwitch); }
        let mut parts = vec![];
        if let Some(s) = switcher { parts.push(format!("{s} switches windows")); }
        if app_shortcuts > 0 { parts.push(format!("App shortcuts: {app_shortcuts}")); }
        BentoBody::Basic { line: if parts.is_empty() { off_line(ModuleId::AppSwitch).into() } else { parts.join(" · ") } }
    }

    /// Scroll speed 0.25…4 (1 = as Windows has it) on a log scale, and whether the wheel is reversed (settings, also when off).
    pub fn mouse(speed: f64, reverse: bool) -> BentoBody {
        let s = speed.clamp(0.25, 4.0);
        let f = clamp((s.ln() - 0.25f64.ln()) / (4.0f64.ln() - 0.25f64.ln()));
        let v = if speed == speed.round() { format!("{}×", speed as i64) } else { format!("{speed:.1}×") };
        BentoBody::Sliders { sliders: vec![BentoSlider { label: "Speed".into(), fraction: Some(f), value: v }], rows: vec![BentoLine::new("Reverse wheel", if reverse { "On" } else { "Off" })] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ModuleId::*;

    const NOW: f64 = 1_794_400_000.0;
    fn design_items() -> Vec<BentoItem> { BentoLayout::DESIGN_ORDER.iter().map(|m| BentoItem::new(*m, BentoLayout::default_size(*m))).collect() }
    fn ids(b: &BentoLayout) -> Vec<&str> { b.items.iter().map(|i| i.id.as_str()).collect() }

    // ----- layout -----

    #[test] fn the_default_board_puts_gadgets_that_are_on_first() {
        let b = BentoLayout::defaults(|m| matches!(m, Clipboard | KeepAwake));
        assert_eq!(&ids(&b)[..2], ["awake", "clipboard"], "on gadgets in design order");
        assert_eq!(b.items.len(), ModuleId::ALL.len() - 1, "every gadget once, not the board itself");
        assert_eq!(b.item(Clipboard).unwrap().size, BentoSize::Wide);
        assert_eq!(b.item(KeepAwake).unwrap().size, BentoSize::Small);
        assert!(b.items.iter().all(|i| !i.hidden));
        assert!(b.item(WidgetBoard).is_none());
    }

    #[test] fn the_design_board_packs_into_six_columns() {
        let (p, rows) = pack(&design_items(), 6, &["add"]);
        assert_eq!(rows, 3);
        let at = |id: &str| { let p = p.iter().find(|p| p.id == id).unwrap(); [p.column, p.row, p.columns, p.rows] };
        assert_eq!(at("monitor"), [0, 0, 2, 1]);
        assert_eq!(at("snap"), [2, 0, 1, 1]);
        assert_eq!(at("awake"), [3, 0, 1, 1]);
        assert_eq!(at("clipboard"), [4, 0, 2, 1]);
        assert_eq!(at("devices"), [2, 1, 2, 1]);
        assert_eq!(at("shelf"), [0, 2, 2, 1]);
        assert_eq!(at("mouse"), [4, 2, 1, 1]);
        assert_eq!(at("add"), [5, 2, 1, 1]);
        let mut cells = std::collections::BTreeSet::new();
        for q in &p { for x in q.column..q.column + q.columns { for y in q.row..q.row + q.rows { assert!(cells.insert((x, y)), "two tiles on one cell"); } } }
    }

    #[test] fn a_large_tile_lets_later_small_ones_fill_the_gap() {
        let items = vec![BentoItem::new(SystemMonitor, BentoSize::Large), BentoItem::new(WindowSnap, BentoSize::Small), BentoItem::new(KeepAwake, BentoSize::Small), BentoItem::new(Clipboard, BentoSize::Small)];
        let (p, rows) = pack(&items, 3, &[]);
        let at = |id: &str| { let p = p.iter().find(|p| p.id == id).unwrap(); (p.column, p.row) };
        assert_eq!(rows, 3);
        assert_eq!((at("monitor"), at("snap"), at("awake"), at("clipboard")), ((0, 0), (2, 0), (2, 1), (0, 2)), "the small ones fill beside the large one");
    }
    #[test] fn narrow_boards_reflow_and_narrow_wide_tiles() {
        let items = design_items();
        let (two, _) = pack(&items, 2, &[]);
        assert!(two.iter().all(|p| p.column + p.columns <= 2));
        let (one, rows) = pack(&items, 1, &[]);
        assert!(one.iter().all(|p| p.columns == 1));
        assert!(rows >= items.len());
        assert_eq!(columns_for_width(1138.0, 150.0, 12.0), 6);
        assert_eq!(columns_for_width(700.0, 150.0, 12.0), 4);
        assert_eq!(columns_for_width(400.0, 150.0, 12.0), 2);
    }

    #[test] fn edits_move_resize_hide_and_come_back() {
        let mut b = BentoLayout::defaults(|_| false);
        b.move_before(Clipboard, Some(SystemMonitor));
        b.set_size(Clipboard, BentoSize::Large);
        b.set_hidden(DiskMap, true);
        assert_eq!(b.items[0].id, "clipboard");
        assert_eq!(b.item(Clipboard).unwrap().size, BentoSize::Large);
        assert_eq!(b.hidden_modules(), vec![DiskMap]);
        assert!(!b.visible().iter().any(|i| i.id == "diskmap"));
        // It survives a save and a load.
        let again: BentoLayout = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(again, b);
        b.set_hidden(DiskMap, false);
        assert_eq!(b.items.last().unwrap().id, "diskmap", "a tile that comes back goes to the end");
        b.step(Clipboard, 1);
        assert_eq!(b.visible()[..2].iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["monitor", "clipboard"]);
        b.step(Clipboard, -1);
        b.step(Clipboard, -1);
        assert_eq!(b.visible()[0].id, "clipboard", "nothing before the first");
        let last = b.visible().last().unwrap().module().unwrap();
        b.step(last, 1);
        assert_eq!(b.visible().last().unwrap().module(), Some(last), "nothing after the last");
    }

    #[test] fn a_drop_takes_the_targets_place() {
        let mk = |v: &[ModuleId]| BentoLayout { version: 1, items: v.iter().map(|m| BentoItem::new(*m, BentoSize::Small)).collect() };
        let mut b = mk(&[SystemMonitor, WindowSnap, KeepAwake, Clipboard]);
        b.drop_onto(SystemMonitor, KeepAwake);
        assert_eq!(ids(&b), ["snap", "awake", "monitor", "clipboard"]);
        b.drop_onto(Clipboard, SystemMonitor);
        assert_eq!(ids(&b), ["snap", "awake", "clipboard", "monitor"]);
        b.drop_onto(WindowSnap, WindowSnap);
        assert_eq!(b.items[0].id, "snap");
        for m in board_modules() { for s in BentoSize::ALL { assert!(!size_text(m, s).is_empty(), "{m:?} {s:?}"); } }
    }

    #[test] fn boards_from_other_versions_load_safely() {
        let text = r#"{"version":3,"items":[{"id":"futureTool","size":"huge"},{"id":"clipboard","size":"giant","hidden":true},
            {"id":"clipboard","size":"wide"},{"id":"board"},{"id":"mouse","hidden":"yes"}]}"#;
        let b: BentoLayout = serde_json::from_str(text).unwrap();
        let b = b.normalized();
        assert_eq!(b.items.len(), ModuleId::ALL.len() - 1);
        assert_eq!(b.items[0].id, "clipboard");
        assert_eq!(b.item(Clipboard).unwrap().size, BentoSize::Small, "an unknown size becomes one cell");
        assert!(b.item(Clipboard).unwrap().hidden, "the first entry wins");
        assert!(!b.item(MouseTune).unwrap().hidden, "a broken flag becomes the default");
        assert!(!b.items.iter().any(|i| i.id == "futureTool" || i.id == "board"), "unknown ids and the board itself go");
    }

    #[test] fn the_board_in_settings_keeps_everything_else_and_is_off_by_default() {
        use crate::core::settings::AppSettings;
        let old: AppSettings = serde_json::from_str(r#"{"enabled":{"keepAwake":true},"dimIntensity":0.42,"pinnedTools":["clipboard"]}"#).unwrap();
        assert!(old.bento.is_none(), "a file from before the board has none");
        assert!(!old.is_on(WidgetBoard), "the board is off until it is turned on");
        assert!(old.is_on(KeepAwake));
        assert_eq!(old.dim_intensity, 0.42);
        assert_eq!(old.board_layout().items[0].id, "awake", "the default board starts with what is on");
        // Changing the board later does not reorder it when gadgets are switched.
        let mut s = old.clone();
        s.bento = Some(s.board_layout());
        s.set_on(DiskMap, true);
        assert_eq!(s.board_layout().items, s.bento.clone().unwrap().items);
        // A broken board in an otherwise good file falls back; the rest stays.
        let bad: AppSettings = serde_json::from_str(r#"{"enabled":{"keepAwake":true},"bento":{"items":7}}"#).unwrap();
        assert!(bad.bento.is_none());
        assert!(bad.is_on(KeepAwake));
        let sizes: AppSettings = serde_json::from_str(r#"{"bento":{"items":[{"id":"clipboard","size":"wide"}]}}"#).unwrap();
        assert_eq!(sizes.board_layout().items.len(), ModuleId::ALL.len() - 1, "missing gadgets are added");
        assert!(sizes.board_layout().items.iter().all(|i| !i.hidden || i.id != "clipboard"));
    }

    #[test] fn small_tiles_use_short_names_that_fit_a_narrow_cell() {
        for m in board_modules() {
            assert_eq!(tile_name(m, BentoSize::Wide), m.name());
            assert!(tile_name(m, BentoSize::Small).chars().count() <= 11, "{m:?}");
        }
        assert_eq!(tile_name(CalendarClock, BentoSize::Small), "Calendar");
        assert_eq!(tile_name(CalendarClock, BentoSize::Large), "Calendar Clock");
    }

    #[test] fn the_board_is_a_gadget_with_a_key_and_a_name() {
        assert_eq!(ModuleId::from_key("board"), Some(WidgetBoard));
        assert_eq!(WidgetBoard.name(), "Widget Board");
        assert!(ModuleId::ALL.contains(&WidgetBoard));
        assert!(!board_modules().contains(&WidgetBoard));
    }

    // ----- formats and bodies -----

    #[test] fn formats() {
        assert_eq!(fmt::rate(1_200_000.0), "1.2MB/s");
        assert_eq!(fmt::rate(84_000.0), "84KB/s");
        assert_eq!(fmt::rate(0.0), "0B/s");
        assert_eq!(fmt::size(612_000_000_000.0), "612GB");
        assert_eq!(fmt::size(1_000_000_000_000.0), "1TB");
        assert_eq!(fmt::size(2_000_000_000_000.0), "2TB");
        assert_eq!(fmt::remaining(74.0 * 60.0), "1:14");
        assert_eq!(fmt::remaining(14.0 * 60.0 - 5.0), "14 min");
        assert_eq!(fmt::remaining(45.0), "45 sec");
        assert_eq!(fmt::duration(7200.0), "2 h");
        assert_eq!(fmt::duration(5400.0), "1 h 30 min");
        assert_eq!(fmt::age(20.0), "just now");
        assert_eq!(fmt::age(180.0), "3 min");
        assert_eq!(fmt::age(7200.0), "2 h");
        assert_eq!(fmt::age(100_000.0), "yesterday");
        assert_eq!(fmt::age(5.0 * 86400.0), "5 d");
        assert_eq!(fmt::weekday(3), "Wednesday");
        assert_eq!(fmt::one_line("meet\nFriday   3 pm", 120), "meet Friday 3 pm");
        assert_eq!(fmt::one_line("abcdef", 3), "abc…");
        assert_eq!(BentoRing { label: "CPU".into(), fraction: None, tone: Tone::Green }.text(), "—");
        assert_eq!(BentoRing { label: "CPU".into(), fraction: Some(0.374), tone: Tone::Green }.text(), "37%");
        assert_eq!(BentoDot::Off.text(), "Off");
        assert_eq!(BentoDot::Error.text(), "On · error");
    }

    #[test] fn off_gadgets_without_data_are_basic_tiles() {
        for body in [build::monitor(false, Some(3.0), Some(40.0), Some(0.6), Some(1.0), Some(1.0)),
                     build::clipboard(false, &[(RowKind::Text, "secret".into(), Some(NOW))], NOW, 8),
                     build::capture(false, 2, Some("Ctrl+Alt+2")),
                     build::devices(false, Some(0.5), Some(80), Some("Speakers")),
                     build::shelf(false, &[(RowKind::File, "a.pdf".into())]),
                     build::tray(false, true, false),
                     build::keep_awake(false, true, true, Some(NOW), None, None, 60, NOW)] {
            assert!(matches!(body, BentoBody::Basic { .. }), "{body:?}");
        }
        // Off, yet real: values that live in the settings or the system.
        assert!(matches!(build::focus_dim(false, 0.5, false), BentoBody::Level { .. }));
        assert!(matches!(build::mouse(1.0, true), BentoBody::Sliders { .. }));
        assert!(matches!(build::disk(Some(612), Some(1000), &[]), BentoBody::Usage { .. }));
    }

    #[test] fn the_monitor_uses_only_measured_numbers() {
        let BentoBody::Gauges { rings, side } = build::monitor(true, Some(2.0), Some(37.0), None, Some(1_200_000.0), None) else { panic!() };
        assert_eq!(rings.iter().map(|r| r.text()).collect::<Vec<_>>(), ["2%", "37%", "—"]);
        assert_eq!(side.iter().map(|l| l.value.as_str()).collect::<Vec<_>>(), ["", "1.2MB/s", "—"]);
    }

    #[test] fn the_keep_awake_ring_needs_the_real_start() {
        let until = NOW + 74.0 * 60.0;
        let BentoBody::Timer { fraction, big, caption } = build::keep_awake(true, true, true, Some(until), Some(until - 7200.0), None, 30, NOW) else { panic!() };
        assert_eq!((big.as_str(), caption.as_str()), ("1:14", "left · 2 h"));
        assert!((fraction.unwrap() - 74.0 / 120.0).abs() < 1e-6);
        // Without the start the ring is not drawn (no guessing from the preset).
        let BentoBody::Timer { fraction, caption, .. } = build::keep_awake(true, true, true, Some(until), None, None, 30, NOW) else { panic!() };
        assert_eq!((fraction, caption.as_str()), (None, "left"));
        let BentoBody::Timer { big, caption, .. } = build::keep_awake(true, true, false, None, None, None, 30, NOW) else { panic!() };
        assert_eq!((big.as_str(), caption.as_str()), ("On", "Staying awake"));
        let BentoBody::Timer { big, caption, .. } = build::keep_awake(true, true, false, None, None, Some("Zoom is running"), 30, NOW) else { panic!() };
        assert_eq!((big.as_str(), caption.as_str()), ("On", "Zoom is running"));
        let BentoBody::Timer { big, caption, fraction } = build::keep_awake(true, false, false, None, None, None, 30, NOW) else { panic!() };
        assert_eq!((big.as_str(), caption.as_str(), fraction), ("Off", "Click to stay awake for 30 min", Some(0.0)));
    }

    #[test] fn the_clipboard_shows_titles_and_ages_but_never_image_content() {
        let items = [(RowKind::Text, "meet\nFriday 3 pm".to_string(), Some(NOW - 10.0)), (RowKind::Image, "Image".to_string(), Some(NOW - 720.0)), (RowKind::Link, "github.com".to_string(), Some(NOW - 180.0))];
        let BentoBody::List { rows, .. } = build::clipboard(true, &items, NOW, 2) else { panic!() };
        assert_eq!(rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["meet Friday 3 pm", "Image"]);
        assert_eq!(rows.iter().map(|r| r.detail.clone().unwrap()).collect::<Vec<_>>(), ["just now", "12 min"]);
        let BentoBody::List { rows, .. } = build::clipboard(true, &[(RowKind::Text, "x".into(), None)], NOW, 8) else { panic!() };
        assert_eq!(rows[0].detail, None, "no age is invented when the copy time is unknown");
        let BentoBody::List { rows, empty } = build::clipboard(true, &[], NOW, 8) else { panic!() };
        assert!(rows.is_empty());
        assert_eq!(empty, "Nothing copied yet");
    }

    #[test] fn devices_show_brightness_only_for_a_display_that_takes_it() {
        let BentoBody::Sliders { sliders, rows } = build::devices(true, Some(0.64), None, Some("Headset")) else { panic!() };
        assert_eq!(sliders.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["Volume"]);
        assert_eq!(sliders[0].value, "64");
        assert_eq!(rows[0].label, "Headset");
        let BentoBody::Sliders { sliders, .. } = build::devices(true, None, Some(80), None) else { panic!() };
        assert_eq!(sliders.iter().map(|s| s.value.as_str()).collect::<Vec<_>>(), ["—", "80"]);
    }

    #[test] fn the_calendar_tile_never_invents_an_event() {
        let worlds = vec![("Tokyo".to_string(), "09:00".to_string()), ("Paris".into(), "01:00".into()), ("Lima".into(), "19:00".into())];
        let BentoBody::Calendar { weekday, day, lines, note } = build::calendar(true, 3, 14, &worlds) else { panic!() };
        assert_eq!((weekday.as_str(), day.as_str(), lines.len(), note), ("Wednesday", "14", 2, None));
        let BentoBody::Calendar { lines, note, .. } = build::calendar(true, 3, 14, &[]) else { panic!() };
        assert!(lines.is_empty());
        assert_eq!(note.as_deref(), Some("Click for the calendar"));
        let BentoBody::Calendar { lines, note, .. } = build::calendar(false, 3, 14, &worlds) else { panic!() };
        assert!(lines.is_empty(), "no clocks from a gadget that is off");
        assert!(note.unwrap().starts_with("Turn on"));
    }

    #[test] fn disk_segments_come_from_the_scan_only() {
        let BentoBody::Usage { big, caption, segments } = build::disk(Some(612_000_000_000), Some(1_000_000_000_000), &[]) else { panic!() };
        assert_eq!((big.as_str(), caption.as_str()), ("612GB", "/ 1TB used"));
        assert_eq!(segments.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["Used"]);
        let BentoBody::Usage { segments, .. } = build::disk(Some(600), Some(1000), &[("Users".into(), 300), ("Windows".into(), 100)]) else { panic!() };
        assert_eq!(segments.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["Users", "Windows", "Other"]);
        assert!((segments.iter().map(|s| s.fraction).sum::<f64>() - 0.6).abs() < 1e-9);
        assert!(matches!(build::disk(None, None, &[]), BentoBody::Basic { .. }));
        assert!(matches!(build::disk(Some(1), Some(0), &[]), BentoBody::Basic { .. }), "no division by a zero disk");
    }

    #[test] fn the_tray_tile_has_no_invented_icon_count_and_app_switch_is_basic() {
        assert_eq!(build::tray(true, true, false), BentoBody::Strip { caption: "Icons are folded away".into() });
        assert_eq!(build::tray(true, false, false), BentoBody::Strip { caption: "All icons are in view".into() });
        assert_eq!(build::tray(true, true, true), BentoBody::Strip { caption: "Windows would not fold the icons".into() });
        assert_eq!(build::app_switch(true, Some("Alt+Tab"), 2), BentoBody::Basic { line: "Alt+Tab switches windows · App shortcuts: 2".into() });
        assert_eq!(build::app_switch(true, None, 0), BentoBody::Basic { line: off_line(AppSwitch).into() });
    }

    #[test] fn snap_captions_count_the_shortcuts() {
        let BentoBody::Layouts { shapes, caption } = build::window_snap(true, Some("Ctrl+Alt+←"), 3) else { panic!() };
        assert_eq!((shapes.len(), caption.as_deref()), (4, Some("Ctrl+Alt+← and 3 more")));
        let BentoBody::Layouts { caption, .. } = build::window_snap(true, None, 1) else { panic!() };
        assert_eq!(caption.as_deref(), Some("Shortcuts: 1"));
        let BentoBody::Layouts { caption, .. } = build::window_snap(false, Some("x"), 1) else { panic!() };
        assert_eq!(caption.as_deref(), Some(off_line(WindowSnap)));
        let BentoBody::Stack { caption, .. } = build::capture(true, 1, None) else { panic!() };
        assert_eq!(caption, "Floats open: 1");
    }

    #[test] fn the_mouse_speed_is_a_log_scale() {
        let BentoBody::Sliders { sliders, rows } = build::mouse(1.0, false) else { panic!() };
        assert_eq!(sliders[0].value, "1×");
        assert!((sliders[0].fraction.unwrap() - 4f64.ln() / 16f64.ln()).abs() < 1e-9);
        assert_eq!(rows[0].value, "Off");
        let BentoBody::Sliders { sliders, .. } = build::mouse(1.5, true) else { panic!() };
        assert_eq!(sliders[0].value, "1.5×");
        let BentoBody::Level { fraction, caption, .. } = build::focus_dim(true, 0.5, true) else { panic!() };
        assert_eq!((fraction, caption.as_str()), (0.5, "Paused"));
    }
}
