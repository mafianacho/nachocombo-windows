//! Inline markdown for Nacho's chat bubbles. Port of `PetMarkdown.swift`. The macOS app used Foundation's markdown
//! parser (`inlineOnlyPreservingWhitespace`); this is a small CommonMark inline parser with the same visible result:
//! `**굵게**` / `__굵게__`, `*기울임*` / `_기울임_`, `` `코드` ``, `~~지움~~` (markers dropped, no strike style),
//! `[글](링크)`, `<https://…>` autolinks and backslash escapes. Line breaks and spaces are kept as written; block syntax
//! (lists, headings) is left as typed. A half-written marker (an answer still streaming: `**굵`) stays plain until it
//! closes. Links keep only web and mail addresses: the answer is model output, so an app link (`nachopet://card?…`) or
//! a `file:` link must not become a click that approves a card or opens a local file. Those stay plain text.
//!
//! Windows difference: a single `~` is never a strike marker (Foundation took `~a~` as one), so ranges like `3~5개`
//! stay as typed.

/// A run of Nacho's answer with its inline style.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PetMarkdownSpan {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    /// Only `http`, `https` and `mailto` links (see [`PetMarkdown::LINK_SCHEMES`]).
    pub link: Option<String>,
}

impl PetMarkdownSpan {
    pub fn plain(text: &str) -> Self { Self { text: text.into(), ..Default::default() } }
}

pub struct PetMarkdown;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style { Strong, Emph, Strike }

#[derive(Debug)]
enum Tok {
    Text(String),
    Code(String),
    Link(Vec<Tok>, Option<String>),
    Delim { ch: char, count: usize, orig: usize, can_open: bool, can_close: bool, opens: Vec<Style>, closes: Vec<Style> },
}

impl PetMarkdown {
    pub const LINK_SCHEMES: [&'static str; 3] = ["http", "https", "mailto"];

    /// The link when its scheme is a web or mail one (case-insensitive), else None.
    pub fn safe_link(url: &str) -> Option<String> {
        let (scheme, rest) = url.split_once(':')?;
        let ok_scheme = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
        if !ok_scheme || rest.is_empty() || url.chars().any(|c| c.is_whitespace() || c.is_control()) { return None; }
        Self::LINK_SCHEMES.contains(&scheme.to_lowercase().as_str()).then(|| url.to_string())
    }

    pub fn spans(source: &str) -> Vec<PetMarkdownSpan> {
        // Same trigger set as the Mac: without one of these the text is shown exactly as written.
        if !(source.contains(['*', '`', '_', '~']) || source.contains("](")) {
            return vec![PetMarkdownSpan::plain(source)];
        }
        let chars: Vec<char> = source.chars().collect();
        let mut toks = parse(&chars);
        process_emphasis(&mut toks);
        let mut out: Vec<PetMarkdownSpan> = vec![];
        render(&toks, &mut Attrs::default(), &mut out);
        if out.is_empty() { vec![PetMarkdownSpan::plain(source)] } else { out }
    }
}

// MARK: parsing

fn is_punct(c: char) -> bool { c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace() && !c.is_control()) }

fn push_text(toks: &mut Vec<Tok>, s: &str) {
    if s.is_empty() { return; }
    if let Some(Tok::Text(t)) = toks.last_mut() { t.push_str(s); } else { toks.push(Tok::Text(s.into())); }
}

fn run_len(c: &[char], i: usize, ch: char) -> usize { c[i..].iter().take_while(|&&x| x == ch).count() }

/// A code span starting at `i` (a backtick run): (content, end index after the closing run).
fn code_span(c: &[char], i: usize) -> Option<(String, usize)> {
    let n = run_len(c, i, '`');
    let mut j = i + n;
    while j < c.len() {
        if c[j] == '`' {
            let m = run_len(c, j, '`');
            if m == n {
                let mut s: String = c[i + n..j].iter().map(|&x| if x == '\n' { ' ' } else { x }).collect();
                if s.len() >= 2 && s.starts_with(' ') && s.ends_with(' ') && !s.chars().all(|x| x == ' ') { s = s[1..s.len() - 1].to_string(); }
                return Some((s, j + m));
            }
            j += m;
        } else {
            j += 1;
        }
    }
    None
}

