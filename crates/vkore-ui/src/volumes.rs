//! The volume browser: a Kore machine's DagFS volume, navigable from the
//! host through Kore's own engine.
//!
//! One worker thread owns the open volume (a [`Browser`] holds the disk's
//! lock for as long as it is open) and answers the view's requests in order;
//! the view never blocks on the disk. Installing or removing closes the
//! browser, writes through Kore's all-or-nothing publication, and reopens.

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, scrollable, space, text_input};
use iced_winit::winit::event_loop::EventLoopProxy;

use vkore_volume::{
    Browser, Disk, DiskKind, Entry, EntryKind, ExportOptions, Listing, Place, Preview, VolPath,
    preview::{self, Capture},
};

use crate::{Message, UiElement, UserEvent, fonts, theme, widgets};

/// The most a preview reads of a file. A PNG's size and Kore's capture
/// record sit in its first few hundred bytes; this is also the most text the
/// pane shows.
const PREVIEW_HEAD_BYTES: usize = 32 << 10;

/// What the view asks of the worker.
pub enum Request {
    Open {
        disk: Disk,
        label: String,
    },
    List(VolPath),
    /// A file's summary, from its head; `len` is its length as listed.
    Preview {
        path: VolPath,
        len: u64,
    },
    Export {
        path: VolPath,
        destination: PathBuf,
    },
    View(VolPath),
    Install {
        source: PathBuf,
        destination: VolPath,
    },
    Remove(VolPath),
    Close,
}

/// What the worker answers.
#[derive(Debug)]
pub enum Reply {
    Opened {
        label: String,
        kind: DiskKind,
        generation: u64,
        fallback: bool,
    },
    Listed(VolPath, Result<Listing, String>),
    Previewed(VolPath, Result<Summary, String>),
    Done(String),
    Failed(String),
    Closed,
}

/// A preview as the pane shows it.
#[derive(Debug, Clone)]
pub enum Summary {
    Image {
        width: u32,
        height: u32,
        capture: Option<Box<Capture>>,
        text: Vec<(String, String)>,
    },
    Text {
        text: String,
        lines: usize,
        truncated: bool,
    },
    Binary {
        len: u64,
        head: String,
    },
}

/// Start the worker; requests go in through the returned sender.
pub fn spawn(proxy: EventLoopProxy<UserEvent>) -> mpsc::Sender<Request> {
    let (sender, receiver) = mpsc::channel::<Request>();
    thread::Builder::new()
        .name("vkore-ui-volume".into())
        .spawn(move || worker(receiver, proxy))
        .expect("start the volume worker");
    sender
}

