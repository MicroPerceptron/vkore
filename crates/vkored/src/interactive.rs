//! Authenticated, reconnectable interactive harness API.

use crate::{
    BackendEvent, BackendRole, InferenceBackend, InferenceObservation, ObservationKind,
    ProjectConfig, ProjectStore, SequenceIdentity, SettingsStore, UiApprovalService,
};
use kore_hostlink::{
    AGENT_TEXT_MAX, AgentEventKind, AgentRequestKind, AgentRequestView, AgentToolDescriptor,
    BrokerActionKind, BrokerOffer, BrokerOfferKind, InferenceRequestKind, InferenceRequestView,
    RequestId, RouteId, TOOL_EFFECT_OBSERVE, TOOL_RESULT_WORKSPACE, TaskBudget, TaskGeneration,
    TaskIntentRequest, ToolArgument, ToolArgumentKind, ToolInputSchema, ToolResultKind,
    decode_agent_event, decode_broker_reply,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const HARNESS_API_VERSION: u16 = 1;
const EVENTS_MAX: usize = 4096;
const PROMPT_BYTES_MAX: usize = kore_hostlink::TASK_INTENT_MAX;
const PENDING_MAX: usize = 32;
const POLL_TICKS_MAX: u64 = 30_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateSession {
    pub api_version: u16,
    pub project_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptRequest {
    pub api_version: u16,
    pub prompt_id: String,
    pub idempotency_key: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CancelRequest {
    pub api_version: u16,
    pub prompt_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Open,
    Busy,
    Closing,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStatus {
    Queued,
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionState {
    pub api_version: u16,
    pub project_id: String,
    pub session_id: String,
    pub status: SessionStatus,
    pub generation: u64,
    pub backend_incarnation: u64,
    pub last_event_sequence: u64,
    pub prompts: BTreeMap<String, PromptStatus>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SessionOpened,
    UserPrompt,
    BrokerProposal,
    Capability,
    Inference,
    Validation,
    ToolCall,
    ToolResult,
    Assistant,
    Error,
    PromptTerminal,
    ConversationClosed,
}

pub(crate) fn emit_inference_observation(
    observation: InferenceObservation,
    emit: &mut dyn FnMut(EventKind, bool, Value),
) {
    let kind = match observation.kind {
        ObservationKind::BrokerProposal => EventKind::BrokerProposal,
        ObservationKind::Validation => EventKind::Validation,
        ObservationKind::ToolCall => EventKind::ToolCall,
        ObservationKind::ToolResult => EventKind::ToolResult,
        _ => EventKind::Inference,
    };
    emit(kind, false, inference_observation_payload(&observation));
}

fn inference_observation_payload(observation: &InferenceObservation) -> Value {
    json!({
        "role": format!("{:?}", observation.role),
        "kind": format!("{:?}", observation.kind),
        "turn": observation.turn,
        "payload": observation.payload,
        "accepted": observation.accepted,
        "detail": observation.detail,
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HarnessEvent {
    pub api_version: u16,
    pub sequence: u64,
    #[serde(default)]
    pub timestamp_unix_ms: u64,
    pub project_id: String,
    pub session_id: String,
    pub prompt_id: Option<String>,
    pub generation: u64,
    pub kind: EventKind,
    pub public: bool,
    pub payload: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HarnessError {
    UntrustedBrowserRequest,
    UnsupportedVersion,
    InvalidIdentity,
    PromptTooLarge,
    UnknownSession,
    UnknownPrompt,
    SessionClosed,
    Busy,
    DuplicatePrompt,
    IdempotencyConflict,
    EventCursorExpired,
    Backpressure,
    Io(String),
    Runtime(String),
    NonLoopback,
    Http(String),
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for HarnessError {}

struct SessionData {
    state: SessionState,
    events: VecDeque<HarnessEvent>,
    idempotency: BTreeMap<String, (String, String)>,
    cancellations: BTreeSet<String>,
}

struct Shared {
    sessions: BTreeMap<String, SessionData>,
    next_sequence: u64,
    queued: usize,
    transcript_root: PathBuf,
}

impl Shared {
    fn emit(
        &mut self,
        session_id: &str,
        prompt_id: Option<&str>,
        kind: EventKind,
        public: bool,
        payload: Value,
    ) -> Result<HarnessEvent, HarnessError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(HarnessError::UnknownSession)?;
        let event = HarnessEvent {
            api_version: HARNESS_API_VERSION,
            sequence: self.next_sequence,
            timestamp_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            project_id: session.state.project_id.clone(),
            session_id: session_id.into(),
            prompt_id: prompt_id.map(str::to_owned),
            generation: session.state.generation,
            kind,
            public,
            payload,
        };
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(HarnessError::Backpressure)?;
        append_transcript(&self.transcript_root, &event)?;
        if session.events.len() == EVENTS_MAX {
            session.events.pop_front();
        }
        session.state.last_event_sequence = event.sequence;
        session.events.push_back(event.clone());
        Ok(event)
    }
}

fn append_transcript(root: &Path, event: &HarnessEvent) -> Result<(), HarnessError> {
    std::fs::create_dir_all(root).map_err(|error| HarnessError::Io(error.to_string()))?;
    let path = root.join(format!("{}.jsonl", safe_id(&event.session_id)));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| HarnessError::Io(error.to_string()))?;
    serde_json::to_writer(&mut file, event).map_err(|error| HarnessError::Io(error.to_string()))?;
    file.write_all(b"\n")
        .and_then(|_| file.sync_data())
        .map_err(|error| HarnessError::Io(error.to_string()))
}

enum Command {
    Prompt {
        session: SessionState,
        prompt: PromptRequest,
    },
    Close {
        session: SessionState,
    },
    Shutdown,
}

pub trait TaskRuntime: Send + 'static {
    fn run_prompt(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
        response_published: &dyn Fn() -> bool,
    ) -> PromptStatus;
    fn close(&mut self, session: &SessionState, emit: &mut dyn FnMut(EventKind, bool, Value));
    fn backend_incarnation(&self) -> u64 {
        0
    }
}

pub struct HarnessService {
    handle: HarnessHandle,
    worker: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub struct HarnessHandle {
    shared: Arc<Mutex<Shared>>,
    commands: mpsc::SyncSender<Command>,
}

impl HarnessService {
    pub fn start(runtime: impl TaskRuntime, transcript_root: impl Into<PathBuf>) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            sessions: BTreeMap::new(),
            next_sequence: 1,
            queued: 0,
            transcript_root: transcript_root.into(),
        }));
        let (commands, receiver) = mpsc::sync_channel(PENDING_MAX);
        let handle = HarnessHandle {
            shared: Arc::clone(&shared),
            commands,
        };
        let worker = thread::spawn(move || worker_loop(runtime, shared, receiver));
        Self {
            handle,
            worker: Some(worker),
        }
    }

    pub fn handle(&self) -> HarnessHandle {
        self.handle.clone()
    }

    pub fn shutdown(mut self) {
        let _ = self.handle.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for HarnessService {
    fn drop(&mut self) {
        let _ = self.handle.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    mut runtime: impl TaskRuntime,
    shared: Arc<Mutex<Shared>>,
    receiver: mpsc::Receiver<Command>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Prompt { session, prompt } => {
                if let Ok(mut shared) = shared.lock() {
                    shared.queued = shared.queued.saturating_sub(1);
                    if let Some(data) = shared.sessions.get_mut(&session.session_id) {
                        data.state.status = SessionStatus::Busy;
                        data.state
                            .prompts
                            .insert(prompt.prompt_id.clone(), PromptStatus::Running);
                    }
                }
                let shared_emit = Arc::clone(&shared);
                let session_id = session.session_id.clone();
                let prompt_id = prompt.prompt_id.clone();
                let response_published = std::cell::Cell::new(false);
                let mut emit = |kind, public, payload| {
                    if let Ok(mut shared) = shared_emit.lock() {
                        let response =
                            public && matches!(kind, EventKind::Assistant | EventKind::Error);
                        // Cleanup diagnostics remain visible in Working, but
                        // must not become a second final response.
                        let public = public && !(response && response_published.get());
                        let published = shared
                            .emit(&session_id, Some(&prompt_id), kind, public, payload)
                            .is_ok();
                        if response && public {
                            response_published.set(published);
                        }
                    }
                };
                let cancel_shared = Arc::clone(&shared);
                let cancel_session = session.session_id.clone();
                let cancel_prompt = prompt.prompt_id.clone();
                let cancelled = move || {
                    cancel_shared
                        .lock()
                        .ok()
                        .and_then(|shared| {
                            shared
                                .sessions
                                .get(&cancel_session)
                                .map(|session| session.cancellations.contains(&cancel_prompt))
                        })
                        .unwrap_or(true)
                };
                let status = runtime.run_prompt(&session, &prompt, &mut emit, &cancelled, &|| {
                    response_published.get()
                });
                let backend_incarnation = runtime.backend_incarnation();
                if let Ok(mut shared) = shared.lock() {
                    if let Some(data) = shared.sessions.get_mut(&session.session_id) {
                        data.state.prompts.insert(prompt.prompt_id.clone(), status);
                        data.state.backend_incarnation = backend_incarnation;
                        data.state.status = if data.state.status == SessionStatus::Closing {
                            SessionStatus::Closing
                        } else if status == PromptStatus::Failed {
                            SessionStatus::Failed
                        } else {
                            SessionStatus::Open
                        };
                    }
                    let _ = shared.emit(
                        &session.session_id,
                        Some(&prompt.prompt_id),
                        EventKind::PromptTerminal,
                        true,
                        json!({"status": status}),
                    );
                }
            }
            Command::Close { session } => {
                let shared_emit = Arc::clone(&shared);
                let session_id = session.session_id.clone();
                let mut emit = |kind, public, payload| {
                    if let Ok(mut shared) = shared_emit.lock() {
                        let _ = shared.emit(&session_id, None, kind, public, payload);
                    }
                };
                runtime.close(&session, &mut emit);
                if let Ok(mut shared) = shared.lock() {
                    if let Some(data) = shared.sessions.get_mut(&session.session_id) {
                        data.state.status = SessionStatus::Closed;
                        data.state.generation = data.state.generation.saturating_add(1);
                    }
                    let _ = shared.emit(
                        &session.session_id,
                        None,
                        EventKind::ConversationClosed,
                        true,
                        json!({"kv_erased": true, "route_retired": true}),
                    );
                }
            }
            Command::Shutdown => break,
        }
    }
}

impl HarnessHandle {
    pub fn create_session(&self, request: CreateSession) -> Result<SessionState, HarnessError> {
        if request.api_version != HARNESS_API_VERSION {
            return Err(HarnessError::UnsupportedVersion);
        }
        if !valid_id(&request.project_id) || !valid_id(&request.session_id) {
            return Err(HarnessError::InvalidIdentity);
        }
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?;
        if let Some(existing) = shared.sessions.get(&request.session_id) {
            if existing.state.project_id == request.project_id {
                return Ok(existing.state.clone());
            }
            return Err(HarnessError::DuplicatePrompt);
        }
        let state = SessionState {
            api_version: HARNESS_API_VERSION,
            project_id: request.project_id,
            session_id: request.session_id.clone(),
            status: SessionStatus::Open,
            generation: 1,
            backend_incarnation: 0,
            last_event_sequence: 0,
            prompts: BTreeMap::new(),
        };
        shared.sessions.insert(
            request.session_id.clone(),
            SessionData {
                state: state.clone(),
                events: VecDeque::with_capacity(EVENTS_MAX),
                idempotency: BTreeMap::new(),
                cancellations: BTreeSet::new(),
            },
        );
        shared.emit(
            &request.session_id,
            None,
            EventKind::SessionOpened,
            true,
            json!({"provenance":"local_client"}),
        )?;
        Ok(shared.sessions[&request.session_id].state.clone())
    }

    pub fn submit_prompt(
        &self,
        session_id: &str,
        prompt: PromptRequest,
    ) -> Result<String, HarnessError> {
        if prompt.api_version != HARNESS_API_VERSION {
            return Err(HarnessError::UnsupportedVersion);
        }
        if !valid_id(&prompt.prompt_id) || !valid_id(&prompt.idempotency_key) {
            return Err(HarnessError::InvalidIdentity);
        }
        if prompt.text.is_empty() || prompt.text.len() > PROMPT_BYTES_MAX {
            return Err(HarnessError::PromptTooLarge);
        }
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?;
        if shared.queued >= PENDING_MAX {
            return Err(HarnessError::Backpressure);
        }
        let data = shared
            .sessions
            .get_mut(session_id)
            .ok_or(HarnessError::UnknownSession)?;
        if matches!(
            data.state.status,
            SessionStatus::Closing | SessionStatus::Closed
        ) {
            return Err(HarnessError::SessionClosed);
        }
        let prompt_digest = blake3::hash(
            serde_json::to_vec(&(&prompt.prompt_id, &prompt.text))
                .unwrap_or_default()
                .as_slice(),
        )
        .to_hex()
        .to_string();
        if let Some((existing, digest)) = data.idempotency.get(&prompt.idempotency_key) {
            return if existing == &prompt.prompt_id && digest == &prompt_digest {
                Ok(existing.clone())
            } else {
                Err(HarnessError::IdempotencyConflict)
            };
        }
        if data.state.prompts.contains_key(&prompt.prompt_id) {
            return Err(HarnessError::DuplicatePrompt);
        }
        data.idempotency.insert(
            prompt.idempotency_key.clone(),
            (prompt.prompt_id.clone(), prompt_digest),
        );
        data.state
            .prompts
            .insert(prompt.prompt_id.clone(), PromptStatus::Queued);
        let session = data.state.clone();
        shared.queued += 1;
        shared.emit(
            session_id,
            Some(&prompt.prompt_id),
            EventKind::UserPrompt,
            true,
            json!({
                "text": prompt.text,
                "intent_id": intent_identifier(&session, &prompt),
                "provenance": "frontend_task_intent"
            }),
        )?;
        let id = prompt.prompt_id.clone();
        if self
            .commands
            .try_send(Command::Prompt { session, prompt })
            .is_err()
        {
            if let Some(data) = shared.sessions.get_mut(session_id) {
                data.state.prompts.remove(&id);
                data.idempotency
                    .retain(|_, (prompt_id, _)| prompt_id != &id);
            }
            shared.queued = shared.queued.saturating_sub(1);
            return Err(HarnessError::Backpressure);
        }
        Ok(id)
    }

    pub fn cancel_prompt(
        &self,
        session_id: &str,
        request: CancelRequest,
    ) -> Result<(), HarnessError> {
        if request.api_version != HARNESS_API_VERSION {
            return Err(HarnessError::UnsupportedVersion);
        }
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?;
        let session = shared
            .sessions
            .get_mut(session_id)
            .ok_or(HarnessError::UnknownSession)?;
        if !session.state.prompts.contains_key(&request.prompt_id) {
            return Err(HarnessError::UnknownPrompt);
        }
        session.cancellations.insert(request.prompt_id);
        Ok(())
    }

    pub fn close_session(&self, session_id: &str) -> Result<(), HarnessError> {
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?;
        let data = shared
            .sessions
            .get_mut(session_id)
            .ok_or(HarnessError::UnknownSession)?;
        if data.state.status == SessionStatus::Closed {
            return Ok(());
        }
        data.state.status = SessionStatus::Closing;
        data.cancellations
            .extend(data.state.prompts.keys().cloned());
        let session = data.state.clone();
        drop(shared);
        self.commands
            .try_send(Command::Close { session })
            .map_err(|_| HarnessError::Backpressure)
    }

    pub fn state(&self, session_id: &str) -> Result<SessionState, HarnessError> {
        self.shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?
            .sessions
            .get(session_id)
            .map(|session| session.state.clone())
            .ok_or(HarnessError::UnknownSession)
    }

    pub fn events(&self, session_id: &str, after: u64) -> Result<Vec<HarnessEvent>, HarnessError> {
        let shared = self
            .shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?;
        let session = shared
            .sessions
            .get(session_id)
            .ok_or(HarnessError::UnknownSession)?;
        if let Some(first) = session.events.front()
            && after != 0
            && after.saturating_add(1) < first.sequence
        {
            return Err(HarnessError::EventCursorExpired);
        }
        Ok(session
            .events
            .iter()
            .filter(|event| event.sequence > after)
            .cloned()
            .collect())
    }
}

fn intent_identifier(session: &SessionState, prompt: &PromptRequest) -> String {
    let payload = serde_json::to_vec(&(
        HARNESS_API_VERSION,
        &session.project_id,
        &session.session_id,
        session.generation,
        &prompt.prompt_id,
        &prompt.idempotency_key,
        &prompt.text,
    ))
    .unwrap_or_default();
    blake3::hash(&payload).to_hex().to_string()
}

#[derive(Clone, Debug)]
pub struct GuestOffer {
    pub reference: [u8; 16],
    pub kind: BrokerOfferKind,
    pub permitted_ops: u64,
    pub max_uses: u64,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct BrokerDecision {
    pub plan: [u8; 16],
    pub action: BrokerActionKind,
    pub selectors: Vec<[u8; 16]>,
    pub label: String,
    pub budget: TaskBudget,
}

#[derive(Clone, Debug)]
pub struct GuestAgent {
    pub id: [u8; 16],
    pub generation: u64,
    pub tools: Vec<GuestTool>,
    pub tool_calls: u32,
}

#[derive(Clone, Debug)]
pub struct GuestTool {
    pub name: String,
    pub usage: String,
    pub summary: String,
    pub input: ToolInputSchema,
    pub effects: u16,
    pub result_sensitivity: u8,
    pub remaining_uses: u32,
}

pub trait GuestBridge: Send + 'static {
    fn offers(&self, session: &SessionState) -> Vec<GuestOffer>;
    fn admit(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        decision: &BrokerDecision,
    ) -> Result<GuestAgent, String>;
    fn execute(
        &mut self,
        agent: &GuestAgent,
        command: &str,
        arguments: &[OwnedArgument],
    ) -> Result<String, String>;
    fn close(&mut self, agent: &GuestAgent) -> Result<(), String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedArgument {
    Utf8(String),
    Path(String),
    U64(u64),
    Bool(bool),
}

fn encode_guest_tools(tools: &[GuestTool], output: &mut [u8]) -> Option<usize> {
    let mut length = 0;
    for tool in tools {
        length += (AgentToolDescriptor {
            name: &tool.name,
            usage: &tool.usage,
            summary: &tool.summary,
            input: tool.input,
            effects: tool.effects,
            result_sensitivity: tool.result_sensitivity,
            remaining_uses: tool.remaining_uses,
        })
        .encode(output.get_mut(length..)?)?;
    }
    Some(length)
}

#[derive(Default)]
pub struct DeterministicGuest {
    agents: BTreeMap<[u8; 16], DeterministicAgent>,
    objects: BTreeMap<String, String>,
}

struct DeterministicAgent {
    generation: u64,
    paths: BTreeSet<String>,
    tools: BTreeSet<String>,
}

impl DeterministicGuest {
    pub fn new() -> Self {
        Self {
            agents: BTreeMap::new(),
            objects: [
                (
                    "/X".into(),
                    "X: capability-scoped deterministic workspace object".into(),
                ),
                ("/Y".into(), "Y: not granted by a request for X".into()),
                (
                    "/identity".into(),
                    "vKore deterministic Kore-agent harness".into(),
                ),
            ]
            .into_iter()
            .collect(),
        }
    }
}

const OFFER_X: [u8; 16] = [0x11; 16];
const OFFER_Y: [u8; 16] = [0x22; 16];
const OFFER_CAT: [u8; 16] = [0x33; 16];
const OFFER_IDENTITY: [u8; 16] = [0x66; 16];

impl GuestBridge for DeterministicGuest {
    fn offers(&self, _: &SessionState) -> Vec<GuestOffer> {
        vec![
            GuestOffer {
                reference: OFFER_X,
                kind: BrokerOfferKind::Object,
                permitted_ops: 1,
                max_uses: 1,
                label: "workspace/X".into(),
            },
            GuestOffer {
                reference: OFFER_Y,
                kind: BrokerOfferKind::Object,
                permitted_ops: 1,
                max_uses: 1,
                label: "workspace/Y".into(),
            },
            GuestOffer {
                reference: OFFER_IDENTITY,
                kind: BrokerOfferKind::Object,
                permitted_ops: 1,
                max_uses: 1,
                label: "workspace/identity".into(),
            },
            GuestOffer {
                reference: OFFER_CAT,
                kind: BrokerOfferKind::Executable,
                permitted_ops: 4,
                max_uses: 8,
                label: "cat".into(),
            },
        ]
    }

    fn admit(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        decision: &BrokerDecision,
    ) -> Result<GuestAgent, String> {
        if decision.action != BrokerActionKind::CreateAgent {
            return Err("resident broker refused non-create deterministic plan".into());
        }
        let offered: BTreeSet<_> = self
            .offers(session)
            .into_iter()
            .map(|offer| offer.reference)
            .collect();
        if decision
            .selectors
            .iter()
            .any(|selector| !offered.contains(selector))
        {
            return Err("proposal selected a non-offered resource".into());
        }
        let cat_selected = decision.selectors.contains(&OFFER_CAT);
        if decision.budget.tool_calls != 0 && !cat_selected {
            return Err("proposal omitted the executable scope".into());
        }
        let mut paths = BTreeSet::new();
        if decision.selectors.contains(&OFFER_X) {
            paths.insert("/X".into());
        }
        if decision.selectors.contains(&OFFER_Y) {
            paths.insert("/Y".into());
        }
        if decision.selectors.contains(&OFFER_IDENTITY) {
            paths.insert("/identity".into());
        }
        let id = digest16(
            b"vkore.guest.agent",
            &[session.session_id.as_bytes(), prompt.prompt_id.as_bytes()],
        );
        let tool_calls = u32::try_from(decision.budget.tool_calls).unwrap_or(u32::MAX);
        let tools = if cat_selected && tool_calls != 0 {
            vec![GuestTool {
                name: "cat".into(),
                usage: "cat /absolute/path".into(),
                summary: "Read one object from the installed filesystem view.".into(),
                input: ToolInputSchema {
                    minimum: 1,
                    maximum: 1,
                    kinds: [ToolArgumentKind::Path.mask(), 0, 0, 0],
                    whitespace: 0,
                },
                effects: TOOL_EFFECT_OBSERVE,
                result_sensitivity: TOOL_RESULT_WORKSPACE,
                remaining_uses: tool_calls,
            }]
        } else {
            Vec::new()
        };
        let installed_tools = tools.iter().map(|tool| tool.name.clone()).collect();
        let agent = GuestAgent {
            id,
            generation: session.generation,
            tool_calls,
            tools,
        };
        self.agents.insert(
            id,
            DeterministicAgent {
                generation: agent.generation,
                paths,
                tools: installed_tools,
            },
        );
        Ok(agent)
    }

    fn execute(
        &mut self,
        agent: &GuestAgent,
        command: &str,
        arguments: &[OwnedArgument],
    ) -> Result<String, String> {
        let held = self.agents.get(&agent.id).ok_or("agent is not live")?;
        if held.generation != agent.generation {
            return Err("stale agent generation".into());
        }
        if !held.tools.contains(command) {
            return Err("executable is outside the installed scope".into());
        }
        let [OwnedArgument::Path(path)] = arguments else {
            return Err("cat requires one guest path".into());
        };
        if !held.paths.contains(path) {
            return Err("object is outside the installed filesystem view".into());
        }
        self.objects
            .get(path)
            .cloned()
            .ok_or_else(|| "object missing".into())
    }

    fn close(&mut self, agent: &GuestAgent) -> Result<(), String> {
        let Some(held) = self.agents.get(&agent.id) else {
            return Ok(());
        };
        if held.generation != agent.generation {
            return Err("stale agent generation".into());
        }
        self.agents.remove(&agent.id);
        Ok(())
    }
}

struct RuntimeSession {
    broker: SequenceIdentity,
    // The inference sequence owns bounded conversation history, not
    // authority. Prompt-scoped GuestAgents below can therefore rotate without
    // erasing the user's prior turns.
    agent_sequence: SequenceIdentity,
    turn: u64,
    incarnation: u64,
}

pub struct BackendHarness<B, G> {
    backend: B,
    guest: G,
    sessions: BTreeMap<String, RuntimeSession>,
    tick: u64,
}

impl<B: InferenceBackend + Send + 'static, G: GuestBridge> BackendHarness<B, G> {
    pub fn new(backend: B, guest: G) -> Self {
        Self {
            backend,
            guest,
            sessions: BTreeMap::new(),
            tick: 1,
        }
    }

    pub fn reset_backend(&mut self) {
        self.backend.reset();
    }

    fn open_session(&mut self, session: &SessionState) -> Result<(), String> {
        let generation = TaskGeneration::new(session.generation).ok_or("invalid generation")?;
        let broker = SequenceIdentity {
            route: RouteId(digest16(b"broker", &[session.session_id.as_bytes()])),
            generation,
            role: BackendRole::Broker,
        };
        self.backend
            .open(broker)
            .map_err(|error| format!("open broker: {error:?}"))?;
        let agent_sequence = SequenceIdentity {
            route: RouteId(digest16(b"agent", &[session.session_id.as_bytes()])),
            generation,
            role: BackendRole::Agent,
        };
        if let Err(error) = self.backend.open(agent_sequence) {
            let _ = self.backend.close(broker);
            return Err(format!("open agent: {error:?}"));
        }
        self.sessions.insert(
            session.session_id.clone(),
            RuntimeSession {
                broker,
                agent_sequence,
                turn: 0,
                incarnation: self.backend.metrics().incarnation,
            },
        );
        Ok(())
    }

    fn lease_prompt_authority(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<GuestAgent, String> {
        let broker = self
            .sessions
            .get(&session.session_id)
            .ok_or("runtime session is not open")?
            .broker;
        let offers = self.guest.offers(session);
        let borrowed: Vec<_> = offers
            .iter()
            .map(|offer| BrokerOffer {
                reference: offer.reference,
                kind: offer.kind,
                permitted_ops: offer.permitted_ops,
                max_uses: offer.max_uses,
                label: offer.label.as_str(),
            })
            .collect();
        let plan = digest16(
            b"plan",
            &[session.session_id.as_bytes(), prompt.prompt_id.as_bytes()],
        );
        let budget = TaskBudget {
            cpu_share: 1,
            cpu_time_ns: 5_000_000_000,
            memory_bytes: 256 * 1024 * 1024,
            scratch_bytes: 16 * 1024 * 1024,
            storage_bytes: 16 * 1024 * 1024,
            prompt_bytes: PROMPT_BYTES_MAX as u64,
            result_bytes: 64 * 1024,
            tool_calls: 8,
            worker_jobs: 1,
            outstanding_io: 4,
            wall_duration_ns: 30_000_000_000,
        };
        let mut inner = [0u8; kore_hostlink::INFERENCE_PART_MAX];
        let inner_len = TaskIntentRequest {
            plan,
            intent: &prompt.text,
            ceiling: budget,
            offers: &borrowed,
            attachments: &[],
        }
        .encode(&mut inner)
        .ok_or("task intent exceeds wire bounds")?;
        let payload = inference_record(InferenceRequestKind::BrokerPlan, 1, &inner[..inner_len])?;
        let request_id = request_id(session, &prompt.prompt_id, 0);
        self.backend
            .submit(broker, request_id, &payload, self.tick)
            .map_err(|error| format!("broker submit: {error:?}"))?;
        let reply = self.wait_completed(broker, request_id, emit, cancelled)?;
        let decoded = decode_broker_reply(&reply).ok_or("broker returned malformed proposal")?;
        if decoded.plan != plan {
            return Err("broker plan identity mismatch".into());
        }
        let decision = BrokerDecision {
            plan: decoded.plan,
            action: decoded.action,
            selectors: decoded.selectors().collect(),
            label: decoded.label.into(),
            budget: decoded.budget,
        };
        emit(
            EventKind::BrokerProposal,
            false,
            json!({
                "action": format!("{:?}", decision.action), "selectors": decision.selectors.iter().map(hex16).collect::<Vec<_>>(),
                "label": decision.label, "authenticated_intent": true
            }),
        );
        let guest_agent = self.guest.admit(session, prompt, &decision)?;
        let capabilities = offers
            .iter()
            .filter(|offer| decision.selectors.contains(&offer.reference))
            .map(|offer| {
                json!({
                    "kind": match offer.kind {
                        BrokerOfferKind::Object => "object",
                        BrokerOfferKind::View => "view",
                        BrokerOfferKind::Executable => "executable",
                        BrokerOfferKind::Budget => "budget",
                        BrokerOfferKind::ExistingAgent => "existing_agent",
                        BrokerOfferKind::ExistingGrant => "existing_grant",
                    },
                    "label": offer.label,
                })
            })
            .collect::<Vec<_>>();
        emit(
            EventKind::Capability,
            false,
            json!({
                "state":"installed", "agent":hex16(&guest_agent.id), "generation":guest_agent.generation,
                "tools":guest_agent.tools.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
                "capabilities": capabilities,
                "scope":"prompt", "prompt_id":prompt.prompt_id,
                "committed_by":"resident_broker_bridge"
            }),
        );
        Ok(guest_agent)
    }

    fn release_prompt_authority(
        &mut self,
        prompt: &PromptRequest,
        agent: &GuestAgent,
        emit: &mut dyn FnMut(EventKind, bool, Value),
    ) -> Result<(), String> {
        self.guest.close(agent)?;
        emit(
            EventKind::Capability,
            false,
            json!({
                "state":"teardown", "agent":hex16(&agent.id), "generation":agent.generation,
                "scope":"prompt", "prompt_id":prompt.prompt_id,
            }),
        );
        Ok(())
    }

    fn wait_completed(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<u8>, String> {
        for _ in 0..POLL_TICKS_MAX {
            self.tick = self.tick.saturating_add(1);
            if cancelled() {
                let _ = self.backend.cancel(sequence, request, self.tick);
                return Err("cancelled".into());
            }
            let event = self.backend.poll(self.tick);
            while let Some(observation) = self.backend.take_observation() {
                if matches!(
                    observation.kind,
                    ObservationKind::BrokerProposal
                        | ObservationKind::ToolCall
                        | ObservationKind::ToolResult
                        | ObservationKind::AssistantMessage
                ) {
                    continue;
                }
                emit(
                    EventKind::Inference,
                    false,
                    inference_observation_payload(&observation),
                );
            }
            match event {
                Some(BackendEvent::Completed {
                    sequence: event_sequence,
                    request: event_request,
                    payload,
                    ..
                }) if event_sequence == sequence && event_request == request => return Ok(payload),
                Some(BackendEvent::Failed {
                    sequence: event_sequence,
                    request: event_request,
                    failure,
                    ..
                }) if event_sequence == sequence && event_request == request => {
                    return Err(format!("backend failed: {failure:?}"));
                }
                Some(BackendEvent::Cancelled {
                    sequence: event_sequence,
                    request: event_request,
                    ..
                }) if event_sequence == sequence && event_request == request => {
                    return Err("cancelled".into());
                }
                _ => {}
            }
            thread::sleep(Duration::from_millis(1));
        }
        let _ = self.backend.cancel(sequence, request, self.tick);
        Err("backend timeout".into())
    }
}

impl<B: InferenceBackend + Send + 'static, G: GuestBridge> TaskRuntime for BackendHarness<B, G> {
    fn run_prompt(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
        _response_published: &dyn Fn() -> bool,
    ) -> PromptStatus {
        if !self.sessions.contains_key(&session.session_id)
            && let Err(error) = self.open_session(session)
        {
            emit(
                EventKind::Error,
                true,
                json!({"stage":"broker", "error":error}),
            );
            return if cancelled() {
                PromptStatus::Cancelled
            } else {
                PromptStatus::Failed
            };
        }
        let (sequence, incarnation, turn) = {
            let runtime = &self.sessions[&session.session_id];
            (
                runtime.agent_sequence,
                runtime.incarnation,
                runtime.turn + 1,
            )
        };
        if self.backend.metrics().incarnation != incarnation {
            emit(
                EventKind::Error,
                true,
                json!({"error":"backend restarted; route retired"}),
            );
            return PromptStatus::Failed;
        }
        let mut agent = match self.lease_prompt_authority(session, prompt, emit, cancelled) {
            Ok(agent) => agent,
            Err(error) => {
                emit(
                    EventKind::Error,
                    true,
                    json!({"stage":"broker", "error":error}),
                );
                return if cancelled() {
                    PromptStatus::Cancelled
                } else {
                    PromptStatus::Failed
                };
            }
        };
        let status = (|| {
            let mut tools = [0_u8; AGENT_TEXT_MAX];
            let Some(tools_len) = encode_guest_tools(&agent.tools, &mut tools) else {
                emit(
                    EventKind::Error,
                    true,
                    json!({"error":"agent tool catalog exceeded bounds"}),
                );
                return PromptStatus::Failed;
            };
            let mut last_tool_result = None;
            let next = AgentRequestView {
                kind: AgentRequestKind::UserText,
                turn,
                call_id: 0,
                result: None,
                tools: &tools[..tools_len],
                resources: &[],
                text: &prompt.text,
            };
            let mut inner = [0u8; kore_hostlink::INFERENCE_PART_MAX];
            let Some(inner_len) = next.encode(&mut inner) else {
                emit(
                    EventKind::Error,
                    true,
                    json!({"error":"agent request exceeded bounds"}),
                );
                return PromptStatus::Failed;
            };
            let payload = match inference_record(
                InferenceRequestKind::AgentTurn,
                next.turn,
                &inner[..inner_len],
            ) {
                Ok(payload) => payload,
                Err(error) => {
                    emit(EventKind::Error, true, json!({"error":error}));
                    return PromptStatus::Failed;
                }
            };
            let request = request_id(session, &prompt.prompt_id, next.turn);
            if let Err(error) = self.backend.submit(sequence, request, &payload, self.tick) {
                emit(
                    EventKind::Error,
                    true,
                    json!({"stage":"inference", "error":format!("{error:?}")}),
                );
                return PromptStatus::Failed;
            }
            emit(
                EventKind::Inference,
                false,
                json!({"role":"worker", "turn":next.turn, "state":"submitted"}),
            );
            let output = match self.wait_completed(sequence, request, emit, cancelled) {
                Ok(output) => output,
                Err(error) => {
                    emit(EventKind::Error, true, json!({"error":error}));
                    return if cancelled() {
                        PromptStatus::Cancelled
                    } else {
                        PromptStatus::Failed
                    };
                }
            };
            let Some(event) = decode_agent_event(&output) else {
                emit(
                    EventKind::Validation,
                    true,
                    json!({"accepted":false,"reason":"malformed agent event"}),
                );
                return PromptStatus::Failed;
            };
            emit(
                EventKind::Validation,
                false,
                json!({"accepted":true,"kind":format!("{:?}", event.kind),"turn":event.turn}),
            );
            if let Some(runtime) = self.sessions.get_mut(&session.session_id) {
                runtime.turn = event.turn;
            }
            match event.kind {
                AgentEventKind::ToolCall => {
                    if !consume_tool(&mut agent, event.command) {
                        emit(
                            EventKind::Error,
                            true,
                            json!({"error":"agent exceeded an installed tool-use limit"}),
                        );
                        return PromptStatus::Failed;
                    }
                    let arguments: Vec<_> = event.arguments().map(owned_argument).collect();
                    emit(
                        EventKind::ToolCall,
                        true,
                        json!({"command":event.command,"call_id":event.call_id,"arguments":format!("{arguments:?}"),"execution_domain":"guest"}),
                    );
                    let result = self.guest.execute(&agent, event.command, &arguments);
                    let (kind, text) = match result {
                        Ok(text) => (ToolResultKind::Output, text),
                        Err(error) => (ToolResultKind::Refused, error),
                    };
                    emit(
                        EventKind::ToolResult,
                        true,
                        json!({"call_id":event.call_id,"result":format!("{kind:?}"),"text":text,"execution_domain":"guest"}),
                    );
                    last_tool_result = Some(text.clone());
                    run_tool_continuation(
                        self,
                        session,
                        prompt,
                        emit,
                        cancelled,
                        sequence,
                        &mut agent,
                        event.turn + 1,
                        event.call_id,
                        kind,
                        text,
                        &mut last_tool_result,
                    )
                }
                AgentEventKind::Respond | AgentEventKind::AssistantText => {
                    emit(
                        EventKind::Assistant,
                        true,
                        json!({"text":event.text,"validated":true}),
                    );
                    PromptStatus::Completed
                }
                AgentEventKind::Completed => {
                    let text = last_tool_result.unwrap_or_else(|| "Completed.".into());
                    emit(
                        EventKind::Assistant,
                        true,
                        json!({"text":text,"validated":true}),
                    );
                    PromptStatus::Completed
                }
                AgentEventKind::Refusal => {
                    emit(
                        EventKind::Error,
                        true,
                        json!({"error":"agent refused request"}),
                    );
                    PromptStatus::Failed
                }
                AgentEventKind::Cancelled => PromptStatus::Cancelled,
            }
        })();
        if let Err(error) = self.release_prompt_authority(prompt, &agent, emit) {
            emit(
                EventKind::Error,
                true,
                json!({"stage":"authority_teardown", "error":error}),
            );
            return PromptStatus::Failed;
        }
        status
    }

    fn close(&mut self, session: &SessionState, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        if let Some(runtime) = self.sessions.remove(&session.session_id) {
            let _ = self.backend.close(runtime.agent_sequence);
            let _ = self.backend.close(runtime.broker);
            emit(
                EventKind::Capability,
                false,
                json!({"state":"teardown","scope":"session","generation":session.generation}),
            );
        }
    }

    fn backend_incarnation(&self) -> u64 {
        self.backend.metrics().incarnation
    }
}

#[allow(clippy::too_many_arguments)]
fn run_tool_continuation<B: InferenceBackend + Send + 'static, G: GuestBridge>(
    harness: &mut BackendHarness<B, G>,
    session: &SessionState,
    prompt: &PromptRequest,
    emit: &mut dyn FnMut(EventKind, bool, Value),
    cancelled: &dyn Fn() -> bool,
    sequence: SequenceIdentity,
    agent: &mut GuestAgent,
    mut turn: u64,
    mut call_id: u64,
    mut result_kind: ToolResultKind,
    mut owned_text: String,
    last_tool_result: &mut Option<String>,
) -> PromptStatus {
    let mut remaining_tool_calls = agent.tool_calls.saturating_sub(1);
    for _ in 0..agent.tool_calls {
        let mut tools = [0_u8; AGENT_TEXT_MAX];
        let Some(tools_len) = encode_guest_tools(&agent.tools, &mut tools) else {
            return PromptStatus::Failed;
        };
        let request_view = AgentRequestView {
            kind: AgentRequestKind::ToolResult,
            turn,
            call_id,
            result: Some(result_kind),
            tools: &tools[..tools_len],
            resources: &[],
            text: &owned_text,
        };
        let mut inner = [0u8; kore_hostlink::INFERENCE_PART_MAX];
        let Some(inner_len) = request_view.encode(&mut inner) else {
            return PromptStatus::Failed;
        };
        let payload = match inference_record(
            InferenceRequestKind::AgentTurn,
            request_view.turn,
            &inner[..inner_len],
        ) {
            Ok(payload) => payload,
            Err(_) => return PromptStatus::Failed,
        };
        let request = request_id(session, &prompt.prompt_id, request_view.turn);
        if harness
            .backend
            .submit(sequence, request, &payload, harness.tick)
            .is_err()
        {
            return PromptStatus::Failed;
        }
        let output = match harness.wait_completed(sequence, request, emit, cancelled) {
            Ok(output) => output,
            Err(error) => {
                emit(EventKind::Error, true, json!({"error":error}));
                return if cancelled() {
                    PromptStatus::Cancelled
                } else {
                    PromptStatus::Failed
                };
            }
        };
        let Some(event) = decode_agent_event(&output) else {
            return PromptStatus::Failed;
        };
        if let Some(runtime) = harness.sessions.get_mut(&session.session_id) {
            runtime.turn = event.turn;
        }
        emit(
            EventKind::Validation,
            false,
            json!({"accepted":true,"kind":format!("{:?}",event.kind),"turn":event.turn}),
        );
        match event.kind {
            AgentEventKind::ToolCall => {
                if remaining_tool_calls == 0 || !consume_tool(agent, event.command) {
                    emit(
                        EventKind::Error,
                        true,
                        json!({"error":"agent exceeded an installed tool-use limit"}),
                    );
                    return PromptStatus::Failed;
                }
                remaining_tool_calls -= 1;
                let arguments: Vec<_> = event.arguments().map(owned_argument).collect();
                emit(
                    EventKind::ToolCall,
                    true,
                    json!({"command":event.command,"call_id":event.call_id,"arguments":format!("{arguments:?}"),"execution_domain":"guest"}),
                );
                let result = harness.guest.execute(agent, event.command, &arguments);
                let (kind, text) = match result {
                    Ok(text) => (ToolResultKind::Output, text),
                    Err(error) => (ToolResultKind::Refused, error),
                };
                emit(
                    EventKind::ToolResult,
                    true,
                    json!({"call_id":event.call_id,"result":format!("{kind:?}"),"text":text,"execution_domain":"guest"}),
                );
                *last_tool_result = Some(text.clone());
                owned_text = text;
                turn = event.turn + 1;
                call_id = event.call_id;
                result_kind = kind;
            }
            AgentEventKind::Respond | AgentEventKind::AssistantText => {
                emit(
                    EventKind::Assistant,
                    true,
                    json!({"text":event.text,"validated":true}),
                );
                return PromptStatus::Completed;
            }
            AgentEventKind::Completed => {
                emit(
                    EventKind::Assistant,
                    true,
                    json!({"text":last_tool_result.clone().unwrap_or_else(|| "Completed.".into()),"validated":true}),
                );
                return PromptStatus::Completed;
            }
            AgentEventKind::Refusal => return PromptStatus::Failed,
            AgentEventKind::Cancelled => return PromptStatus::Cancelled,
        }
    }
    emit(
        EventKind::Error,
        true,
        json!({"error":"tool loop limit reached"}),
    );
    PromptStatus::Failed
}

fn consume_tool(agent: &mut GuestAgent, command: &str) -> bool {
    let Some(tool) = agent.tools.iter_mut().find(|tool| tool.name == command) else {
        return false;
    };
    if tool.remaining_uses == 0 {
        return false;
    }
    tool.remaining_uses -= 1;
    true
}

fn owned_argument(argument: ToolArgument<'_>) -> OwnedArgument {
    match argument {
        ToolArgument::Utf8(value) => OwnedArgument::Utf8(value.into()),
        ToolArgument::Path(value) => OwnedArgument::Path(value.into()),
        ToolArgument::U64(value) => OwnedArgument::U64(value),
        ToolArgument::Bool(value) => OwnedArgument::Bool(value),
    }
}

fn inference_record(
    kind: InferenceRequestKind,
    turn: u64,
    input: &[u8],
) -> Result<Vec<u8>, String> {
    let mut output = [0u8; kore_hostlink::INLINE_MAX];
    let len = InferenceRequestView {
        kind,
        turn,
        max_output_tokens: 512,
        timeout_ticks: POLL_TICKS_MAX as u32,
        input,
    }
    .encode(&mut output)
    .ok_or("inference request exceeds wire bounds")?;
    Ok(output[..len].to_vec())
}

fn request_id(session: &SessionState, prompt_id: &str, turn: u64) -> RequestId {
    RequestId(digest16(
        b"request",
        &[
            session.session_id.as_bytes(),
            prompt_id.as_bytes(),
            &turn.to_le_bytes(),
        ],
    ))
}

fn digest16(domain: &[u8], parts: &[&[u8]]) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    for part in parts {
        hasher.update(&((*part).len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let mut output = [0u8; 16];
    output.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    if output == [0; 16] {
        output[0] = 1;
    }
    output
}

fn hex16(value: &[u8; 16]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}
fn safe_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub struct HarnessHttpServer {
    server: tiny_http::Server,
    handle: HarnessHandle,
    address: SocketAddr,
    projects: Arc<ProjectStore>,
    approvals: Arc<UiApprovalService>,
    settings: Arc<SettingsStore>,
    transcript_root: PathBuf,
}

impl HarnessHttpServer {
    pub fn bind(address: SocketAddr, handle: HarnessHandle) -> Result<Self, HarnessError> {
        let root = std::env::temp_dir().join(format!("vkore-projects-{}", std::process::id()));
        Self::bind_with_state(address, handle, root)
    }

    pub fn bind_with_state(
        address: SocketAddr,
        handle: HarnessHandle,
        project_root: impl Into<PathBuf>,
    ) -> Result<Self, HarnessError> {
        if !address.ip().is_loopback() {
            return Err(HarnessError::NonLoopback);
        }
        let project_root = project_root.into();
        let state_root = project_root.parent().unwrap_or(&project_root).to_path_buf();
        let server = tiny_http::Server::http(address)
            .map_err(|error| HarnessError::Http(error.to_string()))?;
        let address = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| HarnessError::Http("HTTP server did not bind an IP address".into()))?;
        Ok(Self {
            server,
            handle,
            address,
            projects: Arc::new(
                ProjectStore::open(&project_root)
                    .map_err(|error| HarnessError::Io(error.to_string()))?,
            ),
            approvals: Arc::new(
                UiApprovalService::open(&project_root.join("policy")).map_err(HarnessError::Io)?,
            ),
            settings: Arc::new(
                SettingsStore::open(state_root.join("settings.json"))
                    .map_err(|error| HarnessError::Io(error.to_string()))?,
            ),
            transcript_root: state_root.join("transcripts"),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    pub fn poll(&self, timeout: Duration) -> Result<bool, HarnessError> {
        let Some(mut request) = self
            .server
            .recv_timeout(timeout)
            .map_err(|error| HarnessError::Http(error.to_string()))?
        else {
            return Ok(false);
        };
        let host = header_value(&request, "Host").unwrap_or_default();
        let origin = header_value(&request, "Origin");
        let ui_client = header_value(&request, "X-vKore-Client");
        let method = request.method().as_str().to_owned();
        let url = request.url().to_owned();
        let mut body = Vec::new();
        request
            .as_reader()
            .take(65_537)
            .read_to_end(&mut body)
            .map_err(|error| HarnessError::Http(error.to_string()))?;
        let result = validate_browser_request(
            self.address,
            &method,
            &host,
            origin.as_deref(),
            ui_client.as_deref(),
        )
        .and_then(|_| {
            route_http(
                HttpRouteState {
                    handle: &self.handle,
                    projects: &self.projects,
                    approvals: &self.approvals,
                    settings: &self.settings,
                    transcript_root: &self.transcript_root,
                },
                &method,
                &url,
                &body,
            )
        });
        let (status, content_type, response) = match result {
            Ok((content_type, body)) => (200, content_type, body),
            Err(error) => {
                let status = match error {
                    HarnessError::UntrustedBrowserRequest => 403,
                    HarnessError::UnknownSession | HarnessError::UnknownPrompt => 404,
                    HarnessError::IdempotencyConflict | HarnessError::DuplicatePrompt => 409,
                    HarnessError::Backpressure | HarnessError::Busy => 429,
                    _ => 400,
                };
                (
                    status,
                    "application/json",
                    serde_json::to_vec(&json!({"error":format!("{error:?}")})).unwrap_or_default(),
                )
            }
        };
        let mut response = tiny_http::Response::from_data(response).with_status_code(status);
        if let Ok(header) = tiny_http::Header::from_bytes("Content-Type", content_type) {
            response = response.with_header(header);
        }
        if let Ok(header) = tiny_http::Header::from_bytes("Cache-Control", "no-store") {
            response = response.with_header(header);
        }
        request
            .respond(response)
            .map_err(|error| HarnessError::Http(error.to_string()))?;
        Ok(true)
    }
}

struct HttpRouteState<'a> {
    handle: &'a HarnessHandle,
    projects: &'a ProjectStore,
    approvals: &'a UiApprovalService,
    settings: &'a SettingsStore,
    transcript_root: &'a Path,
}

fn route_http(
    state: HttpRouteState<'_>,
    method: &str,
    url: &str,
    body: &[u8],
) -> Result<(&'static str, Vec<u8>), HarnessError> {
    let HttpRouteState {
        handle,
        projects,
        approvals,
        settings,
        transcript_root,
    } = state;
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    if method == "GET" && path == "/" {
        return Ok((
            "application/json",
            serde_json::to_vec(&json!({
                "api_version": HARNESS_API_VERSION,
                "client": "vkore-ui",
                "renderer": "axiom",
                "message": "Launch the native vKore UI and connect it to this loopback address."
            }))
            .unwrap(),
        ));
    }
    if method == "GET" && path == "/api/v1/health" {
        return Ok((
            "application/json",
            serde_json::to_vec(&json!({"ok":true,"api_version":HARNESS_API_VERSION})).unwrap(),
        ));
    }
    let segments: Vec<_> = path.trim_matches('/').split('/').collect();
    let value = match (method, segments.as_slice()) {
        ("GET", ["api", "v1", "catalog"]) => json!({
            "api_version": HARNESS_API_VERSION,
            "backends": [
                {"id":"llama_cpp","name":"llama.cpp","real_inference":true,"status":"caller_supplied"}
            ],
            "addons": [
                {"id":"git","name":"Git","version":"0.1.0","publisher":"MicroPerceptron","domain":"host","tools":[
                    {"name":"status","description":"Show branch and working-tree status for one exact repository.","risk":"read_only"},
                    {"name":"diff","description":"Show a bounded working-tree or staged patch, optionally for one path.","risk":"read_only"},
                    {"name":"log","description":"Return bounded structured commit history from one exact repository.","risk":"read_only"},
                    {"name":"show","description":"Show one exact Git revision and optional repository-relative path.","risk":"read_only"},
                    {"name":"stage","description":"Stage an explicit list of repository-relative paths.","risk":"external_write"},
                    {"name":"unstage","description":"Remove explicit paths from the index without changing working files.","risk":"external_write"},
                    {"name":"commit","description":"Commit the already-staged index with an explicit message and author identity.","risk":"external_write"}
                ],"network_destinations":[]},
                {"id":"gmail","name":"Gmail","version":"0.1.0","publisher":"MicroPerceptron","domain":"host","tools":[
                    {"name":"search","description":"Search the connected Gmail mailbox and return bounded message identifiers.","risk":"read_only"},
                    {"name":"read","description":"Read one exact Gmail message by identifier.","risk":"read_only"},
                    {"name":"send","description":"Send one plain-text email from the connected Gmail account.","risk":"external_write"},
                    {"name":"create_draft","description":"Create one plain-text draft in the connected Gmail account.","risk":"external_write"},
                    {"name":"trash","description":"Move one exact Gmail message to trash.","risk":"destructive"}
                ],"network_destinations":["gmail.googleapis.com:443"]},
                {"id":"reference.host","name":"Reference host add-on","version":"0.1.0","publisher":"MicroPerceptron","domain":"host","tools":[{"name":"echo","description":"Return a bounded message from an isolated host add-on process.","risk":"read_only"}],"network_destinations":[]},
                {"id":"reference.guest","name":"Reference guest add-on","version":"0.1.0","publisher":"MicroPerceptron","domain":"guest","tools":[{"name":"list_workspace","description":"List names in the agent's capability-scoped Kore workspace.","risk":"read_only"}],"network_destinations":[]}
            ],
            "package_actions":{"install":"Use vkore-packages verified local import or download APIs","automatic_download":false}
        }),
        ("GET", ["api", "v1", "projects"]) => serde_json::to_value(projects.list()).unwrap(),
        ("GET", ["api", "v1", "settings"]) => settings.view(projects).map_err(settings_error)?,
        ("POST", ["api", "v1", "settings"]) => {
            let patch: Value = serde_json::from_slice(body).map_err(http_json)?;
            settings.update(&patch, projects).map_err(settings_error)?
        }
        ("GET", ["api", "v1", "activity"]) => serde_json::to_value(
            crate::settings::recent_activity(transcript_root, projects).map_err(settings_error)?,
        )
        .unwrap(),
        ("GET", ["api", "v1", "approvals"]) => approvals.state().map_err(HarnessError::Io)?,
        ("POST", ["api", "v1", "projects", project, "approvals"]) => {
            let value: Value = serde_json::from_slice(body).map_err(http_json)?;
            approvals
                .request(
                    project,
                    value["session_id"].as_str().unwrap_or("project-setup"),
                    value["addon_id"]
                        .as_str()
                        .ok_or(HarnessError::InvalidIdentity)?,
                    value["tool"]
                        .as_str()
                        .ok_or(HarnessError::InvalidIdentity)?,
                )
                .map_err(HarnessError::Io)?
        }
        ("POST", ["api", "v1", "approvals", request_id, "decision"]) => {
            let value: Value = serde_json::from_slice(body).map_err(http_json)?;
            approvals
                .decide(
                    request_id,
                    value["allow"].as_bool().unwrap_or(false),
                    value["lifetime"].as_str().unwrap_or("once"),
                    value["ui_session"].as_str().unwrap_or("harness-ui"),
                )
                .map_err(HarnessError::Io)?
        }
        ("POST", ["api", "v1", "grants", grant_id, "revoke"]) => {
            let value: Value = serde_json::from_slice(body).map_err(http_json)?;
            approvals
                .revoke(
                    grant_id,
                    value["ui_session"].as_str().unwrap_or("harness-ui"),
                )
                .map_err(HarnessError::Io)?
        }
        ("POST", ["api", "v1", "projects", "preflight"]) => {
            let mut config: ProjectConfig = serde_json::from_slice(body).map_err(http_json)?;
            if config.workspace.state.as_os_str().is_empty() {
                config.workspace.state = projects.derived_state(&config.id, &config.workspace.root);
            }
            serde_json::to_value(crate::project::preflight(&config).map_err(project_error)?)
                .unwrap()
        }
        ("POST", ["api", "v1", "projects"]) => {
            let mut config: ProjectConfig = serde_json::from_slice(body).map_err(http_json)?;
            if config.workspace.state.as_os_str().is_empty() {
                config.workspace.state = projects.derived_state(&config.id, &config.workspace.root);
            }
            serde_json::to_value(projects.save(config).map_err(project_error)?).unwrap()
        }
        ("POST", ["api", "v1", "projects", "from-workspace"]) => {
            let value: Value = serde_json::from_slice(body).map_err(http_json)?;
            let root = PathBuf::from(
                value["root"]
                    .as_str()
                    .ok_or(HarnessError::InvalidIdentity)?,
            );
            let inference = settings
                .default_inference(projects)
                .map_err(settings_error)?;
            serde_json::to_value(
                projects
                    .create_from_workspace(&root, inference)
                    .map_err(project_error)?,
            )
            .unwrap()
        }
        ("GET", ["api", "v1", "projects", project]) => {
            serde_json::to_value(projects.get(project).map_err(project_error)?).unwrap()
        }
        ("POST", ["api", "v1", "projects", project, "archive"]) => {
            let value: Value = serde_json::from_slice(body).map_err(http_json)?;
            serde_json::to_value(
                projects
                    .archive(project, value["archived"].as_bool().unwrap_or(true))
                    .map_err(project_error)?,
            )
            .unwrap()
        }
        ("POST", ["api", "v1", "projects", project, "duplicate"]) => {
            let value: Value = serde_json::from_slice(body).map_err(http_json)?;
            let id = value["id"].as_str().ok_or(HarnessError::InvalidIdentity)?;
            let name = value["name"]
                .as_str()
                .ok_or(HarnessError::InvalidIdentity)?;
            serde_json::to_value(
                projects
                    .duplicate(project, id, name)
                    .map_err(project_error)?,
            )
            .unwrap()
        }
        ("POST", ["api", "v1", "picker"]) => {
            let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            let kind = value["kind"].as_str().unwrap_or("directory");
            let path = native_picker(kind == "directory")?;
            json!({"path":path})
        }
        ("POST", ["api", "v1", "artifacts", "inspect"]) => {
            let value: Value = serde_json::from_slice(body).map_err(http_json)?;
            let path = PathBuf::from(
                value["path"]
                    .as_str()
                    .ok_or(HarnessError::InvalidIdentity)?,
            );
            inspect_artifact(&path)?
        }
        ("POST", ["api", "v1", "sessions"]) => {
            let request: CreateSession = serde_json::from_slice(body)
                .map_err(|error| HarnessError::Http(error.to_string()))?;
            serde_json::to_value(handle.create_session(request)?).unwrap()
        }
        ("POST", ["api", "v1", "sessions", session, "prompts"]) => {
            let request: PromptRequest = serde_json::from_slice(body)
                .map_err(|error| HarnessError::Http(error.to_string()))?;
            json!({"prompt_id":handle.submit_prompt(session, request)?})
        }
        (
            "POST",
            [
                "api",
                "v1",
                "sessions",
                session,
                "prompts",
                prompt_id,
                "cancel",
            ],
        ) => {
            let request: CancelRequest = serde_json::from_slice(body)
                .map_err(|error| HarnessError::Http(error.to_string()))?;
            if request.prompt_id != *prompt_id {
                return Err(HarnessError::InvalidIdentity);
            }
            handle.cancel_prompt(session, request)?;
            json!({"cancelled":true})
        }
        ("GET", ["api", "v1", "sessions", session]) => {
            serde_json::to_value(handle.state(session)?).unwrap()
        }
        ("GET", ["api", "v1", "sessions", session, "events"]) => {
            let after = query
                .strip_prefix("after=")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            return Ok((
                "text/event-stream",
                encode_sse(&handle.events(session, after)?),
            ));
        }
        ("DELETE", ["api", "v1", "sessions", session]) => {
            handle.close_session(session)?;
            json!({"closing":true})
        }
        _ => return Err(HarnessError::Http("not found".into())),
    };
    Ok((
        "application/json",
        serde_json::to_vec(&value).unwrap_or_default(),
    ))
}

fn header_value(request: &tiny_http::Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str().to_owned())
}

fn validate_browser_request(
    address: SocketAddr,
    method: &str,
    host: &str,
    origin: Option<&str>,
    ui_client: Option<&str>,
) -> Result<(), HarnessError> {
    let local_host = browser_host_name(host).is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || matches!(host, "127.0.0.1" | "::1")
    });
    if !local_host {
        return Err(HarnessError::UntrustedBrowserRequest);
    }
    if let Some(origin) = origin {
        let allowed = [
            format!("http://{}", address),
            format!("http://localhost:{}", address.port()),
            format!("http://127.0.0.1:{}", address.port()),
        ];
        if !allowed.iter().any(|value| value == origin) {
            return Err(HarnessError::UntrustedBrowserRequest);
        }
        if method != "GET" && ui_client != Some("harness-ui-v1") {
            return Err(HarnessError::UntrustedBrowserRequest);
        }
    }
    Ok(())
}

fn browser_host_name(host: &str) -> Option<&str> {
    let host = host.trim();
    if let Some(bracketed) = host.strip_prefix('[') {
        let (name, suffix) = bracketed.split_once(']')?;
        return valid_optional_port(suffix).then_some(name);
    }

    match host.split_once(':') {
        Some((name, port)) if !name.is_empty() && valid_port(port) => Some(name),
        Some(_) => None,
        None if !host.is_empty() => Some(host),
        None => None,
    }
}

fn valid_optional_port(suffix: &str) -> bool {
    suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port)
}

fn valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port.parse::<u16>().is_ok()
}

fn http_json(error: serde_json::Error) -> HarnessError {
    HarnessError::Http(error.to_string())
}
fn project_error(error: crate::project::ProjectError) -> HarnessError {
    HarnessError::Http(error.to_string())
}
fn settings_error(error: crate::settings::SettingsError) -> HarnessError {
    HarnessError::Http(error.to_string())
}

fn inspect_artifact(path: &Path) -> Result<Value, HarnessError> {
    let mut file =
        std::fs::File::open(path).map_err(|error| HarnessError::Io(error.to_string()))?;
    let metadata = file
        .metadata()
        .map_err(|error| HarnessError::Io(error.to_string()))?;
    if !metadata.is_file() {
        return Err(HarnessError::InvalidIdentity);
    }
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| HarnessError::Io(error.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact");
    let quantization = [
        "Q2_K", "Q3_K", "Q4_K_M", "Q4_K_S", "Q5_K_M", "Q5_K_S", "Q6_K", "Q8_0", "F16",
    ]
    .into_iter()
    .find(|quant| name.to_ascii_uppercase().contains(quant))
    .unwrap_or("unknown");
    Ok(json!({
        "path": path,
        "name": name,
        "bytes": metadata.len(),
        "blake3": hasher.finalize().to_hex().to_string(),
        "family": name.split(['-', '.']).next().unwrap_or("unknown"),
        "quantization": quantization,
        "estimated_memory_bytes": metadata.len().saturating_mul(6).saturating_div(5),
        "license": "read from verified package manifest when installed",
        "context_tokens": "configured by project; verify against GGUF metadata"
    }))
}

#[cfg(target_os = "windows")]
fn native_picker(directory: bool) -> Result<Option<PathBuf>, HarnessError> {
    let script = if directory {
        "Add-Type -AssemblyName System.Windows.Forms; $d=New-Object System.Windows.Forms.FolderBrowserDialog; if($d.ShowDialog() -eq 'OK'){[Console]::Out.Write($d.SelectedPath)}"
    } else {
        "Add-Type -AssemblyName System.Windows.Forms; $d=New-Object System.Windows.Forms.OpenFileDialog; if($d.ShowDialog() -eq 'OK'){[Console]::Out.Write($d.FileName)}"
    };
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-STA", "-Command", script])
        .output()
        .map_err(|error| HarnessError::Io(error.to_string()))?;
    if !output.status.success() {
        return Err(HarnessError::Io(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty()).then(|| PathBuf::from(value)))
}

#[cfg(target_os = "linux")]
fn native_picker(directory: bool) -> Result<Option<PathBuf>, HarnessError> {
    let mut command = std::process::Command::new("zenity");
    command.arg("--file-selection");
    if directory {
        command.arg("--directory");
    }
    let output = command
        .output()
        .map_err(|error| HarnessError::Io(error.to_string()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty()).then(|| PathBuf::from(value)))
}

#[cfg(target_os = "macos")]
fn native_picker(directory: bool) -> Result<Option<PathBuf>, HarnessError> {
    let script = if directory {
        "POSIX path of (choose folder)"
    } else {
        "POSIX path of (choose file)"
    };
    let output = std::process::Command::new("osascript")
        .args(["-e", script])
        .output()
        .map_err(|error| HarnessError::Io(error.to_string()))?;
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!value.is_empty()).then(|| PathBuf::from(value)))
}

fn encode_sse(events: &[HarnessEvent]) -> Vec<u8> {
    let mut output = Vec::new();
    for event in events {
        let _ = writeln!(output, "id: {}", event.sequence);
        let _ = writeln!(output, "event: {:?}", event.kind);
        let _ = writeln!(
            output,
            "data: {}\n",
            serde_json::to_string(event).unwrap_or_else(|_| "{}".into())
        );
    }
    output
}

pub fn loopback_ip(address: SocketAddr) -> bool {
    matches!(address.ip(), IpAddr::V4(ip) if ip.is_loopback())
        || matches!(address.ip(), IpAddr::V6(ip) if ip.is_loopback())
}