/// `[text](dest "title")` starting at `i` (`[`): (text range, destination, end index).
fn inline_link(c: &[char], i: usize) -> Option<(usize, usize, String, usize)> {
    let mut depth = 0;
    let mut j = i + 1;
    let close = loop {
        if j >= c.len() { return None; }
        match c[j] {
            '\\' if j + 1 < c.len() => j += 2,
            '`' => match code_span(c, j) { Some((_, e)) => j = e, None => j += run_len(c, j, '`') },
            '[' => { depth += 1; j += 1; }
            ']' if depth == 0 => break j,
            ']' => { depth -= 1; j += 1; }
            _ => j += 1,
        }
    };
    if c.get(close + 1) != Some(&'(') { return None; }
    let mut k = close + 2;
    let skip_ws = |k: &mut usize| while *k < c.len() && c[*k].is_whitespace() { *k += 1 };
    skip_ws(&mut k);
    let mut dest = String::new();
    if c.get(k) == Some(&'<') {
        k += 1;
        while k < c.len() && c[k] != '>' {
            if c[k] == '\n' || c[k] == '<' { return None; }
            dest.push(c[k]);
            k += 1;
        }
        if k >= c.len() { return None; }
        k += 1;
    } else {
        let mut parens = 0;
        while k < c.len() && !c[k].is_whitespace() && !c[k].is_control() {
            match c[k] {
                '\\' if k + 1 < c.len() && c[k + 1].is_ascii_punctuation() => { dest.push(c[k + 1]); k += 2; continue; }
                '(' => parens += 1,
                ')' if parens == 0 => break,
                ')' => parens -= 1,
                _ => {}
            }
            dest.push(c[k]);
            k += 1;
        }
        if parens != 0 { return None; }
    }
    let before_title = k;
    skip_ws(&mut k);
    if k > before_title && k < c.len() && matches!(c[k], '"' | '\'' | '(') {
        let end = if c[k] == '(' { ')' } else { c[k] };
        k += 1;
        while k < c.len() && c[k] != end {
            if c[k] == '\\' { k += 1; }
            k += 1;
        }
        if k >= c.len() { return None; }
        k += 1;
        skip_ws(&mut k);
    }
    if c.get(k) != Some(&')') { return None; }
    Some((i + 1, close, dest, k + 1))
}

/// `<scheme:…>` starting at `i`: (url, end index).
fn autolink(c: &[char], i: usize) -> Option<(String, usize)> {
    let end = (i + 1..c.len()).find(|&k| c[k] == '>')?;
    let url: String = c[i + 1..end].iter().collect();
    if url.contains(['<', ' ', '\n']) { return None; }
    let (scheme, _) = url.split_once(':')?;
    let ok = (2..=32).contains(&scheme.len()) && scheme.chars().next()?.is_ascii_alphabetic()
        && scheme.chars().all(|x| x.is_ascii_alphanumeric() || "+-.".contains(x));
    ok.then_some((url, end + 1))
}

fn parse(c: &[char]) -> Vec<Tok> {
    let mut toks = vec![];
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            '\\' if i + 1 < c.len() && c[i + 1].is_ascii_punctuation() => {
                push_text(&mut toks, &c[i + 1].to_string());
                i += 2;
            }
            '`' => match code_span(c, i) {
                Some((s, e)) => { toks.push(Tok::Code(s)); i = e; }
                None => {
                    let n = run_len(c, i, '`');
                    push_text(&mut toks, &"`".repeat(n));
                    i += n;
                }
            },
            '[' => match inline_link(c, i) {
                Some((a, b, dest, e)) => {
                    let mut inner = parse(&c[a..b]);
                    process_emphasis(&mut inner);
                    toks.push(Tok::Link(inner, PetMarkdown::safe_link(&dest)));
                    i = e;
                }
                None => { push_text(&mut toks, "["); i += 1; }
            },
            '<' => match autolink(c, i) {
                Some((url, e)) => {
                    let link = PetMarkdown::safe_link(&url);
                    toks.push(Tok::Link(vec![Tok::Text(url)], link));
                    i = e;
                }
                None => { push_text(&mut toks, "<"); i += 1; }
            },
            '*' | '_' | '~' => {
                let n = run_len(c, i, ch);
                let before = if i == 0 { ' ' } else { c[i - 1] };
                let after = c.get(i + n).copied().unwrap_or(' ');
                let left = !after.is_whitespace() && (!is_punct(after) || before.is_whitespace() || is_punct(before));
                let right = !before.is_whitespace() && (!is_punct(before) || after.is_whitespace() || is_punct(after));
                let (can_open, can_close) = match ch {
                    '_' => (left && (!right || is_punct(before)), right && (!left || is_punct(after))),
                    '~' if n != 2 => (false, false),
                    _ => (left, right),
                };
                if can_open || can_close {
                    toks.push(Tok::Delim { ch, count: n, orig: n, can_open, can_close, opens: vec![], closes: vec![] });
                } else {
                    push_text(&mut toks, &ch.to_string().repeat(n));
                }
                i += n;
            }
            _ => {
                let start = i;
                while i < c.len() && !matches!(c[i], '\\' | '`' | '[' | '<' | '*' | '_' | '~') { i += 1; }
                if i == start { i += 1; }
                push_text(&mut toks, &c[start..i].iter().collect::<String>());
            }
        }
    }
    toks
}