fn worker(requests: mpsc::Receiver<Request>, proxy: EventLoopProxy<UserEvent>) {
    let reply = |reply: Reply| {
        let _ = proxy.send_event(UserEvent::Volume(reply));
    };
    let mut open: Option<(Disk, Browser)> = None;
    while let Ok(request) = requests.recv() {
        match request {
            Request::Open { disk, label } => {
                // Release any disk held before taking another.
                drop(open.take());
                match Browser::open(&disk) {
                    Ok(browser) => {
                        let status = browser.status();
                        reply(Reply::Opened {
                            label,
                            kind: disk.kind(),
                            generation: status.generation,
                            fallback: status.used_fallback_root,
                        });
                        open = Some((disk, browser));
                    }
                    Err(error) if error.is_blank() => reply(Reply::Failed(format!(
                        "{label}: a blank disk — its machine formats it at first boot"
                    ))),
                    Err(error) if error.is_in_use() => reply(Reply::Failed(format!(
                        "{label}: in use — a machine is running on it; stop it first"
                    ))),
                    Err(error) => reply(Reply::Failed(format!("{label}: {error}"))),
                }
            }
            Request::List(path) => {
                let Some((_, browser)) = open.as_ref() else {
                    continue;
                };
                let listing = browser.list(&path).map_err(|error| error.to_string());
                reply(Reply::Listed(path, listing));
            }
            Request::Preview { path, len } => {
                let Some((_, browser)) = open.as_ref() else {
                    continue;
                };
                let summary = browser
                    .head(&path, PREVIEW_HEAD_BYTES)
                    .map(|bytes| summarize(&bytes, len))
                    .map_err(|error| error.to_string());
                reply(Reply::Previewed(path, summary));
            }
            Request::Export { path, destination } => {
                let Some((_, browser)) = open.as_ref() else {
                    continue;
                };
                reply(
                    match browser.export(&path, &destination, ExportOptions::default()) {
                        Ok(exported) => Reply::Done(format!(
                            "saved {} ({}, {})",
                            exported.destination.display(),
                            widgets::count(exported.files, "file", "files"),
                            widgets::count(exported.bytes, "byte", "bytes")
                        )),
                        Err(error) => Reply::Failed(error.to_string()),
                    },
                );
            }
            Request::View(path) => {
                let Some((_, browser)) = open.as_ref() else {
                    continue;
                };
                reply(match view_in_host(browser, &path) {
                    Ok(file) => {
                        Reply::Done(format!("opened {} in the host's viewer", file.display()))
                    }
                    Err(error) => Reply::Failed(error),
                });
            }
            Request::Install {
                source,
                destination,
            } => {
                let Some((disk, browser)) = open.take() else {
                    continue;
                };
                // The browser's lock must close before the engine writes.
                drop(browser);
                let outcome = disk.install(&source, &destination, false);
                reply(match &outcome {
                    Ok(installed) => Reply::Done(format!(
                        "installed {} ({}) at {destination}, generation {}",
                        widgets::count(installed.files, "file", "files"),
                        widgets::count(installed.bytes, "byte", "bytes"),
                        installed.generation
                    )),
                    Err(error) => Reply::Failed(error.to_string()),
                });
                reopen(disk, &mut open, &reply);
            }
            Request::Remove(path) => {
                let Some((disk, browser)) = open.take() else {
                    continue;
                };
                drop(browser);
                reply(match disk.remove(&path) {
                    Ok(removed) => Reply::Done(format!(
                        "removed {path} (generation {})",
                        removed.generation
                    )),
                    Err(error) => Reply::Failed(error.to_string()),
                });
                reopen(disk, &mut open, &reply);
            }
            Request::Close => {
                drop(open.take());
                reply(Reply::Closed);
            }
        }
    }
}

fn reopen(disk: Disk, open: &mut Option<(Disk, Browser)>, reply: &impl Fn(Reply)) {
    match Browser::open(&disk) {
        Ok(browser) => {
            let status = browser.status();
            reply(Reply::Opened {
                label: disk.path().display().to_string(),
                kind: disk.kind(),
                generation: status.generation,
                fallback: status.used_fallback_root,
            });
            *open = Some((disk, browser));
        }
        Err(error) => reply(Reply::Failed(error.to_string())),
    }
}

/// The pane's summary of a file of `len` bytes from its leading `bytes`.
fn summarize(bytes: &[u8], len: u64) -> Summary {
    match preview::preview(bytes, len) {
        Preview::Image {
            width,
            height,
            capture,
            text,
            ..
        } => Summary::Image {
            width,
            height,
            capture,
            text,
        },
        Preview::Text {
            text,
            lines,
            truncated,
        } => Summary::Text {
            // Set for the pane once, here off the event loop, rather than on
            // every frame.
            text: fonts::shown(fonts::MONO, text, fonts::Breaks::Keep).into_owned(),
            lines,
            truncated,
        },
        Preview::Binary { len, head } => Summary::Binary {
            len,
            head: head
                .chunks(16)
                .take(8)
                .map(|row| {
                    row.iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join("\n"),
        },
    }
}

/// Copy a file out to a private directory and hand it to the host's own
/// viewer (`xdg-open`, or `open` on macOS).
fn view_in_host(browser: &Browser, path: &VolPath) -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("vkore-view-{}", std::process::id()));
    vkore_private_dir(&base).map_err(|error| format!("{}: {error}", base.display()))?;
    browser
        .export(path, &base, ExportOptions { replace: true })
        .map_err(|error| error.to_string())?;
    let file = base.join(path.name().unwrap_or("file"));
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("{opener}: {error}"))?;
    Ok(file)
}

