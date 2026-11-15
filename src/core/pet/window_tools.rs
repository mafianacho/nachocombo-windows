//! The Window Snap tools of Nacho and MCP (`window_displays`, `window_place`, `window_tile`, `window_split`,
//! `window_ratio`, `window_move_display`, `window_arrange_apps`, `window_undo`), the platform-free part: the geometry
//! and argument checks of the macOS `WindowTiling.swift` and the decisions of the macOS app's `WindowToolRunner.swift`
//! (which window a name means, which display, where every window goes, what the result says). The Win32 shell only
//! reads the windows and displays, moves the windows in the planned order and reads their frames back.
//!
//! Windows changes: rectangles use top-left screen coordinates (macOS: Cocoa, bottom-left), so row 0 is the top row
//! and `y` of `window_place` is measured down from the top of the usable area, as the tool contract says on both
//! systems. Display numbers run from the left, starting at 1 (`PetDisplays`); `main` is the display Windows lists
//! first. A window is the Nth window of its program in the shell's front-to-back list (the numbers
//! `combo_list_windows` shows); minimized windows are never moved. A move Windows refuses (a window of an app running
//! as administrator) is reported as `elevated`. Window titles are never read or returned.
use super::commands::{PetCommandOutcome, PetCommandProblem, PetCommands};
use super::desktop::{PetAppEntry, PetDisplays, PetPermissionHelp};
use super::desktop_runner::{PetDesktopRunner, PetWindowInfo};
use super::json::JsonObject;
use crate::core::layout::{self, Rect};
use serde_json::{json, Value};
use std::collections::HashSet;

// MARK: - names of the closed lists

/// One window as a tool caller names it: an app (display name, English name or exe name, as in `combo_layout_windows`)
/// and the window's order in that app, front to back, from 1. `id` is the same thing as one string, `"<app>#<n>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowSelector {
    pub app: String,
    pub window: usize,
}

impl WindowSelector {
    pub fn new(app: &str, window: usize) -> Self { Self { app: app.to_string(), window } }
    pub fn id(&self) -> String { format!("{}#{}", self.app, self.window) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSplitDirection {
    /// Side by side: columns, left to right.
    Horizontal,
    /// Stacked: rows, top to bottom.
    Vertical,
    /// Four cells in two rows: top-left, top-right, bottom-left, bottom-right.
    Quadrant,
}

/// Presets of `window_arrange_apps`: how the listed apps share a display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowArrangement { Grid, Columns, Rows, Quadrants }

/// Order in which windows fill grid cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowFill {
    /// Left to right along the top row first.
    RowMajor,
    /// Top to bottom down the left column first.
    ColumnMajor,
}

/// How a grid uses several displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSpan {
    /// Every display gets its own grid; the windows are shared out between them in order.
    PerDisplay,
    /// One grid over the area all chosen displays cover together (cells may sit across a display edge).
    Across,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowCoordinateUnit {
    /// 0...1 of the display's usable area (taskbar excluded). Origin top-left.
    Ratio,
    /// Pixels from the top-left corner of the usable area.
    Pixel,
}

/// A closed list of the tool schemas: every value and its JSON name.
trait Choice: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    fn raw(self) -> &'static str;
    fn from_raw(raw: &str) -> Option<Self> { Self::ALL.iter().copied().find(|c| c.raw() == raw) }
}

impl Choice for WindowSplitDirection {
    const ALL: &'static [Self] = &[Self::Horizontal, Self::Vertical, Self::Quadrant];
    fn raw(self) -> &'static str {
        match self { Self::Horizontal => "horizontal", Self::Vertical => "vertical", Self::Quadrant => "quadrant" }
    }
}

impl Choice for WindowArrangement {
    const ALL: &'static [Self] = &[Self::Grid, Self::Columns, Self::Rows, Self::Quadrants];
    fn raw(self) -> &'static str {
        match self { Self::Grid => "grid", Self::Columns => "columns", Self::Rows => "rows", Self::Quadrants => "quadrants" }
    }
}

impl Choice for WindowFill {
    const ALL: &'static [Self] = &[Self::RowMajor, Self::ColumnMajor];
    fn raw(self) -> &'static str { match self { Self::RowMajor => "row_major", Self::ColumnMajor => "column_major" } }
}

impl Choice for WindowSpan {
    const ALL: &'static [Self] = &[Self::PerDisplay, Self::Across];
    fn raw(self) -> &'static str { match self { Self::PerDisplay => "per_display", Self::Across => "across" } }
}

impl Choice for WindowCoordinateUnit {
    const ALL: &'static [Self] = &[Self::Ratio, Self::Pixel];
    fn raw(self) -> &'static str { match self { Self::Ratio => "ratio", Self::Pixel => "pixel" } }
}

// MARK: - geometry

fn union(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect::new(x, y, a.right().max(b.right()) - x, a.bottom().max(b.bottom()) - y)
}

#[cfg(test)]
fn encloses(outer: &Rect, inner: &Rect) -> bool {
    inner.x >= outer.x && inner.y >= outer.y && inner.right() <= outer.right() && inner.bottom() <= outer.bottom()
}

/// Pure geometry of the window tools. Areas are usable display frames (work areas) in top-left screen coordinates;
/// grid cells come out in the order windows fill them. Errors are English texts for the model.
pub struct WindowTiling;

impl WindowTiling {
    /// Technical limit of one call (payload size), not a layout limit: windows per call.
    pub const MAX_WINDOWS: usize = 200;
    pub const MAX_APPS: usize = 200;
    pub const MAX_ROWS_OR_COLUMNS: usize = 100;
    pub const MAX_RATIO_PARTS: usize = 4;
    pub const MAX_RATIO_WEIGHT: usize = 100;
    pub const MAX_DISPLAYS: usize = 16;
    pub const UNDO_STEPS: usize = 5;
    /// A cell with a shorter side than this still gets placed; the result says it may not fit the app's own minimum.
    pub const COMFORTABLE_SIDE: f64 = 120.0;

    /// Splits `length` into pieces proportional to `weights`, on whole pixels: the boundaries are integer fractions of
    /// the length and the last piece ends exactly at `length`, so neighbours never overlap or leave a gap.
    /// Pieces are (offset, length).
    pub fn divide(length: f64, weights: &[usize]) -> Vec<(f64, f64)> {
        let sum: usize = weights.iter().sum();
        if sum == 0 || weights.is_empty() { return vec![]; }
        let whole = length.floor() as i64;
        let mut out = Vec::with_capacity(weights.len());
        let (mut cumulative, mut start) = (0usize, 0.0);
        for (index, weight) in weights.iter().enumerate() {
            cumulative += weight;
            let end = if index == weights.len() - 1 { length } else { (whole * cumulative as i64 / sum as i64) as f64 };
            out.push((start, end - start));
            start = end;
        }
        out
    }

    /// Cells per row of the automatic grid for `count` windows: `ceil(sqrt(n))` columns, the last row may be shorter
    /// (and then its cells are wider). 1 → [1], 2 → [2], 3 → [2,1], 4 → [2,2], 5 → [3,2], 7 → [3,3,1], 9 → [3,3,3].
    pub fn grid_rows(count: usize) -> Vec<usize> {
        if count == 0 { return vec![]; }
        let columns = Self::root(count);
        let mut rows = vec![];
        let mut left = count;
        while left > 0 { rows.push(columns.min(left)); left = left.saturating_sub(columns); }
        rows
    }

    /// The smallest whole number whose square holds `count`.
    fn root(count: usize) -> usize {
        let mut columns = 1;
        while columns * columns < count { columns += 1; }
        columns
    }

    /// Rows of cells; `shape[i]` is the number of cells of row i (top to bottom). Cells come out row by row.
    fn rows(shape: &[usize], visible: Rect, gap: f64) -> Vec<Rect> {
        let heights = Self::divide(visible.h, &vec![1; shape.len()]);
        let mut cells = vec![];
        for (row, &count) in shape.iter().enumerate() {
            let (offset, height) = heights[row];
            for (x, width) in Self::divide(visible.w, &vec![1; count]) {
                cells.push(layout::apply_gap(Rect::new(visible.x + x, visible.y + offset, width, height), visible, gap, true));
            }
        }
        cells
    }

    /// A full `rows` x `cols` matrix over `area`, in fill order (row by row or column by column).
    pub fn matrix(rows: usize, cols: usize, area: Rect, gap: f64, fill: WindowFill) -> Vec<Rect> {
        if rows == 0 || cols == 0 { return vec![]; }
        let by_row = Self::rows(&vec![cols; rows], area, gap);
        if fill == WindowFill::RowMajor { return by_row; }
        (0..cols).flat_map(|c| (0..rows).map(move |r| (r, c))).map(|(r, c)| by_row[r * cols + c]).collect()
    }

    fn check_count(count: usize) -> Result<(), String> {
        if (1..=Self::MAX_WINDOWS).contains(&count) { Ok(()) } else { Err(format!("A grid needs 1 to {} windows.", Self::MAX_WINDOWS)) }
    }

    /// Automatic grid for `count` windows (any number): `ceil(sqrt(n))` columns, short last row stretched.
    pub fn grid(count: usize, visible: Rect, gap: f64) -> Result<Vec<Rect>, String> {
        Self::check_count(count)?;
        Ok(Self::rows(&Self::grid_rows(count), visible, gap))
    }

    /// Grid of `rows` x `cols` (either may be absent, then it follows from the count) in fill order, on one area.
    pub fn grid_shaped(count: usize, rows: Option<usize>, cols: Option<usize>, fill: WindowFill, area: Rect, gap: f64) -> Result<Vec<Rect>, String> {
        Self::check_count(count)?;
        if rows.is_none() && cols.is_none() && fill == WindowFill::RowMajor { return Self::grid(count, area, gap); }
        let (r, c) = match (rows, cols) {
            (Some(r), Some(c)) => (r, c),
            (Some(r), None) => (r, if r == 0 { 0 } else { count.div_ceil(r) }),
            (None, Some(c)) => (if c == 0 { 0 } else { count.div_ceil(c) }, c),
            (None, None) => { let c = Self::root(count); (count.div_ceil(c), c) }
        };
        if r < 1 || c < 1 || r > Self::MAX_ROWS_OR_COLUMNS || c > Self::MAX_ROWS_OR_COLUMNS {
            return Err(format!("rows and cols must be from 1 to {}.", Self::MAX_ROWS_OR_COLUMNS));
        }
        if r * c < count { return Err(format!("{r} x {c} has {} cells for {count} windows; use more rows or columns.", r * c)); }
        Ok(Self::matrix(r, c, area, gap, fill).into_iter().take(count).collect())
    }

    /// Grid over several displays. `areas` are the usable frames of the chosen displays, in the order given.
    /// `PerDisplay`: the windows are shared out in order (a full grid per display when rows and cols are given,
    /// otherwise evenly) and each display gets its own grid. `Across`: one grid over the bounding box of all displays.
    pub fn tile(count: usize, rows: Option<usize>, cols: Option<usize>, fill: WindowFill, span: WindowSpan, areas: &[Rect], gap: f64) -> Result<Vec<Rect>, String> {
        let Some(&first) = areas.first() else { return Err("There is no display to place the windows on.".into()) };
        Self::check_count(count)?;
        if areas.len() == 1 || span == WindowSpan::Across {
            let area = areas[1..].iter().fold(first, |a, b| union(a, *b));
            let mut c = cols;
            if areas.len() > 1 && rows.is_none() && cols.is_none() {
                // Auto shape over several displays: a whole number of columns per display.
                c = Some(Self::root(count).div_ceil(areas.len()) * areas.len());
            }
            return Self::grid_shaped(count, rows, c, fill, area, gap);
        }
        let mut sizes = vec![];
        if let (Some(r), Some(c)) = (rows, cols) {
            let mut left = count;
            for _ in areas { let take = left.min(r * c); sizes.push(take); left -= take; }
            if left > 0 {
                return Err(format!("{r} x {c} on {} display{} has {} cells for {count} windows.", areas.len(),
                                   if areas.len() == 1 { "" } else { "s" }, r * c * areas.len()));
            }
        } else {
            for i in 0..areas.len() { sizes.push(count / areas.len() + usize::from(i < count % areas.len())); }
        }
        let mut cells = vec![];
        for (index, &size) in sizes.iter().enumerate() {
            if size > 0 { cells.extend(Self::grid_shaped(size, rows, cols, fill, areas[index], gap)?); }
        }
        Ok(cells)
    }

    /// 2, 3 or 4 equal parts. `Quadrant` needs 4 parts.
    pub fn split(parts: usize, direction: WindowSplitDirection, visible: Rect, gap: f64) -> Result<Vec<Rect>, String> {
        if !(2..=4).contains(&parts) { return Err("parts must be 2, 3 or 4.".into()); }
        match direction {
            WindowSplitDirection::Horizontal => Ok(Self::rows(&[parts], visible, gap)),
            WindowSplitDirection::Vertical => Ok(Self::rows(&vec![1; parts], visible, gap)),
            WindowSplitDirection::Quadrant => {
                if parts != 4 { return Err("quadrant needs parts = 4.".into()); }
                Ok(Self::rows(&[2, 2], visible, gap))
            }
        }
    }

    /// 2…4 parts in the given proportions (70:30 → weights [70, 30]); only horizontal or vertical.
    pub fn ratio(weights: &[i64], direction: WindowSplitDirection, visible: Rect, gap: f64) -> Result<Vec<Rect>, String> {
        if !(2..=Self::MAX_RATIO_PARTS).contains(&weights.len()) { return Err(format!("ratios needs 2 to {} numbers.", Self::MAX_RATIO_PARTS)); }
        if !weights.iter().all(|w| (1..=Self::MAX_RATIO_WEIGHT as i64).contains(w)) {
            return Err(format!("every ratio must be a whole number from 1 to {}.", Self::MAX_RATIO_WEIGHT));
        }
        let weights: Vec<usize> = weights.iter().map(|w| *w as usize).collect();
        match direction {
            WindowSplitDirection::Quadrant => Err("ratios split in one direction: horizontal or vertical.".into()),
            WindowSplitDirection::Horizontal => Ok(Self::divide(visible.w, &weights).into_iter()
                .map(|(x, w)| layout::apply_gap(Rect::new(visible.x + x, visible.y, w, visible.h), visible, gap, true)).collect()),
            WindowSplitDirection::Vertical => Ok(Self::divide(visible.h, &weights).into_iter()
                .map(|(y, h)| layout::apply_gap(Rect::new(visible.x, visible.y + y, visible.w, h), visible, gap, true)).collect()),
        }
    }

