//! Offline page navigation and input for the Kitesurf WASI-P2 prototype.

use std::{cell::RefCell, rc::Rc};

use boa_engine::{Context, JsResult, JsValue, NativeFunction, Source, js_string};
use kitesurf_core::{
    DomHandle,
    wasi::{Url, register_ua_fonts, set_wasi_fetcher},
};

wit_bindgen::generate!({ path: "wit", world: "browser" });

const WIDTH: u32 = 720;
const HEIGHT: u32 = 420;
const BASE: &str = "https://kitesurf.local/";

#[derive(PartialEq, Eq)]
enum Page {
    Index,
    About,
    Web(Url),
}

impl Page {
    fn typed(address: &str) -> Result<Self, String> {
        let address = address.trim();
        match address.trim_start_matches('/') {
            "" | "index.html" => Ok(Self::Index),
            "about.html" => Ok(Self::About),
            _ => {
                let url = if address.contains("://") {
                    Url::parse(address)
                } else if address.contains('.') {
                    Url::parse(&format!("https://{address}"))
                } else {
                    return Err("Enter a website address, such as example.com.".into());
                };
                url.map(Self::Web)
                    .map_err(|_| "Enter a valid HTTPS address.".into())
            }
        }
    }

    fn location(&self) -> &str {
        match self {
            Self::Index => "index.html",
            Self::About => "about.html",
            Self::Web(url) => url.as_str(),
        }
    }

    fn html(&self) -> Option<&'static str> {
        match self {
            Self::Index => Some(include_str!("index.html")),
            Self::About => Some(include_str!("about.html")),
            Self::Web(_) => None,
        }
    }
}

fn fetch_resource(address: &str, kind: &str) -> Option<Vec<u8>> {
    let kind = match kind {
        "Stylesheet" => ResourceKind::Stylesheet,
        "Image" => ResourceKind::Image,
        _ => return None,
    };
    fetch_https(address, kind)
        .ok()
        .map(|response| response.bytes)
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    // Boa callbacks access only the currently executing page's DOM.
    static SCRIPT_DOCUMENT: RefCell<Option<Rc<DomHandle>>> = const { RefCell::new(None) };
}

struct Document {
    dom: Rc<DomHandle>,
    script: Option<Context>,
    page: Page,
    scroll_y: u32,
}

impl Document {
    fn open(page: Page) -> Result<Self, String> {
        let (dom, page) = match page {
            Page::Web(url) => {
                let response = fetch_https(url.as_str(), ResourceKind::Document)?;
                let url = Url::parse(&response.location).map_err(|_| "Invalid page address.")?;
                let dom = DomHandle::new(&String::from_utf8_lossy(&response.bytes), url.as_str());
                return Ok(Self {
                    dom: Rc::new(dom),
                    page: Page::Web(url),
                    script: None,
                    scroll_y: 0,
                });
            }
            bundled => (
                Rc::new(DomHandle::scripted(
                    bundled.html().expect("bundled page has HTML"),
                    &format!("{BASE}{}", bundled.location()),
                )),
                bundled,
            ),
        };
        let mut script = Context::default();
        for (name, count, function) in [
            ("findElement", 1, NativeFunction::from_fn_ptr(find_element)),
            ("readText", 1, NativeFunction::from_fn_ptr(read_text)),
            ("writeText", 2, NativeFunction::from_fn_ptr(write_text)),
        ] {
            script
                .register_global_builtin_callable(js_string!(name), count, function)
                .map_err(|error| error.to_string())?;
        }
        let mut document = Self {
            dom,
            script: Some(script),
            page,
            scroll_y: 0,
        };
        document.eval(include_str!("dom.js"))?;
        let nodes = document
            .dom
            .query_selector_all("script")
            .map_err(|_| "Cannot find page scripts")?;
        for node in nodes {
            let source = document.dom.text_content(node as u32);
            document.eval(&source)?;
        }
        Ok(document)
    }

    fn eval(&mut self, source: &str) -> Result<(), String> {
        let Some(script) = &mut self.script else {
            return Ok(());
        };
        SCRIPT_DOCUMENT.set(Some(Rc::clone(&self.dom)));
        let result = script.eval(Source::from_bytes(source));
        SCRIPT_DOCUMENT.set(None);
        result.map(|_| ()).map_err(|error| error.to_string())
    }

    fn link(&self, href: &str) -> Result<Page, String> {
        let Page::Web(url) = &self.page else {
            return Page::typed(href);
        };
        let base = self
            .dom
            .query_selector("base[href]")
            .ok()
            .and_then(|node| u32::try_from(node).ok())
            .and_then(|node| self.dom.element_attribute(node, Some("base"), "href"))
            .and_then(|base| url.join(&base).ok());
        base.as_ref()
            .unwrap_or(url)
            .join(href)
            .map(Page::Web)
            .map_err(|_| "This link has an invalid address.".into())
    }

