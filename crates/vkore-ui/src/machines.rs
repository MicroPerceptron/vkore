//! Machines: create them, boot them, follow their boots, stop them.
//!
//! Each running machine has a worker thread that owns its [`Session`]: it
//! follows the boot, relays every event to the view, and stops the machine
//! when asked (the power button, then QEMU's end) or when the window closes.

use crate::typography::Typography;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, space, text_input};
use iced_winit::winit::event_loop::EventLoopProxy;

use vkore_vm::{Display, Stage};
use vkore_volume::workspace::DEFAULT_EXCLUDES;
use vkored::{
    Event, MachineConfig, MachineStore, State as VkoreState, WorkspaceConfig, Writeback,
    run::Session,
};

use crate::{
    Message, UiElement, UserEvent, fonts,
    kit::{self, Status},
    motion::{Beat, Steps},
    theme,
    visuals::Icon,
    widgets,
};

mod tiles;

/// What a machine's worker reports.
#[derive(Debug)]
pub enum Update {
    Catalog(Result<vkored::catalog::Catalog, String>),
    Line(String),
    Console(Result<vkored::console::Console, String>),
    Paste {
        epoch: u64,
        result: Result<String, String>,
    },
    Stage(Stage),
    Finished(Result<String, String>),
    ImportProgress(vkored::images::Progress),
    Created(Result<(), String>),
    Hardware(Result<Box<vkore_vm::hardware::HostResources>, String>),
    HardwareSaved(Result<(), String>),
    HardwarePrepared(Result<Box<vkore_vm::hardware::HostResources>, String>),
}

/// A machine the view started.
pub struct Running {
    stop: mpsc::Sender<()>,
    pub stage: Option<Stage>,
    /// What the boot reported, one escaped line each, as the pane shows it.
    pub log: String,
    lines: usize,
    pub started: Instant,
    pub console: Option<vkored::console::Console>,
    console_error: String,
}

impl Running {
    /// The most lines the log keeps.
    const LINES: usize = 400;

    pub(crate) fn new(stop: mpsc::Sender<()>) -> Self {
        Self {
            stop,
            stage: None,
            log: String::new(),
            lines: 0,
            started: Instant::now(),
            console: None,
            console_error: String::new(),
        }
    }

    /// Appends `line`, set for the pane once, here rather than every frame.
    fn push(&mut self, line: &str) {
        use std::fmt::Write as _;
        if self.lines == Self::LINES {
            // Drop the oldest quarter at once, not a line per arrival.
            let cut = self
                .log
                .match_indices('\n')
                .nth(Self::LINES / 4 - 1)
                .map_or(self.log.len(), |(index, _)| index + 1);
            self.log.drain(..cut);
            self.lines -= Self::LINES / 4;
        }
        if !self.log.is_empty() {
            self.log.push('\n');
        }
        let at = self.started.elapsed().as_secs_f32();
        let line = fonts::shown(fonts::MONO, line, fonts::Breaks::Escape);
        let _ = write!(self.log, "{at:7.1}s  {line}");
        self.lines += 1;
    }
}

/// A run that has ended: what it came to, and what it reported on the way.
pub struct Finished {
    pub summary: String,
    pub log: String,
    pub console: Option<vkored::console::Output>,
}

/// Machines, and the form that creates one.
pub struct State {
    pub account: vkored::account::Account,
    pub state_root: PathBuf,
    pub host: Result<vkore_vm::hardware::HostResources, String>,
    pub form_hardware: crate::hardware::Draft,
    pub edit_hardware: Option<(String, crate::hardware::Draft)>,
    hardware_busy: bool,
    pub configure_new: bool,
    pub creating: bool,
    pub configs: Vec<MachineConfig>,
    pub selected: Option<String>,
    pub plugins: crate::plugins::State,
    layouts: BTreeMap<String, tiles::Workspace<Tile>>,
    pub console_focus: Option<String>,
    console_focus_epoch: u64,
    paste_pending: bool,
    pub running: BTreeMap<String, Running>,
    /// How each machine's last run from this window ended, and its log.
    pub finished: BTreeMap<String, Finished>,
    pub form_name: String,
    pub official_images: Vec<vkored::catalog::Image>,
    pub selected_image: Option<String>,
    pub image_menu_open: bool,
    pub catalog_error: Option<String>,
    catalog_cancel: Option<Arc<AtomicBool>>,
    catalog_worker: Option<thread::JoinHandle<()>>,
    pub import_progress: Option<vkored::images::Progress>,
    importing: Option<Arc<AtomicBool>>,
    import_worker: Option<thread::JoinHandle<()>>,
    pub form_workspace: String,
    pub status: String,
    pub closing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Console,
    Overview,
    BootActivity,
    Plugins,
    #[cfg(feature = "browser-preview")]
    Plugin(crate::plugins::TileId),
}

impl Tile {
    #[cfg(test)]
    const BUILTIN: [Self; 4] = [
        Self::Console,
        Self::Overview,
        Self::BootActivity,
        Self::Plugins,
    ];

    fn info(self) -> tiles::TileInfo {
        let (title, icon, closable) = match self {
            Self::Console => ("Console", Icon::Terminal, false),
            Self::Overview => ("Overview", Icon::Dashboard, false),
            Self::BootActivity => ("Boot activity", Icon::Activity, false),
            Self::Plugins => ("Plugins", Icon::Plugin, false),
            #[cfg(feature = "browser-preview")]
            Self::Plugin(_) => ("Plugin", Icon::Plugin, true),
        };
        tiles::TileInfo {
            title,
            icon,
            closable,
        }
    }
}

fn workspace() -> tiles::Workspace<Tile> {
    use iced_widget::pane_grid::Axis;
    use tiles::Layout;

    tiles::Workspace::new(Layout::split(
        Axis::Vertical,
        0.57,
        Layout::split(
            Axis::Horizontal,
            0.66,
            Layout::tile(Tile::Console),
            Layout::tile(Tile::Plugins),
        ),
        Layout::split(
            Axis::Horizontal,
            0.54,
            Layout::tile(Tile::Overview),
            Layout::tile(Tile::BootActivity),
        ),
    ))
}

#[derive(Debug, Clone)]
pub enum MachineMessage {
    OpenLibrary,
    OpenCreate,
    Select(String),
    TileLayout(String, tiles::Change<Tile>),
    Hardware(bool, crate::hardware::Change),
    EditHardware(String),
    ConfigureNew(bool),
    SaveHardware,
    CancelHardware,
    RefreshHardware,
    FinishHardware,
    Start(String, bool),
    Stop(String),
    ConsoleFocus(Option<String>),
    ConsolePaste(String),
    ConsoleInput(String, Vec<u8>),
    ConsoleResize(String, u16, u16),
    FormName(String),
    SelectImage(String),
    ToggleImageMenu,
    RefreshImages,
    CancelImport,
    FormWorkspace(String),
    Create,
    Refresh,
}

impl State {
    pub fn new(state_root: PathBuf) -> Self {
        let host = vkore_vm::hardware::HostResources::probe(&state_root);
        let form_hardware = crate::hardware::Draft::new(host.as_ref().ok(), Default::default());
        let mut state = Self {
            account: Default::default(),
            host,
            form_hardware,
            edit_hardware: None,
            hardware_busy: false,
            configure_new: false,
            creating: false,
            state_root,
            configs: Vec::new(),
            selected: None,
            plugins: Default::default(),
            layouts: BTreeMap::new(),
            console_focus: None,
            console_focus_epoch: 0,
            paste_pending: false,
            running: BTreeMap::new(),
            finished: BTreeMap::new(),
            form_name: String::new(),
            official_images: Vec::new(),
            selected_image: None,
            image_menu_open: false,
            catalog_error: None,
            catalog_cancel: None,
            catalog_worker: None,
            import_progress: None,
            importing: None,
            import_worker: None,
            form_workspace: String::new(),
            status: String::new(),
            closing: false,
        };
        state.refresh();
        state
    }

    fn store(&self) -> Result<MachineStore, String> {
        VkoreState::open(&self.state_root)
            .map(MachineStore::new)
            .map_err(|error| format!("{}: {error}", self.state_root.display()))
    }

