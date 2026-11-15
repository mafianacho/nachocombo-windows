//! Calendar Clock logic, ported from `ClockFormat` and `WorldClock` in the Mac app: how the clock reads, the month grid,
//! and the time in other cities. No Windows calls: the shell passes the current time in.
use serde::{Deserialize, Serialize};

/// How the clock writes hours. A value this version does not know reads as `Unknown`, which follows Windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClockHours {
    #[default]
    System,
    Twelve,
    TwentyFour,
    #[serde(other)]
    Unknown,
}

impl ClockHours {
    /// Whether the clock shows 24 hours, given what Windows uses for its own clock.
    pub fn is_24(self, system_24: bool) -> bool {
        match self { ClockHours::Twelve => false, ClockHours::TwentyFour => true, ClockHours::System | ClockHours::Unknown => system_24 }
    }
}

/// Most world clocks the calendar shows.
pub const MAX_WORLD_CLOCKS: usize = 6;

// ---------- dates ----------

/// A wall-clock date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil { pub year: i32, pub month: u32, pub day: u32, pub hour: u32, pub minute: u32, pub second: u32 }

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y as i32, m, d)
}

/// 0 = Sunday … 6 = Saturday.
pub fn weekday_of_days(days: i64) -> u32 { (days + 4).rem_euclid(7) as u32 }

impl Civil {
    pub fn from_unix(secs: i64) -> Civil {
        let days = secs.div_euclid(86_400);
        let rest = secs.rem_euclid(86_400) as u32;
        let (year, month, day) = civil_from_days(days);
        Civil { year, month, day, hour: rest / 3600, minute: rest % 3600 / 60, second: rest % 60 }
    }
    /// Seconds since 1970 as if this wall-clock time were UTC.
    pub fn to_unix(self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86_400 + (self.hour * 3600 + self.minute * 60 + self.second) as i64
    }
    pub fn weekday(&self) -> u32 { weekday_of_days(days_from_civil(self.year, self.month, self.day)) }
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month { 1 | 3 | 5 | 7 | 8 | 10 | 12 => 31, 4 | 6 | 9 | 11 => 30, _ => if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 { 29 } else { 28 } }
}

/// The month `step` months away, years rolling over.
pub fn shift_month(year: i32, month: u32, step: i32) -> (i32, u32) {
    let n = year * 12 + month as i32 - 1 + step;
    (n.div_euclid(12), n.rem_euclid(12) as u32 + 1)
}

/// Windows counts the first day of the week from Monday = 0; here Sunday = 0.
pub fn first_weekday_from_windows(v: u32) -> u32 { (v + 1) % 7 }

/// The weeks of a month for a grid: each cell is a day of the month, 0 for a blank. `first` is the weekday the week
/// starts on (0 = Sunday).
pub fn month_weeks(year: i32, month: u32, first: u32) -> Vec<[u32; 7]> {
    let lead = ((weekday_of_days(days_from_civil(year, month, 1)) + 7 - first % 7) % 7) as usize;
    let days = days_in_month(year, month) as usize;
    let mut weeks = vec![[0u32; 7]; (lead + days).div_ceil(7)];
    for d in 1..=days { let p = lead + d - 1; weeks[p / 7][p % 7] = d as u32; }
    weeks
}

/// Whole days from the date of `from` to the date of `to`.
pub fn days_between(from: (i32, u32, u32), to: (i32, u32, u32)) -> i64 { days_from_civil(to.0, to.1, to.2) - days_from_civil(from.0, from.1, from.2) }

/// "Today", "Tomorrow", "In 12 days", "3 days ago": how far a day is from today.
pub fn relative_day(diff: i64) -> String {
    match diff { 0 => "Today".into(), 1 => "Tomorrow".into(), -1 => "Yesterday".into(), d if d > 1 => format!("In {d} days"), d => format!("{} days ago", -d) }
}

/// Weekday numbers (0 = Sunday) in the order the grid shows them.
pub fn week_order(first: u32) -> [u32; 7] { std::array::from_fn(|i| (first % 7 + i as u32) % 7) }

// ---------- how it reads ----------

