//! Machines: create them, boot them, follow their boots, stop them.
//!
//! Each running machine has a worker thread that owns its [`Session`]: it
//! follows the boot, relays every event to the view, and stops the machine
//! when asked (the power button, then QEMU's end) or when the window closes.

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

use crate::{Message, UiElement, UserEvent, fonts, theme, widgets};

/// What a machine's worker reports.
#[derive(Debug)]
pub enum Update {
    Line(String),
    Stage(Stage),
    Finished(Result<String, String>),
    ImportProgress(vkored::images::Progress),
    Created(Result<(), String>),
}

/// A machine the view started.
pub struct Running {
    stop: mpsc::Sender<()>,
    pub stage: Option<Stage>,
    /// What the boot reported, one escaped line each, as the pane shows it.
    pub log: String,
    lines: usize,
    pub started: Instant,
}

impl Running {
    /// The most lines the log keeps.
    const LINES: usize = 400;

    fn new(stop: mpsc::Sender<()>) -> Self {
        Self {
            stop,
            stage: None,
            log: String::new(),
            lines: 0,
            started: Instant::now(),
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
}

/// Machines, and the form that creates one.
pub struct State {
    pub state_root: PathBuf,
    pub configs: Vec<MachineConfig>,
    pub selected: Option<String>,
    pub running: BTreeMap<String, Running>,
    /// How each machine's last run from this window ended, and its log.
    pub finished: BTreeMap<String, Finished>,
    pub form_name: String,
    pub official_images: Vec<vkored::catalog::Image>,
    pub selected_image: Option<String>,
    pub image_menu_open: bool,
    pub catalog_error: Option<String>,
    importing: Option<Arc<AtomicBool>>,
    import_worker: Option<thread::JoinHandle<()>>,
    pub form_workspace: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum MachineMessage {
    Select(String),
    Start(String, bool),
    Stop(String),
    FormName(String),
    SelectImage(String),
    ToggleImageMenu,
    CancelImport,
    FormWorkspace(String),
    Create,
    Refresh,
}

impl State {
    pub fn new(state_root: PathBuf) -> Self {
        let mut state = Self {
            state_root,
            configs: Vec::new(),
            selected: None,
            running: BTreeMap::new(),
            finished: BTreeMap::new(),
            form_name: String::new(),
            official_images: Vec::new(),
            selected_image: None,
            image_menu_open: false,
            catalog_error: None,
            importing: None,
            import_worker: None,
            form_workspace: String::new(),
            status: String::new(),
        };
        match vkored::catalog::official() {
            Ok(catalog) => state.official_images = catalog.images().to_vec(),
            Err(error) => state.catalog_error = Some(error),
        }
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
            Ok(configs) => self.configs = configs,
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

    pub fn update(&mut self, message: MachineMessage, proxy: &EventLoopProxy<UserEvent>) {
        match message {
            MachineMessage::Select(name) => self.selected = Some(name),
            MachineMessage::Refresh => self.refresh(),
            MachineMessage::FormName(value) => self.form_name = value,
            MachineMessage::SelectImage(id) => {
                if self.importing.is_none()
                    && self.official_images.iter().any(|image| image.id == id)
                {
                    self.selected_image = Some(id);
                    self.image_menu_open = false;
                }
            }
            MachineMessage::ToggleImageMenu => {
                if self.importing.is_none() && !self.official_images.is_empty() {
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

    fn create(&mut self, proxy: EventLoopProxy<UserEvent>) {
        if self.importing.is_some() {
            return;
        }
        self.image_menu_open = false;
        let Some(image_id) = self
            .selected_image
            .clone()
            .filter(|id| self.official_images.iter().any(|image| &image.id == id))
        else {
            self.status = "Select an available official image first".into();
            return;
        };
        let absolute = |text: &str| {
            std::fs::canonicalize(text.trim()).map_err(|error| format!("{}: {error}", text.trim()))
        };
        let created = (|| {
            let mut config = MachineConfig::new(self.form_name.trim(), "/pending.iso");
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
                            .map(MachineStore::new)
                            .map_err(|e| e.to_string())
                            .and_then(|store| {
                                vkored::images::create_official(
                                    &store,
                                    config,
                                    &image_id,
                                    &signal,
                                    |progress| send(Update::ImportProgress(progress)),
                                )
                                .map(|_| ())
                            });
                        send(Update::Created(outcome));
                    });
                match result {
                    Ok(worker) => {
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
        if self.running.contains_key(&name) {
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
            Update::ImportProgress(progress) => {
                self.status = format!(
                    "Downloading image: {} MiB{}",
                    progress.bytes >> 20,
                    progress
                        .total
                        .map(|n| format!(" / {} MiB", n >> 20))
                        .unwrap_or_default()
                );
            }
            Update::Created(result) => {
                self.importing = None;
                if let Some(worker) = self.import_worker.take() {
                    let _ = worker.join();
                }
                match result {
                    Ok(()) => {
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
                }
            }
            Update::Finished(outcome) => {
                // The log stays: how a run ended is the evidence.
                let log = self
                    .running
                    .remove(&name)
                    .map(|running| running.log)
                    .unwrap_or_default();
                let summary = match outcome {
                    Ok(summary) => summary,
                    Err(error) => format!("failed: {error}"),
                };
                self.status = format!("{name}: {summary}");
                self.finished.insert(name, Finished { summary, log });
                self.refresh();
            }
        }
    }

    /// Ask every running machine to stop (the window is closing).
    pub fn stop_all(&mut self) {
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
    let mut sink = |event: &Event| match event {
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

fn stage_label(stage: Option<Stage>) -> (&'static str, theme::Tone) {
    match stage {
        None | Some(Stage::Firmware) => ("starting", theme::TEXT_MUTED),
        Some(Stage::Kernel) => ("kernel up", theme::TEXT_SECONDARY),
        Some(Stage::Services) => ("services", theme::TEXT_SECONDARY),
        Some(Stage::Shell) => ("shell ready", theme::SUCCESS),
        Some(Stage::Ready) => ("ready", theme::SUCCESS),
        Some(Stage::Degraded) => ("degraded", theme::WARNING),
        Some(Stage::Failed) => ("failed", theme::ERROR),
    }
}

/// The machines page.
pub fn view(state: &State) -> UiElement<'_> {
    let mut list = column![widgets::label("Your machines", theme::TEXT_MUTED)].spacing(10);
    if state.configs.is_empty() {
        list = list
            .push(widgets::body("No machines yet", 15., theme::TEXT))
            .push(widgets::body(
                "Add an official image below to get started.",
                13.,
                theme::TEXT_MUTED,
            ));
    }
    for config in &state.configs {
        let selected = state.selected.as_deref() == Some(config.name.as_str());
        let (label, color) = match state.running.get(&config.name) {
            Some(running) => stage_label(running.stage),
            None => ("stopped", theme::TEXT_MUTED),
        };
        list = list.push(
            button(
                row![
                    widgets::data(config.name.as_str(), theme::TEXT).size(theme::SIZE_SM),
                    space::horizontal(),
                    widgets::badge(label, color),
                ]
                .align_y(alignment::Vertical::Center),
            )
            .width(Length::Fill)
            .padding([10, 12])
            .style(widgets::button_style(widgets::Button::Row(selected)))
            .on_press(wrap(MachineMessage::Select(config.name.clone()))),
        );
    }
    let selected_image = state
        .official_images
        .iter()
        .find(|image| Some(&image.id) == state.selected_image.as_ref());
    let label = selected_image.map_or_else(
        || {
            if state.official_images.is_empty() {
                "No official images available".to_owned()
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
                widgets::data(label, theme::TEXT).width(Length::Fill),
                widgets::mono(
                    if state.image_menu_open { "^" } else { "v" },
                    theme::TEXT_MUTED
                ),
            ]
            .spacing(theme::SPACE_1)
            .align_y(alignment::Vertical::Center)
        )
        .width(Length::Fill)
        .padding([7, 10])
        .style(widgets::button_style(widgets::Button::Secondary))
        .on_press_maybe(
            (!state.official_images.is_empty() && state.importing.is_none())
                .then(|| wrap(MachineMessage::ToggleImageMenu))
        )
    ]
    .spacing(theme::SPACE_1);
    if state.image_menu_open && state.importing.is_none() {
        let mut options = column![].spacing(theme::SPACE_1);
        for image in &state.official_images {
            options = options.push(
                button(widgets::data(
                    format!("{} ({})", image.name, image.version),
                    theme::TEXT,
                ))
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
    let create_action = if state.importing.is_some() {
        Some(wrap(MachineMessage::CancelImport))
    } else if selected_image.is_some() && vkored::config::valid_name(state.form_name.trim()) {
        Some(wrap(MachineMessage::Create))
    } else {
        None
    };
    let form = column![
        widgets::body("Create a machine", 18., theme::TEXT),
        widgets::body("Start with a verified Kore image.", 13., theme::TEXT_MUTED),
        widgets::label("Machine name", theme::TEXT_MUTED),
        text_input("e.g. inference", &state.form_name)
            .on_input(|value| wrap(MachineMessage::FormName(value)))
            .font(fonts::MONO)
            .padding([7, 10])
            .size(theme::SIZE_SM)
            .style(widgets::field),
        widgets::label("Official image", theme::TEXT_MUTED),
        images,
        widgets::data(
            state
                .catalog_error
                .as_deref()
                .unwrap_or(if state.official_images.is_empty() {
                    "Official downloads have not been published yet."
                } else {
                    "The official image is verified before it is added."
                }),
            theme::TEXT_MUTED
        ),
        widgets::label("Project folder · optional", theme::TEXT_MUTED),
        text_input("/path/to/project", &state.form_workspace)
            .on_input(|value| wrap(MachineMessage::FormWorkspace(value)))
            .font(fonts::MONO)
            .padding([7, 10])
            .size(theme::SIZE_SM)
            .style(widgets::field),
        button(widgets::control(if state.importing.is_some() {
            "Cancel import"
        } else {
            "Create machine"
        }))
        .height(theme::CONTROL_SM)
        .padding([8, 14])
        .style(widgets::button_style(widgets::Button::Primary))
        .on_press_maybe(create_action),
    ]
    .spacing(12);
    let left = widgets::scroll(
        column![
            container(list)
                .padding(18)
                .width(Length::Fill)
                .style(widgets::card),
            container(form)
                .padding(18)
                .width(Length::Fill)
                .style(widgets::card),
        ]
        .spacing(16),
    )
    .height(Length::Fill)
    .width(Length::FillPortion(2).max(360))
    .id("machine-list");

    let detail: UiElement<'_> = match state
        .selected
        .as_ref()
        .and_then(|name| state.configs.iter().find(|config| &config.name == name))
    {
        None => widgets::empty_state(
            crate::visuals::Icon::Machine,
            if state.configs.is_empty() {
                "A home for your Kore machines"
            } else {
                "Choose a machine"
            },
            if state.configs.is_empty() {
                "Official images will appear here when they are released. Your machines, boot status and controls will live in this workspace."
            } else {
                "Select a machine to see its resources, manage its power and follow its boot."
            },
        ),
        Some(config) => {
            let running = state.running.get(&config.name);
            let (status, tone) =
                running.map_or(("Stopped", theme::TEXT_MUTED), |run| stage_label(run.stage));
            let metric = |label, value: String, unit| {
                container(
                    column![
                        widgets::label(label, theme::TEXT_MUTED),
                        widgets::data(value, theme::TEXT).size(20.),
                        widgets::body(unit, 12., theme::TEXT_MUTED)
                    ]
                    .spacing(8),
                )
                .padding(14)
                .width(Length::Fill)
                .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 8.))
            };
            let mut pane = column![
                row![
                    widgets::data(config.name.as_str(), theme::TEXT).size(23.),
                    space::horizontal(),
                    widgets::badge(status, tone)
                ]
                .spacing(12)
                .align_y(alignment::Vertical::Center),
                widgets::body("Machine overview", 13., theme::TEXT_MUTED),
                row![
                    metric("CPUs", config.cpus.to_string(), "cores"),
                    metric("Memory", config.memory_mib.to_string(), "MiB"),
                    metric("Disk", config.disk_mib.to_string(), "MiB")
                ]
                .spacing(10),
                widgets::label("Boot image", theme::TEXT_MUTED),
                widgets::data(config.iso.display().to_string(), theme::TEXT_SECONDARY),
                widgets::label("SHA-256", theme::TEXT_MUTED),
                widgets::data(
                    config
                        .iso_sha256
                        .as_deref()
                        .unwrap_or("Recorded after the first boot"),
                    theme::TEXT_MUTED
                ),
            ]
            .spacing(14);
            if let Some(workspace) = &config.workspace {
                pane = pane.push(widgets::data(
                    format!(
                        "project  {} → ~{}/{} ({:?})",
                        workspace.root.display(),
                        workspace.face,
                        workspace.name,
                        workspace.writeback
                    ),
                    theme::TEXT_SECONDARY,
                ));
            }
            let controls = match running {
                None => row![
                    button(widgets::control("Start in a window"))
                        .height(theme::CONTROL_SM)
                        .padding([8, 14])
                        .style(widgets::button_style(widgets::Button::Primary))
                        .on_press(wrap(MachineMessage::Start(config.name.clone(), true))),
                    button(widgets::control("Start headless"))
                        .height(theme::CONTROL_SM)
                        .padding([8, 14])
                        .style(widgets::button_style(widgets::Button::Secondary))
                        .on_press(wrap(MachineMessage::Start(config.name.clone(), false))),
                ],
                Some(_) => row![
                    button(widgets::control("Stop"))
                        .height(theme::CONTROL_SM)
                        .padding([8, 14])
                        .style(widgets::button_style(widgets::Button::Danger))
                        .on_press(wrap(MachineMessage::Stop(config.name.clone()))),
                ],
            }
            .spacing(theme::SPACE_2);
            pane = pane.push(controls);
            let finished = state.finished.get(&config.name);
            if let Some(finished) = finished {
                pane = pane.push(widgets::data(finished.summary.as_str(), theme::TEXT_MUTED));
            }
            let log = match (running, finished) {
                (Some(running), _) => running.log.as_str(),
                (None, Some(finished)) => finished.log.as_str(),
                (None, None) => "",
            };
            pane = pane.push(widgets::label("Boot activity", theme::TEXT_MUTED));
            if log.is_empty() {
                pane = pane.push(
                    container(
                        column![
                            widgets::body("Ready when you are", 16., theme::TEXT),
                            widgets::body(
                                "Start this machine to follow its boot activity here.",
                                13.,
                                theme::TEXT_MUTED
                            ),
                        ]
                        .spacing(8),
                    )
                    .padding(20)
                    .width(Length::Fill)
                    .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 8.)),
                );
            } else {
                pane = pane.push(
                    container(
                        widgets::scroll(widgets::mono(log, theme::TEXT_SECONDARY))
                            .height(220)
                            .anchor_bottom(),
                    )
                    .padding(14)
                    .width(Length::Fill)
                    .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 8.)),
                );
            }
            widgets::scroll(pane.spacing(16))
                .height(Length::Fill)
                .into()
        }
    };
    let mut page = column![
        widgets::page_header("Machines", "Your Kore guests, ready for work."),
        row![
            left,
            container(detail)
                .id("machine-detail")
                .width(Length::FillPortion(3))
                .height(Length::Fill)
                .padding(24)
                .style(widgets::card),
        ]
        .spacing(theme::SPACE_4)
        .height(Length::Fill),
    ]
    .spacing(theme::SPACE_4);
    if !state.status.is_empty() {
        page = page.push(widgets::data(state.status.as_str(), theme::TEXT_MUTED));
    }
    widgets::page_frame(page)
}
