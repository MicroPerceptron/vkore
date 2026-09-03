mod fonts;
mod render_backend;
mod theme;
mod topology;

use crate::topology::{layout, model, view};
mod widgets;

use axgfx::Extent2d;
use axui_iced::Renderer as AxRenderer;
use iced_core::Renderer as _;
use iced_core::{
    Background, Border, Color, Element, Length, Size, Theme, alignment, keyboard, mouse,
    renderer::{self, Scale},
    shell,
};
use iced_runtime::user_interface::{self, UserInterface};
use iced_widget::{
    button, column, container, row, scrollable, space, stack, text, text_editor, text_input,
};
use iced_winit::{conversion, winit};
use serde_json::{Value, json};
use softbuffer::{Context as SoftContext, Surface as SoftSurface};
use std::{
    alloc::System,
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    num::NonZeroU32,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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

type UiElement<'a> = Element<'a, Message, Theme, AxUi>;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(750);

fn native_renderer() -> AxRenderer<System> {
    AxRenderer::new_in(renderer::Settings::default(), System)
}

#[derive(Debug, Clone)]
enum Message {
    SelectProject(usize),
    NewProject,
    EditProject,
    ConfigAction(iced_core::text::editor::Action),
    WizardField(&'static str, String),
    WizardToggle(&'static str),
    WizardChoice(&'static str, &'static str),
    WizardAddon(String),
    WizardPage(u8),
    Preflight,
    SaveProject,
    Browse(RequestKind),
    CloseEditor,
    ToggleSession,
    CleanSession,
    PromptAction(iced_core::text::editor::Action),
    SelectText(String, iced_core::text::editor::Action),
    SendPrompt,
    CancelPrompt,
    ToggleWorking(String),
    ArchiveProject,
    DuplicateProject,
    ExportProject,
    View(View),
    DecideApproval(bool),
    TopologySelect(String),
    TopologyPan(f32, f32),
    TopologyZoom(iced_core::Point, f32),
    TopologyFit,
    TopologyReset,
    TopologyInspector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// The transcript and composer.
    Session,
    /// The agent spawn tree and its inspector.
    Agents,
    /// The workspace permission graph and its inspector.
    Filesystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Health,
    Catalog,
    Projects,
    Approvals,
    Preflight,
    SaveProject,
    StartSession,
    StopSession,
    SendPrompt,
    CancelPrompt,
    RequestApproval,
    DecideApproval,
    RevokeGrant,
    ArchiveProject,
    DuplicateProject,
    PickWorkspace,
    PickKore,
    PickLlama,
    PickModel,
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkRow {
    speaker: String,
    body: String,
    kind: String,
}

#[derive(Debug, Clone)]
struct CatalogEntry {
    kind: String,
    label: String,
}

fn event_timestamp(event: &Value) -> Option<u64> {
    event["timestamp_unix_ms"].as_u64()
}

fn format_event_time(timestamp_ms: u64) -> Option<String> {
    let utc = chrono::DateTime::from_timestamp_millis(i64::try_from(timestamp_ms).ok()?)?;
    let rendered = utc
        .with_timezone(&chrono::Local)
        .format("%I:%M:%S %p")
        .to_string();
    Some(rendered.strip_prefix('0').unwrap_or(&rendered).to_owned())
}

fn format_elapsed(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        format!("{milliseconds} ms")
    } else if milliseconds < 60_000 {
        let seconds = milliseconds as f64 / 1_000.0;
        if milliseconds < 10_000 {
            format!("{seconds:.1} s")
        } else {
            format!("{seconds:.0} s")
        }
    } else {
        let minutes = milliseconds / 60_000;
        let seconds = ((milliseconds % 60_000) + 500) / 1_000;
        format!("{minutes}m {seconds}s")
    }
}

fn message_meta(label: &str, event: &Value, started_at: Option<u64>) -> String {
    let timestamp = event_timestamp(event);
    let mut parts = vec![label.to_owned()];
    if let Some(time) = timestamp.and_then(format_event_time) {
        parts.push(time);
    }
    if let (Some(started), Some(finished)) = (started_at, timestamp)
        && finished >= started
    {
        parts.push(format!(
            "responded in {}",
            format_elapsed(finished - started)
        ));
    }
    parts.join(" · ")
}

fn embedded_json(value: &Value) -> Option<Value> {
    match value {
        Value::String(value) => serde_json::from_str(value).ok(),
        Value::Object(_) | Value::Array(_) => Some(value.clone()),
        _ => None,
    }
}

fn broker_catalog(events: &[&Value]) -> BTreeMap<u64, CatalogEntry> {
    let mut entries = BTreeMap::new();
    for event in events {
        if event["kind"] != "inference"
            || event["payload"]["kind"] != "Input"
            || event["payload"]["role"] != "Broker"
        {
            continue;
        }
        let Some(input) = embedded_json(&event["payload"]["payload"]) else {
            continue;
        };
        for tuple in input["offer_catalog"].as_array().into_iter().flatten() {
            let Some(tuple) = tuple.as_array() else {
                continue;
            };
            if let (Some(index), Some(kind), Some(label)) = (
                tuple.first().and_then(Value::as_u64),
                tuple.get(1).and_then(Value::as_str),
                tuple.get(2).and_then(Value::as_str),
            ) {
                entries.insert(
                    index,
                    CatalogEntry {
                        kind: kind.into(),
                        label: label.into(),
                    },
                );
            }
        }
    }
    entries
}

fn display_resource(label: &str) -> String {
    label
        .strip_prefix("workspace/")
        .unwrap_or(label)
        .trim_start_matches('/')
        .to_owned()
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn format_broker_action(action: &Value, catalog: &BTreeMap<u64, CatalogEntry>) -> String {
    let action_name = action["action"].as_str().unwrap_or("decision");
    if action_name == "create_agent" {
        let selected = action["selectors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .filter_map(|index| catalog.get(&index).map(|entry| (index, entry)))
            .collect::<Vec<_>>();
        let tools = selected
            .iter()
            .filter(|(_, entry)| entry.kind == "x")
            .map(|(_, entry)| display_resource(&entry.label))
            .collect::<Vec<_>>();
        let resources = selected
            .iter()
            .filter(|(_, entry)| matches!(entry.kind.as_str(), "o" | "v"))
            .map(|(_, entry)| display_resource(&entry.label))
            .collect::<Vec<_>>();
        let unresolved = action["selectors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .filter(|index| !catalog.contains_key(index))
            .map(|index| format!("#{index}"))
            .collect::<Vec<_>>();
        let mut visible_resources = resources;
        visible_resources.extend(unresolved);
        return format!(
            "Tool_use: create_agent(Agent 1, [{}], [{}])",
            tools.join(", "),
            visible_resources.join(", ")
        );
    }
    if action_name == "refused" {
        let reason = action["reason"]
            .as_str()
            .or_else(|| action["detail"].as_str())
            .unwrap_or("the requested authority was not available");
        return format!("Refused: {reason}");
    }

    let arguments = action
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter(|(key, _)| key.as_str() != "action")
        .filter_map(|(key, value)| scalar(value).map(|value| format!("{key}: {value}")))
        .collect::<Vec<_>>();
    format!("Tool_use: {action_name}({})", arguments.join(", "))
}

fn clean_tool_argument(argument: &str) -> String {
    let argument = argument.trim();
    for prefix in ["Path(\"", "Object(\"", "Text(\""] {
        if let Some(value) = argument
            .strip_prefix(prefix)
            .and_then(|value| value.strip_suffix("\")"))
        {
            return display_resource(value);
        }
    }
    display_resource(argument.trim_matches('"'))
}

fn tool_arguments(value: &Value) -> Vec<String> {
    if let Some(arguments) = value.as_array() {
        return arguments
            .iter()
            .filter_map(|argument| match argument {
                Value::String(argument) => Some(clean_tool_argument(argument)),
                Value::Object(argument) => argument
                    .get("value")
                    .or_else(|| argument.get("path"))
                    .and_then(Value::as_str)
                    .map(clean_tool_argument),
                _ => scalar(argument),
            })
            .collect();
    }
    let Some(arguments) = value.as_str() else {
        return Vec::new();
    };
    arguments
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(", ")
        .filter(|argument| !argument.is_empty())
        .map(clean_tool_argument)
        .collect()
}

fn decision_payload(event: &Value) -> Option<Value> {
    let payload = &event["payload"];
    if let Some(inner) = embedded_json(&payload["payload"]) {
        Some(inner)
    } else if payload.is_object() {
        Some(payload.clone())
    } else {
        None
    }
}

fn meaningful_error(event: &Value) -> String {
    let payload = &event["payload"];
    payload["message"]
        .as_str()
        .or_else(|| payload["detail"].as_str())
        .or_else(|| payload.as_str())
        .unwrap_or("The request could not be completed.")
        .to_owned()
}

fn work_rows(events: &[&Value]) -> Vec<WorkRow> {
    let catalog = broker_catalog(events);
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();

    for event in events {
        let event_kind = event["kind"].as_str().unwrap_or("event");
        let row = match event_kind {
            "broker_proposal" => decision_payload(event).map(|action| WorkRow {
                speaker: "Broker".into(),
                body: format_broker_action(&action, &catalog),
                kind: "decision".into(),
            }),
            "tool_call" => decision_payload(event).map(|call| {
                let command = call["command"]
                    .as_str()
                    .or_else(|| call["tool"].as_str())
                    .unwrap_or("tool");
                let arguments = tool_arguments(&call["arguments"]);
                WorkRow {
                    speaker: "Agent 1".into(),
                    body: format!(
                        "Tool_use: {command}{}",
                        if arguments.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", arguments.join(" "))
                        }
                    ),
                    kind: "decision".into(),
                }
            }),
            "error" => Some(WorkRow {
                speaker: "Error".into(),
                body: meaningful_error(event),
                kind: "error".into(),
            }),
            _ => None,
        };
        if let Some(row) = row
            && seen.insert((row.speaker.clone(), row.body.clone()))
        {
            rows.push(row);
        }
    }
    rows
}

fn session_work_rows(events: &[Value]) -> Vec<WorkRow> {
    let mut rows = Vec::new();
    for user in events.iter().filter(|event| event["kind"] == "user_prompt") {
        let prompt_id = user["prompt_id"].as_str().unwrap_or_default();
        let prompt_events = events
            .iter()
            .filter(|event| event["prompt_id"].as_str() == Some(prompt_id))
            .collect::<Vec<_>>();
        rows.extend(work_rows(&prompt_events));
    }
    rows
}

/// A panel carrying a single hairline rule on one edge.
///
/// The display list has no per-edge border width, so the rule is drawn as a
/// full border and the panel is clipped by its neighbours on the other three
/// sides. This is exactly how the sidebar and header read in the design.
fn divider_right(background: u32) -> iced_widget::container::Style {
    let mut style = theme::panel(background, None, 0.0);
    style.border = theme::border(theme::BORDER, 0.0);
    style
}

fn divider_bottom(background: u32) -> iced_widget::container::Style {
    divider_right(background)
}

fn divider_left(background: u32) -> iced_widget::container::Style {
    divider_right(background)
}

fn input_style(
    _theme: &Theme,
    status: iced_widget::text_input::Status,
) -> iced_widget::text_input::Style {
    let focused = matches!(status, iced_widget::text_input::Status::Focused { .. });
    iced_widget::text_input::Style {
        background: Background::Color(theme::rgb(theme::BACKGROUND)),
        border: Border {
            color: rgb(if focused { 0x596573 } else { 0x343b45 }),
            width: 1.0,
            radius: 8.0.into(),
        },
        placeholder: theme::rgb(theme::BORDER_DIM),
        value: theme::rgb(theme::TEXT),
        selection: theme::rgb(theme::MAGENTA),
    }
}

struct Request {
    kind: RequestKind,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

#[derive(Debug)]
enum UserEvent {
    Response(RequestKind, Result<ApiResponse, String>),
    Poll(Result<Vec<Value>, String>),
}

#[derive(Debug)]
struct ApiResponse {
    content_type: String,
    body: Vec<u8>,
}

impl ApiResponse {
    fn json(self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|error| error.to_string())
    }
}

struct Network {
    sender: mpsc::Sender<Request>,
    session: mpsc::Sender<PollTarget>,
}

struct PollTarget {
    session: Option<String>,
    active: bool,
}

impl Network {
    fn start(server: SocketAddr, proxy: EventLoopProxy<UserEvent>) -> Self {
        let (sender, receiver) = mpsc::channel::<Request>();
        let request_proxy = proxy.clone();
        thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                let result = http(server, request.method, &request.path, request.body.as_ref());
                let _ = request_proxy.send_event(UserEvent::Response(request.kind, result));
            }
        });

        let (session_sender, session_receiver) = mpsc::channel::<PollTarget>();
        thread::spawn(move || {
            let mut session = None;
            let mut active = false;
            let mut cursor = 0u64;
            loop {
                while let Ok(next) = session_receiver.try_recv() {
                    if session != next.session {
                        session = next.session;
                        cursor = 0;
                    }
                    active = next.active;
                }
                let Some(id) = session.as_deref() else {
                    match session_receiver.recv() {
                        Ok(next) => {
                            if session != next.session {
                                session = next.session;
                                cursor = 0;
                            }
                            active = next.active;
                            continue;
                        }
                        Err(_) => break,
                    }
                };
                match http(
                    server,
                    "GET",
                    &format!("/api/v1/sessions/{id}/events?after={cursor}"),
                    None,
                )
                .and_then(parse_sse)
                {
                    Ok(events) => {
                        if let Some(sequence) = events
                            .iter()
                            .filter_map(|event| event["sequence"].as_u64())
                            .max()
                        {
                            cursor = cursor.max(sequence);
                        }
                        if !events.is_empty() {
                            let _ = proxy.send_event(UserEvent::Poll(Ok(events)));
                        }
                    }
                    Err(error) => {
                        let _ = proxy.send_event(UserEvent::Poll(Err(error)));
                    }
                }
                thread::sleep(if active {
                    ACTIVE_POLL_INTERVAL
                } else {
                    IDLE_POLL_INTERVAL
                });
            }
        });
        Self {
            sender,
            session: session_sender,
        }
    }

    fn request(&self, request: Request) {
        let _ = self.sender.send(request);
    }

    fn poll(&self, session: Option<String>, active: bool) {
        let _ = self.session.send(PollTarget { session, active });
    }
}

struct Model {
    network: Network,
    connected: bool,
    projects: Vec<Value>,
    catalog: Value,
    approvals: Value,
    selected: Option<usize>,
    launch_project: Option<String>,
    auto_open_pending: bool,
    session: Option<String>,
    prompt_id: Option<String>,
    prompt: text_editor::Content<AxRenderer<System>>,
    events: Vec<Value>,
    selectable_text: BTreeMap<String, text_editor::Content<AxRenderer<System>>>,
    expanded_work: BTreeSet<String>,
    view: View,
    topology: view::State,
    topology_graph: model::Graph,
    topology_placements: Vec<layout::Placement>,
    editor: Option<text_editor::Content<AxRenderer<System>>>,
    wizard_page: u8,
    preflight: Option<String>,
    approval: Option<Value>,
    status: String,
    busy: bool,
    restart_after_stop: bool,
}

impl Model {
    #[cfg(test)]
    fn new(network: Network) -> Self {
        Self::with_launch_project(network, None)
    }

    fn with_launch_project(network: Network, launch_project: Option<String>) -> Self {
        let model = Self {
            network,
            connected: false,
            projects: Vec::new(),
            catalog: Value::Null,
            approvals: Value::Null,
            selected: None,
            launch_project,
            auto_open_pending: true,
            session: None,
            prompt_id: None,
            prompt: text_editor::Content::new(),
            events: Vec::new(),
            selectable_text: BTreeMap::new(),
            expanded_work: BTreeSet::new(),
            view: View::Session,
            topology: view::State::default(),
            topology_graph: model::Graph::default(),
            topology_placements: Vec::new(),
            editor: None,
            wizard_page: 0,
            preflight: None,
            approval: None,
            status: "Connecting to the vKore daemon…".into(),
            busy: false,
            restart_after_stop: false,
        };
        model.reload();
        model
    }

    fn reload(&self) {
        for (kind, path) in [
            (RequestKind::Health, "/api/v1/health"),
            (RequestKind::Catalog, "/api/v1/catalog"),
            (RequestKind::Projects, "/api/v1/projects"),
            (RequestKind::Approvals, "/api/v1/approvals"),
        ] {
            self.network.request(Request {
                kind,
                method: "GET",
                path: path.into(),
                body: None,
            });
        }
    }

    /// Refreshes everything derived from the event stream.
    ///
    /// The selectable transcript and the topology graph are both projections
    /// of the same events. Refreshing them together is what keeps a view from
    /// showing one and not the other.
    fn events_changed(&mut self) {
        self.sync_selectable_text();
        self.refresh_topology();
    }

    /// Rebuilds the derived graph and its placement.
    ///
    /// Derivation walks the whole stream, so it happens when the stream
    /// changes rather than once per frame.
    fn refresh_topology(&mut self) {
        self.topology_graph = model::Graph::from_events(&self.events, self.session.is_some());
        self.topology_placements =
            layout::tidy(&self.topology_graph.parents(), layout::Metrics::AGENTS);
        // A changed lineage invalidates a fit taken against the old one.
        self.topology.fitted = false;
        if let Some(selected) = self.topology.selected.clone()
            && self.topology_graph.index_of(&selected).is_none()
        {
            self.topology.selected = None;
        }
    }

    /// The area a topology canvas gets, once the shell has taken its share.
    fn canvas_area(&self, viewport: Size) -> Size {
        let sidebar = if viewport.width <= 720.0 { 0.0 } else { 248.0 };
        let inspector = if self.topology.inspector { 300.0 } else { 0.0 };
        Size::new(
            (viewport.width - sidebar - inspector).max(1.0),
            (viewport.height - 64.0).max(1.0),
        )
    }

    /// Fits a topology view the first time it is shown at a given size.
    ///
    /// Fitting needs the viewport, which only the shell knows, so it happens
    /// here rather than while building the view.
    fn fit_topology(&mut self, viewport: Size) {
        if self.view == View::Session || self.topology.fitted {
            return;
        }
        let area = self.canvas_area(viewport);
        let placements = self.topology_placements.clone();
        self.topology.fit(&placements, area);
    }

    fn project(&self) -> Option<&Value> {
        self.selected.and_then(|index| self.projects.get(index))
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::SelectProject(index) => {
                if self.session.is_none() {
                    self.selected = Some(index);
                    self.events.clear();
                    self.selectable_text.clear();
                    self.status = "Opening a fresh isolated Kore sandbox…".into();
                    self.update(Message::ToggleSession);
                }
            }
            Message::NewProject => {
                let root = std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.editor = Some(
                    text_editor::Content::with_text(&serde_json::to_string_pretty(&json!({
                        "schema_version":1,
                        "id":"my-project",
                        "name":"My project",
                        "workspace":{"root":root,"state":"","access":"read_only","exclusions":[".git/objects"],"max_write_bytes":0,"max_write_files":0},
                        "inference":{"backend":"deterministic","kore_image":null,"llama_server":null,"model":null,"extra_args":[],"context_tokens":32768,"output_tokens":512,"threads":4,"slots":3},
                        "addons":[],
                        "approvals":{"default_scope":"once","confirm_external_writes":true,"confirm_destructive":true,"standing_policy_enabled":false},
                        "retention":{"transcript_days":30,"clean_kv_on_close":true,"retain_raw_model_output":false},
                        "resources":{"memory_mib":2048,"virtual_cpus":2,"prompt_seconds":300,"agent_limit":4},
                        "archived":false
                    }))
                    .unwrap()),
                );
                self.wizard_page = 0;
                self.preflight = None;
            }
            Message::EditProject => {
                if let Some(project) = self.project() {
                    self.editor = Some(text_editor::Content::with_text(
                        &serde_json::to_string_pretty(project).unwrap_or_default(),
                    ));
                    self.wizard_page = 0;
                    self.preflight = None;
                }
            }
            Message::ConfigAction(action) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.perform(action);
                    self.preflight = None;
                }
            }
            Message::WizardField(pointer, value) => self.update_config(pointer, &value),
            Message::WizardToggle(pointer) => self.toggle_config(pointer),
            Message::WizardChoice(pointer, value) => self.update_config(pointer, value),
            Message::WizardAddon(id) => self.toggle_addon(&id),
            Message::WizardPage(page) => {
                self.wizard_page = page.min(4);
                if self.wizard_page == 4 {
                    self.preflight = None;
                }
            }
            Message::Preflight => self.submit_editor(RequestKind::Preflight),
            Message::SaveProject => self.submit_editor(RequestKind::SaveProject),
            Message::Browse(kind) => {
                let directory = kind == RequestKind::PickWorkspace;
                self.network.request(Request {
                    kind,
                    method: "POST",
                    path: "/api/v1/picker".into(),
                    body: Some(json!({"kind":if directory {"directory"} else {"file"}})),
                });
            }
            Message::CloseEditor => {
                self.editor = None;
                self.preflight = None;
            }
            Message::ToggleSession => {
                if let Some(session) = self.session.clone() {
                    self.busy = true;
                    self.network.request(Request {
                        kind: RequestKind::StopSession,
                        method: "DELETE",
                        path: format!("/api/v1/sessions/{session}"),
                        body: None,
                    });
                } else if let Some(project) = self.project() {
                    if let Some(addon) = project["addons"]
                        .as_array()
                        .and_then(|addons| addons.iter().find(|addon| addon["enabled"] == true))
                    {
                        let addon_id = addon["id"].as_str().unwrap_or_default().to_owned();
                        let tool = addon["tool_ceiling"]
                            .as_array()
                            .and_then(|tools| tools.first())
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let project_id = project["id"].as_str().unwrap_or_default().to_owned();
                        self.busy = true;
                        self.network.request(Request {
                            kind: RequestKind::RequestApproval,
                            method: "POST",
                            path: format!("/api/v1/projects/{project_id}/approvals"),
                            body: Some(json!({"session_id":"project-start","addon_id":addon_id,"tool":tool})),
                        });
                    } else {
                        self.start_session();
                    }
                }
            }
            Message::CleanSession => {
                if self.session.is_some() {
                    self.restart_after_stop = true;
                    self.update(Message::ToggleSession);
                } else {
                    self.update(Message::ToggleSession);
                }
            }
            Message::PromptAction(action) => self.prompt.perform(action),
            Message::SelectText(key, action) => {
                if !action.is_edit()
                    && let Some(content) = self.selectable_text.get_mut(&key)
                {
                    content.perform(action);
                }
            }
            Message::SendPrompt => self.send_prompt(),
            Message::CancelPrompt => {
                if let (Some(session), Some(prompt_id)) = (&self.session, &self.prompt_id) {
                    self.network.request(Request {
                        kind: RequestKind::CancelPrompt,
                        method: "POST",
                        path: format!("/api/v1/sessions/{session}/prompts/{prompt_id}/cancel"),
                        body: Some(json!({"api_version":1,"prompt_id":prompt_id})),
                    });
                }
            }
            Message::ToggleWorking(prompt_id) => {
                if !self.expanded_work.remove(&prompt_id) {
                    self.expanded_work.insert(prompt_id);
                }
            }
            Message::ArchiveProject => {
                if let Some(project) = self.project() {
                    let id = project["id"].as_str().unwrap_or_default();
                    self.network.request(Request {
                        kind: RequestKind::ArchiveProject,
                        method: "POST",
                        path: format!("/api/v1/projects/{id}/archive"),
                        body: Some(json!({"archived":true})),
                    });
                }
            }
            Message::DuplicateProject => {
                if let Some(project) = self.project() {
                    let source = project["id"].as_str().unwrap_or_default();
                    let suffix = now_id();
                    self.network.request(Request {
                        kind: RequestKind::DuplicateProject,
                        method: "POST",
                        path: format!("/api/v1/projects/{source}/duplicate"),
                        body: Some(json!({
                            "id":format!("{source}-copy-{suffix}"),
                            "name":format!("{} copy", project["name"].as_str().unwrap_or("Project"))
                        })),
                    });
                }
            }
            Message::ExportProject => {
                if let Some(project) = self.project() {
                    let id = project["id"].as_str().unwrap_or("project");
                    let path = format!("{id}.vkore.json");
                    match std::fs::write(
                        &path,
                        serde_json::to_vec_pretty(project).unwrap_or_default(),
                    ) {
                        Ok(()) => self.status = format!("Exported {path}"),
                        Err(error) => self.status = format!("Export failed: {error}"),
                    }
                }
            }
            Message::View(value) => {
                // Entering a topology view fits it once; returning keeps the
                // reader's own pan and zoom.
                if value != View::Session && self.view == View::Session {
                    self.topology.fitted = false;
                }
                self.view = value;
            }
            Message::TopologySelect(id) => {
                self.topology.selected =
                    (self.topology.selected.as_deref() != Some(id.as_str())).then_some(id);
            }
            Message::TopologyPan(dx, dy) => {
                self.topology.tx += dx;
                self.topology.ty += dy;
            }
            Message::TopologyZoom(pointer, factor) => {
                self.topology.zoom_about(pointer, factor);
            }
            Message::TopologyFit => self.topology.fitted = false,
            Message::TopologyReset => {
                let selected = self.topology.selected.take();
                self.topology = view::State::default();
                self.topology.selected = selected;
            }
            Message::TopologyInspector => {
                self.topology.inspector = !self.topology.inspector;
            }
            Message::DecideApproval(allow) => {
                if let Some(request) = self.approval.as_ref() {
                    let id = request["id"].as_str().unwrap_or_default();
                    self.busy = true;
                    self.network.request(Request {
                        kind: RequestKind::DecideApproval,
                        method: "POST",
                        path: format!("/api/v1/approvals/{id}/decision"),
                        body: Some(
                            json!({"allow":allow,"lifetime":"once","ui_session":"axiom-ui"}),
                        ),
                    });
                }
            }
        }
    }

    fn submit_editor(&mut self, kind: RequestKind) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        match serde_json::from_str(&source.text()) {
            Ok(body) => {
                self.busy = true;
                self.network.request(Request {
                    kind,
                    method: "POST",
                    path: if kind == RequestKind::Preflight {
                        "/api/v1/projects/preflight".into()
                    } else {
                        "/api/v1/projects".into()
                    },
                    body: Some(body),
                });
            }
            Err(error) => self.preflight = Some(format!("Invalid JSON: {error}")),
        }
    }

    fn update_config(&mut self, pointer: &str, value: &str) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&source.text()) else {
            return;
        };
        let Some(slot) = config.pointer_mut(pointer) else {
            return;
        };
        *slot = match slot {
            Value::Number(_) => value
                .parse::<u64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::from(0)),
            Value::Array(_) if pointer == "/inference/extra_args" => Value::Array(
                value
                    .split_whitespace()
                    .map(|part| Value::String(part.to_owned()))
                    .collect(),
            ),
            Value::Null if value.is_empty() => Value::Null,
            _ => Value::String(value.to_owned()),
        };
        self.editor = Some(text_editor::Content::with_text(
            &serde_json::to_string_pretty(&config).unwrap_or_default(),
        ));
        self.preflight = None;
    }

    fn toggle_config(&mut self, pointer: &str) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&source.text()) else {
            return;
        };
        let Some(slot) = config.pointer_mut(pointer) else {
            return;
        };
        *slot = Value::Bool(!slot.as_bool().unwrap_or(false));
        self.editor = Some(text_editor::Content::with_text(
            &serde_json::to_string_pretty(&config).unwrap_or_default(),
        ));
        self.preflight = None;
    }

    fn toggle_addon(&mut self, id: &str) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&source.text()) else {
            return;
        };
        let Some(addons) = config["addons"].as_array_mut() else {
            return;
        };
        if let Some(addon) = addons.iter_mut().find(|addon| addon["id"] == id) {
            addon["enabled"] = Value::Bool(!addon["enabled"].as_bool().unwrap_or(false));
        } else if let Some(catalog) = self.catalog["addons"]
            .as_array()
            .and_then(|entries| entries.iter().find(|addon| addon["id"] == id))
        {
            let tools = catalog["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tool| tool["name"].as_str())
                .collect::<Vec<_>>();
            addons.push(json!({
                "id": id,
                "enabled": true,
                "mediation": catalog["domain"].as_str().unwrap_or("host"),
                "tool_ceiling": tools,
            }));
        }
        self.editor = Some(text_editor::Content::with_text(
            &serde_json::to_string_pretty(&config).unwrap_or_default(),
        ));
        self.preflight = None;
    }

    fn start_session(&mut self) {
        let Some(project) = self.project() else {
            return;
        };
        let project_id = project["id"].as_str().unwrap_or_default().to_owned();
        let session_id = format!("ui-{project_id}-{}", now_id());
        self.busy = true;
        self.network.request(Request {
            kind: RequestKind::StartSession,
            method: "POST",
            path: "/api/v1/sessions".into(),
            body: Some(json!({"api_version":1,"project_id":project_id,"session_id":session_id})),
        });
    }

    fn send_prompt(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let text = self.prompt.text();
        let text = text.trim();
        if text.is_empty() || self.prompt_id.is_some() {
            return;
        }
        let prompt_id = format!("p-{}", now_id());
        self.expanded_work.insert(prompt_id.clone());
        self.prompt_id = Some(prompt_id.clone());
        self.busy = true;
        self.network.poll(Some(session.clone()), true);
        self.network.request(Request {
            kind: RequestKind::SendPrompt,
            method: "POST",
            path: format!("/api/v1/sessions/{session}/prompts"),
            body: Some(json!({
                "api_version":1,
                "prompt_id":prompt_id,
                "idempotency_key":format!("native-{}", now_id()),
                "text":text
            })),
        });
        self.prompt = text_editor::Content::new();
    }

    fn response(&mut self, kind: RequestKind, result: Result<ApiResponse, String>) {
        let result = result.and_then(ApiResponse::json);
        match (kind, result) {
            (RequestKind::Health, Ok(_)) => {
                self.connected = true;
                self.status = "Connected to the resident vKore broker.".into();
            }
            (RequestKind::Catalog, Ok(value)) => self.catalog = value,
            (RequestKind::Projects, Ok(Value::Array(projects))) => {
                self.projects = projects;
                if self
                    .selected
                    .is_some_and(|index| index >= self.projects.len())
                {
                    self.selected = None;
                }
                if self.auto_open_pending {
                    self.auto_open_pending = false;
                    let requested = self.launch_project.take();
                    let selected = requested.as_ref().and_then(|project_id| {
                        self.projects
                            .iter()
                            .position(|project| project["id"].as_str() == Some(project_id))
                    });
                    let only_active = (requested.is_none())
                        .then(|| {
                            let mut active =
                                self.projects.iter().enumerate().filter(|(_, project)| {
                                    !project["archived"].as_bool().unwrap_or(false)
                                });
                            let first = active.next().map(|(index, _)| index);
                            first.filter(|_| active.next().is_none())
                        })
                        .flatten();
                    if let Some(index) = selected.or(only_active) {
                        self.update(Message::SelectProject(index));
                    } else if let Some(project_id) = requested {
                        self.status = format!(
                            "Launch project {project_id:?} is not available from this daemon."
                        );
                    }
                }
            }
            (RequestKind::Approvals, Ok(value)) => self.approvals = value,
            (RequestKind::Preflight, Ok(value)) => {
                self.busy = false;
                self.preflight = Some(serde_json::to_string_pretty(&value).unwrap_or_default());
                self.status = "Preflight passed.".into();
            }
            (RequestKind::SaveProject, Ok(value)) => {
                self.busy = false;
                let id = value["id"].as_str().unwrap_or_default();
                if let Some(index) = self.projects.iter().position(|item| item["id"] == id) {
                    self.projects[index] = value;
                    self.selected = Some(index);
                } else {
                    self.projects.push(value);
                    self.selected = Some(self.projects.len() - 1);
                }
                self.editor = None;
                self.preflight = None;
                self.status = "Project saved after daemon preflight. Starting sandbox…".into();
                if self.session.is_some() {
                    self.restart_after_stop = true;
                }
                self.update(Message::ToggleSession);
            }
            (RequestKind::StartSession, Ok(value)) => {
                self.busy = false;
                self.session = value["session_id"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value["id"].as_str().map(str::to_owned));
                self.events.clear();
                self.selectable_text.clear();
                self.expanded_work.clear();
                self.prompt_id = None;
                self.network.poll(self.session.clone(), false);
                self.status = "Chat session ready. Kore starts with the first message.".into();
            }
            (RequestKind::StopSession, Ok(_)) => {
                self.busy = false;
                self.network.poll(None, false);
                self.session = None;
                self.prompt_id = None;
                self.expanded_work.clear();
                self.selectable_text.clear();
                self.status = "Routes retired and session KV erased.".into();
                for grant in self.approvals["grants"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|grant| grant["committed"].as_bool().unwrap_or(false))
                {
                    if let Some(id) = grant["id"].as_str() {
                        self.network.request(Request {
                            kind: RequestKind::RevokeGrant,
                            method: "POST",
                            path: format!("/api/v1/grants/{id}/revoke"),
                            body: Some(json!({"ui_session":"axiom-ui"})),
                        });
                    }
                }
                if self.restart_after_stop {
                    self.restart_after_stop = false;
                    self.update(Message::ToggleSession);
                } else {
                    self.reload();
                }
            }
            (RequestKind::SendPrompt, Ok(_)) => {
                self.busy = false;
                self.status = "Prompt accepted by the broker.".into();
            }
            (RequestKind::CancelPrompt, Ok(_)) => {
                self.status = "Cancellation requested.".into();
            }
            (RequestKind::RequestApproval, Ok(value)) => {
                self.busy = false;
                self.approval = value.get("request").cloned();
            }
            (RequestKind::DecideApproval, Ok(_)) => {
                self.busy = false;
                self.approval = None;
                self.reload();
                self.start_session();
            }
            (RequestKind::RevokeGrant, Ok(_)) => self.reload(),
            (RequestKind::ArchiveProject, Ok(_)) => {
                self.selected = None;
                self.reload();
                self.status = "Project archived.".into();
            }
            (RequestKind::DuplicateProject, Ok(value)) => {
                self.projects.push(value);
                self.selected = Some(self.projects.len() - 1);
                self.status = "Project duplicated. Starting sandbox…".into();
                self.update(Message::ToggleSession);
            }
            (
                kind @ (RequestKind::PickWorkspace
                | RequestKind::PickKore
                | RequestKind::PickLlama
                | RequestKind::PickModel),
                Ok(value),
            ) => {
                if let Some(path) = value["path"].as_str() {
                    self.apply_picker(kind, path);
                }
            }
            (_, Err(error)) => {
                self.busy = false;
                self.status = format!("Request refused: {error}");
                if kind == RequestKind::Preflight {
                    self.preflight = Some(self.status.clone());
                }
                if kind == RequestKind::SendPrompt {
                    self.prompt_id = None;
                    self.network.poll(self.session.clone(), false);
                }
            }
            _ => {}
        }
    }

    fn poll_result(&mut self, result: Result<Vec<Value>, String>) {
        match result {
            Ok(events) => {
                let mut prompt_finished = false;
                for event in events {
                    if event["kind"] == "prompt_terminal"
                        && event["prompt_id"].as_str() == self.prompt_id.as_deref()
                    {
                        self.prompt_id = None;
                        self.busy = false;
                        prompt_finished = true;
                    }
                    self.events.push(event);
                }
                if prompt_finished {
                    self.network.poll(self.session.clone(), false);
                }
                self.events_changed();
            }
            Err(error) => self.status = format!("Event stream interrupted: {error}"),
        }
    }

    fn sync_selectable_text(&mut self) {
        let mut desired = BTreeMap::new();
        for user in self
            .events
            .iter()
            .filter(|event| event["kind"] == "user_prompt")
        {
            let prompt_id = user["prompt_id"].as_str().unwrap_or_default();
            // The meta line is drawn above the card as its own mono label, so
            // the selectable content is the message body alone.
            desired.insert(
                format!("prompt:{prompt_id}:user"),
                user["payload"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
            let prompt_events = self
                .events
                .iter()
                .filter(|event| event["prompt_id"].as_str() == Some(prompt_id))
                .collect::<Vec<_>>();
            for (index, row) in work_rows(&prompt_events).into_iter().enumerate() {
                // The speaker is drawn as its own label above the body so it
                // can carry the row's colour; only the body is selectable.
                desired.insert(format!("prompt:{prompt_id}:work:{index}"), row.body);
            }
            if let Some(assistant) = prompt_events.iter().find(|event| {
                event["kind"] == "assistant" && event["public"].as_bool().unwrap_or(false)
            }) {
                desired.insert(
                    format!("prompt:{prompt_id}:assistant"),
                    assistant["payload"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                );
            }
        }

        let activity = session_work_rows(&self.events);
        for (index, row) in activity.iter().enumerate() {
            desired.insert(
                format!("activity:{index}"),
                format!("{}:\n{}", row.speaker, row.body),
            );
        }
        if let Some(row) = activity.iter().rev().find(|row| row.speaker == "Broker") {
            desired.insert(
                "authority:latest".into(),
                format!("{}:\n{}", row.speaker, row.body),
            );
        }

        self.selectable_text
            .retain(|key, _| desired.contains_key(key));
        for (key, value) in desired {
            let unchanged = self
                .selectable_text
                .get(&key)
                .is_some_and(|content| content.text() == value);
            if !unchanged {
                self.selectable_text
                    .insert(key, text_editor::Content::with_text(&value));
            }
        }
    }

    /// Read-only conversation text the reader can select and copy.
    ///
    /// The face is explicit rather than derived: prose carries the body face,
    /// while machine-shaped content -- logs, tool arguments, JSON -- carries
    /// mono so its columns line up.
    fn selectable<'a>(
        &'a self,
        key: &str,
        size: f32,
        color: u32,
        face: iced_core::Font,
    ) -> UiElement<'a> {
        let key_owned = key.to_owned();
        let content = self
            .selectable_text
            .get(key)
            .expect("selectable conversation text must be synchronized");
        text_editor(content)
            .on_action(move |action| Message::SelectText(key_owned.clone(), action))
            .padding(0)
            .size(size)
            .font(face)
            .style(widgets::selectable(color))
            .into()
    }

    fn apply_picker(&mut self, kind: RequestKind, path: &str) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&editor.text()) else {
            self.preflight = Some("Fix the project JSON before applying a selected path.".into());
            return;
        };
        let pointer = match kind {
            RequestKind::PickWorkspace => "/workspace/root",
            RequestKind::PickKore => "/inference/kore_image",
            RequestKind::PickLlama => "/inference/llama_server",
            RequestKind::PickModel => "/inference/model",
            _ => return,
        };
        if let Some(slot) = config.pointer_mut(pointer) {
            *slot = Value::String(path.into());
            self.editor = Some(text_editor::Content::with_text(
                &serde_json::to_string_pretty(&config).unwrap_or_default(),
            ));
            self.preflight = None;
        }
    }

    fn view(&self, viewport: Size) -> UiElement<'_> {
        let base: UiElement<'_> = if viewport.width <= 720.0 {
            self.main_panel()
        } else if viewport.width <= 1050.0 {
            row![self.sidebar(210.0), self.main_panel()]
                .height(Length::Fill)
                .into()
        } else {
            row![self.sidebar(248.0), self.main_panel()]
                .height(Length::Fill)
                .into()
        };
        if self.editor.is_some() {
            stack![base, self.modal_layer(self.editor_panel())]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else if self.approval.is_some() {
            stack![base, self.modal_layer(self.approval_panel())]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            base
        }
    }

    fn modal_layer<'a>(&self, content: UiElement<'a>) -> UiElement<'a> {
        container(content)
            .center(Length::Fill)
            .style(|_| {
                let mut overlay = theme::panel(theme::BACKGROUND, None, 0.0);
                if let Some(Background::Color(color)) = overlay.background.as_mut() {
                    color.a = 0.86;
                }
                overlay
            })
            .into()
    }

    fn sidebar(&self, width: f32) -> UiElement<'_> {
        // The mark is the one illustration in the system and needs an image
        // path the display list does not yet carry, so the brand row is the
        // wordmark alone until that lands.
        let brand = container(
            text("KORE AGENTS")
                .font(fonts::DISPLAY)
                .size(15.0)
                .color(theme::rgb(theme::TEXT)),
        )
        .padding([14, 14]);
        let mut projects = column![].spacing(theme::SPACE_1);
        for (index, project) in self
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| !p["archived"].as_bool().unwrap_or(false))
        {
            let active = self.selected == Some(index);
            let backend = project["inference"]["backend"]
                .as_str()
                .unwrap_or("deterministic")
                .replace('_', " ");
            let access = project["workspace"]["access"]
                .as_str()
                .unwrap_or("read only")
                .replace('_', " ");
            // A 5x5 square, not a circle: the system marks state with a mark,
            // never with a fill behind the row.
            let dot = container(space::Space::new().width(5).height(5)).style(move |_| {
                theme::panel(
                    if active {
                        theme::MAGENTA
                    } else {
                        theme::BORDER_DIM
                    },
                    None,
                    1.0,
                )
            });
            let label = column![
                widgets::body(
                    project["name"].as_str().unwrap_or("Project").to_owned(),
                    theme::SIZE_SM,
                    if active {
                        theme::TEXT
                    } else {
                        theme::TEXT_SECONDARY
                    },
                ),
                widgets::mono(format!("{backend} · {access}"), theme::TEXT_MUTED),
            ]
            .spacing(2);
            projects = projects.push(
                button(
                    row![dot, label]
                        .spacing(theme::SPACE_2)
                        .align_y(alignment::Vertical::Center),
                )
                .width(Length::Fill)
                .padding([9, 10])
                .style(widgets::button_style(widgets::Button::Row(active)))
                .on_press(Message::SelectProject(index)),
            );
        }
        let project_idle = self.project().is_some() && self.session.is_none();
        let project_editable = self.project().is_some() && self.prompt_id.is_none() && !self.busy;
        let nav = |label, message| {
            button(widgets::label(label, theme::TEXT_MUTED))
                .padding([7, 8])
                .width(Length::Fill)
                .style(widgets::button_style(widgets::Button::Ghost))
                .on_press_maybe(message)
        };
        let body = column![
            brand,
            container(widgets::label("Projects", theme::TEXT_MUTED)).padding(iced_core::Padding {
                top: 10.0,
                right: 14.0,
                bottom: 6.0,
                left: 14.0
            }),
            container(scrollable(projects).height(Length::Fill)).padding([0, 10]),
            container(
                button(widgets::label("New project", theme::TEXT_SECONDARY))
                    .width(Length::Fill)
                    .padding([9, 10])
                    .style(widgets::button_style(widgets::Button::Secondary))
                    .on_press_maybe(self.session.is_none().then_some(Message::NewProject))
            )
            .padding(10),
            space::vertical(),
            container(space::Space::new().height(1).width(Length::Fill)).style(|_| theme::panel(
                theme::BORDER,
                None,
                0.0
            )),
            container(
                column![
                    nav(
                        "Project settings",
                        project_editable.then_some(Message::EditProject)
                    ),
                    nav(
                        "Duplicate project",
                        project_idle.then_some(Message::DuplicateProject)
                    ),
                    nav(
                        "Export configuration",
                        self.project().map(|_| Message::ExportProject)
                    ),
                    nav(
                        "Archive project",
                        project_idle.then_some(Message::ArchiveProject)
                    ),
                    nav("Transcript & audit", Some(Message::View(View::Session))),
                ]
                .spacing(1)
            )
            .padding(iced_core::Padding {
                top: 6.0,
                right: 10.0,
                bottom: 10.0,
                left: 10.0
            }),
        ];
        container(body)
            .width(width)
            .height(Length::Fill)
            .style(|_| divider_right(theme::BACKGROUND))
            .into()
    }

    fn main_panel(&self) -> UiElement<'_> {
        let project_name = self
            .project()
            .and_then(|p| p["name"].as_str())
            .unwrap_or("No project open");
        // The subtitle carries what status chips used to: backend, workspace
        // root, and either the live session id or that the sandbox is stopped.
        let subtitle = match self.project() {
            None => "create a project to begin".to_owned(),
            Some(project) => {
                let backend = project["inference"]["backend"]
                    .as_str()
                    .unwrap_or("deterministic")
                    .replace('_', " ");
                let root = project["workspace"]["root"]
                    .as_str()
                    .unwrap_or("no workspace");
                match self.session.as_deref() {
                    Some(session) => format!("{backend} · {root} · {session}"),
                    None => format!("{backend} · {root} · sandbox stopped"),
                }
            }
        };
        let view_button = |label: &'static str, target: View| {
            button(widgets::label(
                label,
                if self.view == target {
                    theme::TEXT
                } else {
                    theme::TEXT_MUTED
                },
            ))
            .height(30)
            .padding([0, 12])
            .style(widgets::button_style(widgets::Button::Choice(
                self.view == target,
            )))
            .on_press(Message::View(target))
        };
        let top = container(
            row![
                column![
                    widgets::body(project_name.to_owned(), theme::SIZE_SM, theme::TEXT)
                        .wrapping(iced_core::text::Wrapping::None)
                        .ellipsis(iced_core::text::Ellipsis::End),
                    widgets::mono(subtitle, theme::TEXT_MUTED)
                        .wrapping(iced_core::text::Wrapping::None)
                        .ellipsis(iced_core::text::Ellipsis::End),
                ]
                .spacing(2)
                .width(Length::Fill)
                .clip(true),
                button(widgets::label(
                    if self.session.is_some() {
                        "Stop"
                    } else {
                        "Start"
                    },
                    theme::TEXT_SECONDARY
                ))
                .height(30)
                .padding([0, 12])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press_maybe(
                    (self.project().is_some() && !self.busy).then_some(Message::ToggleSession)
                ),
                button(widgets::label("Clean", theme::ERROR))
                    .height(30)
                    .padding([0, 12])
                    .style(widgets::button_style(widgets::Button::Danger))
                    .on_press_maybe(self.session.as_ref().map(|_| Message::CleanSession)),
                view_button("Session", View::Session),
                view_button("Agents", View::Agents),
                view_button("Filesystem", View::Filesystem),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
        )
        .padding([0, 22])
        .height(64)
        .width(Length::Fill)
        .align_y(alignment::Vertical::Center)
        .style(|_| divider_bottom(theme::BACKGROUND));

        if self.view != View::Session {
            let graph = &self.topology_graph;
            let placements = &self.topology_placements;
            let (title, subtitle, body) = match self.view {
                View::Agents => (
                    "Agents",
                    "capability inheritance and live state",
                    self.agents_rail(),
                ),
                _ => (
                    "Filesystem",
                    "grants held against this workspace",
                    self.authority_rail(),
                ),
            };
            let mut layers = row![
                container(view::canvas_layer(graph, placements, &self.topology))
                    .width(Length::Fill)
                    .height(Length::Fill)
            ];
            if self.topology.inspector {
                layers = layers.push(self.inspector(title, subtitle, body));
            }
            return container(column![
                top,
                self.topology_controls(),
                layers.height(Length::Fill)
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::panel(theme::BACKGROUND, None, 0.0))
            .into();
        }

        // An empty harness stays empty. The composer sits at the bottom with
        // nothing above it: no headline, no helper copy, no status chip.
        let conversation: UiElement<'_> = if self.events.is_empty() {
            container(space::vertical()).height(Length::Fill).into()
        } else {
            let mut items = column![].spacing(theme::SPACE_4);
            let prompts = self
                .events
                .iter()
                .filter(|event| event["kind"] == "user_prompt");
            for user in prompts {
                let prompt_id = user["prompt_id"].as_str().unwrap_or_default();
                let prompt_events = self
                    .events
                    .iter()
                    .filter(|event| event["prompt_id"].as_str() == Some(prompt_id))
                    .collect::<Vec<_>>();
                let started_at = event_timestamp(user);
                let user_key = format!("prompt:{prompt_id}:user");
                items = items.push(
                    column![
                        widgets::label(message_meta("You", user, None), theme::TEXT_MUTED),
                        container(self.selectable(
                            &user_key,
                            theme::SIZE_SM,
                            theme::TEXT,
                            fonts::BODY
                        ))
                        .padding([13, 15])
                        .width(Length::Fill)
                        .style(|_| theme::panel(
                            theme::PANEL,
                            Some(theme::BORDER),
                            theme::RADIUS_LG
                        )),
                    ]
                    .spacing(theme::SPACE_2),
                );

                let rows = work_rows(&prompt_events);
                let step_count = rows.len();
                let terminal = prompt_events
                    .iter()
                    .find(|event| event["kind"] == "prompt_terminal");
                let active = terminal.is_none() && self.prompt_id.as_deref() == Some(prompt_id);
                let failed = terminal.is_some_and(|event| event["payload"]["status"] == "failed");
                let ended_at = terminal
                    .and_then(|event| event_timestamp(event))
                    .or_else(|| {
                        prompt_events
                            .last()
                            .and_then(|event| event_timestamp(event))
                    })
                    .or(started_at);
                let work_title = if active {
                    "Working".to_owned()
                } else if failed {
                    "Stopped · failed".to_owned()
                } else {
                    format!(
                        "Worked for {}",
                        format_elapsed(
                            ended_at
                                .unwrap_or_default()
                                .saturating_sub(started_at.unwrap_or_default())
                        )
                    )
                };
                let state_color = if active {
                    theme::MAGENTA
                } else if failed {
                    theme::ERROR
                } else {
                    theme::SUCCESS
                };
                let expanded = self.expanded_work.contains(prompt_id);
                let work_header = button(
                    row![
                        container(space::Space::new().width(5).height(5))
                            .style(move |_| theme::panel(state_color, None, 1.0)),
                        widgets::label(work_title, theme::TEXT),
                        space::horizontal(),
                        widgets::mono(
                            format!(
                                "{step_count} step{}",
                                if step_count == 1 { "" } else { "s" }
                            ),
                            theme::TEXT_MUTED
                        ),
                        widgets::mono(
                            if expanded { "\u{25b2}" } else { "\u{25bc}" },
                            theme::TEXT_MUTED
                        ),
                    ]
                    .spacing(9)
                    .align_y(alignment::Vertical::Center),
                )
                .padding([10, 13])
                .width(Length::Fill)
                .style(widgets::button_style(widgets::Button::Ghost))
                .on_press(Message::ToggleWorking(prompt_id.to_owned()));
                let mut timeline = column![work_header].spacing(0);
                if expanded {
                    let mut details = column![].spacing(theme::SPACE_2);
                    if rows.is_empty() {
                        details = details
                            .push(widgets::mono("waiting for the sandbox", theme::TEXT_MUTED));
                    }
                    for (index, work) in rows.into_iter().enumerate() {
                        // The label carries the row's meaning: the accent for
                        // authority and broker activity, error red for a
                        // failure, off-white for ordinary progress.
                        let label_color = match work.kind.as_str() {
                            "error" => theme::ERROR,
                            "authority" | "broker_proposal" | "capability" => theme::MAGENTA,
                            _ => theme::TEXT,
                        };
                        let body_color = if work.kind == "error" {
                            theme::ERROR
                        } else {
                            theme::TEXT_SECONDARY
                        };
                        let work_key = format!("prompt:{prompt_id}:work:{index}");
                        details = details.push(
                            container(
                                column![
                                    widgets::label(&work.speaker, label_color),
                                    self.selectable(
                                        &work_key,
                                        theme::SIZE_MONO,
                                        body_color,
                                        fonts::MONO
                                    ),
                                ]
                                .spacing(theme::SPACE_1),
                            )
                            .padding([8, 10])
                            .width(Length::Fill)
                            .style(|_| {
                                theme::panel(
                                    theme::BACKGROUND,
                                    Some(theme::BORDER),
                                    theme::RADIUS_SM,
                                )
                            }),
                        );
                    }
                    timeline = timeline.push(
                        container(details)
                            .padding([12, 13])
                            .width(Length::Fill)
                            .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), 0.0)),
                    );
                }
                items =
                    items.push(container(timeline).width(Length::Fill).style(|_| {
                        theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG)
                    }));

                if let Some(assistant) = prompt_events.iter().find(|event| {
                    event["kind"] == "assistant" && event["public"].as_bool().unwrap_or(false)
                }) {
                    let assistant_key = format!("prompt:{prompt_id}:assistant");
                    items = items.push(
                        column![
                            widgets::label(
                                message_meta("Validated assistant", assistant, started_at),
                                theme::TEXT_MUTED
                            ),
                            // The one magenta border in the turn: a validated
                            // answer is the only thing that earns the accent.
                            container(self.selectable(
                                &assistant_key,
                                theme::SIZE_SM,
                                theme::TEXT,
                                fonts::BODY
                            ))
                            .padding([13, 15])
                            .width(Length::Fill)
                            .style(|_| theme::panel(
                                theme::PANEL,
                                Some(theme::MAGENTA),
                                theme::RADIUS_LG
                            )),
                        ]
                        .spacing(theme::SPACE_2),
                    );
                }
            }
            container(scrollable(items).height(Length::Fill))
                .padding([28, 40])
                .height(Length::Fill)
                .into()
        };

        let send = if self.prompt_id.is_some() {
            button(widgets::label("Cancel", theme::ERROR))
                .height(30)
                .padding([0, 12])
                .style(widgets::button_style(widgets::Button::Danger))
                .on_press(Message::CancelPrompt)
        } else {
            button(widgets::label("Send", theme::MAGENTA))
                .height(30)
                .padding([0, 14])
                .style(widgets::button_style(widgets::Button::Primary))
                .on_press_maybe(
                    (self.session.is_some() && !self.prompt.text().trim().is_empty())
                        .then_some(Message::SendPrompt),
                )
        };
        let access = self
            .project()
            .and_then(|project| project["workspace"]["access"].as_str())
            .unwrap_or("read only")
            .replace('_', " ");
        let prompt = text_editor(&self.prompt)
            .placeholder("Describe the change")
            .on_action(Message::PromptAction)
            .key_binding(|press| {
                if matches!(press.key, keyboard::Key::Named(keyboard::key::Named::Enter))
                    && !press.modifiers.shift()
                {
                    Some(iced_core::text::editor::Binding::Custom(
                        Message::SendPrompt,
                    ))
                } else {
                    iced_core::text::editor::Binding::from_key_press(press)
                }
            })
            .height(72)
            .padding([14, 16])
            .size(theme::SIZE_SM)
            .font(fonts::BODY)
            .style(widgets::editor);
        let composer = container(column![
            prompt,
            container(
                row![
                    space::horizontal(),
                    container(widgets::label(access, theme::TEXT_MUTED))
                        .padding([3, 7])
                        .style(|_| theme::panel(
                            theme::BACKGROUND,
                            Some(theme::BORDER),
                            theme::RADIUS_SM
                        )),
                    send,
                ]
                .spacing(theme::SPACE_2)
                .align_y(alignment::Vertical::Center)
            )
            .padding(iced_core::Padding {
                top: 8.0,
                right: 10.0,
                bottom: 8.0,
                left: 16.0,
            })
            .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), 0.0)),
        ])
        .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG));
        let composer_wrap = container(composer)
            .padding(iced_core::Padding {
                top: 10.0,
                right: 40.0,
                bottom: 18.0,
                left: 40.0,
            })
            .width(Length::Fill)
            .style(|_| theme::panel(theme::BACKGROUND, None, 0.0));
        container(column![top, conversation, composer_wrap])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::panel(theme::BACKGROUND, None, 0.0))
            .into()
    }

    /// The legend and view controls that sit above a topology canvas.
    fn topology_controls(&self) -> UiElement<'_> {
        let rule = |color: u32, dashed: bool| {
            container(space::Space::new().width(16).height(1)).style(move |_| {
                theme::panel(if dashed { theme::MAGENTA } else { color }, None, 0.0)
            })
        };
        let legend = row![
            rule(theme::MAGENTA, false),
            widgets::mono("spawn lineage", theme::TEXT_MUTED),
            rule(theme::BORDER_DIM, false),
            widgets::mono("withheld capability", theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center);

        let control = |label: &'static str, message: Message| {
            button(widgets::label(label, theme::TEXT_SECONDARY))
                .height(28)
                .padding([0, 9])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press(message)
        };
        container(
            row![
                legend,
                space::horizontal(),
                button(widgets::label(
                    "Inspector",
                    if self.topology.inspector {
                        theme::TEXT
                    } else {
                        theme::TEXT_MUTED
                    }
                ))
                .height(28)
                .padding([0, 9])
                .style(widgets::button_style(widgets::Button::Choice(
                    self.topology.inspector
                )))
                .on_press(Message::TopologyInspector),
                control("Fit", Message::TopologyFit),
                control("Reset", Message::TopologyReset),
                container(widgets::mono(self.topology.readout(), theme::TEXT_MUTED))
                    .width(44)
                    .align_x(alignment::Horizontal::Right),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
        )
        .padding([12, 18])
        .width(Length::Fill)
        .style(|_| divider_bottom(theme::BACKGROUND))
        .into()
    }

    /// The topology inspector: what the selected node holds and can reach.
    ///
    /// It belongs to the topology views rather than standing beside the
    /// session, which is why it is built here and not in the shell row.
    fn inspector<'a>(
        &'a self,
        title: &'static str,
        subtitle: &'static str,
        body: UiElement<'a>,
    ) -> UiElement<'a> {
        container(column![
            container(
                column![
                    widgets::mono(title, theme::TEXT),
                    widgets::mono(subtitle, theme::TEXT_MUTED),
                ]
                .spacing(theme::SPACE_1)
            )
            .padding(iced_core::Padding {
                top: 14.0,
                right: 14.0,
                bottom: 12.0,
                left: 14.0,
            })
            .width(Length::Fill)
            .style(|_| divider_bottom(theme::BACKGROUND)),
            container(scrollable(body).height(Length::Fill)).padding(14),
        ])
        .width(300)
        .height(Length::Fill)
        .style(|_| divider_left(theme::BACKGROUND))
        .into()
    }

    fn agents_rail(&self) -> UiElement<'_> {
        let status = if self.session.is_none() {
            "offline"
        } else if self.prompt_id.is_some() {
            "running"
        } else {
            "ready"
        };
        let kv = |label: &'static str, value: String| {
            row![
                text(label).size(11).color(theme::rgb(theme::TEXT_MUTED)),
                space::horizontal(),
                text(value)
                    .size(11)
                    .color(theme::rgb(theme::TEXT_SECONDARY)),
            ]
        };
        let worker = container(
            column![
                text("Worker agent").size(12).color(theme::rgb(theme::TEXT)),
                kv("Status", status.into()),
                kv(
                    "Generation",
                    if self.session.is_some() { "1" } else { "—" }.into()
                ),
                kv("Parent", "resident broker".into()),
                kv(
                    "Inference route",
                    if self.session.is_some() {
                        "isolated"
                    } else {
                        "retired"
                    }
                    .into()
                ),
                row![
                    container(space::Space::new().height(4).width(Length::FillPortion(1)))
                        .style(|_| theme::panel(theme::MAGENTA, None, 1.0)),
                    container(space::Space::new().height(4).width(Length::FillPortion(3)))
                        .style(|_| theme::panel(theme::BORDER, None, 1.0)),
                ],
                text("Budget bounded by project limits")
                    .size(11)
                    .color(theme::rgb(theme::TEXT_MUTED)),
            ]
            .spacing(8),
        )
        .padding(14)
        .width(Length::Fill)
        .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG));

        let resources = self.project().map(|p| &p["resources"]);
        let resource = |key: &str, fallback: &str| {
            resources
                .and_then(|r| r[key].as_u64())
                .map(|value| value.to_string())
                .unwrap_or_else(|| fallback.into())
        };
        let limits = container(
            column![
                text("Project limits")
                    .size(12)
                    .color(theme::rgb(theme::TEXT)),
                kv("Agents", resource("agent_limit", "—")),
                kv("Timeout", format!("{}s", resource("prompt_seconds", "—"))),
                kv("Memory", format!("{} MiB", resource("memory_mib", "—"))),
            ]
            .spacing(9),
        )
        .padding(14)
        .width(Length::Fill)
        .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG));
        column![worker, limits].spacing(10).into()
    }

    fn authority_rail(&self) -> UiElement<'_> {
        let capability_count = self
            .events
            .iter()
            .filter(|event| event["kind"] == "capability")
            .count();
        let proposal: UiElement<'_> = if self.selectable_text.contains_key("authority:latest") {
            self.selectable(
                "authority:latest",
                theme::SIZE_MONO,
                theme::TEXT_SECONDARY,
                fonts::MONO,
            )
        } else {
            text("No prompt-scoped authority has been proposed.")
                .size(11)
                .color(theme::rgb(theme::TEXT_SECONDARY))
                .into()
        };
        column![
            container(
                column![
                    text("Prompt authority")
                        .size(12)
                        .color(theme::rgb(theme::TEXT)),
                    proposal,
                ]
                .spacing(8)
            )
            .padding(14)
            .width(Length::Fill)
            .style(|_| theme::panel(
                theme::PANEL,
                Some(theme::BORDER),
                theme::RADIUS_LG
            )),
            container(
                column![
                    text("Capability ledger")
                        .size(12)
                        .color(theme::rgb(theme::TEXT)),
                    text(format!("{capability_count} visible capability events"))
                        .size(11)
                        .color(theme::rgb(theme::TEXT_MUTED)),
                    text(if self.approvals.is_null() {
                        "No committed grants"
                    } else {
                        "Committed policy loaded"
                    })
                    .size(11)
                    .color(theme::rgb(theme::MAGENTA)),
                ]
                .spacing(8)
            )
            .padding(14)
            .width(Length::Fill)
            .style(|_| theme::panel(
                theme::PANEL,
                Some(theme::BORDER),
                theme::RADIUS_LG
            )),
        ]
        .spacing(10)
        .into()
    }

    fn activity_rail(&self) -> UiElement<'_> {
        let rows = session_work_rows(&self.events);
        if rows.is_empty() {
            return container(
                column![
                    text("Activity").size(12).color(theme::rgb(theme::TEXT)),
                    text("Broker and agent outputs will appear here as they are validated.")
                        .size(11)
                        .color(theme::rgb(theme::TEXT_MUTED)),
                ]
                .spacing(8),
            )
            .padding(14)
            .width(Length::Fill)
            .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG))
            .into();
        }
        let mut activity = column![].spacing(10);
        for (index, row) in rows.iter().enumerate().rev().take(30) {
            let key = format!("activity:{index}");
            activity = activity.push(
                container(self.selectable(
                    &key,
                    theme::SIZE_MONO,
                    if row.kind == "error" {
                        theme::ERROR
                    } else {
                        theme::TEXT_SECONDARY
                    },
                    fonts::MONO,
                ))
                .padding([8, 10])
                .width(Length::Fill)
                .style(|_| theme::panel(theme::BACKGROUND, Some(theme::BORDER), theme::RADIUS_MD)),
            );
        }
        activity.into()
    }

    fn editor_panel(&self) -> UiElement<'_> {
        let source = self.editor.as_ref().expect("editor panel requires content");
        let config = serde_json::from_str::<Value>(&source.text()).unwrap_or(Value::Null);
        let value = |pointer: &str| {
            config
                .pointer(pointer)
                .map(|value| match value {
                    Value::String(value) => value.clone(),
                    Value::Null => String::new(),
                    Value::Array(values) => values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };
        let field = |label: &'static str, pointer: &'static str, placeholder: &'static str| {
            column![
                text(label)
                    .size(11)
                    .color(theme::rgb(theme::TEXT_SECONDARY)),
                text_input(placeholder, value(pointer))
                    .on_input(move |value| Message::WizardField(pointer, value))
                    .padding([9, 10])
                    .size(12)
                    .style(input_style),
            ]
            .spacing(6)
        };
        let picker = |label: &'static str,
                      pointer: &'static str,
                      placeholder: &'static str,
                      kind: RequestKind| {
            column![
                text(label)
                    .size(11)
                    .color(theme::rgb(theme::TEXT_SECONDARY)),
                row![
                    text_input(placeholder, value(pointer))
                        .on_input(move |value| Message::WizardField(pointer, value))
                        .padding([9, 10])
                        .size(12)
                        .style(input_style),
                    button(text("Browse").size(12))
                        .padding([9, 10])
                        .style(widgets::button_style(widgets::Button::Secondary))
                        .on_press(Message::Browse(kind)),
                ]
                .spacing(6),
            ]
            .spacing(6)
        };
        let toggle = |label: &'static str, pointer: &'static str| {
            let checked = config
                .pointer(pointer)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            button(
                row![
                    text(if checked { "[x]" } else { "[ ]" })
                        .size(13)
                        .color(rgb(if checked { 0xb8ef6a } else { 0x8d96a5 })),
                    text(label).size(12),
                ]
                .spacing(8),
            )
            .padding([7, 8])
            .style(widgets::button_style(widgets::Button::Ghost))
            .on_press(Message::WizardToggle(pointer))
        };
        let choice = |label: &'static str, pointer: &'static str, option: &'static str| {
            let selected = config.pointer(pointer).and_then(Value::as_str) == Some(option);
            button(text(label).size(12))
                .padding([8, 10])
                .style(widgets::button_style(widgets::Button::Row(selected)))
                .on_press(Message::WizardChoice(pointer, option))
        };

        let page: UiElement<'_> = match self.wizard_page {
            0 => column![
                row![
                    field("Project name", "/name", "My project"),
                    field("Portable project ID", "/id", "my-project"),
                ]
                .spacing(13),
                picker(
                    "Workspace root",
                    "/workspace/root",
                    "Choose a project folder",
                    RequestKind::PickWorkspace,
                ),
                field(
                    "vKore state directory",
                    "/workspace/state",
                    "Outside the workspace"
                ),
                column![
                    text("Workspace access")
                        .size(11)
                        .color(theme::rgb(theme::TEXT_SECONDARY)),
                    row![
                        choice("Read only", "/workspace/access", "read_only"),
                        choice("Review changes", "/workspace/access", "review_changes"),
                        choice(
                            "Automatic writeback",
                            "/workspace/access",
                            "automatic_writeback"
                        ),
                    ]
                    .spacing(6),
                ]
                .spacing(6),
                row![
                    field("Maximum write bytes", "/workspace/max_write_bytes", "0"),
                    field("Maximum write files", "/workspace/max_write_files", "0"),
                ]
                .spacing(13),
            ]
            .spacing(13)
            .into(),
            1 => {
                let real = config.pointer("/inference/backend").and_then(Value::as_str)
                    == Some("llama_cpp");
                let mut form = column![
                    column![
                        text("Inference backend")
                            .size(11)
                            .color(theme::rgb(theme::TEXT_SECONDARY)),
                        row![
                            choice(
                                "Deterministic security oracle",
                                "/inference/backend",
                                "deterministic"
                            ),
                            choice("llama.cpp", "/inference/backend", "llama_cpp"),
                        ]
                        .spacing(6),
                    ]
                    .spacing(6),
                    picker(
                        "Kore guest image",
                        "/inference/kore_image",
                        "Optional for deterministic testing",
                        RequestKind::PickKore
                    ),
                ]
                .spacing(13);
                if real {
                    form = form
                        .push(picker(
                            "llama-server executable",
                            "/inference/llama_server",
                            "llama-server",
                            RequestKind::PickLlama,
                        ))
                        .push(picker(
                            "GGUF model",
                            "/inference/model",
                            "Model file",
                            RequestKind::PickModel,
                        ))
                        .push(field(
                            "Advanced llama.cpp arguments",
                            "/inference/extra_args",
                            "--device none",
                        ));
                }
                form.push(
                    row![
                        field("Context tokens", "/inference/context_tokens", "32768"),
                        field("Output tokens", "/inference/output_tokens", "512"),
                        field("Threads", "/inference/threads", "4"),
                        field("Inference slots", "/inference/slots", "3"),
                    ]
                    .spacing(10),
                )
                .push(
                    container(text(if real {
                        "llama.cpp runs behind the authenticated Kore bridge; paths remain host-side configuration."
                    } else {
                        "Deterministic mode tests the complete authenticated broker, agent, tool, and UI flow without downloading a model."
                    }).size(11).color(theme::rgb(theme::MAGENTA)))
                    .padding(10)
                    .style(|_| theme::panel(theme::PANEL, Some(theme::SUCCESS), theme::RADIUS_MD)),
                )
                .into()
            }
            2 => {
                let mut addons = column![
                    text("Host and guest add-ons").size(13).color(theme::rgb(theme::TEXT)),
                    text("Enable only the bounded capabilities this project needs. Authority is still proposed per prompt.")
                        .size(11)
                        .color(theme::rgb(theme::TEXT_MUTED)),
                ]
                .spacing(8);
                if let Some(entries) = self.catalog["addons"].as_array() {
                    for addon in entries {
                        let id = addon["id"].as_str().unwrap_or_default().to_owned();
                        let name = addon["name"].as_str().unwrap_or("Add-on");
                        let domain = addon["domain"].as_str().unwrap_or("bounded");
                        let enabled = config["addons"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|selected| selected["id"] == id && selected["enabled"] == true);
                        let detail = addon["tools"]
                            .as_array()
                            .map(|tools| format!("{} declared tool(s)", tools.len()))
                            .unwrap_or_else(|| "No declared tools".into());
                        addons = addons.push(
                            button(
                                row![
                                    text(if enabled { "[x]" } else { "[ ]" })
                                        .size(14)
                                        .color(rgb(if enabled { 0xb8ef6a } else { 0x8d96a5 })),
                                    column![
                                        text(domain.to_uppercase())
                                            .size(9)
                                            .color(theme::rgb(theme::TEXT_MUTED)),
                                        text(name.to_owned())
                                            .size(12)
                                            .color(theme::rgb(theme::TEXT)),
                                        text(detail).size(11).color(theme::rgb(theme::TEXT_MUTED)),
                                    ]
                                    .spacing(4),
                                ]
                                .spacing(10),
                            )
                            .padding(11)
                            .width(Length::Fill)
                            .style(widgets::button_style(widgets::Button::Choice(enabled)))
                            .on_press(Message::WizardAddon(id)),
                        );
                    }
                }
                addons.into()
            }
            3 => column![
                text("Authority defaults")
                    .size(13)
                    .color(theme::rgb(theme::TEXT)),
                row![
                    choice("Allow once", "/approvals/default_scope", "once"),
                    choice("For this session", "/approvals/default_scope", "session"),
                ]
                .spacing(6),
                toggle(
                    "Confirm external writes",
                    "/approvals/confirm_external_writes"
                ),
                toggle(
                    "Confirm destructive operations",
                    "/approvals/confirm_destructive"
                ),
                toggle(
                    "Enable standing policy",
                    "/approvals/standing_policy_enabled"
                ),
                container(space::Space::new().height(1).width(Length::Fill))
                    .style(|_| theme::panel(theme::BORDER, None, 0.0)),
                text("Retention and project limits")
                    .size(13)
                    .color(theme::rgb(theme::TEXT)),
                row![
                    field("Transcript days", "/retention/transcript_days", "30"),
                    field("Memory (MiB)", "/resources/memory_mib", "2048"),
                    field("Virtual CPUs", "/resources/virtual_cpus", "2"),
                ]
                .spacing(10),
                row![
                    field("Prompt timeout (s)", "/resources/prompt_seconds", "300"),
                    field("Agent limit", "/resources/agent_limit", "4"),
                ]
                .spacing(10),
                toggle(
                    "Clean Kore KV state on close",
                    "/retention/clean_kv_on_close"
                ),
                toggle(
                    "Retain raw model output",
                    "/retention/retain_raw_model_output"
                ),
            ]
            .spacing(10)
            .into(),
            _ => {
                let report = self.preflight.as_deref().unwrap_or(
                    "Run preflight to resolve paths, scan workspace bounds, and verify the selected runtime before saving.",
                );
                column![
                    text("Review and preflight")
                        .size(13)
                        .color(theme::rgb(theme::TEXT)),
                    container(
                        column![
                            text(value("/name")).size(16).color(theme::rgb(theme::TEXT)),
                            text(format!(
                                "{} · {}",
                                value("/workspace/access"),
                                value("/inference/backend")
                            ))
                            .size(11)
                            .color(theme::rgb(theme::TEXT_MUTED)),
                            text(value("/workspace/root"))
                                .size(11)
                                .color(theme::rgb(theme::TEXT_SECONDARY)),
                        ]
                        .spacing(6),
                    )
                    .padding(13)
                    .width(Length::Fill)
                    .style(|_| theme::panel(
                        theme::BACKGROUND,
                        Some(theme::BORDER),
                        theme::RADIUS_SM
                    )),
                    container(text(report.to_owned()).size(11).color(rgb(
                        if self.preflight.is_some() {
                            0xb8ef6a
                        } else {
                            0xf5c66a
                        }
                    )))
                    .padding(11)
                    .width(Length::Fill)
                    // Warning before a preflight has run, success after it.
                    .style(|_| theme::panel(
                        theme::PANEL,
                        Some(if self.preflight.is_some() {
                            theme::SUCCESS
                        } else {
                            theme::WARNING
                        }),
                        theme::RADIUS_MD
                    )),
                    text("Advanced portable configuration")
                        .size(11)
                        .color(theme::rgb(theme::TEXT_MUTED)),
                    text_editor(source)
                        .on_action(Message::ConfigAction)
                        .height(150)
                        .size(11)
                        .style(widgets::editor),
                ]
                .spacing(11)
                .into()
            }
        };

        let mut steps = row![].spacing(4);
        for index in 0..5 {
            let done = index <= usize::from(self.wizard_page);
            steps = steps.push(
                container(space::Space::new().height(1).width(Length::Fill)).style(move |_| {
                    theme::panel(if done { theme::MAGENTA } else { theme::BORDER }, None, 0.0)
                }),
            );
        }
        let back = button(text("Back").size(12))
            .padding([8, 11])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press_maybe(self.wizard_page.checked_sub(1).map(Message::WizardPage));
        let next: UiElement<'_> = if self.wizard_page < 4 {
            button(text("Next").size(12))
                .padding([8, 13])
                .style(widgets::button_style(widgets::Button::Primary))
                .on_press(Message::WizardPage(self.wizard_page + 1))
                .into()
        } else {
            row![
                button(text("Run preflight").size(12))
                    .padding([8, 11])
                    .style(widgets::button_style(widgets::Button::Secondary))
                    .on_press_maybe((!self.busy).then_some(Message::Preflight)),
                button(text("Save project").size(12))
                    .padding([8, 13])
                    .style(widgets::button_style(widgets::Button::Primary))
                    .on_press_maybe(
                        (!self.busy && self.preflight.is_some()).then_some(Message::SaveProject)
                    ),
            ]
            .spacing(8)
            .into()
        };
        let dialog = column![
            text("Project configuration")
                .size(22)
                .color(theme::rgb(theme::TEXT)),
            text("Portable vKore schema · credentials and capabilities are never stored here.")
                .size(11)
                .color(theme::rgb(theme::TEXT_MUTED)),
            steps,
            scrollable(page).height(Length::Fill),
            container(
                row![
                    button(text("Cancel").size(12))
                        .padding([8, 11])
                        .style(widgets::button_style(widgets::Button::Danger))
                        .on_press(Message::CloseEditor),
                    space::horizontal(),
                    back,
                    next,
                ]
                .spacing(8)
                .align_y(alignment::Vertical::Center),
            )
            .padding(iced_core::Padding {
                top: 16.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0
            })
            .width(Length::Fill)
            .style(|_| divider_right(0x11151a)),
        ]
        .spacing(12);
        container(dialog)
            .padding(22)
            .width(760)
            .height(Length::FillPortion(9))
            .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_MD))
            .into()
    }

    fn approval_panel(&self) -> UiElement<'_> {
        let request = self.approval.as_ref().unwrap_or(&Value::Null);
        container(
            column![
                text("Review exact authority")
                    .size(22)
                    .color(theme::rgb(theme::TEXT)),
                text(
                    request["effect_summary"]
                        .as_str()
                        .unwrap_or("The add-on requests a bounded tool ceiling.")
                )
                .size(12)
                .color(theme::rgb(theme::WARNING)),
                container(
                    text(serde_json::to_string_pretty(request).unwrap_or_default())
                        .size(11)
                        .color(theme::rgb(theme::TEXT_SECONDARY)),
                )
                .padding(11)
                .style(|_| theme::panel(
                    theme::BACKGROUND,
                    Some(theme::BORDER),
                    theme::RADIUS_SM
                )),
                row![
                    button(text("Deny").size(12))
                        .padding([8, 11])
                        .style(widgets::button_style(widgets::Button::Danger))
                        .on_press(Message::DecideApproval(false)),
                    space::horizontal(),
                    button(text("Allow once").size(12))
                        .padding([8, 13])
                        .style(widgets::button_style(widgets::Button::Primary))
                        .on_press(Message::DecideApproval(true)),
                ]
                .spacing(8)
            ]
            .spacing(14),
        )
        .padding(24)
        .width(620)
        .style(|_| theme::panel(theme::PANEL, Some(theme::WARNING), theme::RADIUS_MD))
        .into()
    }
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
    profile: bool,
}

