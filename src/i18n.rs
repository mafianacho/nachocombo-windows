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
        _ => "{}",
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

pub fn t(s: &str) -> Cow<'_, str> {
    let lang = language();
    if lang == "en" || s.is_empty() { return Cow::Borrowed(s); }
    TABLE.with(|cell| {
        let mut c = cell.borrow_mut();
        if c.as_ref().is_none_or(|(l, _)| *l != lang) { *c = Some((lang.clone(), build(&lang))); }
        let table = &c.as_ref().unwrap().1;
        if let Some(v) = table.exact.get(s) { return Cow::Owned(v.clone()); }
        for (parts, v) in &table.patterns {
            if let Some(args) = captures(parts, s) { return Cow::Owned(fill(v, &args)); }
        }
        Cow::Borrowed(s)
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
}