const WEEKDAY_EN: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const WEEKDAY_KO: [&str; 7] = ["일", "월", "화", "수", "목", "금", "토"];
const WEEKDAY_JA: [&str; 7] = ["日", "月", "火", "水", "木", "金", "土"];
const WEEKDAY_ZH: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
const MONTH_EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTH_FULL_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// A short weekday for the grid's header.
pub fn weekday_short(lang: &str, wd: u32) -> String {
    let i = (wd % 7) as usize;
    match lang { "ko" => WEEKDAY_KO[i], "ja" => WEEKDAY_JA[i], "zh" => WEEKDAY_ZH[i], _ => WEEKDAY_EN[i] }.to_string()
}

/// "Sat Nov 14", "11월 14일 (토)": the date and weekday in the order of the language. Empty when both are off.
pub fn date_text(lang: &str, c: &Civil, date: bool, weekday: bool) -> String {
    let wd = c.weekday() as usize;
    let (m, d) = (c.month, c.day);
    match (lang, date, weekday) {
        (_, false, false) => String::new(),
        ("ko", true, true) => format!("{m}월 {d}일 ({})", WEEKDAY_KO[wd]),
        ("ko", true, false) => format!("{m}월 {d}일"),
        ("ko", false, true) => format!("{}요일", WEEKDAY_KO[wd]),
        ("ja", true, true) => format!("{m}月{d}日({})", WEEKDAY_JA[wd]),
        ("ja", true, false) => format!("{m}月{d}日"),
        ("ja", false, true) => format!("{}曜日", WEEKDAY_JA[wd]),
        ("zh", true, true) => format!("{m}月{d}日 周{}", WEEKDAY_ZH[wd]),
        ("zh", true, false) => format!("{m}月{d}日"),
        ("zh", false, true) => format!("周{}", WEEKDAY_ZH[wd]),
        (_, true, true) => format!("{} {} {d}", WEEKDAY_EN[wd], MONTH_EN[m as usize - 1]),
        (_, true, false) => format!("{} {d}", MONTH_EN[m as usize - 1]),
        (_, false, true) => WEEKDAY_EN[wd].to_string(),
    }
}

/// "3:05 PM", "15:05:09", "오후 3:05". `h24` is whether hours run to 24.
pub fn time_text(lang: &str, c: &Civil, seconds: bool, h24: bool) -> String {
    let tail = if seconds { format!(":{:02}", c.second) } else { String::new() };
    if h24 { return format!("{:02}:{:02}{tail}", c.hour, c.minute); }
    let h = match c.hour % 12 { 0 => 12, h => h };
    let pm = c.hour >= 12;
    let hm = format!("{h}:{:02}{tail}", c.minute);
    match lang {
        "ko" => format!("{} {hm}", if pm { "오후" } else { "오전" }),
        "ja" => format!("{}{hm}", if pm { "午後" } else { "午前" }),
        "zh" => format!("{}{hm}", if pm { "下午" } else { "上午" }),
        _ => format!("{hm} {}", if pm { "PM" } else { "AM" }),
    }
}

/// The whole clock line: the date and weekday if wanted, then the time.
pub fn clock_text(lang: &str, c: &Civil, date: bool, weekday: bool, seconds: bool, hours: ClockHours, system_24: bool) -> String {
    let d = date_text(lang, c, date, weekday);
    let t = time_text(lang, c, seconds, hours.is_24(system_24));
    if d.is_empty() { t } else { format!("{d} {t}") }
}

/// "November 2026", "2026년 11월".
pub fn month_title(lang: &str, year: i32, month: u32) -> String {
    match lang {
        "ko" => format!("{year}년 {month}월"),
        "ja" | "zh" => format!("{year}年{month}月"),
        _ => format!("{} {year}", MONTH_FULL_EN[month as usize - 1]),
    }
}

/// The long date under the grid: "Saturday, November 14".
pub fn day_title(lang: &str, c: &Civil) -> String {
    const LONG: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    match lang {
        "ko" | "ja" | "zh" => date_text(lang, c, true, true),
        _ => format!("{}, {} {}", LONG[c.weekday() as usize], MONTH_FULL_EN[c.month as usize - 1], c.day),
    }
}

// ---------- world clocks ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dst { None, Us, Eu, South, Egypt }

