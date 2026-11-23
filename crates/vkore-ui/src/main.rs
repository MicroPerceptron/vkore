//! vKore's native desktop: Kore machines, and their DagFS volumes browsed
//! from the host through Kore's own engine. Rendered with Iced over Axiom's
//! software rasterizer. Accounts are optional and authorize image downloads.

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

mod account;
mod console;
mod fonts;
mod google_sign_in;
mod hardware;
mod logo;
mod machines;
mod navigation;
mod render_backend;
mod settings;
mod splash;
mod theme;
mod typography;
#[cfg(test)]
use typography::Typography;
mod visuals;
mod volumes;
mod widgets;

use axgfx::Extent2d;
use axui_iced::Renderer as AxRenderer;
use iced_core::Renderer as _;
use iced_core::{
    Element, Length, Size, Theme, mouse,
    renderer::{self, Scale},
    shell,
};
use iced_runtime::user_interface::{self, UserInterface};
#[cfg(test)]
use iced_widget::text;
use iced_widget::{container, row, stack};
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
    ToggleNavigation,
    Appearance(theme::Appearance),
    Settings(settings::SettingsMessage),
    Account(account::Action),
    Volume(volumes::VolumeMessage),
    Machine(machines::MachineMessage),
}

/// The desktop pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Volumes,
    Machines,
    Settings,
    Account,
}

/// What the workers send the event loop.
#[derive(Debug)]
pub enum UserEvent {
    Account(account::Reply),
    SystemTheme(Option<bool>),
    Volume(volumes::Reply),
    Machine(String, machines::Update),
}