/// `path` with its links resolved, so two spellings of one disk compare equal.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

fn vkore_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// Where a browser is, and what it shows.
pub struct State {
    pub requests: mpsc::Sender<Request>,
    pub disk_input: String,
    pub export_input: String,
    pub import_input: String,
    pub label: Option<String>,
    pub kind: Option<DiskKind>,
    pub generation: u64,
    pub fallback: bool,
    pub place: Place,
    pub path: Vec<String>,
    pub listing: Option<Listing>,
    pub selected: Option<usize>,
    pub summary: Option<(VolPath, Result<Summary, String>)>,
    pub status: String,
    pub busy: bool,
    pub confirm_remove: bool,
    /// The disk last sent to the worker to open, until it closes.
    held: Option<PathBuf>,
}

impl State {
    pub fn new(requests: mpsc::Sender<Request>) -> Self {
        let downloads = std::env::var_os("XDG_DOWNLOAD_DIR")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            requests,
            disk_input: String::new(),
            export_input: downloads.display().to_string(),
            import_input: String::new(),
            label: None,
            kind: None,
            generation: 0,
            fallback: false,
            place: Place::Home(0),
            path: vec!["pictures".into(), "screenshots".into()],
            listing: None,
            selected: None,
            summary: None,
            status: "Open a Kore disk image, a disk's block device, or a machine's disk.".into(),
            busy: false,
            confirm_remove: false,
            held: None,
        }
    }

    /// Whether the browser holds the disk at `path`, or is opening it.
    pub fn holds(&self, path: &Path) -> bool {
        self.held
            .as_deref()
            .is_some_and(|held| held == canonical(path))
    }

    /// The directory being shown.
    pub fn here(&self) -> Option<VolPath> {
        let mut path = match self.place {
            Place::Home(face) => VolPath::home(face).ok()?,
            Place::Root => VolPath::root(),
        };
        for name in &self.path {
            path = path.join(name).ok()?;
        }
        Some(path)
    }

    fn selected_entry(&self) -> Option<&Entry> {
        match (&self.listing, self.selected) {
            (Some(Listing::Directory { entries }), Some(index)) => entries.get(index),
            _ => None,
        }
    }

    fn selected_path(&self) -> Option<VolPath> {
        self.here()?.join(&self.selected_entry()?.name).ok()
    }

    pub fn send(&mut self, request: Request) {
        self.busy = true;
        let _ = self.requests.send(request);
    }

    pub fn list(&mut self) {
        if let Some(here) = self.here() {
            self.selected = None;
            self.summary = None;
            self.confirm_remove = false;
            self.send(Request::List(here));
        }
    }

    /// Open `input`: a path, or `@machine` for a machine's own disk.
    pub fn open(&mut self, input: &str, machine_disk: impl Fn(&str) -> Option<PathBuf>) {
        let input = input.trim();
        let path = match input.strip_prefix('@') {
            Some(machine) => match machine_disk(machine) {
                Some(path) => path,
                None => {
                    self.status = format!("no machine named {machine}");
                    return;
                }
            },
            None => PathBuf::from(input),
        };
        match Disk::new(&path) {
            Ok(disk) => {
                self.held = Some(canonical(&path));
                self.disk_input = input.to_owned();
                self.send(Request::Open {
                    disk,
                    label: input.to_owned(),
                });
            }
            Err(error) => self.status = error.to_string(),
        }
    }

    pub fn reply(&mut self, reply: Reply) {
        self.busy = false;
        match reply {
            Reply::Opened {
                label,
                kind,
                generation,
                fallback,
            } => {
                self.label = Some(label);
                self.kind = Some(kind);
                self.generation = generation;
                self.fallback = fallback;
                self.status = format!("generation {generation}");
                self.list();
            }
            Reply::Listed(path, listing) => {
                if Some(&path) != self.here().as_ref() {
                    return;
                }
                match listing {
                    Ok(listing) => self.listing = Some(listing),
                    Err(error) => {
                        // A folder the machine never created: show where it
                        // would be, and say so.
                        self.listing = None;
                        self.status = error;
                    }
                }
            }
            Reply::Previewed(path, summary) => {
                if Some(&path) == self.selected_path().as_ref() {
                    self.summary = Some((path, summary));
                }
            }
            Reply::Done(message) => self.status = message,
            Reply::Failed(message) => self.status = message,
            Reply::Closed => {
                self.held = None;
                self.label = None;
                self.listing = None;
                self.status = "closed".into();
            }
        }
    }

    pub fn update(
        &mut self,
        message: VolumeMessage,
        machine_disk: impl Fn(&str) -> Option<PathBuf>,
    ) {
        match message {
            VolumeMessage::DiskInput(value) => self.disk_input = value,
            VolumeMessage::Open => {
                let input = self.disk_input.clone();
                self.open(&input, machine_disk);
            }
            VolumeMessage::Close => self.send(Request::Close),
            VolumeMessage::Place(place) => {
                self.place = place;
                self.path.clear();
                self.list();
            }
            VolumeMessage::Crumb(depth) => {
                self.path.truncate(depth);
                self.list();
            }
            VolumeMessage::Select(index) => {
                self.selected = Some(index);
                self.summary = None;
                self.confirm_remove = false;
                match self.selected_entry().map(|entry| entry.kind) {
                    Some(EntryKind::Directory) => {
                        let name = self.selected_entry().map(|entry| entry.name.clone());
                        if let Some(name) = name {
                            self.path.push(name);
                            self.list();
                        }
                    }
                    Some(EntryKind::File) => {
                        let len = self.selected_entry().map_or(0, |entry| entry.len);
                        if let Some(path) = self.selected_path() {
                            self.send(Request::Preview { path, len });
                        }
                    }
                    None => {}
                }
            }
            VolumeMessage::Refresh => self.list(),
            VolumeMessage::ExportInput(value) => self.export_input = value,
            VolumeMessage::ImportInput(value) => self.import_input = value,
            VolumeMessage::Export => {
                let path = self
                    .selected_path()
                    .or_else(|| self.here().filter(|here| !here.is_top()));
                if let Some(path) = path {
                    self.send(Request::Export {
                        path,
                        destination: PathBuf::from(self.export_input.trim()),
                    });
                }
            }
            VolumeMessage::View => {
                if let Some(path) = self.selected_path() {
                    self.send(Request::View(path));
                }
            }
            VolumeMessage::Import => {
                let source = PathBuf::from(self.import_input.trim());
                let Some(name) = source
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
                else {
                    self.status = "name a host file or directory to import".into();
                    return;
                };
                match self.here().and_then(|here| here.join(&name).ok()) {
                    Some(destination) if matches!(self.place, Place::Home(_)) => {
                        self.send(Request::Install {
                            source,
                            destination,
                        });
                    }
                    _ => self.status = "imports land below a terminal's home".into(),
                }
            }
            VolumeMessage::Remove => {
                if !self.confirm_remove {
                    self.confirm_remove = true;
                    return;
                }
                self.confirm_remove = false;
                if let Some(path) = self.selected_path() {
                    self.send(Request::Remove(path));
                }
            }
        }
    }
}

