//! The volume browser: a Kore machine's DagFS volume, navigable from the
//! host through Kore's own engine.
//!
//! One worker thread owns the open volume (a [`Browser`] holds the disk's
//! lock for as long as it is open) and answers the view's requests in order;
//! the view never blocks on the disk. Installing or removing closes the
//! browser, writes through Kore's all-or-nothing publication, and reopens.

use crate::typography::Typography;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
};

use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, space, text_input};
use iced_winit::winit::event_loop::EventLoopProxy;

use vkore_volume::{
    Browser, Disk, DiskKind, Entry, EntryKind, ExportOptions, Listing, Place, Preview, VolPath,
    preview::{self, Capture},
};

use crate::{
    Message, UiElement, UserEvent, fonts, kit,
    motion::{self, Beat},
    theme,
    visuals::{self, Icon},
    widgets,
};

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
    PickImage,
    DiscoverDisks,
    IdentifyDisks(vkore_volume::PhysicalDisks),
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
        format_version: u32,
        fallback: bool,
        read_only: bool,
    },
    ImagePicked(Option<PathBuf>),
    Disks(Result<vkore_volume::PhysicalDisks, String>),
    OpenFailed(String),
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
            Request::PickImage => {
                let path = rfd::FileDialog::new()
                    .set_title("Open a Kore disk image")
                    .add_filter("Disk images", &["raw", "img", "disk"])
                    .add_filter("All files", &["*"])
                    .pick_file();
                reply(Reply::ImagePicked(path));
            }
            Request::DiscoverDisks => reply(Reply::Disks(
                vkore_volume::physical_disks().map_err(|error| error.to_string()),
            )),
            Request::IdentifyDisks(mut inventory) => {
                match vkore_volume::identify_disks(&mut inventory) {
                    Ok(()) => reply(Reply::Disks(Ok(inventory))),
                    Err(error) => reply(Reply::Disks(Err(error.to_string()))),
                }
            }
            Request::Open { disk, label } => {
                // Release any disk held before taking another.
                drop(open.take());
                match Browser::open_with_authorization(&disk) {
                    Ok(browser) => {
                        let status = browser.status();
                        reply(Reply::Opened {
                            label,
                            kind: disk.kind(),
                            generation: status.generation,
                            format_version: status.format_version,
                            fallback: status.used_fallback_root,
                            read_only: browser.is_authorized_read_only(),
                        });
                        open = Some((disk, browser));
                    }
                    Err(error) if error.is_blank() => reply(Reply::OpenFailed(format!(
                        "{label}: a blank disk — its machine formats it at first boot"
                    ))),
                    Err(error) if error.is_in_use() => reply(Reply::OpenFailed(format!(
                        "{label}: in use — a machine is running on it; stop it first"
                    ))),
                    Err(error) => reply(Reply::OpenFailed(format!("{label}: {error}"))),
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
                if browser.is_authorized_read_only() || browser.status().used_fallback_root {
                    reply(Reply::Failed("This volume is open read-only.".into()));
                    open = Some((disk, browser));
                    continue;
                }
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
                if browser.is_authorized_read_only() || browser.status().used_fallback_root {
                    reply(Reply::Failed("This volume is open read-only.".into()));
                    open = Some((disk, browser));
                    continue;
                }
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
                format_version: status.format_version,
                fallback: status.used_fallback_root,
                read_only: browser.is_authorized_read_only(),
            });
            *open = Some((disk, browser));
        }
        Err(error) => reply(Reply::OpenFailed(error.to_string())),
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
    pub format_version: Option<u32>,
    pub fallback: bool,
    pub read_only: bool,
    pub disk_choices: Option<vkore_volume::PhysicalDisks>,
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
            format_version: None,
            fallback: false,
            read_only: false,
            disk_choices: None,
            place: Place::Root,
            path: Vec::new(),
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
        if self.requests.send(request).is_err() {
            self.busy = false;
            self.status = "The volume worker stopped. Restart vKore to reconnect.".into();
        }
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
        if self.busy {
            return;
        }
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
        self.open_path(path, input.to_owned());
    }

    fn open_path(&mut self, path: PathBuf, label: String) {
        match Disk::new(&path) {
            Ok(disk) => {
                self.label = None;
                self.format_version = None;
                self.listing = None;
                self.selected = None;
                self.summary = None;
                self.disk_choices = None;
                self.place = Place::Root;
                self.path.clear();
                self.status =
                    "Opening volume… Disk access may require system authorization.".into();
                self.held = Some(canonical(&path));
                self.disk_input = label.clone();
                self.send(Request::Open { disk, label });
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
                format_version,
                fallback,
                read_only,
            } => {
                self.label = Some(label);
                self.kind = Some(kind);
                self.generation = generation;
                self.format_version = Some(format_version);
                self.fallback = fallback;
                self.read_only = read_only || fallback;
                self.status = format!("generation {generation}");
                self.list();
            }
            Reply::ImagePicked(path) => {
                if let Some(path) = path {
                    self.open_path(path.clone(), path.display().to_string());
                }
            }
            Reply::Disks(result) => match result {
                Ok(disks) => {
                    self.disk_choices = Some(disks);
                    self.status.clear();
                }
                Err(error) => self.status = error,
            },
            Reply::OpenFailed(error) => {
                self.held = None;
                self.label = None;
                self.format_version = None;
                self.listing = None;
                self.selected = None;
                self.summary = None;
                self.read_only = false;
                self.status = error;
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
                self.format_version = None;
                self.listing = None;
                self.summary = None;
                self.selected = None;
                self.read_only = false;
                self.status = "closed".into();
            }
        }
    }

    pub fn update(
        &mut self,
        message: VolumeMessage,
        machine_disk: impl Fn(&str) -> Option<PathBuf>,
    ) {
        if self.busy {
            return;
        }
        if self.disk_choices.is_some()
            && !matches!(
                &message,
                VolumeMessage::ChooseDisk
                    | VolumeMessage::IdentifyDisks
                    | VolumeMessage::CancelDisk
                    | VolumeMessage::SelectDisk(_)
            )
        {
            return;
        }
        match message {
            VolumeMessage::PickImage => self.send(Request::PickImage),
            VolumeMessage::ChooseDisk => self.send(Request::DiscoverDisks),
            VolumeMessage::IdentifyDisks => {
                if let Some(inventory) = self.disk_choices.clone() {
                    self.status = "Identifying DagFS volumes…".into();
                    self.send(Request::IdentifyDisks(inventory));
                }
            }
            VolumeMessage::CancelDisk => self.disk_choices = None,
            VolumeMessage::SelectDisk(path) => {
                self.open_path(path.clone(), path.display().to_string())
            }
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
            VolumeMessage::Import if self.read_only => {}
            VolumeMessage::Remove if self.read_only => {}
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
    PickImage,
    ChooseDisk,
    IdentifyDisks,
    SelectDisk(PathBuf),
    CancelDisk,
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
pub fn view<'a>(state: &'a State, machines: &[String], ty: Typography) -> UiElement<'a> {
    let page = content(state, machines, ty);
    let Some(inventory) = &state.disk_choices else {
        return page;
    };
    let mut choices = column![].spacing(theme::SPACE_2);
    for disk in &inventory.disks {
        let mut details = column![
            ty.data(
                disk.name()
                    .map(str::to_owned)
                    .unwrap_or_else(|| disk.path.display().to_string()),
                theme::TEXT
            ),
        ]
        .spacing(theme::SPACE_1);
        if disk.name().is_some() {
            details = details.push(ty.data(disk.path.display().to_string(), theme::TEXT_SECONDARY));
        }
        details = details.push(ty.data(
            format!(
                "{} · {:.1} GiB",
                disk.description,
                disk.bytes as f64 / (1u64 << 30) as f64
            ),
            theme::TEXT_MUTED,
        ));
        if let Some(label) = &disk.host.label
            && Some(label.as_str()) != disk.name()
        {
            details =
                details.push(ty.data(format!("Filesystem label: {label}"), theme::TEXT_MUTED));
        }
        if let Some(kind) = &disk.host.fstype {
            let version = disk
                .host
                .fsver
                .as_ref()
                .map(|v| format!(" · version {v}"))
                .unwrap_or_default();
            details =
                details.push(ty.data(format!("Host reports: {kind}{version}"), theme::TEXT_MUTED));
        }
        let (icon, description, tone) = match &disk.dagfs {
            vkore_volume::DagfsPresence::Detected(status) => (
                Icon::Check,
                format!(
                    "DagFS v{} · generation {}{}",
                    status.format_version,
                    status.generation,
                    if status.used_fallback_root {
                        " · fallback root"
                    } else {
                        ""
                    }
                ),
                theme::SUCCESS,
            ),
            vkore_volume::DagfsPresence::NeedsAuthorization => (
                Icon::Lock,
                "DagFS unknown · authorize identification".into(),
                theme::TEXT_SECONDARY,
            ),
            vkore_volume::DagfsPresence::Blank => (
                Icon::Info,
                "Blank volume · no DagFS yet".into(),
                theme::TEXT_SECONDARY,
            ),
            vkore_volume::DagfsPresence::NotDetected => (
                Icon::Info,
                "DagFS not detected".into(),
                theme::TEXT_SECONDARY,
            ),
            vkore_volume::DagfsPresence::Unavailable(reason) => (
                Icon::Alert,
                format!("DagFS unverified · {reason}"),
                theme::WARNING,
            ),
        };
        let plate = ty.size(40.);
        let card = row![
            container(visuals::icon(
                Icon::Volume,
                theme::TEXT_SECONDARY,
                plate * 0.5
            ))
            .center(plate)
            .style(widgets::well),
            column![details, kit::chip(ty, icon, description, tone)]
                .spacing(theme::SPACE_2)
                .width(Length::Fill),
            visuals::icon(Icon::ChevronRight, theme::TEXT_MUTED, ty.size(16.)),
        ]
        .spacing(theme::SPACE_4 - 2.)
        .align_y(alignment::Vertical::Center);
        choices = choices.push(kit::lift(
            widgets::Button::Secondary,
            !state.busy,
            button(card)
                .width(Length::Fill)
                .padding(12)
                .style(widgets::button_style_fading(widgets::Button::Secondary))
                .on_press_maybe(
                    (!state.busy).then(|| wrap(VolumeMessage::SelectDisk(disk.path.clone()))),
                ),
        ));
    }
    if inventory.disks.is_empty() {
        choices = choices.push(ty.empty_state(
            Icon::Volume,
            "No disks reported",
            "This host did not list any physical disks.",
        ));
    }
    let blocked = inventory
        .disks
        .iter()
        .any(|disk| disk.dagfs == vkore_volume::DagfsPresence::NeedsAuthorization);
    let mut notes = column![].spacing(theme::SPACE_1);
    if let Some(warning) = &inventory.metadata_warning {
        notes = notes.push(ty.data(warning, theme::WARNING));
    }
    if !state.status.is_empty() {
        notes = notes.push(ty.data(&state.status, theme::TEXT_SECONDARY));
    }
    let panel = container(column![
        ty.body("Choose a physical disk", 20.0, theme::TEXT),
        ty.body("Stop any machine using the disk first. Access is read-only; Linux may ask you to authorize it.", theme::SIZE_SM, theme::TEXT_SECONDARY),
        widgets::scroll(choices).height(Length::Fill),
        notes,
        row![
            kit::icon_label(ty, Icon::Info, "Identify DagFS…", widgets::Button::Primary,
                (!state.busy && blocked).then(|| wrap(VolumeMessage::IdentifyDisks))),
            kit::icon_label(ty, Icon::Refresh, "Refresh", widgets::Button::Secondary,
                (!state.busy).then(|| wrap(VolumeMessage::ChooseDisk))),
            kit::icon_label(ty, Icon::Close, "Cancel", widgets::Button::Secondary,
                Some(wrap(VolumeMessage::CancelDisk))),
        ].spacing(theme::SPACE_2).wrap(),
    ].spacing(theme::SPACE_2)).padding(24).width(Length::Fill.max(680))
        .height(Length::Fill.max(520)).style(widgets::dialog);
    iced_widget::stack![
        page,
        iced_widget::opaque(
            container(panel)
                .style(|appearance| container::Style {
                    background: Some(iced_core::Background::Color(iced_core::Color {
                        a: 0.92,
                        ..theme::color(appearance, theme::BACKGROUND)
                    })),
                    ..container::Style::default()
                })
                .padding(24)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
        )
    ]
    .into()
}

