//! Four languages. Text is translated where it is drawn: an exact match first, then a pattern such as "{} running" whose
//! `{}` parts are carried over. Gadget names and the brand's Anton lines stay in English, like product names.
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

pub const LANGS: [(&str, &str); 4] = [("en", "English"), ("ko", "한국어"), ("ja", "日本語"), ("zh", "简体中文")];

struct Table { exact: HashMap<String, String>, patterns: Vec<(Vec<String>, String)> }

thread_local! {
    static LANG: RefCell<String> = RefCell::new(system_language());
    static TABLE: RefCell<Option<(String, Table)>> = const { RefCell::new(None) };
}

fn source(lang: &str) -> &'static str {
    match lang {
        "ko" => include_str!("../assets/i18n/ko.json"),
        "ja" => include_str!("../assets/i18n/ja.json"),
        "zh" => include_str!("../assets/i18n/zh.json"),
        // English has a table too, for the few sentences the pet's core writes in Korean (the macOS app's words).
        _ => include_str!("../assets/i18n/en.json"),
    }
}

/// The Windows display language, when it is one of ours.
pub fn system_language() -> String {
    #[cfg(windows)]
    {
        let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() } & 0x3ff;
        return match id { 0x12 => "ko", 0x11 => "ja", 0x04 => "zh", _ => "en" }.to_string();
    }
    #[allow(unreachable_code)]
    "en".to_string()
}

/// `None` follows Windows.
pub fn set_language(choice: Option<&str>) { LANG.with(|l| *l.borrow_mut() = choice.map(String::from).unwrap_or_else(system_language)); }
pub fn language() -> String { LANG.with(|l| l.borrow().clone()) }

fn build(lang: &str) -> Table {
    let map: HashMap<String, String> = serde_json::from_str(source(lang)).unwrap_or_default();
    let mut exact = HashMap::new();
    let mut patterns = vec![];
    for (k, v) in map {
        if v.is_empty() || k == v { continue; }
        if k.contains("{}") { patterns.push((k.split("{}").map(String::from).collect::<Vec<String>>(), v)); } else { exact.insert(k, v); }
    }
    // Longer patterns first, so "{} items on the shelf" wins over "{} items".
    patterns.sort_by_key(|p| std::cmp::Reverse(p.0.iter().map(|s| s.len()).sum::<usize>()));
    Table { exact, patterns }
}

/// The pieces between the fixed parts, or None when `s` does not have this shape.
fn captures(parts: &[String], s: &str) -> Option<Vec<String>> {
    let rest = s.strip_prefix(parts[0].as_str())?;
    let mut rest = rest;
    let mut out = vec![];
    for (i, p) in parts[1..].iter().enumerate() {
        let last = i == parts.len() - 2;
        let at = if last { if p.is_empty() { Some(rest.len()) } else { rest.rfind(p.as_str()).filter(|at| at + p.len() == rest.len()) } } else if p.is_empty() { None } else { rest.find(p.as_str()) };
        let at = at?;
        // Only a trailing placeholder may be empty: it is the plural "s" of "{} item{}".
        if at == 0 && !(last && p.is_empty()) { return None; }
        out.push(rest[..at].to_string());
        rest = &rest[at + p.len()..];
    }
    rest.is_empty().then_some(out)
}

/// Puts the captured pieces into a translation: `{}` in order, or `{0}` `{1}` where the language reorders them.
fn fill(t: &str, args: &[String]) -> String {
    let mut out = t.to_string();
    for (i, a) in args.iter().enumerate() { out = out.replace(&format!("{{{i}}}"), a); }
    for a in args { if let Some(at) = out.find("{}") { out.replace_range(at..at + 2, a); } }
    out
}

fn has_hangul(s: &str) -> bool { s.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)) }