/// The browser's own messages.
#[derive(Debug, Clone)]
pub enum VolumeMessage {
    DiskInput(String),
    Open,
    Close,
    Place(Place),
    Crumb(usize),
    Select(usize),
    Refresh,
    ExportInput(String),
    ImportInput(String),
    Export,
    View,
    Import,
    Remove,
}

fn size(len: u64) -> String {
    match len {
        0..1024 => format!("{len} B"),
        1024..1_048_576 => format!("{:.1} KiB", len as f64 / 1024.0),
        _ => format!("{:.1} MiB", len as f64 / 1_048_576.0),
    }
}

fn wrap(message: VolumeMessage) -> Message {
    Message::Volume(message)
}

/// The browser.
pub fn view<'a>(state: &'a State, machines: &[String]) -> UiElement<'a> {
    let open_row = row![
        text_input(
            "/path/to/disk.raw, /dev/nvme0n1p5, or @machine",
            &state.disk_input
        )
        .on_input(|value| wrap(VolumeMessage::DiskInput(value)))
        .on_submit(wrap(VolumeMessage::Open))
        .font(fonts::MONO)
        .padding([7, 10])
        .size(theme::SIZE_SM)
        .style(widgets::field),
        button(widgets::control("Open"))
            .height(theme::CONTROL_SM)
            .padding([8, 14])
            .style(widgets::button_style(widgets::Button::Primary))
            .on_press_maybe((!state.busy).then(|| wrap(VolumeMessage::Open))),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Center);
    let mut quick = row![].spacing(theme::SPACE_1);
    for machine in machines {
        let input = format!("@{machine}");
        quick = quick.push(
            button(widgets::mono(input.clone(), theme::TEXT_SECONDARY))
                .padding([4, 8])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press(wrap(VolumeMessage::DiskInput(input))),
        );
    }
    let mut page = column![widgets::display("Volumes"), open_row, quick].spacing(theme::SPACE_4);
    let Some(label) = &state.label else {
        return container(page.push(widgets::data(state.status.as_str(), theme::TEXT_MUTED)))
            .padding(theme::SPACE_6)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
    };

    let kind = match state.kind {
        Some(DiskKind::Device) => "disk",
        _ => "image",
    };
    let header = row![
        widgets::data(
            format!(
                "{label} · {kind} · generation {}{}",
                state.generation,
                if state.fallback {
                    " · fallback root: read-only"
                } else {
                    ""
                }
            ),
            theme::TEXT_SECONDARY,
        ),
        space::horizontal(),
        button(widgets::control("Refresh"))
            .height(theme::CONTROL_SM)
            .padding([6, 12])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press(wrap(VolumeMessage::Refresh)),
        button(widgets::control("Close"))
            .height(theme::CONTROL_SM)
            .padding([6, 12])
            .style(widgets::button_style(widgets::Button::Ghost))
            .on_press(wrap(VolumeMessage::Close)),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Center);
    let places = row![
        place_button("Terminal 0", Place::Home(0), state.place),
        place_button("Terminal 1", Place::Home(1), state.place),
        place_button("Volume root", Place::Root, state.place),
    ]
    .spacing(theme::SPACE_1);
    let mut crumbs = row![
        button(widgets::mono(
            match state.place {
                Place::Home(face) => format!("~{face}"),
                Place::Root => "/".into(),
            },
            theme::TEXT
        ))
        .padding([4, 6])
        .style(widgets::button_style(widgets::Button::Ghost))
        .on_press(wrap(VolumeMessage::Crumb(0)))
    ]
    .spacing(2)
    .align_y(alignment::Vertical::Center);
    for (depth, name) in state.path.iter().enumerate() {
        // The root's own crumb is `/`: a separator before its first
        // component would read `/ / work`.
        if depth > 0 || matches!(state.place, Place::Home(_)) {
            crumbs = crumbs.push(widgets::mono("/", theme::TEXT_MUTED));
        }
        crumbs = crumbs.push(
            button(widgets::data(name.as_str(), theme::TEXT))
                .padding([4, 6])
                .style(widgets::button_style(widgets::Button::Ghost))
                .on_press(wrap(VolumeMessage::Crumb(depth + 1))),
        );
    }

    let mut list = column![].spacing(2);
    match &state.listing {
        Some(Listing::Directory { entries }) if !entries.is_empty() => {
            for (index, entry) in entries.iter().enumerate() {
                let selected = state.selected == Some(index);
                let (glyph, detail) = match entry.kind {
                    EntryKind::Directory => ("▸", String::new()),
                    EntryKind::File => (" ", size(entry.len)),
                };
                list = list.push(
                    button(
                        row![
                            widgets::mono(glyph, theme::MAGENTA),
                            widgets::data(entry.name.as_str(), theme::TEXT).size(theme::SIZE_SM),
                            space::horizontal(),
                            widgets::mono(detail, theme::TEXT_MUTED),
                        ]
                        .spacing(theme::SPACE_2)
                        .align_y(alignment::Vertical::Center),
                    )
                    .width(Length::Fill)
                    .padding([7, 10])
                    .style(widgets::button_style(widgets::Button::Row(selected)))
                    .on_press(wrap(VolumeMessage::Select(index))),
                );
            }
        }
        Some(Listing::EmptyHome) => {
            list = list.push(widgets::body(
                "This terminal's home is created at its machine's first boot.",
                theme::SIZE_SM,
                theme::TEXT_MUTED,
            ));
        }
        Some(Listing::File { .. }) | Some(Listing::Directory { .. }) | None => {
            list = list.push(widgets::body(
                "Nothing here.",
                theme::SIZE_SM,
                theme::TEXT_MUTED,
            ));
        }
    }

    let detail = detail_pane(state);
    let body = row![
        container(scrollable(list).height(Length::Fill))
            .width(Length::FillPortion(3))
            .height(Length::Fill),
        container(detail)
            .width(Length::FillPortion(2))
            .height(Length::Fill)
            .padding(theme::SPACE_4)
            .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG)),
    ]
    .spacing(theme::SPACE_4)
    .height(Length::Fill);

    let transfer = row![
        text_input("host folder to save into", &state.export_input)
            .on_input(|value| wrap(VolumeMessage::ExportInput(value)))
            .font(fonts::MONO)
            .padding([7, 10])
            .size(theme::SIZE_SM)
            .style(widgets::field),
        button(widgets::control("Save here"))
            .height(theme::CONTROL_SM)
            .padding([8, 14])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press_maybe((!state.busy).then(|| wrap(VolumeMessage::Export))),
        text_input("host file or folder to import here", &state.import_input)
            .on_input(|value| wrap(VolumeMessage::ImportInput(value)))
            .on_submit(wrap(VolumeMessage::Import))
            .font(fonts::MONO)
            .padding([7, 10])
            .size(theme::SIZE_SM)
            .style(widgets::field),
        button(widgets::control("Import"))
            .height(theme::CONTROL_SM)
            .padding([8, 14])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press_maybe(
                (!state.busy && matches!(state.place, Place::Home(_)))
                    .then(|| wrap(VolumeMessage::Import))
            ),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Center);

    page = column![
        widgets::display("Volumes"),
        header,
        places,
        crumbs,
        body,
        transfer,
        widgets::data(
            if state.busy {
                "working…"
            } else {
                state.status.as_str()
            },
            theme::TEXT_MUTED,
        ),
    ]
    .spacing(theme::SPACE_2);
    container(page)
        .padding(theme::SPACE_6)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn place_button<'a>(label: &'a str, place: Place, current: Place) -> UiElement<'a> {
    button(widgets::control(label))
        .height(theme::CONTROL_SM)
        .padding([6, 12])
        .style(widgets::button_style(widgets::Button::Choice(
            place == current,
        )))
        .on_press(wrap(VolumeMessage::Place(place)))
        .into()
}