    pub fn refresh(&mut self) {
        match self
            .store()
            .and_then(|store| store.list().map_err(|error| error.to_string()))
        {
            Ok(configs) => {
                self.layouts
                    .retain(|name, _| configs.iter().any(|config| &config.name == name));
                self.plugins
                    .retain_machines(|name| configs.iter().any(|config| config.name == name));
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|name| !configs.iter().any(|config| &config.name == name))
                {
                    self.unfocus_console();
                    self.selected = None;
                    self.edit_hardware = None;
                }
                self.configs = configs;
            }
            Err(error) => self.status = error,
        }
    }

    /// Machine `name`'s disk, for the volume browser.
    pub fn disk_of(&self, name: &str) -> Option<PathBuf> {
        let store = self.store().ok()?;
        store.get(name).ok()?;
        Some(store.state().machine(name).disk)
    }

    pub fn names(&self) -> Vec<String> {
        self.configs
            .iter()
            .map(|config| config.name.clone())
            .collect()
    }

    pub fn update_plugin(
        &mut self,
        action: crate::plugins::Action,
        proxy: &EventLoopProxy<UserEvent>,
    ) {
        #[cfg(feature = "browser-preview")]
        let previous = self.plugins.opened_tile();
        self.plugins.update(action, proxy);
        #[cfg(feature = "browser-preview")]
        self.place_plugin_tile(previous);
    }

    pub fn plugin_reply(&mut self, reply: crate::plugins::Reply) {
        #[cfg(feature = "browser-preview")]
        let previous = self.plugins.opened_tile();
        self.plugins.reply(reply);
        #[cfg(feature = "browser-preview")]
        self.place_plugin_tile(previous);
    }

    #[cfg(feature = "browser-preview")]
    fn place_plugin_tile(&mut self, previous: Option<(String, crate::plugins::TileId)>) {
        let next = self.plugins.opened_tile();
        if previous == next {
            return;
        }
        if let Some((name, tile)) = previous
            && let Some(layout) = self.layouts.get_mut(&name)
        {
            layout.change(tiles::Change::Close(Tile::Plugin(tile)));
        }
        if let Some((name, tile)) = next {
            self.unfocus_console();
            self.layouts
                .entry(name)
                .or_insert_with(workspace)
                .append(Tile::Plugin(tile));
        }
    }

    pub fn update(&mut self, message: MachineMessage, proxy: &EventLoopProxy<UserEvent>) {
        if self.closing {
            return;
        }
        // Freeze the draft and its target until completion. Stop remains usable.
        if self.hardware_busy
            && !matches!(
                message,
                MachineMessage::Stop(_) | MachineMessage::CancelImport
            )
        {
            return;
        }
        match message {
            MachineMessage::ConsoleFocus(name) => {
                self.unfocus_console();
                self.console_focus = name;
            }
            MachineMessage::ConsolePaste(name) => {
                if !self.paste_pending && self.console_focus.as_ref() == Some(&name) {
                    self.paste_pending = true;
                    let epoch = self.console_focus_epoch;
                    let proxy = proxy.clone();
                    iced_winit::clipboard::Clipboard::new().read(
                        iced_core::clipboard::Kind::Text,
                        move |result| {
                            let result = match result {
                                Ok(iced_core::clipboard::Content::Text(text))
                                    if text.len() <= vkored::console::MAX_PENDING_BYTES =>
                                {
                                    Ok(text)
                                }
                                Ok(_) => Err("Paste plain text of at most 16 KiB".into()),
                                Err(_) => Err("Could not read the clipboard".into()),
                            };
                            let _ = proxy.send_event(UserEvent::Machine(
                                name,
                                Update::Paste { epoch, result },
                            ));
                        },
                    );
                }
            }
            MachineMessage::ConsoleResize(name, rows, cols) => {
                if self.selected.as_ref() == Some(&name)
                    && let Some(console) = self
                        .running
                        .get_mut(&name)
                        .and_then(|run| run.console.as_mut())
                {
                    console.resize(rows, cols);
                }
            }
            MachineMessage::ConsoleInput(name, bytes) => {
                if self.console_focus.as_ref() == Some(&name)
                    && self.selected.as_ref() == Some(&name)
                    && let Some(running) = self.running.get_mut(&name)
                    && let Some(console) = &mut running.console
                {
                    match console.send_input(&bytes) {
                        Ok(()) => running.console_error.clear(),
                        Err(error) => running.console_error = error.into(),
                    }
                }
            }
            MachineMessage::OpenLibrary | MachineMessage::OpenCreate => {
                self.unfocus_console();
                self.configure_new = false;
                self.creating = matches!(message, MachineMessage::OpenCreate);
                self.selected = None;
                self.edit_hardware = None;
            }
            MachineMessage::Select(name) => {
                if self.configs.iter().any(|config| config.name == name) {
                    self.unfocus_console();
                    self.configure_new = false;
                    self.creating = false;
                    self.selected = Some(name);
                    self.edit_hardware = None;
                }
            }
            MachineMessage::TileLayout(name, change) => {
                if self.selected.as_ref() == Some(&name) && self.edit_hardware.is_none() {
                    // A hidden or replaced terminal must not retain keyboard input.
                    if !matches!(change, tiles::Change::Resize(_)) {
                        self.unfocus_console();
                    }
                    if let tiles::Change::Close(tile) = change {
                        if !tile.info().closable {
                            return;
                        }
                        #[cfg(feature = "browser-preview")]
                        if let Tile::Plugin(id) = tile {
                            self.plugins.close_tile(&name, id);
                        }
                    }
                    self.layouts
                        .entry(name)
                        .or_insert_with(workspace)
                        .change(change);
                }
            }
            MachineMessage::Hardware(editing, change) => {
                if !self.hardware_busy
                    && self.importing.is_none()
                    && let Ok(host) = &self.host
                {
                    if editing {
                        if let Some((_, draft)) = &mut self.edit_hardware {
                            draft.change(change, host);
                        }
                    } else {
                        self.form_hardware.change(change, host);
                    }
                }
            }
            MachineMessage::ConfigureNew(open) => {
                self.configure_new = open;
                self.edit_hardware = None;
            }
            MachineMessage::EditHardware(name) => {
                self.configure_new = false;
                if !self.running.contains_key(&name)
                    && let Some(config) = self.configs.iter().find(|c| c.name == name)
                {
                    let fixed = self
                        .store()
                        .is_ok_and(|s| s.state().machine(&name).disk.exists());
                    self.edit_hardware =
                        Some((name, crate::hardware::Draft::from_config(config, fixed)));
                }
            }
            MachineMessage::CancelHardware => {
                if !self.hardware_busy {
                    self.edit_hardware = None;
                }
            }
            MachineMessage::SaveHardware => {
                if !self.hardware_busy
                    && let Some((name, draft)) = self.edit_hardware.clone()
                {
                    let root = self.state_root.clone();
                    let proxy = proxy.clone();
                    self.hardware_busy = true;
                    self.status = "Checking hardware settings and permissions…".into();
                    thread::spawn(move || {
                        let result = VkoreState::open(root)
                            .map(MachineStore::new)
                            .map_err(|e| e.to_string())
                            .and_then(|store| {
                                vkored::hardware::save_authorized(
                                    &store,
                                    &name,
                                    draft.cpus,
                                    draft.memory_mib,
                                    draft.disk_mib,
                                    draft.resources,
                                )
                            });
                        let _ = proxy
                            .send_event(UserEvent::Machine(name, Update::HardwareSaved(result)));
                    });
                }
            }
            MachineMessage::RefreshHardware => self.refresh_hardware(proxy.clone()),
            MachineMessage::FinishHardware => {
                if self.configure_new && self.importing.is_none() {
                    let mut config = MachineConfig::new("draft", "/pending.iso");
                    self.form_hardware.apply(&mut config);
                    let root = self.state_root.clone();
                    let proxy = proxy.clone();
                    self.hardware_busy = true;
                    self.status = "Checking hardware settings and permissions…".into();
                    thread::spawn(move || {
                        let result = vkored::hardware::prepare(&config, &root).map(Box::new);
                        let _ = proxy.send_event(UserEvent::Machine(
                            String::new(),
                            Update::HardwarePrepared(result),
                        ));
                    });
                }
            }
            MachineMessage::Refresh => self.refresh(),
            MachineMessage::RefreshImages => self.refresh_images(proxy.clone()),
            MachineMessage::FormName(value) => self.form_name = value,
            MachineMessage::SelectImage(id) => {
                if self.importing.is_none()
                    && self.catalog_cancel.is_none()
                    && self.official_images.iter().any(|image| image.id == id)
                {
                    if let Some(image) = self.official_images.iter().find(|i| i.id == id) {
                        self.form_hardware =
                            crate::hardware::Draft::new(self.host.as_ref().ok(), image.minimum);
                    }
                    self.selected_image = Some(id);
                    self.image_menu_open = false;
                }
            }
            MachineMessage::ToggleImageMenu => {
                if self.importing.is_none()
                    && self.catalog_cancel.is_none()
                    && !self.official_images.is_empty()
                {
                    self.image_menu_open = !self.image_menu_open;
                }
            }
            MachineMessage::FormWorkspace(value) => self.form_workspace = value,
            MachineMessage::Create => self.create(proxy.clone()),
            MachineMessage::CancelImport => {
                if let Some(cancel) = &self.importing {
                    cancel.store(true, Ordering::Relaxed);
                }
                self.status = "Cancelling image import".into();
            }
            MachineMessage::Start(name, window) => self.start(name, window, proxy.clone()),
            MachineMessage::Stop(name) => {
                if let Some(running) = self.running.get(&name) {
                    let _ = running.stop.send(());
                    self.status = format!("{name}: stopping");
                }
            }
        }
    }

    pub fn refresh_images(&mut self, proxy: EventLoopProxy<UserEvent>) {
        if self.catalog_cancel.is_some() || self.importing.is_some() || self.closing {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let account = self.account.clone();
        match thread::Builder::new()
            .name("vkore-image-catalog".into())
            .spawn(move || {
                let result = vkored::catalog::fetch_with_account(&signal, &account);
                let _ =
                    proxy.send_event(UserEvent::Machine(String::new(), Update::Catalog(result)));
            }) {
            Ok(worker) => {
                self.catalog_worker = Some(worker);
                self.catalog_cancel = Some(cancel);
                self.catalog_error = None;
                self.image_menu_open = false;
            }
            Err(error) => self.catalog_error = Some(error.to_string()),
        }
    }

    fn refresh_hardware(&mut self, proxy: EventLoopProxy<UserEvent>) {
        if self.hardware_busy {
            return;
        }
        self.hardware_busy = true;
        self.status = "Refreshing host resources…".into();
        let root = self.state_root.clone();
        thread::spawn(move || {
            let result = vkore_vm::hardware::HostResources::probe(&root).map(Box::new);
            let _ = proxy.send_event(UserEvent::Machine(String::new(), Update::Hardware(result)));
        });
    }

    fn create(&mut self, proxy: EventLoopProxy<UserEvent>) {
        if self.importing.is_some() || self.catalog_cancel.is_some() {
            return;
        }
        self.image_menu_open = false;
        let account = self.account.clone();
        let Some(image_id) = self
            .selected_image
            .clone()
            .filter(|id| self.official_images.iter().any(|image| &image.id == id))
        else {
            self.status = "Select an available official image first".into();
            return;
        };
        let selected_sha256 = self
            .official_images
            .iter()
            .find(|image| image.id == image_id)
            .expect("selection checked")
            .sha256()
            .to_owned();
        let absolute = |text: &str| {
            std::fs::canonicalize(text.trim()).map_err(|error| format!("{}: {error}", text.trim()))
        };
        let created = (|| {
            let mut config = MachineConfig::new(self.form_name.trim(), "/pending.iso");
            self.form_hardware.apply(&mut config);
            let host = self.host.as_ref().map_err(Clone::clone)?;
            vkored::hardware::validate_host(&config, host)?;
            if config.resources.reserve_storage && config.disk_mib > host.storage_available_mib {
                return Err("not enough free storage for this reservation".into());
            }
            if !self.form_workspace.trim().is_empty() {
                let root = absolute(&self.form_workspace)?;
                let name = root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or("the workspace needs a directory name")?
                    .to_owned();
                config.workspace = Some(WorkspaceConfig {
                    root,
                    name,
                    face: 0,
                    exclude: DEFAULT_EXCLUDES
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect(),
                    writeback: Writeback::Review,
                    max_files: 256,
                    max_bytes: 64 << 20,
                });
            }
            Ok::<_, String>(config)
        })();
        match created {
            Ok(config) => {
                let root = self.state_root.clone();
                let cancelled = Arc::new(AtomicBool::new(false));
                let signal = cancelled.clone();
                let result = thread::Builder::new()
                    .name("vkore-image-import".into())
                    .spawn(move || {
                        let name = config.name.clone();
                        let send = |update| {
                            let _ = proxy.send_event(UserEvent::Machine(name.clone(), update));
                        };
                        let outcome = VkoreState::open(root)
                            .map(|state| MachineStore::new(state).with_account(account))
                            .map_err(|e| e.to_string())
                            .and_then(|store| {
                                vkored::images::create_official(
                                    &store,
                                    config,
                                    &image_id,
                                    Some(&selected_sha256),
                                    &signal,
                                    |progress| send(Update::ImportProgress(progress)),
                                )
                                .map(|_| ())
                            });
                        send(Update::Created(outcome));
                    });
                match result {
                    Ok(worker) => {
                        self.import_progress = None;
                        self.import_worker = Some(worker);
                        self.importing = Some(cancelled);
                        self.status = "Importing and checking image…".into();
                    }
                    Err(error) => self.status = error.to_string(),
                }
            }
            Err(error) => self.status = error,
        }
    }

    fn start(&mut self, name: String, window: bool, proxy: EventLoopProxy<UserEvent>) {
        if self.closing || self.running.contains_key(&name) {
            return;
        }
        let Some(qemu) = vkored::run::default_qemu() else {
            self.status = "qemu-system-x86_64 is not on PATH".into();
            return;
        };
        let (stop, stopping) = mpsc::channel::<()>();
        let root = self.state_root.clone();
        let worker_name = name.clone();
        let spawned = thread::Builder::new()
            .name(format!("vkore-ui-machine-{name}"))
            .spawn(move || {
                let send = |update: Update| {
                    let _ = proxy.send_event(UserEvent::Machine(worker_name.clone(), update));
                };
                let outcome = run_machine(&root, &worker_name, &qemu, window, &stopping, &send);
                send(Update::Finished(outcome));
            });
        match spawned {
            Ok(_) => {
                self.finished.remove(&name);
                self.running.insert(name.clone(), Running::new(stop));
                self.status = format!("{name}: starting");
            }
            Err(error) => self.status = format!("{name}: {error}"),
        }
    }

    pub fn machine_update(&mut self, name: String, update: Update) {
        match update {
            Update::Paste { epoch, result } => {
                self.paste_pending = false;
                if !self.closing
                    && !self.hardware_busy
                    && epoch == self.console_focus_epoch
                    && self.console_focus.as_ref() == Some(&name)
                    && self.selected.as_ref() == Some(&name)
                    && let Some(running) = self.running.get_mut(&name)
                    && let Some(console) = &mut running.console
                {
                    let result =
                        result.and_then(|text| console.send_paste(&text).map_err(str::to_owned));
                    running.console_error = result.err().unwrap_or_default();
                }
            }
            Update::Console(result) => {
                if self.console_focus.as_ref() == Some(&name) {
                    self.unfocus_console();
                }
                if let Some(running) = self.running.get_mut(&name) {
                    match result {
                        Ok(console) => running.console = Some(console),
                        Err(error) => {
                            running.console_error = format!("Console connection failed: {error}")
                        }
                    }
                }
            }
            Update::Catalog(result) => {
                self.catalog_cancel = None;
                if let Some(worker) = self.catalog_worker.take() {
                    let _ = worker.join();
                }
                match result {
                    Ok(catalog) => {
                        let previous = self
                            .official_images
                            .iter()
                            .find(|image| Some(&image.id) == self.selected_image.as_ref())
                            .cloned();
                        let had_selection = self.selected_image.is_some();
                        self.official_images = catalog.images().to_vec();
                        self.catalog_error = None;
                        if had_selection
                            && !previous.as_ref().is_some_and(|old| {
                                self.official_images.iter().any(|new| new == old)
                            })
                        {
                            self.selected_image = None;
                            self.catalog_error = Some("Your selected image changed or is no longer available. Select an image again.".into());
                        }
                    }
                    Err(error) => self.catalog_error = Some(error),
                }
            }
            Update::Hardware(result) => {
                self.hardware_busy = false;
                match result {
                    Ok(host) => {
                        self.host = Ok(*host);
                        self.status = "Host resources refreshed".into();
                    }
                    Err(error) => self.status = error,
                }
            }
            Update::HardwarePrepared(result) => {
                self.hardware_busy = false;
                match result {
                    Ok(host) => {
                        self.host = Ok(*host);
                        self.configure_new = false;
                        self.status = "Hardware settings ready".into();
                    }
                    Err(error) => self.status = error,
                }
            }
            Update::HardwareSaved(result) => {
                self.hardware_busy = false;
                match result {
                    Ok(()) => {
                        self.edit_hardware = None;
                        self.refresh();
                        self.status = "Hardware settings saved".into();
                    }
                    Err(error) => self.status = error,
                }
            }
            Update::ImportProgress(progress) => {
                self.import_progress = Some(progress);
                self.status = format!(
                    "{}: {} MiB{}",
                    progress.phase.label(),
                    progress.bytes >> 20,
                    progress
                        .total
                        .map(|n| format!(" / {} MiB", n >> 20))
                        .unwrap_or_default()
                );
            }
            Update::Created(result) => {
                self.import_progress = None;
                self.importing = None;
                if let Some(worker) = self.import_worker.take() {
                    let _ = worker.join();
                }
                match result {
                    Ok(()) => {
                        self.configure_new = false;
                        self.creating = false;
                        self.status = format!("Created {name}");
                        self.selected = Some(name);
                        self.refresh();
                    }
                    Err(error) => self.status = error,
                }
            }
            Update::Line(line) => {
                if let Some(running) = self.running.get_mut(&name) {
                    running.push(&line);
                }
            }
            Update::Stage(stage) => {
                if let Some(running) = self.running.get_mut(&name) {
                    running.stage = Some(stage);
                    if !self.closing {
                        self.status = format!("{name}: {}", stage_status(Some(stage)).label);
                    }
                }
            }
            Update::Finished(outcome) => {
                if self.console_focus.as_ref() == Some(&name) {
                    self.unfocus_console();
                }
                // The log stays: how a run ended is the evidence.
                let running = self.running.remove(&name);
                let console = running
                    .as_ref()
                    .and_then(|run| run.console.as_ref())
                    .map(|console| console.output().clone());
                let log = running.map(|run| run.log).unwrap_or_default();
                let summary = match outcome {
                    Ok(summary) => summary,
                    Err(error) => format!("failed: {error}"),
                };
                self.status = format!("{name}: {summary}");
                self.finished.insert(
                    name,
                    Finished {
                        summary,
                        log,
                        console,
                    },
                );
                self.refresh();
            }
        }
    }

    /// Ask every running machine to stop (the window is closing).
    /// Return true only when there is nothing to drain, or the user explicitly
    /// closes again. A Finished update means stop *and collection* have ended.
    pub fn request_close(&mut self) -> bool {
        let leave = self.closing || self.running.is_empty();
        self.closing = true;
        self.stop_all();
        self.status =
            "Stopping machines and collecting changes; close again to leave immediately".into();
        leave
    }

    /// Invalidate pending clipboard reads whenever keyboard focus changes.
    pub fn unfocus_console(&mut self) {
        self.console_focus = None;
        self.console_focus_epoch = self.console_focus_epoch.wrapping_add(1);
    }

    /// Poll existing nonblocking shell sockets; no per-byte worker messages.
    pub fn poll_consoles(&mut self) -> bool {
        let mut changed = false;
        for running in self.running.values_mut() {
            if let Some(console) = &mut running.console {
                changed |= console.poll();
            }
        }
        changed
    }

    pub fn has_live_console(&self) -> bool {
        self.running
            .values()
            .any(|run| run.console.as_ref().is_some_and(|c| c.connected()))
    }

    pub fn stop_all(&mut self) {
        if let Some(cancel) = &self.catalog_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(cancel) = &self.importing {
            cancel.store(true, Ordering::Relaxed);
        }
        for running in self.running.values() {
            let _ = running.stop.send(());
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.catalog_cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.catalog_worker.take() {
            let _ = worker.join();
        }
        // Let the import's temporary file cleanup run before process exit.
        // Socket waits are bounded by the image client's read timeout.
        if let Some(cancel) = &self.importing {
            cancel.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = self.import_worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_machine(
    root: &std::path::Path,
    name: &str,
    qemu: &std::path::Path,
    window: bool,
    stopping: &mpsc::Receiver<()>,
    send: &dyn Fn(Update),
) -> Result<String, String> {
    let store = VkoreState::open(root)
        .map(MachineStore::new)
        .map_err(|error| error.to_string())?;
    let config = store.get(name).map_err(|e| e.to_string())?;
    let host = vkore_vm::hardware::HostResources::probe(root)?;
    send(Update::Line(
        "Checking hardware settings and permissions…".into(),
    ));
    if !vkored::hardware::uses_guardian(&config) {
        vkored::hardware::authorize_configuration(&config, &host)?;
    } else {
        // The hardware guardian batches device, hugepage and memlock permissions.
        vkored::hardware::validate_host(&config, &host)?;
    }
    let mut sink = |event: &Event| match event {
        Event::Console { socket } => {
            send(Update::Line(describe(event)));
            send(Update::Console(
                vkored::console::Console::connect(socket).map_err(|e| e.to_string()),
            ));
        }
        Event::Stage { stage, .. } => {
            send(Update::Stage(*stage));
            send(Update::Line(format!("boot: {stage:?}")));
        }
        Event::Qemu { name, .. } if name == "RTC_CHANGE" => {}
        other => send(Update::Line(describe(other))),
    };
    let display = if window {
        Display::Window
    } else {
        Display::Headless
    };
    let mut session = Session::start(&store, name, qemu, display, &mut sink)
        .map_err(|error| error.to_string())?;
    loop {
        let running = session
            .poll(Duration::from_millis(250), &mut sink)
            .map_err(|error| error.to_string())?;
        if !running || stopping.try_recv().is_ok() {
            break;
        }
    }
    if session.machine().running() {
        session
            .stop(Duration::from_secs(10), &mut sink)
            .map_err(|error| error.to_string())?;
    }
    let changes = session
        .finish(&store, &mut sink)
        .map_err(|error| error.to_string())?;
    Ok(match changes {
        Some(changes) if !changes.is_empty() => format!(
            "stopped; the machine changed {} file(s) in its project (vkore machine apply {name})",
            changes.file_count()
        ),
        _ => "stopped".into(),
    })
}

fn describe(event: &Event) -> String {
    match event {
        Event::Preflight(preflight) => format!(
            "preflight: sha256 {} · {:?} on {:?}",
            &preflight.medium.sha256[..16],
            preflight.vendor,
            preflight.accel
        ),
        Event::Staged { files, skipped, .. } => format!(
            "staged the project ({}, {skipped} left out)",
            widgets::count(*files, "file", "files")
        ),
        Event::Started { pid } => format!("QEMU started (pid {pid})"),
        Event::CpuPowerManagement => {
            "CPU power management: native MONITOR/MWAIT enabled on pinned CPUs".into()
        }
        Event::MemoryBacking { summary } => summary.clone(),
        Event::Console { socket } => {
            format!("console: the shell is on COM2 at {}", socket.display())
        }
        Event::Qemu { name, .. } => format!("qemu: {name}"),
        Event::Stopping => "stopping".into(),
        Event::Stopped { forced } => {
            if *forced {
                "stopped (QEMU ended)".into()
            } else {
                "stopped".into()
            }
        }
        Event::Exited { code } => format!("QEMU exited ({code:?})"),
        Event::Changes {
            created,
            modified,
            deleted,
            ..
        } => {
            format!("changes: {created} created, {modified} modified, {deleted} deleted")
        }
        Event::Applied(applied) => format!(
            "applied {} written, {} deleted",
            applied.written, applied.deleted
        ),
        Event::Warning { message } => format!("warning: {message}"),
        Event::Stage { stage, .. } => format!("boot: {stage:?}"),
    }
}

fn wrap(message: MachineMessage) -> Message {
    Message::Machine(message)
}

/// How far a boot has come, as a status: five stops from firmware to ready.
///
/// A boot in flight turns, a usable machine pulses (it is alive), and a
/// machine that stopped short holds still in the colour of how it stopped.
fn stage_status(stage: Option<Stage>) -> Status {
    const STOPS: u8 = 5;
    let along = |done, working, tone| {
        Some(Steps {
            total: STOPS,
            done,
            working,
            tone,
        })
    };
    match stage {
        None | Some(Stage::Firmware) => Status {
            beat: Beat::Spin,
            tone: theme::TEXT_SECONDARY,
            label: "starting",
            steps: along(0, true, theme::TEXT_SECONDARY),
        },
        Some(Stage::Kernel) => Status {
            beat: Beat::Spin,
            tone: theme::TEXT_SECONDARY,
            label: "kernel up",
            steps: along(1, true, theme::TEXT_SECONDARY),
        },
        Some(Stage::Services) => Status {
            beat: Beat::Spin,
            tone: theme::TEXT_SECONDARY,
            label: "services",
            steps: along(2, true, theme::TEXT_SECONDARY),
        },
        Some(Stage::Shell) => Status {
            beat: Beat::Pulse,
            tone: theme::SUCCESS,
            label: "shell ready",
            steps: along(4, true, theme::SUCCESS),
        },
        Some(Stage::Ready) => Status {
            beat: Beat::Pulse,
            tone: theme::SUCCESS,
            label: "ready",
            steps: along(STOPS, false, theme::SUCCESS),
        },
        Some(Stage::Degraded) => Status {
            beat: Beat::Solid,
            tone: theme::WARNING,
            label: "degraded",
            steps: along(STOPS - 1, false, theme::WARNING),
        },
        Some(Stage::Failed) => Status {
            beat: Beat::Solid,
            tone: theme::ERROR,
            label: "failed",
            steps: None,
        },
    }
}

/// A machine that is not running.
const STOPPED: Status = Status {
    beat: Beat::Hollow,
    tone: theme::TEXT_MUTED,
    label: "stopped",
    steps: None,
};

fn machine_status(running: Option<&Running>) -> Status {
    running.map_or(STOPPED, |run| stage_status(run.stage))
}

/// The machine library, creation flow, or selected instance.
pub fn view(state: &State, ty: Typography, appearance: iced_core::Theme) -> UiElement<'_> {
    let mut page = if state.creating || state.configure_new {
        creation(state, ty)
    } else if let Some(config) = state
        .configs
        .iter()
        .find(|config| Some(&config.name) == state.selected.as_ref())
    {
        instance(state, config, ty, &appearance)
    } else {
        library(state, ty)
    };
    if !state.status.is_empty() {
        page = page.push(
            ty.data(state.status.as_str(), theme::TEXT_MUTED)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        );
    }
    widgets::page_frame(page.spacing(theme::SPACE_4).height(Length::Fill))
}

type PageColumn<'a> = iced_widget::Column<'a, Message, iced_core::Theme, crate::AxUi>;

fn library(state: &State, ty: Typography) -> PageColumn<'_> {
    let mut list = column![kit::heading(ty, Icon::Machine, "Your machines")].spacing(10);
    if state.configs.is_empty() {
        list = list.push(ty.empty_state(
            Icon::Machine,
            "No machines yet",
            "Create a machine from an official Kore image.",
        ));
    }
    for config in &state.configs {
        let selected = state.selected.as_deref() == Some(config.name.as_str());
        let status = machine_status(state.running.get(&config.name));
        let plate = ty.size(40.);
        list = list.push(kit::lift(
            widgets::Button::Row(selected),
            true,
            button(
                row![
                    // The plate takes the machine's state, so a running one
                    // stands out of the list before a word is read.
                    container(crate::visuals::icon(
                        Icon::Machine,
                        if state.running.contains_key(&config.name) {
                            status.tone
                        } else {
                            theme::TEXT_MUTED
                        },
                        plate * 0.5,
                    ))
                    .center(plate)
                    .style(|appearance| theme::panel(
                        appearance,
                        theme::PANEL_FILE,
                        Some(theme::SEPARATOR),
                        theme::RADIUS_MD
                    )),
                    column![
                        ty.data(config.name.as_str(), theme::TEXT)
                            .size(ty.size(theme::SIZE_SM)),
                        row![
                            kit::figure(ty, Icon::Cpu, config.cpus.to_string()),
                            kit::figure(
                                ty,
                                Icon::Memory,
                                format!("{:.1} GiB", f64::from(config.memory_mib) / 1024.)
                            ),
                            kit::figure(
                                ty,
                                Icon::Volume,
                                format!("{:.0} GiB", config.disk_mib as f64 / 1024.)
                            ),
                        ]
                        .spacing(theme::SPACE_4),
                    ]
                    .spacing(theme::SPACE_1)
                    .width(Length::Fill),
                    kit::status_mark(ty, status),
                ]
                .spacing(theme::SPACE_4 - 2.)
                .align_y(alignment::Vertical::Center),
            )
            .width(Length::Fill)
            .padding([10, 12])
            .style(widgets::button_style_fading(widgets::Button::Row(selected)))
            .on_press(wrap(MachineMessage::Select(config.name.clone()))),
        ));
    }

    column![
        row![
            ty.page_header(Icon::Machine, "Machines", "Create or open a Kore machine."),
            space::horizontal(),
            action(
                ty,
                Icon::Plus,
                "New machine",
                widgets::Button::Primary,
                MachineMessage::OpenCreate
            ),
        ]
        .align_y(alignment::Vertical::Center)
        .spacing(theme::SPACE_2),
        widgets::scroll(
            container(list)
                .id("machine-library")
                .padding(theme::SPACE_4)
                .width(Length::Fill)
                .style(widgets::card),
        )
        .height(Length::Fill),
    ]
}

fn creation(state: &State, ty: Typography) -> PageColumn<'_> {
    let content = if state.configure_new {
        new_hardware(state, ty)
    } else {
        widgets::scroll(create_form(state, ty))
            .height(Length::Fill)
            .into()
    };
    column![
        back(ty),
        container(content)
            .id("machine-creation")
            .width(Length::Fill.max(800))
            .height(Length::Fill)
            .padding(theme::SPACE_4)
            .style(widgets::card),
    ]
}

fn create_form(state: &State, ty: Typography) -> PageColumn<'_> {
    let selected_image = state
        .official_images
        .iter()
        .find(|image| Some(&image.id) == state.selected_image.as_ref());
    let label = selected_image.map_or_else(
        || {
            if state.catalog_cancel.is_some() {
                "Loading official images…".to_owned()
            } else if state.catalog_error.is_some() && state.official_images.is_empty() {
                "Official images unavailable".to_owned()
            } else if state.official_images.is_empty() {
                "No compatible public images".to_owned()
            } else {
                "Select an official image".to_owned()
            }
        },
        |image| format!("{} ({})", image.name, image.version),
    );
    // Compose the dropdown from the same paragraph-backed controls as the
    // rest of the UI: Axiom's immediate-text path does not honor menu alignment.
    let mut images = column![
        button(
            row![
                ty.data(label, theme::TEXT).width(Length::Fill),
                crate::visuals::icon(
                    if state.image_menu_open {
                        Icon::ChevronUp
                    } else {
                        Icon::ChevronDown
                    },
                    theme::TEXT_MUTED,
                    ty.size(14.),
                ),
            ]
            .spacing(theme::SPACE_1)
            .align_y(alignment::Vertical::Center)
        )
        .width(Length::Fill)
        .padding([7, 10])
        .style(widgets::button_style(widgets::Button::Secondary))
        .on_press_maybe(
            (!state.official_images.is_empty()
                && state.importing.is_none()
                && state.catalog_cancel.is_none())
            .then(|| wrap(MachineMessage::ToggleImageMenu))
        )
    ]
    .spacing(theme::SPACE_1);
    if state.image_menu_open && state.importing.is_none() {
        let mut options = column![].spacing(theme::SPACE_1);
        for image in &state.official_images {
            options = options.push(
                button(ty.data(format!("{} ({})", image.name, image.version), theme::TEXT))
                    .width(Length::Fill)
                    .padding([7, 10])
                    .style(widgets::button_style(widgets::Button::Row(
                        Some(&image.id) == state.selected_image.as_ref(),
                    )))
                    .on_press(wrap(MachineMessage::SelectImage(image.id.clone()))),
            );
        }
        images = images.push(
            container(widgets::scroll(options).height(Length::Shrink.max(160)))
                .padding(4)
                .style(|appearance| {
                    theme::panel(
                        appearance,
                        theme::PANEL,
                        Some(theme::BORDER),
                        theme::RADIUS_MD,
                    )
                }),
        );
    }
    if let Some(image) = selected_image {
        let (profile, icon) = match image.profile {
            vkored::catalog::Profile::Inference => ("Headless inference", Icon::Terminal),
            vkored::catalog::Profile::Desktop => ("Desktop", Icon::Window),
        };
        images = images.push(
            row![
                kit::figure(
                    ty,
                    Icon::Download,
                    format!("{:.1} MiB", image.bytes() as f64 / (1024. * 1024.))
                ),
                kit::figure(ty, icon, profile),
            ]
            .spacing(theme::SPACE_4),
        );
    }
    if state.importing.is_some() {
        images = images.push(ty.data(state.status.as_str(), theme::TEXT_SECONDARY));
        if let Some(progress) = state.import_progress
            && let Some(total) = progress.total
        {
            images = images.push(
                iced_widget::progress_bar(
                    0.0..=1.0,
                    (progress.bytes as f64 / total.max(1) as f64) as f32,
                )
                .girth(4)
                .style(widgets::download_progress),
            );
        }
    }
    if state.importing.is_some() {
        images = images.push(kit::icon_label(
            ty,
            Icon::Close,
            "Cancel import",
            widgets::Button::Secondary,
            Some(wrap(MachineMessage::CancelImport)),
        ));
    }
    let create_action = if state.catalog_cancel.is_none()
        && selected_image.is_some()
        && vkored::config::valid_name(state.form_name.trim())
    {
        Some(wrap(MachineMessage::Create))
    } else {
        None
    };
    let mut header = column![
        ty.body("Create a machine", 18., theme::TEXT),
        ty.body("Start with a verified Kore image.", 13., theme::TEXT_MUTED),
    ]
    .spacing(12);
    if state.account.status().is_none() {
        header = header.push(crate::account::download_hint(ty));
    }
    let mut form = column![
        header,
        ty.label("Machine name", theme::TEXT_MUTED),
        text_input("e.g. inference", &state.form_name)
            .on_input_maybe(
                state
                    .importing
                    .is_none()
                    .then_some(|value| wrap(MachineMessage::FormName(value)))
            )
            .font(fonts::MONO)
            .padding([7, 10])
            .size(ty.size(theme::SIZE_SM))
            .style(widgets::field),
        row![
            ty.label("Official image", theme::TEXT_MUTED),
            space::horizontal(),
            kit::icon_button(
                ty,
                Icon::Refresh,
                "Refresh images",
                widgets::Button::Secondary,
                (state.catalog_cancel.is_none()
                    && state.importing.is_none()
                    && !state.hardware_busy)
                    .then_some(wrap(MachineMessage::RefreshImages))
            )
        ]
        .align_y(alignment::Vertical::Center),
        images,
        ty.data(
            state
                .catalog_error
                .as_deref()
                .unwrap_or(if state.catalog_cancel.is_some() {
                    "Checking dev.microperceptron.com…"
                } else if state.official_images.is_empty() {
                    "No public x86_64 Kore ISOs are listed. Refresh to check again."
                } else {
                    "The official image is verified before it is added."
                }),
            theme::TEXT_MUTED
        ),
        kit::heading(ty, Icon::Cpu, "Hardware"),
        row![
            kit::figure(ty, Icon::Cpu, state.form_hardware.cpus.to_string()),
            kit::figure(
                ty,
                Icon::Memory,
                format!(
                    "{:.1} GiB",
                    f64::from(state.form_hardware.memory_mib) / 1024.
                )
            ),
            kit::figure(
                ty,
                Icon::Volume,
                format!("{:.1} GiB", state.form_hardware.disk_mib as f64 / 1024.)
            ),
            kit::figure(
                ty,
                Icon::Network,
                state.form_hardware.resources.network.label().to_string()
            ),
        ]
        .spacing(theme::SPACE_4),
        kit::tip_text(
            ty,
            row![
                kit::figure(ty, Icon::Info, "Image minimum"),
                kit::figure(ty, Icon::Cpu, state.form_hardware.minimum.cpus.to_string()),
                kit::figure(
                    ty,
                    Icon::Memory,
                    format!("{} MiB", state.form_hardware.minimum.memory_mib)
                ),
                kit::figure(
                    ty,
                    Icon::Volume,
                    format!("{} MiB", state.form_hardware.minimum.disk_mib)
                ),
            ]
            .spacing(theme::SPACE_4),
            "The smallest machine this image will boot",
            iced_widget::tooltip::Position::Bottom,
        ),
        kit::icon_label(
            ty,
            Icon::Settings,
            "Configure hardware",
            widgets::Button::Secondary,
            state
                .importing
                .is_none()
                .then_some(wrap(MachineMessage::ConfigureNew(true))),
        ),
        ty.label("Project folder · optional", theme::TEXT_MUTED),
        text_input("/path/to/project", &state.form_workspace)
            .on_input_maybe(
                state
                    .importing
                    .is_none()
                    .then_some(|value| wrap(MachineMessage::FormWorkspace(value)))
            )
            .font(fonts::MONO)
            .padding([7, 10])
            .size(ty.size(theme::SIZE_SM))
            .style(widgets::field),
    ]
    .spacing(12);
    if state.importing.is_none() {
        form = form.push(kit::icon_label(
            ty,
            Icon::Plus,
            "Create machine",
            widgets::Button::Primary,
            create_action,
        ));
    }
    form
}

