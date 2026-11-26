//! Opt-in smoke test for a real Servo helper and the desktop compositor.
//! VKORE_SERVO=/absolute/path/to/vkore-servo cargo run -p vkore-servo-wire --features presentation --example servo_surface_smoke
use std::{
    io::{self, Write},
    num::NonZeroU32,
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};
use vkore_servo_wire::presentation::{Presentation, TileRect};
use vkore_servo_wire::{self as wire, Command as Input, Event};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

const TILE: TileRect = TileRect {
    x: 12.,
    y: 18.,
    width: 640.,
    height: 360.,
};

enum Reply {
    Notice(Event),
    #[cfg(target_os = "linux")]
    Buffer(u64, wire::DmaBuffer, std::os::fd::OwnedFd),
}
struct Test {
    native: Presentation,
    child: Child,
    input: Box<dyn Write>,
    messages: mpsc::Receiver<io::Result<Reply>>,
    window: Option<Rc<Window>>,
    start: Instant,
    url: String,
    navigated: bool,
    loaded: bool,
    location: String,
    painted: bool,
    passed: bool,
    frames: u32,
    dirty: bool,
    waiting: bool,
}
impl Test {
    fn send(&mut self, c: Input) {
        wire::write(&mut self.input, &c).unwrap();
        self.input.flush().unwrap();
    }
    fn tick(&mut self, events: &ActiveEventLoop) {
        if self.start.elapsed() > Duration::from_secs(30) {
            events.exit();
            return;
        }
        while let Ok(reply) = self.messages.try_recv() {
            match reply.unwrap() {
                Reply::Notice(Event::Ready(wire::VERSION, renderer, surface)) => {
                    println!("Renderer {renderer}, surface {surface:?}");
                    self.native.attach(surface).unwrap();
                    self.native
                        .place(self.window.as_ref().unwrap(), 1., None)
                        .unwrap();
                    self.send(Input::Open(0));
                }
                Reply::Notice(Event::Changed(_)) => self.dirty = true,
                Reply::Notice(Event::Loading(_, wire::Load::Done)) if !self.navigated => {
                    self.navigated = true;
                    let scale = self.window.as_ref().unwrap().scale_factor() as f32;
                    self.send(Input::Resize(
                        0,
                        (TILE.width * scale) as u32,
                        (TILE.height * scale) as u32,
                        scale,
                    ));
                    self.send(Input::Navigate(0, self.url.clone()));
                }
                Reply::Notice(Event::Location(_, url, _, _)) => {
                    self.location = url;
                }
                Reply::Notice(Event::Loading(_, wire::Load::Done))
                    if self.location.starts_with("https://") =>
                {
                    self.loaded = true;
                    self.dirty = true;
                }
                Reply::Notice(Event::Presented(_)) => {
                    #[cfg(target_os = "macos")]
                    self.native
                        .receive_frame()
                        .expect("import browser IOSurface");
                    self.painted = true;
                }
                Reply::Notice(Event::Error(reason)) => panic!("{reason}"),
                #[cfg(target_os = "linux")]
                Reply::Buffer(id, f, fd) => {
                    assert_eq!(id, 0);
                    self.native.import(fd, f).expect("import DMA-BUF");
                }
                _ => {}
            }
        }
        #[cfg(target_os = "linux")]
        {
            let progress = self
                .native
                .poll()
                .expect("Wayland compositor accepts surface");
            self.painted |= progress.imported;
            debug_assert!(!progress.pending || self.waiting);
        }
        if self.painted {
            self.waiting = false;
            self.frames += 1;
            self.native
                .place(
                    self.window.as_ref().unwrap(),
                    self.window.as_ref().unwrap().scale_factor(),
                    Some(TILE),
                )
                .unwrap();
            self.send(Input::ShowSurface(true));
            self.painted = false;
            if self.loaded {
                #[cfg(target_os = "macos")]
                if !has_page_pixels(self.window.as_ref().unwrap()) {
                    self.dirty = true;
                    return;
                }
                self.passed = true;
                events.exit();
            }
        }
        if self.dirty && !self.waiting {
            self.waiting = true;
            self.dirty = false;
            self.send(Input::Paint(0));
        }
    }
}
impl Drop for Test {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl ApplicationHandler for Test {
    fn resumed(&mut self, e: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let w = Rc::new(
            e.create_window(
                Window::default_attributes()
                    .with_title("vKore GPU smoke test")
                    .with_inner_size(winit::dpi::LogicalSize::new(700, 420)),
            )
            .unwrap(),
        );
        let ctx = softbuffer::Context::new(w.clone()).unwrap();
        let mut s = softbuffer::Surface::new(&ctx, w.clone()).unwrap();
        let size = w.inner_size();
        s.resize(
            NonZeroU32::new(size.width).unwrap(),
            NonZeroU32::new(size.height).unwrap(),
        )
        .unwrap();
        let mut b = s.buffer_mut().unwrap();
        b.fill(0x202020);
        // Red reference stripe marks the top of the host window.
        b[..size.width as usize * 8].fill(0xff0000);
        w.pre_present_notify();
        b.present().unwrap();
        self.window = Some(w);
    }
    fn about_to_wait(&mut self, e: &ActiveEventLoop) {
        self.tick(e);
        e.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(16)));
    }
    fn window_event(&mut self, e: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested) {
            e.exit();
        }
    }
}
/// Render only this test window's layer tree. A presentation acknowledgment
/// alone also succeeds when the browser tile is blank.
#[cfg(target_os = "macos")]
fn has_page_pixels(window: &Window) -> bool {
    use objc2_app_kit::NSView;
    use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
        unreachable!()
    };
    // Winit owns this view and calls us on the AppKit main thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let layer = view.layer().unwrap();
    let bounds = layer.bounds();
    let (w, h) = (
        bounds.size.width.round() as usize,
        bounds.size.height.round() as usize,
    );
    if w < (TILE.x + TILE.width) as usize
        || h < (TILE.y + TILE.height) as usize
        || w > 2048
        || h > 2048
    {
        return false;
    }
    let mut rgba = vec![0_u8; w * h * 4];
    let rgb = CGColorSpace::new_device_rgb().unwrap();
    // The context cannot outlive the buffer. Premultiplied-last RGBA.
    let ctx =
        unsafe { CGBitmapContextCreate(rgba.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&rgb), 1) }
            .unwrap();
    if layer.isGeometryFlipped() {
        objc2_core_graphics::CGContext::translate_ctm(Some(&ctx), 0., h as f64);
        objc2_core_graphics::CGContext::scale_ctm(Some(&ctx), 1., -1.);
    }
    layer.renderInContext(&ctx);
    drop(ctx);
    let mut dark = 0;
    let mut light = 0;
    for row in rgba
        .chunks_exact(w * 4)
        .skip(TILE.y as usize + 1)
        .take(TILE.height as usize - 2)
    {
        for pixel in row
            .as_chunks::<4>()
            .0
            .iter()
            .skip(TILE.x as usize + 1)
            .take(TILE.width as usize - 2)
        {
            if pixel[3] > 245 {
                let luma = (u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2])) / 3;
                dark += usize::from(luma < 128);
                light += usize::from(luma > 180);
            }
        }
    }
    if let Some(path) = std::env::var_os("VKORE_TEST_TILE_PPM") {
        let mut f = std::fs::File::create(path).unwrap();
        write!(f, "P6\n{w} {h}\n255\n").unwrap();
        let bytes: Vec<_> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| p[..3].iter().copied())
            .collect();
        f.write_all(&bytes).unwrap();
    }
    rgba[..3] == [255, 0, 0] && dark >= 30 && light >= 30
}

