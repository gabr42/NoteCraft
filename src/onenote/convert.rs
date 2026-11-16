//! Converts parsed OneNote pages into NoteCraft notes.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{Local, NaiveDateTime, TimeZone};
use regex::Regex;

use super::parse::*;

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

/// Converted page: title, times (UTC), nesting level and the rendered outline blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub title: String,
    pub created: Option<NaiveDateTime>,
    pub updated: Option<NaiveDateTime>,
    pub level: u32,
    /// One entry per outline/picture/attachment/table: its Markdown lines.
    pub blocks: Vec<Vec<String>>,
}

/// Fonts rendered as code.
const MONO: [&str; 6] = ["courier new", "consolas", "lucida console", "courier", "monaco", "andale mono"];
/// Deepest OE nesting followed (guards against cycles in the object graph).
const MAX_NEST: usize = 128;
/// Most OEs rendered per page (guards against exponential fan-out in a broken object graph).
const MAX_VISITS: usize = 500_000;
/// Property id (without the top bit) of the "completed" flag inside a note tag state.
const P_TAG_COMPLETED: u32 = 0x1400_346F;

static HYPERLINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\x{FDDF}HYPERLINK "([^"]*)""#).unwrap());
static TITLE_ESC_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\([\\*_`\[\]<])").unwrap());

/// Decodes UTF-16LE like Python's `decode('utf-16le', 'replace')`: lone surrogates and a stray last
/// byte become U+FFFD.
fn decode_utf16le(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let mut s = String::from_utf16_lossy(&units);
    if b.len() % 2 == 1 {
        s.push('\u{FFFD}');
    }
    s
}

fn le_u32(b: &[u8]) -> Option<u32> {
    let a: [u8; 4] = b.try_into().ok()?;
    Some(u32::from_le_bytes(a))
}

fn le_f32(b: Option<&[u8]>) -> f64 {
    match b.and_then(|b| <[u8; 4]>::try_from(b).ok()) {
        Some(a) => f32::from_le_bytes(a) as f64,
        None => 0.0,
    }
}

/// Python's `round(x, 1)`: exact decimal value rounded half to even. `-0.0` and NaN become `0.0`
/// so the result sorts consistently.
fn round1(x: f64) -> f64 {
    let r: f64 = format!("{x:.1}").parse().unwrap_or(0.0);
    if r == 0.0 || r.is_nan() { 0.0 } else { r }
}

/// Extension of a file name like Python's `os.path.splitext(name)[1]` (with the dot, or empty).
fn splitext(name: &str) -> &str {
    let base = &name[name.rfind(['/', '\\']).map_or(0, |i| i + 1)..];
    match base.rfind('.') {
        Some(d) if !base[..d].chars().all(|c| c == '.') => &base[d..],
        _ => "",
    }
}

/// Guesses a picture extension from its first bytes.
fn sniff(d: &[u8]) -> &'static str {
    if d.starts_with(b"\x89PNG\r\n\x1a\n") {
        ".png"
    } else if d.starts_with(b"\xff\xd8\xff") {
        ".jpg"
    } else if d.starts_with(b"GIF8") {
        ".gif"
    } else if d.starts_with(b"BM") {
        ".bmp"
    } else {
        ".bin"
    }
}

/// Wraps `body` in an HTML tag.
fn tagged(tag: &str, body: &str) -> String {
    format!("<{tag}>{body}</{tag}>")
}

/// Turns the pages of one section into Markdown lines. Pictures and attachments are handed to
/// `add_resource(bytes, extension with dot)`, which returns their file name in the resources folder.
pub struct Converter<'a> {
    one: &'a OneFile,
    add_resource: &'a mut dyn FnMut(&[u8], &str) -> String,
    /// Object space of the page being converted.
    sp: Option<&'a Space>,
    /// Problems found, each starting with the page title.
    pub warnings: Vec<String>,
    /// Problems of the page in progress (the title is not final yet).
    page_warn: Vec<String>,
    nest: usize,
    visits: usize,
}

impl<'a> Converter<'a> {
    pub fn new(one: &'a OneFile, add_resource: &'a mut dyn FnMut(&[u8], &str) -> String) -> Self {
        Converter { one, add_resource, sp: None, warnings: Vec::new(), page_warn: Vec::new(), nest: 0, visits: 0 }
    }

    // ---- helpers ----

    fn get(&self, id: Option<&ObjId>) -> Option<&'a Obj> {
        self.sp?.objects.get(id?)
    }

    /// Existing child objects listed under the given property ids, in that order.
    fn kids(&self, ob: &Obj, keys: &[u32]) -> Vec<(ObjId, &'a Obj)> {
        let mut out = Vec::new();
        for &k in keys {
            for id in ob.props.get_ids(k).iter().flatten() {
                if let Some(x) = self.get(Some(id)) {
                    out.push((*id, x));
                }
            }
        }
        out
    }

    fn text_of(&self, rt: &Obj) -> String {
        let p = &rt.props;
        let txt = if let Some(raw) = p.get_bytes(P_RICH_UNI) {
            decode_utf16le(raw)
        } else if let Some(raw) = p.get_bytes(P_TEXT_ASCII) {
            let cs = p.get_bytes(P_CHARSET).and_then(|b| b.first().copied()).unwrap_or(0);
            let enc = match cs {
                238 => encoding_rs::WINDOWS_1250,
                204 => encoding_rs::WINDOWS_1251,
                161 => encoding_rs::WINDOWS_1253,
                162 => encoding_rs::WINDOWS_1254,
                186 => encoding_rs::WINDOWS_1257,
                _ => encoding_rs::WINDOWS_1252,
            };
            enc.decode_without_bom_handling(raw).0.into_owned()
        } else {
            return String::new();
        };
        txt.trim_end_matches('\0').to_string()
    }

    /// Splits the text into runs (by code point) with their format properties.
    fn runs(&self, rt: &Obj, txt: &[char]) -> Vec<(String, Option<&'a PropSet>)> {
        let idxs: Vec<usize> = rt
            .props
            .get_bytes(P_RUN_INDEX)
            .map(|b| b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize).collect())
            .unwrap_or_default();
        let fmts = rt.props.get_ids(P_RUN_FMT);
        let mut bounds = vec![0usize];
        bounds.extend(idxs);
        bounds.push(txt.len());
        let mut out = Vec::new();
        for i in 0..bounds.len() - 1 {
            let a = bounds[i];
            let b = bounds[i + 1].min(txt.len());
            if b <= a {
                continue;
            }
            let f = fmts.get(i).and_then(|id| self.get(id.as_ref())).map(|o| &o.props);
            out.push((txt[a..b].iter().collect(), f));
        }
        out
    }

    fn render_rich(&self, rt: &Obj) -> String {
        let txt = self.text_of(rt);
        if txt.is_empty() {
            return String::new();
        }
        let style = self
            .get(rt.props.get_ids(P_PARA_STYLE).first().and_then(|i| i.as_ref()))
            .map(|ps| u16s(ps.props.get_bytes(P_PARA_STYLE_ID).unwrap_or(&[])).to_lowercase())
            .unwrap_or_default();
        let empty = PropSet::default();
        let chars: Vec<char> = txt.chars().collect();
        let mut parts: Vec<String> = Vec::new();
        for (t, f) in self.runs(rt, &chars) {
            let f = f.unwrap_or(&empty);
            if f.get_bool(P_HIDDEN) {
                continue;
            }
            let mut t = t;
            let mut link: Option<String> = None;
            if t.contains('\u{FDDF}') {
                if let Some(c) = HYPERLINK_RE.captures(&t) {
                    let l = c[1].to_string();
                    let m = c.get(0).unwrap();
                    t = format!("{}{}", &t[..m.start()], &t[m.end()..]).replace('\u{FDDF}', "");
                    if t.is_empty() {
                        t = l.clone();
                    }
                    link = Some(l).filter(|l| !l.is_empty());
                }
            }
            t = t
                .replace("\r\n", "\n")
                .replace('\r', "\n")
                .replace('\u{0B}', "\n")
                .replace('\u{2028}', "\n")
                .replace('\0', "")
                .replace('\u{FDDF}', "");
            let font = f.get_bytes(P_FONT).filter(|b| !b.is_empty()).map(|b| u16s(b).to_lowercase());
            let code = font.is_some_and(|f| MONO.contains(&f.as_str()));
            let ws = [' ', '\t'];
            let core = t.trim_matches(ws);
            if core.is_empty() {
                parts.push(t);
                continue;
            }
            let lead = &t[..t.len() - t.trim_start_matches(ws).len()];
            let trail = &t[t.trim_end_matches(ws).len()..];
            let mut body = if code {
                format!("`{}`", core.replace('`', "'"))
            } else if link.is_some() {
                esc_plain(core)
            } else {
                esc(core)
            };
            if !code {
                if f.get_bool(P_SUB) {
                    body = tagged("sub", &body);
                }
                if f.get_bool(P_SUPER) {
                    body = tagged("sup", &body);
                }
            }
            if let Some(l) = &link {
                body = if core == l { format!("<{l}>") } else { format!("[{body}]({l})") };
            } else if !code && f.get_bool(P_UNDER) {
                body = tagged("u", &body);
            }
            if f.get_bool(P_STRIKE) {
                body = format!("~~{body}~~");
            }
            let (b, i) = (f.get_bool(P_BOLD), f.get_bool(P_ITALIC));
            if b && i {
                body = format!("***{body}***");
            } else if b {
                body = format!("**{body}**");
            } else if i {
                body = format!("*{body}*");
            }
            parts.push(format!("{lead}{body}{trail}"));
        }
        let mut s = parts.concat().replace('\n', "  \n");
        let b = style.as_bytes();
        if b.len() == 2 && b[0] == b'h' && (b'1'..=b'6').contains(&b[1]) && !s.trim().is_empty() && !s.contains('\n') {
            s = format!("{} {}", "#".repeat((b[1] - b'0') as usize), s);
        }
        s
    }

    fn image_md(&mut self, im: &Obj) -> String {
        let r = im.props.get_ids(P_PICTURE).first().and_then(|i| i.as_ref());
        let fd = r.and_then(|r| self.sp?.filedecl.get(r));
        let mut alt = u16s(im.props.get_bytes(P_IMG_ALT).unwrap_or(&[]));
        if alt.is_empty() {
            alt = "image1".to_string();
        }
        let Some(fd) = fd else {
            self.page_warn.push("image without data".to_string());
            return String::new();
        };
        let guid = fd.0.replace("<ifndf>{", "").replace('}', "").to_lowercase();
        let one = self.one;
        let data = one.filedata.get(&guid).filter(|d| !d.is_empty());
        let Some(data) = data else {
            self.page_warn.push(format!("image data missing {}", fd.0));
            return String::new();
        };
        let mut ext = fd.1.to_lowercase();
        if ext.is_empty() {
            ext = sniff(data).to_string();
        }
        if !ext.starts_with('.') {
            ext.insert(0, '.');
        }
        let name = (self.add_resource)(data, &ext);
        format!("![{}](resources/{name})", alt.replace('\n', " "))
    }

    fn embedded_md(&mut self, ef: &Obj) -> String {
        let r = ef.props.get_ids(P_EMB_CONT).first().and_then(|i| i.as_ref());
        let fd = r.and_then(|r| self.sp?.filedecl.get(r));
        let mut name = u16s(ef.props.get_bytes(P_EMB_NAME).unwrap_or(&[]));
        if name.is_empty() {
            name = "file".to_string();
        }
        let Some(fd) = fd else { return String::new() };
        let guid = fd.0.replace("<ifndf>{", "").replace('}', "").to_lowercase();
        let one = self.one;
        let data = one.filedata.get(&guid).filter(|d| !d.is_empty());
        let Some(data) = data else {
            self.page_warn.push(format!("embedded data missing {name}"));
            return format!("`{name}` (attachment data missing)");
        };
        let mut ext = splitext(&name).to_lowercase();
        if ext.is_empty() {
            ext = fd.1.to_lowercase();
        }
        let fname = (self.add_resource)(data, &ext);
        format!("[{}](resources/{fname})", esc(&name))
    }

    /// `x` / ` ` from the first note tag state of the object, `None` when it has no state set.
    fn todo_state(&self, ob: &Obj) -> Option<char> {
        for e in ob.props.get_array(P_NOTETAG_STATES) {
            if let PropValue::Set(ps) = e {
                let done = ps.get_bytes(P_TAG_COMPLETED).and_then(le_u32).is_some_and(|v| v != 0);
                return Some(if done { 'x' } else { ' ' });
            }
        }
        None
    }

    // ---- content tree ----

    fn content_lines(&mut self, c: &Obj) -> Vec<String> {
        match c.jcid {
            J_RICHTEXT => vec![self.render_rich(c)],
            J_IMAGE => vec![self.image_md(c)],
            J_EMBEDDED => vec![self.embedded_md(c)],
            J_TABLE => self.table_lines(c),
            _ => Vec::new(),
        }
    }

    fn table_lines(&mut self, t: &Obj) -> Vec<String> {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (_, r) in self.kids(t, &[P_ELEM]) {
            let mut cells = Vec::new();
            for (_, cell) in self.kids(r, &[P_ELEM]) {
                let mut txt: Vec<String> = Vec::new();
                for (_, oe) in self.kids(cell, &[P_ELEM, P_CONTENT]) {
                    txt.extend(self.oe_lines(oe, 0, true, None).into_iter().filter(|l| !l.trim().is_empty()));
                }
                let joined: Vec<&str> = txt.iter().map(|x| x.trim()).collect();
                cells.push(joined.join("<br />").replace('|', "\\|"));
            }
            rows.push(cells);
        }
        if rows.is_empty() {
            return Vec::new();
        }
        let n = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        for r in rows.iter_mut() {
            r.resize(n, String::new());
        }
        let mut out = vec![format!("| {} |", rows[0].join(" | ")), format!("|{}|", vec!["---"; n].join("|"))];
        for r in &rows[1..] {
            out.push(format!("| {} |", r.join(" | ")));
        }
        out
    }

    fn list_marker(&self, oe: &Obj, counters: &mut HashMap<usize, u32>, depth: usize) -> Option<String> {
        let ln = self.kids(oe, &[P_LIST_NODES]);
        let (_, first) = ln.first()?;
        let f = first.props.get_bytes(P_NUMLIST_FMT).map(decode_utf16le).unwrap_or_default();
        let numbered =
            f.chars().any(|ch| (ch as u32) < 0x20 || ch == '\u{FFFD}') || f.chars().any(|ch| ch.is_alphanumeric());
        if numbered {
            let n = counters.entry(depth).or_insert(0);
            *n += 1;
            Some(format!("{n}. "))
        } else {
            Some("- ".to_string())
        }
    }

    fn oe_lines(
        &mut self,
        oe: &Obj,
        depth: usize,
        cell: bool,
        counters: Option<&mut HashMap<usize, u32>>,
    ) -> Vec<String> {
        if self.nest >= MAX_NEST || self.visits >= MAX_VISITS {
            if self.nest >= MAX_NEST {
                self.page_warn.push("content nested too deeply".to_string());
            }
            return Vec::new();
        }
        self.visits += 1;
        self.nest += 1;
        let r = self.oe_lines_inner(oe, depth, cell, counters);
        self.nest -= 1;
        r
    }

    fn oe_lines_inner(
        &mut self,
        oe: &Obj,
        depth: usize,
        cell: bool,
        counters: Option<&mut HashMap<usize, u32>>,
    ) -> Vec<String> {
        let mut own = HashMap::new();
        let counters = counters.unwrap_or(&mut own);
        let mut lines = Vec::new();
        let mut marker = if cell { None } else { self.list_marker(oe, counters, depth) };
        if marker.is_none() {
            counters.remove(&depth);
        }
        let mut body: Vec<String> = Vec::new();
        let content = self.kids(oe, &[P_CONTENT]);
        for (_, c) in &content {
            body.extend(self.content_lines(c));
        }
        let mut todo = None;
        if !cell {
            todo = self.todo_state(oe);
            if todo.is_none() {
                for (_, c) in &content {
                    todo = self.todo_state(c);
                    if todo.is_some() {
                        break;
                    }
                }
            }
        }
        let pad = "  ".repeat(depth);
        if todo.is_some() && marker.is_none() {
            marker = Some("- ".to_string());
        }
        let mlen = marker.as_deref().map_or(0, |m| m.chars().count());
        for (i, b) in body.iter().enumerate() {
            let mut seg = b.split('\n');
            let first = seg.next().unwrap_or("");
            if i == 0 {
                let mk = format!(
                    "{}{}",
                    marker.as_deref().unwrap_or(""),
                    todo.map(|t| format!("[{t}]  ")).unwrap_or_default()
                );
                lines.push(format!("{pad}{mk}{first}"));
            } else {
                lines.push(format!("{pad}{}{first}", " ".repeat(mlen)));
            }
            for s2 in seg {
                lines.push(format!("{pad}{}{s2}", " ".repeat(mlen)));
            }
        }
        let mut sub_counters = HashMap::new();
        for (_, ch) in self.kids(oe, &[P_ELEM]) {
            lines.extend(self.oe_lines(ch, depth + 1, cell, Some(&mut sub_counters)));
        }
        lines
    }

    fn outline_lines(&mut self, ol: &Obj) -> Vec<String> {
        let mut lines = Vec::new();
        let mut counters = HashMap::new();
        for (_, oe) in self.kids(ol, &[P_ELEM]) {
            lines.extend(self.oe_lines(oe, 0, false, Some(&mut counters)));
        }
        lines
    }

    // ---- pages ----

    /// Converts one page object space. Warnings go to `page_warn`.
    fn page_from_space(&mut self, gosid: &ExGuid) -> Page {
        let mut pg = Page::default();
        let Some(sp) = self.one.spaces.get(gosid) else {
            self.page_warn.push("page space missing".to_string());
            return pg;
        };
        self.sp = Some(sp);
        self.nest = 0;
        self.visits = 0;
        let first_root = sp.roots.iter().min_by_key(|(k, _)| **k).map(|(_, v)| v);
        let root = sp
            .roots
            .get(&1)
            .and_then(|r| self.get(Some(r)).map(|o| (*r, o)))
            .or_else(|| first_root.and_then(|r| self.get(Some(r)).map(|o| (*r, o))));
        let Some(root) = root else { return pg };
        let mut pnode: Option<&Obj> = None;
        let mut stack = vec![root];
        let mut seen: HashSet<ObjId> = HashSet::new();
        while let Some((id, x)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if x.jcid == J_PAGE {
                pnode = Some(x);
                break;
            }
            stack.extend(self.kids(x, &[P_CONTENT, P_ELEM]));
        }
        let Some(pnode) = pnode else {
            self.page_warn.push("no page node".to_string());
            return pg;
        };
        let pr = &pnode.props;
        pg.title = u16s(pr.get_bytes(P_CACHED_TITLE).unwrap_or(&[]));
        pg.created = pr
            .get_bytes(P_CREATED32)
            .and_then(time32)
            .or_else(|| pr.get_bytes(P_TOPO_CREATED).and_then(filetime));
        pg.updated =
            pr.get_bytes(P_MODSTAMP).and_then(filetime).or_else(|| pr.get_bytes(P_MOD32).and_then(time32));
        pg.level = pr.get_bytes(P_PAGE_LEVEL).and_then(le_u32).unwrap_or(0);
        let elems: Vec<&Obj> = self.kids(pnode, &[P_ELEM, P_CONTENT]).into_iter().map(|(_, o)| o).collect();
        let mut sorted: Vec<(f64, f64, &Obj)> = elems
            .iter()
            .filter(|e| e.jcid != J_TITLE)
            .map(|e| (round1(le_f32(e.props.get_bytes(P_OFF_V))), round1(le_f32(e.props.get_bytes(P_OFF_H))), *e))
            .collect();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for (_, _, e) in sorted {
            match e.jcid {
                J_OUTLINE => {
                    let ls = self.outline_lines(e);
                    if ls.iter().any(|l| !l.trim().is_empty()) {
                        pg.blocks.push(ls);
                    }
                }
                J_IMAGE => {
                    let l = self.image_md(e);
                    pg.blocks.push(vec![l]);
                }
                J_EMBEDDED => {
                    let l = self.embedded_md(e);
                    pg.blocks.push(vec![l]);
                }
                J_TABLE => {
                    let l = self.table_lines(e);
                    pg.blocks.push(l);
                }
                _ => {}
            }
        }
        let mut title_text = String::new();
        for e in elems.iter().filter(|e| e.jcid == J_TITLE) {
            for (_, ol) in self.kids(e, &[P_ELEM]) {
                for (_, oe) in self.kids(ol, &[P_ELEM]) {
                    let ls: Vec<String> = self.oe_lines(oe, 0, false, None);
                    let trimmed: Vec<&str> = ls.iter().map(|l| l.trim()).collect();
                    title_text.push_str(&trimmed.join(" "));
                }
            }
        }
        if pg.title.is_empty() {
            pg.title = TITLE_ESC_RE.replace_all(&title_text, "$1").trim().to_string();
        }
        pg
    }

    /// Converts every page of the section, in file order.
    pub fn section_pages(&mut self) -> Result<Vec<Page>, String> {
        let one = self.one;
        let gosid = one.root_gosid.ok_or("Converter.section_pages: the file has no root object space")?;
        let sp = one.spaces.get(&gosid).ok_or("Converter.section_pages: the root object space is missing")?;
        self.sp = Some(sp);
        let root = self
            .get(sp.roots.get(&1))
            .ok_or("Converter.section_pages: the section has no root object")?;
        let mut pages = Vec::new();
        for (_, series) in self.kids(root, &[P_ELEM]) {
            if series.jcid != J_PAGESERIES {
                continue;
            }
            let metas: Vec<&Obj> = series
                .props
                .get_ids(P_META_ABOVE)
                .iter()
                .filter_map(|m| m.as_ref().and_then(|m| sp.objects.get(m)))
                .collect();
            for (i, gs) in series.props.get_ids(P_CHILD_GRAPH).iter().enumerate() {
                let Some(gs) = gs.as_ref().filter(|g| one.spaces.contains_key(*g)) else { continue };
                let meta = metas.get(i).or(metas.first()).copied();
                self.page_warn.clear();
                let mut pg = self.page_from_space(gs);
                self.sp = Some(sp);
                if let Some(meta) = meta {
                    let t = u16s(meta.props.get_bytes(P_CACHED_TITLE).unwrap_or(&[]));
                    if !t.is_empty() {
                        pg.title = t;
                    }
                    if let Some(c) = meta.props.get_bytes(P_TOPO_CREATED).and_then(filetime) {
                        pg.created = Some(c);
                    }
                    if let Some(lv) = meta.props.get_bytes(P_PAGE_LEVEL).filter(|b| !b.is_empty()) {
                        pg.level = le_u32(lv).unwrap_or(0);
                    }
                }
                let name = if pg.title.is_empty() { UNTITLED_PAGE } else { pg.title.as_str() };
                for w in self.page_warn.drain(..) {
                    self.warnings.push(format!("{name}: {w}"));
                }
                pages.push(pg);
            }
        }
        Ok(pages)
    }
}