fn new_hardware(state: &State, ty: Typography) -> UiElement<'_> {
    widgets::scroll(column![
            ty.body("New machine hardware",23.,theme::TEXT),
            ty.body("Start with half your physical cores and memory. Adjust the allocation to suit this image.",13.,theme::TEXT_MUTED),
            crate::hardware::view(&state.form_hardware,&state.host,false,state.hardware_busy||state.importing.is_some(),ty),
            kit::icon_label(ty, Icon::Check, "Done", widgets::Button::Primary,
                (!state.hardware_busy && state.importing.is_none())
                    .then_some(wrap(MachineMessage::FinishHardware))),
        ].spacing(18)).height(Length::Fill).into()
}

fn edit_hardware<'a>(state: &'a State, config: &MachineConfig, ty: Typography) -> UiElement<'a> {
    let (_, draft) = state.edit_hardware.as_ref().unwrap();
    widgets::scroll(
        column![
            ty.data(format!("Configure {}", config.name), theme::TEXT)
                .size(ty.size(23.)),
            crate::hardware::view(draft, &state.host, true, state.hardware_busy, ty),
            row![
                kit::icon_label(
                    ty,
                    Icon::Check,
                    "Save hardware",
                    widgets::Button::Primary,
                    (!state.hardware_busy).then_some(wrap(MachineMessage::SaveHardware)),
                ),
                kit::icon_label(
                    ty,
                    Icon::Close,
                    "Cancel",
                    widgets::Button::Secondary,
                    (!state.hardware_busy).then_some(wrap(MachineMessage::CancelHardware)),
                ),
            ]
            .spacing(8),
        ]
        .spacing(18),
    )
    .height(Length::Fill)
    .into()
}

