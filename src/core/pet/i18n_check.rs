//! Every Korean sentence the pet's code can draw has a translation in each language file. The macOS app's words are
//! Korean; Windows keeps them as the lookup key, and `i18n::t` turns them into English, 日本語 and 简体中文 where they
//! are drawn. A new Korean literal in `src/pet_*.rs` or `src/core/pet/` fails here until `assets/i18n/{en,ja,zh}.json`
//! have it. Text only the server reads (tool descriptions, the selection message) sits between `i18n-exempt` markers.
use std::collections::HashMap;
use std::path::Path;

fn hangul(s: &str) -> bool { s.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)) }

/// The string literals of one source file that hold Hangul, as the key they are translated by (`{name}` → `{}`).
fn literals(src: &str) -> Vec<String> {
    let mut out = vec![];
    let mut exempt = false;
    let body = src.split("#[cfg(test)]").next().unwrap_or("");
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("// i18n-exempt:begin") { exempt = true; continue; }
        if t.starts_with("// i18n-exempt:end") { exempt = false; continue; }
        if exempt || t.starts_with("//") { continue; }
        let mut rest = line;
        while let Some(a) = rest.find('"') {
            let after = &rest[a + 1..];
            let mut end = None;
            let mut esc = false;
            for (i, c) in after.char_indices() {
                if esc { esc = false; } else if c == '\\' { esc = true; } else if c == '"' { end = Some(i); break; }
            }
            let Some(e) = end else { break };
            let lit = &after[..e];
            if hangul(lit) { out.push(key(lit)); }
            rest = &after[e + 1..];
        }
    }
    out
}

fn key(lit: &str) -> String {
    let lit = lit.replace("\\\"", "\"").replace("\\n", "\n").replace("\\'", "'");
    let mut out = String::new();
    let mut chars = lit.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut inner = String::new();
            let mut closed = false;
            for d in chars.by_ref() { if d == '}' { closed = true; break; } inner.push(d); }
            if closed && !inner.contains('{') { if !out.ends_with("{}") { out.push_str("{}"); } continue; }
            out.push('{'); out.push_str(&inner);
        } else { out.push(c); }
    }
    out
}

fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("core/clock.rs")];
    for dir in [root.clone(), root.join("core/pet")] {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let wanted = if dir == root { name.starts_with("pet_") } else { true };
            if wanted && name.ends_with(".rs") && name != "i18n_check.rs" { files.push(p); }
        }
    }
    files.sort();
    files.into_iter().map(|p| (p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read_to_string(&p).unwrap())).collect()
}

fn table(json: &str) -> HashMap<String, String> { serde_json::from_str(json).unwrap() }

#[test]
fn every_korean_sentence_the_pet_draws_is_translated() {
    let langs = [("en", include_str!("../../../assets/i18n/en.json")), ("ja", include_str!("../../../assets/i18n/ja.json")),
                 ("zh", include_str!("../../../assets/i18n/zh.json"))];
    let tables: Vec<(&str, HashMap<String, String>)> = langs.iter().map(|(l, j)| (*l, table(j))).collect();
    let mut missing = vec![];
    let mut seen = 0;
    for (file, src) in sources() {
        for k in literals(&src) {
            seen += 1;
            for (lang, t) in &tables {
                if !t.get(&k).is_some_and(|v| !v.is_empty()) { missing.push(format!("{lang}: {file}: {k}")); }
            }
        }
    }
    assert!(seen > 300, "the scan found only {seen} literals; did the sources move?");
    assert!(missing.is_empty(), "not translated ({}):\n{}", missing.len(), missing.join("\n"));
}

/// The placeholders of a Korean key and of its translation agree (`{}` count, or `{0}`…`{n}` that reorder).
#[test]
fn translations_keep_the_placeholders() {
    for (lang, json) in [("en", include_str!("../../../assets/i18n/en.json")), ("ja", include_str!("../../../assets/i18n/ja.json")),
                         ("zh", include_str!("../../../assets/i18n/zh.json"))] {
        for (k, v) in table(json) {
            if !hangul(&k) || v.is_empty() { continue; }
            let n = k.matches("{}").count();
            let plain = v.matches("{}").count();
            let numbered = (0..n).filter(|i| v.contains(&format!("{{{i}}}"))).count();
            assert!(plain == n && numbered == 0 || plain == 0 && numbered == n, "{lang}: placeholders differ: {k} → {v}");
        }
    }
}

#[test]
fn the_pet_speaks_every_language() {
    use crate::i18n::{set_language, t};
    set_language(Some("en"));
    assert_eq!(t("로그인 전"), "Signed out");
    assert_eq!(t("나초에게 묻기"), "Ask Nacho");
    assert_eq!(t("한 번에 보낼 수 있는 길이는 5자까지예요."), "You can send up to 5 characters at a time.");
    assert_eq!(t("무엇을: 결제 · 스타벅스"), "What: Payment · 스타벅스");
    assert_eq!(t("숫자키를 누르고 Enter · 3월 5일 10:00 KST까지"), "Press a number key, then Enter · until 3/5 10:00 KST");
    assert_eq!(t("Settings"), "Settings", "English text is left alone");
    set_language(Some("ja"));
    assert_eq!(t("로그인 전"), "ログイン前");
    assert_eq!(t("지금은 일을 맡길 수 없어요. 크레딧이 모자라요."), "今はお願いできません。クレジットが足りません。");
    set_language(Some("zh"));
    assert_eq!(t("실행하고 항상 허용"), "运行并始终允许");
    assert_eq!(t("수요일"), "星期三");
    set_language(Some("ko"));
    assert_eq!(t("로그인 전"), "로그인 전");
    set_language(None);
}

#[test]
fn an_approval_card_shows_what_the_action_does_not_its_internal_name() {
    use crate::core::pet::transcript::PetText;
    assert_eq!(PetText::humanize_action_names("PC에서 combo_quit_app 실행"), "PC에서 앱 종료 실행");
    assert_eq!(PetText::humanize_action_names("combo_appswitch_quit"), "앱 종료");
    assert_eq!(PetText::humanize_action_names("combo_clear_shelf, combo_unheard_of!"), "선반 비우기, 도구!");
    assert_eq!(PetText::humanize_action_names("my_combo_quit_app stays"), "my_combo_quit_app stays");
    assert_eq!(PetText::humanize_action_names("결제 승인"), "결제 승인");
}