    /// How many cells are shorter than `COMFORTABLE_SIDE` on a side (apps may refuse to get that small; the result
    /// reports it).
    pub fn tight_cells(cells: &[Rect]) -> usize { cells.iter().filter(|c| c.w.min(c.h) < Self::COMFORTABLE_SIDE).count() }

    /// Usable frame of Nacho's 1-based display number (from the left), or None.
    pub fn visible_frame(number: i64, full: &[Rect], visible: &[Rect]) -> Option<Rect> {
        if full.len() != visible.len() { return None; }
        PetDisplays::index_for_number(number, full).map(|i| visible[i])
    }
}

// MARK: - free placement

/// One window of `window_place`: where it goes inside a display's usable area, top-left origin.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowPlacementItem {
    pub target: WindowSelector,
    pub display: Option<i64>,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub z: Option<i64>,
}

pub struct WindowPlacement;

impl WindowPlacement {
    /// Screen frame of an item inside `visible`, clamped so the window stays on that display, on whole pixels.
    /// The flag says that the request reached outside the usable area (or was bigger than it).
    pub fn frame(item: &WindowPlacementItem, unit: WindowCoordinateUnit, visible: Rect) -> (Rect, bool) {
        let (sx, sy) = if unit == WindowCoordinateUnit::Ratio { (visible.w, visible.h) } else { (1.0, 1.0) };
        let wanted = ((item.x * sx).round(), (item.y * sy).round(), (item.w * sx).round(), (item.h * sy).round());
        let w = visible.w.min(wanted.2.max(1.0));
        let h = visible.h.min(wanted.3.max(1.0));
        let x = wanted.0.max(0.0).min(visible.w - w);
        let y = wanted.1.max(0.0).min(visible.h - h);
        let clamped = (wanted.0 - x).abs() > 0.5 || (wanted.1 - y).abs() > 0.5 || (wanted.2 - w).abs() > 0.5 || (wanted.3 - h).abs() > 0.5;
        (Rect::new(visible.x + x, visible.y + y, w, h), clamped)
    }

    /// Order in which windows are brought to the front: ascending `z`; items without one keep their list position.
    /// The last one ends up on top.
    pub fn raise_order(items: &[WindowPlacementItem]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&i| (items[i].z.unwrap_or(i as i64), i));
        order
    }
}

// MARK: - tool requests

/// A checked window tool call.
#[derive(Clone, Debug, PartialEq)]
pub enum PetWindowToolRequest {
    Displays,
    /// `items` in the order given (z order: `WindowPlacement::raise_order`).
    Place { items: Vec<WindowPlacementItem>, unit: WindowCoordinateUnit },
    /// `windows: None` = every open window of every app (the "whole checkerboard"). `gap: None` = the app's snap gap.
    Tile { windows: Option<Vec<WindowSelector>>, rows: Option<usize>, cols: Option<usize>, displays: Option<Vec<i64>>,
           span: WindowSpan, fill: WindowFill, gap: Option<f64> },
    Split { parts: usize, direction: WindowSplitDirection, windows: Vec<WindowSelector>, display: Option<i64> },
    Ratio { weights: Vec<i64>, direction: WindowSplitDirection, windows: Vec<WindowSelector>, display: Option<i64> },
    MoveDisplay { windows: Vec<WindowSelector>, display: i64 },
    ArrangeApps { apps: Vec<String>, arrangement: WindowArrangement, display: Option<i64> },
    Undo { steps: usize },
}

impl PetWindowToolRequest {
    /// The target cells of a request that fits one display (split, ratio, arrange_apps). Tile and place need the
    /// display list and the window count, so `PetWindowTools::plan` computes them with `WindowTiling::tile` /
    /// `WindowPlacement::frame`.
    pub fn cells(&self, visible: Rect, gap: f64) -> Option<Result<Vec<Rect>, String>> {
        match self {
            Self::Split { parts, direction, .. } => Some(WindowTiling::split(*parts, *direction, visible, gap)),
            Self::Ratio { weights, direction, .. } => Some(WindowTiling::ratio(weights, *direction, visible, gap)),
            Self::ArrangeApps { apps, arrangement, .. } => Some(match arrangement {
                WindowArrangement::Grid => WindowTiling::grid(apps.len(), visible, gap),
                WindowArrangement::Columns => Ok(WindowTiling::matrix(1, apps.len(), visible, gap, WindowFill::RowMajor)),
                WindowArrangement::Rows => Ok(WindowTiling::matrix(apps.len(), 1, visible, gap, WindowFill::RowMajor)),
                WindowArrangement::Quadrants => WindowTiling::split(4, WindowSplitDirection::Quadrant, visible, gap),
            }),
            Self::Displays | Self::Tile { .. } | Self::Place { .. } | Self::MoveDisplay { .. } | Self::Undo { .. } => None,
        }
    }
}

/// One display as the shell reads it: full frame, work area (taskbar excluded) and the name people see.
#[derive(Clone, Debug, PartialEq)]
pub struct PetDisplayInfo {
    pub frame: Rect,
    pub work: Rect,
    pub name: String,
}

/// One window move of a plan.
#[derive(Clone, Debug, PartialEq)]
pub struct PetWindowMove {
    pub id: isize,
    /// `"<app> #<n>"` (the macOS label; it also works as an `id` in the next call).
    pub label: String,
    /// The frame before the move (what `window_undo` puts back).
    pub before: Rect,
    /// Where the window should go, screen coordinates.
    pub requested: Rect,
    /// Said in the result's reason (`clamped to the display`).
    pub note: Option<String>,
    /// Position in the request; the result lists the windows in this order.
    pub item: usize,
}

/// What a window tool call does. `moves` are in the order the shell applies them; the shell brings each moved window
/// to the front, so the last one ends on top (z order of `window_place`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetWindowPlan {
    pub moves: Vec<PetWindowMove>,
    /// `"<label>: <why>"` for every window that could not be found (the result's `missing`).
    pub missing: Vec<String>,
    /// The same windows as (label, why), for the result's `windows` rows.
    pub problems: Vec<(String, String)>,
    /// Opening words of the message (`Tile`, `Place`, `Move to display 2` …).
    pub what: String,
    /// Cells under `WindowTiling::COMFORTABLE_SIDE` on a side.
    pub tight: usize,
    /// Full display frames, to report frames from their display's top-left corner.
    pub screens: Vec<Rect>,
}

/// One remembered placement: (window id, label, frame before the move) of every window it moved.
pub type PetWindowSnapshot = Vec<(isize, String, Rect)>;

/// Frames of the windows just before a tool moved them, for `window_undo` (macOS `WindowUndoStack`).
#[derive(Clone, Debug, PartialEq)]
pub struct PetWindowUndoStack {
    pub capacity: usize,
    snapshots: Vec<PetWindowSnapshot>,
}

impl Default for PetWindowUndoStack {
    fn default() -> Self { Self::new(10) }
}

impl PetWindowUndoStack {
    pub fn new(capacity: usize) -> Self { Self { capacity: capacity.max(1), snapshots: vec![] } }
    pub fn len(&self) -> usize { self.snapshots.len() }
    pub fn is_empty(&self) -> bool { self.snapshots.is_empty() }

    /// Remember one placement. An empty one is ignored. The oldest falls out beyond `capacity`.
    pub fn push(&mut self, snapshot: PetWindowSnapshot) {
        if snapshot.is_empty() { return; }
        self.snapshots.push(snapshot);
        if self.snapshots.len() > self.capacity { self.snapshots.drain(..self.snapshots.len() - self.capacity); }
    }

    /// The newest `steps` snapshots, newest first (apply them in this order so the oldest frame wins), removed from
    /// the stack.
    pub fn pop(&mut self, steps: usize) -> Vec<PetWindowSnapshot> {
        (0..steps).map_while(|_| self.snapshots.pop()).collect()
    }
}

// MARK: - argument checks

fn bad(text: impl Into<String>) -> PetCommandProblem { PetCommandProblem::BadParams(text.into()) }

fn only(o: &JsonObject, allowed: &[&str]) -> Result<(), PetCommandProblem> {
    let mut extra: Vec<&str> = o.keys().map(String::as_str).filter(|k| !allowed.contains(k)).collect();
    extra.sort_unstable();
    if extra.is_empty() { Ok(()) } else { Err(bad(format!("unknown field {}", extra.join(", ")))) }
}

fn int(v: Option<&Value>, name: &str, lo: i64, hi: i64) -> Result<Option<i64>, PetCommandProblem> {
    let Some(v) = v else { return Ok(None) };
    let Some(n) = PetCommands::integer(Some(v)) else { return Err(bad(format!("{name} must be a whole number from {lo} to {hi}"))) };
    if !(lo..=hi).contains(&n) { return Err(bad(format!("{name} must be from {lo} to {hi}"))); }
    Ok(Some(n))
}

fn number(v: Option<&Value>, name: &str, lo: f64, hi: f64) -> Result<f64, PetCommandProblem> {
    let Some(n) = v.filter(|v| v.is_number()).and_then(Value::as_f64) else { return Err(bad(format!("{name} must be a number"))) };
    if !n.is_finite() || !(lo..=hi).contains(&n) { return Err(bad(format!("{name} must be from {lo} to {hi}"))); }
    Ok(n)
}

fn choice<T: Choice>(v: Option<&Value>, name: &str) -> Result<Option<T>, PetCommandProblem> {
    let Some(v) = v else { return Ok(None) };
    match v.as_str().and_then(T::from_raw) {
        Some(found) => Ok(Some(found)),
        None => Err(bad(format!("{name} must be one of {}", T::ALL.iter().map(|c| c.raw()).collect::<Vec<_>>().join(", ")))),
    }
}

fn selector(o: &JsonObject) -> Result<WindowSelector, PetCommandProblem> {
    let mut order = int(o.get("window"), "window", 1, 20)?;
    let mut app: Option<String> = None;
    if let Some(Value::String(id)) = o.get("id") {
        if o.contains_key("app") || o.contains_key("window") { return Err(bad("give either id or app (+ window), not both")); }
        let Some((name, n)) = id.split_once('#') else { return Err(bad("id must look like notepad.exe#1")) };
        let Some(n) = n.parse::<i64>().ok().filter(|n| (1..=20).contains(n)) else { return Err(bad("id must look like notepad.exe#1")) };
        app = Some(name.to_string());
        order = Some(n);
    } else if let Some(Value::String(raw)) = o.get("app") {
        app = Some(raw.clone());
    }
    let max = PetCommands::MAX_APP_NAME_LENGTH;
    let Some(name) = app.map(|a| a.trim().to_string()).filter(|n| !n.is_empty() && n.chars().count() <= max) else {
        return Err(bad(format!("each window needs an app (or id) of 1 to {max} characters")));
    };
    Ok(WindowSelector { app: name, window: order.unwrap_or(1) as usize })
}

fn selectors(v: Option<&Value>, max: usize) -> Result<Vec<WindowSelector>, PetCommandProblem> {
    let Some(Value::Array(list)) = v else { return Err(bad("windows must be a list of {app, window}")) };
    if list.is_empty() || list.len() > max { return Err(bad(format!("windows needs 1 to {max} items"))); }
    let mut seen = HashSet::new();
    list.iter().map(|item| {
        let Value::Object(o) = item else { return Err(bad("each window must be an object {app, window}")) };
        only(o, &["app", "window", "id"])?;
        let r = selector(o)?;
        if !seen.insert(r.id().to_lowercase()) { return Err(bad(format!("the same window is listed twice: {}", r.id()))); }
        Ok(r)
    }).collect()
}

fn display_list(v: Option<&Value>) -> Result<Option<Vec<i64>>, PetCommandProblem> {
    let Some(v) = v else { return Ok(None) };
    let max = WindowTiling::MAX_DISPLAYS;
    let list = v.as_array().filter(|l| !l.is_empty() && l.len() <= max)
        .ok_or_else(|| bad(format!("displays must list 1 to {max} display numbers")))?;
    let mut numbers = vec![];
    for item in list { numbers.extend(int(Some(item), "each display", 1, max as i64)?); }
    if numbers.iter().collect::<HashSet<_>>().len() != numbers.len() { return Err(bad("displays lists a display twice")); }
    Ok(Some(numbers))
}

// MARK: - the tools

/// What the shell needs to run the window tools: argument checks, plans, and the answers.
pub struct PetWindowTools;

/// One window a request names, found or not.
struct Found<'a> {
    index: usize,
    label: String,
    window: Option<&'a PetWindowInfo>,
    problem: Option<String>,
}

impl Found<'_> {
    fn frame(&self) -> Rect { self.window.map(|w| w.frame).unwrap_or_default() }
}

/// The windows and displays one plan works with.
struct Desk<'a> {
    windows: &'a [PetWindowInfo],
    running: Vec<PetAppEntry>,
    full: Vec<Rect>,
    works: Vec<Rect>,
}

fn plural(n: usize) -> &'static str { if n == 1 { "" } else { "s" } }

fn details(value: Value) -> Option<JsonObject> { value.as_object().cloned() }

fn failure(message: impl Into<String>) -> PetCommandOutcome { PetCommandOutcome::new(false, message) }

/// A frame from the top-left corner of the display it is on (like `combo_list_windows`).
fn report(frame: Rect, screens: &[Rect]) -> Value {
    let d = layout::screen_index(&frame, screens).map(|i| screens[i]).unwrap_or_default();
    json!({"x": (frame.x - d.x).round() as i64, "y": (frame.y - d.y).round() as i64,
           "width": frame.w.round() as i64, "height": frame.h.round() as i64})
}

impl<'a> Desk<'a> {
    fn new(windows: &'a [PetWindowInfo], displays: &[PetDisplayInfo]) -> Self {
        Self { windows, running: PetDesktopRunner::running_entries(windows),
               full: displays.iter().map(|d| d.frame).collect(), works: displays.iter().map(|d| d.work).collect() }
    }

