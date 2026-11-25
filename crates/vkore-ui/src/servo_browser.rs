//! Host chrome for the optional native Servo helper. All browser IO is off the UI thread.
//!
//! The helper owns the engine and sends what the page says about itself: its
//! title, icon, address, load progress, pointer shape and the link under the
//! pointer. This module keeps that per tab and draws the browser around the
//! page: tabs that carry an icon and a title, a toolbar with an address bar, a
//! progress line, and a status line. The page itself is a native surface the
//! desktop composes over the tile; nothing here reads its pixels.
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
    process::{Child, Command as Process, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use iced_core::{Length, Padding, Rectangle, Theme, alignment, image::Handle, keyboard, mouse};
use iced_widget::{button, canvas, column, container, image, row, space, stack, text_input};
use iced_winit::winit::event_loop::EventLoopProxy;
use vkore_servo_wire::{self as wire, Command, Event, Pointer};

use crate::{
    AxUi, Message, UiElement, UserEvent, fonts, kit,
    motion::{self, Beat},
    plugins, theme,
    typography::Typography,
    visuals::{self, Icon},
    widgets,
};

#[derive(Clone, Debug)]
pub enum Action {
    New,
    Close(u64),
    Select(u64),
    Edit(u64, String),
    Go(u64),
    Send(Command),
}

/// How long the progress line takes to move between the stops a load reports.
const PROGRESS: Duration = Duration::from_millis(420);

/// One tab, as the page last described it.
struct Tab {
    /// What the address bar shows: the page's address, or what is being typed.
    address: String,
    /// The page's own address.
    location: String,
    host: String,
    /// The page's title, as the helper sent it (outside text: shown through
    /// the data face).
    title: String,
    favicon: Option<Handle>,
    /// The address of the link under the pointer.
    status: String,
    cursor: wire::Cursor,
    back: bool,
    forward: bool,
    phase: wire::Load,
    progress: motion::Ease,
    finished: Option<Instant>,
    dirty: bool,
    waiting: bool,
    size: (u32, u32),
}

impl Default for Tab {
    fn default() -> Self {
        Self {
            address: String::new(),
            location: String::new(),
            host: String::new(),
            title: String::new(),
            favicon: None,
            status: String::new(),
            cursor: wire::Cursor::Default,
            back: false,
            forward: false,
            phase: wire::Load::Done,
            progress: motion::Ease::new(PROGRESS),
            finished: None,
            dirty: false,
            waiting: false,
            size: (0, 0),
        }
    }
}

/// The host of an `https://` address, for a tab with no title.
fn host_of(location: &str) -> &str {
    location
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or_default()
}

impl Tab {
    fn loading(&self) -> bool {
        self.phase != wire::Load::Done
    }

    /// What a tab is called: the page's title, else its host, else nothing yet.
    fn label(&self) -> &str {
        let title = self.title.trim();
        if !title.is_empty() {
            title
        } else if !self.host.is_empty() {
            &self.host
        } else {
            "New tab"
        }
    }

    /// Takes in what the helper says about this tab. Returns whether anything
    /// the interface shows changed (a paint request is not one).
    fn notice(&mut self, event: Event, now: Instant) -> bool {
        match event {
            Event::Changed(_) => {
                self.dirty = true;
                false
            }
            Event::Loading(_, load) => {
                match load {
                    wire::Load::Started => {
                        self.progress.jump(0.0);
                        self.progress.aim(0.18, now);
                        self.finished = None;
                    }
                    wire::Load::Parsed => self.progress.aim(0.72, now),
                    wire::Load::Done => {
                        self.progress.aim(1.0, now);
                        self.finished = Some(now);
                    }
                }
                self.phase = load;
                true
            }
            Event::Location(_, url, back, forward) => {
                if self.location != url {
                    self.address =
                        fonts::shown(fonts::MONO, &url, fonts::Breaks::Escape).into_owned();
                    let host = host_of(&url).to_owned();
                    if host != self.host {
                        // A page on another site is not the page whose title
                        // and icon these were; the new ones will follow.
                        self.title.clear();
                        self.favicon = None;
                        self.host = host;
                    }
                    self.location = url;
                }
                self.back = back;
                self.forward = forward;
                true
            }
            Event::Title(_, title) => {
                self.title = title;
                true
            }
            Event::Favicon(_, icon) => {
                // A malformed icon is dropped, never drawn.
                self.favicon = icon
                    .and_then(|icon| icon.pixels().ok().map(|px| (icon.width, icon.height, px)))
                    .map(|(width, height, pixels)| Handle::from_rgba(width, height, pixels));
                true
            }
            Event::Status(_, status) => {
                let changed = self.status != status;
                self.status = status;
                changed
            }
            Event::Cursor(_, cursor) => {
                self.cursor = cursor;
                true
            }
            _ => false,
        }
    }

    /// The progress line: how far, how visible, and whether it is moving.
    fn bar(&self, now: Instant) -> (f32, f32, bool) {
        let level = self.progress.level(now);
        match (self.phase, self.finished) {
            (wire::Load::Done, Some(done)) => {
                // Full, held a moment, then gone.
                let gone = (now.saturating_duration_since(done).as_secs_f32() - 0.25) / 0.3;
                let opacity = (1.0 - gone).clamp(0.0, 1.0);
                (level, opacity, opacity > 0.0)
            }
            (wire::Load::Done, None) => (0.0, 0.0, false),
            _ => (level, 1.0, true),
        }
    }
}

enum Reply {
    #[cfg(target_os = "linux")]
    Dma(u64, wire::DmaBuffer, std::os::fd::OwnedFd),
    Notice(Event),
    End(io::Error),
}

struct Helper(Child);
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct Pane {
    presentation: wire::presentation::Presentation,
    pub bounds: Cell<Option<Rectangle>>,
    presented: Option<u64>,
    visible: bool,
    #[cfg(target_os = "linux")]
    importing: Option<u64>,
    _helper: Helper,
    input: mpsc::SyncSender<Command>,
    output: mpsc::Receiver<Reply>,
    tabs: BTreeMap<u64, Tab>,
    active: u64,
    next: u64,
    status: String,
    renderer: String,
    ready: bool,
    focused: bool,
}

fn helper_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("VKORE_SERVO") {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return Ok(path);
        }
        return Err(io::Error::other(
            "VKORE_SERVO must name an absolute executable path.",
        ));
    }
    Ok(std::env::current_exe()?
        .with_file_name(format!("vkore-servo{}", std::env::consts::EXE_SUFFIX)))
}

