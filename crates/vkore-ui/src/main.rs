//! vKore's native desktop: Kore machines, and their DagFS volumes browsed
//! from the host through Kore's own engine. Rendered with Iced over Axiom's
//! software rasterizer; no browser, no daemon, no login.

mod fonts;
mod logo;
mod machines;
mod render_backend;
mod splash;
mod theme;
mod volumes;
mod widgets;

use axgfx::Extent2d;
use axui_iced::Renderer as AxRenderer;
use iced_core::Renderer as _;
use iced_core::{
    Element, Length, Size, Theme, alignment, mouse,
    renderer::{self, Scale},
    shell,
};
use iced_runtime::user_interface::{self, UserInterface};
use iced_widget::{button, column, container, row, stack, text};
use iced_winit::{conversion, winit};
use softbuffer::{Context as SoftContext, Surface as SoftSurface};
use std::{
    alloc::System,
    num::NonZeroU32,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use crate::render_backend::NativeBackend;

/// The renderer every widget in this binary is generic over.
pub(crate) type AxUi = AxRenderer<System>;

pub(crate) type UiElement<'a> = Element<'a, Message, Theme, AxUi>;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// The renderer, with the body face as the default: a widget that names no
/// face (a text field, a bare label) still shapes with a registered one.
fn native_renderer() -> AxRenderer<System> {
    AxRenderer::new_in(
        renderer::Settings {
            default_font: fonts::BODY,
            default_text_size: iced_core::Pixels(theme::SIZE_SM),
            ..renderer::Settings::default()
        },
        System,
    )
}

/// What the view emits.
#[derive(Debug, Clone)]
pub enum Message {
    Show(Page),
    Volume(volumes::VolumeMessage),
    Machine(machines::MachineMessage),
}

/// The two pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Volumes,
    Machines,
}

/// What the workers send the event loop.
#[derive(Debug)]
pub enum UserEvent {
    Volume(volumes::Reply),
    Machine(String, machines::Update),
}

struct Model {
    page: Page,
    volumes: volumes::State,
    machines: machines::State,
    proxy: EventLoopProxy<UserEvent>,
    booted_at: Option<Instant>,
    status: String,
    /// A machine to start once the browser has let go of its disk.
    pending_start: Option<(String, bool)>,
    /// The disk the browser let go of for a machine, reopened when the
    /// machine stops: what it saved is what one looks at next.
    reopen: Option<(String, String)>,
}

impl Model {
    fn update(&mut self, message: Message) {
        match message {
            Message::Show(page) => {
                self.page = page;
                if page == Page::Machines {
                    self.machines.refresh();
                }
            }
            Message::Volume(message) => {
                let machines = &self.machines;
                self.volumes.update(message, |name| machines.disk_of(name));
            }
            Message::Machine(machines::MachineMessage::Start(name, window)) => {
                let disk = self.machines.disk_of(&name);
                if disk.as_deref().is_some_and(|disk| self.volumes.holds(disk)) {
                    // Kore needs its disk whole: the browser closes first, and
                    // the machine starts once the worker has let go.
                    self.reopen = self
                        .volumes
                        .label
                        .clone()
                        .map(|label| (name.clone(), label));
                    self.pending_start = Some((name, window));
                    self.volumes.send(volumes::Request::Close);
                } else {
                    self.start(name, window);
                }
            }
            Message::Machine(message) => self.machines.update(message, &self.proxy),
        }
    }

    fn start(&mut self, name: String, window: bool) {
        self.machines
            .update(machines::MachineMessage::Start(name, window), &self.proxy);
    }