/// The way back to the machine library: a glyph, named by its tooltip.
fn back<'a>(ty: Typography) -> UiElement<'a> {
    kit::icon_button(
        ty,
        Icon::Back,
        "All machines",
        widgets::Button::Secondary,
        Some(wrap(MachineMessage::OpenLibrary)),
    )
}

fn action<'a>(
    ty: Typography,
    icon: Icon,
    label: &'static str,
    style: widgets::Button,
    message: MachineMessage,
) -> UiElement<'a> {
    kit::icon_label(ty, icon, label, style, Some(wrap(message)))
}

fn instance<'a>(
    state: &'a State,
    config: &'a MachineConfig,
    ty: Typography,
    appearance: &iced_core::Theme,
) -> PageColumn<'a> {
    let controls = back(ty);
    if state
        .edit_hardware
        .as_ref()
        .is_some_and(|(name, _)| name == &config.name)
    {
        return column![
            controls,
            container(edit_hardware(state, config, ty))
                .padding(theme::SPACE_4)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(widgets::card),
        ];
    }

    let running = state.running.get(&config.name);
    let status = machine_status(running);
    // Who this is, and how it is doing.
    let mut state_line = row![kit::status(ty, status)]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center);
    if let Some(steps) = status.steps {
        state_line = state_line.push(crate::motion::steps(steps, ty.size(80.), ty.size(6.)));
    }
    let plate = ty.size(44.);
    let identity = row![
        container(crate::visuals::icon(
            Icon::Machine,
            if running.is_some() {
                status.tone
            } else {
                theme::TEXT_MUTED
            },
            plate * 0.5,
        ))
        .center(plate)
        .style(move |appearance| if running.is_some() {
            theme::tinted(appearance, status.tone, 0.13, 0.30, theme::RADIUS_MD)
        } else {
            widgets::well(appearance)
        }),
        column![
            ty.data(config.name.as_str(), theme::TEXT)
                .size(ty.size(20.))
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
            state_line,
        ]
        .spacing(theme::SPACE_1 + 2.),
    ]
    .spacing(theme::SPACE_4 - 2.)
    .align_y(alignment::Vertical::Center);
    // What can be done with it, set against the right edge.
    let mut actions = row![]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center);
    if running.is_some() {
        actions = actions.push(action(
            ty,
            Icon::Stop,
            "Stop",
            widgets::Button::Danger,
            MachineMessage::Stop(config.name.clone()),
        ));
    } else {
        actions = actions
            .push(kit::icon_button(
                ty,
                Icon::Settings,
                "Hardware",
                widgets::Button::Secondary,
                Some(wrap(MachineMessage::EditHardware(config.name.clone()))),
            ))
            .push(kit::icon_button(
                ty,
                Icon::Window,
                "Start in a window",
                widgets::Button::Secondary,
                Some(wrap(MachineMessage::Start(config.name.clone(), true))),
            ))
            .push(kit::tip_text(
                ty,
                action(
                    ty,
                    Icon::Play,
                    "Start",
                    widgets::Button::Primary,
                    MachineMessage::Start(config.name.clone(), false),
                ),
                "Start headless, without a display window",
                iced_widget::tooltip::Position::Bottom,
            ));
    }
    let header: UiElement<'_> = container(
        row![controls, identity, space::horizontal(), actions]
            .spacing(theme::SPACE_4)
            .align_y(alignment::Vertical::Center),
    )
    .padding([theme::SPACE_2 + 4., theme::SPACE_4])
    .width(Length::Fill)
    .style(widgets::card)
    .into();
    static DEFAULT: std::sync::LazyLock<tiles::Workspace<Tile>> =
        std::sync::LazyLock::new(workspace);
    let workspace = state.layouts.get(&config.name).unwrap_or(&DEFAULT);
    let terminal = crate::console::Presentation::tile(ty, appearance);
    column![
        header,
        container(workspace.view(
            ty,
            |tile| {
                let info = tile.info();
                #[cfg(feature = "browser-preview")]
                if let Tile::Plugin(id) = tile {
                    return tiles::TileInfo {
                        title: state.plugins.tile_title(&config.name, id),
                        ..info
                    };
                }
                info
            },
            |tile| match tile {
                Tile::Overview => overview(state, config, ty),
                Tile::BootActivity => boot_activity(state, &config.name, ty),
                Tile::Console => console_tile(state, &config.name, terminal),
                Tile::Plugins => state.plugins.machine(&config.name, ty),
                #[cfg(feature = "browser-preview")]
                Tile::Plugin(id) => state.plugins.tile(&config.name, id, ty),
            },
            |change| wrap(MachineMessage::TileLayout(config.name.clone(), change))
        ))
        .id("machine-workspace")
        .width(Length::Fill)
        .height(Length::Fill),
    ]
}