impl Pane {
    pub fn open(proxy: EventLoopProxy<UserEvent>) -> io::Result<Self> {
        let mut command = Process::new(helper_path()?);
        let presentation = wire::presentation::Presentation::prepare(&mut command)?;
        #[cfg(target_os = "linux")]
        let (mut stdin, received_fds) = {
            let (host, child) = std::os::unix::net::UnixStream::pair()?;
            command.stdin(Stdio::from(std::os::fd::OwnedFd::from(child)));
            (host.try_clone()?, host)
        };
        #[cfg(not(target_os = "linux"))]
        command.stdin(Stdio::piped());
        let mut helper = Helper(command.stdout(Stdio::piped()).spawn().map_err(|e| {
            io::Error::new(e.kind(), format!("Could not start the native Servo helper ({e}). Install vkore-servo beside vkore-ui or set VKORE_SERVO."))
        })?);
        #[cfg(not(target_os = "linux"))]
        let mut stdin = helper.0.stdin.take().expect("piped");
        let mut stdout = io::BufReader::new(helper.0.stdout.take().expect("piped"));
        let (input, commands) = mpsc::sync_channel::<Command>(64);
        thread::Builder::new()
            .name("vkore-servo-input".into())
            .spawn(move || {
                for command in commands {
                    if wire::write(&mut stdin, &command)
                        .and_then(|()| stdin.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let (replies, output) = mpsc::sync_channel(4);
        thread::Builder::new()
            .name("vkore-servo-output".into())
            .spawn(move || {
                loop {
                    let reply = match wire::read(&mut stdout) {
                        #[cfg(target_os = "linux")]
                        Ok(Event::DmaFrame(id, frame)) => {
                            wire::linux::receive_buffer(&received_fds)
                                .map(|fd| Reply::Dma(id, frame, fd))
                        }
                        result => result.map(Reply::Notice),
                    }
                    .unwrap_or_else(Reply::End);
                    let end = matches!(reply, Reply::End(_));
                    if replies.send(reply).is_err()
                        || proxy.send_event(UserEvent::BrowserFrame).is_err()
                        || end
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            presentation,
            bounds: Cell::default(),
            presented: None,
            visible: false,
            #[cfg(target_os = "linux")]
            importing: None,
            _helper: helper,
            input,
            output,
            tabs: BTreeMap::from([(0, Tab::default())]),
            active: 0,
            next: 1,
            status: "Starting Servo…".into(),
            renderer: String::new(),
            ready: false,
            focused: false,
        })
    }

    fn send(&mut self, command: Command) -> bool {
        match self.input.try_send(command) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(Command::Pointer(_, _, _, Pointer::Move))) => false,
            Err(e) => {
                self.status = format!("Browser input unavailable: {e}");
                false
            }
        }
    }

    pub fn blur(&mut self) {
        self.focused = false;
    }

    /// Takes in everything the helper has said, and paints if a frame is due.
    /// Returns whether the interface has anything new to show.
    pub fn receive(&mut self) -> bool {
        let mut changed = false;
        while let Ok(reply) = self.output.try_recv() {
            changed |= self.handle(reply);
        }
        self.paint_if_due();
        changed
    }

    /// Asks for the next frame the moment one is ready, instead of waiting for
    /// the interface's next redraw to notice: the redraw is paced for the
    /// interface, and a page that changed should not wait on it. Only the
    /// tile that is on screen, one frame in flight, as before.
    fn paint_if_due(&mut self) {
        #[cfg(target_os = "linux")]
        if self.importing.is_some() {
            return;
        }
        let id = self.active;
        let due = self.ready
            && self.bounds.get().is_some()
            && !self.tabs.values().any(|tab| tab.waiting)
            && self
                .tabs
                .get(&id)
                .is_some_and(|tab| tab.dirty && tab.size != (0, 0));
        if due {
            self.update(Action::Send(Command::Paint(id)));
        }
    }

    fn handle(&mut self, reply: Reply) -> bool {
        match reply {
            #[cfg(target_os = "linux")]
            Reply::Dma(id, frame, fd) => {
                if self.active != id {
                    if let Some(tab) = self.tabs.get_mut(&id) {
                        tab.waiting = false;
                    }
                    return true;
                }
                match self.presentation.import(fd, frame) {
                    Ok(()) => self.importing = Some(id),
                    Err(err) => {
                        self.ready = false;
                        self.status = err.to_string();
                    }
                }
                true
            }
            Reply::Notice(Event::DmaFrame(..)) => {
                self.ready = false;
                self.status = "Unexpected GPU buffer transport.".into();
                true
            }
            Reply::Notice(Event::Ready(wire::VERSION, renderer, surface)) => {
                if let Err(e) = self.presentation.attach(surface) {
                    self.status = e.to_string();
                    return true;
                }
                self.renderer = renderer;
                self.ready = true;
                self.status.clear();
                self.send(Command::Open(0));
                true
            }
            Reply::Notice(Event::Ready(..)) => {
                self.status = "The Servo helper uses an incompatible protocol.".into();
                true
            }
            Reply::Notice(Event::Presented(id)) => {
                #[cfg(target_os = "macos")]
                if let Err(error) = self.presentation.receive_frame() {
                    self.ready = false;
                    self.status = error.to_string();
                    return true;
                }
                // The first frame of a tab changes what the interface places;
                // the ones after it are the page, already on the surface.
                let first = self.presented != Some(id);
                self.presented = Some(id);
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.waiting = false;
                }
                first
            }
            Reply::Notice(Event::Error(e)) => {
                self.status = e;
                true
            }
            Reply::Notice(
                event @ (Event::Changed(id)
                | Event::Loading(id, _)
                | Event::Location(id, ..)
                | Event::Title(id, _)
                | Event::Favicon(id, _)
                | Event::Status(id, _)
                | Event::Cursor(id, _)),
            ) => self
                .tabs
                .get_mut(&id)
                .is_some_and(|tab| tab.notice(event, Instant::now())),
            Reply::End(e) => {
                if self.status.is_empty() || self.status == "Starting Servo…" {
                    self.status = if e.kind() == io::ErrorKind::UnexpectedEof {
                        "The browser helper disconnected. Close this tile and reopen Servo to try again.".into()
                    } else {
                        format!("Servo stopped ({e}).")
                    };
                }
                self.ready = false;
                true
            }
        }
    }

    pub fn update(&mut self, action: Action) -> bool {
        match action {
            Action::New if self.ready && self.tabs.len() < wire::MAX_TABS => {
                let id = self.next;
                if self.send(Command::Open(id)) {
                    self.tabs.insert(id, Tab::default());
                    self.presented = None;
                    self.active = id;
                    self.next += 1;
                }
            }
            Action::Select(id)
                if self.ready && self.tabs.contains_key(&id) && self.send(Command::Select(id)) =>
            {
                self.presented = None;
                self.active = id;
                self.tabs.get_mut(&id).unwrap().size = (0, 0);
            }
            Action::Close(id) if self.tabs.contains_key(&id) => {
                if self.tabs.len() == 1 {
                    return true;
                }
                if self.send(Command::Close(id)) {
                    self.tabs.remove(&id);
                    if id == self.active {
                        self.active = *self.tabs.first_key_value().expect("remaining tab").0;
                        self.presented = None;
                        self.tabs.get_mut(&self.active).unwrap().size = (0, 0);
                        self.send(Command::Select(self.active));
                    }
                }
            }
            Action::Edit(id, address)
                if self.active == id && address.len() <= wire::MAX_ADDRESS =>
            {
                self.tabs.get_mut(&id).expect("active tab").address =
                    fonts::shown(fonts::MONO, &address, fonts::Breaks::Escape).into_owned();
            }
            Action::Go(id) if self.ready && self.active == id => {
                self.status.clear();
                self.send(Command::Navigate(id, self.tabs[&id].address.clone()));
            }
            Action::Send(Command::Paint(id)) if id != self.active => {}
            Action::Send(command) if self.ready && self.send(command.clone()) => match command {
                Command::Pointer(_, _, _, Pointer::Down) => self.focused = true,
                Command::Resize(id, w, h, _) => {
                    if let Some(tab) = self.tabs.get_mut(&id) {
                        tab.size = (w, h);
                    }
                }
                Command::Paint(id) => {
                    if let Some(tab) = self.tabs.get_mut(&id) {
                        tab.dirty = false;
                        tab.waiting = true;
                    }
                }
                _ => {}
            },
            _ => {}
        }
        false
    }

    pub fn present(&mut self, window: &std::rc::Rc<crate::winit::window::Window>) -> bool {
        let mut redraw = false;
        #[cfg(target_os = "linux")]
        match self.presentation.poll() {
            Ok(progress) => {
                redraw |= progress.pending && self.bounds.get().is_some();
                if progress.imported
                    && let Some(id) = self.importing.take()
                {
                    self.presented = Some(id);
                    if let Some(tab) = self.tabs.get_mut(&id) {
                        tab.waiting = false;
                    }
                    redraw = true;
                }
            }
            Err(e) => {
                redraw |= self.ready;
                self.ready = false;
                self.status = e.to_string();
            }
        }
        const SIZE_LIMIT: &str = "The browser surface exceeds its display limit.";
        let mut bounds = self
            .bounds
            .get()
            .filter(|r| r.width >= 1. && r.height >= 1.);
        if let Some(r) = bounds
            && self.ready
        {
            let scale = window.scale_factor() as f32;
            let size = (
                (r.width * scale).round() as u32,
                (r.height * scale).round() as u32,
            );
            if wire::check_size(size.0, size.1).is_err() {
                redraw |= self.status != SIZE_LIMIT;
                self.status = SIZE_LIMIT.into();
                bounds = None;
            } else {
                if self.status == SIZE_LIMIT {
                    self.status.clear();
                    redraw = true;
                }
                if self.tabs[&self.active].size != size {
                    redraw = true;
                    self.update(Action::Send(Command::Resize(
                        self.active,
                        size.0,
                        size.1,
                        scale,
                    )));
                }
            }
        }
        let visible = bounds.filter(|_| self.ready && self.presented == Some(self.active));
        if let Err(error) = self.presentation.place(
            window,
            window.scale_factor(),
            visible.map(|r| wire::presentation::TileRect {
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            }),
        ) {
            redraw |= self.ready;
            self.status = error.to_string();
            self.ready = false;
        } else if self.visible != visible.is_some()
            && self.send(Command::ShowSurface(visible.is_some()))
        {
            self.visible = visible.is_some();
        }
        redraw
    }

    pub fn view<'a>(&'a self, machine: &'a str, tile: u64, ty: Typography) -> UiElement<'a> {
        #[cfg(target_os = "linux")]
        let importing = self.importing.is_some();
        #[cfg(not(target_os = "linux"))]
        let importing = false;
        chrome(
            Chrome {
                tabs: &self.tabs,
                active: self.active,
                tile,
                ready: self.ready,
                status: &self.status,
                renderer: &self.renderer,
                bounds: &self.bounds,
                focused: self.focused,
                importing,
            },
            ty,
            Instant::now(),
            move |action| Message::Plugin(plugins::Action::Servo(machine.into(), tile, action)),
        )
    }
}

/// What the browser around the page needs to know, apart from how to reach
/// the helper: plain data, so the chrome can be drawn without one.
struct Chrome<'a> {
    tabs: &'a BTreeMap<u64, Tab>,
    active: u64,
    tile: u64,
    ready: bool,
    status: &'a str,
    renderer: &'a str,
    bounds: &'a Cell<Option<Rectangle>>,
    focused: bool,
    importing: bool,
}

/// The tab's small picture: a turning mark while it loads, its icon when it
/// has one, a globe when it has none.
fn tab_lead<'a>(tab: &Tab, ty: Typography) -> UiElement<'a> {
    let side = ty.size(16.);
    if tab.loading() {
        motion::beacon(Beat::Spin, theme::TEXT_SECONDARY, side)
    } else if let Some(icon) = &tab.favicon {
        image(icon.clone()).width(side).height(side).into()
    } else {
        visuals::icon(Icon::Globe, theme::TEXT_MUTED, side)
    }
}

/// The tab strip: each tab an icon and a title cut with an ellipsis, the whole
/// of it (title and address) a tooltip away, a close control, and a new-tab
/// control after the last.
fn tab_strip<'a>(
    c: &Chrome<'a>,
    ty: Typography,
    emit: impl Fn(Action) -> Message + Copy + 'a,
) -> UiElement<'a> {
    let mut strip = row![]
        .spacing(theme::SPACE_1)
        .align_y(alignment::Vertical::Center);
    for (&id, tab) in c.tabs {
        let kind = if id == c.active {
            widgets::Button::Choice(true)
        } else {
            widgets::Button::Ghost
        };
        let body = row![
            tab_lead(tab, ty),
            ty.data(tab.label(), theme::TEXT)
                .width(Length::Fill)
                .wrapping(iced_core::text::Wrapping::None)
                .ellipsis(iced_core::text::Ellipsis::End),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center);
        let select = kit::lift(
            kind,
            true,
            button(body)
                .width(Length::Fill)
                .height(ty.control_height())
                .padding([0, 10])
                .style(widgets::button_style_fading(kind))
                .on_press(emit(Action::Select(id))),
        );
        let named: UiElement<'a> = if tab.location.is_empty() {
            select
        } else {
            kit::tip(
                select,
                column![
                    ty.data(tab.label(), theme::TEXT),
                    ty.data(tab.address.as_str(), theme::TEXT_MUTED),
                ]
                .spacing(theme::SPACE_1),
                iced_widget::tooltip::Position::Bottom,
            )
        };
        strip = strip.push(
            container(
                row![
                    named,
                    kit::icon_button(
                        ty,
                        Icon::Close,
                        "Close tab",
                        widgets::Button::Ghost,
                        Some(emit(Action::Close(id))),
                    ),
                ]
                .spacing(2)
                .align_y(alignment::Vertical::Center),
            )
            .width(Length::Fill.max(240)),
        );
    }
    strip
        .push(kit::icon_button(
            ty,
            Icon::Plus,
            "New tab",
            widgets::Button::Secondary,
            (c.ready && c.tabs.len() < wire::MAX_TABS).then(|| emit(Action::New)),
        ))
        .push(space::horizontal())
        .into()
}