    fn user_event(&mut self, event: UserEvent) {
        match event {
            UserEvent::Volume(reply) => {
                let closed = matches!(reply, volumes::Reply::Closed);
                self.volumes.reply(reply);
                if closed && let Some((name, window)) = self.pending_start.take() {
                    self.start(name, window);
                }
            }
            UserEvent::Machine(name, update) => {
                let finished = matches!(update, machines::Update::Finished(_));
                let reopen = finished
                    && self
                        .reopen
                        .as_ref()
                        .is_some_and(|(machine, _)| *machine == name);
                self.machines.machine_update(name, update);
                if reopen && let Some((_, label)) = self.reopen.take() {
                    let machines = &self.machines;
                    self.volumes.open(&label, |name| machines.disk_of(name));
                }
            }
        }
    }

    /// Whether the splash has frames left to show. The first call after it
    /// ends forgets when it began, so the question is not asked again.
    fn splashing(&mut self) -> bool {
        match self.booted_at {
            Some(at) if at.elapsed() < splash::LENGTH => true,
            Some(_) => {
                self.booted_at = None;
                false
            }
            None => false,
        }
    }

    fn view(&self, _viewport: Size) -> UiElement<'_> {
        let page = match self.page {
            Page::Volumes => volumes::view(&self.volumes, &self.machines.names()),
            Page::Machines => machines::view(&self.machines),
        };
        let base: UiElement<'_> = row![
            sidebar(self.page, self.machines.running.len(), &self.status),
            page
        ]
        .height(Length::Fill)
        .into();
        let base: UiElement<'_> = container(base)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::panel(theme::BACKGROUND, None, 0.0))
            .into();
        if let Some(phase) = self.booted_at.and_then(|at| splash::progress(at.elapsed())) {
            return stack![base, splash::overlay(phase)]
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }
        base
    }
}

/// The navigation rail: the two pages, and what is running.
fn sidebar(current: Page, running: usize, status: &str) -> UiElement<'_> {
    let brand = row![
        splash::mark(20.0),
        text("vKore")
            .font(fonts::DISPLAY)
            .size(15.0)
            .color(theme::rgb(theme::TEXT)),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Center);
    let nav = |label: &'static str, page: Page| {
        button(widgets::nav(label))
            .height(34)
            .padding([0, 10])
            .width(Length::Fill)
            .style(widgets::button_style(widgets::Button::Choice(
                current == page,
            )))
            .on_press(Message::Show(page))
    };
    container(
        column![
            brand,
            nav("Volumes", Page::Volumes),
            nav("Machines", Page::Machines),
            iced_widget::space::vertical(),
            widgets::mono(
                if running == 0 {
                    "no machine running".to_owned()
                } else {
                    format!("{running} machine(s) running")
                },
                theme::TEXT_MUTED,
            ),
            widgets::data(status, theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_2),
    )
    .padding(14)
    .width(220)
    .height(Length::Fill)
    .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), 0.0))
    .into()
}

struct App {
    model: Model,
    context: SoftContext<OwnedDisplayHandle>,
    surface: Option<SoftSurface<OwnedDisplayHandle, Rc<Window>>>,
    renderer: AxRenderer<System>,
    backend: NativeBackend,
    cache: user_interface::Cache,
    events: Vec<iced_core::Event>,
    cursor: mouse::Cursor,
    modifiers: ModifiersState,
    redraw: bool,
    redraw_requested: bool,
    last_frame: Option<Instant>,
}

