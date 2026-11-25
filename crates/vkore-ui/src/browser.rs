//! Browser controls and tab state. The worker runs one independent store per tab.

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use iced_core::{Length, alignment};
use iced_widget::{column, row, text_input};
use iced_winit::winit::event_loop::EventLoopProxy;
use vkored::{
    browser::{BrowserFrame, Compiled, Operation, Session},
    plugins::Bundle,
};

use crate::{
    Message, UiElement, UserEvent, fonts, plugin_view, plugins, theme, typography::Typography,
    widgets,
};

const MAX_TABS: usize = 4;
type TabId = u64;

#[derive(Clone, Debug)]
pub enum Action {
    New,
    Select(TabId),
    Close(TabId),
    Edit(TabId, String),
    Navigate(TabId),
    Click(TabId, u32, u32),
    Back(TabId),
}

enum Request {
    Open(TabId),
    Close(TabId),
    Navigate(TabId, String),
    Click(TabId, u32, u32),
    Back(TabId),
}

enum Reply {
    Frame(TabId, BrowserFrame, bool),
    Error(TabId, String, bool),
    Closed,
}

struct Page {
    image: plugin_view::Frame,
    location: String,
    can_go_back: bool,
    can_navigate: bool,
}

#[derive(Default)]
struct Tab {
    page: Option<Page>,
    address: String,
    status: String,
}

pub struct Pane {
    input: mpsc::SyncSender<Request>,
    output: mpsc::Receiver<Reply>,
    tabs: BTreeMap<TabId, Tab>,
    active: TabId,
    next: TabId,
    pending: bool,
}

static WORKER_RUNNING: AtomicBool = AtomicBool::new(false);

struct Worker;

impl Drop for Worker {
    fn drop(&mut self) {
        WORKER_RUNNING.store(false, Ordering::Release);
    }
}

impl Pane {
    pub fn open(bundle: Bundle, proxy: EventLoopProxy<UserEvent>) -> std::io::Result<Self> {
        if WORKER_RUNNING.swap(true, Ordering::Acquire) {
            return Err(std::io::Error::other(
                "The previous browser is still closing.",
            ));
        }
        let worker = Worker;
        let (input, events) = mpsc::sync_channel(1);
        let (frames, output) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("vkore-browser".into())
            .spawn(move || {
                let _worker = worker;
                let publish = |frame| {
                    frames.send(frame).is_ok() && proxy.send_event(UserEvent::BrowserFrame).is_ok()
                };
                let mut sessions = BTreeMap::new();
                // Compilation and linking are shared. Each store owns its own page state.
                let compiled = Compiled::new(&bundle);
                drop(bundle);
                for request in events {
                    let result = match request {
                        Request::Open(id) => {
                            let result = compiled
                                .as_ref()
                                .map_err(|error| format!("{error:#}"))
                                .and_then(|compiled| {
                                    compiled.open().map_err(|error| format!("{error:#}"))
                                });
                            match result {
                                Ok((session, first)) => {
                                    let can_navigate = session.can_navigate();
                                    sessions.insert(id, session);
                                    Reply::Frame(id, first, can_navigate)
                                }
                                Err(error) => Reply::Error(id, error, true),
                            }
                        }
                        Request::Close(id) => {
                            sessions.remove(&id);
                            Reply::Closed
                        }
                        Request::Navigate(id, path) => {
                            run(&mut sessions, id, |session| session.navigate(path))
                        }
                        Request::Click(id, x, y) => {
                            run(&mut sessions, id, |session| session.click(x, y))
                        }
                        Request::Back(id) => run(&mut sessions, id, Session::back),
                    };
                    if !publish(result) {
                        break;
                    }
                }
            })?;
        let mut pane = Self {
            input,
            output,
            tabs: BTreeMap::new(),
            active: 0,
            next: 0,
            pending: false,
        };
        pane.new_tab()?;
        Ok(pane)
    }

    fn new_tab(&mut self) -> std::io::Result<()> {
        let id = self.next;
        if self.send(Request::Open(id)) {
            self.tabs.insert(
                id,
                Tab {
                    status: "Starting browser…".into(),
                    ..Default::default()
                },
            );
            self.active = id;
            self.next += 1;
            return Ok(());
        }
        Err(std::io::Error::other(
            "The browser worker did not accept the tab.",
        ))
    }

    fn send(&mut self, request: Request) -> bool {
        if self.pending || self.input.try_send(request).is_err() {
            return false;
        }
        self.pending = true;
        true
    }

