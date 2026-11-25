//! Host chrome for the optional native Servo helper. All browser IO is off the UI thread.
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command as Process, Stdio},
    sync::mpsc,
    thread,
};

use iced_core::{ContentFit, Length, Rectangle, Theme, alignment, image::Handle, keyboard, mouse};
use iced_widget::{canvas, column, image, row, stack, text_input};
use iced_winit::winit::event_loop::EventLoopProxy;
use vkore_servo_wire::{self as wire, Command, Event, Pointer};

use crate::{
    AxUi, Message, UiElement, UserEvent, fonts, plugin_view, plugins, theme,
    typography::Typography, widgets,
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

#[derive(Default)]
struct Tab {
    address: String,
    location: String,
    back: bool,
    loading: bool,
    frame: Option<Handle>,
    dirty: bool,
    waiting: bool,
    size: (u32, u32),
}

enum Reply {
    Notice(Event),
    Frame(u64, Handle),
    End(String),
}

struct Helper(Child);
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct Pane {
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
        let mut helper = Helper(Process::new(helper_path()?).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("Could not start the native Servo helper ({e}). Install vkore-servo beside vkore-ui or set VKORE_SERVO.")))?);
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
                let mut read = || -> io::Result<Reply> {
                    let event = wire::read(&mut stdout)?;
                    if let Event::Frame(id, w, h) = event {
                        let mut pixels = vec![0; wire::pixel_bytes(w, h)?];
                        stdout.read_exact(&mut pixels)?;
                        Ok(Reply::Frame(id, Handle::from_rgba(w, h, pixels)))
                    } else {
                        Ok(Reply::Notice(event))
                    }
                };
                loop {
                    let reply = read().unwrap_or_else(|e| Reply::End(e.to_string()));
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

    pub fn receive(&mut self) {
        let Ok(reply) = self.output.try_recv() else {
            return;
        };
        match reply {
            Reply::Notice(Event::Ready(wire::VERSION, renderer)) => {
                self.renderer = renderer;
                self.ready = true;
                self.status.clear();
                self.send(Command::Open(0));
            }
            Reply::Notice(Event::Ready(_, _)) => {
                self.status = "The Servo helper uses an incompatible protocol.".into();
            }
            Reply::Notice(Event::Changed(id)) => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.dirty = true;
                }
            }
            Reply::Notice(Event::Loading(id, loading)) => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.loading = loading;
                }
            }
            Reply::Notice(Event::Location(id, url, back)) => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    if tab.location != url {
                        tab.address =
                            fonts::shown(fonts::MONO, &url, fonts::Breaks::Escape).into_owned();
                        tab.location = url;
                    }
                    tab.back = back;
                }
            }
            Reply::Frame(id, image) => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.frame = Some(image);
                    tab.waiting = false;
                }
            }
            Reply::Notice(Event::Error(e)) => self.status = e,
            Reply::End(e) => {
                if self.status.is_empty() || self.status == "Starting Servo…" {
                    self.status = format!("Servo stopped ({e}).");
                }
                self.ready = false;
            }
            Reply::Notice(Event::Frame(..)) => unreachable!(),
        }
    }

    pub fn update(&mut self, action: Action) -> bool {
        match action {
            Action::New if self.ready && self.tabs.len() < wire::MAX_TABS => {
                let id = self.next;
                if self.send(Command::Open(id)) {
                    self.tabs.insert(id, Tab::default());
                    self.active = id;
                    self.next += 1;
                }
            }
            Action::Select(id)
                if self.ready && self.tabs.contains_key(&id) && self.send(Command::Select(id)) =>
            {
                self.active = id
            }
            Action::Close(id) if self.tabs.contains_key(&id) => {
                if self.tabs.len() == 1 {
                    return true;
                }
                if self.send(Command::Close(id)) {
                    self.tabs.remove(&id);
                    if id == self.active {
                        self.active = *self.tabs.first_key_value().expect("remaining tab").0;
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
            Action::Send(command) if self.ready && self.send(command.clone()) => match command {
                Command::Pointer(_, _, _, Pointer::Down) => self.focused = true,
                Command::Resize(id, w, h) => {
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

    pub fn view<'a>(&'a self, machine: &'a str, tile: u64, ty: Typography) -> UiElement<'a> {
        let emit =
            move |action| Message::Plugin(plugins::Action::Servo(machine.into(), tile, action));
        let id = self.active;
        let tab = &self.tabs[&id];
        let strip = plugin_view::tabs(
            ty,
            self.tabs.iter().map(|(&id, tab)| {
                (
                    id,
                    tab.location
                        .strip_prefix("https://")
                        .and_then(|s| s.split('/').next())
                        .unwrap_or("New tab"),
                )
            }),
            id,
            (self.ready && self.tabs.len() < wire::MAX_TABS).then(|| emit(Action::New)),
            |id| emit(Action::Select(id)),
            |id| Some(emit(Action::Close(id))),
        );
        let address = text_input("Website address", &tab.address)
            .id(format!("servo-{tile}-{id}"))
            .font(fonts::MONO)
            .size(ty.size(theme::SIZE_SM))
            .padding([7, 10])
            .style(widgets::field)
            .on_input(move |value| emit(Action::Edit(id, value)))
            .on_submit(emit(Action::Go(id)));
        let mut content = column![
            strip,
            row![
                plugin_view::control(
                    ty,
                    "Back",
                    (self.ready && tab.back).then(|| emit(Action::Send(Command::Back(id))))
                ),
                address,
                plugin_view::control(ty, "Go", self.ready.then(|| emit(Action::Go(id))))
            ]
            .align_y(alignment::Vertical::Center)
            .spacing(theme::SPACE_2)
        ]
        .spacing(theme::SPACE_2)
        .height(Length::Fill);
        if !self.status.is_empty() {
            content = content.push(ty.data(&self.status, theme::TEXT_MUTED));
        }
        let mut surface = stack![].width(Length::Fill).height(Length::Fill);
        if let Some(frame) = &tab.frame {
            surface = surface.push(
                image(frame.clone())
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .content_fit(ContentFit::Fill),
            );
        }
        if self.ready {
            surface = surface.push(
                canvas(Target {
                    id,
                    size: tab.size,
                    focused: self.focused,
                    paint: tab.dirty && !tab.waiting,
                    emit,
                })
                .width(Length::Fill)
                .height(Length::Fill),
            );
        }
        let footer = if tab.loading {
            format!("Loading… · {}", self.renderer)
        } else {
            self.renderer.clone()
        };
        content
            .push(surface)
            .push(ty.data(footer, theme::TEXT_MUTED))
            .into()
    }
}

struct Target<F> {
    id: u64,
    size: (u32, u32),
    focused: bool,
    paint: bool,
    emit: F,
}
impl<F: Fn(Action) -> Message> canvas::Program<Message, Theme, AxUi> for Target<F> {
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
                let scale = (wire::MAX_WIDTH as f32 / bounds.width.max(1.))
                    .min(wire::MAX_HEIGHT as f32 / bounds.height.max(1.))
                    .min(1.);
                let size = (
                    ((bounds.width * scale).round() as u32).max(1),
                    ((bounds.height * scale).round() as u32).max(1),
                );
                if size != self.size {
                    Command::Resize(id, size.0, size.1)
                } else if self.paint {
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
        _: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry<AxUi>> {
        Vec::new()
    }
}