impl App {
    fn process_events(&mut self, window: &Rc<Window>) {
        if self.events.is_empty() {
            return;
        }
        let size = logical_size(window);
        let mut interface = UserInterface::build(
            self.model.view(size),
            size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let waker = shell::Waker::noop();
        let mut messages = shell::Bus::new();
        let (state, _) = interface.update(
            window,
            &waker,
            &self.events,
            self.cursor,
            &mut self.renderer,
            &mut messages,
        );
        if let user_interface::State::Updated {
            mouse_interaction, ..
        } = state
        {
            if let Some(icon) = conversion::mouse_interaction(mouse_interaction) {
                window.set_cursor(icon);
                window.set_cursor_visible(true);
            } else {
                window.set_cursor_visible(false);
            }
        }
        self.events.clear();
        self.cache = interface.into_cache();
        for message in messages {
            self.model.update(message);
        }
        self.redraw = true;
    }

    fn draw(&mut self, window: &Rc<Window>) -> Result<(), String> {
        let physical = window.inner_size();
        if physical.width == 0 || physical.height == 0 {
            return Ok(());
        }
        let scale = window.scale_factor() as f32;
        self.renderer.hint(Scale {
            window: scale,
            application: 1.0,
        });
        let mut interface = UserInterface::build(
            self.model.view(logical_size(window)),
            logical_size(window),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let now = Instant::now();
        let waker = shell::Waker::noop();
        let mut messages = shell::Bus::new();
        let (state, _) = interface.update(
            window,
            &waker,
            &[iced_core::Event::Window(
                iced_core::window::Event::RedrawRequested(now),
            )],
            self.cursor,
            &mut self.renderer,
            &mut messages,
        );
        let animate = matches!(
            state,
            user_interface::State::Updated {
                redraw_request: iced_core::window::RedrawRequest::NextFrame,
                ..
            }
        );
        interface.draw(
            &mut self.renderer,
            &Theme::Dark,
            &renderer::Style {
                text_color: theme::rgb(theme::TEXT),
            },
            self.cursor,
        );
        self.cache = interface.into_cache();
        let mut model_changed = false;
        for message in messages {
            self.model.update(message);
            model_changed = true;
        }
        // The splash runs on the clock, not on events: keep drawing until it
        // is done. The frame that finds it done draws without it.
        let again = animate || model_changed || self.model.splashing();
        let frame = self
            .renderer
            .take_frame()
            .map_err(|error| format!("{error:?}"))?;
        let text_errors = self.renderer.take_text_errors();
        if !text_errors.is_empty() {
            return Err(format!("unsupported text request: {text_errors:?}"));
        }
        let geometry_errors = self.renderer.take_geometry_errors();
        if !geometry_errors.is_empty() {
            return Err(format!("unsupported geometry request: {geometry_errors:?}"));
        }
        let image_errors = self.renderer.take_image_errors();
        if !image_errors.is_empty() {
            return Err(format!("unsupported image request: {image_errors:?}"));
        }
        // The frame names its masks and images by identity; hand their bytes
        // over before it is rendered.
        self.backend.set_images(self.renderer.take_images());
        axgfx::frame_requirements(frame.view())
            .map_err(|error| format!("axgfx rejected the display list: {error:?}"))?;
        if !self.backend.begin_frame(&frame) {
            self.redraw = again;
            self.last_frame = Some(Instant::now());
            return Ok(());
        }
        let surface = self.surface.as_mut().ok_or("surface unavailable")?;
        surface
            .resize(
                NonZeroU32::new(physical.width).unwrap(),
                NonZeroU32::new(physical.height).unwrap(),
            )
            .map_err(|error| error.to_string())?;
        let mut buffer = surface.buffer_mut().map_err(|error| error.to_string())?;
        let extent =
            Extent2d::new(physical.width, physical.height).map_err(|error| format!("{error:?}"))?;
        self.backend
            .render_changed(&frame, &mut buffer, extent, physical.width as usize)
            .map_err(|error| format!("{error:?}"))?;
        window.pre_present_notify();
        buffer.present().map_err(|error| error.to_string())?;
        self.backend.mark_visible(frame);
        self.redraw = again;
        self.last_frame = Some(Instant::now());
        Ok(())
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let window = Rc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("vKore")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 800))
                        .with_min_inner_size(winit::dpi::PhysicalSize::new(900, 560)),
                )
                .expect("create vKore window"),
        );
        self.surface = Some(
            SoftSurface::new(&self.context, Rc::clone(&window))
                .expect("create presentation surface"),
        );
        self.redraw = true;
        self.redraw_requested = true;
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        self.model.user_event(event);
        self.redraw = true;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self
            .surface
            .as_ref()
            .map(SoftSurface::window)
            .map(Rc::clone)
        else {
            return;
        };
        if window.id() != window_id {
            return;
        }
        match &event {
            WindowEvent::CloseRequested => {
                // Machines started here stop with the window.
                self.model.machines.stop_all();
                event_loop.exit();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = mouse::Cursor::Available(conversion::cursor_position(
                    *position,
                    window.scale_factor() as f32,
                ));
            }
            WindowEvent::CursorLeft { .. } => self.cursor = mouse::Cursor::Unavailable,
            WindowEvent::ModifiersChanged(value) => self.modifiers = value.state(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.redraw = true,
            WindowEvent::RedrawRequested => {
                self.redraw_requested = false;
                if self.redraw
                    && let Err(error) = self.draw(&window)
                {
                    eprintln!("vkore-ui: render failed: {error}");
                    self.model.status = format!("render failed: {error}");
                }
                return;
            }
            _ => {}
        }
        if let Some(event) =
            conversion::window_event(event, window.scale_factor() as f32, self.modifiers)
        {
            self.events.push(event);
            self.process_events(&window);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        if !self.redraw || self.redraw_requested {
            return;
        }
        let now = Instant::now();
        let due = self
            .last_frame
            .and_then(|last| last.checked_add(FRAME_INTERVAL))
            .unwrap_or(now);
        if due <= now {
            if let Some(window) = self.surface.as_ref().map(SoftSurface::window) {
                self.redraw_requested = true;
                window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(due));
        }
    }
}