/// Renders a page: front matter (`title`, `updated`, `created`), then the blocks separated by `---`.
/// Picture and attachment links are relative to the page (`resources/<name>`).
pub fn page_markdown(p: &Page) -> String {
    let title = if p.title.is_empty() { UNTITLED_PAGE } else { p.title.as_str() };
    let mut out: Vec<String> = vec!["---".to_string(), format!("title: {}", front_title(title))];
    if let Some(u) = p.updated {
        out.push(format!("updated: {}", local_stamp(u)));
    }
    if let Some(c) = p.created {
        out.push(format!("created: {}", local_stamp(c)));
    }
    out.push("---".to_string());
    out.push(String::new());
    for (i, b) in p.blocks.iter().enumerate() {
        if i > 0 {
            out.push("---".to_string());
        }
        out.extend(b.iter().cloned());
    }
    let text = out.join("\n");
    strip_spans(&format!("{}\n", text.trim_end()))
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

    // --- converter tests ---

    fn oid(n: u32) -> ObjId {
        ([n as u8; 16], n)
    }

    fn pset(items: Vec<(u32, PropValue)>) -> PropSet {
        PropSet(items.into_iter().map(|(k, v)| (k & 0x7FFF_FFFF, v)).collect())
    }

    fn utf16(s: &str) -> PropValue {
        PropValue::Bytes(s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect())
    }

    fn ids(v: &[ObjId]) -> PropValue {
        PropValue::Ids(v.iter().map(|i| Some(*i)).collect())
    }

    /// Builds a space object by object; ids count up from 1.
    #[derive(Default)]
    struct TSpace {
        sp: Space,
        next: u32,
    }

    impl TSpace {
        fn add(&mut self, jcid: u32, props: Vec<(u32, PropValue)>) -> ObjId {
            self.next += 1;
            let id = oid(self.next);
            self.sp.objects.insert(id, Obj { jcid, props: pset(props) });
            id
        }

        fn rich(&mut self, text: &str, mut props: Vec<(u32, PropValue)>) -> ObjId {
            props.push((P_RICH_UNI, utf16(text)));
            self.add(J_RICHTEXT, props)
        }

        /// An OE holding the given content objects.
        fn oe(&mut self, content: &[ObjId], mut props: Vec<(u32, PropValue)>) -> ObjId {
            props.push((P_CONTENT, ids(content)));
            self.add(J_OE, props)
        }
    }

    fn one_with(sp: Space) -> OneFile {
        let g = oid(900);
        let mut one = OneFile::default();
        one.spaces.insert(g, sp);
        one.root_gosid = Some(g);
        one
    }

    fn render(ts: TSpace, rt: ObjId) -> String {
        let one = one_with(ts.sp);
        let mut cb = |_: &[u8], _: &str| "x.png".to_string();
        let mut c = Converter::new(&one, &mut cb);
        let sp = &one.spaces[&one.root_gosid.unwrap()];
        c.sp = Some(sp);
        c.render_rich(&sp.objects[&rt])
    }

    fn idx(v: &[u32]) -> PropValue {
        PropValue::Bytes(v.iter().flat_map(|x| x.to_le_bytes()).collect())
    }

    #[test]
    fn runs_formatting() {
        let mut ts = TSpace::default();
        let f1 = ts.add(0, vec![(P_BOLD, PropValue::Bool(true))]);
        let f2 = ts.add(0, vec![(P_ITALIC, PropValue::Bool(true)), (P_UNDER, PropValue::Bool(true))]);
        let f3 = ts.add(0, vec![(P_SUPER, PropValue::Bool(true))]);
        let f4 = ts.add(0, vec![(P_STRIKE, PropValue::Bool(true)), (P_SUB, PropValue::Bool(true))]);
        let rt = ts.rich(
            "abcdefgh",
            vec![(P_RUN_INDEX, idx(&[2, 4, 6])), (P_RUN_FMT, ids(&[f1, f2, f3, f4]))],
        );
        assert_eq!(render(ts, rt), "**ab***<u>cd</u>*<sup>ef</sup>~~<sub>gh</sub>~~");
    }

    #[test]
    fn underline_not_on_links() {
        let mut ts = TSpace::default();
        let f = ts.add(0, vec![(P_UNDER, PropValue::Bool(true))]);
        let rt = ts.rich(
            "\u{fddf}HYPERLINK \"https://a.b\"x",
            vec![(P_RUN_INDEX, idx(&[])), (P_RUN_FMT, ids(&[f]))],
        );
        assert_eq!(render(ts, rt), "[x](https://a.b)");
    }

    #[test]
    fn code_run() {
        let mut ts = TSpace::default();
        let f = ts.add(
            0,
            vec![
                (P_FONT, utf16("Consolas")),
                (P_BOLD, PropValue::Bool(true)),
                (P_UNDER, PropValue::Bool(true)),
            ],
        );
        let rt = ts.rich("text", vec![(P_RUN_FMT, ids(&[f]))]);
        assert_eq!(render(ts, rt), "**`text`**");
    }

    #[test]
    fn line_breaks() {
        let mut ts = TSpace::default();
        let rt = ts.rich("a\r\nb\u{000B}c\u{2028}d", vec![]);
        assert_eq!(render(ts, rt), "a  \nb  \nc  \nd");
    }

    #[test]
    fn heading_style() {
        let mut ts = TSpace::default();
        let st = ts.add(0, vec![(P_PARA_STYLE_ID, utf16("h2"))]);
        let rt = ts.rich("Title", vec![(P_PARA_STYLE, ids(&[st]))]);
        assert_eq!(render(ts, rt), "## Title");
    }

    fn lines_of(ts: TSpace, oes: &[ObjId], table: bool) -> Vec<String> {
        let one = one_with(ts.sp);
        let mut cb = |_: &[u8], _: &str| "x.png".to_string();
        let mut c = Converter::new(&one, &mut cb);
        let sp = &one.spaces[&one.root_gosid.unwrap()];
        c.sp = Some(sp);
        if table {
            return c.table_lines(&sp.objects[&oes[0]]);
        }
        let mut counters = HashMap::new();
        let mut out = Vec::new();
        for oe in oes {
            out.extend(c.oe_lines(&sp.objects[oe], 0, false, Some(&mut counters)));
        }
        out
    }

    #[test]
    fn lists_and_todos() {
        let mut ts = TSpace::default();
        let num = ts.add(J_NUMLIST, vec![(P_NUMLIST_FMT, utf16("\u{1}."))]);
        let bul = ts.add(J_NUMLIST, vec![(P_NUMLIST_FMT, utf16("\u{2022}"))]);
        let ta = ts.rich("a", vec![]);
        let tb = ts.rich("b", vec![]);
        let tc = ts.rich("c", vec![]);
        let td = ts.rich("d", vec![]);
        let b = ts.oe(&[tb], vec![(P_LIST_NODES, ids(&[bul]))]);
        let a = ts.oe(&[ta], vec![(P_LIST_NODES, ids(&[num])), (P_ELEM, ids(&[b]))]);
        let c = ts.oe(&[tc], vec![(P_LIST_NODES, ids(&[num]))]);
        let state = PropValue::Array(vec![PropValue::Set(pset(vec![(
            0x1400_346F,
            PropValue::Bytes(vec![1, 0, 0, 0]),
        )]))]);
        let d = ts.oe(&[td], vec![(P_NOTETAG_STATES, state)]);
        assert_eq!(lines_of(ts, &[a, c, d], false), vec!["1. a", "  - b", "2. c", "- [x]  d"]);
    }

    #[test]
    fn table() {
        let mut ts = TSpace::default();
        let mut rows = Vec::new();
        for r in [["x", "a|b"], ["y", ""]] {
            let mut cells = Vec::new();
            for t in r {
                let oe = if t.is_empty() {
                    ts.oe(&[], vec![])
                } else {
                    let rt = ts.rich(t, vec![]);
                    ts.oe(&[rt], vec![])
                };
                cells.push(ts.add(J_CELL, vec![(P_ELEM, ids(&[oe]))]));
            }
            rows.push(ts.add(J_ROW, vec![(P_ELEM, ids(&cells))]));
        }
        let t = ts.add(J_TABLE, vec![(P_ELEM, ids(&rows))]);
        assert_eq!(lines_of(ts, &[t], true), vec!["| x | a\\|b |", "|---|---|", "| y |  |"]);
    }

    #[test]
    fn untitled_page() {
        assert_eq!(page_markdown(&Page::default()), "---\ntitle: Untitled page\n---\n");
    }

    #[test]
    fn page_md_shape() {
        let d = |h| chrono::NaiveDate::from_ymd_opt(2024, 5, 6).unwrap().and_hms_opt(h, 0, 0).unwrap();
        let p = Page {
            title: "T: x".to_string(),
            created: Some(d(1)),
            updated: Some(d(2)),
            level: 0,
            blocks: vec![vec!["one".to_string()], vec!["![alt](resources/abc.png)".to_string()]],
        };
        let want = format!(
            "---\ntitle: \"T: x\"\nupdated: {}\ncreated: {}\n---\n\none\n---\n![alt](resources/abc.png)\n",
            local_stamp(d(2)),
            local_stamp(d(1))
        );
        assert_eq!(page_markdown(&p), want);
    }

    /// Root space (section -> series -> one page) and one page space holding `build`'s elements.
    fn section_with_page(build: impl FnOnce(&mut TSpace) -> Vec<ObjId>, title: &str) -> OneFile {
        let mut page = TSpace::default();
        let elems = build(&mut page);
        let mut props = vec![(P_ELEM, ids(&elems))];
        if !title.is_empty() {
            props.push((P_CACHED_TITLE, utf16(title)));
        }
        let pg = page.add(J_PAGE, props);
        let gs = oid(901);
        page.sp.roots.insert(1, pg);
        let mut root = TSpace::default();
        let series = root.add(J_PAGESERIES, vec![(P_CHILD_GRAPH, ids(&[gs]))]);
        let sec = root.add(J_SECTION, vec![(P_ELEM, ids(&[series]))]);
        root.sp.roots.insert(1, sec);
        let mut one = one_with(root.sp);
        one.spaces.insert(gs, page.sp);
        one
    }

    #[test]
    fn missing_picture_is_warned() {
        let one = section_with_page(
            |ts| {
                let im = ts.add(J_IMAGE, vec![]);
                vec![im]
            },
            "My page",
        );
        let mut cb = |_: &[u8], _: &str| "x.png".to_string();
        let mut c = Converter::new(&one, &mut cb);
        let pages = c.section_pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].title, "My page");
        assert_eq!(pages[0].blocks, vec![vec![String::new()]]);
        assert_eq!(c.warnings.len(), 1);
        assert!(c.warnings[0].contains("My page"), "{:?}", c.warnings);
    }

    #[test]
    fn title_from_outline_and_order() {
        let one = section_with_page(
            |ts| {
                let t1 = ts.rich("lower", vec![]);
                let o1 = ts.oe(&[t1], vec![]);
                let ol1 = ts.add(
                    J_OUTLINE,
                    vec![(P_ELEM, ids(&[o1])), (P_OFF_V, PropValue::Bytes(5.0f32.to_le_bytes().to_vec()))],
                );
                let t2 = ts.rich("upper", vec![]);
                let o2 = ts.oe(&[t2], vec![]);
                let ol2 = ts.add(
                    J_OUTLINE,
                    vec![(P_ELEM, ids(&[o2])), (P_OFF_V, PropValue::Bytes(1.0f32.to_le_bytes().to_vec()))],
                );
                let tt = ts.rich("My*title", vec![]);
                let to = ts.oe(&[tt], vec![]);
                let tol = ts.add(J_OUTLINE, vec![(P_ELEM, ids(&[to]))]);
                let title = ts.add(J_TITLE, vec![(P_ELEM, ids(&[tol]))]);
                vec![ol1, ol2, title]
            },
            "",
        );
        let mut cb = |_: &[u8], _: &str| "x.png".to_string();
        let mut c = Converter::new(&one, &mut cb);
        let pages = c.section_pages().unwrap();
        assert_eq!(pages[0].title, "My*title");
        assert_eq!(pages[0].blocks, vec![vec!["upper".to_string()], vec!["lower".to_string()]]);
    }

    #[test]
    fn round1_half_even() {
        assert_eq!(round1(0.25), 0.2);
        assert_eq!(round1(0.35), 0.3);
        assert_eq!(round1(-0.04), 0.0);
    }

    #[test]
    fn cycles_do_not_overflow() {
        let mut ts = TSpace::default();
        let a = ts.oe(&[], vec![]);
        ts.sp.objects.get_mut(&a).unwrap().props.0.insert(P_ELEM & 0x7FFF_FFFF, ids(&[a]));
        assert!(lines_of(ts, &[a], false).is_empty());
    }

    /// Converts the section in `ONENOTE_SAMPLE` and writes a plain dump to `ONENOTE_DUMP` (read-only on the source).
    #[test]
    #[ignore]
    fn real_sample_dump() {
        use md5::{Digest, Md5};
        let path = std::env::var("ONENOTE_SAMPLE").expect("set ONENOTE_SAMPLE to a .one file");
        let dump = std::env::var("ONENOTE_DUMP").expect("set ONENOTE_DUMP to an output file");
        let one = OneFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let mut cb = |d: &[u8], ext: &str| format!("{:x}{ext}", Md5::digest(d));
        let mut c = Converter::new(&one, &mut cb);
        let pages = c.section_pages().unwrap();
        let mut out = String::new();
        for p in &pages {
            let f = |t: Option<NaiveDateTime>| t.map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string()).unwrap_or_default();
            out.push_str(&format!("### {} | {} | {} | {}\n", p.title, p.level, f(p.created), f(p.updated)));
            for (i, b) in p.blocks.iter().enumerate() {
                if i > 0 {
                    out.push_str("---\n");
                }
                out.push_str(&b.join("\n"));
                out.push('\n');
            }
        }
        std::fs::write(&dump, out).unwrap();
        eprintln!("{} pages, {} warnings: {:?}", pages.len(), c.warnings.len(), &c.warnings[..c.warnings.len().min(5)]);
        for p in pages.iter().take(5) {
            eprintln!("{:?} level {} blocks {}", p.title, p.level, p.blocks.len());
        }
        let md = pages.iter().map(page_markdown).collect::<Vec<_>>();
        eprintln!("first markdown:\n{}", md.first().map(|s| &s[..s.len().min(400)]).unwrap_or(""));
    }
}
