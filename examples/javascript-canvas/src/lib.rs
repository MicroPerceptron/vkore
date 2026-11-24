//! WASI-P2 component that runs JavaScript inside its own Boa engine.

use std::cell::RefCell;

use boa_engine::{Context, JsObject, JsValue, Source};

wit_bindgen::generate!({
    path: "../../crates/vkored/wit/gpu-canvas",
    world: "canvas",
});

struct Script {
    context: Context,
    frame: JsObject,
}

impl Script {
    fn new() -> Self {
        let mut context = Context::default();
        let frame = context
            .eval(Source::from_bytes(include_bytes!("animation.js")))
            .expect("valid bundled animation script")
            .as_callable()
            .expect("animation script returns a function");
        Self { context, frame }
    }

    fn phase(&mut self, seconds: f32) -> f32 {
        self.frame
            .call(
                &JsValue::undefined(),
                &[JsValue::new(f64::from(seconds))],
                &mut self.context,
            )
            .expect("animation frame succeeds")
            .as_number()
            .expect("animation frame returns a number") as f32
    }
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::new(Script::new());
}

struct JavascriptCanvas;

impl Guest for JavascriptCanvas {
    fn shader_source() -> String {
        // Initialize Boa once before the first frame.
        SCRIPT.with(|_| {});
        include_str!("../../gpu-canvas/src/canvas.wgsl").into()
    }

    fn render(elapsed_seconds: f32) {
        draw(SCRIPT.with(|script| script.borrow_mut().phase(elapsed_seconds)));
    }
}

export!(JavascriptCanvas);
