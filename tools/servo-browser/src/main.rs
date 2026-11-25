//! One native Servo engine per desktop browser tile. No listening sockets.
#[cfg(target_os = "linux")]
mod dmabuf;
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::{self, Write},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use servo::{
    DevicePoint, EventLoopWaker, InputEvent, Key, KeyState, KeyboardEvent, Modifiers, MouseButton,
    MouseButtonAction, MouseButtonEvent, MouseMoveEvent, NavigationRequest, RenderingContext,
    Servo, ServoBuilder, WebView, WebViewBuilder, WebViewDelegate, WheelDelta, WheelEvent,
    WheelMode, WindowRenderingContext,
};
use url::Url;
use vkore_servo_wire::{self as wire, Command, Event, Pointer};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle},
    window::{Window, WindowId},
};

#[derive(Clone)]
struct Wake(EventLoopProxy<Message>, Arc<AtomicBool>);
impl EventLoopWaker for Wake {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {
        if !self.1.swap(true, Ordering::AcqRel) {
            let _ = self.0.send_event(Message::Spin);
        }
    }
}

enum Message {
    Input(Command, std::sync::mpsc::SyncSender<()>),
    Spin,
    End,
}

fn send(event: Event) {
    let mut out = io::stdout().lock();
    if wire::write(&mut out, &event)
        .and_then(|()| out.flush())
        .is_err()
    {
        std::process::exit(0);
    }
}

struct Observer {
    id: u64,
    dirty: Cell<bool>,
    back: Cell<bool>,
}
impl Observer {
    fn invalidate(&self) {
        if !self.dirty.replace(true) {
            send(Event::Changed(self.id));
        }
    }
    fn location(&self, view: &WebView) {
        if let Some(url) = view.url() {
            let address: String = url.as_str().chars().take(wire::MAX_ADDRESS).collect();
            send(Event::Location(self.id, address, self.back.get()));
        }
    }
}

impl WebViewDelegate for Observer {
    fn notify_crashed(&self, _: WebView, reason: String, _: Option<String>) {
        send(Event::Error(format!(
            "Page stopped: {}",
            reason.chars().take(256).collect::<String>()
        )));
    }
    fn notify_load_status_changed(&self, _: WebView, status: servo::LoadStatus) {
        send(Event::Loading(
            self.id,
            matches!(
                status,
                servo::LoadStatus::Started | servo::LoadStatus::HeadParsed
            ),
        ));
    }
    fn notify_new_frame_ready(&self, _: WebView) {
        self.invalidate();
    }
    fn notify_url_changed(&self, view: WebView, _: Url) {
        self.location(&view);
    }
    fn notify_history_changed(&self, view: WebView, _: Vec<Url>, current: usize) {
        self.back.set(current > 0);
        self.location(&view);
    }
    fn request_navigation(&self, _: WebView, request: NavigationRequest) {
        if request.url.as_str() == "about:blank" || website(&request.url) {
            request.allow();
        } else {
            request.deny();
        }
    }
}