struct Model {
    page: Page,
    navigation_collapsed: bool,
    appearance: theme::Appearance,
    system_dark: Option<bool>,
    settings: settings::State,
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
        // Closing is a drain: no new run, import, authorization or API start
        // may be queued while the existing sessions finish collecting work.
        if self.machines.closing {
            return;
        }
        match message {
            Message::ToggleNavigation => self.navigation_collapsed = !self.navigation_collapsed,
            Message::Account(action) => self.settings.account.update(action, &self.proxy),
            Message::Settings(message) => self.settings.update(message),
            Message::Appearance(appearance) => {
                self.appearance = appearance;
                if let Err(error) = appearance.save(&self.machines.state_root) {
                    self.status =
                        format!("Appearance changed for this session; could not save: {error}");
                }
            }
            Message::Show(page) => {
                self.machines.unfocus_console();
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
            UserEvent::Account(reply) => {
                if self.settings.account.reply(reply) {
                    self.machines.refresh_images(self.proxy.clone());
                }
            }
            UserEvent::SystemTheme(dark) => self.system_dark = dark,
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
        let ty = self.settings.typography;
        let page = match self.page {
            Page::Volumes => volumes::view(&self.volumes, &self.machines.names(), ty),
            Page::Machines => {
                machines::view(&self.machines, ty, self.appearance.iced(self.system_dark))
            }
            Page::Settings => settings::view(self.appearance, self.system_dark, &self.settings),
            Page::Account => account::page(&self.settings.account, ty),
        };
        let base: UiElement<'_> = row![
            navigation::view(
                self.page,
                self.machines.running.len(),
                &self.status,
                self.navigation_collapsed,
                self.settings.account.preview(),
                ty
            ),
            page
        ]
        .height(Length::Fill)
        .into();
        let base: UiElement<'_> = container(base)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.0))
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

#[cfg(target_os = "linux")]
fn system_dark_value(scheme: mundy::ColorScheme) -> Option<bool> {
    match scheme {
        mundy::ColorScheme::Light => Some(false),
        mundy::ColorScheme::Dark => Some(true),
        mundy::ColorScheme::NoPreference => None,
    }
}

fn window_theme(appearance: theme::Appearance) -> Option<winit::window::Theme> {
    match appearance {
        theme::Appearance::System => None,
        theme::Appearance::Light => Some(winit::window::Theme::Light),
        theme::Appearance::Dark => Some(winit::window::Theme::Dark),
    }
}

struct App {
    model: Model,
    window_appearance: theme::Appearance,
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
        if self.events.iter().any(|event| {
            matches!(
                event,
                iced_core::Event::Mouse(mouse::Event::ButtonPressed(_))
                    | iced_core::Event::Window(iced_core::window::Event::Unfocused)
            )
        }) {
            self.model.machines.unfocus_console();
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
        if self.window_appearance != self.model.appearance {
            window.set_theme(window_theme(self.model.appearance));
            self.window_appearance = self.model.appearance;
        }
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
        let appearance = self.model.appearance.iced(self.model.system_dark);
        interface.draw(
            &mut self.renderer,
            &appearance,
            &renderer::Style {
                text_color: theme::color(&appearance, theme::TEXT),
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
        if let Some(reported) = event_loop.system_theme() {
            self.model.system_dark = Some(reported == winit::window::Theme::Dark);
        }
        let window = Rc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("vKore")
                        .with_theme(window_theme(self.model.appearance))
                        .with_inner_size(winit::dpi::LogicalSize::new(1280, 800))
                        .with_min_inner_size(winit::dpi::LogicalSize::new(900, 560)),
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.model.user_event(event);
        self.redraw = true;
        if self.model.machines.closing && self.model.machines.running.is_empty() {
            event_loop.exit();
        }
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
            WindowEvent::ThemeChanged(reported) => {
                if self.model.appearance == theme::Appearance::System {
                    self.model.user_event(UserEvent::SystemTheme(Some(
                        *reported == winit::window::Theme::Dark,
                    )));
                    self.redraw = true;
                    window.request_redraw();
                }
            }
            WindowEvent::CloseRequested => {
                // Machines started here stop with the window: pressing their
                // power buttons and leaving now would end QEMU under a guest
                // still shutting down. Wait for them to finish; a second
                // close leaves at once.
                self.model.settings.stop();
                self.model.pending_start = None;
                self.model.reopen = None;
                if self.model.machines.request_close() {
                    event_loop.exit();
                } else {
                    self.redraw = true;
                    window.request_redraw();
                }
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
                // Explicit window themes suppress native notifications. Query
                // the system again when returning to System before drawing.
                if self.window_appearance != self.model.appearance
                    && self.model.appearance == theme::Appearance::System
                    && let Some(reported) = event_loop.system_theme()
                {
                    self.model.system_dark = Some(reported == winit::window::Theme::Dark);
                }
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
        self.redraw |= self.model.machines.poll_consoles();
        // Shell sockets are nonblocking and only sampled while connected.
        let console_due = self
            .model
            .machines
            .has_live_console()
            .then(|| Instant::now() + FRAME_INTERVAL);
        event_loop.set_control_flow(console_due.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
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
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                console_due.map_or(due, |poll| due.min(poll)),
            ));
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
    appearance: Option<theme::Appearance>,
    state: Option<PathBuf>,
    disk: Option<String>,
}

fn parse_launch_options(args: impl IntoIterator<Item = String>) -> Result<LaunchOptions, String> {
    let mut options = LaunchOptions {
        appearance: None,
        state: None,
        disk: None,
    };
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--state" => {
                options.state = Some(args.next().ok_or("--state needs a directory")?.into())
            }
            "--theme" => {
                options.appearance = Some(theme::Appearance::parse(
                    &args.next().ok_or("--theme needs system, light or dark")?,
                )?)
            }
            "--open" => options.disk = Some(args.next().ok_or("--open needs a disk")?),
            _ => {
                return Err(
                    "usage: vkore-ui [--state DIR] [--open DISK|@MACHINE] [--theme system|light|dark]"
                        .into(),
                );
            }
        }
    }
    Ok(options)
}

fn main() {
    if let Some(code) = vkore_volume::helper_entry() {
        std::process::exit(code);
    }
    if let Some(code) = vkore_vm::passthrough::helper_entry() {
        std::process::exit(code);
    }
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
    // Resolve/create state before probing storage or loading preferences.
    // Discovery and launch helpers must share one absolute root, even when
    // the requested relative directory does not exist yet.
    let state_root = vkored::State::open(&state_root)
        .map(|state| state.root().to_owned())
        .unwrap_or_else(|error| {
            eprintln!(
                "vkore-ui: state directory {}: {error}",
                state_root.display()
            );
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
    #[cfg(target_os = "linux")]
    let system_dark =
        mundy::Preferences::once_blocking(mundy::Interest::ColorScheme, Duration::from_millis(200))
            .and_then(|preferences| system_dark_value(preferences.color_scheme));
    #[cfg(not(target_os = "linux"))]
    let system_dark = None; // Read from ActiveEventLoop before the first frame.
    #[cfg(target_os = "linux")]
    let _system_theme_subscription = {
        let theme_proxy = proxy.clone();
        mundy::Preferences::subscribe(mundy::Interest::ColorScheme, move |preferences| {
            let _ = theme_proxy.send_event(UserEvent::SystemTheme(system_dark_value(
                preferences.color_scheme,
            )));
        })
    };
    let context = SoftContext::new(event_loop.owned_display_handle())
        .expect("create software presentation context");
    let (saved_appearance, appearance_status) = match theme::Appearance::load(&state_root) {
        Ok(appearance) => (appearance, String::new()),
        Err(error) => (
            theme::Appearance::default(),
            format!("Could not load appearance: {error}"),
        ),
    };
    let mut model = Model {
        page: Page::Volumes,
        navigation_collapsed: false,
        appearance: options.appearance.unwrap_or(saved_appearance),
        system_dark,
        volumes: volumes::State::new(volumes::spawn(proxy.clone())),
        machines: machines::State::new(state_root.clone()),
        settings: settings::State::new(state_root),
        proxy,
        booted_at: Some(Instant::now()),
        status: appearance_status,
        pending_start: None,
        reopen: None,
    };
    model.machines.account = model.settings.account.account.clone();
    model.machines.refresh_images(model.proxy.clone());
    if let Some(disk) = options.disk {
        let machines = &model.machines;
        model.volumes.open(&disk, |name| machines.disk_of(name));
    }
    let window_appearance = model.appearance;
    let mut app = App {
        window_appearance,
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

    fn sidebar(current: Page, running: usize, status: &str, ty: Typography) -> UiElement<'_> {
        navigation::view(
            current,
            running,
            status,
            false,
            account::Preview::default(),
            ty,
        )
    }

    /// Text a guest may write into a file name, a log, or a capture's
    /// metadata: scripts and symbols none of the bundled faces cover, an
    /// emoji, a private-use code point, bidirectional overrides, a combining
    /// mark, controls, and a terminal escape.
    const GUEST_TEXT: &str = "日本語のスクショ 😀 \u{E800} \u{202E}txt.exe\u{202C} e\u{301} Ωμέγα Кириллица \u{1b}[31mred\u{1b}[0m\ttab\r\n";

    /// Lays out and draws `element` the way the shell does, and returns the
    /// frame's glyph run count and its text refusals.
    fn draw(element: UiElement<'_>, size: Size) -> (usize, Vec<axui_iced::TextError>) {
        draw_in_theme(element, size, &Theme::Dark)
    }

    fn draw_in_theme(
        element: UiElement<'_>,
        size: Size,
        appearance: &Theme,
    ) -> (usize, Vec<axui_iced::TextError>) {
        draw_at_scale(element, size, appearance, 1., Some("preview"))
    }

    #[derive(Default)]
    struct LayoutBounds(Vec<(iced_core::widget::Id, iced_core::Rectangle)>);
    impl iced_core::widget::Operation for LayoutBounds {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced_core::widget::Operation)) {
            operate(self);
        }
        fn container(&mut self, id: Option<&iced_core::widget::Id>, bounds: iced_core::Rectangle) {
            if let Some(id) = id {
                self.0.push((id.clone(), bounds));
            }
        }
        fn scrollable(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _: iced_core::Rectangle,
            _: iced_core::Vector,
            _: &mut dyn iced_core::widget::operation::Scrollable,
        ) {
            if let Some(id) = id {
                self.0.push((id.clone(), bounds));
            }
        }
    }
    impl LayoutBounds {
        fn get(&self, name: &'static str) -> Option<iced_core::Rectangle> {
            self.0
                .iter()
                .find(|(id, _)| *id == iced_core::widget::Id::new(name))
                .map(|(_, bounds)| *bounds)
        }
        fn check(&self) {
            if let (Some(rail), Some(account)) =
                (self.get("navigation"), self.get("navigation-account"))
            {
                assert!(account.x >= rail.x && account.x + account.width <= rail.x + rail.width);
                assert!(
                    account.y >= rail.y && account.y + account.height <= rail.y + rail.height,
                    "account leaves the navigation rail: {account:?}, {rail:?}"
                );
            }
            if let (Some(frame), Some(content)) = (self.get("page-frame"), self.get("page-content"))
            {
                assert!(
                    (frame.center_x() - content.center_x()).abs() < 1.,
                    "page is off-center: {frame:?}, {content:?}"
                );
                assert!(content.width <= 1440. && content.x >= frame.x + 23.);
                assert!(content.x + content.width <= frame.x + frame.width - 23.);
                if let Some(memory) = self.get("hardware-memory") {
                    assert!(
                        memory.x + memory.width <= content.x + content.width + 1.,
                        "memory panel overflows: {memory:?}, page: {content:?}"
                    );
                }
                if let (Some(list), Some(detail)) =
                    (self.get("machine-list"), self.get("machine-detail"))
                {
                    assert!(
                        list.width <= 360.,
                        "creation panel must not stretch on wide screens"
                    );
                    assert!(
                        (detail.x - (list.x + list.width + 16.)).abs() < 1.,
                        "panes must remain adjacent"
                    );
                    assert!(
                        (detail.x + detail.width - (content.x + content.width)).abs() < 1.,
                        "detail should use the remaining width"
                    );
                }
                if let (Some(appearance), Some(api)) =
                    (self.get("settings-appearance"), self.get("settings-api"))
                {
                    if content.width >= 1240. {
                        assert!(
                            (api.y - appearance.y).abs() < 1.,
                            "wide cards must share a row"
                        );
                        assert!(api.x >= appearance.x + appearance.width + 23.);
                    } else {
                        assert!(
                            (api.x - appearance.x).abs() < 1.,
                            "stacked cards must align"
                        );
                        assert!(api.y >= appearance.y + appearance.height + 23.);
                    }
                    for card in [appearance, api] {
                        assert!(
                            card.x >= content.x
                                && card.x + card.width <= content.x + content.width + 1.
                        );
                    }
                }
            }
        }
    }

    pub(super) fn draw_at_scale(
        element: UiElement<'_>,
        size: Size,
        appearance: &Theme,
        scale: f32,
        snapshot: Option<&str>,
    ) -> (usize, Vec<axui_iced::TextError>) {
        draw_interactive(
            element,
            size,
            appearance,
            scale,
            snapshot,
            mouse::Cursor::Unavailable,
            &[],
        )
    }

    fn draw_interactive(
        element: UiElement<'_>,
        size: Size,
        appearance: &Theme,
        scale: f32,
        snapshot: Option<&str>,
        cursor: mouse::Cursor,
        events: &[iced_core::Event],
    ) -> (usize, Vec<axui_iced::TextError>) {
        fonts::register().expect("MONOLITH faces register");
        let mut renderer = native_renderer();
        renderer.hint(Scale {
            window: scale,
            application: 1.,
        });
        renderer.reset(iced_core::Rectangle::with_size(size));
        let mut interface =
            UserInterface::build(element, size, user_interface::Cache::new(), &mut renderer);
        let mut events = events.to_vec();
        events.push(iced_core::Event::Window(
            iced_core::window::Event::RedrawRequested(Instant::now()),
        ));
        let _ = interface.update(
            &iced_core::window::Headless,
            &shell::Waker::noop(),
            &events,
            cursor,
            &mut renderer,
            &mut shell::Bus::new(),
        );
        interface.draw(
            &mut renderer,
            appearance,
            &renderer::Style {
                text_color: theme::color(appearance, theme::TEXT),
            },
            cursor,
        );
        let mut bounds = LayoutBounds::default();
        interface.operate(&renderer, &mut bounds);
        if snapshot != Some("preview") {
            assert!(bounds.get("page-frame").is_some());
            assert!(bounds.get("page-content").is_some());
        }
        if snapshot == Some("settings") {
            assert!(bounds.get("settings-appearance").is_some());
            assert!(bounds.get("settings-api").is_some());
        }
        bounds.check();
        let frame = renderer.take_frame().expect("a frame");
        axgfx::frame_requirements(frame.view()).expect("desktop accepts the display list");
        assert!(renderer.take_geometry_errors().is_empty());
        assert!(renderer.take_image_errors().is_empty());
        // Optional artifact from the actual desktop renderer for visual QA on
        // hosts where a window server is unavailable. Run one page test at a time.
        if size.width >= 900.0
            && let Some(snapshot) = snapshot
            && let Some(path) = std::env::var_os("VKORE_UI_PREVIEW")
        {
            use std::io::Write;
            let width = (size.width * scale) as u32;
            let height = (size.height * scale) as u32;
            let mut pixels = vec![0; (width * height) as usize];
            let mut backend = NativeBackend::new().expect("native backend");
            backend.set_images(renderer.take_images());
            backend
                .render_changed(
                    &frame,
                    &mut pixels,
                    Extent2d::new(width, height).unwrap(),
                    width as usize,
                )
                .expect("render preview");
            let mode = if matches!(appearance, Theme::Light) {
                "light"
            } else {
                "dark"
            };
            let suffix = if snapshot == "preview" {
                format!("{mode}.ppm")
            } else {
                format!(
                    "{snapshot}.{}x{}@{scale}.{mode}.ppm",
                    size.width, size.height
                )
            };
            let mut output = std::io::BufWriter::new(
                std::fs::File::create(PathBuf::from(path).with_extension(suffix)).unwrap(),
            );
            writeln!(output, "P6\n{width} {height}\n255").unwrap();
            for pixel in pixels {
                output
                    .write_all(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8])
                    .unwrap();
            }
        }
        (frame.glyph_runs().len(), renderer.take_text_errors())
    }

    #[test]
    fn scrollbars_reserve_a_gutter_and_pages_render_after_scrolling() {
        fonts::register().unwrap();
        let mut renderer = native_renderer();
        let content = container(
            widgets::scroll(
                container(text("Tall content"))
                    .id("scroll-content")
                    .width(Length::Fill)
                    .height(1200),
            )
            .height(Length::Fill),
        )
        .id("scroll-viewport")
        .width(360)
        .height(500);
        let mut interface = UserInterface::build(
            content,
            Size::new(360., 500.),
            user_interface::Cache::new(),
            &mut renderer,
        );
        let mut bounds = LayoutBounds::default();
        interface.operate(&renderer, &mut bounds);
        let viewport = bounds.get("scroll-viewport").unwrap();
        let content = bounds.get("scroll-content").unwrap();
        assert!(
            viewport.x + viewport.width - content.x - content.width >= 12.,
            "scrollbar overlaps content: {viewport:?}, {content:?}"
        );

        let root = tempfile::tempdir().unwrap();
        let machines = machines::State::new(root.path().to_owned());
        let settings = settings::State::new(root.path().to_owned());
        for appearance in [Theme::Light, Theme::Dark] {
            for (size, scale) in [(Size::new(900., 560.), 1.), (Size::new(1280., 800.), 2.)] {
                for page in [Page::Machines, Page::Settings] {
                    let content = match page {
                        Page::Machines => {
                            machines::view(&machines, Typography::default(), appearance.clone())
                        }
                        _ => settings::view(theme::Appearance::System, None, &settings),
                    };
                    let element =
                        container(row![sidebar(page, 0, "", Typography::default()), content])
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(|appearance| {
                                theme::panel(appearance, theme::BACKGROUND, None, 0.)
                            })
                            .into();
                    let (_, refused) = draw_interactive(
                        element,
                        size,
                        &appearance,
                        scale,
                        Some(if page == Page::Machines {
                            "machines-scrolled"
                        } else {
                            "settings-scrolled"
                        }),
                        mouse::Cursor::Available(iced_core::Point::new(400., 300.)),
                        &[iced_core::Event::Mouse(mouse::Event::WheelScrolled {
                            delta: mouse::ScrollDelta::Lines { x: 0., y: -4. },
                        })],
                    );
                    assert!(refused.is_empty(), "{refused:?}");
                }
            }
        }
    }

    #[test]
    fn navigation_buttons_work_in_both_modes() {
        fonts::register().unwrap();
        for collapsed in [false, true] {
            let mut renderer = native_renderer();
            renderer.reset(iced_core::Rectangle::with_size(Size::new(900., 560.)));
            let content = navigation::view(
                Page::Machines,
                0,
                "",
                collapsed,
                account::Preview::default(),
                Typography::default(),
            );
            let mut interface = UserInterface::build(
                content,
                Size::new(900., 560.),
                user_interface::Cache::new(),
                &mut renderer,
            );
            let mut bounds = LayoutBounds::default();
            interface.operate(&renderer, &mut bounds);
            for id in ["navigation-toggle", "navigation-account"] {
                let center = bounds.get(id).expect("navigation control").center();
                let mut messages = shell::Bus::new();
                let events = [
                    iced_core::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                    iced_core::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                ];
                let _ = interface.update(
                    &iced_core::window::Headless,
                    &shell::Waker::noop(),
                    &events,
                    mouse::Cursor::Available(center),
                    &mut renderer,
                    &mut messages,
                );
                interface.operate(&renderer, &mut LayoutBounds::default());
                interface.draw(
                    &mut renderer,
                    &Theme::Light,
                    &renderer::Style {
                        text_color: theme::color(&Theme::Light, theme::TEXT),
                    },
                    mouse::Cursor::Available(center),
                );
                let frame = renderer.take_frame().unwrap();
                axgfx::frame_requirements(frame.view())
                    .expect("desktop accepts the hovered navigation");
                renderer.reset(iced_core::Rectangle::with_size(Size::new(900., 560.)));
                let messages: Vec<_> = messages.into_iter().collect();
                assert_eq!(messages.len(), 1, "{id}: {messages:?}");
                assert!(matches!(
                    (id, &messages[0]),
                    ("navigation-toggle", Message::ToggleNavigation)
                        | ("navigation-account", Message::Show(Page::Account))
                ));
            }
        }
    }

    #[test]
    fn account_navigation_fits_both_widths_and_large_text() {
        let root = tempfile::tempdir().unwrap();
        let mut settings = settings::State::new(root.path().to_owned());
        for large in [false, true] {
            settings.typography.text_size = if large {
                typography::TextSize::ExtraLarge
            } else {
                typography::TextSize::Default
            };
            for collapsed in [false, true] {
                for (width, height) in [(900., 560.), (1440., 900.)] {
                    for appearance in [Theme::Light, Theme::Dark] {
                        // Deliberately long external text exercises ellipsis and escaping.
                        let preview = account::Preview {
                            email: Some("a.long.account.name+development@example.com".to_owned()),
                            pending: false,
                        };
                        let content = container(row![
                            navigation::view(
                                Page::Account,
                                2,
                                "",
                                collapsed,
                                preview,
                                settings.typography
                            ),
                            account::page(&settings.account, settings.typography)
                        ])
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.))
                        .into();
                        let snapshot = match (collapsed, large) {
                            (false, false) => "account-expanded",
                            (true, false) => "account-collapsed",
                            (false, true) => "account-expanded-large",
                            (true, true) => "account-collapsed-large",
                        };
                        let (_, refused) = draw_at_scale(
                            content,
                            Size::new(width, height),
                            &appearance,
                            1.,
                            Some(snapshot),
                        );
                        assert!(refused.is_empty(), "{refused:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn account_authorization_renders_at_compact_and_wide_sizes() {
        let root = tempfile::tempdir().unwrap();
        let mut settings = settings::State::new(root.path().to_owned());
        for pending in [false, true] {
            if pending {
                settings.account.preview_sign_in();
            }
            for (width, height) in [(900., 560.), (1760., 1000.)] {
                for appearance in [Theme::Light, Theme::Dark] {
                    let content = account::page(&settings.account, settings.typography);
                    let content = container(row![
                        navigation::view(
                            Page::Account,
                            0,
                            "",
                            false,
                            settings.account.preview(),
                            settings.typography
                        ),
                        content
                    ])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.))
                    .into();
                    let (_, refused) = draw_at_scale(
                        content,
                        Size::new(width, height),
                        &appearance,
                        1.,
                        Some("account"),
                    );
                    assert!(refused.is_empty(), "{refused:?}");
                }
            }
        }
    }

    #[test]
    fn all_pages_and_splash_render_in_each_appearance() {
        let root = tempfile::tempdir().unwrap();
        let store = vkored::MachineStore::new(vkored::State::open(root.path()).unwrap());
        store
            .create(&vkored::MachineConfig::new("inference", "/images/kore.iso"))
            .unwrap();
        let mut machines = machines::State::new(root.path().to_owned());
        machines.selected = Some("inference".into());
        let (volumes, _worker) = guest_listing();
        let settings = settings::State::new(root.path().to_owned());
        for (choice, system_dark) in [
            (theme::Appearance::Dark, None),
            (theme::Appearance::Light, None),
            (theme::Appearance::System, Some(true)),
            (theme::Appearance::System, Some(false)),
        ] {
            let appearance = choice.iced(system_dark);
            for page in [Page::Volumes, Page::Machines, Page::Settings, Page::Account] {
                let content = match page {
                    Page::Volumes => volumes::view(&volumes, &[], Typography::default()),
                    Page::Machines => {
                        machines::view(&machines, Typography::default(), appearance.clone())
                    }
                    Page::Settings => settings::view(choice, system_dark, &settings),
                    Page::Account => account::page(&settings.account, settings.typography),
                };
                let shell = container(row![sidebar(page, 0, "", Typography::default()), content])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.0));
                let (runs, refused) =
                    draw_in_theme(shell.into(), Size::new(1280.0, 800.0), &appearance);
                assert!(runs > 10);
                assert!(refused.is_empty(), "{choice:?}: {refused:?}");
            }
            let (_, refused) = draw_in_theme(
                splash::overlay(splash::progress(Duration::from_millis(1600)).unwrap()),
                Size::new(640.0, 480.0),
                &appearance,
            );
            assert!(refused.is_empty());
        }
    }

