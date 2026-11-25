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
    navigated: bool,
    loaded: bool,
    target: bool,
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
                Reply::Notice(Event::Loading(_, false)) if !self.navigated => {
                    self.navigated = true;
                    self.send(Input::Resize(0, 640, 360, 1.));
                    self.send(Input::Navigate(0, "https://example.org".into()));
                }
                Reply::Notice(Event::Location(_, url, _)) => {
                    self.target = url.starts_with("https://example.org/")
                }
                Reply::Notice(Event::Loading(_, false)) if self.target => {
                    self.loaded = true;
                    self.dirty = true;
                }
                Reply::Notice(Event::Presented(_)) => self.painted = true,
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
                    1.,
                    Some(TileRect {
                        x: 12.,
                        y: 18.,
                        width: 640.,
                        height: 360.,
                    }),
                )
                .unwrap();
            self.send(Input::ShowSurface(true));
            self.painted = false;
            if self.loaded {
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
        navigated: false,
        loaded: false,
        target: false,
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
        "Presented {} frames, including the loaded page.",
        test.frames
    );
}