fn content<'a>(state: &'a State, machines: &[String], ty: Typography) -> UiElement<'a> {
    let Some(label) = &state.label else {
        return connect(state, machines, ty);
    };
    let plate = ty.size(44.);

    // What this volume is, as facts rather than one run-on line.
    let (kind_icon, kind) = match state.kind {
        Some(DiskKind::Device) => (Icon::Volume, "disk"),
        _ => (Icon::File, "image"),
    };
    let mut facts = row![
        kit::chip(ty, kind_icon, kind, theme::TEXT_SECONDARY),
        match state.format_version {
            Some(version) =>
                kit::chip(ty, Icon::Check, format!("DagFS v{version}"), theme::SUCCESS),
            None => kit::chip(ty, Icon::Alert, "DagFS version unknown", theme::WARNING),
        },
        kit::chip(
            ty,
            Icon::Hash,
            format!("generation {}", state.generation),
            theme::TEXT_SECONDARY
        ),
    ]
    .spacing(theme::SPACE_2);
    if state.fallback {
        facts = facts.push(kit::chip(
            ty,
            Icon::Lock,
            "fallback root · read-only",
            theme::WARNING,
        ));
    } else if state.read_only {
        facts = facts.push(kit::chip(
            ty,
            Icon::Lock,
            "authorized read-only",
            theme::TEXT_SECONDARY,
        ));
    }
    let header = container(
        row![
            container(visuals::icon(Icon::Volume, theme::MAGENTA, plate * 0.5))
                .center(plate)
                .style(|appearance| theme::tinted(
                    appearance,
                    theme::MAGENTA,
                    0.13,
                    0.30,
                    theme::RADIUS_MD
                )),
            column![
                ty.data(label.as_str(), theme::TEXT)
                    .size(ty.size(20.))
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                facts.wrap(),
            ]
            .spacing(theme::SPACE_1 + 2.)
            .width(Length::Fill),
            kit::icon_button(
                ty,
                Icon::Refresh,
                "Refresh",
                widgets::Button::Secondary,
                Some(wrap(VolumeMessage::Refresh)),
            ),
            kit::icon_button(
                ty,
                Icon::Close,
                "Close volume",
                widgets::Button::Ghost,
                Some(wrap(VolumeMessage::Close)),
            ),
        ]
        .spacing(theme::SPACE_4 - 2.)
        .align_y(alignment::Vertical::Center),
    )
    .padding([theme::SPACE_2 + 4., theme::SPACE_4])
    .width(Length::Fill)
    .style(widgets::card);

    // Where to look: the volume's places as a segmented choice, then the path
    // taken to get here.
    let places = row![
        place_button(
            ty,
            Icon::Terminal,
            "Terminal 0",
            Place::Home(0),
            state.place
        ),
        place_button(
            ty,
            Icon::Terminal,
            "Terminal 1",
            Place::Home(1),
            state.place
        ),
        place_button(ty, Icon::Volume, "Volume root", Place::Root, state.place),
    ]
    .spacing(theme::SPACE_2);
    let crumb = |content: UiElement<'a>, depth: usize| -> UiElement<'a> {
        kit::lift(
            widgets::Button::Ghost,
            true,
            button(content)
                .padding([4, 6])
                .style(widgets::button_style_fading(widgets::Button::Ghost))
                .on_press(wrap(VolumeMessage::Crumb(depth))),
        )
    };
    let mut crumbs = row![
        visuals::icon(Icon::Folder, theme::TEXT_MUTED, ty.size(14.)),
        crumb(
            ty.mono(
                match state.place {
                    Place::Home(face) => format!("~{face}"),
                    Place::Root => "/".into(),
                },
                theme::TEXT
            )
            .into(),
            0
        ),
    ]
    .spacing(theme::SPACE_1)
    .align_y(alignment::Vertical::Center);
    for (depth, name) in state.path.iter().enumerate() {
        crumbs = crumbs
            .push(visuals::icon(
                Icon::ChevronRight,
                theme::TEXT_MUTED,
                ty.size(12.),
            ))
            .push(crumb(ty.data(name.as_str(), theme::TEXT).into(), depth + 1));
    }
    let trail = container(crumbs)
        .padding([theme::SPACE_1, theme::SPACE_2])
        .width(Length::Fill)
        .style(widgets::well);

    let mut list = column![].spacing(2);
    match &state.listing {
        Some(Listing::Directory { entries }) if !entries.is_empty() => {
            for (index, entry) in entries.iter().enumerate() {
                let selected = state.selected == Some(index);
                let (glyph, trailing): (UiElement<'_>, UiElement<'_>) = match entry.kind {
                    EntryKind::Directory => (
                        visuals::icon(Icon::Folder, theme::TEXT_SECONDARY, ty.size(16.)),
                        visuals::icon(Icon::ChevronRight, theme::TEXT_MUTED, ty.size(14.)),
                    ),
                    EntryKind::File => (
                        visuals::icon(Icon::File, theme::TEXT_MUTED, ty.size(16.)),
                        ty.mono(size(entry.len), theme::TEXT_MUTED).into(),
                    ),
                };
                list = list.push(kit::lift(
                    widgets::Button::Row(selected),
                    true,
                    button(
                        row![
                            glyph,
                            ty.data(entry.name.as_str(), theme::TEXT)
                                .size(ty.size(theme::SIZE_SM)),
                            space::horizontal(),
                            trailing,
                        ]
                        .spacing(theme::SPACE_2 + 2.)
                        .align_y(alignment::Vertical::Center),
                    )
                    .width(Length::Fill)
                    .padding([7, 10])
                    .style(widgets::button_style_fading(widgets::Button::Row(selected)))
                    .on_press(wrap(VolumeMessage::Select(index))),
                ));
            }
        }
        Some(Listing::EmptyHome) => {
            list = list.push(quiet(
                ty,
                Icon::Terminal,
                "This terminal's home is created at its machine's first boot.",
            ));
        }
        Some(Listing::File { .. }) | Some(Listing::Directory { .. }) | None => {
            list = list.push(quiet(ty, Icon::Folder, "Nothing here."));
        }
    }

    let detail = detail_pane(state, ty);
    let body = row![
        container(
            column![
                row![
                    ty.label("Name", theme::TEXT_MUTED),
                    space::horizontal(),
                    ty.label("Size", theme::TEXT_MUTED),
                ]
                .padding([0, 10]),
                widgets::scroll(list).height(Length::Fill),
            ]
            .spacing(theme::SPACE_2),
        )
        .width(Length::FillPortion(3))
        .padding(12)
        .style(widgets::card)
        .height(Length::Fill),
        container(detail)
            .width(Length::FillPortion(2))
            .height(Length::Fill)
            .padding(theme::SPACE_4)
            .style(widgets::card),
    ]
    .spacing(theme::SPACE_4)
    .height(Length::Fill);

    // Moving files: each direction is its own well, named for what it does.
    let field =
        |placeholder: &'static str, value: &'a str, on_input: fn(String) -> VolumeMessage| {
            text_input(placeholder, value)
                .on_input(move |value| wrap(on_input(value)))
                .font(fonts::MONO)
                .padding([7, 10])
                .size(ty.size(theme::SIZE_SM))
                .style(widgets::field)
        };
    let save = container(
        column![
            kit::heading(ty, Icon::Download, "Save to this computer"),
            row![
                field(
                    "host folder to save into",
                    &state.export_input,
                    VolumeMessage::ExportInput
                ),
                kit::icon_label(
                    ty,
                    Icon::Download,
                    "Save here",
                    widgets::Button::Secondary,
                    (!state.busy).then(|| wrap(VolumeMessage::Export)),
                ),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(theme::SPACE_1 + 2.),
    )
    .padding(theme::SPACE_2 + 4.)
    .width(Length::FillPortion(1))
    .style(widgets::well);
    let add = container(
        column![
            kit::heading(ty, Icon::Plus, "Add from this computer"),
            row![
                field(
                    "host file or folder to import here",
                    &state.import_input,
                    VolumeMessage::ImportInput
                )
                .on_submit(wrap(VolumeMessage::Import)),
                kit::icon_label(
                    ty,
                    Icon::Plus,
                    "Import",
                    widgets::Button::Secondary,
                    (!state.busy && !state.read_only && matches!(state.place, Place::Home(_)))
                        .then(|| wrap(VolumeMessage::Import)),
                ),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
        ]
        .spacing(theme::SPACE_1 + 2.),
    )
    .padding(theme::SPACE_2 + 4.)
    .width(Length::FillPortion(1))
    .style(widgets::well);
    let transfer = row![save, add].spacing(theme::SPACE_2);

    let page = column![
        ty.page_header(
            Icon::Volume,
            "Volumes",
            "Explore the files inside your Kore workspace."
        ),
        header,
        places,
        trail,
        body,
        transfer,
        progress(state, ty),
    ]
    .spacing(theme::SPACE_2 + 2.);
    widgets::page_frame(page)
}

/// A quiet line in an otherwise empty list: a glyph and a sentence.
fn quiet<'a>(ty: Typography, icon: Icon, text: &'static str) -> UiElement<'a> {
    container(
        row![
            visuals::icon(icon, theme::TEXT_MUTED, ty.size(16.)),
            ty.body(text, theme::SIZE_SM, theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center),
    )
    .padding(theme::SPACE_4)
    .into()
}

/// What the worker is doing, or last said: a turning mark while it works, the
/// words beside an info glyph after, nothing when there is nothing to say.
fn progress<'a>(state: &'a State, ty: Typography) -> UiElement<'a> {
    if state.busy {
        row![
            motion::beacon(Beat::Spin, theme::TEXT_SECONDARY, ty.size(16.)),
            ty.body("Working…", theme::SIZE_SM, theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center)
        .into()
    } else if state.status.is_empty() {
        space::vertical().height(0).into()
    } else {
        row![
            visuals::icon(Icon::Info, theme::TEXT_MUTED, ty.size(14.)),
            ty.data(state.status.as_str(), theme::TEXT_MUTED)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center)
        .into()
    }
}