    pub fn receive(&mut self) {
        let Ok(reply) = self.output.try_recv() else {
            return;
        };
        self.pending = false;
        match reply {
            Reply::Frame(id, frame, can_navigate) => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.address = fonts::shown(fonts::MONO, &frame.location, fonts::Breaks::Escape)
                        .into_owned();
                    tab.status = if can_navigate {
                        String::new()
                    } else {
                        "Rebuild this browser bundle to enable typed navigation.".into()
                    };
                    tab.page = Some(Page {
                        image: plugin_view::Frame::rgba(frame.width, frame.height, frame.rgba),
                        location: frame.location,
                        can_go_back: frame.can_go_back,
                        can_navigate,
                    });
                }
            }
            Reply::Error(id, error, fatal) => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.status = error.chars().take(512).collect();
                    if fatal {
                        tab.page = None;
                    }
                }
            }
            Reply::Closed => {}
        }
    }

    /// Returns true when the last tab closes the tile.
    pub fn update(&mut self, action: Action) -> bool {
        match action {
            Action::New if !self.pending && self.tabs.len() < MAX_TABS => {
                if let Err(error) = self.new_tab()
                    && let Some(tab) = self.tabs.get_mut(&self.active)
                {
                    tab.status = error.to_string();
                }
            }
            Action::Select(id) if self.tabs.contains_key(&id) => self.active = id,
            Action::Close(id) if self.tabs.contains_key(&id) => {
                if self.tabs.len() == 1 {
                    return true;
                }
                if self.send(Request::Close(id)) {
                    self.tabs.remove(&id);
                    if self.active == id {
                        self.active = *self
                            .tabs
                            .range(..id)
                            .next_back()
                            .or_else(|| self.tabs.first_key_value())
                            .expect("closing a tab leaves another tab")
                            .0;
                    }
                }
            }
            Action::Edit(id, value) if self.active == id && value.len() <= 256 => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    tab.address =
                        fonts::shown(fonts::MONO, &value, fonts::Breaks::Escape).into_owned();
                }
            }
            Action::Navigate(id) if self.active == id => {
                if let Some(tab) = self.tabs.get(&id)
                    && tab.page.as_ref().is_some_and(|page| page.can_navigate)
                {
                    self.send(Request::Navigate(id, tab.address.clone()));
                }
            }
            Action::Click(id, x, y) if self.active == id => {
                self.send(Request::Click(id, x, y));
            }
            Action::Back(id) if self.active == id => {
                self.send(Request::Back(id));
            }
            _ => {}
        }
        false
    }

    pub fn view<'a>(
        &'a self,
        machine: &'a str,
        tile: plugins::TileId,
        ty: Typography,
    ) -> UiElement<'a> {
        let emit =
            move |action| Message::Plugin(plugins::Action::Browser(machine.into(), tile, action));
        let id = self.active;
        let strip = plugin_view::tabs(
            ty,
            self.tabs.iter().map(|(&id, tab)| {
                let label = tab.page.as_ref().map_or("New tab", |page| &page.location);
                (id, label)
            }),
            id,
            (!self.pending && self.tabs.len() < MAX_TABS).then(|| emit(Action::New)),
            |id| emit(Action::Select(id)),
            |id| (!self.pending || self.tabs.len() == 1).then(|| emit(Action::Close(id))),
        );
        let tab = &self.tabs[&id];
        let mut content = column![strip].spacing(theme::SPACE_2).height(Length::Fill);
        if let Some(page) = &tab.page {
            let address = text_input("Bundled page path", &tab.address)
                .id(format!("browser-{tile}-{id}"))
                .font(fonts::MONO)
                .size(ty.size(theme::SIZE_SM))
                .padding([7, 10])
                .width(Length::Fill)
                .style(widgets::field);
            let address = if page.can_navigate {
                address
                    .on_input(move |value| emit(Action::Edit(id, value)))
                    .on_submit(emit(Action::Navigate(id)))
            } else {
                address
            };
            content = content.push(
                row![
                    plugin_view::control(
                        ty,
                        "Back",
                        (page.can_go_back && !self.pending).then(|| emit(Action::Back(id)))
                    ),
                    address,
                    plugin_view::control(
                        ty,
                        "Go",
                        (page.can_navigate && !self.pending).then(|| emit(Action::Navigate(id)))
                    ),
                ]
                .align_y(alignment::Vertical::Center)
                .spacing(theme::SPACE_2),
            );
        }
        if !tab.status.is_empty() {
            content = content.push(
                ty.data(&tab.status, theme::TEXT_MUTED)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            );
        }
        if let Some(page) = &tab.page {
            content =
                content
                    .push(page.image.view(
                        (!self.pending).then_some(move |x, y| emit(Action::Click(id, x, y))),
                    ));
        }
        content.into()
    }
}

fn run(
    sessions: &mut BTreeMap<TabId, Session>,
    id: TabId,
    call: impl FnOnce(&mut Session) -> Operation,
) -> Reply {
    let Some(session) = sessions.get_mut(&id) else {
        return Reply::Error(id, "This tab has no active page.".into(), true);
    };
    match call(session) {
        Ok(Ok(frame)) => Reply::Frame(id, frame, session.can_navigate()),
        Ok(Err(error)) => Reply::Error(id, error, false),
        Err(error) => {
            sessions.remove(&id);
            Reply::Error(id, format!("{error:#}"), true)
        }
    }
}