/// A machine at a glance: what it holds of the host, as three metric cards;
/// what it boots, as detail wells whose long values truncate (the whole of one
/// is a tooltip away); and what it can do, as glyph chips.
fn overview<'a>(state: &'a State, config: &'a MachineConfig, ty: Typography) -> UiElement<'a> {
    let metric = |content: UiElement<'a>| -> UiElement<'a> {
        container(content)
            .padding([theme::SPACE_4, theme::SPACE_2])
            .width(Length::Fill)
            .center_x(Length::Fill)
            .style(widgets::well)
            .into()
    };
    let host = state.host.as_ref().ok();
    let memory_gib = f64::from(config.memory_mib) / 1024.;
    let disk_gib = config.disk_mib as f64 / 1024.;
    // A share of the host's, when the host is known; the figure alone when not.
    let cpu = match host.filter(|host| host.logical_cpus > 0) {
        Some(host) => kit::gauge(
            ty,
            Icon::Cpu,
            share(f64::from(config.cpus), host.logical_cpus as f64),
            load_tone(f64::from(config.cpus), host.logical_cpus as f64),
            config.cpus.to_string(),
            "CPUs",
            format!("of {}", host.logical_cpus),
        ),
        None => kit::tile_stat(ty, Icon::Cpu, config.cpus.to_string(), "CPUs", "allocated"),
    };
    let memory = match host.filter(|host| host.memory_mib > 0) {
        Some(host) => {
            let total = host.memory_mib as f64;
            kit::gauge(
                ty,
                Icon::Memory,
                share(f64::from(config.memory_mib), total),
                load_tone(f64::from(config.memory_mib), total),
                format!("{memory_gib:.1} GiB"),
                "Memory",
                format!("of {:.0} GiB", total / 1024.),
            )
        }
        None => kit::tile_stat(
            ty,
            Icon::Memory,
            format!("{memory_gib:.1} GiB"),
            "Memory",
            "allocated",
        ),
    };
    let disk = kit::tile_stat(
        ty,
        Icon::Volume,
        format!("{disk_gib:.1} GiB"),
        "Disk",
        "allocated",
    );

    // A detail: its glyph and name, then its value on one line, cut in the
    // middle when it does not fit, with the whole of it as a tooltip.
    let detail = |icon, title, value: String| -> UiElement<'a> {
        let shown = ty
            .data(value.clone(), theme::TEXT_SECONDARY)
            .width(Length::Fill)
            .wrapping(iced_core::text::Wrapping::None)
            .ellipsis(iced_core::text::Ellipsis::Middle);
        container(
            column![
                kit::heading(ty, icon, title),
                kit::tip(
                    shown,
                    ty.data(value, theme::TEXT)
                        .wrapping(iced_core::text::Wrapping::WordOrGlyph),
                    iced_widget::tooltip::Position::Top,
                ),
            ]
            .spacing(theme::SPACE_1 + 2.),
        )
        .padding(theme::SPACE_2 + 4.)
        .width(Length::Fill)
        .style(widgets::well)
        .into()
    };

    let resources = &config.resources;
    let mut features = row![
        kit::feature(ty, Icon::Cpu, "CPU pinning", resources.pin_cpus),
        kit::feature(
            ty,
            Icon::Activity,
            "Native waits",
            resources.cpu_power_management
        ),
        kit::feature(ty, Icon::Lock, "Memory pinning", resources.pin_memory),
    ]
    .spacing(theme::SPACE_2);
    for (icon, label) in topology(resources) {
        features = features.push(kit::figure(ty, icon, label));
    }
    let hardware: UiElement<'a> = container(
        column![
            kit::heading(ty, Icon::Settings, "Hardware"),
            features.wrap().vertical_spacing(theme::SPACE_2),
        ]
        .spacing(theme::SPACE_2),
    )
    .padding(theme::SPACE_2 + 4.)
    .width(Length::Fill)
    .style(widgets::well)
    .into();

    let mut details = column![
        row![metric(cpu), metric(memory), metric(disk)].spacing(theme::SPACE_2),
        detail(Icon::File, "Boot image", config.iso.display().to_string()),
        detail(
            Icon::Hash,
            "SHA-256",
            config
                .iso_sha256
                .clone()
                .unwrap_or_else(|| "Recorded after the first boot".to_owned()),
        ),
        hardware,
    ]
    .spacing(theme::SPACE_2);
    if let Some(workspace) = &config.workspace {
        details = details.push(detail(
            Icon::Folder,
            "Project folder",
            format!(
                "{} → ~{}/{} ({:?})",
                workspace.root.display(),
                workspace.face,
                workspace.name,
                workspace.writeback
            ),
        ));
    }
    widgets::scroll(details).height(Length::Fill).into()
}

