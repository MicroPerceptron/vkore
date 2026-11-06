//! Machines: create them, boot them, follow their boots, stop them.
//!
//! Each running machine has a worker thread that owns its [`Session`]: it
//! follows the boot, relays every event to the view, and stops the machine
//! when asked (the power button, then QEMU's end) or when the window closes.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, scrollable, space, text_input};
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
    pub form_iso: String,
    pub form_workspace: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum MachineMessage {
    Select(String),
    Start(String, bool),
    Stop(String),
    FormName(String),
    FormIso(String),
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
            form_iso: String::new(),
            form_workspace: String::new(),
            status: String::new(),
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
            MachineMessage::FormIso(value) => self.form_iso = value,
            MachineMessage::FormWorkspace(value) => self.form_workspace = value,
            MachineMessage::Create => self.create(),
            MachineMessage::Start(name, window) => self.start(name, window, proxy.clone()),
            MachineMessage::Stop(name) => {
                if let Some(running) = self.running.get(&name) {
                    let _ = running.stop.send(());
                    self.status = format!("{name}: stopping");
                }
            }
        }
    }

    fn create(&mut self) {
        let absolute = |text: &str| {
            std::fs::canonicalize(text.trim()).map_err(|error| format!("{}: {error}", text.trim()))
        };
        let created = (|| {
            let store = self.store()?;
            let mut config = MachineConfig::new(self.form_name.trim(), absolute(&self.form_iso)?);
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
            store.create(&config).map_err(|error| error.to_string())?;
            Ok::<_, String>(config.name)
        })();
        match created {
            Ok(name) => {
                self.status = format!("created {name}");
                self.form_name.clear();
                self.refresh();
                self.selected = Some(name);
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
        for running in self.running.values() {
            let _ = running.stop.send(());
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

fn stage_label(stage: Option<Stage>) -> (&'static str, u32) {
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
    let mut list = column![].spacing(theme::SPACE_1);
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
                    widgets::mono(label, color),
                ]
                .align_y(alignment::Vertical::Center),
            )
            .width(Length::Fill)
            .padding([7, 10])
            .style(widgets::button_style(widgets::Button::Row(selected)))
            .on_press(wrap(MachineMessage::Select(config.name.clone()))),
        );
    }
    let form = column![
        widgets::label("New machine", theme::TEXT_MUTED),
        text_input("name", &state.form_name)
            .on_input(|value| wrap(MachineMessage::FormName(value)))
            .font(fonts::MONO)
            .padding([7, 10])
            .size(theme::SIZE_SM)
            .style(widgets::field),
        text_input("/path/to/kore-x86-release.iso", &state.form_iso)
            .on_input(|value| wrap(MachineMessage::FormIso(value)))
            .font(fonts::MONO)
            .padding([7, 10])
            .size(theme::SIZE_SM)
            .style(widgets::field),
        text_input("project folder to stage (optional)", &state.form_workspace)
            .on_input(|value| wrap(MachineMessage::FormWorkspace(value)))
            .font(fonts::MONO)
            .padding([7, 10])
            .size(theme::SIZE_SM)
            .style(widgets::field),
        button(widgets::control("Create"))
            .height(theme::CONTROL_SM)
            .padding([8, 14])
            .style(widgets::button_style(widgets::Button::Primary))
            .on_press(wrap(MachineMessage::Create)),
    ]
    .spacing(theme::SPACE_2);
    let left = column![scrollable(list).height(Length::Fill), form]
        .spacing(theme::SPACE_4)
        .width(Length::FillPortion(2));

    let detail: UiElement<'_> = match state
        .selected
        .as_ref()
        .and_then(|name| state.configs.iter().find(|config| &config.name == name))
    {
        None => widgets::body(
            "Select a machine, or create one from a Kore ISO.",
            theme::SIZE_SM,
            theme::TEXT_MUTED,
        )
        .into(),
        Some(config) => {
            let running = state.running.get(&config.name);
            let mut pane = column![
                widgets::data(config.name.as_str(), theme::TEXT).size(theme::SIZE_MD),
                widgets::data(
                    format!("iso      {}", config.iso.display()),
                    theme::TEXT_SECONDARY
                ),
                widgets::data(
                    format!(
                        "sha256   {}",
                        config.iso_sha256.as_deref().unwrap_or("(not booted yet)")
                    ),
                    theme::TEXT_SECONDARY,
                ),
                widgets::mono(
                    format!(
                        "machine  {} CPUs · {} MiB · {} MiB disk",
                        config.cpus, config.memory_mib, config.disk_mib
                    ),
                    theme::TEXT_SECONDARY,
                ),
            ]
            .spacing(theme::SPACE_1);
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
            pane = pane.push(
                container(
                    scrollable(widgets::mono(log, theme::TEXT_SECONDARY))
                        .height(Length::Fill)
                        .anchor_bottom()
                        .style(widgets::hidden_scrollbar),
                )
                .padding(theme::SPACE_2)
                .height(Length::Fill)
                .width(Length::Fill)
                .style(|_| theme::panel(theme::PANEL_FILE, Some(theme::BORDER), theme::RADIUS_MD)),
            );
            pane.spacing(theme::SPACE_2).height(Length::Fill).into()
        }
    };
    container(
        column![
            widgets::display("Machines"),
            row![
                left,
                container(detail)
                    .width(Length::FillPortion(5))
                    .height(Length::Fill)
                    .padding(theme::SPACE_4)
                    .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG)),
            ]
            .spacing(theme::SPACE_4)
            .height(Length::Fill),
            widgets::data(state.status.as_str(), theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_4),
    )
    .padding(theme::SPACE_6)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