/// The browser: tabs, a toolbar, the progress line, the page, and a footer.
fn chrome<'a>(
    c: Chrome<'a>,
    ty: Typography,
    now: Instant,
    emit: impl Fn(Action) -> Message + Copy + 'a,
) -> UiElement<'a> {
    let id = c.active;
    let tab = &c.tabs[&id];
    let strip = tab_strip(&c, ty, emit);

    // The address bar carries what the page is: a turning mark while it loads,
    // a lock for a secure page.
    let marker: UiElement<'a> = if tab.loading() {
        motion::beacon(Beat::Spin, theme::TEXT_SECONDARY, ty.size(14.))
    } else if tab.location.starts_with("https://") {
        visuals::icon(Icon::Lock, theme::TEXT_SECONDARY, ty.size(14.))
    } else {
        visuals::icon(Icon::Globe, theme::TEXT_MUTED, ty.size(14.))
    };
    let address = text_input("Enter a website address", &tab.address)
        .id(format!("servo-{}-{id}", c.tile))
        .font(fonts::MONO)
        .size(ty.size(theme::SIZE_SM))
        .padding(Padding {
            top: 7.,
            right: 10.,
            bottom: 7.,
            left: 12. + ty.size(14.) + 10.,
        })
        .style(widgets::field)
        .on_input(move |value| emit(Action::Edit(id, value)))
        .on_submit(emit(Action::Go(id)));
    let omnibox = stack![
        address,
        container(marker)
            .height(Length::Fill)
            .padding(Padding::default().left(12.))
            .align_y(alignment::Vertical::Center),
    ];
    let toolbar = row![
        kit::icon_button(
            ty,
            Icon::Back,
            "Back",
            widgets::Button::Ghost,
            (c.ready && tab.back).then(|| emit(Action::Send(Command::Back(id)))),
        ),
        kit::icon_button(
            ty,
            Icon::Forward,
            "Forward",
            widgets::Button::Ghost,
            (c.ready && tab.forward).then(|| emit(Action::Send(Command::Forward(id)))),
        ),
        kit::icon_button(
            ty,
            Icon::Refresh,
            "Reload",
            widgets::Button::Ghost,
            (c.ready && !tab.location.is_empty()).then(|| emit(Action::Send(Command::Reload(id)))),
        ),
        omnibox,
    ]
    .align_y(alignment::Vertical::Center)
    .spacing(theme::SPACE_1);
    let (level, opacity, moving) = tab.bar(now);

    let mut content = column![
        strip,
        column![toolbar, motion::loadbar(level, opacity, moving, 2.)].spacing(theme::SPACE_1),
    ]
    .spacing(theme::SPACE_2)
    .height(Length::Fill);
    if !c.status.is_empty() {
        content = content.push(kit::caution_data(ty, c.status));
    }
    let surface = canvas(Target {
        id,
        size: tab.size,
        bounds: c.bounds,
        focused: c.focused,
        cursor: tab.cursor,
        paint: c.ready
            && !c.importing
            && tab.dirty
            && !c.tabs.values().any(|tab| tab.waiting)
            && tab.size != (0, 0),
        emit,
    })
    .width(Length::Fill)
    .height(Length::Fill);
    // Under the page: the link the pointer is over, as a browser shows it, or
    // nothing but the renderer's name when it is over none.
    let footer: UiElement<'a> = if !tab.status.is_empty() {
        row![
            visuals::icon(Icon::Globe, theme::TEXT_MUTED, ty.size(13.)),
            ty.data(tab.status.as_str(), theme::TEXT_MUTED)
                .width(Length::Fill)
                .wrapping(iced_core::text::Wrapping::None)
                .ellipsis(iced_core::text::Ellipsis::Middle),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center)
        .into()
    } else if c.ready && !c.renderer.is_empty() {
        row![
            space::horizontal(),
            visuals::icon(Icon::Cpu, theme::TEXT_MUTED, ty.size(13.)),
            ty.data(c.renderer, theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_1 + 2.)
        .align_y(alignment::Vertical::Center)
        .into()
    } else {
        space::vertical().height(0).into()
    };
    content.push(surface).push(footer).into()
}