fn website(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn address(text: &str) -> Option<Url> {
    let text = text.trim();
    if text.is_empty() || text.len() > wire::MAX_ADDRESS {
        return None;
    }
    let url = if text.contains("://") {
        Url::parse(text)
    } else {
        Url::parse(&format!("https://{text}"))
    }
    .ok()?;
    website(&url).then_some(url)
}

struct Tab {
    view: WebView,
    observer: Rc<Observer>,
}
struct Browser {
    tabs: BTreeMap<u64, Tab>,
    active: Option<u64>,
    servo: Servo,
    context: Rc<WindowRenderingContext>,
    // The native surface must outlive every offscreen context.
    #[cfg(target_os = "macos")]
    export: wire::macos::Export,
    #[cfg(target_os = "linux")]
    dma: Option<dmabuf::Exporter>,
    window: Window,
}

impl Browser {
    fn new(events: &ActiveEventLoop, wake: Wake) -> Result<Self, Box<dyn std::error::Error>> {
        let size = PhysicalSize::new(640, 360);
        let window = events.create_window(
            Window::default_attributes()
                .with_visible(false)
                .with_inner_size(size),
        )?;
        let context = Rc::new(
            WindowRenderingContext::new(events.display_handle()?, window.window_handle()?, size)
                .map_err(|e| format!("Could not create a Servo GL surface: {e:?}"))?,
        );
        context
            .make_current()
            .map_err(|e| format!("Could not activate Servo GL: {e:?}"))?;
        let renderer = context.gleam_gl_api().get_string(gleam::gl::RENDERER);
        let preferences = servo::Preferences {
            dom_webgpu_enabled: true,
            dom_webgl2_enabled: true,
            dom_web_animations_enabled: true,
            dom_intersection_observer_enabled: true,
            ..Default::default()
        };
        let servo = ServoBuilder::default()
            .preferences(preferences)
            .event_loop_waker(Box::new(wake))
            .build();
        #[cfg(not(target_os = "macos"))]
        let handle = window.window_handle()?.as_raw();
        #[cfg(target_os = "macos")]
        let (export, surface) = {
            let name = std::ffi::CString::new(std::env::var("VKORE_SURFACE_SERVER")?)?;
            (
                wire::macos::Export::connect(&name)?,
                wire::Surface::MacIoSurface,
            )
        };
        #[cfg(not(target_os = "macos"))]
        let surface = match handle {
            RawWindowHandle::Wayland(_) => wire::Surface::Wayland,
            RawWindowHandle::Xlib(h) => wire::Surface::X11(h.window.try_into()?),
            RawWindowHandle::Xcb(h) => wire::Surface::X11(h.window.get()),
            RawWindowHandle::Win32(h) => wire::Surface::Win32(h.hwnd.get()),
            _ => return Err("This display server does not support embedded Servo surfaces. On Linux start vKore with an X11 or XWayland display.".into()),
        };
        #[cfg(target_os = "linux")]
        let dma = matches!(surface, wire::Surface::Wayland)
            .then(dmabuf::Exporter::new)
            .transpose()?;
        send(Event::Ready(wire::VERSION, renderer, surface));
        Ok(Self {
            tabs: BTreeMap::new(),
            active: None,
            servo,
            context,
            #[cfg(target_os = "macos")]
            export,
            #[cfg(target_os = "linux")]
            dma,
            window,
        })
    }

    fn select(&mut self, id: u64) {
        if !self.tabs.contains_key(&id) {
            return;
        }
        if let Some(previous) = self.active.and_then(|old| self.tabs.get(&old)) {
            previous.view.set_focused(false);
            previous.view.hide();
        }
        self.active = Some(id);
        let tab = &self.tabs[&id];
        tab.view.show();
        tab.view.set_focused(true);
        tab.observer.invalidate();
    }

    fn input(&mut self, command: Command) -> io::Result<()> {
        match command {
            Command::ShowSurface(show) => {
                if matches!(
                    self.window.window_handle().map(|h| h.as_raw()),
                    Ok(RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_))
                ) {
                    self.window.set_visible(show);
                }
            }
            Command::Open(id)
                if self.tabs.len() < wire::MAX_TABS && !self.tabs.contains_key(&id) =>
            {
                let context: Rc<dyn RenderingContext> = self.context.clone();
                #[cfg(target_os = "linux")]
                let context: Rc<dyn RenderingContext> = if self.dma.is_some() {
                    Rc::new(self.context.offscreen_context(PhysicalSize::new(640, 360)))
                } else {
                    context
                };
                let observer = Rc::new(Observer {
                    id,
                    dirty: Cell::new(false),
                    back: Cell::new(false),
                });
                let view = WebViewBuilder::new(&self.servo, context)
                    .delegate(observer.clone())
                    .build();
                self.tabs.insert(id, Tab { view, observer });
                self.select(id);
            }
            Command::Close(id) => {
                self.tabs.remove(&id);
                if self.active == Some(id) {
                    self.active = None;
                }
            }
            Command::Select(id) => self.select(id),
            other => {
                let Some(id) = self.active else { return Ok(()) };
                let tab = &self.tabs[&id];
                let view = &tab.view;
                match other {
                    Command::Navigate(target, text) if id == target => {
                        if let Some(url) = address(&text) {
                            view.load(url);
                        } else {
                            send(Event::Error("Enter an HTTPS website address.".into()));
                        }
                    }
                    Command::Back(target) if id == target => {
                        view.go_back(1);
                    }
                    Command::Resize(target, w, h, scale) if id == target => {
                        wire::check_size(w, h)?;
                        if !(0.25..=8.).contains(&scale) {
                            return Err(io::Error::other("Invalid display scale"));
                        }
                        view.set_hidpi_scale_factor(euclid::Scale::new(scale));
                        let size = PhysicalSize::new(w, h);
                        let _ = self.window.request_inner_size(size);
                        view.resize(size);
                        tab.observer.invalidate();
                    }
                    Command::Paint(target) if id == target => {
                        tab.observer.dirty.set(false);
                        view.paint();
                        #[cfg(target_os = "linux")]
                        if let Some(dma) = &self.dma {
                            let ctx = view.rendering_context();
                            ctx.prepare_for_rendering();
                            let size = ctx.size();
                            let (fd, frame) =
                                dma.snapshot(ctx.gleam_gl_api().as_ref(), size.width, size.height)?;
                            wire::linux::send_buffer(io::stdin(), fd)?;
                            send(Event::DmaFrame(id, frame));
                            return Ok(());
                        }
                        #[cfg(target_os = "macos")]
                        {
                            self.context.prepare_for_rendering();
                            let size = self.context.size();
                            self.export.send(size.width, size.height)?;
                        }
                        #[cfg(not(target_os = "macos"))]
                        self.context.present();
                        send(Event::Presented(id));
                    }
                    Command::Pointer(target, x, y, pointer) if id == target => {
                        let point = DevicePoint::new(x as f32, y as f32).into();
                        let input = match pointer {
                            Pointer::Move => InputEvent::MouseMove(MouseMoveEvent::new(point)),
                            Pointer::Wheel(y) => InputEvent::Wheel(WheelEvent::new(
                                WheelDelta {
                                    x: 0.,
                                    y: y as f64,
                                    z: 0.,
                                    mode: WheelMode::DeltaPixel,
                                },
                                point,
                            )),
                            Pointer::Down | Pointer::Up => {
                                InputEvent::MouseButton(MouseButtonEvent::new(
                                    if matches!(pointer, Pointer::Down) {
                                        MouseButtonAction::Down
                                    } else {
                                        MouseButtonAction::Up
                                    },
                                    MouseButton::Primary,
                                    point,
                                ))
                            }
                        };
                        view.notify_input_event(input);
                    }
                    Command::Key(target, key, down, flags) if id == target => {
                        if let Ok(key) = key.parse::<Key>() {
                            let mut event = KeyboardEvent::from_state_and_key(
                                if down { KeyState::Down } else { KeyState::Up },
                                key,
                            );
                            for (bit, modifier) in [
                                (1, Modifiers::SHIFT),
                                (2, Modifiers::CONTROL),
                                (4, Modifiers::ALT),
                                (8, Modifiers::META),
                            ] {
                                event.event.modifiers.set(modifier, flags & bit != 0);
                            }
                            view.notify_input_event(InputEvent::Keyboard(event));
                        }
                    }
                    _ => {}
                }
            }
        }
        self.servo.spin_event_loop();
        Ok(())
    }
}

