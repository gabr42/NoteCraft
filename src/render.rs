//! Turning page Markdown into what the viewer, the search index and the printer need.

use base64::Engine as _;
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd};
use regex::Regex;
use std::borrow::Cow;
use std::path::Path;
use std::sync::LazyLock;

static RE_TABLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<table\b.*?</table\s*>").unwrap());
static RE_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<tr\b[^>]*>(.*?)</tr\s*>").unwrap());
static RE_CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<t[dh]\b[^>]*>(.*?)</t[dh]\s*>").unwrap());
static RE_ANCHOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<a\b[^>]*?href\s*=\s*["']([^"']*)["'][^>]*>(.*?)</a\s*>"#).unwrap());
static RE_IMG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)<img\b[^>]*?src\s*=\s*["']([^"']*)["'][^>]*>"#).unwrap());
static RE_PARA_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)</p\s*>|<p\b[^>]*>|<br\s*/?>|</li\s*>|</div\s*>").unwrap());
static RE_BR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
static RE_BLOCK_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</(p|div|h[1-6]|ul|ol)\s*>").unwrap());
static RE_LI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<li\b[^>]*>").unwrap());
static RE_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</?[a-z][a-z0-9]*(?:\s[^<>]*)?/?>").unwrap());
static RE_SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_SRC_ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\b(src|href)\s*=\s*(["'])([^"']*)(["'])"#).unwrap());

/// True when the page contains HTML the viewer cannot show as-is.
pub fn has_html(body: &str) -> bool {
    RE_TAG.is_match(body) || body.contains("<!--")
}

/// Markdown the viewer can show: HTML tables become Markdown tables, other HTML tags are dropped.
/// Also keeps every line on its own line (as in OneNote) and shows a lone `---` as a separator.
pub fn display_markdown(body: &str) -> Cow<'_, str> {
    Cow::Owned(keep_line_breaks(&html_to_markdown(body)))
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*') || t.chars().all(|c| c == '_'))
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// Single line breaks become real line breaks; `text` + `---` stays text followed by a separator.
pub fn keep_line_breaks(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out = String::with_capacity(md.len() + lines.len() * 2);
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        if is_fence(line) {
            in_fence = !in_fence;
        } else if !in_fence {
            let prev_text = i > 0 && !lines[i - 1].trim().is_empty() && !is_fence(lines[i - 1]);
            if is_rule(line) && prev_text {
                out.push('\n');
            }
        }
        out.push_str(line);
        if !in_fence && !is_fence(line) {
            let next = lines.get(i + 1).copied().unwrap_or("");
            let t = line.trim_start();
            let continues = !line.trim().is_empty()
                && !next.trim().is_empty()
                && !is_rule(line)
                && !is_rule(next)
                && !is_fence(next)
                && !next.trim().chars().all(|c| c == '=')
                && !t.starts_with('#')
                && !t.starts_with('|')
                && !line.ends_with("  ")
                && !line.ends_with('\\');
            if continues {
                out.push_str("  ");
            }
        }
        if i + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}

fn html_to_markdown(body: &str) -> Cow<'_, str> {
    if !has_html(body) {
        return Cow::Borrowed(body);
    }
    // Leave fenced code blocks untouched.
    let mut out = String::with_capacity(body.len());
    let mut chunk = String::new();
    let mut in_fence = false;
    for line in body.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            if !in_fence {
                out.push_str(&convert_html(&chunk));
                chunk.clear();
            }
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
        } else {
            chunk.push_str(line);
        }
    }
    out.push_str(&convert_html(&chunk));
    Cow::Owned(out)
}