impl App {
    fn process_events(&mut self, window: &Rc<Window>) {
        if self.events.is_empty() {
            return;
        }
        let size = logical_size(window);
        self.model.fit_topology(size);
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
        let record_started = Instant::now();
        let scale = window.scale_factor() as f32;
        self.renderer.hint(Scale {
            window: scale,
            application: 1.0,
        });
        self.model.fit_topology(logical_size(window));
        let mut interface = UserInterface::build(
            self.model.view(logical_size(window)),
            logical_size(window),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
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
        // The frame names its coverage masks by identity; hand the bytes over
        // before it is rendered.
        self.backend.set_masks(self.renderer.take_images());
        let text_stats = self.renderer.take_text_stats();
        let record_ns = record_started.elapsed().as_nanos();
        axgfx::frame_requirements(frame.view())
            .map_err(|error| format!("axgfx rejected the display list: {error:?}"))?;
        if !self.backend.begin_frame(&frame) {
            if self.profile {
                eprintln!(
                    "vkore-ui frame: record {:.3} ms ({} runs, {} glyphs); exact retained hit, 0 raster/present bytes",
                    record_ns as f64 / 1_000_000.0,
                    text_stats.glyph_runs,
                    text_stats.glyphs,
                );
            }
            self.redraw = false;
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
        let render = self
            .backend
            .render_changed(&frame, &mut buffer, extent, physical.width as usize)
            .map_err(|error| format!("{error:?}"))?;
        window.pre_present_notify();
        buffer.present().map_err(|error| error.to_string())?;
        self.backend.mark_visible(frame);
        if self.profile {
            eprintln!(
                "vkore-ui frame: record {:.3} ms ({} runs, {} glyphs); structure {:.3} ms ({} commands, {} kernel/{} reference items, {} glyphs, {} atlas hits/{} misses, {} pixel candidates, {} coverage samples); direct present {} bytes; scratch high-water {} pixels",
                record_ns as f64 / 1_000_000.0,
                text_stats.glyph_runs,
                text_stats.glyphs,
                render.structure_ns as f64 / 1_000_000.0,
                render.commands,
                render.kernel_items,
                render.scalar_items,
                render.glyphs,
                render.atlas_hits,
                render.atlas_misses,
                render.pixels_written,
                render.coverage_samples,
                u64::from(physical.width) * u64::from(physical.height) * 4,
                render.scratch_high_water_pixels,
            );
        }
        self.redraw = false;
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
                        .with_title("vKore · Axiom")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 800))
                        .with_min_inner_size(winit::dpi::PhysicalSize::new(800, 560)),
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
        match event {
            UserEvent::Response(kind, result) => self.model.response(kind, result),
            UserEvent::Poll(result) => self.model.poll_result(result),
        }
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
                    self.model.status = format!("Axiom render failed: {error}");
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

fn http(
    server: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<ApiResponse, String> {
    let payload = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut stream = TcpStream::connect_timeout(&server, Duration::from_secs(3))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {server}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    )
    .map_err(|error| error.to_string())?;
    stream
        .write_all(&payload)
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    let headers = String::from_utf8_lossy(&bytes[..split]);
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or("malformed HTTP status")?;
    let content_type = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        })
        .map(|(_, value)| value.trim().to_owned())
        .unwrap_or_default();
    let response = ApiResponse {
        content_type,
        body: bytes[split + 4..].to_vec(),
    };
    if (200..300).contains(&status) {
        Ok(response)
    } else {
        let detail = serde_json::from_slice::<Value>(&response.body)
            .ok()
            .and_then(|value| value["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(&response.body).into_owned());
        Err(format!("HTTP {status}: {detail}"))
    }
}

fn parse_sse(response: ApiResponse) -> Result<Vec<Value>, String> {
    if !response.content_type.starts_with("text/event-stream") {
        return Err("daemon did not return an event stream".into());
    }
    String::from_utf8(response.body)
        .map_err(|error| error.to_string())?
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect()
}

fn now_id() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchOptions {
    server: SocketAddr,
    project: Option<String>,
}

fn parse_launch_options(args: impl IntoIterator<Item = String>) -> Result<LaunchOptions, String> {
    let mut server = "127.0.0.1:8787".parse().unwrap();
    let mut project = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--server" => {
                server = args
                    .next()
                    .ok_or("missing --server address")?
                    .parse()
                    .map_err(|_| "invalid --server address")?;
            }
            "--project" => {
                let value = args.next().ok_or("missing --project id")?;
                if value.trim().is_empty() {
                    return Err("empty --project id".into());
                }
                project = Some(value);
            }
            _ => {
                return Err(
                    "usage: vkore-ui [--server 127.0.0.1:8787] [--project PROJECT_ID]".into(),
                );
            }
        }
    }
    Ok(LaunchOptions { server, project })
}

