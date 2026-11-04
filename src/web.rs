//! The page area: a WebView2 view that shows the page formatted (bold, tables, pictures) and lets
//! the person edit it in place. The page logic is in `assets/editor.js`; this module hosts the view,
//! serves its files and passes messages between the view and the app.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use eframe::egui;
use wry::http::{Request, Response, header::CONTENT_TYPE};
use wry::raw_window_handle::HasWindowHandle;
use wry::{WebContext, WebView, WebViewBuilder, dpi};

const ORIGIN: &str = "http://notecraft.localhost";
const EDITOR_HTML: &str = include_str!("../assets/editor.html");
const EDITOR_JS: &str = include_str!("../assets/editor.js");
const EDITOR_CSS: &str = include_str!("../assets/editor.css");

/// What the page view tells the app.
pub enum WebMsg {
    Ready,
    Loaded,
    /// The page was edited; the whole page as Markdown.
    Changed(String),
    /// A checkbox was ticked while reading (n-th checkbox, new state).
    Toggle(usize, bool),
    /// A link was clicked.
    Open(String),
    /// A keyboard shortcut pressed inside the page.
    Key(String),
    /// A picture was pasted or dropped while editing.
    Image { mime: String, data: Vec<u8> },
    Error(String),
}

pub struct PageWeb {
    view: WebView,
    _context: WebContext,
    inbox: Rc<RefCell<Vec<WebMsg>>>,
    pub ready: bool,
    /// The page and text (hash) the view shows.
    pub shown: Option<(PathBuf, u64)>,
    pub editable: bool,
    pub highlight: String,
    pub loaded_at: Option<std::time::Instant>,
    bounds: Option<[i32; 4]>,
    visible: bool,
}

impl PageWeb {
    pub fn new(window: &impl HasWindowHandle, ctx: &egui::Context) -> Result<PageWeb, String> {
        let data = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("NoteCraft")
            .join("WebView2");
        let mut context = WebContext::new(Some(data));
        let inbox: Rc<RefCell<Vec<WebMsg>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = inbox.clone();
        let repaint = ctx.clone();
        let view = WebViewBuilder::new_with_web_context(&mut context)
            .with_custom_protocol("notecraft".into(), |_id, req| serve(&req))
            .with_ipc_handler(move |req: Request<String>| {
                if let Some(m) = parse(req.body()) {
                    sink.borrow_mut().push(m);
                }
                repaint.request_repaint();
            })
            .with_navigation_handler(|url| url.starts_with(ORIGIN))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_visible(false)
            .with_background_color((255, 255, 255, 255))
            .with_url(format!("{ORIGIN}/editor.html"))
            .build_as_child(window)
            .map_err(|e| format!("The page view could not start: {e}"))?;
        Ok(PageWeb {
            view,
            _context: context,
            inbox,
            ready: false,
            shown: None,
            editable: false,
            highlight: String::new(),
            loaded_at: None,
            bounds: None,
            visible: false,
        })
    }

    pub fn take_messages(&self) -> Vec<WebMsg> {
        std::mem::take(&mut *self.inbox.borrow_mut())
    }

    /// Puts the view over `rect` (egui points), or hides it.
    pub fn place(&mut self, rect: Option<egui::Rect>, pixels_per_point: f32) {
        let Some(rect) = rect.filter(|r| r.width() > 4.0 && r.height() > 4.0) else {
            if self.visible {
                let _ = self.view.set_visible(false);
                self.visible = false;
            }
            return;
        };
        let x = (rect.min.x * pixels_per_point).round() as i32;
        let y = (rect.min.y * pixels_per_point).round() as i32;
        let w = (rect.width() * pixels_per_point).round().max(1.0) as i32;
        let h = (rect.height() * pixels_per_point).round().max(1.0) as i32;
        let b = [x, y, w, h];
        if self.bounds != Some(b) {
            let _ = self.view.set_bounds(wry::Rect {
                position: dpi::PhysicalPosition::new(x, y).into(),
                size: dpi::PhysicalSize::new(w as u32, h as u32).into(),
            });
            self.bounds = Some(b);
        }
        if !self.visible {
            let _ = self.view.set_visible(true);
            self.visible = true;
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Where the view is, in physical pixels: x, y, width, height.
    pub fn bounds(&self) -> Option<[i32; 4]> {
        self.bounds
    }

    pub fn load(&mut self, path: &Path, hash: u64, html: &str, base: &str, editable: bool, keep_scroll: bool) {
        let js = format!("nc.load({}, {}, {editable}, {keep_scroll})", js_str(html), js_str(base));
        let _ = self.view.evaluate_script(&js);
        self.shown = Some((path.to_path_buf(), hash));
        self.editable = editable;
    }

    pub fn set_editable(&mut self, on: bool) {
        let _ = self.view.evaluate_script(&format!("nc.setEditable({on})"));
        self.editable = on;
    }

    pub fn set_highlight(&mut self, query: &str) {
        let words: Vec<String> = query.split_whitespace().map(js_str).collect();
        let _ = self.view.evaluate_script(&format!("nc.highlight([{}])", words.join(",")));
        self.highlight = query.to_string();
    }

    pub fn insert_image(&self, src: &str) {
        let _ = self.view.evaluate_script(&format!("nc.insertImage({})", js_str(src)));
    }

    pub fn focus(&self) {
        let _ = self.view.focus();
        let _ = self.view.evaluate_script("nc.focus()");
    }

    /// Gives the keyboard back to the rest of the window.
    pub fn focus_parent(&self) {
        let _ = self.view.focus_parent();
    }

    /// Takes a PNG picture of what the view shows (for --screenshot); arrives later in `out`.
    #[cfg(windows)]
    pub fn capture(&self, out: Arc<Mutex<Option<Vec<u8>>>>) {
        use webview2_com::CapturePreviewCompletedHandler;
        use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
        use windows::Win32::System::Com::STREAM_SEEK_SET;
        use windows::Win32::UI::Shell::SHCreateMemStream;
        use wry::WebViewExtWindows as _;

        let Some(stream) = (unsafe { SHCreateMemStream(None) }) else { return };
        let reader = stream.clone();
        let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
            if result.is_ok() {
                let mut data = Vec::new();
                unsafe {
                    let _ = reader.Seek(0, STREAM_SEEK_SET, None);
                    let mut buf = vec![0u8; 1 << 16];
                    loop {
                        let mut read = 0u32;
                        let hr = reader.Read(buf.as_mut_ptr().cast(), buf.len() as u32, Some(&mut read));
                        if hr.is_err() || read == 0 {
                            break;
                        }
                        data.extend_from_slice(&buf[..read as usize]);
                    }
                }
                *out.lock().unwrap() = Some(data);
            }
            Ok(())
        }));
        unsafe {
            let _ = self.view.webview().CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler);
        }
    }

    #[cfg(not(windows))]
    pub fn capture(&self, _out: Arc<Mutex<Option<Vec<u8>>>>) {}
}

