//! Opt-in latency measurement for a real Servo helper. No network: it drives
//! the blank tab the helper opens with, so what it times is the helper's own
//! command handling and paint path, not a page.
//!
//! VKORE_SERVO=/absolute/path/to/vkore-servo cargo run --release \
//!   -p vkore-servo-wire --features presentation --example servo_latency
//!
//! Setting `VKORE_LIVE_URL=https://...` also loads that address afterwards and
//! prints every event the page produces (location, load phases, title,
//! favicon, link status, pointer shape) with the time since the request. That
//! one goes to the network, so it is never run by default.
//!
//! It prints the median and 95th percentile, in milliseconds, of:
//! - `paint`: a paint request to the helper's acknowledgement, on an idle tab.
//! - `burst`: a burst of pointer moves followed by a paint request, to the
//!   acknowledgement. Commands are handled in order, so this is how long the
//!   helper takes to get through the burst and reach the paint.
use std::{
    io::{self, Write},
    num::NonZeroU32,
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use vkore_servo_wire::presentation::Presentation;
use vkore_servo_wire::{self as wire, Command as Input, Event, Pointer};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;
const IDLE_PAINTS: usize = 40;
const BURSTS: usize = 12;
const BURST_MOVES: usize = 600;

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Starting,
    Warm,
    Paint,
    Burst,
    Live,
    Done,
}

struct Test {
    native: Presentation,
    child: Child,
    input: Box<dyn Write>,
    messages: mpsc::Receiver<io::Result<Event>>,
    window: Option<Rc<Window>>,
    start: Instant,
    phase: Phase,
    sent: Option<Instant>,
    paint: Vec<f64>,
    burst: Vec<f64>,
    live: Option<Instant>,
    finished: Option<Instant>,
}

impl Test {
    fn send(&mut self, command: Input) {
        wire::write(&mut self.input, &command).unwrap();
        self.input.flush().unwrap();
    }
    fn paint_now(&mut self) {
        self.sent = Some(Instant::now());
        self.send(Input::Paint(0));
    }
    fn burst_now(&mut self) {
        self.sent = Some(Instant::now());
        for n in 0..BURST_MOVES {
            let (x, y) = (
                (n * 7 % WIDTH as usize) as u32,
                (n * 5 % HEIGHT as usize) as u32,
            );
            self.send(Input::Pointer(0, x, y, Pointer::Move));
        }
        self.send(Input::Paint(0));
    }
    fn tick(&mut self, events: &ActiveEventLoop) {
        if self.start.elapsed() > Duration::from_secs(90) {
            eprintln!("timed out in the middle of a measurement");
            std::process::exit(1);
        }
        while let Ok(reply) = self.messages.try_recv() {
            match reply.unwrap() {
                Event::Ready(wire::VERSION, renderer, surface) => {
                    println!("renderer: {renderer}");
                    self.native.attach(surface).unwrap();
                    self.native
                        .place(self.window.as_ref().unwrap(), 1., None)
                        .unwrap();
                    self.send(Input::Open(0));
                    self.send(Input::Resize(0, WIDTH, HEIGHT, 1.0));
                    self.phase = Phase::Warm;
                    self.paint_now();
                }
                Event::Presented(_) => {
                    #[cfg(target_os = "macos")]
                    self.native.receive_frame().expect("import browser surface");
                    let ms = self.sent.take().map(|t| t.elapsed().as_secs_f64() * 1e3);
                    match (self.phase, ms) {
                        (Phase::Warm, _) => {
                            self.phase = Phase::Paint;
                            self.paint_now();
                        }
                        (Phase::Paint, Some(ms)) => {
                            self.paint.push(ms);
                            if self.paint.len() == IDLE_PAINTS {
                                self.phase = Phase::Burst;
                                self.burst_now();
                            } else {
                                self.paint_now();
                            }
                        }
                        (Phase::Burst, Some(ms)) => {
                            self.burst.push(ms);
                            if self.burst.len() == BURSTS {
                                self.phase = Phase::Done;
                                report("paint (idle)", &self.paint);
                                report(
                                    &format!("burst ({BURST_MOVES} moves + paint)"),
                                    &self.burst,
                                );
                                match std::env::var("VKORE_LIVE_URL") {
                                    Ok(url) => {
                                        println!("\nloading {url}");
                                        self.phase = Phase::Live;
                                        self.live = Some(Instant::now());
                                        self.send(Input::Navigate(0, url));
                                    }
                                    Err(_) => events.exit(),
                                }
                            } else {
                                self.burst_now();
                            }
                        }
                        _ => {}
                    }
                }
                Event::Error(reason) => panic!("{reason}"),
                event if self.phase == Phase::Live => {
                    let ms = self.live.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1e3);
                    let line = match &event {
                        Event::Location(_, url, back, forward) => {
                            format!("location {url} (back {back}, forward {forward})")
                        }
                        Event::Loading(_, load) => {
                            if matches!(load, wire::Load::Done) {
                                self.finished = Some(Instant::now());
                            }
                            format!("loading {load:?}")
                        }
                        Event::Title(_, title) => format!("title {title:?}"),
                        Event::Favicon(_, Some(icon)) => {
                            format!("favicon {}x{}", icon.width, icon.height)
                        }
                        Event::Favicon(_, None) => "favicon none".into(),
                        Event::Status(_, status) => format!("status {status:?}"),
                        Event::Cursor(_, cursor) => format!("cursor {cursor:?}"),
                        _ => continue,
                    };
                    println!("{ms:>8.1} ms  {line}");
                }
                _ => {}
            }
        }
        // Give a loaded page a moment to report its title and icon.
        if self.phase == Phase::Live
            && self
                .finished
                .is_some_and(|t| t.elapsed() > Duration::from_secs(3))
        {
            events.exit();
        }
    }
}

fn report(name: &str, samples: &[f64]) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    println!(
        "{name:<34} n={:<3} median {:>8.2} ms   p95 {:>8.2} ms   max {:>8.2} ms",
        sorted.len(),
        at(0.5),
        at(0.95),
        sorted[sorted.len() - 1]
    );
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
                    .with_title("vKore latency measurement")
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
        e.set_control_flow(ControlFlow::wait_duration(Duration::from_millis(1)));
    }
    fn window_event(&mut self, e: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested) {
            e.exit();
        }
    }
}

fn main() {
    let helper = std::env::var_os("VKORE_SERVO")
        .expect("set VKORE_SERVO to the absolute path of the vkore-servo helper");
    let mut command = Command::new(helper);
    let native = Presentation::prepare(&mut command).unwrap();
    #[cfg(target_os = "linux")]
    compile_error!("this measurement runs on macOS; the Linux transport needs a socket pair");
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start the helper");
    let input = child.stdin.take().unwrap();
    let mut output = io::BufReader::new(child.stdout.take().unwrap());
    let (replies, messages) = mpsc::channel();
    thread::spawn(move || {
        loop {
            let reply = wire::read::<Event>(&mut output);
            let end = reply.is_err();
            if replies.send(reply).is_err() || end {
                break;
            }
        }
    });
    let events = EventLoop::new().unwrap();
    events
        .run_app(&mut Test {
            native,
            child,
            input: Box::new(input),
            messages,
            window: None,
            start: Instant::now(),
            phase: Phase::Starting,
            sent: None,
            paint: Vec::new(),
            burst: Vec::new(),
            live: None,
            finished: None,
        })
        .unwrap();
}