fn main() {
    let options = parse_launch_options(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("vkore-ui: {error}");
        std::process::exit(2);
    });
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    let network = Network::start(options.server, event_loop.create_proxy());
    let context = SoftContext::new(event_loop.owned_display_handle())
        .expect("create software presentation context");
    let mut app = App {
        model: Model::with_launch_project(network, options.project),
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
        profile: std::env::var_os("VKORE_UI_PROFILE").is_some(),
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run vKore UI");
}

#[cfg(test)]
mod tests {
    use super::*;
    use hostlinkd::{
        BackendHarness, DeterministicBackend, DeterministicGuest, HarnessHttpServer, HarnessService,
    };

    fn populated_fixture_model() -> Model {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.connected = true;
        model.status = "Kore sandbox ready".into();
        model.projects = vec![json!({
            "id":"axiom-control-plane",
            "name":"Axiom control plane",
            "archived":false,
            "inference":{"backend":"deterministic"},
            "resources":{"memory_mib":4096,"virtual_cpus":4,"prompt_seconds":300,"agent_limit":4},
            "addons":[]
        })];
        model.selected = Some(0);
        model.session = Some("native-session".into());
        model.expanded_work.insert("native-prompt".into());
        model.events = vec![
            json!({"kind":"user_prompt","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_000_000_u64,"payload":{"text":"Read the verification file and explain the result."}}),
            json!({"kind":"inference","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_001_000_u64,"payload":{"kind":"ModelOutputChunk","role":"broker","turn":1,"payload":"Granting the exact read capability."}}),
            json!({"kind":"tool_call","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_002_000_u64,"payload":{"tool":"read_workspace","path":"tool-test.txt"}}),
            json!({"kind":"tool_result","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_003_000_u64,"payload":{"text":"MANIFOLD-ORANGE-742"}}),
            json!({"kind":"assistant","prompt_id":"native-prompt","public":true,"timestamp_unix_ms":1_750_000_004_000_u64,"payload":{"text":"The verification code is MANIFOLD-ORANGE-742. The read stayed inside the project workspace and used prompt-scoped authority."}}),
            json!({"kind":"prompt_terminal","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_004_000_u64,"payload":{"status":"completed"}}),
        ];
        // Events set directly bypass the update path that keeps the selectable
        // conversation editors in step, and the view expects them present.
        model.events_changed();
        model
    }

    fn percentile(samples: &mut [u128], numerator: usize, denominator: usize) -> u128 {
        samples.sort_unstable();
        let index = (samples.len().saturating_sub(1) * numerator) / denominator;
        samples[index]
    }

    fn write_ppm(path: &str, width: usize, height: usize, pixels: &[u32]) {
        let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
        bytes.reserve(width * height * 3);
        for pixel in pixels {
            bytes.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]);
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn render_test_frame(
        frame: &axui_core::Frame<System>,
        target: &mut [u32],
        width: u32,
        height: u32,
    ) -> render_backend::RenderProfile {
        let mut backend = NativeBackend::new().unwrap();
        assert!(backend.begin_frame(frame));
        backend
            .render_changed(
                frame,
                target,
                Extent2d::new(width, height).unwrap(),
                width as usize,
            )
            .unwrap()
    }

    #[test]
    #[ignore = "manual reproducible UI capture and timing probe"]
    fn capture_and_benchmark_ui() {
        let mut model = populated_fixture_model();
        if let Ok(view) = std::env::var("VKORE_UI_VIEW") {
            model.view = match view.as_str() {
                "agents" => View::Agents,
                "filesystem" => View::Filesystem,
                _ => View::Session,
            };
        }
        let size = Size::new(1280.0, 800.0);
        model.fit_topology(size);
        let extent = Extent2d::new(1280, 800).unwrap();
        let mut renderer = native_renderer();
        renderer.hint(Scale {
            window: 1.0,
            application: 1.0,
        });
        let mut cache = user_interface::Cache::new();
        let mut target = vec![0u32; 1280 * 800];
        let mut record_ns = Vec::new();
        let mut kernel_full_ns = Vec::new();
        let mut reference_full_ns = Vec::new();
        let mut kernel_damage_ns = Vec::new();
        let mut reference_damage_ns = Vec::new();
        let mut stable_submit_ns = Vec::new();
        let mut kernel_stats = render_backend::RenderProfile::default();
        let mut damage_stats = render_backend::RenderProfile::default();
        let mut reference_damage_stats = render_backend::RenderProfile::default();
        let mut backend = NativeBackend::new().unwrap();
        let mut benchmark_frame = None;
        let mut benchmark_masks = Vec::new();

        for iteration in 0..70 {
            let started = Instant::now();
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                std::mem::take(&mut cache),
                &mut renderer,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style {
                    text_color: theme::rgb(theme::TEXT),
                },
                mouse::Cursor::Unavailable,
            );
            cache = interface.into_cache();
            let frame = renderer.take_frame().unwrap();
            let masks = renderer.take_images();
            axgfx::frame_requirements(frame.view()).unwrap();
            let recorded = Instant::now();
            backend.set_masks(masks.clone());
            backend.invalidate_for_measurement();
            assert!(backend.begin_frame(&frame));
            let stats = backend
                .render_changed(&frame, &mut target, extent, 1280)
                .unwrap();
            benchmark_frame = Some(frame.clone());
            benchmark_masks = masks.clone();
            backend.mark_visible(frame);
            let finished = Instant::now();
            std::hint::black_box(&target);
            if iteration >= 10 {
                record_ns.push(recorded.duration_since(started).as_nanos());
                kernel_full_ns.push(finished.duration_since(recorded).as_nanos());
            }
            kernel_stats = stats;
        }

        let benchmark_frame = benchmark_frame.unwrap();
        let mut reference_target = vec![0u32; 1280 * 800];
        let mut reference = NativeBackend::reference_for_measurement().unwrap();
        reference.set_masks(benchmark_masks.clone());
        let mut reference_stats = render_backend::RenderProfile::default();
        for iteration in 0..70 {
            let started = Instant::now();
            reference_stats = reference
                .render_changed(&benchmark_frame, &mut reference_target, extent, 1280)
                .unwrap();
            let finished = Instant::now();
            std::hint::black_box(&reference_target);
            if iteration >= 10 {
                reference_full_ns.push(finished.duration_since(started).as_nanos());
            }
        }

        let damage_rect = axgfx::PixelRect::new(416, 608, 448, 128).unwrap();
        for iteration in 0..70 {
            let started = Instant::now();
            damage_stats = backend
                .render_damage_for_measurement(
                    &benchmark_frame,
                    &mut target,
                    extent,
                    1280,
                    axgfx::Damage::Rects(&[damage_rect]),
                )
                .unwrap();
            let finished = Instant::now();
            std::hint::black_box(&target);
            if iteration >= 10 {
                kernel_damage_ns.push(finished.duration_since(started).as_nanos());
            }
        }
        for iteration in 0..70 {
            let started = Instant::now();
            reference_damage_stats = reference
                .render_damage_for_measurement(
                    &benchmark_frame,
                    &mut reference_target,
                    extent,
                    1280,
                    axgfx::Damage::Rects(&[damage_rect]),
                )
                .unwrap();
            let finished = Instant::now();
            std::hint::black_box(&reference_target);
            if iteration >= 10 {
                reference_damage_ns.push(finished.duration_since(started).as_nanos());
            }
        }

        let mut differing_pixels = 0u64;
        let mut max_channel_delta = 0u32;
        for (kernel, reference) in target.iter().zip(&reference_target) {
            if kernel != reference {
                differing_pixels += 1;
            }
            for shift in [24, 16, 8, 0] {
                max_channel_delta = max_channel_delta
                    .max(((kernel >> shift) & 0xff).abs_diff((reference >> shift) & 0xff));
            }
        }
        assert_eq!(differing_pixels, 0, "kernel and reference pixels drifted");
        assert_eq!(max_channel_delta, 0, "kernel and reference colors drifted");

        for iteration in 0..70 {
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                std::mem::take(&mut cache),
                &mut renderer,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style {
                    text_color: theme::rgb(theme::TEXT),
                },
                mouse::Cursor::Unavailable,
            );
            cache = interface.into_cache();
            let frame = renderer.take_frame().unwrap();
            let _masks = renderer.take_images();
            let started = Instant::now();
            assert!(!backend.begin_frame(&frame));
            let finished = Instant::now();
            if iteration >= 10 {
                stable_submit_ns.push(finished.duration_since(started).as_nanos());
            }
        }

        if let Ok(path) = std::env::var("VKORE_UI_CAPTURE") {
            write_ppm(&path, 1280, 800, &target);
        }
        let samples = record_ns.len();
        let record_median = percentile(&mut record_ns, 1, 2);
        let record_p95 = percentile(&mut record_ns, 95, 100);
        let kernel_full_median = percentile(&mut kernel_full_ns, 1, 2);
        let kernel_full_p95 = percentile(&mut kernel_full_ns, 95, 100);
        let reference_full_median = percentile(&mut reference_full_ns, 1, 2);
        let reference_full_p95 = percentile(&mut reference_full_ns, 95, 100);
        let kernel_damage_median = percentile(&mut kernel_damage_ns, 1, 2);
        let kernel_damage_p95 = percentile(&mut kernel_damage_ns, 95, 100);
        let reference_damage_median = percentile(&mut reference_damage_ns, 1, 2);
        let reference_damage_p95 = percentile(&mut reference_damage_ns, 95, 100);
        let stable_submit_median = percentile(&mut stable_submit_ns, 1, 2);
        let stable_submit_p95 = percentile(&mut stable_submit_ns, 95, 100);
        println!(
            "UI_BENCH {{\"implementation\":\"shared_axiom_kernels\",\"width\":1280,\"height\":800,\"samples\":{samples},\"record_median_ns\":{record_median},\"record_p95_ns\":{record_p95},\"kernel_full_median_ns\":{kernel_full_median},\"kernel_full_p95_ns\":{kernel_full_p95},\"reference_full_median_ns\":{reference_full_median},\"reference_full_p95_ns\":{reference_full_p95},\"kernel_damage_median_ns\":{kernel_damage_median},\"kernel_damage_p95_ns\":{kernel_damage_p95},\"reference_damage_median_ns\":{reference_damage_median},\"reference_damage_p95_ns\":{reference_damage_p95},\"stable_submit_median_ns\":{stable_submit_median},\"stable_submit_p95_ns\":{stable_submit_p95},\"kernel_items\":{},\"kernel_scalar_fallback_items\":{},\"reference_scalar_items\":{},\"damage_kernel_items\":{},\"damage_scalar_fallback_items\":{},\"reference_damage_scalar_items\":{},\"full_pixel_candidates\":{},\"damage_pixel_candidates\":{},\"reference_damage_pixel_candidates\":{},\"damage_width\":448,\"damage_height\":128,\"differing_pixels\":{differing_pixels},\"max_channel_delta\":{max_channel_delta},\"full_present_bytes\":4096000,\"damage_present_bytes\":229376,\"stable_present_bytes\":0}}",
            kernel_stats.kernel_items,
            kernel_stats.scalar_items,
            reference_stats.scalar_items,
            damage_stats.kernel_items,
            damage_stats.scalar_items,
            reference_damage_stats.scalar_items,
            kernel_stats.pixels_written,
            damage_stats.pixels_written,
            reference_damage_stats.pixels_written,
        );
    }

    #[test]
    fn launch_options_accept_a_preloaded_project() {
        let options = parse_launch_options([
            "--server".into(),
            "127.0.0.1:9876".into(),
            "--project".into(),
            "qwen9b-live-kore".into(),
        ])
        .unwrap();

        assert_eq!(options.server, "127.0.0.1:9876".parse().unwrap());
        assert_eq!(options.project.as_deref(), Some("qwen9b-live-kore"));
        assert!(parse_launch_options(["--project".into(), "".into()]).is_err());
    }

    #[test]
    fn launch_project_opens_directly_and_single_project_is_the_default() {
        for requested in [Some("qwen9b-live-kore".into()), None] {
            let (requests, request_receiver) = mpsc::channel();
            let (sessions, _session_receiver) = mpsc::channel();
            let mut model = Model::with_launch_project(
                Network {
                    sender: requests,
                    session: sessions,
                },
                requested,
            );
            while request_receiver.try_recv().is_ok() {}
            let projects = json!([{
                "id":"qwen9b-live-kore",
                "name":"Qwen 9B live Kore",
                "archived":false,
                "addons":[]
            }]);

            model.response(
                RequestKind::Projects,
                Ok(ApiResponse {
                    content_type: "application/json".into(),
                    body: serde_json::to_vec(&projects).unwrap(),
                }),
            );

            assert_eq!(model.selected, Some(0));
            assert!(model.busy);
            let request = request_receiver
                .recv_timeout(Duration::from_millis(100))
                .unwrap();
            assert_eq!(request.kind, RequestKind::StartSession);
            assert_eq!(request.path, "/api/v1/sessions");
        }
    }

    #[test]
    fn working_rows_show_only_readable_validated_ai_activity() {
        let events = vec![
            json!({"kind":"user_prompt","prompt_id":"p","payload":{"text":"read X"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"Input","role":"Broker","payload":"{\"authenticated_task_intent\":\"read X\",\"offer_catalog\":[[18,\"o\",\"workspace/X\",[],1,null],[31,\"x\",\"cat\",[],2,null]]}"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"ModelPrompt","role":"Broker","payload":"SECRET SYSTEM PROMPT"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"ModelOutputChunk","role":"Broker","payload":"{\"action\":"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"RawModelCompletion","role":"Broker","payload":"{\"action\":\"create_agent\",\"selectors\":[31,18]}"}}),
            json!({"kind":"broker_proposal","prompt_id":"p","payload":{"kind":"BrokerProposal","role":"Broker","payload":"{\"action\":\"create_agent\",\"budget\":{\"tool_calls\":2},\"label\":\"agent/prompt\",\"selectors\":[31,18]}"}}),
            json!({"kind":"tool_call","prompt_id":"p","payload":{"kind":"ToolCall","role":"Agent","payload":"{\"arguments\":[\"Path(\\\"/X\\\")\"],\"call_id\":1,\"command\":\"cat\"}"}}),
            json!({"kind":"tool_call","prompt_id":"p","payload":{"command":"cat","arguments":"[Path(\"/X\")]"}}),
            json!({"kind":"tool_result","generation":1,"payload":{"text":"MANIFOLD-ORANGE-742"}}),
            json!({"kind":"assistant","public":true,"payload":{"text":"The code is MANIFOLD-ORANGE-742"}}),
        ];
        let refs = events.iter().collect::<Vec<_>>();

        let rows = work_rows(&refs);

        assert_eq!(
            rows.len(),
            2,
            "duplicate calls and hidden records stay hidden"
        );
        assert_eq!(rows[0].speaker, "Broker");
        assert_eq!(rows[0].body, "Tool_use: create_agent(Agent 1, [cat], [X])");
        assert_eq!(rows[1].speaker, "Agent 1");
        assert_eq!(rows[1].body, "Tool_use: cat X");
        let visible = rows
            .iter()
            .map(|row| format!("{}: {}", row.speaker, row.body))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!visible.contains("SECRET SYSTEM PROMPT"));
        assert!(!visible.contains("MANIFOLD-ORANGE-742"));
        assert!(!visible.contains('{'));
    }

    #[test]
    fn message_metadata_includes_timestamp_and_end_to_end_duration() {
        let assistant = json!({
            "timestamp_unix_ms": 1_750_000_002_500_u64,
            "prompt_id": "p"
        });

        let metadata = message_meta("AGENT 1", &assistant, Some(1_750_000_000_000));

        assert!(metadata.starts_with("AGENT 1 · "));
        assert!(metadata.contains("responded in 2.5 s"));
    }

    #[test]
    fn working_panel_toggle_is_reversible() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.expanded_work.insert("p".into());

        model.update(Message::ToggleWorking("p".into()));
        assert!(!model.expanded_work.contains("p"));

        model.update(Message::ToggleWorking("p".into()));
        assert!(model.expanded_work.contains("p"));
    }

    #[test]
    fn conversation_text_is_selectable_but_read_only() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.events = vec![json!({
            "kind":"user_prompt",
            "prompt_id":"p",
            "timestamp_unix_ms":1_750_000_000_000_u64,
            "payload":{"text":"select this request"}
        })];
        model.events_changed();
        let key = "prompt:p:user";
        let original = model.selectable_text[key].text();

        model.update(Message::SelectText(
            key.into(),
            iced_core::text::editor::Action::SelectAll,
        ));
        assert_eq!(
            model.selectable_text[key].selection(),
            Some(original.clone())
        );

        model.update(Message::SelectText(
            key.into(),
            iced_core::text::editor::Action::Edit(iced_core::text::editor::Edit::Insert('!')),
        ));
        assert_eq!(model.selectable_text[key].text(), original);
    }

    #[test]
    fn saving_an_open_project_restarts_its_chat_session() {
        let (requests, request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        request_receiver.try_iter().for_each(drop);
        model.projects = vec![json!({
            "id":"saved-project",
            "name":"Saved project",
            "archived":false,
            "addons":[],
            "inference":{"backend":"deterministic"},
        })];
        model.selected = Some(0);
        model.session = Some("old-session".into());

        let saved = model.projects[0].clone();
        model.response(
            RequestKind::SaveProject,
            Ok(ApiResponse {
                content_type: "application/json".into(),
                body: serde_json::to_vec(&saved).unwrap(),
            }),
        );

        assert!(model.restart_after_stop);
        let stop = request_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(stop.kind, RequestKind::StopSession);
        assert_eq!(stop.path, "/api/v1/sessions/old-session");

        model.response(
            RequestKind::StopSession,
            Ok(ApiResponse {
                content_type: "application/json".into(),
                body: br#"{"closed":true}"#.to_vec(),
            }),
        );

        assert!(!model.restart_after_stop);
        let start = request_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(start.kind, RequestKind::StartSession);
        assert_eq!(start.path, "/api/v1/sessions");
        assert_eq!(start.body.unwrap()["project_id"], "saved-project");
    }

    #[test]
    fn complete_shell_records_text_and_renders_directly() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.connected = true;
        model.status = "Native Axiom renderer test".into();
        model.projects = vec![json!({
            "id":"test-project",
            "name":"Test project",
            "archived":false,
            "resources":{"memory_mib":2048,"virtual_cpus":2,"prompt_seconds":300,"agent_limit":4},
            "addons":[]
        })];
        model.selected = Some(0);

        let mut renderer = native_renderer();
        renderer.hint(Scale {
            window: 1.0,
            application: 1.0,
        });
        let mut interface = UserInterface::build(
            model.view(Size::new(900.0, 600.0)),
            Size::new(900.0, 600.0),
            user_interface::Cache::new(),
            &mut renderer,
        );
        interface.draw(
            &mut renderer,
            &Theme::Dark,
            &renderer::Style::default(),
            mouse::Cursor::Unavailable,
        );
        let frame = renderer.take_frame().unwrap();
        let _masks = renderer.take_images();
        assert!(frame.commands().len() > 10);
        assert!(
            !frame.glyphs().is_empty(),
            "the native shell must retain text"
        );
        let requirements = axgfx::frame_requirements(frame.view()).unwrap();
        assert!(requirements.features().contains(axgfx::Features::GLYPH_RUN));

        let mut target = vec![0u32; 900 * 600];
        let stats = render_test_frame(&frame, &mut target, 900, 600);
        assert_eq!(stats.scratch_high_water_pixels, 0);
        assert!(stats.pixels_written > 900 * 600 / 8);
        assert!(target.iter().any(|pixel| *pixel != 0x0008_0a0d));
    }

    #[test]
    fn expanded_and_collapsed_working_panel_render_directly() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.session = Some("native-session".into());
        model.expanded_work.insert("native-prompt".into());
        model.events = vec![
            json!({"kind":"user_prompt","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_000_000_u64,"payload":{"text":"Read the verification file"}}),
            json!({"kind":"inference","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_001_000_u64,"payload":{"kind":"ModelOutputChunk","role":"broker","turn":1,"payload":"Granting the exact read capability."}}),
            json!({"kind":"tool_call","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_002_000_u64,"payload":{"tool":"read_workspace","path":"tool-test.txt"}}),
            json!({"kind":"tool_result","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_003_000_u64,"payload":{"text":"MANIFOLD-ORANGE-742"}}),
            json!({"kind":"assistant","prompt_id":"native-prompt","public":true,"timestamp_unix_ms":1_750_000_004_000_u64,"payload":{"text":"The verification code is MANIFOLD-ORANGE-742."}}),
            json!({"kind":"prompt_terminal","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_004_000_u64,"payload":{"status":"completed"}}),
        ];
        model.events_changed();

        for expanded in [true, false] {
            if expanded != model.expanded_work.contains("native-prompt") {
                model.update(Message::ToggleWorking("native-prompt".into()));
            }
            let mut renderer = native_renderer();
            renderer.hint(Scale {
                window: 1.0,
                application: 1.0,
            });
            let size = Size::new(1100.0, 760.0);
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                user_interface::Cache::new(),
                &mut renderer,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style::default(),
                mouse::Cursor::Unavailable,
            );
            let frame = renderer.take_frame().unwrap();
            let _masks = renderer.take_images();
            axgfx::frame_requirements(frame.view()).unwrap();
            assert!(frame.glyphs().len() > 30);

            let mut target = vec![0u32; 1100 * 760];
            let stats = render_test_frame(&frame, &mut target, 1100, 760);
            assert!(stats.pixels_written > 1100 * 760 / 8);
        }
    }

    #[test]
    fn every_native_wizard_page_records_and_renders() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.catalog = json!({
            "addons":[
                {"id":"reference.host","name":"Reference host add-on","domain":"host","tools":[{"name":"echo"}]},
                {"id":"reference.guest","name":"Reference guest add-on","domain":"guest","tools":[{"name":"list_workspace"}]}
            ]
        });
        model.update(Message::NewProject);

        for page in 0..5 {
            model.wizard_page = page;
            let mut renderer = native_renderer();
            renderer.hint(Scale {
                window: 1.25,
                application: 1.0,
            });
            let size = Size::new(1024.0, 640.0);
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                user_interface::Cache::new(),
                &mut renderer,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style {
                    text_color: theme::rgb(theme::TEXT),
                },
                mouse::Cursor::Unavailable,
            );
            let frame = renderer.take_frame().unwrap();
            let _masks = renderer.take_images();
            axgfx::frame_requirements(frame.view()).unwrap();
            assert!(frame.glyphs().len() > 20, "wizard page {page} lost text");

            let mut target = vec![0u32; 1280 * 800];
            let stats = render_test_frame(&frame, &mut target, 1280, 800);
            assert!(stats.pixels_written > 1280 * 800 / 5);
        }
    }

    #[test]
    fn native_transport_drives_a_complete_prompt_lifecycle() {
        let temporary = tempfile::tempdir().unwrap();
        let service = HarnessService::start(
            BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
            temporary.path().join("transcripts"),
        );
        let server = HarnessHttpServer::bind_with_state(
            "127.0.0.1:0".parse().unwrap(),
            service.handle(),
            temporary.path().join("projects"),
        )
        .unwrap();
        let address = server.local_addr();

        let health = roundtrip(&server, address, "GET", "/api/v1/health", None)
            .json()
            .unwrap();
        assert_eq!(health["ok"], true);
        let session = roundtrip(
            &server,
            address,
            "POST",
            "/api/v1/sessions",
            Some(&json!({
                "api_version":1,
                "project_id":"native-project",
                "session_id":"native-session"
            })),
        )
        .json()
        .unwrap();
        assert_eq!(session["session_id"], "native-session");
        roundtrip(
            &server,
            address,
            "POST",
            "/api/v1/sessions/native-session/prompts",
            Some(&json!({
                "api_version":1,
                "prompt_id":"native-prompt",
                "idempotency_key":"native-key",
                "text":"hello from axiom"
            })),
        );

        let mut events = Vec::new();
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            events = parse_sse(roundtrip(
                &server,
                address,
                "GET",
                "/api/v1/sessions/native-session/events?after=0",
                None,
            ))
            .unwrap();
            if events
                .iter()
                .any(|event| event["kind"] == "prompt_terminal")
            {
                break;
            }
        }
        assert!(events.iter().any(|event| event["kind"] == "user_prompt"));
        assert!(events.iter().any(|event| event["kind"] == "assistant"));
        assert!(
            events
                .iter()
                .any(|event| event["kind"] == "prompt_terminal")
        );
        roundtrip(
            &server,
            address,
            "DELETE",
            "/api/v1/sessions/native-session",
            None,
        );
    }

    fn roundtrip(
        server: &HarnessHttpServer,
        address: SocketAddr,
        method: &'static str,
        path: &'static str,
        body: Option<&Value>,
    ) -> ApiResponse {
        let body = body.cloned();
        let client = thread::spawn(move || http(address, method, path, body.as_ref()).unwrap());
        server.poll(Duration::from_secs(1)).unwrap();
        client.join().unwrap()
    }
}