struct App {
    browser: Option<Browser>,
    wake: Wake,
}
impl ApplicationHandler<Message> for App {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        if self.browser.is_none() {
            match Browser::new(events, self.wake.clone()) {
                Ok(browser) => self.browser = Some(browser),
                Err(error) => {
                    send(Event::Error(error.to_string()));
                    events.exit();
                }
            }
        }
    }
    fn user_event(&mut self, events: &ActiveEventLoop, event: Message) {
        let Some(browser) = &mut self.browser else {
            return;
        };
        match event {
            Message::Input(command, handled) => {
                let result = browser.input(command);
                let _ = handled.send(());
                if let Err(error) = result {
                    send(Event::Error(error.to_string()));
                    events.exit();
                }
            }
            Message::Spin => {
                self.wake.1.store(false, Ordering::Release);
                browser.servo.spin_event_loop();
            }
            Message::End => events.exit(),
        }
    }
    fn new_events(&mut self, _: &ActiveEventLoop, cause: winit::event::StartCause) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. })
            && let Some(browser) = &self.browser
        {
            browser.servo.spin_event_loop();
        }
    }
    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        let animating = self.browser.as_ref().is_some_and(|b| {
            b.active
                .and_then(|id| b.tabs.get(&id))
                .is_some_and(|t| t.view.animating() && !t.observer.dirty.get())
        });
        events.set_control_flow(if animating {
            winit::event_loop::ControlFlow::wait_duration(std::time::Duration::from_millis(33))
        } else {
            winit::event_loop::ControlFlow::Wait
        });
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let mut builder = EventLoop::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false)
            .with_activate_ignoring_other_apps(false);
    }
    let events = builder.build()?;
    let proxy = events.create_proxy();
    let wake = Wake(proxy.clone(), Arc::default());
    thread::Builder::new()
        .name("vkore-input".into())
        .spawn(move || {
            let mut input = io::stdin().lock();
            let (handled, ack) = std::sync::mpsc::sync_channel(1);
            while let Ok(command) = wire::read::<Command>(&mut input) {
                if proxy
                    .send_event(Message::Input(command, handled.clone()))
                    .is_err()
                    || ack.recv().is_err()
                {
                    return;
                }
            }
            let _ = proxy.send_event(Message::End);
        })?;
    events.run_app(&mut App {
        browser: None,
        wake,
    })?;
    Ok(())
}