/// CommonMark "process emphasis": match closers with the nearest fitting opener, innermost markers first.
fn process_emphasis(toks: &mut [Tok]) {
    let delim = |t: &Tok| match t { Tok::Delim { ch, count, orig, can_open, can_close, .. } => Some((*ch, *count, *orig, *can_open, *can_close)), _ => None };
    let mut active: Vec<usize> = (0..toks.len()).filter(|&i| delim(&toks[i]).is_some()).collect();
    let mut ci = 0;
    while ci < active.len() {
        let closer = active[ci];
        let Some((ch, ccount, corig, copen, cclose)) = delim(&toks[closer]) else { ci += 1; continue };
        if !cclose || ccount == 0 { ci += 1; continue; }
        let mut found = None;
        for oi in (0..ci).rev() {
            let Some((och, ocount, oorig, oopen, oclose)) = delim(&toks[active[oi]]) else { continue };
            if och != ch || !oopen || ocount == 0 { continue; }
            if ch == '~' && ocount != ccount { continue; }
            // rule of 3: a run that can both open and close does not pair across a multiple of 3
            if ch != '~' && (oclose || copen) && (oorig + corig) % 3 == 0 && !(oorig % 3 == 0 && corig % 3 == 0) { continue; }
            found = Some(oi);
            break;
        }
        let Some(oi) = found else {
            if !copen { active.remove(ci); } else { ci += 1; }
            continue;
        };
        let opener = active[oi];
        let ocount = delim(&toks[opener]).map(|d| d.1).unwrap_or(0);
        let (used, style) = match ch {
            '~' => (2, Style::Strike),
            _ if ocount >= 2 && ccount >= 2 => (2, Style::Strong),
            _ => (1, Style::Emph),
        };
        if let Tok::Delim { count, opens, .. } = &mut toks[opener] { *count -= used; opens.insert(0, style); }
        if let Tok::Delim { count, closes, .. } = &mut toks[closer] { *count -= used; closes.push(style); }
        // delimiters between them can no longer match
        active.drain(oi + 1..ci);
        ci = oi + 1;
        if delim(&toks[opener]).map(|d| d.1) == Some(0) { active.remove(oi); ci -= 1; }
        if delim(&toks[closer]).map(|d| d.1) == Some(0) { active.remove(ci); }
    }
}

// MARK: rendering

#[derive(Clone, Default)]
struct Attrs { strong: usize, emph: usize, code: bool, link: Option<String> }

fn emit(out: &mut Vec<PetMarkdownSpan>, text: &str, a: &Attrs) {
    if text.is_empty() { return; }
    let span = PetMarkdownSpan { text: text.into(), bold: a.strong > 0, italic: a.emph > 0, code: a.code, link: a.link.clone() };
    if let Some(last) = out.last_mut() {
        if (last.bold, last.italic, last.code, &last.link) == (span.bold, span.italic, span.code, &span.link) {
            last.text.push_str(&span.text);
            return;
        }
    }
    out.push(span);
}

fn apply(a: &mut Attrs, s: Style, on: bool) {
    let f = |n: &mut usize| if on { *n += 1 } else { *n = n.saturating_sub(1) };
    match s {
        Style::Strong => f(&mut a.strong),
        Style::Emph => f(&mut a.emph),
        Style::Strike => {}
    }
}