fn logical_size(window: &Window) -> Size {
    let physical = window.inner_size();
    let scale = window.scale_factor() as f32;
    Size::new(
        physical.width as f32 / scale,
        physical.height as f32 / scale,
    )
}

/// What the command line may ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchOptions {
    state: Option<PathBuf>,
    disk: Option<String>,
}

fn parse_launch_options(args: impl IntoIterator<Item = String>) -> Result<LaunchOptions, String> {
    let mut options = LaunchOptions {
        state: None,
        disk: None,
    };
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--state" => {
                options.state = Some(args.next().ok_or("--state needs a directory")?.into())
            }
            "--open" => options.disk = Some(args.next().ok_or("--open needs a disk")?),
            _ => {
                return Err("usage: vkore-ui [--state DIR] [--open DISK|@MACHINE]".into());
            }
        }
    }
    Ok(options)
}

fn main() {
    let options = parse_launch_options(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("vkore-ui: {error}");
        std::process::exit(2);
    });
    let state_root = options
        .state
        .clone()
        .or_else(vkored::State::default_root)
        .unwrap_or_else(|| {
            eprintln!("vkore-ui: no state directory: set $HOME or pass --state");
            std::process::exit(2);
        });
    // Before the first frame, so that frame already shapes in our faces. Axiom
    // keeps shaping to the faces it can draw; registering adds ours to them.
    if let Err(error) = fonts::register() {
        eprintln!("vkore-ui: the bundled faces did not register: {error:?}");
        std::process::exit(2);
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    let proxy = event_loop.create_proxy();
    let context = SoftContext::new(event_loop.owned_display_handle())
        .expect("create software presentation context");
    let mut model = Model {
        page: Page::Volumes,
        volumes: volumes::State::new(volumes::spawn(proxy.clone())),
        machines: machines::State::new(state_root),
        proxy,
        booted_at: Some(Instant::now()),
        status: String::new(),
        pending_start: None,
        reopen: None,
    };
    if let Some(disk) = options.disk {
        let machines = &model.machines;
        model.volumes.open(&disk, |name| machines.disk_of(name));
    }
    let mut app = App {
        model,
        context,
        surface: None,
        renderer: native_renderer(),
        backend: NativeBackend::new().unwrap_or_else(|error| {
            eprintln!("vkore-ui: failed to initialize the Axiom backend: {error:?}");
            std::process::exit(2);
        }),
        cache: user_interface::Cache::new(),
        events: Vec::new(),
        cursor: mouse::Cursor::Unavailable,
        modifiers: ModifiersState::default(),
        redraw: true,
        redraw_requested: false,
        last_frame: None,
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run vKore UI");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use vkore_volume::{Capture, DiskKind, Entry, EntryKind, Listing, VolPath};

    /// Text a guest may write into a file name, a log, or a capture's
    /// metadata: scripts and symbols none of the bundled faces cover, an
    /// emoji, a private-use code point, bidirectional overrides, a combining
    /// mark, controls, and a terminal escape.
    const GUEST_TEXT: &str = "日本語のスクショ 😀 \u{E800} \u{202E}txt.exe\u{202C} e\u{301} Ωμέγα Кириллица \u{1b}[31mred\u{1b}[0m\ttab\r\n";

    /// Lays out and draws `element` the way the shell does, and returns the
    /// frame's glyph run count and its text refusals.
    fn draw(element: UiElement<'_>, size: Size) -> (usize, Vec<axui_iced::TextError>) {
        fonts::register().expect("MONOLITH faces register");
        let mut renderer = native_renderer();
        renderer.reset(iced_core::Rectangle::with_size(size));
        let mut interface =
            UserInterface::build(element, size, user_interface::Cache::new(), &mut renderer);
        interface.draw(
            &mut renderer,
            &Theme::Dark,
            &renderer::Style {
                text_color: theme::rgb(theme::TEXT),
            },
            mouse::Cursor::Unavailable,
        );
        let frame = renderer.take_frame().expect("a frame");
        (frame.glyph_runs().len(), renderer.take_text_errors())
    }

    #[test]
    fn launch_options_name_a_state_directory_and_a_disk() {
        let options = parse_launch_options([
            "--state".into(),
            "/s".into(),
            "--open".into(),
            "@demo".into(),
        ])
        .unwrap();
        assert_eq!(options.state, Some(PathBuf::from("/s")));
        assert_eq!(options.disk.as_deref(), Some("@demo"));
        assert!(parse_launch_options(["--server".into()]).is_err());
    }

    #[test]
    fn every_face_draws_the_interface_symbols_and_anything_a_guest_writes() {
        let size = Size::new(900.0, 80.0);
        for (name, font) in [
            ("display", fonts::DISPLAY),
            ("body", fonts::BODY),
            ("mono", fonts::MONO),
            ("mono label", fonts::MONO_LABEL),
            ("mono nav", fonts::MONO_NAV),
            ("splash", fonts::SPLASH),
        ] {
            let samples = [
                fonts::shown(font, "Volumes", fonts::Breaks::Escape),
                fonts::shown(font, GUEST_TEXT, fonts::Breaks::Escape),
                fonts::shown(font, GUEST_TEXT, fonts::Breaks::Keep),
            ];
            for sample in samples {
                let (runs, refused) = draw(text(sample.as_ref()).font(font).into(), size);
                assert_eq!(refused, [], "{name} refused {sample:?}");
                assert!(runs > 0, "{name} drew nothing for {sample:?}");
            }
        }
        // A widget that names no face takes the renderer's default.
        let shown = fonts::shown(fonts::BODY, GUEST_TEXT, fonts::Breaks::Escape);
        let (runs, refused) = draw(text(shown.as_ref()).into(), size);
        assert_eq!(refused, []);
        assert!(runs > 0);
    }

    #[test]
    fn the_interface_symbols_are_set_in_the_faces_that_carry_them() {
        // Written by the binary, so never escaped: each must be in its face.
        let size = Size::new(900.0, 80.0);
        for (font, sample) in [
            (
                fonts::BODY,
                "a \u{2014} b \u{b7} c\u{2026} 2\u{d7}3 \u{2192}",
            ),
            (fonts::MONO, "\u{25b8} \u{b7} \u{2026} \u{d7} \u{2192}"),
            (fonts::DISPLAY, "VOLUMES MACHINES"),
        ] {
            assert_eq!(fonts::shown(font, sample, fonts::Breaks::Escape), sample);
            let (runs, refused) = draw(text(sample).font(font).into(), size);
            assert_eq!(refused, []);
            assert!(runs > 0);
        }
    }

    /// The volume page as it stands over a guest-written screenshots
    /// folder, its first entry selected and its capture record read.
    fn guest_listing() -> (volumes::State, mpsc::Receiver<volumes::Request>) {
        let (requests, worker) = mpsc::channel();
        let mut state = volumes::State::new(requests);
        state.reply(volumes::Reply::Opened {
            label: "@demo".into(),
            kind: DiskKind::Image,
            generation: 173,
            fallback: false,
        });
        let here = state.here().expect("the screenshots folder");
        let names = [
            "日本語のスクショ.png",
            "😀.log",
            "Кириллица",
            "e\u{301}\u{202E}gnp.exe",
        ];
        state.reply(volumes::Reply::Listed(
            here.clone(),
            Ok(Listing::Directory {
                entries: names
                    .iter()
                    .enumerate()
                    .map(|(index, name)| Entry {
                        name: (*name).into(),
                        kind: if index == 2 {
                            EntryKind::Directory
                        } else {
                            EntryKind::File
                        },
                        len: 1 << (10 * index),
                    })
                    .collect(),
            }),
        ));
        state.update(volumes::VolumeMessage::Select(0), |_| None);
        let selected: VolPath = here.join(names[0]).expect("a valid name");
        state.reply(volumes::Reply::Previewed(
            selected,
            Ok(volumes::Summary::Image {
                width: 1280,
                height: 800,
                capture: Some(Box::new(Capture {
                    software: Some(format!("kore {GUEST_TEXT}")),
                    created: Some("2026-11-02T11:32:55Z".into()),
                    kernel_build: Some("6f56689d9748".into()),
                    uptime_ms: Some("114000".into()),
                    screen: Some("0".into()),
                    mode: Some("screen".into()),
                    app: Some(GUEST_TEXT.into()),
                    rect: Some("0,0,1280,800".into()),
                    density_percent: Some("100".into()),
                })),
                text: vec![
                    ("kore:freeze-us".into(), "412".into()),
                    ("kore:cursor".into(), GUEST_TEXT.into()),
                ],
            }),
        ));
        state.status = GUEST_TEXT.into();
        (state, worker)
    }

    #[test]
    fn the_volume_page_draws_a_guest_written_listing_and_its_capture() {
        let (mut state, _worker) = guest_listing();
        let machines = vec!["demo".to_owned(), GUEST_TEXT.to_owned()];
        let (runs, refused) = draw(volumes::view(&state, &machines), Size::new(1280.0, 800.0));
        assert_eq!(refused, []);
        assert!(runs > 20, "the page drew only {runs} runs");

        // A guest log, as text.
        state.update(volumes::VolumeMessage::Select(1), |_| None);
        let here = state.here().expect("the screenshots folder");
        let log = here.join("😀.log").expect("a valid name");
        state.reply(volumes::Reply::Previewed(
            log,
            Ok(volumes::Summary::Text {
                text: GUEST_TEXT.repeat(64),
                lines: 128,
                truncated: true,
            }),
        ));
        let (_, refused) = draw(volumes::view(&state, &machines), Size::new(1280.0, 800.0));
        assert_eq!(refused, []);
    }

    #[test]
    fn the_shared_kernels_draw_the_volume_page_exactly_as_the_reference() {
        let (state, _worker) = guest_listing();
        let machines = vec!["demo".to_owned()];
        let size = Size::new(1280.0, 800.0);
        fonts::register().expect("MONOLITH faces register");
        let mut renderer = native_renderer();
        renderer.reset(iced_core::Rectangle::with_size(size));
        let mut interface = UserInterface::build(
            volumes::view(&state, &machines),
            size,
            user_interface::Cache::new(),
            &mut renderer,
        );
        // A button resolves its status while events traverse the tree: prime
        // the frame as the window loop does, or every control draws disabled.
        let _ = interface.update(
            &iced_core::window::Headless,
            &shell::Waker::noop(),
            &[iced_core::Event::Window(
                iced_core::window::Event::RedrawRequested(Instant::now()),
            )],
            mouse::Cursor::Unavailable,
            &mut renderer,
            &mut shell::Bus::new(),
        );
        interface.draw(
            &mut renderer,
            &Theme::Dark,
            &renderer::Style {
                text_color: theme::rgb(theme::TEXT),
            },
            mouse::Cursor::Unavailable,
        );
        let frame = renderer.take_frame().expect("a frame");
        assert_eq!(renderer.take_text_errors(), []);
        let masks = renderer.take_images();
        axgfx::frame_requirements(frame.view()).expect("axgfx takes the display list");
        let extent = Extent2d::new(1280, 800).unwrap();

        let mut kernels = NativeBackend::new().unwrap();
        kernels.set_images(masks.clone());
        assert!(kernels.begin_frame(&frame));
        let mut drawn = vec![0u32; 1280 * 800];
        kernels
            .render_changed(&frame, &mut drawn, extent, 1280)
            .unwrap();
        let mut reference = NativeBackend::reference_for_measurement().unwrap();
        reference.set_images(masks);
        let mut expected = vec![0u32; 1280 * 800];
        reference
            .render_changed(&frame, &mut expected, extent, 1280)
            .unwrap();
        let differing = drawn.iter().zip(&expected).filter(|(a, b)| a != b).count();
        assert_eq!(differing, 0, "{differing} pixels differ from the reference");
        let page = drawn.iter().filter(|pixel| **pixel != drawn[0]).count();
        assert!(page > 20_000, "the page drew only {page} pixels");

        // A damaged region redraws to the same pixels on both.
        let damage = axgfx::PixelRect::new(240, 160, 640, 260).unwrap();
        let damage = axgfx::Damage::Rects(&[damage]);
        kernels
            .render_damage_for_measurement(&frame, &mut drawn, extent, 1280, damage)
            .unwrap();
        reference
            .render_damage_for_measurement(&frame, &mut expected, extent, 1280, damage)
            .unwrap();
        assert!(
            drawn == expected,
            "a damaged redraw drifted from the reference"
        );

        // And a frame the backend forgot draws again, whole and the same.
        kernels.invalidate_for_measurement();
        assert!(kernels.begin_frame(&frame));
        drawn.fill(0);
        kernels
            .render_changed(&frame, &mut drawn, extent, 1280)
            .unwrap();
        assert!(
            drawn == expected,
            "a full redraw drifted from the reference"
        );
    }

    #[test]
    fn the_machine_page_and_the_rail_draw() {
        let root = tempfile::tempdir().expect("a state directory");
        let store =
            vkored::MachineStore::new(vkored::State::open(root.path()).expect("state opens"));
        let mut config = vkored::MachineConfig::new("demo", "/isos/kore-x86-release.iso");
        config.screenshots_folder = true;
        store.create(&config).expect("the machine is created");
        let mut machines = machines::State::new(root.path().to_owned());
        machines.selected = Some("demo".into());
        machines.status = GUEST_TEXT.into();
        let (runs, refused) = draw(machines::view(&machines), Size::new(1280.0, 800.0));
        assert_eq!(refused, []);
        assert!(runs > 10, "the page drew only {runs} runs");

        let (runs, refused) = draw(
            sidebar(Page::Machines, 1, GUEST_TEXT),
            Size::new(220.0, 800.0),
        );
        assert_eq!(refused, []);
        assert!(runs > 3);
    }
}
