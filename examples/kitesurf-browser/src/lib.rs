//! Offline page navigation and input for the Kitesurf WASI-P2 prototype.

use std::{cell::RefCell, rc::Rc};

use boa_engine::{Context, JsResult, JsValue, NativeFunction, Source, js_string};
use kitesurf_core::{DomHandle, wasi::register_ua_fonts};

wit_bindgen::generate!({ path: "wit", world: "browser" });

const WIDTH: u32 = 720;
const HEIGHT: u32 = 420;
const BASE: &str = "https://kitesurf.local/";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Index,
    About,
}

impl Page {
    fn resolve(path: &str) -> Result<Self, String> {
        match path.trim().trim_start_matches('/') {
            "" | "index.html" => Ok(Self::Index),
            "about.html" => Ok(Self::About),
            _ => Err("Enter a bundled path: index.html or about.html.".into()),
        }
    }

    fn location(self) -> &'static str {
        match self {
            Self::Index => "index.html",
            Self::About => "about.html",
        }
    }

    fn html(self) -> &'static str {
        match self {
            Self::Index => include_str!("index.html"),
            Self::About => include_str!("about.html"),
        }
    }
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    // Boa callbacks access only the currently executing page's DOM.
    static SCRIPT_DOCUMENT: RefCell<Option<Rc<DomHandle>>> = const { RefCell::new(None) };
}

struct Document {
    dom: Rc<DomHandle>,
    script: Context,
    page: Page,
}

impl Document {
    fn open(page: Page) -> Result<Self, String> {
        let dom = Rc::new(DomHandle::scripted(
            page.html(),
            &format!("{BASE}{}", page.location()),
        ));
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
        let mut document = Self { dom, script, page };
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
        SCRIPT_DOCUMENT.set(Some(Rc::clone(&self.dom)));
        let result = self.script.eval(Source::from_bytes(source));
        SCRIPT_DOCUMENT.set(None);
        result.map(|_| ()).map_err(|error| error.to_string())
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
    fn navigate(&mut self, path: &str) -> Result<BrowserFrame, String> {
        let page = Page::resolve(path)?;
        if page != self.current.page {
            let next = Document::open(page)?;
            self.previous = Some(std::mem::replace(&mut self.current, next));
        }
        self.frame()
    }

    fn frame(&self) -> Result<BrowserFrame, String> {
        Ok(BrowserFrame {
            location: self.current.page.location().into(),
            width: WIDTH,
            height: HEIGHT,
            rgba: self.current.dom.paint_rgba(WIDTH, HEIGHT)?,
            can_go_back: self.previous.is_some(),
        })
    }

    fn click(&mut self, x: u32, y: u32) -> Result<BrowserFrame, String> {
        let mut hit = self.current.dom.element_from_point(x as f32, y as f32);
        while let Ok(node) = u32::try_from(hit) {
            if let Some(handler) = self.current.dom.element_attribute(node, None, "onclick") {
                self.current.eval(&handler)?;
                break;
            }
            if let Some(href) = self.current.dom.element_attribute(node, Some("a"), "href") {
                return self.navigate(&href);
            }
            hit = self.current.dom.parent_id(node);
        }
        self.frame()
    }
}

struct Kitesurf;

impl Guest for Kitesurf {
    fn browser_open() -> Result<BrowserFrame, String> {
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
        let session = Session {
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

    fn browser_navigate(path: String) -> Result<BrowserFrame, String> {
        SESSION.with_borrow_mut(|session| {
            session
                .as_mut()
                .ok_or("Browser is not open")?
                .navigate(&path)
        })
    }
}

export!(Kitesurf);