fn main() {
    let events = EventLoop::new().unwrap();
    let mut cmd = Command::new(
        std::env::var_os("VKORE_SERVO").expect("set VKORE_SERVO to the helper executable"),
    );
    let native = Presentation::prepare(&mut cmd).unwrap();
    #[cfg(target_os = "linux")]
    let (input, fds) = {
        let (host, child) = std::os::unix::net::UnixStream::pair().unwrap();
        cmd.stdin(Stdio::from(std::os::fd::OwnedFd::from(child)));
        (host.try_clone().unwrap(), host)
    };
    #[cfg(not(target_os = "linux"))]
    cmd.stdin(Stdio::piped());
    let mut child = cmd.stdout(Stdio::piped()).spawn().unwrap();
    #[cfg(not(target_os = "linux"))]
    let input = child.stdin.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(4);
    let mut output = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        loop {
            let reply = match wire::read(&mut output) {
                #[cfg(target_os = "linux")]
                Ok(Event::DmaFrame(id, frame)) => {
                    wire::linux::receive_buffer(&fds).map(|fd| Reply::Buffer(id, frame, fd))
                }
                result => result.map(Reply::Notice),
            };
            let end = reply.is_err();
            if tx.send(reply).is_err() || end {
                break;
            }
        }
    });
    let mut test = Test {
        native,
        child,
        input: Box::new(input),
        messages: rx,
        window: None,
        start: Instant::now(),
        url: std::env::args()
            .nth(1)
            .unwrap_or_else(|| "https://example.org/".into()),
        navigated: false,
        loaded: false,
        location: String::new(),
        painted: false,
        passed: false,
        frames: 0,
        dirty: false,
        waiting: false,
    };
    events.run_app(&mut test).unwrap();
    assert!(
        test.passed,
        "no completed webpage frame reached the compositor"
    );
    println!(
        "Presented {} frames, including the loaded page at {}.",
        test.frames,
        test.location.split(['?', '#']).next().unwrap_or_default()
    );
}