/// The page before a volume is open: where one can come from.
fn connect<'a>(state: &'a State, machines: &[String], ty: Typography) -> UiElement<'a> {
    let open_row = row![
        text_input(
            "/path/to/disk.raw, /dev/nvme0n1p5, or @machine",
            &state.disk_input
        )
        .on_input(|value| wrap(VolumeMessage::DiskInput(value)))
        .on_submit(wrap(VolumeMessage::Open))
        .font(fonts::MONO)
        .padding([7, 10])
        .size(ty.size(theme::SIZE_SM))
        .style(widgets::field),
        kit::icon_label(
            ty,
            Icon::Forward,
            "Open",
            widgets::Button::Primary,
            (!state.busy).then(|| wrap(VolumeMessage::Open)),
        ),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Center);
    let mut quick = row![].spacing(theme::SPACE_2);
    for machine in machines {
        let input = format!("@{machine}");
        quick = quick.push(kit::lift(
            widgets::Button::Secondary,
            true,
            button(
                row![
                    visuals::icon(Icon::Machine, theme::TEXT_SECONDARY, ty.size(14.)),
                    ty.data(input.clone(), theme::TEXT_SECONDARY),
                ]
                .spacing(theme::SPACE_1 + 2.)
                .align_y(alignment::Vertical::Center),
            )
            .padding([4, 10])
            .style(widgets::button_style_fading(widgets::Button::Secondary))
            .on_press(wrap(VolumeMessage::DiskInput(input))),
        ));
    }
    let manual = container(
        column![
            kit::heading(ty, Icon::Terminal, "Or type where it is"),
            open_row,
            quick.wrap(),
        ]
        .spacing(theme::SPACE_2),
    )
    .padding(theme::SPACE_4)
    .width(Length::Fill)
    .style(widgets::well);
    let connect = container(
        column![
            container(ty.empty_state(
                Icon::Volume,
                "Your files, closer to Kore",
                "Browse a stopped machine's volume, inspect captures and move project files between Kore and this computer."
            ))
            .height(240),
            row![
                kit::icon_label(
                    ty,
                    Icon::File,
                    "Choose image…",
                    widgets::Button::Primary,
                    (!state.busy).then(|| wrap(VolumeMessage::PickImage)),
                ),
                kit::icon_label(
                    ty,
                    Icon::Volume,
                    "Choose physical disk…",
                    widgets::Button::Secondary,
                    (!state.busy && cfg!(target_os = "linux"))
                        .then(|| wrap(VolumeMessage::ChooseDisk)),
                ),
            ]
            .spacing(theme::SPACE_2)
            .wrap(),
            manual,
            ty.body(
                "Physical disks can be browsed and exported read-only.",
                theme::SIZE_SM,
                theme::TEXT_MUTED
            ),
            progress(state, ty),
        ]
        .spacing(16),
    )
    .padding(24)
    .width(Length::Fill.max(760))
    .style(widgets::card);
    widgets::page_frame(
        column![
            ty.page_header(
                Icon::Volume,
                "Volumes",
                "Explore the files inside your Kore workspace."
            ),
            widgets::scroll(container(connect).center_x(Length::Fill).padding([24, 0]))
                .height(Length::Fill),
        ]
        .spacing(24),
    )
}