    fn paint(&mut self) -> Result<Vec<u8>, String> {
        let (pixels, y) = self
            .dom
            .paint_rgba(WIDTH, HEIGHT, f64::from(self.scroll_y))?;
        self.scroll_y = y;
        Ok(pixels)
    }
}

fn find_element(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    Ok(SCRIPT_DOCUMENT
        .with_borrow(|dom| dom.as_ref().map_or(-1, |dom| dom.get_element_by_id(&id)))
        .into())
}

fn read_text(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node = args.first().cloned().unwrap_or_default().to_u32(context)?;
    let text = SCRIPT_DOCUMENT.with_borrow(|dom| {
        dom.as_ref()
            .map_or_else(String::new, |dom| dom.text_content(node))
    });
    Ok(js_string!(text).into())
}

fn write_text(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let node = args.first().cloned().unwrap_or_default().to_u32(context)?;
    let text = args
        .get(1)
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    SCRIPT_DOCUMENT.with_borrow(|dom| {
        if let Some(dom) = dom {
            dom.set_node_text(node, &text);
        }
    });
    Ok(JsValue::undefined())
}

struct Session {
    current: Document,
    previous: Option<Document>,
}

impl Session {
    fn navigate(&mut self, page: Page) -> Result<BrowserFrame, String> {
        if page != self.current.page {
            let mut next = Document::open(page)?;
            let rgba = next.paint()?;
            self.previous = Some(std::mem::replace(&mut self.current, next));
            return Ok(self.frame_with_pixels(rgba));
        }
        self.frame()
    }

    fn frame(&mut self) -> Result<BrowserFrame, String> {
        let rgba = self.current.paint()?;
        Ok(self.frame_with_pixels(rgba))
    }

    fn frame_with_pixels(&self, rgba: Vec<u8>) -> BrowserFrame {
        BrowserFrame {
            location: self.current.page.location().into(),
            width: WIDTH,
            height: HEIGHT,
            rgba,
            can_go_back: self.previous.is_some(),
        }
    }

    fn click(&mut self, x: u32, y: u32) -> Result<BrowserFrame, String> {
        // The engine hit test expects document coordinates.
        let y = y.saturating_add(self.current.scroll_y);
        let mut hit = self.current.dom.element_from_point(x as f32, y as f32);
        while let Ok(node) = u32::try_from(hit) {
            if self.current.script.is_some()
                && let Some(handler) = self.current.dom.element_attribute(node, None, "onclick")
            {
                self.current.eval(&handler)?;
                break;
            }
            if let Some(href) = self.current.dom.element_attribute(node, Some("a"), "href") {
                return self.navigate(self.current.link(&href)?);
            }
            hit = self.current.dom.parent_id(node);
        }
        self.frame()
    }
}

struct Kitesurf;

impl Guest for Kitesurf {
    fn browser_open() -> Result<BrowserFrame, String> {
        set_wasi_fetcher(fetch_resource);
        // Font files are supplied by the pinned Kitesurf checkout.
        register_ua_fonts(
            include_bytes!("../../engine/public/fonts/NotoSans.ttf").to_vec(),
            include_bytes!("../../engine/public/fonts/NotoSans-Italic.ttf").to_vec(),
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            "noto",
        );
        let mut session = Session {
            current: Document::open(Page::Index)?,
            previous: None,
        };
        let frame = session.frame()?;
        SESSION.set(Some(session));
        Ok(frame)
    }

    fn browser_click(x: u32, y: u32) -> Result<BrowserFrame, String> {
        SESSION
            .with_borrow_mut(|session| session.as_mut().ok_or("Browser is not open")?.click(x, y))
    }

    fn browser_back() -> Result<BrowserFrame, String> {
        SESSION.with_borrow_mut(|session| {
            let session = session.as_mut().ok_or("Browser is not open")?;
            if let Some(previous) = session.previous.take() {
                session.current = previous;
            }
            session.frame()
        })
    }

    fn browser_navigate(address: String) -> Result<BrowserFrame, String> {
        SESSION.with_borrow_mut(|session| {
            session
                .as_mut()
                .ok_or("Browser is not open")?
                .navigate(Page::typed(&address)?)
        })
    }

    fn browser_scroll(delta_y: i32) -> Result<BrowserFrame, String> {
        SESSION.with_borrow_mut(|session| {
            let session = session.as_mut().ok_or("Browser is not open")?;
            session.current.scroll_y = session.current.scroll_y.saturating_add_signed(delta_y);
            session.frame()
        })
    }
}

export!(Kitesurf);