    fn resolve(&self, index: usize, r: &WindowSelector) -> Found<'a> {
        let missing = |why: String| Found { index, label: format!("{} #{}", r.app, r.window), window: None, problem: Some(why) };
        let entry = match PetDesktopRunner::lookup(&r.app, &self.running, &[], true) {
            Ok(entry) => entry,
            Err(outcome) => return missing(outcome.message),
        };
        let label = format!("{} #{}", entry.display_name(), r.window);
        let mine = PetDesktopRunner::windows_of(&entry.bundle_id, self.windows);
        match mine.get(r.window.wrapping_sub(1)) {
            None => missing(format!("{} has {} open window{}, not {}", entry.display_name(), mine.len(), plural(mine.len()), r.window)),
            Some(w) if w.minimized => Found { index, label, window: None, problem: Some("the window is minimized".into()) },
            Some(w) => Found { index, label, window: Some(*w), problem: None },
        }
    }

    /// Every selector, in order; selectors that cannot be found carry the reason (they never stop the others).
    fn resolve_all(&self, refs: &[WindowSelector]) -> Vec<Found<'a>> {
        let mut seen = HashSet::new();
        refs.iter().enumerate().map(|(index, r)| {
            let mut f = self.resolve(index, r);
            if let Some(w) = f.window {
                if !seen.insert(w.id) { f.window = None; f.problem = Some("the same window is listed twice".into()); }
            }
            f
        }).collect()
    }

    /// Every open, not minimized window of every app, app by app (front app first), at most `MAX_WINDOWS`.
    fn all_open_windows(&self) -> Vec<Found<'a>> {
        let mut found = vec![];
        for entry in &self.running {
            for (order, w) in PetDesktopRunner::windows_of(&entry.bundle_id, self.windows).into_iter().enumerate() {
                if w.minimized || found.len() >= WindowTiling::MAX_WINDOWS { continue; }
                found.push(Found { index: found.len(), label: format!("{} #{}", entry.display_name(), order + 1), window: Some(w), problem: None });
            }
        }
        found
    }

    fn visible_frames(&self, numbers: &[i64]) -> Result<Vec<Rect>, PetCommandOutcome> {
        numbers.iter().map(|&n| WindowTiling::visible_frame(n, &self.full, &self.works).ok_or_else(|| {
            failure(format!("there is no display {n} (this PC has {}); see window_displays", self.full.len()))
        })).collect()
    }

    /// The work area of the display a window is on.
    fn usable_frame(&self, frame: Rect) -> Rect {
        layout::screen_index(&frame, &self.works).map(|i| self.works[i]).unwrap_or_default()
    }

    fn problems(found: &[Found]) -> Vec<(String, String)> {
        found.iter().filter_map(|f| f.problem.clone().map(|why| (f.label.clone(), why))).collect()
    }

    fn nothing_found(found: &[Found]) -> PetCommandOutcome {
        let missing: Vec<String> = Self::problems(found).iter().map(|(l, why)| format!("{l}: {why}")).collect();
        let mut o = failure(format!("No window could be found: {}", missing.join("; ")));
        o.details = details(json!({"missing": missing}));
        o
    }

    fn plan(&self, moves: Vec<PetWindowMove>, problems: Vec<(String, String)>, what: String, tight: usize) -> PetWindowPlan {
        let missing = problems.iter().map(|(l, why)| format!("{l}: {why}")).collect();
        PetWindowPlan { moves, missing, problems, what, tight, screens: self.full.clone() }
    }

    fn moves(cells: impl IntoIterator<Item = (Rect, &'a PetWindowInfo, usize, String)>) -> Vec<PetWindowMove> {
        cells.into_iter().map(|(cell, w, item, label)| PetWindowMove { id: w.id, label, before: w.frame, requested: cell, note: None, item }).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn tile(&self, refs: Option<&Vec<WindowSelector>>, rows: Option<usize>, cols: Option<usize>, numbers: Option<&Vec<i64>>,
            span: WindowSpan, fill: WindowFill, gap: f64) -> Result<PetWindowPlan, PetCommandOutcome> {
        let found = refs.map_or_else(|| self.all_open_windows(), |r| self.resolve_all(r));
        let usable: Vec<&Found> = found.iter().filter(|f| f.window.is_some()).collect();
        let Some(first) = usable.first() else {
            return Err(if refs.is_none() { failure("There are no open windows to tile.") } else { Self::nothing_found(&found) });
        };
        let mut areas = self.visible_frames(numbers.map_or(&[][..], |n| n.as_slice()))?;
        if areas.is_empty() { areas.push(self.usable_frame(first.frame())); }
        let cells = WindowTiling::tile(usable.len(), rows, cols, fill, span, &areas, gap).map_err(failure)?;
        let moves = Self::moves(usable.iter().zip(&cells).filter_map(|(f, c)| f.window.map(|w| (*c, w, f.index, f.label.clone()))));
        let what = if refs.is_none() { "All windows tiled" } else { "Tile" };
        Ok(self.plan(moves, Self::problems(&found), what.into(), WindowTiling::tight_cells(&cells)))
    }

    fn place(&self, items: &[WindowPlacementItem], unit: WindowCoordinateUnit) -> Result<PetWindowPlan, PetCommandOutcome> {
        let refs: Vec<WindowSelector> = items.iter().map(|i| i.target.clone()).collect();
        let found = self.resolve_all(&refs);
        if found.iter().all(|f| f.window.is_none()) { return Err(Self::nothing_found(&found)); }
        let (mut moves, mut kept, mut problems) = (vec![], vec![], vec![]);
        for f in &found {
            let Some(w) = f.window else { problems.push((f.label.clone(), f.problem.clone().unwrap_or_default())); continue };
            let item = &items[f.index];
            let area = match item.display {
                Some(n) => match self.visible_frames(&[n]) {
                    Ok(list) => list[0],
                    Err(o) => { problems.push((f.label.clone(), o.message)); continue; }
                },
                None => self.usable_frame(w.frame),
            };
            let (frame, clamped) = WindowPlacement::frame(item, unit, area);
            moves.push(PetWindowMove { id: w.id, label: f.label.clone(), before: w.frame, requested: frame,
                                       note: clamped.then(|| "clamped to the display".to_string()), item: f.index });
            kept.push(item.clone());
        }
        if moves.is_empty() {
            let why: Vec<String> = problems.iter().map(|(l, why)| format!("{l}: {why}")).collect();
            let mut o = failure(format!("No window could be placed: {}", why.join("; ")));
            o.details = details(json!({"missing": why}));
            return Err(o);
        }
        // Applied in z order: the shell brings each moved window to the front, so the highest z ends on top.
        let moves = WindowPlacement::raise_order(&kept).into_iter().map(|i| moves[i].clone()).collect();
        Ok(self.plan(moves, problems, "Place".into(), 0))
    }

    fn fit_one_display(&self, req: &PetWindowToolRequest, refs: &[WindowSelector], display: Option<i64>, gap: f64) -> Result<PetWindowPlan, PetCommandOutcome> {
        let found = self.resolve_all(refs);
        let Some(first) = found.iter().find(|f| f.window.is_some()) else { return Err(Self::nothing_found(&found)) };
        let visible = match display {
            Some(n) => self.visible_frames(&[n])?[0],
            None => self.usable_frame(first.frame()),
        };
        let cells = req.cells(visible, gap).ok_or_else(|| failure("This request does not place windows."))?.map_err(failure)?;
        let moves = Self::moves(found.iter().filter_map(|f| Some((*cells.get(f.index)?, f.window?, f.index, f.label.clone()))));
        Ok(self.plan(moves, Self::problems(&found), "Placement".into(), WindowTiling::tight_cells(&cells)))
    }

    fn move_to_display(&self, refs: &[WindowSelector], display: i64) -> Result<PetWindowPlan, PetCommandOutcome> {
        let destination = self.visible_frames(&[display])?[0];
        let found = self.resolve_all(refs);
        let moves = Self::moves(found.iter().filter_map(|f| {
            let w = f.window?;
            Some((layout::move_to(w.frame, self.usable_frame(w.frame), destination), w, f.index, f.label.clone()))
        }));
        if moves.is_empty() { return Err(Self::nothing_found(&found)); }
        Ok(self.plan(moves, Self::problems(&found), format!("Move to display {display}"), 0))
    }
}

impl PetWindowTools {
    /// The tool actions (`window_<action>`, Nacho: `combo_window_<action>`).
    pub const ACTIONS: [&'static str; 8] = ["displays", "place", "tile", "split", "ratio", "move_display", "arrange_apps", "undo"];

    /// Checks the arguments of one window tool against closed lists and ranges (the macOS schemas); unknown fields
    /// are refused.
    pub fn parse(action: &str, params: Option<&Value>) -> Result<PetWindowToolRequest, PetCommandProblem> {
        let empty = JsonObject::new();
        let a = match params {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(o)) => o,
            Some(_) => return Err(bad("parameters must be an object")),
        };
        let max_display = WindowTiling::MAX_DISPLAYS as i64;
        let display = |a: &JsonObject| int(a.get("display"), "display", 1, max_display);
        match action {
            "displays" => {
                only(a, &[])?;
                Ok(PetWindowToolRequest::Displays)
            }
            "tile" => {
                only(a, &["windows", "all_windows", "rows", "cols", "display", "displays", "span", "fill", "gap"])?;
                let all = match a.get("all_windows") {
                    None => false,
                    Some(Value::Bool(b)) => *b,
                    Some(_) => return Err(bad("all_windows must be true or false")),
                };
                if all && a.contains_key("windows") { return Err(bad("give windows or all_windows, not both")); }
                if !all && !a.contains_key("windows") { return Err(bad("windows (a list) or all_windows: true is required")); }
                if a.contains_key("display") && a.contains_key("displays") { return Err(bad("give display or displays, not both")); }
                let displays = match display_list(a.get("displays"))? { Some(list) => Some(list), None => display(a)?.map(|d| vec![d]) };
                let max_rc = WindowTiling::MAX_ROWS_OR_COLUMNS as i64;
                Ok(PetWindowToolRequest::Tile {
                    windows: if all { None } else { Some(selectors(a.get("windows"), WindowTiling::MAX_WINDOWS)?) },
                    rows: int(a.get("rows"), "rows", 1, max_rc)?.map(|n| n as usize),
                    cols: int(a.get("cols"), "cols", 1, max_rc)?.map(|n| n as usize),
                    displays,
                    span: choice(a.get("span"), "span")?.unwrap_or(WindowSpan::PerDisplay),
                    fill: choice(a.get("fill"), "fill")?.unwrap_or(WindowFill::RowMajor),
                    gap: int(a.get("gap"), "gap", 0, 100)?.map(|g| g as f64),
                })
            }
            "place" => {
                only(a, &["items", "unit"])?;
                let max = WindowTiling::MAX_WINDOWS;
                let list = a.get("items").and_then(Value::as_array).filter(|l| !l.is_empty() && l.len() <= max)
                    .ok_or_else(|| bad(format!("items must list 1 to {max} windows")))?;
                let unit = choice(a.get("unit"), "unit")?.unwrap_or(WindowCoordinateUnit::Ratio);
                let limit = if unit == WindowCoordinateUnit::Ratio { 10.0 } else { 100_000.0 };
                let mut seen = HashSet::new();
                let items = list.iter().map(|raw| {
                    let Value::Object(o) = raw else { return Err(bad("each item must be an object {app, x, y, w, h}")) };
                    only(o, &["app", "window", "id", "display", "x", "y", "w", "h", "z"])?;
                    let target = selector(o)?;
                    if !seen.insert(target.id().to_lowercase()) { return Err(bad(format!("the same window is listed twice: {}", target.id()))); }
                    let w = number(o.get("w"), "w", 0.0, limit)?;
                    let h = number(o.get("h"), "h", 0.0, limit)?;
                    if w <= 0.0 || h <= 0.0 { return Err(bad("w and h must be above 0")); }
                    Ok(WindowPlacementItem { target, display: display(o)?, x: number(o.get("x"), "x", -limit, limit)?,
                                             y: number(o.get("y"), "y", -limit, limit)?, w, h, z: int(o.get("z"), "z", -1000, 1000)? })
                }).collect::<Result<Vec<_>, _>>()?;
                Ok(PetWindowToolRequest::Place { items, unit })
            }
            "split" => {
                only(a, &["parts", "direction", "windows", "display"])?;
                let parts = int(a.get("parts"), "parts", 2, 4)?.ok_or_else(|| bad("parts is required (2, 3 or 4)"))? as usize;
                let direction = choice(a.get("direction"), "direction")?.ok_or_else(|| bad("direction is required"))?;
                if direction == WindowSplitDirection::Quadrant && parts != 4 { return Err(bad("quadrant needs parts = 4")); }
                let windows = selectors(a.get("windows"), parts)?;
                Ok(PetWindowToolRequest::Split { parts, direction, windows, display: display(a)? })
            }
            "ratio" => {
                only(a, &["ratios", "direction", "windows", "display"])?;
                let max = WindowTiling::MAX_RATIO_PARTS;
                let raw = a.get("ratios").and_then(Value::as_array).filter(|l| (2..=max).contains(&l.len()))
                    .ok_or_else(|| bad(format!("ratios must list 2 to {max} whole numbers, e.g. [70, 30]")))?;
                let mut weights = vec![];
                for v in raw { weights.extend(int(Some(v), "each ratio", 1, WindowTiling::MAX_RATIO_WEIGHT as i64)?); }
                let direction = choice(a.get("direction"), "direction")?.unwrap_or(WindowSplitDirection::Horizontal);
                if direction == WindowSplitDirection::Quadrant { return Err(bad("direction must be horizontal or vertical for ratios")); }
                let windows = selectors(a.get("windows"), weights.len())?;
                Ok(PetWindowToolRequest::Ratio { weights, direction, windows, display: display(a)? })
            }
            "move_display" => {
                only(a, &["windows", "display"])?;
                let display = display(a)?.ok_or_else(|| bad("display is required"))?;
                Ok(PetWindowToolRequest::MoveDisplay { windows: selectors(a.get("windows"), WindowTiling::MAX_WINDOWS)?, display })
            }
            "arrange_apps" => {
                only(a, &["apps", "arrangement", "display"])?;
                let max = WindowTiling::MAX_APPS;
                let raw = a.get("apps").and_then(Value::as_array).filter(|l| (1..=max).contains(&l.len()))
                    .ok_or_else(|| bad(format!("apps must list 1 to {max} app names")))?;
                let mut apps: Vec<String> = vec![];
                for item in raw {
                    let name_max = PetCommands::MAX_APP_NAME_LENGTH;
                    let Some(name) = item.as_str().map(str::trim).filter(|s| !s.is_empty() && s.chars().count() <= name_max) else {
                        return Err(bad(format!("each app must be a name of 1 to {name_max} characters")));
                    };
                    if apps.iter().any(|a| a.to_lowercase() == name.to_lowercase()) { return Err(bad(format!("the same app is listed twice: {name}"))); }
                    apps.push(name.to_string());
                }
                let arrangement = choice(a.get("arrangement"), "arrangement")?.unwrap_or(WindowArrangement::Grid);
                if matches!(arrangement, WindowArrangement::Columns | WindowArrangement::Rows) && apps.len() < 2 {
                    return Err(bad(format!("{} needs at least 2 apps", arrangement.raw())));
                }
                if arrangement == WindowArrangement::Quadrants && apps.len() > 4 { return Err(bad("quadrants takes at most 4 apps")); }
                Ok(PetWindowToolRequest::ArrangeApps { apps, arrangement, display: display(a)? })
            }
            "undo" => {
                only(a, &["steps"])?;
                let steps = int(a.get("steps"), "steps", 1, WindowTiling::UNDO_STEPS as i64)?.unwrap_or(1);
                Ok(PetWindowToolRequest::Undo { steps: steps as usize })
            }
            other => Err(bad(format!("unknown window action {}", other.chars().take(64).collect::<String>()))),
        }
    }

    /// `window_displays`: number from the left, name, main (the display Windows lists first), the full frame in
    /// screen coordinates (the main display's top-left corner is 0,0) and the usable area from the display's own
    /// top-left corner (the coordinates `window_place` uses).
    pub fn displays(displays: &[PetDisplayInfo]) -> PetCommandOutcome {
        let full: Vec<Rect> = displays.iter().map(|d| d.frame).collect();
        let list: Vec<Value> = PetDisplays::ordered(&full).into_iter().enumerate().map(|(n, i)| {
            let d = &displays[i];
            json!({"display": n + 1, "name": d.name, "main": i == 0,
                   "x": d.frame.x.round() as i64, "y": d.frame.y.round() as i64,
                   "width": d.frame.w.round() as i64, "height": d.frame.h.round() as i64,
                   "usable": report(d.work, &[d.frame])})
        }).collect();
        let mut o = PetCommandOutcome::new(true, format!("{} display{}.", list.len(), plural(list.len())));
        o.details = details(json!({"displays": list}));
        o
    }

    /// Where every window goes. `windows` are the shell's windows front to back (own windows excluded), `displays`
    /// in the order Windows lists them, `gap` the app's snap gap (used when the call gives none). Err = the answer to
    /// give instead (nothing found, no such display, a grid that does not fit …). Displays and Undo are not plans.
    pub fn plan(req: &PetWindowToolRequest, windows: &[PetWindowInfo], displays: &[PetDisplayInfo], gap: f64) -> Result<PetWindowPlan, PetCommandOutcome> {
        if displays.is_empty() { return Err(failure("There is no display to place the windows on.")); }
        let desk = Desk::new(windows, displays);
        match req {
            PetWindowToolRequest::Tile { windows: refs, rows, cols, displays: numbers, span, fill, gap: own } =>
                desk.tile(refs.as_ref(), *rows, *cols, numbers.as_ref(), *span, *fill, own.unwrap_or(gap)),
            PetWindowToolRequest::Place { items, unit } => desk.place(items, *unit),
            PetWindowToolRequest::Split { windows: refs, display, .. } | PetWindowToolRequest::Ratio { windows: refs, display, .. } =>
                desk.fit_one_display(req, refs, *display, gap),
            PetWindowToolRequest::ArrangeApps { apps, display, .. } => {
                let refs: Vec<WindowSelector> = apps.iter().map(|a| WindowSelector::new(a, 1)).collect();
                desk.fit_one_display(req, &refs, *display, gap)
            }
            PetWindowToolRequest::MoveDisplay { windows: refs, display } => desk.move_to_display(refs, *display),
            PetWindowToolRequest::Displays | PetWindowToolRequest::Undo { .. } => Err(failure("This request does not place windows.")),
        }
    }

    /// The answer after the shell applied a plan. `applied[i]` is the frame read back after moving `plan.moves[i]`;
    /// None = Windows refused the move (an app running as administrator).
    pub fn outcome(plan: &PetWindowPlan, applied: &[Option<Rect>]) -> PetCommandOutcome {
        let (mut placed, mut different, mut elevated) = (0, 0, 0);
        let mut rows: Vec<(usize, Value)> = vec![];
        for (i, m) in plan.moves.iter().enumerate() {
            let mut row = json!({"window": m.label, "requested": report(m.requested, &plan.screens)});
            match applied.get(i).copied().flatten() {
                None => {
                    row["placed"] = json!(false);
                    row["reason"] = json!("elevated");
                    elevated += 1;
                }
                Some(landed) => {
                    placed += 1;
                    row["applied"] = report(landed, &plan.screens);
                    row["placed"] = json!(true);
                    let mut reasons: Vec<&str> = m.note.as_deref().into_iter().collect();
                    if !layout::matches(&landed, &m.requested) {
                        reasons.push("the app keeps its own minimum or fixed size");
                        different += 1;
                    }
                    row["reason"] = json!(if reasons.is_empty() { "ok".to_string() } else { reasons.join("; ") });
                }
            }
            rows.push((m.item, row));
        }
        rows.sort_by_key(|r| r.0);
        let mut rows: Vec<Value> = rows.into_iter().map(|r| r.1).collect();
        for (label, why) in &plan.problems { rows.push(json!({"window": label, "placed": false, "reason": format!("not found: {why}")})); }
        let total = plan.moves.len() + plan.problems.len();
        let mut text = format!("{}: {placed} of {total} window{} placed", plan.what, plural(total));
        if different > 0 { text += &format!("; {different} differ from the request (see reason)"); }
        if elevated + plan.problems.len() > 0 { text += &format!("; {} not placed", elevated + plan.problems.len()); }
        if plan.tight > 0 {
            text += &format!("; {} cell{} under {} pixels on a side, apps may refuse that size", plan.tight,
                             if plan.tight == 1 { " is" } else { "s are" }, WindowTiling::COMFORTABLE_SIDE as i64);
        }
        text += if placed > 0 { ". window_undo restores them." } else { "." };
        if elevated > 0 { text += &format!(" {elevated} window{} could not be moved: {}", plural(elevated), PetPermissionHelp::ELEVATED_WINDOWS); }
        let mut o = PetCommandOutcome::new(placed > 0, text);
        o.details = details(json!({"windows": rows, "missing": plan.missing}));
        o
    }

    /// What `window_undo` should remember of an applied plan (push it on the undo stack): every window that moved,
    /// with its frame from before.
    pub fn undo_snapshot(plan: &PetWindowPlan, applied: &[Option<Rect>]) -> PetWindowSnapshot {
        plan.moves.iter().enumerate().filter(|(i, _)| applied.get(*i).copied().flatten().is_some())
            .map(|(_, m)| (m.id, m.label.clone(), m.before)).collect()
    }

    /// The moves of `window_undo` for the snapshots popped from the stack (newest first: apply in this order so the
    /// oldest frame wins). Closed or minimized windows go to `missing`.
    pub fn undo_plan(snapshots: &[PetWindowSnapshot], windows: &[PetWindowInfo], displays: &[PetDisplayInfo]) -> PetWindowPlan {
        let mut plan = PetWindowPlan { what: "Undo".into(), screens: displays.iter().map(|d| d.frame).collect(), ..Default::default() };
        for (id, label, frame) in snapshots.iter().flatten() {
            match windows.iter().find(|w| w.id == *id) {
                None => plan.missing.push(format!("{label}: window is gone")),
                Some(w) if w.minimized => plan.missing.push(format!("{label}: the window is minimized")),
                Some(w) => plan.moves.push(PetWindowMove { id: *id, label: label.clone(), before: w.frame, requested: *frame, note: None, item: plan.moves.len() }),
            }
        }
        plan
    }

    /// The answer of `window_undo`. `restored` = (label, frame read back) of every window the shell moved back (None =
    /// Windows refused), `missing` = windows that could not be restored (`undo_plan`'s), `placements` = how many
    /// snapshots were popped (0 = nothing to undo), `displays` to report frames from their display's corner.
    pub fn undo_outcome(restored: &[(String, Option<Rect>)], missing: &[String], placements: usize, displays: &[PetDisplayInfo]) -> PetCommandOutcome {
        if placements == 0 { return failure("Nothing to undo: no placement by these window tools is remembered."); }
        let screens: Vec<Rect> = displays.iter().map(|d| d.frame).collect();
        let mut missing = missing.to_vec();
        let mut rows = vec![];
        let mut elevated = 0;
        for (label, applied) in restored {
            match applied {
                Some(frame) => rows.push(json!({"window": label, "applied": report(*frame, &screens)})),
                None => { missing.push(format!("{label}: elevated")); elevated += 1; }
            }
        }
        let mut text = format!("Restored {} window{} from {placements} placement{}", rows.len(), plural(rows.len()), plural(placements));
        text += &if missing.is_empty() { ".".to_string() } else { format!("; {} could not be restored.", missing.len()) };
        if elevated > 0 { text += &format!(" {elevated} window{} could not be moved: {}", plural(elevated), PetPermissionHelp::ELEVATED_WINDOWS); }
        let mut o = PetCommandOutcome::new(!rows.is_empty(), text);
        o.details = details(json!({"restored": rows, "missing": missing}));
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The macOS tests' screen (1200 x 775 usable, 25 pixels in from the top here).
    const SCREEN: Rect = Rect::new(0.0, 25.0, 1200.0, 775.0);

    fn ok(r: Result<Vec<Rect>, String>) -> Vec<Rect> { r.unwrap_or_else(|e| panic!("unexpected {e}")) }

    /// No overlap, nothing outside `area`, and (without gap) the cells fill the area.
    fn assert_tiles(c: &[Rect], area: Rect, filled: bool) {
        for (i, a) in c.iter().enumerate() {
            assert!(encloses(&area, a), "cell {i} {a:?} outside {area:?}");
            for b in &c[i + 1..] { assert_eq!(a.overlap_area(b), 0.0, "{a:?} overlaps {b:?}"); }
        }
        if filled { assert_eq!(c.iter().map(|r| r.w * r.h).sum::<f64>(), area.w * area.h); }
    }

    fn widths(c: &[Rect]) -> Vec<f64> { c.iter().map(|r| r.w).collect() }

    fn set(values: impl IntoIterator<Item = f64>) -> Vec<f64> {
        let mut v: Vec<f64> = values.into_iter().collect();
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    }

    // MARK: tiling (macOS WindowTilingTests)

    #[test]
    fn grid_shape_for_one_to_nine() {
        let shapes: Vec<Vec<usize>> = (1..=9).map(WindowTiling::grid_rows).collect();
        assert_eq!(shapes, vec![vec![1], vec![2], vec![2, 1], vec![2, 2], vec![3, 2], vec![3, 3], vec![3, 3, 1], vec![3, 3, 2], vec![3, 3, 3]]);
        assert!(WindowTiling::grid_rows(0).is_empty());
    }

    #[test]
    fn grid_frames_for_one_to_nine_tile_the_screen_exactly() {
        for n in 1..=9 {
            let c = ok(WindowTiling::grid(n, SCREEN, 0.0));
            assert_eq!(c.len(), n, "n={n}");
            assert_tiles(&c, SCREEN, true);
        }
    }

    #[test]
    fn grid_reading_order_and_specific_frames() {
        let two = ok(WindowTiling::grid(2, SCREEN, 0.0));
        assert_eq!(two, vec![Rect::new(0.0, 25.0, 600.0, 775.0), Rect::new(600.0, 25.0, 600.0, 775.0)]);
        let four = ok(WindowTiling::grid(4, SCREEN, 0.0));
        assert_eq!((four[0].x, four[0].y), (0.0, SCREEN.y), "first cell is top-left (row 0 is the top row)");
        assert_eq!((four[3].right(), four[3].bottom()), (SCREEN.right(), SCREEN.bottom()), "last cell is bottom-right");
        let three = ok(WindowTiling::grid(3, SCREEN, 0.0));
        assert_eq!(three[2].w, SCREEN.w, "a short last row stretches across the display");
        assert_eq!(three[2].bottom(), SCREEN.bottom(), "and it is the bottom row");
        assert_eq!(three[0].w, 600.0);
    }

    #[test]
    fn grid_odd_sizes_give_no_gap_and_no_overlap() {
        let odd = Rect::new(0.0, 0.0, 1001.0, 703.0);
        for n in 1..=9 { assert_tiles(&ok(WindowTiling::grid(n, odd, 0.0)), odd, true); }
    }

    #[test]
    fn grid_refuses_zero_and_too_many_but_has_no_layout_limit() {
        assert_eq!(WindowTiling::grid(0, SCREEN, 0.0), Err("A grid needs 1 to 200 windows.".to_string()));
        assert!(WindowTiling::grid(201, SCREEN, 0.0).is_err(), "201 windows is over the per-call technical limit");
        let twenty = ok(WindowTiling::grid(20, SCREEN, 0.0));
        assert_eq!(twenty.len(), 20);
        assert_tiles(&twenty, SCREEN, true);
        assert_eq!(ok(WindowTiling::grid(200, SCREEN, 0.0)).len(), 200);
        let tiny = ok(WindowTiling::grid(9, Rect::new(0.0, 0.0, 320.0, 300.0), 0.0));
        assert_eq!(tiny.len(), 9);
        assert_eq!(WindowTiling::tight_cells(&tiny), 9);
        assert_eq!(WindowTiling::tight_cells(&ok(WindowTiling::grid(4, SCREEN, 0.0))), 0);
    }

    #[test]
    fn rows_by_cols_ten_by_two() {
        let c = ok(WindowTiling::grid_shaped(20, Some(10), Some(2), WindowFill::RowMajor, SCREEN, 0.0));
        assert_eq!(c.len(), 20);
        assert_tiles(&c, SCREEN, true);
        assert_eq!(set(widths(&c)), vec![600.0]);
        assert_eq!((c[0].y, c[0].x), (SCREEN.y, 0.0));
        assert_eq!(c[1].x, 600.0, "row major: second window is right of the first");
        assert_eq!(c[2].y, c[0].bottom(), "third window starts the next row, below");
        assert_eq!((c[19].right(), c[19].bottom()), (SCREEN.right(), SCREEN.bottom()));
        let by_column = ok(WindowTiling::grid_shaped(20, Some(10), Some(2), WindowFill::ColumnMajor, SCREEN, 0.0));
        assert_eq!(by_column[1].y, by_column[0].bottom(), "column major: second window is below the first");
        assert_eq!(by_column[10].x, 600.0, "column major: the eleventh window starts the second column");
        let key = |c: &[Rect]| { let mut v: Vec<String> = c.iter().map(|r| format!("{r:?}")).collect(); v.sort(); v };
        assert_eq!(key(&by_column), key(&c), "same cells, different order");
    }

    #[test]
    fn rows_or_cols_alone() {
        let seven = ok(WindowTiling::grid_shaped(7, Some(2), None, WindowFill::RowMajor, SCREEN, 0.0));
        assert_eq!(seven.len(), 7);
        assert_eq!(set(widths(&seven)), vec![300.0], "2 x 4");
        let six = ok(WindowTiling::grid_shaped(6, None, Some(3), WindowFill::RowMajor, SCREEN, 0.0));
        assert_eq!(set(six.iter().map(|r| r.h)), vec![387.0, 388.0]);
        assert_eq!(six[3].h, 388.0, "the odd pixel goes to the bottom row");
        assert_eq!(WindowTiling::grid_shaped(7, Some(2), Some(3), WindowFill::RowMajor, SCREEN, 0.0),
                   Err("2 x 3 has 6 cells for 7 windows; use more rows or columns.".to_string()));
        assert!(WindowTiling::grid_shaped(2, Some(101), Some(1), WindowFill::RowMajor, SCREEN, 0.0).is_err(), "rows above 100");
        assert_eq!(ok(WindowTiling::grid_shaped(3, Some(5), Some(5), WindowFill::RowMajor, SCREEN, 0.0)).len(), 3, "unused cells stay empty");
        // Column major with no shape: the automatic square, filled down the columns.
        let auto = ok(WindowTiling::grid_shaped(4, None, None, WindowFill::ColumnMajor, SCREEN, 0.0));
        assert_eq!((auto[1].x, auto[1].y), (0.0, auto[0].bottom()));
    }

    #[test]
    fn two_displays_per_display_and_across() {
        let left = Rect::new(-1920.0, -180.0, 1920.0, 1040.0);
        let right = Rect::new(0.0, 0.0, 1440.0, 860.0);
        let per = ok(WindowTiling::tile(20, None, None, WindowFill::RowMajor, WindowSpan::PerDisplay, &[left, right], 0.0));
        assert_eq!(per.len(), 20);
        assert_tiles(&per[..10], left, true);
        assert_tiles(&per[10..], right, true);
        let full = ok(WindowTiling::tile(40, Some(10), Some(2), WindowFill::RowMajor, WindowSpan::PerDisplay, &[left, right], 0.0));
        assert_eq!(full.len(), 40);
        assert!(full[..20].iter().all(|c| encloses(&left, c)) && full[20..].iter().all(|c| encloses(&right, c)));
        assert_eq!(WindowTiling::tile(41, Some(10), Some(2), WindowFill::RowMajor, WindowSpan::PerDisplay, &[left, right], 0.0),
                   Err("10 x 2 on 2 displays has 40 cells for 41 windows.".to_string()));
        let both = union(left, right);
        let across = ok(WindowTiling::tile(20, Some(10), Some(2), WindowFill::RowMajor, WindowSpan::Across, &[left, right], 0.0));
        assert_eq!(across.len(), 20);
        assert_tiles(&across, both, true);
        let auto = ok(WindowTiling::tile(20, None, None, WindowFill::RowMajor, WindowSpan::Across, &[left, right], 0.0));
        assert_eq!(auto.len(), 20);
        assert_tiles(&auto, both, false);
        assert_eq!(set(auto.iter().map(|c| c.x)).len() % 2, 0, "columns come in pairs, one half per display");
        assert!(WindowTiling::tile(3, None, None, WindowFill::RowMajor, WindowSpan::PerDisplay, &[], 0.0).is_err(), "no display");
        // Uneven share: 5 windows on 2 displays → 3 and 2.
        let five = ok(WindowTiling::tile(5, None, None, WindowFill::RowMajor, WindowSpan::PerDisplay, &[left, right], 0.0));
        assert_eq!(five.iter().filter(|c| encloses(&left, c)).count(), 3);
    }

    #[test]
    fn splits_in_every_direction() {
        for parts in 2..=4 {
            let columns = ok(WindowTiling::split(parts, WindowSplitDirection::Horizontal, SCREEN, 0.0));
            assert_eq!(columns.len(), parts);
            assert_tiles(&columns, SCREEN, true);
            assert!(columns.windows(2).all(|p| p[0].right() == p[1].x && p[0].x < p[1].x), "left to right");
            assert!(columns.iter().all(|c| c.h == SCREEN.h));
            let rows = ok(WindowTiling::split(parts, WindowSplitDirection::Vertical, SCREEN, 0.0));
            assert_eq!(rows.len(), parts);
            assert_tiles(&rows, SCREEN, true);
            assert!(rows.windows(2).all(|p| p[0].bottom() == p[1].y), "top to bottom");
            assert_eq!(rows[0].y, SCREEN.y);
            assert!(rows.iter().all(|c| c.w == SCREEN.w));
        }
        let q = ok(WindowTiling::split(4, WindowSplitDirection::Quadrant, SCREEN, 0.0));
        assert_tiles(&q, SCREEN, true);
        assert_eq!(q[0], Rect::new(0.0, 25.0, 600.0, 387.0));
        assert_eq!(q[3], Rect::new(600.0, 412.0, 600.0, 388.0), "the odd pixel goes to the bottom row");
        assert_eq!(q[1].x, q[0].right());
        assert_eq!(q[2].y, q[0].bottom());
        assert_eq!(widths(&ok(WindowTiling::split(3, WindowSplitDirection::Horizontal, SCREEN, 0.0))), vec![400.0; 3]);
    }

    #[test]
    fn split_refuses_bad_parts() {
        for bad in [0, 1, 5] { assert!(WindowTiling::split(bad, WindowSplitDirection::Horizontal, SCREEN, 0.0).is_err(), "parts {bad}"); }
        for parts in [2, 3] { assert!(WindowTiling::split(parts, WindowSplitDirection::Quadrant, SCREEN, 0.0).is_err(), "quadrant of {parts}"); }
    }

    #[test]
    fn ratio_sum_and_minimum_size() {
        let c = ok(WindowTiling::ratio(&[70, 30], WindowSplitDirection::Horizontal, SCREEN, 0.0));
        assert_eq!(widths(&c), vec![840.0, 360.0]);
        assert_tiles(&c, SCREEN, true);
        let rows = ok(WindowTiling::ratio(&[1, 2, 1], WindowSplitDirection::Vertical, SCREEN, 0.0));
        assert_eq!(rows.iter().map(|r| r.h).sum::<f64>(), SCREEN.h);
        assert_eq!(rows[0].y, SCREEN.y, "the first share is the top one");
        assert!(rows[1].h > rows[0].h);
        assert_tiles(&rows, SCREEN, true);
        let odd_area = Rect::new(0.0, 0.0, 1001.0, 600.0);
        assert_tiles(&ok(WindowTiling::ratio(&[33, 33, 34], WindowSplitDirection::Horizontal, odd_area, 0.0)), odd_area, true);
        assert_eq!(WindowTiling::tight_cells(&ok(WindowTiling::ratio(&[5, 95], WindowSplitDirection::Horizontal, SCREEN, 0.0))), 1);
        for bad in [vec![100], vec![10, 20, 30, 40, 50], vec![0, 50], vec![50, 101], vec![-1, 5]] {
            assert!(WindowTiling::ratio(&bad, WindowSplitDirection::Horizontal, SCREEN, 0.0).is_err(), "{bad:?}");
        }
        assert!(WindowTiling::ratio(&[1, 1], WindowSplitDirection::Quadrant, SCREEN, 0.0).is_err(), "quadrant ratio");
    }

    #[test]
    fn gaps_leave_space_between_and_at_edges() {
        let c = ok(WindowTiling::split(2, WindowSplitDirection::Horizontal, SCREEN, 10.0));
        assert_eq!(c[0].x, 10.0);
        assert_eq!(c[1].right(), SCREEN.right() - 10.0);
        assert_eq!(c[1].x - c[0].right(), 10.0, "neighbours one gap apart");
        assert_eq!(c[0].y, SCREEN.y + 10.0);
        assert_eq!(c[0].bottom(), SCREEN.bottom() - 10.0);
        let rows = ok(WindowTiling::ratio(&[1, 1], WindowSplitDirection::Vertical, SCREEN, 10.0));
        assert_eq!(rows[1].y - rows[0].bottom(), 10.0);
    }

    #[test]
    fn multi_display_coordinates() {
        // Display index 1 sits left of the main one (negative x), index 2 is above it; numbering is from the left.
        let full = [Rect::new(0.0, 0.0, 1440.0, 900.0), Rect::new(-1920.0, -180.0, 1920.0, 1080.0), Rect::new(100.0, -720.0, 1280.0, 720.0)];
        let visible = [Rect::new(0.0, 0.0, 1440.0, 860.0), Rect::new(-1920.0, -180.0, 1920.0, 1040.0), Rect::new(100.0, -720.0, 1280.0, 680.0)];
        assert_eq!(WindowTiling::visible_frame(1, &full, &visible), Some(visible[1]), "display 1 is the left-most");
        assert_eq!(WindowTiling::visible_frame(2, &full, &visible), Some(visible[0]));
        assert_eq!(WindowTiling::visible_frame(3, &full, &visible), Some(visible[2]));
        assert_eq!(WindowTiling::visible_frame(4, &full, &visible), None);
        assert_eq!(WindowTiling::visible_frame(0, &full, &visible), None);
        assert_eq!(WindowTiling::visible_frame(1, &full, &visible[..2]), None, "lists of different length");
        let left = ok(WindowTiling::grid(4, visible[1], 0.0));
        assert_tiles(&left, visible[1], true);
        assert!(left.iter().all(|c| c.right() <= 0.0), "cells stay on the left display");
        let above = ok(WindowTiling::split(3, WindowSplitDirection::Horizontal, visible[2], 0.0));
        assert_tiles(&above, visible[2], true);
        assert_eq!(above[0].x, 100.0);
        // Moving a window keeps its relative place.
        let moved = layout::move_to(Rect::new(720.0, 437.0, 360.0, 218.0), visible[0], visible[1]);
        assert!(encloses(&visible[1], &moved));
    }

    #[test]
    fn divide_edge_cases() {
        assert!(WindowTiling::divide(100.0, &[]).is_empty());
        assert!(WindowTiling::divide(100.0, &[0, 0]).is_empty());
        let d = WindowTiling::divide(10.0, &[1, 1, 1]);
        assert_eq!(d.iter().map(|p| p.1).sum::<f64>(), 10.0);
        assert_eq!(d.last().map(|p| p.0 + p.1), Some(10.0));
        assert_eq!(d, vec![(0.0, 3.0), (3.0, 3.0), (6.0, 4.0)], "whole-pixel boundaries, the rest in the last piece");
    }

    // MARK: WindowLayoutTests parts the window tools use (gap, display move)

    #[test]
    fn gaps_separate_neighbours_by_one_gap() {
        let half = |x: f64| Rect::new(x, SCREEN.y, 600.0, SCREEN.h);
        let l = layout::apply_gap(half(0.0), SCREEN, 10.0, true);
        let r = layout::apply_gap(half(600.0), SCREEN, 10.0, true);
        assert_eq!(l.x, SCREEN.x + 10.0);
        assert_eq!(r.x - l.right(), 10.0);
        assert_eq!(r.right(), SCREEN.right() - 10.0);
        assert_eq!(l.y, SCREEN.y + 10.0);
    }

    #[test]
    fn move_to_another_display_keeps_proportions() {
        let right = Rect::new(1200.0, 0.0, 2400.0, 1600.0);
        let moved = layout::move_to(Rect::new(0.0, 25.0, 600.0, 775.0), SCREEN, right);
        assert_eq!(moved.x, right.x);
        assert_eq!(moved.w, 1200.0);
        assert_eq!(moved.h, right.h);
    }

    // MARK: free placement (macOS WindowPlacementTests)

    const VISIBLE: Rect = Rect::new(100.0, 50.0, 1000.0, 600.0); // another display: not at the origin

    fn item(x: f64, y: f64, w: f64, h: f64, z: Option<i64>) -> WindowPlacementItem {
        WindowPlacementItem { target: WindowSelector::new("A", 1), display: None, x, y, w, h, z }
    }

    #[test]
    fn ratio_rectangle_uses_top_left_origin() {
        let (frame, clamped) = WindowPlacement::frame(&item(0.25, 0.1, 0.5, 0.4, None), WindowCoordinateUnit::Ratio, VISIBLE);
        assert!(!clamped);
        assert_eq!(frame, Rect::new(350.0, 110.0, 500.0, 240.0), "x from the left, y from the top of the usable area");
    }

    #[test]
    fn pixel_rectangle() {
        let (frame, clamped) = WindowPlacement::frame(&item(10.0, 20.0, 300.0, 200.0, None), WindowCoordinateUnit::Pixel, VISIBLE);
        assert_eq!(frame, Rect::new(110.0, 70.0, 300.0, 200.0));
        assert!(!clamped);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let (past, clamped) = WindowPlacement::frame(&item(0.8, 0.9, 0.5, 0.5, None), WindowCoordinateUnit::Ratio, VISIBLE);
        assert!(clamped);
        assert!(encloses(&VISIBLE, &past), "pushed back inside: {past:?}");
        assert_eq!((past.w, past.h), (500.0, 300.0), "size kept, position moved");
        assert_eq!(past.bottom(), VISIBLE.bottom());
        let (negative, clamped) = WindowPlacement::frame(&item(-0.5, -1.0, 0.2, 0.2, None), WindowCoordinateUnit::Ratio, VISIBLE);
        assert_eq!((negative.x, negative.y), (VISIBLE.x, VISIBLE.y));
        assert!(clamped);
        let (huge, clamped) = WindowPlacement::frame(&item(0.0, 0.0, 5.0, 5.0, None), WindowCoordinateUnit::Ratio, VISIBLE);
        assert_eq!(huge, VISIBLE);
        assert!(clamped);
        let (pixel_huge, clamped) = WindowPlacement::frame(&item(900.0, 500.0, 800.0, 800.0, None), WindowCoordinateUnit::Pixel, VISIBLE);
        assert!(encloses(&VISIBLE, &pixel_huge) && clamped);
    }

    #[test]
    fn overlaps_are_allowed_and_z_order_is_ascending() {
        let a = WindowPlacement::frame(&item(0.3, 0.3, 0.4, 0.4, None), WindowCoordinateUnit::Ratio, VISIBLE).0;
        let b = WindowPlacement::frame(&item(0.4, 0.4, 0.4, 0.4, None), WindowCoordinateUnit::Ratio, VISIBLE).0;
        assert!(a.overlap_area(&b) > 0.0, "overlap is fine");
        let plain = item(0.0, 0.0, 1.0, 1.0, None);
        assert_eq!(WindowPlacement::raise_order(&[plain.clone(), plain.clone(), plain.clone()]), vec![0, 1, 2], "list order by default");
        let z = |n| item(0.0, 0.0, 1.0, 1.0, Some(n));
        assert_eq!(WindowPlacement::raise_order(&[z(9), z(1), z(5)]), vec![1, 2, 0]);
        assert_eq!(WindowPlacement::raise_order(&[z(1), plain.clone()]).len(), 2);
        assert_eq!(WindowPlacement::raise_order(&[z(1), plain]), vec![0, 1], "equal keys keep list order");
    }

    /// The guide's diamond recipe (rows of 1, 2, 3, 2, 1 windows), as `window_place` items rounded to 3 places.
    fn diamond(apps: &[&str]) -> Vec<Value> {
        let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
        let mut items = vec![];
        let mut index = 0;
        for (row, count) in [1usize, 2, 3, 2, 1].into_iter().enumerate() {
            for k in 0..count {
                if index >= apps.len() { break; }
                let cx = 0.5 + (k as f64 - (count - 1) as f64 / 2.0) * 0.22;
                let cy = 0.16 + 0.17 * row as f64;
                items.push(json!({"app": apps[index], "x": r3(cx - 0.1), "y": r3(cy - 0.075), "w": 0.2, "h": 0.15}));
                index += 1;
            }
        }
        items
    }

    #[test]
    fn diamond_recipe_from_the_guide_stays_on_screen_and_keeps_the_shape() {
        let mut frames = vec![];
        for (row, count) in [1usize, 2, 3, 2, 1].into_iter().enumerate() {
            for k in 0..count {
                let cx = 0.5 + (k as f64 - (count - 1) as f64 / 2.0) * 0.22;
                let cy = 0.16 + 0.17 * row as f64;
                let (frame, clamped) = WindowPlacement::frame(&item(cx - 0.1, cy - 0.075, 0.2, 0.15, None), WindowCoordinateUnit::Ratio, VISIBLE);
                assert!(!clamped && encloses(&VISIBLE, &frame));
                frames.push(frame);
            }
        }
        assert_eq!(frames.len(), 9);
        for (i, a) in frames.iter().enumerate() { for b in &frames[i + 1..] { assert_eq!(a.overlap_area(b), 0.0, "diamond cells do not collide"); } }
        assert!((frames[0].cx() - frames[8].cx()).abs() <= 1.0, "tip and tail on the vertical axis");
        assert!((frames[3].x - (VISIBLE.x + 0.28 * VISIBLE.w - 0.1 * VISIBLE.w)).abs() <= 1.0);
        assert!(frames[0].cy() < frames[8].cy(), "y grows downwards: the first is the top tip");
        assert!((frames[1].cx() + frames[2].cx() - 2.0 * frames[0].cx()).abs() <= 2.0, "mirror symmetric");
        let apps = ["Notepad", "Edge", "Terminal", "Explorer", "Slack", "Mail", "Music", "Photos", "Calendar"];
        let Ok(PetWindowToolRequest::Place { items, .. }) = PetWindowTools::parse("place", Some(&json!({"items": diamond(&apps)}))) else {
            panic!("diamond items must parse")
        };
        assert_eq!(items.len(), 9);
        assert!(items.iter().all(|i| !WindowPlacement::frame(i, WindowCoordinateUnit::Ratio, VISIBLE).1));
    }

    #[test]
    fn twenty_two_windows_in_one_call_are_fine() {
        let items: Vec<WindowPlacementItem> = (0..22).map(|n| item((n % 11) as f64 / 11.0, (n / 11) as f64 / 2.0, 1.0 / 11.0, 0.5, None)).collect();
        let frames: Vec<Rect> = items.iter().map(|i| WindowPlacement::frame(i, WindowCoordinateUnit::Ratio, VISIBLE).0).collect();
        assert_eq!(frames.len(), 22);
        assert!(frames.iter().all(|f| encloses(&VISIBLE, f)));
    }

    // MARK: undo (macOS WindowUndoTests)

    fn snap(items: &[(isize, &str, Rect)]) -> PetWindowSnapshot { items.iter().map(|(i, l, r)| (*i, l.to_string(), *r)).collect() }

    #[test]
    fn undo_restores_the_frames_from_before_and_newest_first() {
        let mut stack = PetWindowUndoStack::new(3);
        let (a0, a1) = (Rect::new(0.0, 0.0, 400.0, 300.0), Rect::new(50.0, 50.0, 400.0, 300.0));
        stack.push(snap(&[(1, "A", a0), (2, "B", a1)]));
        stack.push(snap(&[(1, "A", a1)]));
        let one = stack.pop(1);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0], snap(&[(1, "A", a1)]), "the previous placement's frame");
        assert_eq!(stack.len(), 1);
        let rest = stack.pop(5);
        assert_eq!(rest.len(), 1, "asking for more than remembered returns what exists");
        assert_eq!(rest[0].iter().map(|i| i.2).collect::<Vec<_>>(), vec![a0, a1]);
        assert!(stack.pop(1).is_empty() && stack.is_empty());
    }

    #[test]
    fn two_step_undo_applies_oldest_frame_last() {
        let mut stack = PetWindowUndoStack::default();
        stack.push(snap(&[(1, "A", Rect::new(1.0, 1.0, 10.0, 10.0))])); // first placement: before = original
        stack.push(snap(&[(1, "A", Rect::new(2.0, 2.0, 10.0, 10.0))])); // second: before = first result
        let mut current = None;
        for snapshot in stack.pop(2) { for item in snapshot { current = Some(item.2); } }
        assert_eq!(current, Some(Rect::new(1.0, 1.0, 10.0, 10.0)), "back to the original");
    }

    #[test]
    fn capacity_drops_the_oldest_and_empty_snapshots_are_ignored() {
        let mut stack = PetWindowUndoStack::new(2);
        for i in 1..=4 { stack.push(snap(&[(i, "W", Rect::default())])); }
        stack.push(vec![]);
        assert_eq!(stack.len(), 2);
        assert_eq!(stack.pop(2).iter().map(|s| s[0].0).collect::<Vec<_>>(), vec![4, 3]);
        assert_eq!(PetWindowUndoStack::default().capacity, 10);
        assert_eq!(PetWindowUndoStack::new(0).capacity, 1);
    }

    // MARK: arguments (macOS WindowToolArgumentsTests)

    fn parse(action: &str, json: &str) -> Result<PetWindowToolRequest, PetCommandProblem> {
        let v: Value = serde_json::from_str(json).expect("test json");
        PetWindowTools::parse(action, Some(&v))
    }

    #[track_caller]
    fn fails(action: &str, json: &str, part: &str) {
        match parse(action, json) {
            Ok(r) => panic!("{action} {json} was accepted: {r:?}"),
            Err(p) => {
                let m = p.message();
                assert!(m.starts_with("bad parameters: "), "{m}");
                assert!(m.contains(part), "{m} lacks {part}");
            }
        }
    }

    fn sel(app: &str, window: usize) -> WindowSelector { WindowSelector::new(app, window) }

    #[test]
    fn valid_requests() {
        use PetWindowToolRequest as R;
        assert_eq!(parse("displays", "{}"), Ok(R::Displays));
        assert_eq!(PetWindowTools::parse("displays", None), Ok(R::Displays));
        assert_eq!(parse("tile", r#"{"windows":[{"app":"Notepad"},{"app":"Edge","window":2}],"display":2}"#),
                   Ok(R::Tile { windows: Some(vec![sel("Notepad", 1), sel("Edge", 2)]), rows: None, cols: None, displays: Some(vec![2]),
                                span: WindowSpan::PerDisplay, fill: WindowFill::RowMajor, gap: None }));
        assert_eq!(parse("tile", r#"{"all_windows":true,"rows":10,"cols":2,"displays":[1,2],"span":"across","fill":"column_major","gap":8}"#),
                   Ok(R::Tile { windows: None, rows: Some(10), cols: Some(2), displays: Some(vec![1, 2]), span: WindowSpan::Across,
                                fill: WindowFill::ColumnMajor, gap: Some(8.0) }));
        assert_eq!(parse("place", r#"{"items":[{"app":"Notepad","x":0.1,"y":0,"w":0.5,"h":1,"z":3},{"id":"Edge#2","display":2,"x":0,"y":0,"w":1,"h":1}]}"#),
                   Ok(R::Place { items: vec![
                       WindowPlacementItem { target: sel("Notepad", 1), display: None, x: 0.1, y: 0.0, w: 0.5, h: 1.0, z: Some(3) },
                       WindowPlacementItem { target: sel("Edge", 2), display: Some(2), x: 0.0, y: 0.0, w: 1.0, h: 1.0, z: None },
                   ], unit: WindowCoordinateUnit::Ratio }));
        assert_eq!(parse("place", r#"{"unit":"pixel","items":[{"app":"A","x":10,"y":20,"w":300,"h":200}]}"#),
                   Ok(R::Place { items: vec![WindowPlacementItem { target: sel("A", 1), display: None, x: 10.0, y: 20.0, w: 300.0, h: 200.0, z: None }],
                                 unit: WindowCoordinateUnit::Pixel }));
        assert_eq!(parse("split", r#"{"parts":4,"direction":"quadrant","windows":[{"app":"A"}]}"#),
                   Ok(R::Split { parts: 4, direction: WindowSplitDirection::Quadrant, windows: vec![sel("A", 1)], display: None }));
        assert_eq!(parse("ratio", r#"{"ratios":[70,30],"windows":[{"app":"A"},{"app":"B"}]}"#),
                   Ok(R::Ratio { weights: vec![70, 30], direction: WindowSplitDirection::Horizontal, windows: vec![sel("A", 1), sel("B", 1)], display: None }));
        assert_eq!(parse("move_display", r#"{"windows":[{"app":"Slack"}],"display":2}"#), Ok(R::MoveDisplay { windows: vec![sel("Slack", 1)], display: 2 }));
        assert_eq!(parse("arrange_apps", r#"{"apps":["Notepad"," Edge "],"arrangement":"columns"}"#),
                   Ok(R::ArrangeApps { apps: vec!["Notepad".into(), "Edge".into()], arrangement: WindowArrangement::Columns, display: None }));
        assert_eq!(parse("undo", "{}"), Ok(R::Undo { steps: 1 }));
        assert_eq!(parse("undo", r#"{"steps":3}"#), Ok(R::Undo { steps: 3 }));
        assert_eq!(parse("undo", r#"{"steps":3.0}"#), Ok(R::Undo { steps: 3 }), "a whole JSON number may be written 3.0");
        // Korean names and exe ids with a window number.
        assert_eq!(parse("move_display", r#"{"windows":[{"id":"메모장#2"},{"id":"notepad.exe#1"}],"display":1}"#),
                   Ok(R::MoveDisplay { windows: vec![sel("메모장", 2), sel("notepad.exe", 1)], display: 1 }));
    }

    #[test]
    fn no_layout_limit_on_the_number_of_windows() {
        let twenty: Vec<Value> = (1..=20).map(|n| json!({"app": format!("App{n}"), "x": 0, "y": 0, "w": 0.1, "h": 0.1})).collect();
        let Ok(PetWindowToolRequest::Place { items, .. }) = PetWindowTools::parse("place", Some(&json!({"items": twenty}))) else { panic!("20 items") };
        assert_eq!(items.len(), 20);
        let mut many: Vec<Value> = (1..=200).map(|n| json!({"app": format!("App{n}")})).collect();
        let Ok(PetWindowToolRequest::Tile { windows: Some(list), .. }) = PetWindowTools::parse("tile", Some(&json!({"windows": many}))) else { panic!("200 windows") };
        assert_eq!(list.len(), 200);
        many.push(json!({"app": "One more"}));
        let err = PetWindowTools::parse("tile", Some(&json!({"windows": many}))).unwrap_err();
        assert!(err.message().contains("1 to 200"), "{}", err.message());
        assert_eq!(PetCommands::MAX_LAYOUT_ITEMS, 200);
    }

    #[test]
    fn closed_lists_ranges_and_unknown_fields_are_refused() {
        fails("displays", r#"{"x":1}"#, "unknown field x");
        fails("tile", "{}", "all_windows");
        fails("tile", r#"{"windows":[]}"#, "1 to 200");
        fails("tile", r#"{"windows":[{"app":"A"}],"all_windows":true}"#, "not both");
        fails("tile", r#"{"all_windows":"yes"}"#, "all_windows");
        fails("tile", r#"{"windows":[{"app":"A"}],"extra":1}"#, "unknown field extra");
        fails("tile", r#"{"windows":[{"app":"A"}],"b":1,"a":2}"#, "unknown field a, b");
        fails("tile", r#"{"windows":[{"app":"A","title":"secret"}]}"#, "unknown field title");
        fails("tile", r#"{"windows":[{"app":"A"}],"display":0}"#, "display");
        fails("tile", r#"{"windows":[{"app":"A"}],"display":17}"#, "display");
        fails("tile", r#"{"windows":[{"app":"A"}],"display":"2"}"#, "display");
        fails("tile", r#"{"windows":[{"app":"A"}],"display":1,"displays":[1]}"#, "not both");
        fails("tile", r#"{"windows":[{"app":"A"}],"displays":[1,1]}"#, "twice");
        fails("tile", r#"{"windows":[{"app":"A"}],"displays":[]}"#, "displays");
        fails("tile", r#"{"windows":[{"app":"A"}],"displays":[17]}"#, "each display");
        fails("tile", r#"{"windows":[{"app":"A"}],"rows":0}"#, "rows");
        fails("tile", r#"{"windows":[{"app":"A"}],"cols":101}"#, "cols");
        fails("tile", r#"{"windows":[{"app":"A"}],"rows":2.5}"#, "rows must be a whole number");
        fails("tile", r#"{"windows":[{"app":"A"}],"span":"wrap"}"#, "span must be one of per_display, across");
        fails("tile", r#"{"windows":[{"app":"A"}],"fill":"spiral"}"#, "fill must be one of row_major, column_major");
        fails("tile", r#"{"windows":[{"app":"A"}],"gap":101}"#, "gap");
        fails("tile", r#"{"windows":[{"app":"A","window":21}]}"#, "window");
        fails("tile", r#"{"windows":[{"app":""}]}"#, "app");
        fails("tile", r#"{"windows":[{}]}"#, "app");
        fails("tile", r#"{"windows":["A"]}"#, "each window must be an object");
        fails("tile", r#"{"windows":[{"app":"x"}],"rows":true}"#, "rows");
        fails("tile", &format!(r#"{{"windows":[{{"app":"{}"}}]}}"#, "a".repeat(101)), "1 to 100 characters");
        fails("tile", r#"{"windows":[{"id":"Notepad"}]}"#, "id must look like");
        fails("tile", r#"{"windows":[{"id":"Notepad#0"}]}"#, "id must look like");
        fails("tile", r#"{"windows":[{"id":"Notepad#1","app":"Notepad"}]}"#, "either id or app");
        fails("tile", r#"{"windows":[{"app":"A"},{"app":"a"}]}"#, "twice");
        fails("tile", r#"{"windows":[{"id":"A#2"},{"app":"a","window":2}]}"#, "twice");
        fails("place", "{}", "items");
        fails("place", r#"{"items":[]}"#, "1 to 200");
        fails("place", r#"{"items":[{"app":"A","x":0,"y":0,"w":1}]}"#, "h must be a number");
        fails("place", r#"{"items":[{"app":"A","x":0,"y":0,"w":0,"h":1}]}"#, "above 0");
        fails("place", r#"{"items":[{"app":"A","x":0,"y":0,"w":1,"h":-1}]}"#, "h must be from");
        fails("place", r#"{"items":[{"app":"A","x":"0","y":0,"w":1,"h":1}]}"#, "x must be a number");
        fails("place", r#"{"items":[{"app":"A","x":99,"y":0,"w":1,"h":1}]}"#, "x must be from");
        fails("place", r#"{"items":[{"app":"A","x":0,"y":0,"w":1,"h":1,"title":"t"}]}"#, "unknown field title");
        fails("place", r#"{"items":[{"app":"A","x":0,"y":0,"w":1,"h":1,"z":1001}]}"#, "z must be");
        fails("place", r#"{"items":[{"app":"A","x":0,"y":0,"w":1,"h":1,"display":0}]}"#, "display");
        fails("place", r#"{"unit":"em","items":[{"app":"A","x":0,"y":0,"w":1,"h":1}]}"#, "unit must be one of ratio, pixel");
        fails("place", r#"{"items":[{"app":"A","x":0,"y":0,"w":1,"h":1},{"app":"A","x":0,"y":0,"w":1,"h":1}]}"#, "twice");
        fails("split", r#"{"parts":5,"direction":"horizontal","windows":[{"app":"A"}]}"#, "parts");
        fails("split", r#"{"parts":3,"direction":"diagonal","windows":[{"app":"A"}]}"#, "direction must be one of horizontal, vertical, quadrant");
        fails("split", r#"{"parts":3,"direction":"quadrant","windows":[{"app":"A"}]}"#, "quadrant needs parts = 4");
        fails("split", r#"{"parts":2,"direction":"horizontal","windows":[{"app":"A"},{"app":"B"},{"app":"C"}]}"#, "1 to 2");
        fails("split", r#"{"direction":"horizontal","windows":[{"app":"A"}]}"#, "parts is required");
        fails("split", r#"{"parts":2,"windows":[{"app":"A"}]}"#, "direction is required");
        fails("ratio", r#"{"ratios":[100],"windows":[{"app":"A"}]}"#, "2 to 4");
        fails("ratio", r#"{"ratios":[50,0],"windows":[{"app":"A"}]}"#, "each ratio");
        fails("ratio", r#"{"ratios":[50,"x"],"windows":[{"app":"A"}]}"#, "each ratio");
        fails("ratio", r#"{"ratios":[50,50],"direction":"quadrant","windows":[{"app":"A"}]}"#, "horizontal or vertical");
        fails("ratio", r#"{"ratios":[50,50],"windows":[{"app":"A"},{"app":"B"},{"app":"C"}]}"#, "1 to 2");
        fails("move_display", r#"{"windows":[{"app":"A"}]}"#, "display is required");
        fails("arrange_apps", r#"{"apps":[]}"#, "1 to 200");
        fails("arrange_apps", r#"{"apps":["A"],"arrangement":"columns"}"#, "at least 2");
        fails("arrange_apps", r#"{"apps":["A","B","C","D","E"],"arrangement":"quadrants"}"#, "at most 4");
        fails("arrange_apps", r#"{"apps":["A","a"]}"#, "twice");
        fails("arrange_apps", r#"{"apps":["  "]}"#, "each app");
        fails("arrange_apps", r#"{"apps":["A"],"arrangement":"spiral"}"#, "arrangement must be one of");
        fails("undo", r#"{"steps":0}"#, "steps");
        fails("undo", r#"{"steps":6}"#, "steps");
        fails("undo", r#"{"steps":1,"all":true}"#, "unknown field all");
        fails("displays", "[1]", "parameters must be an object");
        fails("nope", "{}", "unknown window action");
    }

    #[test]
    fn requests_turn_into_cells() {
        let cells = |action: &str, json: &str| ok(parse(action, json).unwrap().cells(SCREEN, 0.0).expect("cells"));
        assert_eq!(widths(&cells("arrange_apps", r#"{"apps":["A","B","C"],"arrangement":"columns"}"#)), vec![400.0; 3]);
        assert_eq!(cells("arrange_apps", r#"{"apps":["A","B","C","D","E","F"],"arrangement":"rows"}"#).len(), 6);
        assert_eq!(cells("arrange_apps", r#"{"apps":["A","B"],"arrangement":"quadrants"}"#).len(), 4, "empty quadrants stay empty");
        assert_eq!(cells("arrange_apps", r#"{"apps":["A","B","C","D","E","F","G","H","I","J","K","L"]}"#).len(), 12, "more than 8 apps in one call");
        assert_eq!(cells("ratio", r#"{"ratios":[70,30],"windows":[{"app":"A"}]}"#).len(), 2);
        assert!(PetWindowToolRequest::Undo { steps: 1 }.cells(SCREEN, 0.0).is_none());
        assert!(PetWindowToolRequest::Displays.cells(SCREEN, 0.0).is_none());
    }

    #[test]
    fn every_action_parses_and_the_list_has_eight() {
        assert_eq!(PetWindowTools::ACTIONS.len(), 8);
        for action in PetWindowTools::ACTIONS {
            let err = PetWindowTools::parse(action, Some(&json!({"bogus": 1}))).unwrap_err();
            assert!(err.message().contains("unknown field bogus"), "{action}: {}", err.message());
        }
    }

    // MARK: Windows runner (macOS WindowToolRunner)

    fn win(id: isize, exe: &str, app: &str, frame: Rect, minimized: bool) -> PetWindowInfo {
        PetWindowInfo { id, path: format!(r"C:\Windows\{exe}"), app: app.into(), frame, minimized }
    }

    /// Main display (index 0) at the origin, a second one to its left: display 1 = the left one.
    fn desk() -> Vec<PetDisplayInfo> {
        vec![PetDisplayInfo { frame: Rect::new(0.0, 0.0, 1920.0, 1080.0), work: Rect::new(0.0, 0.0, 1920.0, 1040.0), name: "Main".into() },
             PetDisplayInfo { frame: Rect::new(-1280.0, 0.0, 1280.0, 1024.0), work: Rect::new(-1280.0, 0.0, 1280.0, 984.0), name: "Left".into() }]
    }

    fn windows() -> Vec<PetWindowInfo> {
        vec![win(11, "notepad.exe", "메모장", Rect::new(100.0, 100.0, 800.0, 600.0), false),
             win(21, "msedge.exe", "Microsoft Edge", Rect::new(-1200.0, 50.0, 1000.0, 700.0), false),
             win(12, "notepad.exe", "메모장", Rect::new(300.0, 200.0, 800.0, 600.0), false),
             win(13, "notepad.exe", "메모장", Rect::new(0.0, 0.0, 160.0, 28.0), true),
             win(31, "calc.exe", "계산기", Rect::new(1500.0, 100.0, 320.0, 500.0), false)]
    }

    fn plan(json: &str, action: &str) -> Result<PetWindowPlan, PetCommandOutcome> {
        PetWindowTools::plan(&parse(action, json).expect("valid"), &windows(), &desk(), 0.0)
    }

    fn all_applied(p: &PetWindowPlan) -> Vec<Option<Rect>> { p.moves.iter().map(|m| Some(m.requested)).collect() }

    fn detail<'a>(o: &'a PetCommandOutcome, key: &str) -> &'a Value { o.details.as_ref().and_then(|d| d.get(key)).expect(key) }

    #[test]
    fn displays_answer_numbers_from_the_left_with_usable_area_from_the_corner() {
        let o = PetWindowTools::displays(&desk());
        assert!(o.ok);
        assert_eq!(o.message, "2 displays.");
        let list = detail(&o, "displays").as_array().unwrap();
        assert_eq!(list[0], json!({"display": 1, "name": "Left", "main": false, "x": -1280, "y": 0, "width": 1280, "height": 1024,
                                   "usable": {"x": 0, "y": 0, "width": 1280, "height": 984}}));
        assert_eq!(list[1]["display"], 2);
        assert_eq!(list[1]["main"], true, "main = the display Windows lists first");
        // A taskbar at the top of a display: the usable area starts below it.
        let top_bar = [PetDisplayInfo { frame: Rect::new(0.0, 0.0, 1920.0, 1080.0), work: Rect::new(0.0, 48.0, 1920.0, 1032.0), name: "A".into() }];
        let one = PetWindowTools::displays(&top_bar);
        assert_eq!(one.message, "1 display.");
        assert_eq!(detail(&one, "displays")[0]["usable"]["y"], 48);
    }

    #[test]
    fn selectors_resolve_by_name_exe_id_and_korean_id() {
        let p = plan(r#"{"windows":[{"app":"notepad","window":2},{"id":"msedge.exe#1"},{"id":"메모장#1"}],"display":2}"#, "move_display").unwrap();
        assert_eq!(p.moves.iter().map(|m| m.id).collect::<Vec<_>>(), vec![12, 21, 11], "window N = Nth window of the program, front to back");
        assert_eq!(p.moves[0].label, "메모장 #2", "labels use the app's display name");
        assert_eq!(p.moves[1].label, "Microsoft Edge #1");
        assert!(p.missing.is_empty());
        assert_eq!(p.what, "Move to display 2");
    }

    #[test]
    fn missing_windows_carry_the_lookup_message_and_never_stop_the_others() {
        let p = plan(r#"{"windows":[{"app":"Photoshop"},{"app":"notepad","window":5},{"app":"calc"}],"display":2}"#, "move_display").unwrap();
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.moves[0].id, 31);
        assert_eq!(p.missing.len(), 2);
        assert!(p.missing[0].starts_with("Photoshop #1: No running app on this PC is named \"Photoshop\""), "{}", p.missing[0]);
        assert_eq!(p.missing[1], "notepad #5: 메모장 has 3 open windows, not 5", "a window that is not there keeps the name the caller used");
        let o = PetWindowTools::outcome(&p, &all_applied(&p));
        let rows = detail(&o, "windows").as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2]["reason"], json!("not found: 메모장 has 3 open windows, not 5"), "found windows first, then the missing ones");
        assert_eq!(rows[1]["placed"], false);
        assert_eq!(rows[0]["window"], "계산기 #1");
        assert_eq!(detail(&o, "missing").as_array().unwrap().len(), 2);
        assert!(o.ok && o.message.starts_with("Move to display 2: 1 of 3 windows placed; 2 not placed."), "{}", o.message);
        // Nothing found at all: an answer instead of a plan.
        let none = plan(r#"{"windows":[{"app":"Photoshop"}],"display":1}"#, "move_display").unwrap_err();
        assert!(!none.ok && none.message.starts_with("No window could be found: Photoshop #1: "), "{}", none.message);
        assert_eq!(detail(&none, "missing").as_array().unwrap().len(), 1);
    }

    #[test]
    fn minimized_windows_are_skipped() {
        let p = plan(r#"{"windows":[{"app":"notepad","window":3},{"app":"notepad"}]}"#, "tile").unwrap();
        assert_eq!(p.moves.iter().map(|m| m.id).collect::<Vec<_>>(), vec![11]);
        assert_eq!(p.missing, vec!["메모장 #3: the window is minimized".to_string()]);
        let all = plan(r#"{"all_windows":true}"#, "tile").unwrap();
        assert!(all.moves.iter().all(|m| m.id != 13), "all_windows leaves minimized windows alone");
        assert_eq!(all.moves.len(), 4);
        assert_eq!(all.what, "All windows tiled");
        assert!(all.missing.is_empty());
        let only_min = [win(13, "notepad.exe", "메모장", Rect::default(), true)];
        let r = PetWindowTools::plan(&parse("tile", r#"{"all_windows":true}"#).unwrap(), &only_min, &desk(), 0.0).unwrap_err();
        assert_eq!(r.message, "There are no open windows to tile.");
    }

    #[test]
    fn the_same_window_named_twice_moves_once() {
        let p = plan(r#"{"windows":[{"app":"notepad"},{"id":"메모장#1"}],"display":2}"#, "move_display").unwrap();
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.missing, vec!["메모장 #1: the same window is listed twice".to_string()]);
    }

    #[test]
    fn display_numbers_pick_the_display_from_the_left() {
        // Display 1 is the left one (index 1): a split there lands in negative x.
        let p = plan(r#"{"parts":2,"direction":"horizontal","windows":[{"app":"notepad"},{"app":"calc"}],"display":1}"#, "split").unwrap();
        assert_eq!(p.moves[0].requested, Rect::new(-1280.0, 0.0, 640.0, 984.0));
        assert_eq!(p.moves[1].requested, Rect::new(-640.0, 0.0, 640.0, 984.0));
        let bad = plan(r#"{"parts":2,"direction":"horizontal","windows":[{"app":"notepad"}],"display":3}"#, "split").unwrap_err();
        assert_eq!(bad.message, "there is no display 3 (this PC has 2); see window_displays");
        assert!(plan(r#"{"windows":[{"app":"notepad"}],"display":5}"#, "move_display").is_err());
    }

    #[test]
    fn without_a_display_the_first_found_window_decides() {
        // Edge is on the left display, so the split happens there; a missing first window does not count.
        let p = plan(r#"{"parts":2,"direction":"vertical","windows":[{"app":"Photoshop"},{"app":"edge"}]}"#, "split").unwrap();
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.moves[0].requested, Rect::new(-1280.0, 492.0, 1280.0, 492.0), "Edge keeps its own cell (the second, bottom)");
        assert_eq!(p.what, "Placement");
    }

    #[test]
    fn place_converts_into_screen_coordinates_and_applies_in_z_order() {
        let p = plan(r#"{"items":[{"app":"notepad","x":0,"y":0.5,"w":0.5,"h":0.5,"z":5},{"app":"edge","display":2,"x":0.5,"y":0,"w":0.5,"h":0.5},
                                  {"app":"calc","x":0,"y":0,"w":1,"h":1,"z":-1}]}"#, "place").unwrap();
        // Notepad has no display: it stays on main (index 0, display 2), y from the top of the work area.
        let note = p.moves.iter().find(|m| m.id == 11).unwrap();
        assert_eq!(note.requested, Rect::new(0.0, 520.0, 960.0, 520.0));
        assert_eq!(note.before, Rect::new(100.0, 100.0, 800.0, 600.0));
        let edge = p.moves.iter().find(|m| m.id == 21).unwrap();
        assert_eq!(edge.requested, Rect::new(960.0, 0.0, 960.0, 520.0), "display 2 is the main display");
        assert_eq!(p.moves.iter().map(|m| m.id).collect::<Vec<_>>(), vec![31, 21, 11], "ascending z: calc (-1), edge (list 1), notepad (5) on top");
        let o = PetWindowTools::outcome(&p, &all_applied(&p));
        let rows = detail(&o, "windows").as_array().unwrap();
        assert_eq!(rows.iter().map(|r| r["window"].as_str().unwrap()).collect::<Vec<_>>(), vec!["메모장 #1", "Microsoft Edge #1", "계산기 #1"],
                   "the result keeps the request's order");
        assert_eq!(rows[0]["requested"], json!({"x": 0, "y": 520, "width": 960, "height": 520}));
        assert_eq!(rows[0]["reason"], "ok");
        assert_eq!(o.message, "Place: 3 of 3 windows placed. window_undo restores them.");
    }

    #[test]
    fn place_reports_clamping_and_bad_display_per_window() {
        let p = plan(r#"{"items":[{"app":"notepad","x":0.9,"y":0,"w":0.5,"h":0.5},{"app":"calc","display":7,"x":0,"y":0,"w":1,"h":1}]}"#, "place").unwrap();
        assert_eq!(p.moves.len(), 1);
        assert_eq!(p.moves[0].note.as_deref(), Some("clamped to the display"));
        assert_eq!(p.missing, vec!["계산기 #1: there is no display 7 (this PC has 2); see window_displays".to_string()]);
        let o = PetWindowTools::outcome(&p, &all_applied(&p));
        assert_eq!(detail(&o, "windows")[0]["reason"], "clamped to the display");
        let none = plan(r#"{"items":[{"app":"calc","display":7,"x":0,"y":0,"w":1,"h":1}]}"#, "place").unwrap_err();
        assert!(none.message.starts_with("No window could be placed: 계산기 #1: there is no display 7"), "{}", none.message);
    }

    #[test]
    fn tile_uses_the_shell_gap_unless_the_call_gives_one() {
        let req = parse("tile", r#"{"windows":[{"app":"notepad"},{"app":"calc"}],"display":2}"#).unwrap();
        let snap_gap = PetWindowTools::plan(&req, &windows(), &desk(), 8.0).unwrap();
        assert_eq!(snap_gap.moves[0].requested.x, 8.0);
        let own = parse("tile", r#"{"windows":[{"app":"notepad"},{"app":"calc"}],"display":2,"gap":0}"#).unwrap();
        assert_eq!(PetWindowTools::plan(&own, &windows(), &desk(), 8.0).unwrap().moves[0].requested.x, 0.0, "gap 0 of the call wins");
        let split = parse("split", r#"{"parts":2,"direction":"horizontal","windows":[{"app":"notepad"}],"display":2}"#).unwrap();
        assert_eq!(PetWindowTools::plan(&split, &windows(), &desk(), 8.0).unwrap().moves[0].requested.x, 8.0, "split uses the snap gap");
    }

    #[test]
    fn tile_across_two_displays_and_a_grid_that_does_not_fit() {
        let p = plan(r#"{"all_windows":true,"displays":[1,2],"span":"across","rows":1,"cols":4}"#, "tile").unwrap();
        assert_eq!(p.moves.len(), 4);
        let union_box = Rect::new(-1280.0, 0.0, 3200.0, 1040.0);
        assert!(p.moves.iter().all(|m| encloses(&union_box, &m.requested)));
        assert_eq!(p.moves[0].requested.x, -1280.0, "the grid starts on the left display");
        let per = plan(r#"{"all_windows":true,"displays":[1,2]}"#, "tile").unwrap();
        assert_eq!(per.moves.iter().filter(|m| m.requested.x < 0.0).count(), 2, "per display: shared out evenly");
        let err = plan(r#"{"all_windows":true,"rows":1,"cols":2,"display":2}"#, "tile").unwrap_err();
        assert_eq!(err.message, "1 x 2 has 2 cells for 4 windows; use more rows or columns.");
        let tiny = plan(r#"{"all_windows":true,"rows":20,"cols":20,"display":2}"#, "tile").unwrap();
        assert_eq!(tiny.tight, 4);
        let o = PetWindowTools::outcome(&tiny, &all_applied(&tiny));
        assert!(o.message.contains("; 4 cells are under 120 pixels on a side, apps may refuse that size"), "{}", o.message);
    }

    #[test]
    fn arrange_apps_takes_each_apps_front_window() {
        let p = plan(r#"{"apps":["notepad","calc"],"arrangement":"quadrants","display":2}"#, "arrange_apps").unwrap();
        assert_eq!(p.moves.iter().map(|m| m.id).collect::<Vec<_>>(), vec![11, 31]);
        assert_eq!(p.moves[1].requested, Rect::new(960.0, 0.0, 960.0, 520.0), "second app in the top-right quadrant");
        assert_eq!(p.tight, 0);
    }

    #[test]
    fn move_display_keeps_the_relative_place() {
        let p = plan(r#"{"windows":[{"app":"edge"}],"display":2}"#, "move_display").unwrap();
        let m = &p.moves[0];
        assert_eq!(m.before, Rect::new(-1200.0, 50.0, 1000.0, 700.0));
        assert_eq!(m.requested, layout::move_to(m.before, Rect::new(-1280.0, 0.0, 1280.0, 984.0), Rect::new(0.0, 0.0, 1920.0, 1040.0)));
        assert!(encloses(&Rect::new(0.0, 0.0, 1920.0, 1040.0), &m.requested));
    }

    #[test]
    fn displays_and_undo_are_not_plans_and_no_display_is_an_answer() {
        assert!(PetWindowTools::plan(&PetWindowToolRequest::Displays, &windows(), &desk(), 0.0).is_err());
        assert!(PetWindowTools::plan(&PetWindowToolRequest::Undo { steps: 1 }, &windows(), &desk(), 0.0).is_err());
        let none = PetWindowTools::plan(&parse("tile", r#"{"all_windows":true}"#).unwrap(), &windows(), &[], 0.0).unwrap_err();
        assert_eq!(none.message, "There is no display to place the windows on.");
    }

    #[test]
    fn elevated_windows_are_reported_without_accessibility_wording() {
        let p = plan(r#"{"windows":[{"app":"notepad"},{"app":"calc"}],"display":2}"#, "tile").unwrap();
        let o = PetWindowTools::outcome(&p, &[Some(p.moves[0].requested), None]);
        let rows = detail(&o, "windows").as_array().unwrap();
        assert_eq!(rows[1]["reason"], "elevated");
        assert_eq!(rows[1]["placed"], false);
        assert!(rows[1].get("applied").is_none());
        assert!(rows[1].get("requested").is_some());
        assert!(o.ok, "one window moved");
        assert!(o.message.starts_with("Tile: 1 of 2 windows placed; 1 not placed. window_undo restores them. 1 window could not be moved: "), "{}", o.message);
        assert!(o.message.contains(PetPermissionHelp::ELEVATED_WINDOWS));
        assert!(!o.message.to_lowercase().contains("accessibility"));
        let all_refused = PetWindowTools::outcome(&p, &[None, None]);
        assert!(!all_refused.ok);
        assert!(!all_refused.message.contains("window_undo"), "nothing to undo when nothing moved");
        // Only the moved window is remembered for undo.
        assert_eq!(PetWindowTools::undo_snapshot(&p, &[Some(p.moves[0].requested), None]), vec![(11, "메모장 #1".to_string(), p.moves[0].before)]);
    }

    #[test]
    fn a_window_that_kept_its_own_size_is_reported() {
        let p = plan(r#"{"windows":[{"app":"calc"}],"display":2}"#, "tile").unwrap();
        let landed = Rect::new(0.0, 0.0, 320.0, 500.0);
        let o = PetWindowTools::outcome(&p, &[Some(landed)]);
        let row = &detail(&o, "windows")[0];
        assert_eq!(row["reason"], "the app keeps its own minimum or fixed size");
        assert_eq!(row["applied"], json!({"x": 0, "y": 0, "width": 320, "height": 500}));
        assert_eq!(row["requested"], json!({"x": 0, "y": 0, "width": 1920, "height": 1040}));
        assert_eq!(o.message, "Tile: 1 of 1 window placed; 1 differ from the request (see reason). window_undo restores them.");
        // Within 4 pixels is a match.
        let near = PetWindowTools::outcome(&p, &[Some(Rect::new(2.0, 0.0, 1918.0, 1040.0))]);
        assert_eq!(detail(&near, "windows")[0]["reason"], "ok");
    }

    #[test]
    fn frames_are_reported_from_their_displays_corner() {
        let p = plan(r#"{"windows":[{"app":"notepad"}],"display":1}"#, "tile").unwrap();
        let o = PetWindowTools::outcome(&p, &all_applied(&p));
        assert_eq!(detail(&o, "windows")[0]["requested"], json!({"x": 0, "y": 0, "width": 1280, "height": 984}), "left display, negative screen x");
    }

    #[test]
    fn undo_plan_and_answer() {
        let mut stack = PetWindowUndoStack::default();
        let p = plan(r#"{"windows":[{"app":"notepad"},{"app":"calc"}],"display":2}"#, "tile").unwrap();
        stack.push(PetWindowTools::undo_snapshot(&p, &all_applied(&p)));
        stack.push(snap(&[(99, "Gone #1", Rect::default()), (13, "메모장 #3", Rect::default())]));
        let popped = stack.pop(2);
        let u = PetWindowTools::undo_plan(&popped, &windows(), &desk());
        assert_eq!(u.missing, vec!["Gone #1: window is gone".to_string(), "메모장 #3: the window is minimized".to_string()]);
        assert_eq!(u.moves.iter().map(|m| (m.id, m.requested)).collect::<Vec<_>>(),
                   vec![(11, Rect::new(100.0, 100.0, 800.0, 600.0)), (31, Rect::new(1500.0, 100.0, 320.0, 500.0))], "back to the frames from before");
        let restored: Vec<(String, Option<Rect>)> = vec![(u.moves[0].label.clone(), Some(u.moves[0].requested)), (u.moves[1].label.clone(), None)];
        let o = PetWindowTools::undo_outcome(&restored, &u.missing, popped.len(), &desk());
        assert!(o.ok);
        assert!(o.message.starts_with("Restored 1 window from 2 placements; 3 could not be restored. 1 window could not be moved: "), "{}", o.message);
        assert!(o.message.contains(PetPermissionHelp::ELEVATED_WINDOWS));
        assert_eq!(detail(&o, "restored"), &json!([{"window": "메모장 #1", "applied": {"x": 100, "y": 100, "width": 800, "height": 600}}]));
        assert_eq!(detail(&o, "missing").as_array().unwrap().last().unwrap(), "계산기 #1: elevated");
        let clean = PetWindowTools::undo_outcome(&restored[..1], &[], 1, &desk());
        assert_eq!(clean.message, "Restored 1 window from 1 placement.");
        let nothing = PetWindowTools::undo_outcome(&[], &[], 0, &desk());
        assert!(!nothing.ok);
        assert_eq!(nothing.message, "Nothing to undo: no placement by these window tools is remembered.");
    }
}
