//! Host for the offline browser component. The engine and page pixels live
//! inside a Wasmtime store, with no directory, network, or process grants.

use wasmtime::{Config, Engine, OptLevel, Store, StoreLimits, StoreLimitsBuilder, component};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::plugins::Bundle;

component::bindgen!({
    path: "wit/browser",
    world: "browser",
});

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

pub struct Session {
    browser: Browser,
    store: Store<Context>,
}

impl Session {
    pub fn open(bundle: &Bundle) -> wasmtime::Result<(Self, BrowserFrame)> {
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
        let mut store = Store::new(
            &engine,
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
        let browser = Browser::instantiate(&mut store, &component, &linker)?;
        store.set_fuel(FUEL)?;
        let frame = validate_frame(browser.call_browser_open(&mut store)?)?;
        Ok((Self { browser, store }, frame))
    }

    pub fn click(&mut self, x: u32, y: u32) -> wasmtime::Result<BrowserFrame> {
        self.store.set_fuel(FUEL)?;
        validate_frame(self.browser.call_browser_click(&mut self.store, x, y)?)
    }

    pub fn back(&mut self) -> wasmtime::Result<BrowserFrame> {
        self.store.set_fuel(FUEL)?;
        validate_frame(self.browser.call_browser_back(&mut self.store)?)
    }
}

fn validate_frame(frame: Result<BrowserFrame, String>) -> wasmtime::Result<BrowserFrame> {
    let frame = frame.map_err(|error| {
        wasmtime::format_err!("{}", error.chars().take(512).collect::<String>())
    })?;
    wasmtime::ensure!(
        (1..=MAX_WIDTH).contains(&frame.width)
            && (1..=MAX_HEIGHT).contains(&frame.height)
            && frame.rgba.len() == (frame.width * frame.height * 4) as usize
            && frame.location.len() <= 256,
        "invalid browser frame"
    );
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_kitesurf_runs_script_and_follows_links() {
        let Some(path) = std::env::var_os("VKORE_TEST_BROWSER") else {
            return;
        };
        let bundle = Bundle::open(path.as_ref()).expect("read Kitesurf component");
        let (mut browser, first) = Session::open(&bundle).expect("start Kitesurf");
        assert_eq!(first.location, "index.html");
        assert!(!first.can_go_back);
        let changed = browser.click(155, 312).expect("click page button");
        assert_eq!(changed.location, "index.html");
        assert!(
            changed.rgba != first.rgba,
            "JavaScript should change the DOM"
        );
        let linked = browser.click(283, 312).expect("follow bundled link");
        assert_eq!(linked.location, "about.html");
        assert!(linked.can_go_back);
        let previous = browser.back().expect("go back");
        assert_eq!(previous.location, changed.location);
        assert!(
            previous.rgba == changed.rgba,
            "Back should restore the page state"
        );
    }
}