struct Zone { id: &'static str, minutes: i32, dst: Dst }

const fn z(id: &'static str, minutes: i32, dst: Dst) -> Zone { Zone { id, minutes, dst } }

/// The cities offered, west to east, with their winter offsets and daylight-saving rule. Same list as the Mac app.
const ZONES: [Zone; 38] = [
    z("Pacific/Honolulu", -600, Dst::None), z("America/Anchorage", -540, Dst::Us), z("America/Los_Angeles", -480, Dst::Us),
    z("America/Denver", -420, Dst::Us), z("America/Chicago", -360, Dst::Us), z("America/New_York", -300, Dst::Us),
    z("America/Toronto", -300, Dst::Us), z("America/Mexico_City", -360, Dst::None), z("America/Sao_Paulo", -180, Dst::None),
    z("Atlantic/Reykjavik", 0, Dst::None), z("Europe/London", 0, Dst::Eu), z("Europe/Lisbon", 0, Dst::Eu),
    z("Europe/Paris", 60, Dst::Eu), z("Europe/Berlin", 60, Dst::Eu), z("Europe/Amsterdam", 60, Dst::Eu),
    z("Europe/Stockholm", 60, Dst::Eu), z("Africa/Lagos", 60, Dst::None), z("Africa/Johannesburg", 120, Dst::None),
    z("Europe/Athens", 120, Dst::Eu), z("Africa/Cairo", 120, Dst::Egypt), z("Europe/Istanbul", 180, Dst::None),
    z("Europe/Moscow", 180, Dst::None), z("Asia/Dubai", 240, Dst::None), z("Asia/Karachi", 300, Dst::None),
    z("Asia/Kolkata", 330, Dst::None), z("Asia/Dhaka", 360, Dst::None), z("Asia/Bangkok", 420, Dst::None),
    z("Asia/Jakarta", 420, Dst::None), z("Asia/Singapore", 480, Dst::None), z("Asia/Hong_Kong", 480, Dst::None),
    z("Asia/Shanghai", 480, Dst::None), z("Asia/Taipei", 480, Dst::None), z("Asia/Seoul", 540, Dst::None),
    z("Asia/Tokyo", 540, Dst::None), z("Australia/Perth", 480, Dst::None), z("Australia/Sydney", 600, Dst::South),
    z("Pacific/Auckland", 720, Dst::South), z("UTC", 0, Dst::None),
];

/// The cities offered in the picker, west to east.
pub fn suggestions() -> impl Iterator<Item = &'static str> { ZONES.iter().map(|z| z.id) }

/// "America/New_York" → "New York".
pub fn city(id: &str) -> String { id.rsplit('/').next().unwrap_or(id).replace('_', " ") }

/// Up to `limit` cities whose name or id contains `query` (any case), leaving out those already chosen.
pub fn search_cities(query: &str, chosen: &[String], limit: usize) -> Vec<&'static str> {
    let q = query.trim().to_lowercase().replace(' ', "_");
    suggestions().filter(|id| !chosen.iter().any(|c| c == id)).filter(|id| q.is_empty() || id.to_lowercase().contains(&q)).take(limit).collect()
}

/// Day of the month of the `n`th `wd` (0 = Sunday) of a month.
fn nth_weekday(year: i32, month: u32, wd: u32, n: u32) -> u32 {
    let first = weekday_of_days(days_from_civil(year, month, 1));
    1 + (wd + 7 - first) % 7 + 7 * (n - 1)
}

fn last_weekday(year: i32, month: u32, wd: u32) -> u32 {
    let last = days_in_month(year, month);
    let at = weekday_of_days(days_from_civil(year, month, last));
    last - (at + 7 - wd) % 7
}

fn at(year: i32, month: u32, day: u32, hour: u32) -> i64 { Civil { year, month, day, hour, minute: 0, second: 0 }.to_unix() }

/// Whether daylight saving is in force at `utc` (seconds since 1970) for a zone with winter offset `std` seconds.
fn in_dst(rule: Dst, std: i64, utc: i64) -> bool {
    let year = Civil::from_unix(utc + std).year;
    match rule {
        Dst::None => false,
        // Second Sunday of March 02:00 standard to first Sunday of November 02:00 daylight.
        Dst::Us => utc >= at(year, 3, nth_weekday(year, 3, 0, 2), 2) - std && utc < at(year, 11, nth_weekday(year, 11, 0, 1), 2) - std - 3600,
        // Last Sunday of March to last Sunday of October, both at 01:00 UTC.
        Dst::Eu => utc >= at(year, 3, last_weekday(year, 3, 0), 1) && utc < at(year, 10, last_weekday(year, 10, 0), 1),
        // Southern hemisphere: from the first Sunday of October (Sydney) or the last of September (Auckland), 02:00
        // standard, to the first Sunday of April, 02:00 standard.
        Dst::South => {
            let (start_month, start_day) = if std >= 12 * 3600 { (9, last_weekday(year, 9, 0)) } else { (10, nth_weekday(year, 10, 0, 1)) };
            utc < at(year, 4, nth_weekday(year, 4, 0, 1), 2) - std || utc >= at(year, start_month, start_day, 2) - std
        }
        // Last Friday of April 00:00 to last Thursday of October 24:00 (daylight).
        Dst::Egypt => utc >= at(year, 4, last_weekday(year, 4, 5), 0) - std && utc < at(year, 10, last_weekday(year, 10, 4) + 1, 0) - std - 3600,
    }
}

/// The zone's offset from UTC in seconds at `utc`, or None for a name this version does not know.
pub fn zone_offset(id: &str, utc: i64) -> Option<i64> {
    let zone = ZONES.iter().find(|z| z.id == id)?;
    let std = zone.minutes as i64 * 60;
    Some(std + if in_dst(zone.dst, std, utc) { 3600 } else { 0 })
}

/// How far a city is from here: "+9h", "−3h 30m", "Same time".
pub fn offset_text(diff_seconds: i64) -> String {
    if diff_seconds == 0 { return "Same time".into(); }
    let sign = if diff_seconds > 0 { "+" } else { "−" };
    let minutes = diff_seconds.abs() / 60;
    let (h, m) = (minutes / 60, minutes % 60);
    if m == 0 { format!("{sign}{h}h") } else { format!("{sign}{h}h {m}m") }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 { Civil { year: y, month: mo, day: d, hour: h, minute: mi, second: 0 }.to_unix() }
    fn c(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Civil { Civil { year: y, month: mo, day: d, hour: h, minute: mi, second: s } }

    #[test] fn civil_conversion_roundtrips_and_knows_weekdays() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(Civil::from_unix(0), c(1970, 1, 1, 0, 0, 0));
        assert_eq!(c(2026, 11, 14, 0, 0, 0).weekday(), 6); // a Saturday
        assert_eq!(c(2024, 2, 29, 0, 0, 0).weekday(), 4); // a Thursday
        for days in [-800_000i64, -1, 0, 1, 11_000, 20_000, 60_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        let t = c(2026, 3, 8, 7, 0, 5);
        assert_eq!(Civil::from_unix(t.to_unix()), t);
    }

    #[test] fn month_arithmetic() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2026, 11), 30);
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 11, -23), (2024, 12));
        assert_eq!(first_weekday_from_windows(0), 1); // Monday
        assert_eq!(first_weekday_from_windows(6), 0); // Sunday
        assert_eq!(week_order(1), [1, 2, 3, 4, 5, 6, 0]);
    }

    #[test] fn relative_days() {
        assert_eq!(days_between((2026, 11, 14), (2026, 12, 2)), 18);
        assert_eq!(days_between((2026, 3, 1), (2026, 2, 28)), -1);
        assert_eq!(relative_day(0), "Today");
        assert_eq!(relative_day(1), "Tomorrow");
        assert_eq!(relative_day(-1), "Yesterday");
        assert_eq!(relative_day(18), "In 18 days");
        assert_eq!(relative_day(-3), "3 days ago");
    }

    #[test] fn month_grid_starts_where_the_week_starts() {
        // November 2026 starts on a Sunday.
        let sun = month_weeks(2026, 11, 0);
        assert_eq!(sun.len(), 5);
        assert_eq!(sun[0], [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(sun[4], [29, 30, 0, 0, 0, 0, 0]);
        let mon = month_weeks(2026, 11, 1);
        assert_eq!(mon[0], [0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(mon.len(), 6);
        // Every day appears exactly once, in order.
        let all: Vec<u32> = mon.iter().flatten().copied().filter(|d| *d != 0).collect();
        assert_eq!(all, (1..=30).collect::<Vec<u32>>());
        // A February that fits four rows.
        assert_eq!(month_weeks(2026, 2, 0).len(), 4);
    }

    #[test] fn clock_reads_in_each_language() {
        let t = c(2026, 11, 14, 15, 5, 9);
        assert_eq!(clock_text("en", &t, true, true, false, ClockHours::Twelve, true), "Sat Nov 14 3:05 PM");
        assert_eq!(clock_text("en", &t, false, false, true, ClockHours::TwentyFour, false), "15:05:09");
        assert_eq!(clock_text("en", &t, true, false, false, ClockHours::System, true), "Nov 14 15:05");
        assert_eq!(clock_text("en", &t, false, true, false, ClockHours::System, false), "Sat 3:05 PM");
        assert_eq!(clock_text("ko", &t, true, true, false, ClockHours::Twelve, false), "11월 14일 (토) 오후 3:05");
        assert_eq!(clock_text("ja", &t, true, true, false, ClockHours::Twelve, false), "11月14日(土) 午後3:05");
        assert_eq!(clock_text("zh", &t, true, true, false, ClockHours::Twelve, false), "11月14日 周六 下午3:05");
        assert_eq!(month_title("en", 2026, 11), "November 2026");
        assert_eq!(month_title("ko", 2026, 11), "2026년 11월");
        assert_eq!(day_title("en", &t), "Saturday, November 14");
    }

    #[test] fn twelve_hour_edges() {
        assert_eq!(time_text("en", &c(2026, 1, 1, 0, 7, 0), false, false), "12:07 AM");
        assert_eq!(time_text("en", &c(2026, 1, 1, 12, 0, 0), false, false), "12:00 PM");
        assert_eq!(time_text("en", &c(2026, 1, 1, 3, 4, 5), false, true), "03:04");
        assert_eq!(time_text("ko", &c(2026, 1, 1, 0, 7, 0), false, false), "오전 12:07");
    }

    #[test] fn hours_setting_follows_windows_only_when_asked() {
        assert!(ClockHours::System.is_24(true));
        assert!(!ClockHours::System.is_24(false));
        assert!(!ClockHours::Twelve.is_24(true));
        assert!(ClockHours::TwentyFour.is_24(false));
        // A value a newer or older version wrote must not break the whole settings file.
        assert_eq!(serde_json::from_str::<ClockHours>("\"bogus\"").unwrap(), ClockHours::Unknown);
        assert!(ClockHours::Unknown.is_24(true));
        assert_eq!(serde_json::to_string(&ClockHours::TwentyFour).unwrap(), "\"twentyFour\"");
    }

    #[test] fn city_names_and_offsets() {
        assert_eq!(city("America/New_York"), "New York");
        assert_eq!(city("UTC"), "UTC");
        assert_eq!(offset_text(9 * 3600), "+9h");
        assert_eq!(offset_text(-(3 * 3600 + 1800)), "−3h 30m");
        assert_eq!(offset_text(0), "Same time");
        assert_eq!(zone_offset("Asia/Kolkata", 0), Some(19_800));
        assert_eq!(zone_offset("Mars/Olympus", 0), None);
        assert_eq!(suggestions().count(), 38);
    }

    #[test] fn united_states_switches_on_the_second_sunday_of_march() {
        // 2026: March 8, 02:00 EST = 07:00 UTC; November 1, 02:00 EDT = 06:00 UTC.
        assert_eq!(zone_offset("America/New_York", utc(2026, 3, 8, 6, 59)), Some(-5 * 3600));
        assert_eq!(zone_offset("America/New_York", utc(2026, 3, 8, 7, 0)), Some(-4 * 3600));
        assert_eq!(zone_offset("America/New_York", utc(2026, 11, 1, 5, 59)), Some(-4 * 3600));
        assert_eq!(zone_offset("America/New_York", utc(2026, 11, 1, 6, 0)), Some(-5 * 3600));
        assert_eq!(zone_offset("America/Los_Angeles", utc(2026, 7, 1, 0, 0)), Some(-7 * 3600));
        assert_eq!(zone_offset("America/Los_Angeles", utc(2026, 1, 1, 0, 0)), Some(-8 * 3600));
        assert_eq!(zone_offset("Pacific/Honolulu", utc(2026, 7, 1, 0, 0)), Some(-10 * 3600));
    }

    #[test] fn europe_switches_at_one_utc_on_the_last_sunday() {
        // 2026: March 29 and October 25.
        assert_eq!(zone_offset("Europe/Paris", utc(2026, 3, 29, 0, 59)), Some(3600));
        assert_eq!(zone_offset("Europe/Paris", utc(2026, 3, 29, 1, 0)), Some(7200));
        assert_eq!(zone_offset("Europe/Paris", utc(2026, 10, 25, 0, 59)), Some(7200));
        assert_eq!(zone_offset("Europe/Paris", utc(2026, 10, 25, 1, 0)), Some(3600));
        assert_eq!(zone_offset("Europe/London", utc(2026, 7, 1, 0, 0)), Some(3600));
        assert_eq!(zone_offset("Europe/London", utc(2026, 12, 1, 0, 0)), Some(0));
        assert_eq!(zone_offset("Europe/Moscow", utc(2026, 7, 1, 0, 0)), Some(3 * 3600));
    }

    #[test] fn southern_hemisphere_is_summer_in_january() {
        assert_eq!(zone_offset("Australia/Sydney", utc(2026, 1, 15, 0, 0)), Some(11 * 3600));
        assert_eq!(zone_offset("Australia/Sydney", utc(2026, 7, 15, 0, 0)), Some(10 * 3600));
        // Sydney: ends April 5 at 03:00 local = April 4 16:00 UTC; starts October 4 at 02:00 = October 3 16:00 UTC.
        assert_eq!(zone_offset("Australia/Sydney", utc(2026, 4, 4, 15, 59)), Some(11 * 3600));
        assert_eq!(zone_offset("Australia/Sydney", utc(2026, 4, 4, 16, 0)), Some(10 * 3600));
        assert_eq!(zone_offset("Australia/Sydney", utc(2026, 10, 3, 15, 59)), Some(10 * 3600));
        assert_eq!(zone_offset("Australia/Sydney", utc(2026, 10, 3, 16, 0)), Some(11 * 3600));
        assert_eq!(zone_offset("Pacific/Auckland", utc(2026, 1, 15, 0, 0)), Some(13 * 3600));
        assert_eq!(zone_offset("Pacific/Auckland", utc(2026, 7, 15, 0, 0)), Some(12 * 3600));
        // Auckland's summer time starts in the last week of September.
        assert_eq!(zone_offset("Pacific/Auckland", utc(2026, 9, 20, 0, 0)), Some(12 * 3600));
        assert_eq!(zone_offset("Pacific/Auckland", utc(2026, 10, 1, 0, 0)), Some(13 * 3600));
        assert_eq!(zone_offset("Australia/Perth", utc(2026, 1, 15, 0, 0)), Some(8 * 3600));
    }

    #[test] fn egypt_keeps_summer_time_from_last_friday_of_april() {
        assert_eq!(zone_offset("Africa/Cairo", utc(2026, 1, 15, 0, 0)), Some(2 * 3600));
        assert_eq!(zone_offset("Africa/Cairo", utc(2026, 7, 15, 0, 0)), Some(3 * 3600));
        // 2026: starts Friday April 24 at 00:00 (April 23 22:00 UTC), ends Thursday October 29 at 24:00 (Oct 29 21:00 UTC).
        assert_eq!(zone_offset("Africa/Cairo", utc(2026, 4, 23, 21, 59)), Some(2 * 3600));
        assert_eq!(zone_offset("Africa/Cairo", utc(2026, 4, 23, 22, 0)), Some(3 * 3600));
        assert_eq!(zone_offset("Africa/Cairo", utc(2026, 10, 29, 20, 59)), Some(3 * 3600));
        assert_eq!(zone_offset("Africa/Cairo", utc(2026, 10, 29, 21, 0)), Some(2 * 3600));
    }

    #[test] fn every_suggested_city_resolves() {
        for id in suggestions() { assert!(zone_offset(id, utc(2026, 6, 1, 0, 0)).is_some(), "{id}"); }
    }

    #[test] fn city_search_skips_chosen_and_limits() {
        let none: Vec<String> = vec![];
        assert_eq!(search_cities("tokyo", &none, 6), vec!["Asia/Tokyo"]);
        assert_eq!(search_cities("new york", &none, 6), vec!["America/New_York"]);
        assert_eq!(search_cities("TOKYO", &["Asia/Tokyo".to_string()], 6), Vec::<&str>::new());
        assert_eq!(search_cities("", &none, 4).len(), 4);
        assert_eq!(search_cities("zzz", &none, 4).len(), 0);
    }
}