/// One string through one table: exact, then a pattern. A piece captured from a Korean sentence is translated on its own.
/// `open`: the patterns that start with a `{}` too; they match nearly any line that ends the same way, so they come last.
fn lookup(table: &Table, s: &str, open: bool) -> Option<String> {
    if let Some(v) = table.exact.get(s) { return Some(v.clone()); }
    for (parts, v) in table.patterns.iter().filter(|(p, _)| p[0].is_empty() == open) {
        if let Some(args) = captures(parts, s) {
            // A Korean pattern that starts with `{}` (`{}요일`, `{}원` …) fits the tail of any server sentence. It applies only when
            // every Korean piece it caught is itself a sentence of ours; "다음 주 수요일" is not, so it stays as the server wrote it.
            let ours = parts.iter().any(|p| has_hangul(p));
            let mut out = Vec::with_capacity(args.len());
            let mut fits = true;
            for a in args {
                if !has_hangul(&a) { out.push(a); continue; }
                let t = translate(table, &a);
                if open && ours && t == a { fits = false; break; }
                out.push(t);
            }
            if fits { return Some(fill(v, &out)); }
        }
    }
    None
}

/// `lookup`, and for a line made of several pieces (" · " or a new line apart) each piece on its own; what no table knows stays.
fn translate(table: &Table, s: &str) -> String {
    if let Some(v) = lookup(table, s, false) { return v; }
    for sep in ["\n", " · "] {
        if s.contains(sep) {
            let out = s.split(sep).map(|p| translate(table, p)).collect::<Vec<_>>().join(sep);
            if out != s { return out; }
        }
    }
    lookup(table, s, true).unwrap_or_else(|| s.to_string())
}