fn convert_html(s: &str) -> String {
    if !has_html(s) {
        return s.to_string();
    }
    let s = RE_COMMENT.replace_all(s, "");
    let s = RE_TABLE.replace_all(&s, |c: &regex::Captures| format!("\n\n{}\n\n", table_to_markdown(&c[0])));
    let s = RE_IMG.replace_all(&s, |c: &regex::Captures| format!("![]({})", md_dest(&c[1])));
    let s = RE_ANCHOR.replace_all(&s, |c: &regex::Captures| anchor_to_markdown(&c[1], &c[2]));
    let s = RE_BR.replace_all(&s, "  \n");
    let s = RE_LI.replace_all(&s, "\n- ");
    let s = RE_BLOCK_END.replace_all(&s, "\n\n");
    let s = RE_TAG.replace_all(&s, "");
    // Avoid big gaps left by removed tags.
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.split('\n') {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.pop();
    out
}

fn md_dest(url: &str) -> String {
    let url = url.replace("&amp;", "&");
    if url.contains(' ') || url.contains('(') || url.contains(')') { format!("<{url}>") } else { url }
}

fn anchor_to_markdown(href: &str, inner: &str) -> String {
    let text = RE_TAG.replace_all(inner, "");
    let text = RE_SPACES.replace_all(text.trim(), " ");
    let href_plain = href.replace("&amp;", "&");
    if text.is_empty() {
        return String::new();
    }
    if text.replace("&amp;", "&") == href_plain && href_plain.contains("://") {
        format!("<{href_plain}>")
    } else {
        format!("[{}]({})", text.replace('[', "\\[").replace(']', "\\]"), md_dest(href))
    }
}

fn cell_to_markdown(inner: &str) -> String {
    let inner = RE_IMG.replace_all(inner, |c: &regex::Captures| format!("![]({})", md_dest(&c[1])));
    let inner = RE_ANCHOR.replace_all(&inner, |c: &regex::Captures| anchor_to_markdown(&c[1], &c[2]));
    let parts: Vec<String> = RE_PARA_SPLIT
        .split(&inner)
        .map(|p| RE_SPACES.replace_all(RE_TAG.replace_all(p, "").trim(), " ").to_string())
        .filter(|p| !p.is_empty())
        .collect();
    parts.join(" · ").replace('|', "\\|")
}

fn table_to_markdown(table: &str) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for r in RE_ROW.captures_iter(table) {
        let cells: Vec<String> = RE_CELL.captures_iter(&r[1]).map(|c| cell_to_markdown(&c[1])).collect();
        rows.push(cells);
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return RE_TAG.replace_all(table, " ").to_string();
    }
    for r in &mut rows {
        r.resize(cols, String::new());
    }
    let line = |r: &Vec<String>| format!("| {} |", r.iter().map(|c| if c.is_empty() { " " } else { c.as_str() }).collect::<Vec<_>>().join(" | "));
    let mut out = String::new();
    out.push_str(&line(&rows[0]));
    out.push('\n');
    out.push_str(&format!("|{}", "----|".repeat(cols)));
    for r in rows.iter().skip(1) {
        out.push('\n');
        out.push_str(&line(r));
    }
    out
}

/// Readable text of a page for searching (no Markdown or HTML syntax, no link targets).
pub fn plain_text(body: &str) -> String {
    let md = display_markdown(body);
    let mut out = String::with_capacity(md.len());
    for ev in Parser::new_ext(&md, md_options()) {
        match ev {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::TableRow | TagEnd::TableHead | TagEnd::CodeBlock) => {
                out.push('\n')
            }
            Event::End(TagEnd::TableCell) => out.push_str(" | "),
            Event::TaskListMarker(done) => out.push_str(if done { "[x] " } else { "[ ] " }),
            _ => {}
        }
    }
    out
}

pub fn md_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES
}

// ---------------------------------------------------------------------------------------------
// Printing

pub struct PrintPage<'a> {
    pub title: &'a str,
    pub created: &'a str,
    pub body: &'a str,
    pub dir: &'a Path,
}

fn has_scheme(url: &str) -> bool {
    if url.starts_with('#') || url.starts_with("//") {
        return true;
    }
    match url.find(':') {
        Some(i) => i > 1 && url[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)),
        None => false,
    }
}

pub(crate) fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

fn resolve(dir: &Path, url: &str) -> std::path::PathBuf {
    let decoded = url.replace("%20", " ");
    dir.join(decoded)
}

/// Local images are embedded so the printout is self-contained; other local links become file:// URLs.
fn fix_url(dir: &Path, url: &str, embed: bool) -> String {
    if url.is_empty() || has_scheme(url) {
        return url.to_string();
    }
    let path = resolve(dir, url);
    if embed
        && let Some(mime) = image_mime(&path)
        && let Ok(bytes) = std::fs::read(&path)
    {
        return format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    let abs = std::fs::canonicalize(&path).unwrap_or(path);
    let s = abs.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).to_string();
    format!("file:///{}", s.replace(' ', "%20"))
}

fn fix_html_urls(dir: &Path, html: &str) -> String {
    RE_SRC_ATTR
        .replace_all(html, |c: &regex::Captures| {
            let embed = c[1].eq_ignore_ascii_case("src");
            format!("{}={}{}{}", &c[1], &c[2], fix_url(dir, &c[3], embed), &c[4])
        })
        .to_string()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn body_html(page: &PrintPage) -> String {
    let md = keep_line_breaks(page.body);
    let parser = Parser::new_ext(&md, md_options()).map(|ev| match ev {
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
            let fixed = fix_url(page.dir, &dest_url, true);
            Event::Start(Tag::Image { link_type, dest_url: CowStr::from(fixed), title, id })
        }
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
            let fixed = fix_url(page.dir, &dest_url, false);
            Event::Start(Tag::Link { link_type, dest_url: CowStr::from(fixed), title, id })
        }
        Event::Html(h) => Event::Html(CowStr::from(fix_html_urls(page.dir, &h))),
        Event::InlineHtml(h) => Event::InlineHtml(CowStr::from(fix_html_urls(page.dir, &h))),
        other => other,
    });
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}

/// The page as HTML for the page view (same reading of the text as the printout).
pub fn page_html(body: &str) -> String {
    let md = keep_line_breaks(body);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, Parser::new_ext(&md, md_options()));
    html
}