fn detail_pane(state: &State) -> UiElement<'_> {
    let Some(entry) = state
        .selected_entry()
        .filter(|entry| entry.kind == EntryKind::File)
    else {
        return widgets::body(
            "Select a file to see what it holds. Directories open on select.",
            theme::SIZE_SM,
            theme::TEXT_MUTED,
        )
        .into();
    };
    let mut pane = column![
        widgets::data(entry.name.as_str(), theme::TEXT).size(theme::SIZE_MD),
        widgets::mono(size(entry.len), theme::TEXT_MUTED),
    ]
    .spacing(theme::SPACE_2);
    match state.summary.as_ref().map(|(_, summary)| summary) {
        None => pane = pane.push(widgets::mono("reading…", theme::TEXT_MUTED)),
        Some(Err(error)) => pane = pane.push(widgets::data(error.as_str(), theme::ERROR)),
        Some(Ok(Summary::Image {
            width,
            height,
            capture,
            text,
        })) => {
            pane = pane.push(widgets::mono(
                format!("PNG · {width}×{height}"),
                theme::TEXT_SECONDARY,
            ));
            if let Some(capture) = capture {
                pane = pane.push(widgets::label("Kore capture", theme::MAGENTA));
                for (key, value) in [
                    ("build", &capture.kernel_build),
                    ("taken", &capture.created),
                    ("mode", &capture.mode),
                    ("app", &capture.app),
                    ("screen", &capture.screen),
                    ("rect", &capture.rect),
                    ("density %", &capture.density_percent),
                    ("uptime ms", &capture.uptime_ms),
                ] {
                    if let Some(value) = value {
                        pane = pane.push(widgets::data(format!("{key:<10} {value}"), theme::TEXT));
                    }
                }
                // What Kore said beyond the record's own keywords.
                for (key, value) in text
                    .iter()
                    .filter(|(key, _)| {
                        key.starts_with("kore:") && !Capture::KEYWORDS.contains(&key.as_str())
                    })
                    .take(16)
                {
                    let key = &key["kore:".len()..];
                    pane = pane.push(widgets::data(
                        format!("{key:<10} {value}"),
                        theme::TEXT_SECONDARY,
                    ));
                }
            } else if !text.is_empty() {
                for (key, value) in text.iter().take(8) {
                    pane = pane.push(widgets::data(
                        format!("{key}: {value}"),
                        theme::TEXT_SECONDARY,
                    ));
                }
            }
        }
        Some(Ok(Summary::Text {
            text,
            lines,
            truncated,
        })) => {
            pane = pane.push(widgets::mono(
                format!(
                    "text · {lines} lines{}",
                    if *truncated { " (first part)" } else { "" }
                ),
                theme::TEXT_SECONDARY,
            ));
            pane = pane.push(
                scrollable(widgets::mono(text.as_str(), theme::TEXT))
                    .height(Length::Fill)
                    .style(widgets::hidden_scrollbar),
            );
        }
        Some(Ok(Summary::Binary { len, head })) => {
            pane = pane.push(widgets::mono(
                format!("binary · {}", size(*len)),
                theme::TEXT_SECONDARY,
            ));
            pane = pane.push(widgets::mono(head.as_str(), theme::TEXT_MUTED));
        }
    }
    let actions = row![
        button(widgets::control("Open"))
            .height(theme::CONTROL_SM)
            .padding([6, 12])
            .style(widgets::button_style(widgets::Button::Primary))
            .on_press_maybe((!state.busy).then(|| wrap(VolumeMessage::View))),
        button(widgets::control("Save"))
            .height(theme::CONTROL_SM)
            .padding([6, 12])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press_maybe((!state.busy).then(|| wrap(VolumeMessage::Export))),
        button(widgets::control(if state.confirm_remove {
            "Confirm remove"
        } else {
            "Remove"
        }))
        .height(theme::CONTROL_SM)
        .padding([6, 12])
        .style(widgets::button_style(widgets::Button::Danger))
        .on_press_maybe(
            (!state.busy && matches!(state.place, Place::Home(_)))
                .then(|| wrap(VolumeMessage::Remove)),
        ),
    ]
    .spacing(theme::SPACE_2);
    column![pane, space::vertical(), actions]
        .spacing(theme::SPACE_2)
        .height(Length::Fill)
        .into()
}