/// Base address for relative pictures on a page in `dir`.
pub fn base_url(dir: &Path) -> String {
    let s = dir.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/UNC/").map(|r| format!("//{r}")).unwrap_or_else(|| s.strip_prefix("//?/").unwrap_or(&s).to_string());
    format!("{ORIGIN}/file/{}/", percent_encode(s.trim_end_matches('/')))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A JavaScript string literal.
fn js_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn parse(s: &str) -> Option<WebMsg> {
    let (kind, rest) = s.split_once('\n').unwrap_or((s, ""));
    Some(match kind {
        "ready" => WebMsg::Ready,
        "loaded" => WebMsg::Loaded,
        "changed" => WebMsg::Changed(rest.to_string()),
        "toggle" => {
            let (n, on) = rest.split_once('\n')?;
            WebMsg::Toggle(n.trim().parse().ok()?, on.trim() == "1")
        }
        "open" => WebMsg::Open(rest.to_string()),
        "key" => WebMsg::Key(rest.to_string()),
        "image" => {
            let (mime, data) = rest.split_once('\n')?;
            let data = base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?;
            WebMsg::Image { mime: mime.to_string(), data }
        }
        "error" => WebMsg::Error(rest.to_string()),
        _ => return None,
    })
}

fn respond(mime: &str, body: Cow<'static, [u8]>) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .header(CONTENT_TYPE, mime)
        .header("Cache-Control", "no-store")
        .body(body)
        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&[])))
}

fn not_found() -> Response<Cow<'static, [u8]>> {
    Response::builder().status(404).body(Cow::Borrowed(&[][..])).unwrap_or_else(|_| Response::new(Cow::Borrowed(&[])))
}

/// The view's own files, and the pictures of the notes (nothing else is handed out).
fn serve(req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    match req.uri().path() {
        "/editor.html" => respond("text/html; charset=utf-8", Cow::Borrowed(EDITOR_HTML.as_bytes())),
        "/editor.js" => respond("text/javascript; charset=utf-8", Cow::Borrowed(EDITOR_JS.as_bytes())),
        "/editor.css" => respond("text/css; charset=utf-8", Cow::Borrowed(EDITOR_CSS.as_bytes())),
        p if p.starts_with("/file/") => {
            let path = PathBuf::from(percent_decode(&p["/file/".len()..]));
            match crate::render::image_mime(&path).and_then(|m| std::fs::read(&path).ok().map(|b| (m, b))) {
                Some((mime, bytes)) => respond(mime, Cow::Owned(bytes)),
                None => not_found(),
            }
        }
        _ => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip() {
        let base = base_url(Path::new(r"H:\Notes\My work č"));
        assert_eq!(base, "http://notecraft.localhost/file/H:/Notes/My%20work%20%C4%8D/");
        assert_eq!(percent_decode("H:/Notes/My%20work%20%C4%8D/a.png"), "H:/Notes/My work č/a.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(js_str("a\"b\\c\nd</script>"), "\"a\\\"b\\\\c\\nd</script>\"");
    }

    #[test]
    fn messages_are_read() {
        assert!(matches!(parse("changed\n# T\n\ntext"), Some(WebMsg::Changed(s)) if s == "# T\n\ntext"));
        assert!(matches!(parse("toggle\n3\n1"), Some(WebMsg::Toggle(3, true))));
        assert!(matches!(parse("key\nsave"), Some(WebMsg::Key(k)) if k == "save"));
        assert!(parse("nonsense").is_none());
    }
}