/// What share `used` is of `total`, for a ring: never outside zero to one,
/// and zero for a total that is not a quantity.
fn share(used: f64, total: f64) -> f32 {
    if total > 0.0 && used.is_finite() {
        (used / total).clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

/// A ring is the accent while there is room, and warns when the machine
/// claims most of the host: that is the point at which a second machine, or
/// the host itself, starts to feel it.
fn load_tone(used: f64, total: f64) -> theme::Tone {
    if share(used, total) > 0.75 {
        theme::WARNING
    } else {
        theme::MAGENTA
    }
}

fn boot_activity<'a>(state: &'a State, name: &str, ty: Typography) -> UiElement<'a> {
    let finished = state.finished.get(name);
    let log = state
        .running
        .get(name)
        .map(|run| run.log.as_str())
        .or_else(|| finished.map(|run| run.log.as_str()))
        .unwrap_or_default();
    let mut contents = column![].spacing(theme::SPACE_2);
    if let Some(finished) = finished {
        contents = contents.push(ty.data(finished.summary.as_str(), theme::TEXT_MUTED));
    }
    if log.is_empty() {
        contents = contents.push(
            row![
                crate::visuals::icon(Icon::Activity, theme::TEXT_MUTED, ty.size(16.)),
                ty.body(
                    "Boot activity appears after you start the machine.",
                    13.,
                    theme::TEXT_MUTED,
                ),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
        );
    } else {
        // Log lines were escaped when the worker recorded them.
        contents = contents.push(
            ty.mono(log, theme::TEXT_SECONDARY)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph),
        );
    }
    widgets::scroll(contents)
        .height(Length::Fill)
        .anchor_bottom()
        .into()
}