fn place_button<'a>(
    ty: Typography,
    icon: Icon,
    label: &'static str,
    place: Place,
    current: Place,
) -> UiElement<'a> {
    kit::icon_label(
        ty,
        icon,
        label,
        widgets::Button::Choice(place == current),
        Some(wrap(VolumeMessage::Place(place))),
    )
}

/// Recessed, like a code block: text that is a file's own, not the app's.
fn recessed<'a>(content: UiElement<'a>) -> UiElement<'a> {
    container(content)
        .padding(theme::SPACE_2 + 2.)
        .width(Length::Fill)
        .style(widgets::well)
        .into()
}

fn detail_pane(state: &State, ty: Typography) -> UiElement<'_> {
    let Some(entry) = state
        .selected_entry()
        .filter(|entry| entry.kind == EntryKind::File)
    else {
        return ty.empty_state(
            Icon::Volume,
            "Take a closer look",
            "Select a file to inspect its contents and capture details.",
        );
    };
    let plate = ty.size(40.);
    let mut pane = column![
        row![
            container(visuals::icon(
                Icon::File,
                theme::TEXT_SECONDARY,
                plate * 0.5
            ))
            .center(plate)
            .style(widgets::well),
            column![
                ty.data(entry.name.as_str(), theme::TEXT)
                    .size(ty.size(theme::SIZE_MD))
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                ty.mono(size(entry.len), theme::TEXT_MUTED),
            ]
            .spacing(2)
            .width(Length::Fill),
        ]
        .spacing(theme::SPACE_2 + 4.)
        .align_y(alignment::Vertical::Center),
    ]
    .spacing(theme::SPACE_2 + 2.);
    match state.summary.as_ref().map(|(_, summary)| summary) {
        None => {
            pane = pane.push(
                row![
                    motion::beacon(Beat::Spin, theme::TEXT_SECONDARY, ty.size(16.)),
                    ty.body("Reading…", theme::SIZE_SM, theme::TEXT_MUTED),
                ]
                .spacing(theme::SPACE_2)
                .align_y(alignment::Vertical::Center),
            )
        }
        Some(Err(error)) => {
            pane = pane.push(kit::chip(ty, Icon::Alert, "unreadable", theme::ERROR));
            pane = pane.push(
                ty.data(error.as_str(), theme::ERROR)
                    .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            );
        }
        Some(Ok(Summary::Image {
            width,
            height,
            capture,
            text,
        })) => {
            pane = pane.push(kit::chip(
                ty,
                Icon::Window,
                format!("PNG · {width}×{height}"),
                theme::TEXT_SECONDARY,
            ));
            if let Some(capture) = capture {
                let mut facts = column![kit::heading(ty, Icon::Info, "Kore capture")]
                    .spacing(theme::SPACE_1 + 2.);
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
                        facts = facts.push(kit::pair(ty, key, value.as_str()));
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
                    facts = facts.push(kit::pair(ty, &key["kore:".len()..], value.as_str()));
                }
                pane = pane.push(recessed(
                    widgets::scroll(facts).height(Length::Shrink).into(),
                ));
            } else if !text.is_empty() {
                let mut facts = column![].spacing(theme::SPACE_1 + 2.);
                for (key, value) in text.iter().take(8) {
                    facts = facts.push(kit::pair(ty, key.as_str(), value.as_str()));
                }
                pane = pane.push(recessed(facts.into()));
            }
        }
        Some(Ok(Summary::Text {
            text,
            lines,
            truncated,
        })) => {
            pane = pane.push(kit::chip(
                ty,
                Icon::File,
                format!(
                    "text · {lines} lines{}",
                    if *truncated { " (first part)" } else { "" }
                ),
                theme::TEXT_SECONDARY,
            ));
            pane = pane.push(
                container(
                    widgets::scroll(ty.mono(text.as_str(), theme::TEXT)).height(Length::Fill),
                )
                .padding(theme::SPACE_2 + 2.)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(widgets::well),
            );
        }
        Some(Ok(Summary::Binary { len, head })) => {
            pane = pane.push(kit::chip(
                ty,
                Icon::Hash,
                format!("binary · {}", size(*len)),
                theme::TEXT_SECONDARY,
            ));
            pane = pane.push(recessed(ty.mono(head.as_str(), theme::TEXT_MUTED).into()));
        }
    }
    let can_edit = !state.busy && !state.read_only && matches!(state.place, Place::Home(_));
    let actions = row![
        kit::icon_label(
            ty,
            Icon::Window,
            "Open",
            widgets::Button::Primary,
            (!state.busy).then(|| wrap(VolumeMessage::View)),
        ),
        kit::icon_label(
            ty,
            Icon::Download,
            "Save",
            widgets::Button::Secondary,
            (!state.busy).then(|| wrap(VolumeMessage::Export)),
        ),
        kit::icon_label(
            ty,
            Icon::Trash,
            if state.confirm_remove {
                "Confirm remove"
            } else {
                "Remove"
            },
            widgets::Button::Danger,
            can_edit.then(|| wrap(VolumeMessage::Remove)),
        ),
    ]
    .spacing(theme::SPACE_2)
    .wrap();
    column![pane, space::vertical(), actions]
        .spacing(theme::SPACE_2)
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_starts_at_root_and_failure_releases_ui_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Disk::create_image(dir.path().join("disk.raw"), 16 << 20).unwrap();
        let (sender, receiver) = mpsc::channel();
        let mut state = State::new(sender);
        state.path = vec!["missing".into()];
        state.open_path(disk.path().to_owned(), "chosen disk".into());
        assert!(matches!(receiver.recv().unwrap(), Request::Open { .. }));
        assert_eq!(state.here(), Some(VolPath::root()));
        assert!(state.holds(disk.path()));
        state.reply(Reply::OpenFailed("Authorization cancelled".into()));
        assert!(!state.busy && !state.holds(disk.path()));
        assert!(state.label.is_none() && state.listing.is_none());
    }

    #[test]
    fn cancelling_picker_keeps_session_and_read_only_blocks_keyboard_writes() {
        let (sender, receiver) = mpsc::channel();
        let mut state = State::new(sender);
        state.label = Some("existing disk".into());
        state.reply(Reply::ImagePicked(None));
        assert_eq!(state.label.as_deref(), Some("existing disk"));
        assert!(receiver.try_recv().is_err());
        state.read_only = true;
        state.place = Place::Home(0);
        state.import_input = "/tmp/source".into();
        state.update(VolumeMessage::Import, |_| None);
        state.update(VolumeMessage::Remove, |_| None);
        assert!(!state.confirm_remove);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn identification_is_available_in_the_chooser_and_cancelled_auth_keeps_choices() {
        let (sender, receiver) = mpsc::channel();
        let mut state = State::new(sender);
        state.disk_choices = Some(vkore_volume::PhysicalDisks::default());
        state.update(VolumeMessage::IdentifyDisks, |_| None);
        assert!(state.busy);
        assert!(matches!(
            receiver.recv().unwrap(),
            Request::IdentifyDisks(_)
        ));
        state.reply(Reply::Disks(Err("Authorization cancelled".into())));
        assert!(!state.busy);
        assert!(state.disk_choices.is_some());
        assert_eq!(state.status, "Authorization cancelled");
        state.update(VolumeMessage::CancelDisk, |_| None);
        assert!(state.disk_choices.is_none());
    }

    #[test]
    fn an_opening_disk_cannot_queue_a_second_open() {
        let (sender, receiver) = mpsc::channel();
        let mut state = State::new(sender);
        state.busy = true;
        state.update(VolumeMessage::Open, |_| None);
        state.update(VolumeMessage::PickImage, |_| None);
        assert!(receiver.try_recv().is_err());
    }
}
