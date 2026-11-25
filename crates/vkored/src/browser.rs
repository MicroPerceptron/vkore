//! Host for the offline browser component. The engine and page pixels live
//! inside a Wasmtime store, with no directory, network, or process grants.

use wasmtime::{Config, Engine, OptLevel, Store, StoreLimits, StoreLimitsBuilder, component};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::plugins::Bundle;

#[derive(component::ComponentType, component::Lift)]
#[component(record)]
pub struct BrowserFrame {
    pub location: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    #[component(name = "can-go-back")]
    pub can_go_back: bool,
}

const FUEL: u64 = 250_000_000;
const MAX_WIDTH: u32 = 1920;
const MAX_HEIGHT: u32 = 1080;

struct Context {
    wasi: WasiCtx,
    resources: component::ResourceTable,
    limits: StoreLimits,
}

impl WasiView for Context {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.resources,
        }
    }
}

/// Reused by each tab. Each opened session has its own store and WASI context.
pub struct Compiled {
    component: component::Component,
    linker: component::Linker<Context>,
}

type Page = Result<BrowserFrame, String>;
pub type Operation = wasmtime::Result<Page>;
type PageCall<Params> = component::TypedFunc<Params, (Page,)>;

pub struct Session {
    click: PageCall<(u32, u32)>,
    back: PageCall<()>,
    navigate: Option<PageCall<(String,)>>,
    store: Store<Context>,
}

impl Compiled {
    pub fn new(bundle: &Bundle) -> wasmtime::Result<Self> {
        wasmtime::ensure!(
            bundle.supports_browser(),
            "expected a vKore browser component"
        );
        let mut config = Config::new();
        config.consume_fuel(true);
        // The component already carries optimized Wasm. Avoid expensive
        // native optimization on the browser's cold path.
        config.cranelift_opt_level(OptLevel::None);
        let engine = Engine::new(&config)?;
        let component = component::Component::from_binary(&engine, bundle.bytes())?;
        let mut linker = component::Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        Ok(Self { component, linker })
    }

    pub fn open(&self) -> wasmtime::Result<(Session, BrowserFrame)> {
        let mut store = Store::new(
            self.component.engine(),
            Context {
                wasi: WasiCtx::builder().allow_tcp(false).allow_udp(false).build(),
                resources: component::ResourceTable::new(),
                limits: StoreLimitsBuilder::new()
                    .memory_size(128 * 1024 * 1024)
                    .table_elements(16384)
                    .instances(16)
                    .memories(2)
                    .tables(4)
                    .build(),
            },
        );
        store.limiter(|context| &mut context.limits);
        store.set_fuel(FUEL)?;
        let instance = self.linker.instantiate(&mut store, &self.component)?;
        let open: PageCall<()> = instance.get_typed_func(&mut store, "browser-open")?;
        let click = instance.get_typed_func(&mut store, "browser-click")?;
        let back = instance.get_typed_func(&mut store, "browser-back")?;
        let navigate = instance
            .get_func(&mut store, "browser-navigate")
            .map(|function| function.typed(&store))
            .transpose()?;

        store.set_fuel(FUEL)?;
        let frame = validate_frame(open.call(&mut store, ())?.0)?
            .map_err(|error| wasmtime::format_err!("{error}"))?;
        Ok((
            Session {
                click,
                back,
                navigate,
                store,
            },
            frame,
        ))
    }
}

impl Session {
    pub fn can_navigate(&self) -> bool {
        self.navigate.is_some()
    }

    pub fn click(&mut self, x: u32, y: u32) -> Operation {
        self.store.set_fuel(FUEL)?;
        validate_frame(self.click.call(&mut self.store, (x, y))?.0)
    }

    pub fn back(&mut self) -> Operation {
        self.store.set_fuel(FUEL)?;
        validate_frame(self.back.call(&mut self.store, ())?.0)
    }

    pub fn navigate(&mut self, path: String) -> Operation {
        let Some(navigate) = self.navigate else {
            return Ok(Err(
                "This browser bundle supports link navigation only.".into()
            ));
        };
        if path.len() > 256 {
            return Ok(Err("Enter a path of at most 256 bytes.".into()));
        }
        self.store.set_fuel(FUEL)?;
        validate_frame(navigate.call(&mut self.store, (path,))?.0)
    }
}

fn validate_frame(page: Page) -> Operation {
    let frame = match page {
        Ok(frame) => frame,
        Err(error) => return Ok(Err(error.chars().take(512).collect())),
    };
    wasmtime::ensure!(
        (1..=MAX_WIDTH).contains(&frame.width)
            && (1..=MAX_HEIGHT).contains(&frame.height)
            && frame.rgba.len() == (frame.width * frame.height * 4) as usize
            && frame.location.len() <= 256,
        "invalid browser frame"
    );
    Ok(Ok(frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_bundle_can_open_without_typed_navigation() {
        let Some(path) = std::env::var_os("VKORE_TEST_BROWSER_LEGACY") else {
            return;
        };
        let bundle = Bundle::open(path.as_ref()).expect("read legacy component");
        let browser = Compiled::new(&bundle).expect("compile legacy component");
        let (mut tab, first) = browser.open().expect("open legacy tab");
        assert!(!tab.can_navigate());
        assert_eq!(first.location, "index.html");
        let linked = tab.click(283, 312).unwrap().unwrap();
        assert_eq!(linked.location, "about.html");
    }

    #[test]
    fn bundled_kitesurf_runs_script_and_follows_links() {
        let Some(path) = std::env::var_os("VKORE_TEST_BROWSER") else {
            return;
        };
        let bundle = Bundle::open(path.as_ref()).expect("read Kitesurf component");
        let compiled = Compiled::new(&bundle).expect("compile Kitesurf");
        let (mut browser, first) = compiled.open().expect("start Kitesurf");
        assert_eq!(first.location, "index.html");
        assert!(!first.can_go_back);
        let changed = browser
            .click(155, 312)
            .expect("click page button")
            .expect("page frame");
        assert_eq!(changed.location, "index.html");
        assert!(
            changed.rgba != first.rgba,
            "JavaScript should change the DOM"
        );
        let linked = browser
            .click(283, 312)
            .expect("follow bundled link")
            .expect("page frame");
        assert_eq!(linked.location, "about.html");
        assert!(linked.can_go_back);
        let previous = browser.back().expect("go back").expect("page frame");
        assert_eq!(previous.location, changed.location);
        assert!(
            previous.rgba == changed.rgba,
            "Back should restore the page state"
        );

        let (mut second, fresh) = compiled.open().expect("open another tab");
        assert_eq!(fresh.location, "index.html");
        assert!(fresh.rgba == first.rgba, "new tab gets independent state");
        assert!(second.can_navigate(), "use the current Kitesurf bundle");
        assert!(
            second
                .navigate("https://example.com/".into())
                .unwrap()
                .is_err()
        );
        let about = second.navigate("/about.html".into()).unwrap().unwrap();
        assert_eq!(about.location, "about.html");
        assert_eq!(second.back().unwrap().unwrap().location, "index.html");
        let twice = browser.click(155, 312).unwrap().unwrap();
        assert!(twice.rgba != changed.rgba, "other tab retains its state");
    }
}