    #[test]
    fn hardware_editor_renders_with_hybrid_cores_and_both_appearances() {
        let root = tempfile::tempdir().unwrap();
        let mut state = machines::State::new(root.path().to_owned());
        let host = state.host.as_mut().unwrap();
        host.cpu_name = "Hybrid CPU fixture".into();
        host.core_summary = "4 performance cores · 4 efficiency cores".into();
        host.physical_cores = 8;
        host.logical_cpus = 12;
        host.memory_mib = 32768;
        host.available_memory_mib = 24576;
        host.free_memory_mib = 8192;
        host.storage_available_mib = 131072;
        host.cpu_pinning = true;
        host.cpu_power_management = Ok(());
        host.acceleration = vkore_vm::HostCaps {
            vendor: Some(vkore_vm::Vendor::Intel),
            kvm: true,
            hvf: false,
        };
        host.memory_pinning = true;
        host.hugetlb_supported = true;
        host.system_page_bytes = 4096;
        host.memlock_bytes = Some(65536);
        host.cores = (0..8)
            .map(|id| vkore_vm::hardware::Core {
                package: 0,
                id,
                kind: if id < 4 { "Performance" } else { "Efficiency" }.into(),
                threads: if id < 4 { vec![id, id + 8] } else { vec![id] },
                numa_node: Some(0),
            })
            .collect();
        state.form_hardware = hardware::Draft::new(Some(host), Default::default());
        state.form_hardware.show_cores = true;
        state.configure_new = true;
        for ty in [
            Typography::default(),
            Typography {
                text_size: typography::TextSize::ExtraLarge,
                preset: typography::FontPreset::Mono,
            },
        ] {
            for expanded in [false, true] {
                state.form_hardware.show_cores = expanded;
                for (width, height, scale) in [
                    (900., 560., 1.),
                    (1280., 900., 1.),
                    (2560., 1440., 1.),
                    (1280., 900., 2.),
                ] {
                    for appearance in [Theme::Light, Theme::Dark] {
                        let shell = container(row![
                            sidebar(Page::Machines, 0, "", ty),
                            machines::view(&state, ty, appearance.clone())
                        ])
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(|t| theme::panel(t, theme::BACKGROUND, None, 0.));
                        let (_, refused) = draw_at_scale(
                            shell.into(),
                            Size::new(width, height),
                            &appearance,
                            scale,
                            Some(match (expanded, ty == Typography::default()) {
                                (true, true) => "hardware",
                                (false, true) => "hardware-summary",
                                (true, false) => "hardware-large",
                                (false, false) => "hardware-summary-large",
                            }),
                        );
                        assert!(refused.is_empty(), "{refused:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn memory_controls_render_presets_and_precise_input() {
        let root = tempfile::tempdir().unwrap();
        let mut host = vkore_vm::hardware::HostResources::probe(root.path()).unwrap();
        host.memory_mib = 128 * 1024;
        host.available_memory_mib = 80 * 1024;
        host.hugetlb_supported = true;
        host.memory_pinning = true;
        host.system_page_bytes = 4096;
        host.hugepages = [
            vkore_vm::hardware::PageSize::Huge2M,
            vkore_vm::hardware::PageSize::Huge1G,
        ]
        .into_iter()
        .map(|page| vkore_vm::hardware::HugePool { page, free_mib: 0 })
        .collect();
        host.memory_nodes.clear();
        let mut draft = hardware::Draft::new(Some(&host), Default::default());
        draft.change(hardware::Change::MemoryText("12.5".into()), &host);
        draft.change(
            hardware::Change::Page(vkore_vm::hardware::PageSize::Huge2M),
            &host,
        );
        for (width, height) in [(900., 560.), (1280., 900.)] {
            let ty = if width < 1000. {
                Typography {
                    text_size: typography::TextSize::ExtraLarge,
                    preset: typography::FontPreset::Mono,
                }
            } else {
                Typography::default()
            };
            for appearance in [Theme::Light, Theme::Dark] {
                let content = widgets::page_frame(
                    widgets::scroll(hardware::memory_view(&draft, &host, false, false, ty))
                        .height(Length::Fill),
                );
                let shell = container(row![sidebar(Page::Machines, 0, "", ty), content])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|t| theme::panel(t, theme::BACKGROUND, None, 0.));
                let (_, refused) = draw_at_scale(
                    shell.into(),
                    Size::new(width, height),
                    &appearance,
                    1.,
                    Some("memory-capacity"),
                );
                assert!(refused.is_empty(), "{refused:?}");
            }
        }
    }

    #[test]
    fn storage_controls_render_model_sizes_at_compact_and_large_fonts() {
        let root = tempfile::tempdir().unwrap();
        let mut host = vkore_vm::hardware::HostResources::probe(root.path()).unwrap();
        host.storage_available_mib = 96 * 1024;
        for sparse in [false, true] {
            let mut draft = hardware::Draft::new(Some(&host), Default::default());
            if sparse {
                draft.change(hardware::Change::Reserve, &host);
            }
            draft.change(
                hardware::Change::Disk(if sparse { 256 } else { 64 } * 1024),
                &host,
            );
            for ty in [
                Typography::default(),
                Typography {
                    text_size: typography::TextSize::ExtraLarge,
                    preset: typography::FontPreset::Mono,
                },
            ] {
                for (width, height) in [(900., 560.), (1280., 900.)] {
                    for appearance in [Theme::Light, Theme::Dark] {
                        let content = widgets::page_frame(
                            widgets::scroll(hardware::storage_view(
                                &draft, &host, false, false, ty,
                            ))
                            .height(Length::Fill),
                        );
                        let shell = container(row![sidebar(Page::Machines, 0, "", ty), content])
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(|t| theme::panel(t, theme::BACKGROUND, None, 0.));
                        let (_, refused) = draw_at_scale(
                            shell.into(),
                            Size::new(width, height),
                            &appearance,
                            1.,
                            Some(match (sparse, ty == Typography::default()) {
                                (false, true) => "storage-reserved",
                                (true, true) => "storage-sparse",
                                (false, false) => "storage-reserved-large",
                                (true, false) => "storage-sparse-large",
                            }),
                        );
                        assert!(refused.is_empty(), "{refused:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn gpu_provisioning_controls_render_in_both_themes_and_sizes() {
        let root = tempfile::tempdir().unwrap();
        let mut host = vkore_vm::hardware::HostResources::probe(root.path()).unwrap();
        host.gpus.devices = vec![vkore_vm::devices::Gpu {
            name: "Intel GPU · b080".into(),
            driver: Some("xe".into()),
            pci: vkore_vm::devices::Pci {
                address: "0000:00:02.0".into(),
                vendor_id: 0x8086,
                device_id: 0xb080,
                revision: None,
                iommu_group: Some(1),
                group_members: vec!["0000:00:02.0".into()],
                physical_function: None,
                total_vfs: Some(7),
                enabled_vfs: Some(0),
            },
            virtual_functions: vec![],
        }];
        let mut draft = hardware::Draft::new(Some(&host), Default::default());
        draft.change(
            hardware::Change::Gpu(
                vkore_vm::passthrough::gpu::Assignment::new(&host.gpus.devices[0], 0),
                true,
            ),
            &host,
        );
        assert_eq!(draft.resources.gpu_vfs.len(), 1);
        // Exercise the desktop draft through the actual on-disk format, then
        // edit an unrelated setting. A GPU choice must survive both saves.
        let store = vkored::MachineStore::new(vkored::State::open(root.path()).unwrap());
        let mut config = vkored::MachineConfig::new("gpu", "/image.iso");
        store.create(&config).unwrap();
        draft.apply(&mut config);
        store.save(&config).unwrap();
        let saved = store.get("gpu").unwrap();
        assert_eq!(saved.resources.gpu_vfs, draft.resources.gpu_vfs);
        let mut reopened = hardware::Draft::from_config(&saved, false);
        reopened.change(
            hardware::Change::GuestNuma(vkore_vm::hardware::GuestNuma::Single),
            &host,
        );
        reopened.apply(&mut config);
        store.save(&config).unwrap();
        assert_eq!(
            store.get("gpu").unwrap().resources.gpu_vfs,
            draft.resources.gpu_vfs
        );
        for (width, height) in [(900., 560.), (1280., 900.)] {
            let ty = if width < 1000. {
                Typography {
                    text_size: typography::TextSize::ExtraLarge,
                    preset: typography::FontPreset::Mono,
                }
            } else {
                Typography::default()
            };
            for appearance in [Theme::Light, Theme::Dark] {
                let content = widgets::page_frame(
                    iced_widget::column![
                        ty.page_header("Machine hardware", "GPU sharing and provisioning"),
                        widgets::scroll(hardware::gpu_inventory(&draft, &host, false, false, ty))
                            .height(Length::Fill),
                    ]
                    .spacing(18),
                );
                let shell = container(row![sidebar(Page::Machines, 0, "", ty), content])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|t| theme::panel(t, theme::BACKGROUND, None, 0.));
                let (_, refused) = draw_at_scale(
                    shell.into(),
                    Size::new(width, height),
                    &appearance,
                    1.,
                    Some("gpu"),
                );
                assert!(refused.is_empty(), "{refused:?}");
            }
        }
        let selection = draft.resources.gpu_vfs[0].clone();
        host.gpus.devices.clear();
        draft.change(hardware::Change::Gpu(selection.clone(), false), &host);
        assert!(draft.resources.gpu_vfs.is_empty());
        draft.change(hardware::Change::Gpu(selection, true), &host);
        assert!(draft.resources.gpu_vfs.is_empty());
    }

    #[test]
    fn npu_inventory_renders_devices_routes_and_failure_states() {
        use vkore_vm::devices::{DiscoveryStatus, Inventory, Npu, Pci, Platform, Source, Vendor};
        let device = Npu {
            id: "pci:0001:00:0b.0".into(),
            name: "Intel NPU · Panther Lake".into(),
            vendor: Vendor::Intel,
            source: Source::LinuxPci,
            driver: Some("ivpu".into()),
            core_count: None,
            pci: Some(Pci {
                address: "0001:00:0b.0".into(),
                vendor_id: 0x8086,
                device_id: 0xb03e,
                revision: Some(1),
                iommu_group: Some(12),
                group_members: vec!["0001:00:0b.0".into()],
                physical_function: None,
                total_vfs: Some(0),
                enabled_vfs: None,
            }),
        };
        for scenario in [
            "linux", "selected", "missing", "busy", "macos", "windows", "empty", "failed",
            "partial",
        ] {
            let mut inventory = Inventory::empty(Platform::Linux);
            match scenario {
                "linux" | "selected" | "busy" => inventory.devices.push(device.clone()),
                "macos" => {
                    inventory.platform = Platform::MacOs;
                    inventory.devices.push(Npu {
                        id: "coreml:neural-engine:0".into(),
                        name: "Apple Neural Engine".into(),
                        vendor: Vendor::Apple,
                        source: Source::CoreMl,
                        driver: None,
                        core_count: Some(16),
                        pci: None,
                    });
                }
                "windows" => {
                    inventory.platform = Platform::Windows;
                    inventory.devices.push(Npu {
                        id: "dxcore:12345678".into(),
                        name: "Qualcomm Hexagon NPU".into(),
                        vendor: Vendor::Qualcomm,
                        source: Source::WindowsDxCore,
                        driver: None,
                        core_count: None,
                        pci: None,
                    });
                }
                "failed" => {
                    inventory = Inventory::unavailable(
                        Platform::Windows,
                        "Windows NPU discovery is unavailable",
                    )
                }
                "partial" => {
                    inventory.status = DiscoveryStatus::Partial;
                    inventory.warnings.push(GUEST_TEXT.into());
                    let mut external = device.clone();
                    external.name = GUEST_TEXT.into();
                    inventory.devices.push(external);
                }
                _ => {}
            }
            let selected = if matches!(scenario, "selected" | "missing" | "busy") {
                vec![vkore_vm::passthrough::Selection::from_device(&device).unwrap()]
            } else {
                vec![]
            };
            for (width, height) in [(900., 560.), (1280., 900.)] {
                for appearance in [Theme::Light, Theme::Dark] {
                    let ty = if width < 1000. {
                        Typography {
                            text_size: typography::TextSize::ExtraLarge,
                            preset: typography::FontPreset::Mono,
                        }
                    } else {
                        Typography::default()
                    };
                    let content = widgets::page_frame(
                        iced_widget::column![
                            ty.page_header(
                                "Machine hardware",
                                "Neural processors and guest access"
                            ),
                            widgets::scroll(hardware::npu_inventory(
                                &inventory,
                                &selected,
                                false,
                                scenario == "busy",
                                ty
                            ))
                            .height(Length::Fill),
                        ]
                        .spacing(18),
                    );
                    let shell = container(row![sidebar(Page::Machines, 0, "", ty), content])
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(|t| theme::panel(t, theme::BACKGROUND, None, 0.));
                    let snapshot = format!("npus-{scenario}");
                    let (_, refused) = draw_at_scale(
                        shell.into(),
                        Size::new(width, height),
                        &appearance,
                        1.,
                        Some(&snapshot),
                    );
                    assert!(refused.is_empty(), "{scenario}: {refused:?}");
                }
            }
        }
    }

    #[test]
    fn macos_memory_controls_show_native_pages_and_recovery() {
        let root = tempfile::tempdir().unwrap();
        let mut state = machines::State::new(root.path().to_owned());
        let host = state.host.as_mut().unwrap();
        host.cpu_name = "Apple M4".into();
        host.core_summary = "4 Performance · 6 Efficiency".into();
        host.physical_cores = 10;
        host.logical_cpus = 10;
        host.cpu_pinning = false;
        host.memory_pinning = false;
        host.hugetlb_supported = false;
        host.system_page_bytes = 16384;
        host.memory_mib = 16384;
        host.available_memory_mib = 8192;
        host.free_memory_mib = 512;
        host.memory_nodes.clear();
        host.hugepages.clear();
        state.form_hardware = hardware::Draft::new(Some(host), Default::default());
        state.configure_new = true;
        for recovery in [false, true] {
            state.form_hardware.resources.pages = if recovery {
                vkore_vm::hardware::PageSize::Huge2M
            } else {
                vkore_vm::hardware::PageSize::System
            };
            state.form_hardware.resources.pin_memory = recovery;
            for (width, height) in [(900., 560.), (1280., 1200.)] {
                for appearance in [Theme::Light, Theme::Dark] {
                    let ty = Typography::default();
                    let shell = container(row![
                        sidebar(Page::Machines, 0, "", ty),
                        machines::view(&state, ty, appearance.clone())
                    ])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|t| theme::panel(t, theme::BACKGROUND, None, 0.));
                    let (_, refused) = draw_at_scale(
                        shell.into(),
                        Size::new(width, height),
                        &appearance,
                        1.,
                        Some(if recovery {
                            "macos-memory-recovery"
                        } else {
                            "macos-memory"
                        }),
                    );
                    assert!(refused.is_empty(), "{refused:?}");
                }
            }
        }
    }

    #[test]
    fn typography_presets_render_all_pages_at_large_text_and_compact_sizes() {
        use typography::{FontPreset, TextSize};
        let root = tempfile::tempdir().unwrap();
        let machines = machines::State::new(root.path().to_owned());
        let (volumes, _worker) = guest_listing();
        let mut settings = settings::State::new(root.path().to_owned());
        for preset in FontPreset::ALL {
            for size in TextSize::ALL {
                let ty = Typography {
                    preset,
                    text_size: size,
                };
                settings.typography = ty;
                for (width, height) in [(900., 560.), (1280., 900.)] {
                    for appearance in [Theme::Light, Theme::Dark] {
                        for page in [Page::Machines, Page::Volumes, Page::Settings, Page::Account] {
                            let content = match page {
                                Page::Machines => machines::view(&machines, ty, appearance.clone()),
                                Page::Volumes => volumes::view(&volumes, &[], ty),
                                Page::Account => account::page(&settings.account, ty),
                                Page::Settings => {
                                    settings::view(theme::Appearance::System, None, &settings)
                                }
                            };
                            let shell = container(row![sidebar(page, 0, "", ty), content])
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .style(|t| theme::panel(t, theme::BACKGROUND, None, 0.));
                            let snapshot = (page == Page::Settings
                                && preset == FontPreset::Mono
                                && size == TextSize::ExtraLarge)
                                .then_some("typography");
                            let (_, refused) = draw_at_scale(
                                shell.into(),
                                Size::new(width, height),
                                &appearance,
                                1.,
                                snapshot,
                            );
                            assert!(
                                refused.is_empty(),
                                "{preset:?} {size:?} {page:?}: {refused:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pages_adapt_across_resolutions_and_display_scales() {
        let root = tempfile::tempdir().unwrap();
        let machines = machines::State::new(root.path().to_owned());
        let (volumes, _worker) = guest_listing();
        let settings = settings::State::new(root.path().to_owned());
        for (width, height, scale) in [
            (900., 560., 1.),
            (1280., 800., 1.),
            (1500., 900., 1.),
            (1536., 900., 1.),
            (1920., 1080., 1.),
            (2560., 1440., 1.),
            (900., 1200., 1.),
            (900., 560., 2.),
            (1280., 800., 2.),
        ] {
            for appearance in [Theme::Dark, Theme::Light] {
                for page in [Page::Machines, Page::Volumes, Page::Settings, Page::Account] {
                    let content = match page {
                        Page::Machines => {
                            machines::view(&machines, Typography::default(), appearance.clone())
                        }
                        Page::Volumes => volumes::view(&volumes, &[], Typography::default()),
                        Page::Account => account::page(&settings.account, settings.typography),
                        Page::Settings => settings::view(
                            theme::Appearance::System,
                            Some(appearance == Theme::Dark),
                            &settings,
                        ),
                    };
                    let shell =
                        container(row![sidebar(page, 0, "", Typography::default()), content])
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(|appearance| {
                                theme::panel(appearance, theme::BACKGROUND, None, 0.)
                            });
                    let snapshot = (page == Page::Settings
                        && matches!(width as u32, 900 | 1280 | 2560)
                        && height != 1200.)
                        .then_some("settings");
                    let (_, refused) = draw_at_scale(
                        shell.into(),
                        Size::new(width, height),
                        &appearance,
                        scale,
                        snapshot,
                    );
                    assert!(
                        refused.is_empty(),
                        "{page:?} {width}x{height}@{scale}: {refused:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn compact_pages_draw_without_text_refusals() {
        let root = tempfile::tempdir().unwrap();
        let store = vkored::MachineStore::new(vkored::State::open(root.path()).unwrap());
        store
            .create(&vkored::MachineConfig::new("inference", "/images/kore.iso"))
            .unwrap();
        let mut machines = machines::State::new(root.path().to_owned());
        machines.selected = Some("inference".into());
        let (volumes, _worker) = guest_listing();
        let settings = settings::State::new(root.path().to_owned());
        for appearance in [Theme::Dark, Theme::Light] {
            for page in [Page::Volumes, Page::Machines, Page::Settings, Page::Account] {
                let content = match page {
                    Page::Volumes => volumes::view(&volumes, &[], Typography::default()),
                    Page::Machines => {
                        machines::view(&machines, Typography::default(), appearance.clone())
                    }
                    Page::Account => account::page(&settings.account, settings.typography),
                    Page::Settings => settings::view(
                        theme::Appearance::System,
                        Some(appearance == Theme::Dark),
                        &settings,
                    ),
                };
                let shell = container(row![sidebar(page, 0, "", Typography::default()), content])
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.));
                let (_, refused) = draw_in_theme(shell.into(), Size::new(900., 560.), &appearance);
                assert!(refused.is_empty(), "{page:?}: {refused:?}");
            }
        }
    }

    #[test]
    fn empty_workspace_draws_in_each_appearance() {
        let root = tempfile::tempdir().unwrap();
        let machines = machines::State::new(root.path().to_owned());
        let (sender, _worker) = mpsc::channel();
        let volumes = volumes::State::new(sender);
        for size in [Size::new(1280., 800.), Size::new(900., 560.)] {
            for appearance in [Theme::Dark, Theme::Light] {
                for page in [Page::Machines, Page::Volumes] {
                    let content = match page {
                        Page::Machines => {
                            machines::view(&machines, Typography::default(), appearance.clone())
                        }
                        _ => volumes::view(&volumes, &[], Typography::default()),
                    };
                    let shell =
                        container(row![sidebar(page, 0, "", Typography::default()), content])
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .style(|appearance| {
                                theme::panel(appearance, theme::BACKGROUND, None, 0.)
                            });
                    let (_, refused) = draw_in_theme(shell.into(), size, &appearance);
                    assert!(refused.is_empty(), "{page:?}: {refused:?}");
                }
            }
        }
    }

    #[test]
    fn launch_theme_can_override_the_saved_choice() {
        assert_eq!(
            parse_launch_options(["--theme".into(), "light".into()])
                .unwrap()
                .appearance,
            Some(theme::Appearance::Light)
        );
        assert!(parse_launch_options(["--theme".into(), "unknown".into()]).is_err());
        assert!(parse_launch_options(["--theme".into()]).is_err());
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
            read_only: false,
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
    fn volume_choosers_render_at_small_and_large_sizes() {
        for size in [Size::new(760.0, 600.0), Size::new(1280.0, 800.0)] {
            for light in [true, false] {
                let appearance = if light { Theme::Light } else { Theme::Dark };
                let (sender, _receiver) = mpsc::channel();
                let mut state = volumes::State::new(sender);
                let (_, errors) = draw_at_scale(
                    volumes::view(&state, &[], Typography::default()),
                    size,
                    &appearance,
                    1.0,
                    Some("volume-open"),
                );
                assert!(errors.is_empty());
                state.disk_choices = Some(vec![
                    vkore_volume::PhysicalDisk {
                        path: "/dev/nvme0n1p5".into(),
                        description: "Partition · Kore workspace".into(),
                        bytes: 256 << 30,
                    },
                    vkore_volume::PhysicalDisk {
                        path: "/dev/sdb".into(),
                        description: "Disk · USB storage".into(),
                        bytes: 64 << 30,
                    },
                ]);
                let (_, errors) = draw_at_scale(
                    volumes::view(&state, &[], Typography::default()),
                    size,
                    &appearance,
                    1.0,
                    Some("volume-disks"),
                );
                assert!(errors.is_empty());
            }
        }
    }

    #[test]
    fn the_volume_page_draws_a_guest_written_listing_and_its_capture() {
        let (mut state, _worker) = guest_listing();
        let machines = vec!["demo".to_owned(), GUEST_TEXT.to_owned()];
        let (runs, refused) = draw(
            volumes::view(&state, &machines, Typography::default()),
            Size::new(1280.0, 800.0),
        );
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
        let (_, refused) = draw(
            volumes::view(&state, &machines, Typography::default()),
            Size::new(1280.0, 800.0),
        );
        assert_eq!(refused, []);
    }

    #[test]
    fn the_shared_kernels_draw_the_volume_page_exactly_as_the_reference() {
        for appearance in [Theme::Dark, Theme::Light] {
            let (state, _worker) = guest_listing();
            let machines = vec!["demo".to_owned()];
            let size = Size::new(1280.0, 800.0);
            fonts::register().expect("MONOLITH faces register");
            let mut renderer = native_renderer();
            renderer.reset(iced_core::Rectangle::with_size(size));
            let mut interface = UserInterface::build(
                volumes::view(&state, &machines, Typography::default()),
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
                &appearance,
                &renderer::Style {
                    text_color: theme::color(&appearance, theme::TEXT),
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
    }

    #[test]
    fn a_selected_official_image_draws_in_each_appearance() {
        let root = tempfile::tempdir().unwrap();
        let mut machines = machines::State::new(root.path().to_owned());
        // Test-only release metadata; never offered by the bundled catalog.
        let document = r#"{"schema_version":1,"images":[{
            "id":"test-inference-1","name":"Kore inference","version":"test-1",
            "profile":"inference","architecture":"x86_64","format":"iso",
            "url":"https://example.invalid/test.iso",
            "sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "bytes":40960,"kore_revision":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "dagfs_format_version":FORMAT_VERSION
        }]}"#
            .replace(
                "FORMAT_VERSION",
                &vkore_volume::bounds::FORMAT_VERSION.to_string(),
            );
        machines.official_images = vkored::catalog::Catalog::parse(document.as_bytes())
            .unwrap()
            .images()
            .to_vec();
        machines.selected_image = Some("test-inference-1".into());
        machines.form_name = "inference".into();
        machines.image_menu_open = true;
        for appearance in [Theme::Dark, Theme::Light] {
            let shell = container(row![
                sidebar(Page::Machines, 0, "", Typography::default()),
                machines::view(&machines, Typography::default(), appearance.clone())
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.0));
            let (runs, refused) =
                draw_in_theme(shell.into(), Size::new(1280.0, 800.0), &appearance);
            assert!(refused.is_empty(), "{refused:?}");
            assert!(runs >= 8, "the creation form drew only {runs} runs");
        }
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
        for appearance in [Theme::Dark, Theme::Light] {
            let shell = container(row![
                sidebar(Page::Machines, 0, "", Typography::default()),
                machines::view(&machines, Typography::default(), appearance.clone())
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.0));
            let (runs, refused) =
                draw_in_theme(shell.into(), Size::new(1280.0, 800.0), &appearance);
            assert_eq!(refused, []);
            assert!(runs > 10, "the page drew only {runs} runs");
        }

        let (runs, refused) = draw(
            sidebar(Page::Machines, 1, GUEST_TEXT, Typography::default()),
            Size::new(220.0, 800.0),
        );
        assert_eq!(refused, []);
        assert!(runs > 3);
    }
}