fn console_tile<'a>(
    state: &'a State,
    name: &'a str,
    terminal: crate::console::Presentation,
) -> UiElement<'a> {
    let ty = terminal.ty;
    if let Some(running) = state.running.get(name) {
        if let Some(console) = &running.console {
            return console_pane(
                name,
                running,
                console,
                state.closing || state.hardware_busy,
                state.console_focus.as_deref() == Some(name),
                terminal,
            );
        }
        if !running.console_error.is_empty() {
            return ty
                .data(running.console_error.as_str(), theme::ERROR)
                .wrapping(iced_core::text::Wrapping::WordOrGlyph)
                .into();
        }
    } else if let Some(transcript) = state
        .finished
        .get(name)
        .and_then(|run| run.console.as_ref())
    {
        return crate::console::view(transcript, terminal);
    }
    if state.running.contains_key(name) {
        // Something is on its way: a turning mark, not a sentence.
        return container(
            column![
                crate::motion::beacon(Beat::Spin, theme::TEXT_SECONDARY, ty.size(28.)),
                ty.body("Waiting for the guest console…", 14., theme::TEXT_MUTED),
            ]
            .spacing(theme::SPACE_2)
            .align_x(alignment::Horizontal::Center),
        )
        .center(Length::Fill)
        .into();
    }
    ty.empty_state(
        Icon::Terminal,
        "No console yet",
        "Start the machine to connect to its console.",
    )
}