pub fn t(s: &str) -> Cow<'_, str> {
    let lang = language();
    // English text in English needs no table; only the Korean sentences of the pet's core do.
    if s.is_empty() || lang == "en" && s.is_ascii() { return Cow::Borrowed(s); }
    TABLE.with(|cell| {
        let mut c = cell.borrow_mut();
        if c.as_ref().is_none_or(|(l, _)| *l != lang) { *c = Some((lang.clone(), build(&lang))); }
        let out = translate(&c.as_ref().unwrap().1, s);
        if out == s { Cow::Borrowed(s) } else { Cow::Owned(out) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn patterns_capture_and_reorder() {
        let p: Vec<String> = "{} of {} set".split("{}").map(String::from).collect();
        assert_eq!(captures(&p, "3 of 24 set"), Some(vec!["3".into(), "24".into()]));
        assert_eq!(captures(&p, "nothing set"), None);
        assert_eq!(fill("{1}개 중 {0}개 설정", &["3".into(), "24".into()]), "24개 중 3개 설정");
        assert_eq!(fill("{}개 실행 중", &["2".into()]), "2개 실행 중");
        let q: Vec<String> = "Your box: {}".split("{}").map(String::from).collect();
        assert_eq!(captures(&q, "Your box: Desk"), Some(vec!["Desk".into()]));
        let plural: Vec<String> = "{} item{}".split("{}").map(String::from).collect();
        assert_eq!(captures(&plural, "1 item"), Some(vec!["1".into(), "".into()]));
        assert_eq!(captures(&plural, "3 items"), Some(vec!["3".into(), "s".into()]));
        assert_eq!(fill("항목 {0}개", &["3".into(), "s".into()]), "항목 3개");
    }
    #[test] fn every_language_file_is_valid_json() {
        for (l, _) in LANGS { let _: HashMap<String, String> = serde_json::from_str(source(l)).expect(l); }
    }
    /// Every sentence the Widget Board can say is translated, in each language: a missing key would show English.
    #[test] fn the_widget_board_speaks_every_language() {
        use crate::core::bento::*;
        use crate::core::settings::ModuleId;
        let mut said: Vec<String> = vec![];
        for m in board_modules() {
            said.push(off_line(m).into());
            for s in BentoSize::ALL { said.push(size_text(m, s).into()); }
        }
        let now = 1_794_400_000.0;
        let bodies = [build::monitor(true, Some(1.0), Some(2.0), Some(0.5), Some(1.0), Some(1.0)), build::window_snap(true, Some("Ctrl+Alt+Left"), 3), build::window_snap(true, None, 1),
            build::keep_awake(true, true, true, Some(now + 600.0), Some(now - 600.0), None, 30, now), build::keep_awake(true, true, true, None, None, None, 30, now), build::keep_awake(true, true, false, None, None, None, 30, now),
            build::keep_awake(true, false, false, None, None, None, 30, now), build::keep_awake(true, false, false, None, None, None, 0, now),
            build::clipboard(true, &[], now, 8), build::clipboard(true, &[(RowKind::Image, "x".into(), Some(now - 5.0)), (RowKind::Text, "x".into(), Some(now - 120.0)), (RowKind::Text, "x".into(), Some(now - 7200.0)), (RowKind::Text, "x".into(), Some(now - 100_000.0)), (RowKind::Text, "x".into(), Some(now - 900_000.0))], now, 8),
            build::capture(true, 0, None), build::capture(true, 2, None), build::focus_dim(true, 0.5, false), build::focus_dim(true, 0.5, true), build::devices(true, None, Some(50), None), build::devices(true, Some(0.5), None, Some("x")),
            build::calendar(true, 0, 1, &[]), build::disk(Some(1), Some(10), &[]), build::disk(Some(5), Some(10), &[("a".into(), 1)]), build::shelf(true, &[]), build::tray(true, true, false), build::tray(true, false, false), build::tray(true, true, true),
            build::app_switch(true, Some("Alt+Tab"), 2), build::app_switch(true, None, 1), build::mouse(1.5, true), build::mouse(1.0, false)];
        let mut add = |t: &str| said.extend(fmt::sentences(t).into_iter().map(String::from));
        for b in &bodies {
            match b {
                BentoBody::Basic { line } => add(line),
                BentoBody::Gauges { rings, side } => { rings.iter().for_each(|r| add(&r.label)); side.iter().filter(|l| !l.value.is_empty() || l.label.len() > 1).for_each(|l| add(&l.label)); }
                BentoBody::Layouts { caption, .. } => { if let Some(c) = caption { add(c) } }
                BentoBody::Timer { big, caption, .. } => { if big.chars().any(|c| c.is_alphabetic()) { add(big) } add(caption) }
                BentoBody::List { rows, empty } => { add(empty); rows.iter().for_each(|r| { add(&r.text); if let Some(d) = &r.detail { add(d) } }) }
                BentoBody::Stack { caption, .. } => add(caption),
                BentoBody::Level { low, high, caption, .. } => { add(low); add(high); add(caption) }
                BentoBody::Sliders { sliders, rows } => { sliders.iter().for_each(|s| add(&s.label)); rows.iter().for_each(|r| { add(&r.label); add(&r.value) }) }
                BentoBody::Calendar { weekday, note, .. } => { add(weekday); if let Some(n) = note { add(n) } }
                BentoBody::Usage { caption, segments, .. } => { add(caption); segments.iter().for_each(|s| add(&s.label)) }
                BentoBody::Slots { drop, .. } => add(drop),
                BentoBody::Strip { caption } => add(caption),
            }
        }
        for d in [BentoDot::Off, BentoDot::On, BentoDot::Error] { said.push(d.text().into()); }
        for wd in 0..7 { said.push(fmt::weekday(wd).into()); }
        for secs in [30.0, 300.0, 2.0 * 3600.0, 5400.0] { said.push(fmt::remaining(secs)); said.push(fmt::duration(secs)); }
        let info = crate::core::catalog::info(ModuleId::WidgetBoard);
        said.push(info.tagline.into());
        said.push(info.note.into());
        for f in info.features { said.push(f.title.into()); said.push(f.detail.into()); }
        for t in info.tips { if !t.gesture.is_empty() { said.push(t.gesture.into()); } said.push(t.title.into()); }
        for l in info.limits { said.push((*l).into()); }
        // What the user or Windows names (a copy, a folder, a device) is theirs and stays as it is.
        let own_words = ["CPU", "x", "a"];
        for lang in ["ko", "ja", "zh"] {
            set_language(Some(lang));
            for s in &said {
                if own_words.contains(&s.as_str()) || !s.chars().any(|c| c.is_alphabetic()) { continue; }
                assert_ne!(t(s), s.as_str(), "{lang}: not translated: {s}");
            }
        }
        set_language(Some("ko"));
        assert_eq!(t("14 min"), "14분");
        assert_eq!(t("1 h 30 min"), "1시간 30분");
        assert_eq!(t("Ctrl+Alt+Left and 3 more"), "Ctrl+Alt+Left 외 단축키 3개");
        assert_eq!(t("App shortcuts: 2"), "앱 단축키 2개");
        assert_eq!(t("Floats open: 1"), "떠 있는 캡처 1장");
        assert_eq!(t("Dims the other windows by 50%"), "다른 창을 50% 어둡게");
        set_language(None);
    }
}