pub fn friendly_date(stamp: &str) -> String {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(stamp.trim(), "%Y-%m-%dT%H:%M:%S") {
        return dt.format("%-d %B %Y, %H:%M").to_string();
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(stamp.trim(), "%Y-%m-%d") {
        return d.format("%-d %B %Y").to_string();
    }
    stamp.to_string()
}

pub fn print_document(doc_title: &str, pages: &[PrintPage]) -> String {
    let mut html = String::new();
    html.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>");
    html.push_str(&escape(doc_title));
    html.push_str(
        "</title><style>
@page { margin: 16mm; }
body { font-family: 'Segoe UI', Calibri, Arial, sans-serif; font-size: 11pt; color: #222; max-width: 190mm; margin: 0 auto; }
h1.page-title { font-size: 20pt; font-weight: 600; margin: 0 0 2pt 0; border-bottom: 1px solid #ccc; padding-bottom: 4pt; }
.meta { color: #777; font-size: 9pt; margin-bottom: 12pt; }
section + section { page-break-before: always; }
table { border-collapse: collapse; margin: 6pt 0; }
td, th { border: 1px solid #aaa; padding: 3pt 6pt; vertical-align: top; text-align: left; }
img { max-width: 100%; height: auto; }
a { color: #4a2a8a; word-break: break-all; }
pre, code { font-family: Consolas, monospace; font-size: 9.5pt; background: #f4f4f4; }
pre { padding: 6pt; white-space: pre-wrap; }
blockquote { border-left: 3px solid #bbb; margin-left: 0; padding-left: 10pt; color: #555; }
.hint { background: #fff8d6; border: 1px solid #e6d27a; padding: 6pt 10pt; margin-bottom: 12pt; font-size: 9.5pt; }
@media print { .hint { display: none; } }
</style></head><body>
<div class=\"hint\">The print window opens by itself. To print again, press Ctrl+P. You can also choose \"Save as PDF\" there.</div>
",
    );
    for p in pages {
        html.push_str("<section><h1 class=\"page-title\">");
        html.push_str(&escape(p.title));
        html.push_str("</h1>");
        if !p.created.is_empty() {
            html.push_str("<div class=\"meta\">");
            html.push_str(&escape(&friendly_date(p.created)));
            html.push_str("</div>");
        }
        html.push_str(&body_html(p));
        html.push_str("</section>\n");
    }
    html.push_str("<script>window.addEventListener('load', function () { setTimeout(function () { window.print(); }, 400); });</script></body></html>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_table_becomes_markdown() {
        let src = "<table><thead><tr><th><strong>kids</strong></th><th></th></tr></thead><tbody>\n<tr><td>ALICE</td><td><a href=\"http://x.si/a?b=1&amp;c=2\">http://x.si/a?b=1&amp;c=2</a></td></tr>\n<tr><td>BOB</td><td><p>one</p>\n<p></p><p>two | three</p></td></tr></tbody></table>";
        let md = display_markdown(src);
        assert!(md.contains("| kids |   |"), "{md}");
        assert!(md.contains("| ALICE | <http://x.si/a?b=1&c=2> |"), "{md}");
        assert!(md.contains("| BOB | one · two \\| three |"), "{md}");
    }

    #[test]
    fn span_inside_link_is_dropped() {
        let md = display_markdown("[<span style='font-family:inherit'>here</span>](https://e.com)");
        assert_eq!(md, "[here](https://e.com)");
    }

    #[test]
    fn lines_stay_lines() {
        let md = keep_line_breaks("KNJIGE\nOne\nTwo\n\nA\n---\nB\n```\nx\ny\n```\n");
        assert_eq!(md, "KNJIGE  \nOne  \nTwo\n\nA\n\n---\nB\n```\nx\ny\n```\n");
    }

    #[test]
    fn printout_embeds_images() {
        let dir = std::env::temp_dir().join(format!("notecraft-print-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("res")).unwrap();
        std::fs::write(dir.join("res").join("a b.png"), [137u8, 80, 78, 71]).unwrap();
        let body = "Line one\nLine two\n\n![pic](<res/a b.png>)\n\n<table><tr><td>x &amp; y</td></tr></table>\n";
        let html = print_document("T", &[PrintPage { title: "My <page>", created: "2010-11-11T09:26:57", body, dir: &dir }]);
        assert!(html.contains("data:image/png;base64,"), "{html}");
        assert!(html.contains("My &lt;page&gt;"));
        assert!(html.contains("Line one<br />"), "{html}");
        assert!(html.contains("<td>x &amp; y</td>"));
        assert!(html.contains("window.print()"));
        assert!(html.contains("11 November 2010"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autolinks_survive() {
        assert!(!has_html("see <http://example.com> now"));
    }

    #[test]
    fn plain_text_has_no_markup() {
        let t = plain_text("# Title\n\nSome **bold** [link](http://x)\n");
        assert!(t.contains("Title"));
        assert!(t.contains("Some bold link"));
        assert!(!t.contains("http://x"));
    }
}