fn render(toks: &[Tok], a: &mut Attrs, out: &mut Vec<PetMarkdownSpan>) {
    for t in toks {
        match t {
            Tok::Text(s) => emit(out, s, a),
            Tok::Code(s) => {
                let mut c = a.clone();
                c.code = true;
                emit(out, s, &c);
            }
            Tok::Link(inner, link) => {
                let mut l = a.clone();
                l.link = link.clone();
                render(inner, &mut l, out);
            }
            Tok::Delim { ch, count, opens, closes, .. } => {
                // closing markers sit before the leftover characters, opening ones after them
                for s in closes { apply(a, *s, false); }
                emit(out, &ch.to_string().repeat(*count), a);
                for s in opens { apply(a, *s, true); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! `PetMarkdownTests` of `PetBusySendTests.swift`, plus a few CommonMark corner cases.
    use super::*;

    fn joined(s: &str) -> String { PetMarkdown::spans(s).iter().map(|x| x.text.as_str()).collect() }

    #[test]
    fn bold_renders_without_markers() {
        let spans = PetMarkdown::spans("이건 **중요해요** 정말");
        assert_eq!(spans.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["이건 ", "중요해요", " 정말"]);
        assert_eq!(spans.iter().map(|s| s.bold).collect::<Vec<_>>(), [false, true, false]);
        assert!(!joined("이건 **중요해요** 정말").contains('*'));
    }

    #[test]
    fn line_breaks_and_spaces_stay() {
        assert_eq!(joined("첫 줄\n\n  둘째 **줄**\n셋째"), "첫 줄\n\n  둘째 줄\n셋째");
    }

    #[test]
    fn italic_code_and_link() {
        let spans = PetMarkdown::spans("*기울임* `코드` [문서](https://example.com/a)");
        assert!(spans.iter().any(|s| s.text == "기울임" && s.italic));
        assert!(spans.iter().any(|s| s.text == "코드" && s.code));
        assert!(spans.iter().any(|s| s.text == "문서" && s.link.as_deref() == Some("https://example.com/a")));
    }

    #[test]
    fn only_web_and_mail_links_stay_clickable() {
        let spans = PetMarkdown::spans(
            "[승인](nachopet://card?id=c1&action=approve) [파일](file:///C:/Windows/System32/calc.exe) [스크립트](javascript:alert(1)) [메일](mailto:a@b.co)",
        );
        assert!(spans.iter().any(|s| s.text.contains("승인") && s.link.is_none()));
        assert!(spans.iter().any(|s| s.text.contains("파일") && s.link.is_none()));
        assert!(spans.iter().any(|s| s.text.contains("스크립트") && s.link.is_none()));
        assert!(spans.iter().any(|s| s.text == "메일" && s.link.as_deref() == Some("mailto:a@b.co")));
        assert!(!spans.iter().any(|s| s.link.as_deref().is_some_and(|l| l.starts_with("nachopet") || l.starts_with("file"))));
        assert!(!joined("[스크립트](javascript:alert(1))").contains("javascript"), "the link text only, like the Mac");
    }

    #[test]
    fn plain_text_and_half_written_markers_stay_as_typed() {
        assert_eq!(PetMarkdown::spans("그냥 글"), vec![PetMarkdownSpan::plain("그냥 글")]);
        assert_eq!(joined("아직 **쓰는 중"), "아직 **쓰는 중");
        assert_eq!(joined(""), "");
    }

    #[test]
    fn commonmark_corner_cases() {
        let s = PetMarkdown::spans("***둘 다***");
        assert_eq!((s.len(), s[0].bold, s[0].italic, s[0].text.as_str()), (1, true, true, "둘 다"));
        assert_eq!(joined("snake_case_name 그대로"), "snake_case_name 그대로");
        assert_eq!(joined("3~5개, 2 * 3 = 6"), "3~5개, 2 * 3 = 6");
        assert_eq!(joined("~~지움~~ \\*별\\*"), "지움 *별*");
        let b = PetMarkdown::spans("__굵게__ _기울임_");
        assert!(b[0].bold && b.iter().any(|x| x.italic && x.text == "기울임"));
        let c = PetMarkdown::spans("**`a*b`**");
        assert_eq!((c[0].text.as_str(), c[0].code, c[0].bold), ("a*b", true, true));
        let l = PetMarkdown::spans("보기: <https://example.com/x> [**굵은 링크**](http://e.test \"제목\")");
        assert!(l.iter().any(|x| x.text == "https://example.com/x" && x.link.as_deref() == Some("https://example.com/x")));
        assert!(l.iter().any(|x| x.text == "굵은 링크" && x.bold && x.link.as_deref() == Some("http://e.test")));
        assert_eq!(joined("[닫히지 않은](링크"), "[닫히지 않은](링크");
        assert_eq!(joined("`열린 코드"), "`열린 코드");
    }
}
