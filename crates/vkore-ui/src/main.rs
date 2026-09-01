use axgfx::{Damage, Extent2d};
use axrd::axui_direct::render_xrgb;
use axui_iced::Renderer as AxRenderer;
use iced_core::Renderer as _;
use iced_core::{
    Element, Length, Size, Theme, mouse,
    renderer::{self, Scale},
    shell,
};
use iced_runtime::user_interface::{self, UserInterface};
use iced_widget::{button, column, container, row, scrollable, text, text_editor, text_input};
use iced_winit::{conversion, winit};
use serde_json::{Value, json};
use softbuffer::{Context as SoftContext, Surface as SoftSurface};
use std::{
    alloc::System,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    num::NonZeroU32,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

type UiElement<'a> = Element<'a, Message, Theme, AxRenderer<System>>;

#[derive(Debug, Clone)]
enum Message {
    SelectProject(usize),
    NewProject,
    EditProject,
    ConfigAction(iced_core::text::editor::Action),
    Preflight,
    SaveProject,
    Browse(RequestKind),
    CloseEditor,
    ToggleSession,
    CleanSession,
    PromptChanged(String),
    SendPrompt,
    CancelPrompt,
    ArchiveProject,
    DuplicateProject,
    ExportProject,
    Rail(Rail),
    DecideApproval(bool),
    Reload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rail {
    Agents,
    Authority,
    Activity,
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
    session: mpsc::Sender<Option<String>>,
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

        let (session_sender, session_receiver) = mpsc::channel::<Option<String>>();
        thread::spawn(move || {
            let mut session = None;
            let mut cursor = 0u64;
            loop {
                while let Ok(next) = session_receiver.try_recv() {
                    session = next;
                    cursor = 0;
                }
                let Some(id) = session.as_deref() else {
                    match session_receiver.recv() {
                        Ok(next) => {
                            session = next;
                            cursor = 0;
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
                thread::sleep(Duration::from_millis(180));
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

    fn poll(&self, session: Option<String>) {
        let _ = self.session.send(session);
    }
}

struct Model {
    network: Network,
    connected: bool,
    projects: Vec<Value>,
    catalog: Value,
    approvals: Value,
    selected: Option<usize>,
    session: Option<String>,
    prompt_id: Option<String>,
    prompt: String,
    events: Vec<Value>,
    rail: Rail,
    editor: Option<text_editor::Content<AxRenderer<System>>>,
    preflight: Option<String>,
    approval: Option<Value>,
    status: String,
    busy: bool,
    restart_after_stop: bool,
}

impl Model {
    fn new(network: Network) -> Self {
        let model = Self {
            network,
            connected: false,
            projects: Vec::new(),
            catalog: Value::Null,
            approvals: Value::Null,
            selected: None,
            session: None,
            prompt_id: None,
            prompt: String::new(),
            events: Vec::new(),
            rail: Rail::Agents,
            editor: None,
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

    fn project(&self) -> Option<&Value> {
        self.selected.and_then(|index| self.projects.get(index))
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::SelectProject(index) => {
                if self.session.is_none() {
                    self.selected = Some(index);
                    self.events.clear();
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
                self.preflight = None;
            }
            Message::EditProject => {
                if let Some(project) = self.project() {
                    self.editor = Some(text_editor::Content::with_text(
                        &serde_json::to_string_pretty(project).unwrap_or_default(),
                    ));
                    self.preflight = None;
                }
            }
            Message::ConfigAction(action) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.perform(action);
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
            Message::PromptChanged(value) => self.prompt = value,
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
            Message::Rail(value) => self.rail = value,
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
            Message::Reload => self.reload(),
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
        let text = self.prompt.trim();
        if text.is_empty() || self.prompt_id.is_some() {
            return;
        }
        let prompt_id = format!("p-{}", now_id());
        self.prompt_id = Some(prompt_id.clone());
        self.busy = true;
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
        self.prompt.clear();
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
                self.update(Message::ToggleSession);
            }
            (RequestKind::StartSession, Ok(value)) => {
                self.busy = false;
                self.session = value["session_id"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value["id"].as_str().map(str::to_owned));
                self.events.clear();
                self.prompt_id = None;
                self.network.poll(self.session.clone());
                self.status = "Fresh isolated sandbox started.".into();
            }
            (RequestKind::StopSession, Ok(_)) => {
                self.busy = false;
                self.network.poll(None);
                self.session = None;
                self.prompt_id = None;
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
            }
            _ => {}
        }
    }

    fn poll_result(&mut self, result: Result<Vec<Value>, String>) {
        match result {
            Ok(events) => {
                for event in events {
                    if event["kind"] == "prompt_terminal"
                        && event["prompt_id"].as_str() == self.prompt_id.as_deref()
                    {
                        self.prompt_id = None;
                        self.busy = false;
                    }
                    self.events.push(event);
                }
            }
            Err(error) => self.status = format!("Event stream interrupted: {error}"),
        }
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

    fn view(&self) -> UiElement<'_> {
        let sidebar = self.sidebar();
        let main = self.main_panel();
        let rail = self.rail_panel();
        let base = row![sidebar, main, rail].height(Length::Fill);
        if self.editor.is_some() {
            container(self.editor_panel())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else if self.approval.is_some() {
            container(self.approval_panel())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            base.into()
        }
    }

    fn sidebar(&self) -> UiElement<'_> {
        let mut projects = column![text("vKore").size(24), text("PROJECTS").size(11)].spacing(10);
        for (index, project) in self
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| !p["archived"].as_bool().unwrap_or(false))
        {
            let label = if self.selected == Some(index) {
                format!("● {}", project["name"].as_str().unwrap_or("Project"))
            } else {
                format!("○ {}", project["name"].as_str().unwrap_or("Project"))
            };
            projects = projects.push(
                button(text(label))
                    .width(Length::Fill)
                    .on_press(Message::SelectProject(index)),
            );
        }
        projects = projects.push(
            button(text("＋ New project"))
                .width(Length::Fill)
                .on_press_maybe(self.session.is_none().then_some(Message::NewProject)),
        );
        let project_idle = self.project().is_some() && self.session.is_none();
        let controls = column![
            button(text("Project settings"))
                .width(Length::Fill)
                .on_press_maybe(project_idle.then_some(Message::EditProject)),
            button(text("Duplicate project"))
                .width(Length::Fill)
                .on_press_maybe(project_idle.then_some(Message::DuplicateProject)),
            button(text("Export configuration"))
                .width(Length::Fill)
                .on_press_maybe(self.project().map(|_| Message::ExportProject)),
            button(text("Archive project"))
                .width(Length::Fill)
                .on_press_maybe(project_idle.then_some(Message::ArchiveProject)),
            button(text("Reload daemon state"))
                .width(Length::Fill)
                .on_press(Message::Reload),
        ]
        .spacing(6);
        container(column![scrollable(projects).height(Length::Fill), controls].spacing(12))
            .padding(14)
            .width(230)
            .height(Length::Fill)
            .into()
    }

    fn main_panel(&self) -> UiElement<'_> {
        let project_name = self
            .project()
            .and_then(|p| p["name"].as_str())
            .unwrap_or("No project open");
        let health = if self.connected {
            "● daemon"
        } else {
            "○ daemon"
        };
        let start = if self.session.is_some() {
            "Stop"
        } else {
            "Start"
        };
        let top = row![
            column![text(project_name).size(18), text(&self.status).size(12)].width(Length::Fill),
            text(health),
            button(text(start)).on_press_maybe(
                (self.project().is_some() && !self.busy).then_some(Message::ToggleSession)
            ),
            button(text("Clean"))
                .on_press_maybe(self.session.as_ref().map(|_| Message::CleanSession)),
        ]
        .spacing(10);

        let conversation: UiElement<'_> = if self.session.is_none() {
            container(
                column![
                    text("Bounded agents, visible authority.").size(26),
                    text("Select a project and start its Kore sandbox.")
                ]
                .spacing(8),
            )
            .center(Length::Fill)
            .into()
        } else if self.events.is_empty() {
            container(
                column![
                    text("Sandbox ready.").size(24),
                    text("Prompt-scoped authority will appear here.")
                ]
                .spacing(8),
            )
            .center(Length::Fill)
            .into()
        } else {
            let mut items = column![].spacing(8);
            for event in &self.events {
                let kind = event["kind"].as_str().unwrap_or("event");
                let body = match kind {
                    "user_prompt" => event["payload"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    "assistant" if event["public"].as_bool().unwrap_or(false) => {
                        event["payload"]["text"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned()
                    }
                    _ => serde_json::to_string_pretty(&event["payload"]).unwrap_or_default(),
                };
                items = items.push(
                    container(
                        column![text(kind.to_uppercase()).size(10), text(body).size(13)].spacing(4),
                    )
                    .padding(10)
                    .width(Length::Fill),
                );
            }
            scrollable(items).height(Length::Fill).into()
        };

        let send = if self.prompt_id.is_some() {
            button(text("Cancel")).on_press(Message::CancelPrompt)
        } else {
            button(text("Send")).on_press_maybe(
                (self.session.is_some() && !self.prompt.trim().is_empty())
                    .then_some(Message::SendPrompt),
            )
        };
        let composer = row![
            text_input("Ask the bounded agent…", &self.prompt)
                .on_input(Message::PromptChanged)
                .on_submit(Message::SendPrompt)
                .padding(10)
                .width(Length::Fill),
            send
        ]
        .spacing(8);
        container(column![top, conversation, composer].spacing(12))
            .padding(18)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn rail_panel(&self) -> UiElement<'_> {
        let tabs = row![
            button(text("Agents")).on_press(Message::Rail(Rail::Agents)),
            button(text("Authority")).on_press(Message::Rail(Rail::Authority)),
            button(text("Activity")).on_press(Message::Rail(Rail::Activity)),
        ]
        .spacing(4);
        let body = match self.rail {
            Rail::Agents => json!({
                "status": if self.session.is_some() { if self.prompt_id.is_some() {"running"} else {"ready"} } else {"offline"},
                "parent":"resident broker",
                "inference_route":if self.session.is_some() {"isolated"} else {"retired"},
                "limits":self.project().map(|p| p["resources"].clone()).unwrap_or(Value::Null)
            }),
            Rail::Authority => json!({
                "broker_proposal":self.events.iter().rev().find(|e| e["kind"] == "broker_proposal").map(|e| e["payload"].clone()),
                "capabilities":self.events.iter().filter(|e| e["kind"] == "capability").map(|e| e["payload"].clone()).collect::<Vec<_>>(),
                "committed_policy":self.approvals
            }),
            Rail::Activity => Value::Array(self.events.iter().rev().take(30).cloned().collect()),
        };
        container(
            column![
                tabs,
                scrollable(text(serde_json::to_string_pretty(&body).unwrap_or_default()).size(11))
                    .height(Length::Fill)
            ]
            .spacing(12),
        )
        .padding(12)
        .width(320)
        .height(Length::Fill)
        .into()
    }

    fn editor_panel(&self) -> UiElement<'_> {
        let source = self.editor.as_ref().expect("editor panel requires content");
        let report = self.preflight.as_deref().unwrap_or(
            "Run preflight before saving. The daemon resolves paths and enforces writeback bounds.",
        );
        container(
            column![
                text("Project configuration").size(26),
                text("Portable vKore schema · credentials and capabilities are never stored here."),
                text_editor(source)
                    .on_action(Message::ConfigAction)
                    .height(Length::Fill),
                row![
                    button(text("Workspace…"))
                        .on_press(Message::Browse(RequestKind::PickWorkspace)),
                    button(text("Kore image…")).on_press(Message::Browse(RequestKind::PickKore)),
                    button(text("llama-server…")).on_press(Message::Browse(RequestKind::PickLlama)),
                    button(text("Model…")).on_press(Message::Browse(RequestKind::PickModel)),
                ]
                .spacing(6),
                scrollable(text(report).size(11)).height(140),
                row![
                    button(text("Cancel")).on_press(Message::CloseEditor),
                    button(text("Run preflight"))
                        .on_press_maybe((!self.busy).then_some(Message::Preflight)),
                    button(text("Save project")).on_press_maybe(
                        (!self.busy && self.preflight.is_some()).then_some(Message::SaveProject)
                    ),
                ]
                .spacing(8)
            ]
            .spacing(12),
        )
        .padding(24)
        .width(Length::Fixed(920.0))
        .center(Length::Fill)
        .into()
    }

    fn approval_panel(&self) -> UiElement<'_> {
        let request = self.approval.as_ref().unwrap_or(&Value::Null);
        container(
            column![
                text("Review exact authority").size(26),
                text(
                    request["effect_summary"]
                        .as_str()
                        .unwrap_or("The add-on requests a bounded tool ceiling.")
                ),
                text(serde_json::to_string_pretty(request).unwrap_or_default()).size(11),
                row![
                    button(text("Deny")).on_press(Message::DecideApproval(false)),
                    button(text("Allow once")).on_press(Message::DecideApproval(true)),
                ]
                .spacing(8)
            ]
            .spacing(14),
        )
        .padding(24)
        .width(Length::Fixed(620.0))
        .center(Length::Fill)
        .into()
    }
}

struct App {
    model: Model,
    context: SoftContext<OwnedDisplayHandle>,
    surface: Option<SoftSurface<OwnedDisplayHandle, Rc<Window>>>,
    renderer: AxRenderer<System>,
    cache: user_interface::Cache,
    events: Vec<iced_core::Event>,
    cursor: mouse::Cursor,
    modifiers: ModifiersState,
    redraw: bool,
}

impl App {
    fn process_events(&mut self, window: &Rc<Window>) {
        if self.events.is_empty() {
            return;
        }
        let size = logical_size(window);
        let mut interface = UserInterface::build(
            self.model.view(),
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
        window.request_redraw();
    }

    fn draw(&mut self, window: &Rc<Window>) -> Result<(), String> {
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
            self.model.view(),
            logical_size(window),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        interface.draw(
            &mut self.renderer,
            &Theme::Dark,
            &renderer::Style::default(),
            self.cursor,
        );
        self.cache = interface.into_cache();
        let frame = self
            .renderer
            .take_frame()
            .map_err(|error| format!("{error:?}"))?;
        axgfx::frame_requirements(frame.view())
            .map_err(|error| format!("axgfx rejected the display list: {error:?}"))?;
        let surface = self.surface.as_mut().ok_or("surface unavailable")?;
        surface
            .resize(
                NonZeroU32::new(physical.width).unwrap(),
                NonZeroU32::new(physical.height).unwrap(),
            )
            .map_err(|error| error.to_string())?;
        window.pre_present_notify();
        let mut buffer = surface.buffer_mut().map_err(|error| error.to_string())?;
        let extent =
            Extent2d::new(physical.width, physical.height).map_err(|error| format!("{error:?}"))?;
        render_xrgb(
            frame.view(),
            &mut buffer,
            extent,
            physical.width as usize,
            Damage::Full,
            0x0008_0a0d,
        )
        .map_err(|error| format!("{error:?}"))?;
        buffer.present().map_err(|error| error.to_string())?;
        self.redraw = false;
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
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Response(kind, result) => self.model.response(kind, result),
            UserEvent::Poll(result) => self.model.poll_result(result),
        }
        self.redraw = true;
        if let Some(window) = self.surface.as_ref().map(SoftSurface::window) {
            window.request_redraw();
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
                if let Err(error) = self.draw(&window) {
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
        if self.redraw
            && let Some(window) = self.surface.as_ref().map(SoftSurface::window)
        {
            window.request_redraw();
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

fn server_argument() -> Result<SocketAddr, String> {
    let mut server = "127.0.0.1:8787".parse().unwrap();
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--server" => {
                server = args
                    .next()
                    .ok_or("missing --server address")?
                    .parse()
                    .map_err(|_| "invalid --server address")?;
            }
            _ => return Err("usage: vkore-ui [--server 127.0.0.1:8787]".into()),
        }
    }
    Ok(server)
}

fn main() {
    let server = server_argument().unwrap_or_else(|error| {
        eprintln!("vkore-ui: {error}");
        std::process::exit(2);
    });
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    let network = Network::start(server, event_loop.create_proxy());
    let context = SoftContext::new(event_loop.owned_display_handle())
        .expect("create software presentation context");
    let mut app = App {
        model: Model::new(network),
        context,
        surface: None,
        renderer: AxRenderer::new_in(renderer::Settings::default(), System),
        cache: user_interface::Cache::new(),
        events: Vec::new(),
        cursor: mouse::Cursor::Unavailable,
        modifiers: ModifiersState::default(),
        redraw: true,
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

        let mut renderer = AxRenderer::new_in(renderer::Settings::default(), System);
        renderer.hint(Scale {
            window: 1.0,
            application: 1.0,
        });
        let mut interface = UserInterface::build(
            model.view(),
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
        assert!(frame.commands().len() > 10);
        assert!(
            !frame.glyphs().is_empty(),
            "the native shell must retain text"
        );
        let requirements = axgfx::frame_requirements(frame.view()).unwrap();
        assert!(
            requirements
                .features()
                .contains(axgfx::Features::GLYPH_RASTER)
        );

        let mut target = vec![0u32; 900 * 600];
        let stats = render_xrgb(
            frame.view(),
            &mut target,
            Extent2d::new(900, 600).unwrap(),
            900,
            Damage::Full,
            0x0008_0a0d,
        )
        .unwrap();
        assert_eq!(stats.scratch_high_water, 0);
        assert!(stats.changed_pixels > 900 * 600 / 8);
        assert!(target.iter().any(|pixel| *pixel != 0x0008_0a0d));
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
