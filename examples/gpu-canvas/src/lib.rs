//! Standalone WASI-P2 component for the vKore canvas prototype.
wit_bindgen::generate!({
    path: "../../crates/vkored/wit/gpu-canvas",
    world: "canvas",
});

struct Animation;

impl Guest for Animation {
    fn shader_source() -> String {
        include_str!("canvas.wgsl").into()
    }

    fn render(elapsed_seconds: f32) {
        draw(elapsed_seconds * 1.25);
    }
}

export!(Animation);
