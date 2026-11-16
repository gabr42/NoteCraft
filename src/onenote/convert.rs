//! Converts parsed OneNote pages into NoteCraft notes.

use std::collections::HashSet;
use std::sync::LazyLock;

use chrono::{Local, NaiveDateTime, TimeZone};
use regex::Regex;

/// Name used for a page without a title (both file name and title).
pub const UNTITLED_PAGE: &str = "Untitled page";

static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(?:https?|ftp)://[^\s<>"]+"#).unwrap());
static SPAN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\?[<\u{ab}]span\b[^>\u{bb}\n]*[>\u{bb}]|\\?[<\u{ab}]/span[>\u{bb}]?\u{bb}?")
        .unwrap()
});
static FENCE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(```|~~~)").unwrap());
static TITLE_SPECIAL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[:#\[\]{}&*!|>'"%@`]"#).unwrap());
static SAFE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\\/:*?"<>|\r\n\t\x0b]"#).unwrap());

/// Escapes the Markdown specials `\ * ` [ ] < $` with a backslash.
pub fn esc_plain(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    for ch in t.chars() {
        if matches!(ch, '\\' | '*' | '`' | '[' | ']' | '<' | '$') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Escapes Markdown specials; bare URLs become `<url>` autolinks. Trailing punctuation
/// (`.,;:!?)'`) stays outside the link.
pub fn esc(t: &str) -> String {
    let mut out = String::new();
    let mut pos = 0;
    for m in URL_RE.find_iter(t) {
        let full = m.as_str();
        let url = full.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '\'']);
        let tail = &full[url.len()..];
        out.push_str(&esc_plain(&t[pos..m.start()]));
        out.push('<');
        out.push_str(url);
        out.push('>');
        out.push_str(&esc_plain(tail));
        pos = m.end();
    }
    out.push_str(&esc_plain(&t[pos..]));
    out
}

/// Removes `<span ...>` / `</span>` tags (plain or with guillemets). A line that held nothing but
/// tags is dropped; fenced code is left alone.
pub fn strip_spans(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fenced = false;
    for line in text.split('\n') {
        if FENCE_RE.is_match(line) {
            fenced = !fenced;
        }
        if fenced || !SPAN_RE.is_match(line) {
            out.push(line.to_string());
            continue;
        }
        let new = SPAN_RE.replace_all(line, "");
        // a line of only tags disappears
        if !new.trim().is_empty() {
            out.push(new.into_owned());
        }
    }
    out.join("\n")
}

/// Converts a UTC time from the file to the computer's local time, formatted `%Y-%m-%dT%H:%M:%S`.
pub fn local_stamp(utc: NaiveDateTime) -> String {
    Local.from_utc_datetime(&utc).format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// The value of the `title:` front-matter line, quoted when YAML would misread it.
pub fn front_title(title: &str) -> String {
    if TITLE_SPECIAL_RE.is_match(title) || title != title.trim() {
        format!("\"{}\"", title.replace('"', "\\\""))
    } else {
        title.to_string()
    }
}

/// File-system safe name; an empty result becomes `Untitled page`.
pub fn safe(s: &str) -> String {
    let cleaned = SAFE_RE.replace_all(s, "");
    let name = cleaned.trim().trim_end_matches('.');
    if name.is_empty() { UNTITLED_PAGE.to_string() } else { name.to_string() }
}

/// Hands out unique names within one folder (case-insensitive).
#[derive(Default)]
pub struct NameAllocator {
    used: HashSet<String>,
}

impl NameAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// True for names that always get a suffix: `resources` (any case) and names starting with `.`.
    pub fn reserved(name: &str) -> bool {
        name.eq_ignore_ascii_case("resources") || name.starts_with('.')
    }

    /// Returns `name`, then `name-1`, `name-2`, ... for repeated (case-insensitive) requests.
    pub fn take(&mut self, name: &str) -> String {
        let plain = !Self::reserved(name) && !self.used.contains(&name.to_lowercase());
        let result = if plain {
            name.to_string()
        } else {
            (1..)
                .map(|n| format!("{name}-{n}"))
                .find(|c| !self.used.contains(&c.to_lowercase()))
                .unwrap()
        };
        self.used.insert(result.to_lowercase());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_specials() {
        assert_eq!(esc_plain("a*b[c]<d>$e\\"), r"a\*b\[c\]\<d>\$e\\");
    }

    #[test]
    fn esc_autolink() {
        assert_eq!(esc("see https://x.org/a), now"), "see <https://x.org/a>), now");
    }

    #[test]
    fn spans_removed() {
        assert_eq!(
            strip_spans("<span style='x'>Hi</span>\n<span a>\n```\n<span>\n```"),
            "Hi\n```\n<span>\n```"
        );
        assert_eq!(strip_spans("«span style='x'»Hi«/span»"), "Hi");
    }

    #[test]
    fn title_quoting() {
        assert_eq!(front_title("Plain"), "Plain");
        assert_eq!(front_title("a: b"), "\"a: b\"");
        assert_eq!(front_title("say \"x\""), "\"say \\\"x\\\"\"");
        assert_eq!(front_title(" pad"), "\" pad\"");
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe("a/b:c?"), "abc");
        assert_eq!(safe("x."), "x");
        assert_eq!(safe("  "), "Untitled page");
    }

    #[test]
    fn allocator() {
        let mut a = NameAllocator::new();
        assert_eq!(a.take("Plan"), "Plan");
        assert_eq!(a.take("plan"), "plan-1");
        assert_eq!(a.take("Plan"), "Plan-2");
        assert_eq!(a.take("resources"), "resources-1");
        assert_eq!(a.take(".x"), ".x-1");
    }

    #[test]
    fn local_time() {
        for (y, m, d) in [(2024, 7, 15), (2024, 1, 15)] {
            let t = chrono::NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(12, 30, 5)
                .unwrap();
            let expect = chrono::Utc
                .from_utc_datetime(&t)
                .with_timezone(&Local)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string();
            assert_eq!(local_stamp(t), expect);
        }
    }
}