fn console_pane<'a>(
    name: &'a str,
    running: &'a Running,
    console: &'a vkored::console::Console,
    closing: bool,
    focused: bool,
    terminal: crate::console::Presentation,
) -> UiElement<'a> {
    let ty = terminal.ty;
    let enabled = !closing && console.connected();
    let connection = if console.connected() {
        Status {
            beat: Beat::Pulse,
            tone: theme::SUCCESS,
            label: "connected",
            steps: None,
        }
    } else {
        Status {
            beat: Beat::Hollow,
            tone: theme::TEXT_MUTED,
            label: "disconnected",
            steps: None,
        }
    };
    let output = console.output();
    // One line of what to do now; the whole of it is a tooltip away.
    let hint = if focused {
        "Ctrl+Shift+Esc releases the keyboard"
    } else {
        "Click the terminal to type"
    };
    let mut pane = column![
        row![
            kit::status(ty, connection),
            space::horizontal(),
            kit::tip_text(
                ty,
                kit::figure(ty, Icon::Keyboard, hint),
                "Click to type · Ctrl+Shift+V / Cmd+V pastes · Ctrl+Shift+Esc releases the keyboard",
                iced_widget::tooltip::Position::Top,
            ),
            kit::figure(ty, Icon::Terminal, format!("{} × {}", output.cols, output.rows)),
        ]
        .spacing(theme::SPACE_4)
        .align_y(alignment::Vertical::Center),
        crate::console::interactive(output, name, enabled, focused, terminal),
    ]
    .spacing(12);
    if let Some(error) = console
        .error()
        .or_else(|| (!running.console_error.is_empty()).then_some(running.console_error.as_str()))
    {
        pane = pane.push(ty.data(error.to_owned(), theme::ERROR));
    }
    container(pane)
        .id("machine-console")
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The guest's page size, layout, network and accelerator assignments, each
/// with the glyph that says what kind of thing it is.
fn topology(resources: &vkore_vm::hardware::Resources) -> Vec<(Icon, String)> {
    let layout = match resources.guest_numa {
        vkore_vm::hardware::GuestNuma::Single => "1 NUMA node",
        vkore_vm::hardware::GuestNuma::Split => "2 NUMA nodes",
    };
    let mut parts = vec![
        (Icon::Memory, resources.pages.label().to_owned()),
        (Icon::Dashboard, layout.to_owned()),
        (Icon::Network, resources.network.label().to_owned()),
    ];
    parts.extend(
        resources
            .passthrough
            .iter()
            .map(|s| (Icon::Plugin, format!("NPU {}", s.address))),
    );
    parts.extend(resources.gpu_vfs.iter().map(|s| {
        (
            Icon::Plugin,
            format!("GPU {} VF {}", s.physical_function, s.index + 1),
        )
    }));
    parts
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[test]
    fn closing_waits_for_every_session_to_finish_and_stage_updates_stay_current() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(dir.path().to_owned());
        let (first, first_stop) = mpsc::channel();
        let (second, second_stop) = mpsc::channel();
        state.running.insert("first".into(), Running::new(first));
        state.running.insert("second".into(), Running::new(second));
        state.machine_update("first".into(), Update::Stage(Stage::Ready));
        assert_eq!(state.status, "first: ready");
        assert!(!state.request_close());
        assert!(state.closing);
        assert_eq!(first_stop.try_recv(), Ok(()));
        assert_eq!(second_stop.try_recv(), Ok(()));
        state.machine_update("first".into(), Update::Finished(Ok("collected".into())));
        assert_eq!(state.running.len(), 1);
        state.machine_update("second".into(), Update::Finished(Err("stop failed".into())));
        assert!(state.running.is_empty());
        assert!(state.finished["second"].summary.contains("stop failed"));
        assert!(state.request_close());
        let (stop, _) = mpsc::channel();
        let mut other = State::new(dir.path().to_owned());
        other.running.insert("other".into(), Running::new(stop));
        assert!(!other.request_close());
        assert!(
            other.request_close(),
            "second close explicitly leaves immediately"
        );
    }

    #[test]
    fn a_late_clipboard_result_cannot_type_into_a_new_focus_session() {
        use std::{io::Read, os::unix::net::UnixListener};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console");
        let listener = UnixListener::bind(&path).unwrap();
        let console = vkored::console::Console::connect(&path).unwrap();
        let (mut guest, _) = listener.accept().unwrap();
        guest.set_nonblocking(true).unwrap();
        let (stop, _) = mpsc::channel();
        let mut running = Running::new(stop);
        running.console = Some(console);
        let mut state = State::new(dir.path().to_owned());
        state.running.insert("guest".into(), running);
        state.console_focus = Some("guest".into());
        state.selected = Some("guest".into());
        let old_epoch = state.console_focus_epoch;
        state.paste_pending = true;
        state.unfocus_console();
        state.console_focus = Some("guest".into());
        state.machine_update(
            "guest".into(),
            Update::Paste {
                epoch: old_epoch,
                result: Ok("reboot\n".into()),
            },
        );
        assert!(!state.paste_pending);
        state
            .running
            .get_mut("guest")
            .unwrap()
            .console
            .as_mut()
            .unwrap()
            .poll();
        let mut bytes = [0; 32];
        assert_eq!(
            guest.read(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        state.machine_update(
            "guest".into(),
            Update::Paste {
                epoch: state.console_focus_epoch,
                result: Ok("help".into()),
            },
        );
        state
            .running
            .get_mut("guest")
            .unwrap()
            .console
            .as_mut()
            .unwrap()
            .poll();
        let count = guest.read(&mut bytes).unwrap();
        assert_eq!(&bytes[..count], b"help");
    }

    #[test]
    fn console_pane_renders_guest_text_in_both_themes() {
        use std::{io::Write, os::unix::net::UnixListener};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("console");
        let listener = UnixListener::bind(&path).unwrap();
        let mut console = vkored::console::Console::connect(&path).unwrap();
        let (mut guest, _) = listener.accept().unwrap();
        guest.write_all("Kore shell\r\n> status devices\r\n\u{1b}[1;36mDEVICES\u{1b}[0m\r\nNPU bound · accelerator up\r\nfs: \u{1b}[1;34mpictures/\u{1b}[m\r\n日本語 \u{202e}\r\n\u{1b}[?25h> ".as_bytes()).unwrap();
        assert!(console.poll());
        let (stop, _) = mpsc::channel();
        let mut running = Running::new(stop);
        running.console = Some(console);
        for appearance in [iced_core::Theme::Light, iced_core::Theme::Dark] {
            let (runs, refused) = crate::tests::draw_at_scale(
                widgets::page_frame(console_pane(
                    "headless",
                    &running,
                    running.console.as_ref().unwrap(),
                    false,
                    true,
                    crate::console::Presentation::new(Typography::default(), &appearance, 600.),
                )),
                iced_core::Size::new(900., 560.),
                &appearance,
                1.,
                Some("console"),
            );
            assert!(runs > 5);
            assert!(refused.is_empty(), "{refused:?}");
        }
    }

    #[test]
    fn completion_closes_only_after_success_and_preserves_denied_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = State::new(dir.path().to_owned());
        state.configure_new = true;
        state.hardware_busy = true;
        state.form_hardware.memory_mib = 4096;
        state.machine_update(
            String::new(),
            Update::HardwarePrepared(Err("cancelled".into())),
        );
        assert!(state.configure_new);
        assert!(!state.hardware_busy);
        assert_eq!(state.form_hardware.memory_mib, 4096);
        assert_eq!(state.status, "cancelled");
        let host = vkore_vm::hardware::HostResources::probe(dir.path()).unwrap();
        state.hardware_busy = true;
        state.machine_update(String::new(), Update::HardwarePrepared(Ok(Box::new(host))));
        assert!(!state.configure_new);
        assert!(!state.hardware_busy);
        let config = MachineConfig::new("demo", "/image.iso");
        state.edit_hardware = Some((
            "demo".into(),
            crate::hardware::Draft::from_config(&config, false),
        ));
        state.hardware_busy = true;
        state.machine_update("demo".into(), Update::HardwareSaved(Err("denied".into())));
        assert!(state.edit_hardware.is_some());
        assert!(!state.hardware_busy);
        state.machine_update("demo".into(), Update::HardwareSaved(Ok(())));
        assert!(state.edit_hardware.is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_loading_failure_and_progress_draw_at_compact_and_wide_sizes() {
        let root = tempfile::tempdir().unwrap();
        let mut state = State::new(root.path().to_owned());
        for phase in ["loading", "error", "download", "verification"] {
            state.catalog_cancel = None;
            state.catalog_error = None;
            state.importing = None;
            state.import_progress = None;
            match phase {
                "loading" => state.catalog_cancel = Some(Arc::new(AtomicBool::new(false))),
                "error" => {
                    state.catalog_error = Some(
                        "Cannot load official images. Check your connection and refresh.".into(),
                    )
                }
                _ => {
                    let doc = r#"{"schema_version":1,"images":[{
                        "id":"release-1", "name":"spica headless", "version":"0.0.0a",
                        "profile":"inference", "architecture":"x86_64", "format":"iso",
                        "url":"https://dev.microperceptron.com/dl/release-1/spica.iso",
                        "sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "bytes":168382464,
                        "kore_revision":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "dagfs_format_version":FORMAT_VERSION
                    }]}"#
                        .replace(
                            "FORMAT_VERSION",
                            &vkore_volume::bounds::FORMAT_VERSION.to_string(),
                        );
                    state.official_images = vkored::catalog::Catalog::parse(doc.as_bytes())
                        .unwrap()
                        .images()
                        .to_vec();
                    state.selected_image = Some("release-1".into());
                    state.form_name = "inference".into();
                    state.importing = Some(Arc::new(AtomicBool::new(false)));
                    state.machine_update(
                        String::new(),
                        Update::ImportProgress(vkored::images::Progress {
                            bytes: 80 << 20,
                            total: Some(161 << 20),
                            phase: if phase == "download" {
                                vkored::images::Phase::Downloading
                            } else {
                                vkored::images::Phase::Verifying
                            },
                        }),
                    );
                }
            }
            for appearance in [iced_core::Theme::Dark, iced_core::Theme::Light] {
                for width in [760., 1280.] {
                    let shot = format!("archives-{phase}-{width}");
                    let (_, refused) = crate::tests::draw_at_scale(
                        container(view(&state, Typography::default(), appearance.clone()))
                            .style(|appearance| {
                                theme::panel(appearance, theme::BACKGROUND, None, 0.)
                            })
                            .into(),
                        iced_core::Size::new(width, 800.),
                        &appearance,
                        1.,
                        Some(&shot),
                    );
                    assert!(refused.is_empty(), "{phase}: {refused:?}");
                }
            }
        }
    }
}