struct Target<'a, F> {
    bounds: &'a Cell<Option<Rectangle>>,
    id: u64,
    size: (u32, u32),
    focused: bool,
    cursor: wire::Cursor,
    paint: bool,
    emit: F,
}

/// The pointer shape a page asks for, as the interface can draw it.
fn interaction(cursor: wire::Cursor) -> mouse::Interaction {
    use mouse::Interaction as I;
    use wire::Cursor as C;
    match cursor {
        C::Default => I::Idle,
        C::None => I::Hidden,
        C::Pointer => I::Pointer,
        C::Text => I::Text,
        C::Crosshair => I::Crosshair,
        C::Wait => I::Wait,
        C::Progress => I::Progress,
        C::Help => I::Help,
        C::ContextMenu => I::ContextMenu,
        C::Cell => I::Cell,
        C::Alias => I::Alias,
        C::Copy => I::Copy,
        C::Move => I::Move,
        C::NoDrop => I::NoDrop,
        C::NotAllowed => I::NotAllowed,
        C::Grab => I::Grab,
        C::Grabbing => I::Grabbing,
        C::AllScroll => I::AllScroll,
        C::ResizeHorizontal => I::ResizingHorizontally,
        C::ResizeVertical => I::ResizingVertically,
        C::ResizeDiagonalUp => I::ResizingDiagonallyUp,
        C::ResizeDiagonalDown => I::ResizingDiagonallyDown,
        C::ResizeColumn => I::ResizingColumn,
        C::ResizeRow => I::ResizingRow,
        C::ZoomIn => I::ZoomIn,
        C::ZoomOut => I::ZoomOut,
    }
}
impl<F: Fn(Action) -> Message> canvas::Program<Message, Theme, AxUi> for Target<'_, F> {
    type State = ();
    fn update(
        &self,
        _: &mut (),
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let id = self.id;
        let point = cursor.position_in(bounds);
        let command = match event {
            canvas::Event::Window(iced_core::window::Event::RedrawRequested(_)) => {
                if self.paint {
                    Command::Paint(id)
                } else {
                    return None;
                }
            }
            canvas::Event::Mouse(event) => {
                let pt = point?;
                let x = (pt.x * self.size.0 as f32 / bounds.width) as u32;
                let y = (pt.y * self.size.1 as f32 / bounds.height) as u32;
                let pointer = match event {
                    mouse::Event::ButtonPressed(mouse::Button::Left) => Pointer::Down,
                    mouse::Event::ButtonReleased(mouse::Button::Left) => Pointer::Up,
                    mouse::Event::CursorMoved { .. } => Pointer::Move,
                    mouse::Event::WheelScrolled { delta } => Pointer::Wheel(match delta {
                        mouse::ScrollDelta::Lines { y, .. } => (40. * y) as i32,
                        mouse::ScrollDelta::Pixels { y, .. } => *y as i32,
                    }),
                    _ => return None,
                };
                Command::Pointer(id, x, y, pointer)
            }
            canvas::Event::Keyboard(event) if self.focused => {
                let (key, mods, down) = match event {
                    keyboard::Event::KeyPressed {
                        modified_key,
                        modifiers,
                        ..
                    } => (modified_key, modifiers, true),
                    keyboard::Event::KeyReleased {
                        modified_key,
                        modifiers,
                        ..
                    } => (modified_key, modifiers, false),
                    _ => return None,
                };
                let value = match key {
                    keyboard::Key::Character(c) => c.to_string(),
                    keyboard::Key::Named(keyboard::key::Named::Space) => " ".into(),
                    keyboard::Key::Named(n) => format!("{n:?}"),
                    _ => return None,
                };
                let flags = u8::from(mods.shift())
                    | (u8::from(mods.control()) << 1)
                    | (u8::from(mods.alt()) << 2)
                    | (u8::from(mods.logo()) << 3);
                Command::Key(id, value, down, flags)
            }
            _ => return None,
        };
        Some(canvas::Action::publish((self.emit)(Action::Send(command))).and_capture())
    }
    fn draw(
        &self,
        _: &(),
        _: &AxUi,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry<AxUi>> {
        self.bounds.set(Some(bounds));
        Vec::new()
    }
    fn mouse_interaction(
        &self,
        _: &(),
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            interaction(self.cursor)
        } else {
            mouse::Interaction::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_core::{Size, Theme};

    fn icon(side: u32, rgba: [u8; 4]) -> wire::Icon {
        let pixels: Vec<u8> = rgba
            .iter()
            .copied()
            .cycle()
            .take((side * side * 4) as usize)
            .collect();
        wire::Icon::pack(side, side, &pixels).unwrap()
    }

    fn at(tab: &mut Tab, event: Event) -> bool {
        tab.notice(event, Instant::now())
    }

    #[test]
    fn a_tab_is_called_by_its_title_then_its_host_then_new_tab() {
        let mut tab = Tab::default();
        assert_eq!(tab.label(), "New tab");
        at(
            &mut tab,
            Event::Location(0, "https://example.org/a?b#c".into(), false, false),
        );
        assert_eq!(tab.label(), "example.org");
        at(&mut tab, Event::Title(0, "  Example Domain ".into()));
        assert_eq!(tab.label(), "Example Domain");
        at(&mut tab, Event::Title(0, "   ".into()));
        assert_eq!(tab.label(), "example.org", "a blank title is no title");
    }

    #[test]
    fn another_site_clears_the_old_title_and_icon_but_the_same_site_keeps_them() {
        let mut tab = Tab::default();
        at(
            &mut tab,
            Event::Location(0, "https://example.org/".into(), false, false),
        );
        at(&mut tab, Event::Title(0, "Home".into()));
        at(&mut tab, Event::Favicon(0, Some(icon(16, [1, 2, 3, 255]))));
        assert!(tab.favicon.is_some());
        at(
            &mut tab,
            Event::Location(0, "https://example.org/docs".into(), true, false),
        );
        assert_eq!(tab.title, "Home");
        assert!(tab.favicon.is_some());
        assert!(tab.back && !tab.forward);
        at(
            &mut tab,
            Event::Location(0, "https://other.example/".into(), true, false),
        );
        assert!(tab.title.is_empty());
        assert!(tab.favicon.is_none());
        assert_eq!(tab.host, "other.example");
    }

    #[test]
    fn a_malformed_icon_is_dropped_and_never_drawn() {
        let mut tab = Tab::default();
        at(&mut tab, Event::Favicon(0, Some(icon(32, [9, 9, 9, 255]))));
        assert!(tab.favicon.is_some());
        let bad = wire::Icon {
            width: 4,
            height: 4,
            hex: "zz".into(),
        };
        at(&mut tab, Event::Favicon(0, Some(bad)));
        assert!(tab.favicon.is_none());
        let huge = wire::Icon {
            width: wire::MAX_ICON_SIDE + 1,
            height: 1,
            hex: "00".repeat((wire::MAX_ICON_SIDE as usize + 1) * 4),
        };
        at(&mut tab, Event::Favicon(0, Some(huge)));
        assert!(tab.favicon.is_none());
        at(&mut tab, Event::Favicon(0, Some(icon(8, [0; 4]))));
        at(&mut tab, Event::Favicon(0, None));
        assert!(tab.favicon.is_none());
    }

    #[test]
    fn a_load_moves_the_progress_line_then_holds_it_full_and_hides_it() {
        let mut tab = Tab::default();
        let t0 = Instant::now();
        assert_eq!(tab.bar(t0), (0.0, 0.0, false), "an idle tab shows no line");
        tab.notice(Event::Loading(0, wire::Load::Started), t0);
        let (started, opacity, moving) = tab.bar(t0 + PROGRESS * 3);
        assert!((started - 0.18).abs() < 0.01 && opacity == 1.0 && moving);
        tab.notice(Event::Loading(0, wire::Load::Parsed), t0 + PROGRESS * 3);
        let (parsed, ..) = tab.bar(t0 + PROGRESS * 6);
        assert!(parsed > 0.7 && parsed < 0.75);
        let done = t0 + PROGRESS * 6;
        tab.notice(Event::Loading(0, wire::Load::Done), done);
        assert!(!tab.loading());
        let (full, held, _) = tab.bar(done + PROGRESS);
        assert!((full - 1.0).abs() < 1e-3 && held > 0.0);
        let (_, gone, moving) = tab.bar(done + Duration::from_secs(2));
        assert_eq!(gone, 0.0);
        assert!(!moving, "a hidden line asks for no frames");
        // A new load starts again from nothing.
        tab.notice(
            Event::Loading(0, wire::Load::Started),
            done + Duration::from_secs(3),
        );
        let (restart, opacity, _) = tab.bar(done + Duration::from_secs(3));
        assert!(restart < 0.01 && opacity == 1.0);
    }

    #[test]
    fn only_what_the_interface_shows_asks_for_a_redraw() {
        let mut tab = Tab::default();
        assert!(!at(&mut tab, Event::Changed(0)), "a frame is not interface");
        assert!(tab.dirty);
        assert!(at(&mut tab, Event::Title(0, "x".into())));
        assert!(at(&mut tab, Event::Status(0, "https://a.example/".into())));
        assert!(
            !at(&mut tab, Event::Status(0, "https://a.example/".into())),
            "the same link again changes nothing"
        );
        assert!(!at(&mut tab, Event::Presented(0)));
    }

    #[test]
    fn every_pointer_shape_a_page_can_ask_for_has_an_interaction() {
        use wire::Cursor as C;
        let all = [
            C::Default,
            C::None,
            C::Pointer,
            C::Text,
            C::Crosshair,
            C::Wait,
            C::Progress,
            C::Help,
            C::ContextMenu,
            C::Cell,
            C::Alias,
            C::Copy,
            C::Move,
            C::NoDrop,
            C::NotAllowed,
            C::Grab,
            C::Grabbing,
            C::AllScroll,
            C::ResizeHorizontal,
            C::ResizeVertical,
            C::ResizeDiagonalUp,
            C::ResizeDiagonalDown,
            C::ResizeColumn,
            C::ResizeRow,
            C::ZoomIn,
            C::ZoomOut,
        ];
        let shapes: std::collections::HashSet<_> = all
            .iter()
            .map(|c| format!("{:?}", interaction(*c)))
            .collect();
        assert_eq!(shapes.len(), all.len(), "two shapes drew the same pointer");
        assert_eq!(interaction(C::Pointer), mouse::Interaction::Pointer);
        assert_eq!(interaction(C::Text), mouse::Interaction::Text);
        assert_eq!(interaction(C::None), mouse::Interaction::Hidden);
    }

    #[test]
    fn the_chrome_draws_in_every_state_in_both_appearances() {
        let bounds = Cell::new(None);
        let emit = |action| Message::Plugin(plugins::Action::Servo("demo".into(), 1, action));
        let now = Instant::now();
        let mut tabs = BTreeMap::from([(0, Tab::default())]);
        let states: Vec<(&str, BTreeMap<u64, Tab>, &str)> = {
            // A page that has loaded, with an icon and somewhere to go back to.
            let mut loaded = Tab::default();
            loaded.notice(
                Event::Location(0, "https://example.org/".into(), true, false),
                now,
            );
            loaded.notice(Event::Title(0, "Example Domain".into()), now);
            loaded.notice(Event::Favicon(0, Some(icon(32, [206, 21, 196, 255]))), now);
            loaded.notice(Event::Loading(0, wire::Load::Done), now);
            // Several tabs: one loading, one with an icon, one whose title
            // does not fit, with a link under the pointer.
            let mut loading = Tab::default();
            loading.notice(
                Event::Location(0, "https://slow.example/a".into(), false, false),
                now,
            );
            loading.notice(Event::Loading(0, wire::Load::Started), now);
            loading.notice(Event::Loading(0, wire::Load::Parsed), now);
            loading.notice(
                Event::Status(
                    0,
                    "https://slow.example/some/rather/deep/link?query=1".into(),
                ),
                now,
            );
            let mut long = Tab::default();
            long.notice(
                Event::Location(0, "https://long.example/".into(), true, true),
                now,
            );
            long.notice(
                Event::Title(
                    0,
                    "A very long page title that cannot possibly fit in one tab of the strip"
                        .into(),
                ),
                now,
            );
            long.notice(Event::Favicon(0, Some(icon(16, [63, 203, 110, 255]))), now);
            long.notice(Event::Loading(0, wire::Load::Done), now);
            vec![
                ("new", std::mem::take(&mut tabs), ""),
                ("loaded", BTreeMap::from([(0, loaded)]), ""),
                (
                    "many",
                    BTreeMap::from([(0, loading), (1, long), (2, Tab::default())]),
                    "",
                ),
                (
                    "error",
                    BTreeMap::from([(0, Tab::default())]),
                    "Enter an HTTPS website address.",
                ),
            ]
        };
        let mut tabs_of = states;
        tabs_of[0].1.insert(0, Tab::default());
        for (name, tabs, status) in &tabs_of {
            for (appearance, mode) in [(Theme::Dark, "dark"), (Theme::Light, "light")] {
                // The loading tab is the one in front, a moment into its load,
                // so the progress line is on screen.
                let active = 0;
                let element = chrome(
                    Chrome {
                        tabs,
                        active,
                        tile: 1,
                        ready: true,
                        status,
                        renderer: "Apple M3",
                        bounds: &bounds,
                        focused: false,
                        importing: false,
                    },
                    Typography::default(),
                    now + PROGRESS * 3,
                    emit,
                );
                let (_, refused) = crate::tests::draw_at_scale(
                    iced_widget::container(widgets::page_frame(element))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(|appearance| theme::panel(appearance, theme::BACKGROUND, None, 0.))
                        .into(),
                    Size::new(980., 560.),
                    &appearance,
                    1.,
                    Some(&format!("servo-{name}-{mode}")),
                );
                assert_eq!(refused, [], "{name} {mode}");
            }
        }
    }
}
