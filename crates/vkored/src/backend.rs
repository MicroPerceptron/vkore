//! Bounded inference-backend seam and deterministic reference implementation.

use kore_hostlink::{
    AgentEventKind, AgentEventView, AgentRequestKind, BrokerActionKind, BrokerOfferKind,
    BrokerPlanningReply, BulkId, INFERENCE_REQUEST_MAGIC, InferenceRequestKind, RequestId, RouteId,
    TaskBudget, TaskGeneration, ToolArgument, ToolResultKind, decode_agent_request,
    decode_inference_request, decode_task_intent,
};
use std::collections::VecDeque;

pub const SEQUENCES_MAX: usize = 32;
pub const BROKER_SEQUENCES_MAX: usize = 8;
pub const WORKSPACE_SEQUENCES_MAX: usize = 1;
pub const AGENT_SEQUENCES_MAX: usize = SEQUENCES_MAX - BROKER_SEQUENCES_MAX;
pub const PENDING_PER_SEQUENCE_MAX: usize = 16;
pub const PENDING_TOTAL_MAX: usize = SEQUENCES_MAX * PENDING_PER_SEQUENCE_MAX;
pub const REQUEST_HISTORY_MAX: usize = 256;
pub const CONTEXT_BYTES_MAX: usize = 256 * 1024;
pub const OUTPUT_BYTES_MAX: usize = 64 * 1024;
pub const TOKENS_PER_SEQUENCE_MAX: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendRole {
    Broker,
    Agent,
    Workspace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequenceIdentity {
    pub route: RouteId,
    pub generation: TaskGeneration,
    pub role: BackendRole,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendLimits {
    pub sequences: usize,
    pub brokers: usize,
    pub agents: usize,
    pub pending_per_sequence: usize,
    pub request_history: usize,
    pub context_bytes: usize,
    pub output_bytes: usize,
    pub tokens_per_sequence: u64,
    pub max_output_tokens: u32,
    pub request_timeout_ticks: u64,
}

impl Default for BackendLimits {
    fn default() -> Self {
        Self {
            sequences: SEQUENCES_MAX,
            brokers: BROKER_SEQUENCES_MAX,
            agents: AGENT_SEQUENCES_MAX,
            pending_per_sequence: PENDING_PER_SEQUENCE_MAX,
            request_history: REQUEST_HISTORY_MAX,
            context_bytes: CONTEXT_BYTES_MAX,
            output_bytes: OUTPUT_BYTES_MAX,
            tokens_per_sequence: TOKENS_PER_SEQUENCE_MAX,
            max_output_tokens: 1024,
            request_timeout_ticks: 30_000,
        }
    }
}

impl BackendLimits {
    pub fn validate(self) -> Result<Self, BackendError> {
        if self.sequences == 0
            || self.sequences > SEQUENCES_MAX
            || self.brokers > BROKER_SEQUENCES_MAX
            || self.agents > AGENT_SEQUENCES_MAX
            || self.brokers.saturating_add(self.agents) < self.sequences
            || self.pending_per_sequence == 0
            || self.pending_per_sequence > PENDING_PER_SEQUENCE_MAX
            || self.request_history == 0
            || self.request_history > REQUEST_HISTORY_MAX
            || self.context_bytes == 0
            || self.context_bytes > CONTEXT_BYTES_MAX
            || self.output_bytes == 0
            || self.output_bytes > OUTPUT_BYTES_MAX
            || self.tokens_per_sequence == 0
            || self.tokens_per_sequence > TOKENS_PER_SEQUENCE_MAX
            || self.max_output_tokens == 0
            || self.request_timeout_ticks == 0
        {
            return Err(BackendError::InvalidLimits);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RequestUsage {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendFailure {
    TimedOut,
    HostDisconnected,
    MalformedResponse,
    ModelFailed,
    BackendRestarted,
    OutputLimit,
    ContextLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendEvent {
    Chunk {
        sequence: SequenceIdentity,
        request: RequestId,
        payload: Vec<u8>,
        part: u16,
        submitted_tick: u64,
        turn: u64,
    },
    Completed {
        sequence: SequenceIdentity,
        request: RequestId,
        payload: Vec<u8>,
        part: u16,
        submitted_tick: u64,
        usage: RequestUsage,
        turn: u64,
    },
    /// A terminal response whose bytes must travel on the bounded bulk plane.
    /// The enclosing route/request still supply correlation; the backend
    /// cannot choose another authority-bearing endpoint.
    BulkCompleted {
        sequence: SequenceIdentity,
        request: RequestId,
        bulk_id: BulkId,
        content_kind: u16,
        payload: Vec<u8>,
        submitted_tick: u64,
    },
    Cancelled {
        sequence: SequenceIdentity,
        request: RequestId,
        submitted_tick: u64,
        turn: u64,
    },
    Failed {
        sequence: SequenceIdentity,
        request: RequestId,
        submitted_tick: u64,
        failure: BackendFailure,
        turn: u64,
    },
}

impl BackendEvent {
    #[must_use]
    pub const fn sequence(&self) -> SequenceIdentity {
        match self {
            Self::Chunk { sequence, .. }
            | Self::Completed { sequence, .. }
            | Self::BulkCompleted { sequence, .. }
            | Self::Cancelled { sequence, .. }
            | Self::Failed { sequence, .. } => *sequence,
        }
    }

    #[must_use]
    pub const fn request(&self) -> RequestId {
        match self {
            Self::Chunk { request, .. }
            | Self::Completed { request, .. }
            | Self::BulkCompleted { request, .. }
            | Self::Cancelled { request, .. }
            | Self::Failed { request, .. } => *request,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SequenceMetrics {
    pub submitted: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub first_token_ticks_total: u64,
    pub cancellation_ticks_total: u64,
    pub queue_ticks_total: u64,
    /// Denominator for completion-token throughput; callers choose the tick
    /// duration of their transport loop.
    pub generation_ticks_total: u64,
    pub peak_pending: usize,
    pub context_bytes: usize,
    pub context_tokens: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendMetrics {
    pub incarnation: u64,
    pub restarts: u64,
    pub stale_events: u64,
    pub sequences_open: usize,
    pub pending: usize,
    pub peak_pending: usize,
    pub submitted: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub first_token_ticks_total: u64,
    pub cancellation_ticks_total: u64,
    pub queue_ticks_total: u64,
    pub generation_ticks_total: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendError {
    InvalidLimits,
    InvalidIdentity,
    InvalidRole,
    SequenceCapacity,
    RoleCapacity,
    RouteOccupied,
    RouteUnknown,
    GenerationStale,
    RoleMismatch,
    SequenceClosed,
    Full,
    Duplicate,
    OutOfOrder,
    RequestUnknown,
    InputTooLarge,
    InvalidEncoding,
    ContextLimit,
    TokenLimit,
    OutputLimit,
    TimeoutLimit,
    BackendUnavailable,
}

pub trait InferenceBackend {
    fn reset(&mut self);
    fn open(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError>;
    fn close(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError>;
    fn submit(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError>;
    fn submit_bulk(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        _bulk_id: BulkId,
        content_kind: u16,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        if content_kind != kore_hostlink::INFERENCE_REQUEST_CONTENT {
            return Err(BackendError::InvalidRole);
        }
        self.submit(sequence, request, payload, tick)
    }
    fn cancel(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        tick: u64,
    ) -> Result<BackendEvent, BackendError>;
    fn poll(&mut self, tick: u64) -> Option<BackendEvent>;
    fn metrics(&self) -> BackendMetrics;
    fn sequence_metrics(&self, sequence: SequenceIdentity) -> Option<SequenceMetrics>;
    /// Pop one bounded, non-authoritative observation produced by the real
    /// backend path. Implementations that do not expose model internals keep
    /// the default empty stream.
    fn take_observation(&mut self) -> Option<InferenceObservation> {
        None
    }
}

impl<T: InferenceBackend + ?Sized> InferenceBackend for Box<T> {
    fn reset(&mut self) {
        (**self).reset();
    }

    fn open(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError> {
        (**self).open(sequence)
    }

    fn close(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError> {
        (**self).close(sequence)
    }

    fn submit(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        (**self).submit(sequence, request, payload, tick)
    }

    fn submit_bulk(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        bulk_id: BulkId,
        content_kind: u16,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        (**self).submit_bulk(sequence, request, bulk_id, content_kind, payload, tick)
    }

    fn cancel(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        tick: u64,
    ) -> Result<BackendEvent, BackendError> {
        (**self).cancel(sequence, request, tick)
    }

    fn poll(&mut self, tick: u64) -> Option<BackendEvent> {
        (**self).poll(tick)
    }

    fn metrics(&self) -> BackendMetrics {
        (**self).metrics()
    }

    fn sequence_metrics(&self, sequence: SequenceIdentity) -> Option<SequenceMetrics> {
        (**self).sequence_metrics(sequence)
    }

    fn take_observation(&mut self) -> Option<InferenceObservation> {
        (**self).take_observation()
    }
}

/// Host-only projection for the live inspector. Route/generation/request are
/// correlation, never capability identifiers. Payloads are explicitly
/// bounded by each backend before insertion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InferenceObservation {
    pub role: BackendRole,
    pub route: RouteId,
    pub generation: TaskGeneration,
    pub request: RequestId,
    pub turn: u64,
    pub kind: ObservationKind,
    pub payload: String,
    pub accepted: Option<bool>,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationKind {
    Input,
    ModelPrompt,
    RawModelCompletion,
    ReturnedModelFields,
    Validation,
    AssistantMessage,
    ToolCall,
    ToolResult,
    BrokerProposal,
}

pub(crate) struct RegistryEntry<T> {
    pub(crate) identity: SequenceIdentity,
    pub(crate) slot: u16,
    pub(crate) state: T,
}

pub(crate) struct SequenceRegistry<T> {
    pub(crate) entries: Vec<RegistryEntry<T>>,
    pub(crate) limits: BackendLimits,
}

impl<T> SequenceRegistry<T> {
    pub(crate) fn new(limits: BackendLimits) -> Result<Self, BackendError> {
        let limits = limits.validate()?;
        Ok(Self {
            entries: Vec::with_capacity(limits.sequences),
            limits,
        })
    }

    pub(crate) fn open(
        &mut self,
        identity: SequenceIdentity,
        state: impl FnOnce(u16) -> T,
    ) -> Result<(), BackendError> {
        if identity.route.is_zero() {
            return Err(BackendError::InvalidIdentity);
        }
        if self.entries.iter().any(|entry| entry.identity == identity) {
            return Err(BackendError::RouteOccupied);
        }
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.identity.route == identity.route)
        {
            return Err(if entry.identity.generation != identity.generation {
                BackendError::GenerationStale
            } else {
                BackendError::RoleMismatch
            });
        }
        if self.entries.len() == self.limits.sequences {
            return Err(BackendError::SequenceCapacity);
        }
        let role_count = self
            .entries
            .iter()
            .filter(|entry| entry.identity.role == identity.role)
            .count();
        let role_limit = match identity.role {
            BackendRole::Broker => self.limits.brokers,
            BackendRole::Agent => self.limits.agents,
            BackendRole::Workspace => WORKSPACE_SEQUENCES_MAX,
        };
        if role_count == role_limit {
            return Err(BackendError::RoleCapacity);
        }
        let slot = (0..self.limits.sequences as u16)
            .find(|slot| self.entries.iter().all(|entry| entry.slot != *slot))
            .ok_or(BackendError::SequenceCapacity)?;
        self.entries.push(RegistryEntry {
            identity,
            slot,
            state: state(slot),
        });
        Ok(())
    }

    pub(crate) fn index(&self, identity: SequenceIdentity) -> Result<usize, BackendError> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.identity == identity)
        {
            return Ok(index);
        }
        match self
            .entries
            .iter()
            .find(|entry| entry.identity.route == identity.route)
        {
            Some(entry) if entry.identity.generation != identity.generation => {
                Err(BackendError::GenerationStale)
            }
            Some(_) => Err(BackendError::RoleMismatch),
            None => Err(BackendError::RouteUnknown),
        }
    }

    pub(crate) fn get(
        &self,
        identity: SequenceIdentity,
    ) -> Result<&RegistryEntry<T>, BackendError> {
        let index = self.index(identity)?;
        Ok(&self.entries[index])
    }

    pub(crate) fn get_mut(
        &mut self,
        identity: SequenceIdentity,
    ) -> Result<&mut RegistryEntry<T>, BackendError> {
        let index = self.index(identity)?;
        Ok(&mut self.entries[index])
    }

    pub(crate) fn remove(
        &mut self,
        identity: SequenceIdentity,
    ) -> Result<RegistryEntry<T>, BackendError> {
        let index = self.index(identity)?;
        Ok(self.entries.swap_remove(index))
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    request: RequestId,
    digest: [u8; 32],
    input_bytes: usize,
    input_tokens: u64,
    submitted: u64,
    ready: u64,
    deadline: u64,
    turn: u64,
    agent: Option<AgentPending>,
    broker: Option<BrokerPending>,
}

#[derive(Clone, Copy, Debug)]
enum AgentPending {
    ToolCall { call_id: u64, target: AgentTarget },
    Completed,
}

#[derive(Clone, Copy, Debug)]
enum AgentTarget {
    Identity,
    PromptX,
    PromptY,
}

#[derive(Clone, Copy, Debug)]
enum BrokerPending {
    Create {
        plan: [u8; 16],
        object: [u8; 16],
        executable: [u8; 16],
        budget: TaskBudget,
    },
    GrantObject {
        plan: [u8; 16],
        agent: [u8; 16],
        object: [u8; 16],
    },
    GrantExecutable {
        plan: [u8; 16],
        agent: [u8; 16],
        executable: [u8; 16],
        launch_limit: u32,
    },
    GrantView {
        plan: [u8; 16],
        agent: [u8; 16],
        view: [u8; 16],
    },
    SetBudget {
        plan: [u8; 16],
        agent: [u8; 16],
        budget: TaskBudget,
    },
    InspectAgent {
        plan: [u8; 16],
        agent: [u8; 16],
    },
    CancelAgent {
        plan: [u8; 16],
        agent: [u8; 16],
    },
}

struct DeterministicSequence {
    turns: u64,
    context: [u8; 32],
    context_bytes: usize,
    context_tokens: u64,
    pending: VecDeque<Pending>,
    seen: Vec<RequestId>,
    metrics: SequenceMetrics,
    accepted_turn: u64,
    expected_call: Option<u64>,
    next_call: u64,
}

impl DeterministicSequence {
    fn new(limits: BackendLimits) -> Self {
        Self {
            turns: 0,
            context: [0; 32],
            context_bytes: 0,
            context_tokens: 0,
            pending: VecDeque::with_capacity(limits.pending_per_sequence),
            seen: Vec::with_capacity(limits.request_history),
            metrics: SequenceMetrics::default(),
            accepted_turn: 0,
            expected_call: None,
            next_call: 0,
        }
    }
}

pub struct DeterministicBackend {
    registry: SequenceRegistry<DeterministicSequence>,
    latency_ticks: u64,
    poll_cursor: usize,
    metrics: BackendMetrics,
}

impl DeterministicBackend {
    #[must_use]
    pub fn new(latency_ticks: u64) -> Self {
        Self::with_limits(latency_ticks, BackendLimits::default())
            .expect("default deterministic limits are valid")
    }

    pub fn with_limits(latency_ticks: u64, limits: BackendLimits) -> Result<Self, BackendError> {
        Ok(Self {
            registry: SequenceRegistry::new(limits)?,
            latency_ticks: latency_ticks.max(1),
            poll_cursor: 0,
            metrics: BackendMetrics {
                incarnation: 1,
                ..BackendMetrics::default()
            },
        })
    }

    fn response(
        identity: SequenceIdentity,
        slot: u16,
        sequence: &mut DeterministicSequence,
        input: [u8; 32],
        turn: u64,
        agent: Option<AgentPending>,
        broker: Option<BrokerPending>,
    ) -> Vec<u8> {
        sequence.turns = sequence.turns.saturating_add(1);
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"kore.hostlink.deterministic.v2\0");
        hasher.update(&identity.route.0);
        hasher.update(&identity.generation.get().to_le_bytes());
        hasher.update(&sequence.context);
        hasher.update(&input);
        hasher.update(&sequence.turns.to_le_bytes());
        sequence.context = *hasher.finalize().as_bytes();
        if let Some(broker) = broker {
            let mut wire = [0; kore_hostlink::INFERENCE_PART_MAX];
            let len = match broker {
                BrokerPending::Create {
                    plan,
                    object,
                    executable,
                    budget,
                } => {
                    let selectors = [executable, object];
                    BrokerPlanningReply {
                        plan,
                        action: BrokerActionKind::CreateAgent,
                        target: [0; 16],
                        subject: [0; 16],
                        budget,
                        rights: 0,
                        limit: 0,
                        reason: 0,
                        label: "agent/prompt",
                        selectors: &selectors,
                    }
                    .encode(&mut wire)
                }
                BrokerPending::GrantObject {
                    plan,
                    agent,
                    object,
                } => BrokerPlanningReply {
                    plan,
                    action: BrokerActionKind::GrantObject,
                    target: agent,
                    subject: object,
                    budget: TaskBudget::ZERO,
                    rights: 1,
                    limit: 0,
                    reason: 0,
                    label: "",
                    selectors: &[],
                }
                .encode(&mut wire),
                BrokerPending::GrantExecutable {
                    plan,
                    agent,
                    executable,
                    launch_limit,
                } => BrokerPlanningReply {
                    plan,
                    action: BrokerActionKind::GrantExecutable,
                    target: agent,
                    subject: executable,
                    budget: TaskBudget::ZERO,
                    rights: 0,
                    limit: launch_limit,
                    reason: 0,
                    label: "",
                    selectors: &[],
                }
                .encode(&mut wire),
                BrokerPending::GrantView { plan, agent, view } => BrokerPlanningReply {
                    plan,
                    action: BrokerActionKind::GrantView,
                    target: agent,
                    subject: view,
                    budget: TaskBudget::ZERO,
                    rights: 1,
                    limit: 0,
                    reason: 0,
                    label: "",
                    selectors: &[],
                }
                .encode(&mut wire),
                BrokerPending::SetBudget {
                    plan,
                    agent,
                    budget,
                } => BrokerPlanningReply {
                    plan,
                    action: BrokerActionKind::SetBudget,
                    target: agent,
                    subject: [0; 16],
                    budget,
                    rights: 0,
                    limit: 0,
                    reason: 0,
                    label: "",
                    selectors: &[],
                }
                .encode(&mut wire),
                BrokerPending::InspectAgent { plan, agent } => BrokerPlanningReply {
                    plan,
                    action: BrokerActionKind::InspectAgent,
                    target: agent,
                    subject: [0; 16],
                    budget: TaskBudget::ZERO,
                    rights: 1,
                    limit: 0,
                    reason: 0,
                    label: "",
                    selectors: &[],
                }
                .encode(&mut wire),
                BrokerPending::CancelAgent { plan, agent } => BrokerPlanningReply {
                    plan,
                    action: BrokerActionKind::CancelAgent,
                    target: agent,
                    subject: [0; 16],
                    budget: TaskBudget::ZERO,
                    rights: 0,
                    limit: 0,
                    reason: 1,
                    label: "",
                    selectors: &[],
                }
                .encode(&mut wire),
            }
            .expect("deterministic broker reply respects shared bounds");
            return wire[..len].to_vec();
        }
        if let Some(agent) = agent {
            let mut wire = [0; kore_hostlink::INFERENCE_PART_MAX];
            let event = match agent {
                AgentPending::ToolCall { call_id, target } => {
                    let path = match target {
                        AgentTarget::Identity => "/identity",
                        AgentTarget::PromptX => "/X",
                        AgentTarget::PromptY => "/Y",
                    };
                    let args = [ToolArgument::Path(path)];
                    let event = AgentEventView {
                        kind: AgentEventKind::ToolCall,
                        turn,
                        part: 0,
                        call_id,
                        command: "cat",
                        arguments: &args,
                        text: "",
                    };
                    let len = event
                        .encode(&mut wire)
                        .expect("deterministic agent tool call respects shared bounds");
                    return wire[..len].to_vec();
                }
                AgentPending::Completed => AgentEventView {
                    kind: AgentEventKind::Completed,
                    turn,
                    part: 0,
                    call_id: 0,
                    command: "",
                    arguments: &[],
                    text: "",
                },
            };
            let len = event
                .encode(&mut wire)
                .expect("deterministic agent event respects shared bounds");
            return wire[..len].to_vec();
        }
        let mut out = Vec::with_capacity(74);
        out.push(match identity.role {
            BackendRole::Broker => 0,
            BackendRole::Agent => 1,
            BackendRole::Workspace => 2,
        });
        out.push(u8::try_from(slot).unwrap_or(u8::MAX));
        out.extend_from_slice(&sequence.turns.to_le_bytes());
        out.extend_from_slice(&input);
        out.extend_from_slice(&sequence.context);
        out
    }

    fn refresh_metrics(&mut self) {
        self.metrics.sequences_open = self.registry.entries.len();
        self.metrics.pending = self
            .registry
            .entries
            .iter()
            .map(|entry| entry.state.pending.len())
            .sum();
        self.metrics.peak_pending = self.metrics.peak_pending.max(self.metrics.pending);
    }
}

impl InferenceBackend for DeterministicBackend {
    fn reset(&mut self) {
        self.registry.clear();
        self.poll_cursor = 0;
        self.metrics.incarnation = self.metrics.incarnation.saturating_add(1);
        self.refresh_metrics();
    }

    fn open(&mut self, identity: SequenceIdentity) -> Result<(), BackendError> {
        let limits = self.registry.limits;
        self.registry
            .open(identity, |_| DeterministicSequence::new(limits))?;
        self.refresh_metrics();
        Ok(())
    }

    fn close(&mut self, identity: SequenceIdentity) -> Result<(), BackendError> {
        self.registry.remove(identity)?;
        self.poll_cursor = self
            .poll_cursor
            .min(self.registry.entries.len().saturating_sub(1));
        self.refresh_metrics();
        Ok(())
    }

    #[allow(clippy::collapsible_match)]
    fn submit(
        &mut self,
        identity: SequenceIdentity,
        request: RequestId,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        if request.is_zero() {
            return Err(BackendError::InvalidIdentity);
        }
        let decoded = decode_request(identity.role, payload, false)?;
        let turn = decoded.turn;
        let input = decoded.input;
        let limits = self.registry.limits;
        let entry = self.registry.get_mut(identity)?;
        let sequence = &mut entry.state;
        if input.is_empty() || input.len() > kore_hostlink::INLINE_MAX {
            return Err(BackendError::InputTooLarge);
        }
        if sequence.seen.contains(&request) {
            return Err(BackendError::Duplicate);
        }
        if turn != 0 && turn != sequence.accepted_turn.saturating_add(1) {
            return Err(BackendError::OutOfOrder);
        }
        let mut next_expected_call = sequence.expected_call;
        let mut next_call = sequence.next_call;
        let agent = if identity.role == BackendRole::Agent
            && input
                .get(..4)
                .is_some_and(|magic| magic == kore_hostlink::AGENT_REQUEST_MAGIC.to_le_bytes())
        {
            let message = decode_agent_request(input).ok_or(BackendError::InvalidEncoding)?;
            if message.turn != turn || !sequence.pending.is_empty() {
                return Err(BackendError::OutOfOrder);
            }
            match message.kind {
                AgentRequestKind::UserText if next_expected_call.is_none() => {
                    next_call = next_call.checked_add(1).ok_or(BackendError::Full)?;
                    next_expected_call = Some(next_call);
                    let target = if message.text == "read X" {
                        AgentTarget::PromptX
                    } else if message.text == "read Y" {
                        AgentTarget::PromptY
                    } else {
                        AgentTarget::Identity
                    };
                    Some(AgentPending::ToolCall {
                        call_id: next_call,
                        target,
                    })
                }
                AgentRequestKind::ToolResult if next_expected_call == Some(message.call_id) => {
                    if message.result == Some(ToolResultKind::Output)
                        && message.text.contains("read Y")
                    {
                        next_call = next_call.checked_add(1).ok_or(BackendError::Full)?;
                        next_expected_call = Some(next_call);
                        Some(AgentPending::ToolCall {
                            call_id: next_call,
                            target: AgentTarget::PromptY,
                        })
                    } else {
                        next_expected_call = None;
                        Some(AgentPending::Completed)
                    }
                }
                AgentRequestKind::Cancel => return Err(BackendError::OutOfOrder),
                _ => return Err(BackendError::OutOfOrder),
            }
        } else {
            None
        };
        let broker = if identity.role == BackendRole::Broker
            && input
                .get(..4)
                .is_some_and(|magic| magic == kore_hostlink::BROKER_REQUEST_MAGIC.to_le_bytes())
        {
            let planning = decode_task_intent(input).ok_or(BackendError::InvalidEncoding)?;
            if planning.ceiling.tool_calls == 0 {
                return Err(BackendError::InvalidEncoding);
            }
            let mut object = None;
            let mut fallback_object = None;
            let mut executable = None;
            let mut view = None;
            let mut agent = None;
            for offer in planning.offers() {
                match (planning.intent, offer.kind, offer.label) {
                    ("read X", BrokerOfferKind::Object, "workspace/X")
                        if offer.permitted_ops & 1 != 0 =>
                    {
                        if object.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    ("read X", BrokerOfferKind::Executable, "cat")
                        if offer.permitted_ops != 0 && offer.max_uses != 0 =>
                    {
                        if executable.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    (_, BrokerOfferKind::Object, "workspace/identity")
                        if offer.permitted_ops & 1 != 0 =>
                    {
                        if fallback_object.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    (_, BrokerOfferKind::Executable, "cat")
                        if offer.permitted_ops != 0 && offer.max_uses != 0 =>
                    {
                        if executable.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    ("read Y", BrokerOfferKind::Object, "workspace/Y")
                        if offer.permitted_ops & 1 != 0 =>
                    {
                        if object.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    ("read Y", BrokerOfferKind::ExistingAgent, "agent/prompt") => {
                        if agent.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    ("execute cat", BrokerOfferKind::Executable, "cat")
                        if offer.permitted_ops != 0 && offer.max_uses != 0 =>
                    {
                        if executable.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    ("execute cat", BrokerOfferKind::ExistingAgent, "agent/prompt") => {
                        if agent.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    ("attach V", BrokerOfferKind::View, "workspace/V")
                        if offer.permitted_ops & 1 != 0 =>
                    {
                        if view.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    (
                        "attach V" | "set budget" | "inspect agent" | "cancel agent",
                        BrokerOfferKind::ExistingAgent,
                        "agent/prompt",
                    ) => {
                        if agent.replace(offer.reference).is_some() {
                            return Err(BackendError::InvalidEncoding);
                        }
                    }
                    _ => {}
                }
            }
            Some(match planning.intent {
                "read X" => BrokerPending::Create {
                    plan: planning.plan,
                    object: object.ok_or(BackendError::InvalidEncoding)?,
                    executable: executable.ok_or(BackendError::InvalidEncoding)?,
                    budget: planning.ceiling,
                },
                "read Y" => BrokerPending::GrantObject {
                    plan: planning.plan,
                    agent: agent.ok_or(BackendError::InvalidEncoding)?,
                    object: object.ok_or(BackendError::InvalidEncoding)?,
                },
                "execute cat" => BrokerPending::GrantExecutable {
                    plan: planning.plan,
                    agent: agent.ok_or(BackendError::InvalidEncoding)?,
                    executable: executable.ok_or(BackendError::InvalidEncoding)?,
                    launch_limit: 1,
                },
                "attach V" => BrokerPending::GrantView {
                    plan: planning.plan,
                    agent: agent.ok_or(BackendError::InvalidEncoding)?,
                    view: view.ok_or(BackendError::InvalidEncoding)?,
                },
                "set budget" => BrokerPending::SetBudget {
                    plan: planning.plan,
                    agent: agent.ok_or(BackendError::InvalidEncoding)?,
                    budget: planning.ceiling,
                },
                "inspect agent" => BrokerPending::InspectAgent {
                    plan: planning.plan,
                    agent: agent.ok_or(BackendError::InvalidEncoding)?,
                },
                "cancel agent" => BrokerPending::CancelAgent {
                    plan: planning.plan,
                    agent: agent.ok_or(BackendError::InvalidEncoding)?,
                },
                _ => BrokerPending::Create {
                    plan: planning.plan,
                    object: fallback_object.ok_or(BackendError::InvalidEncoding)?,
                    executable: executable.ok_or(BackendError::InvalidEncoding)?,
                    budget: planning.ceiling,
                },
            })
        } else {
            None
        };
        if sequence.seen.len() == limits.request_history
            || sequence.pending.len() == limits.pending_per_sequence
        {
            return Err(BackendError::Full);
        }
        let input_tokens = estimate_tokens(input.len());
        let pending_bytes = sequence.pending.iter().try_fold(0usize, |total, pending| {
            total
                .checked_add(pending.input_bytes)
                .and_then(|bytes| bytes.checked_add(74))
        });
        let pending_tokens = sequence.pending.iter().try_fold(0u64, |total, pending| {
            total
                .checked_add(pending.input_tokens)
                .and_then(|tokens| tokens.checked_add(estimate_tokens(74)))
        });
        let next_context = sequence
            .context_bytes
            .checked_add(pending_bytes.ok_or(BackendError::ContextLimit)?)
            .and_then(|bytes| bytes.checked_add(input.len()))
            .and_then(|bytes| bytes.checked_add(74))
            .ok_or(BackendError::ContextLimit)?;
        let next_tokens = sequence
            .context_tokens
            .checked_add(pending_tokens.ok_or(BackendError::TokenLimit)?)
            .and_then(|tokens| tokens.checked_add(input_tokens))
            .and_then(|tokens| tokens.checked_add(estimate_tokens(74)))
            .ok_or(BackendError::TokenLimit)?;
        if next_context > limits.context_bytes {
            return Err(BackendError::ContextLimit);
        }
        if next_tokens > limits.tokens_per_sequence {
            return Err(BackendError::TokenLimit);
        }
        sequence.seen.push(request);
        if turn != 0 {
            sequence.accepted_turn = turn;
        }
        sequence.expected_call = next_expected_call;
        sequence.next_call = next_call;
        sequence.pending.push_back(Pending {
            request,
            digest: *blake3::hash(input).as_bytes(),
            input_bytes: input.len(),
            input_tokens,
            submitted: tick,
            ready: tick.saturating_add(self.latency_ticks),
            deadline: tick.saturating_add(limits.request_timeout_ticks),
            turn,
            agent,
            broker,
        });
        sequence.metrics.submitted = sequence.metrics.submitted.saturating_add(1);
        sequence.metrics.input_bytes = sequence
            .metrics
            .input_bytes
            .saturating_add(input.len() as u64);
        sequence.metrics.peak_pending = sequence.metrics.peak_pending.max(sequence.pending.len());
        self.metrics.submitted = self.metrics.submitted.saturating_add(1);
        self.metrics.input_bytes = self.metrics.input_bytes.saturating_add(input.len() as u64);
        self.refresh_metrics();
        Ok(())
    }

    fn cancel(
        &mut self,
        identity: SequenceIdentity,
        request: RequestId,
        _tick: u64,
    ) -> Result<BackendEvent, BackendError> {
        let entry = self.registry.get_mut(identity)?;
        let Some(index) = entry
            .state
            .pending
            .iter()
            .position(|pending| pending.request == request)
        else {
            return Err(BackendError::RequestUnknown);
        };
        let pending = entry
            .state
            .pending
            .remove(index)
            .expect("located deterministic request");
        entry.state.metrics.cancelled = entry.state.metrics.cancelled.saturating_add(1);
        self.metrics.cancelled = self.metrics.cancelled.saturating_add(1);
        self.refresh_metrics();
        Ok(BackendEvent::Cancelled {
            sequence: identity,
            request,
            submitted_tick: pending.submitted,
            turn: pending.turn,
        })
    }

    fn poll(&mut self, tick: u64) -> Option<BackendEvent> {
        let count = self.registry.entries.len();
        for offset in 0..count {
            let index = (self.poll_cursor + offset) % count;
            let Some(pending) = self.registry.entries[index].state.pending.front() else {
                continue;
            };
            if pending.ready > tick && pending.deadline > tick {
                continue;
            }
            let pending = self.registry.entries[index]
                .state
                .pending
                .pop_front()
                .expect("front deterministic request exists");
            self.poll_cursor = (index + 1) % count.max(1);
            let identity = self.registry.entries[index].identity;
            if pending.deadline <= tick && pending.ready > tick {
                let metrics = &mut self.registry.entries[index].state.metrics;
                metrics.failed = metrics.failed.saturating_add(1);
                self.metrics.failed = self.metrics.failed.saturating_add(1);
                self.refresh_metrics();
                return Some(BackendEvent::Failed {
                    sequence: identity,
                    request: pending.request,
                    submitted_tick: pending.submitted,
                    failure: BackendFailure::TimedOut,
                    turn: pending.turn,
                });
            }
            let slot = self.registry.entries[index].slot;
            let sequence = &mut self.registry.entries[index].state;
            let payload = Self::response(
                identity,
                slot,
                sequence,
                pending.digest,
                pending.turn,
                pending.agent,
                pending.broker,
            );
            let output_tokens = estimate_tokens(payload.len());
            sequence.context_bytes = sequence
                .context_bytes
                .saturating_add(pending.input_bytes)
                .saturating_add(payload.len());
            sequence.context_tokens = sequence
                .context_tokens
                .saturating_add(pending.input_tokens)
                .saturating_add(output_tokens);
            sequence.metrics.completed = sequence.metrics.completed.saturating_add(1);
            sequence.metrics.output_bytes = sequence
                .metrics
                .output_bytes
                .saturating_add(payload.len() as u64);
            sequence.metrics.prompt_tokens = sequence
                .metrics
                .prompt_tokens
                .saturating_add(pending.input_tokens);
            sequence.metrics.completion_tokens = sequence
                .metrics
                .completion_tokens
                .saturating_add(output_tokens);
            sequence.metrics.first_token_ticks_total = sequence
                .metrics
                .first_token_ticks_total
                .saturating_add(tick.saturating_sub(pending.submitted));
            sequence.metrics.queue_ticks_total = sequence
                .metrics
                .queue_ticks_total
                .saturating_add(tick.saturating_sub(pending.submitted));
            sequence.metrics.generation_ticks_total = sequence
                .metrics
                .generation_ticks_total
                .saturating_add(tick.saturating_sub(pending.submitted));
            sequence.metrics.context_bytes = sequence.context_bytes;
            sequence.metrics.context_tokens = sequence.context_tokens;
            self.metrics.completed = self.metrics.completed.saturating_add(1);
            self.metrics.output_bytes = self
                .metrics
                .output_bytes
                .saturating_add(payload.len() as u64);
            self.metrics.prompt_tokens = self
                .metrics
                .prompt_tokens
                .saturating_add(pending.input_tokens);
            self.metrics.completion_tokens =
                self.metrics.completion_tokens.saturating_add(output_tokens);
            self.metrics.first_token_ticks_total = self
                .metrics
                .first_token_ticks_total
                .saturating_add(tick.saturating_sub(pending.submitted));
            self.metrics.queue_ticks_total = self
                .metrics
                .queue_ticks_total
                .saturating_add(tick.saturating_sub(pending.submitted));
            self.metrics.generation_ticks_total = self
                .metrics
                .generation_ticks_total
                .saturating_add(tick.saturating_sub(pending.submitted));
            self.refresh_metrics();
            let output_bytes = payload.len() as u64;
            return Some(BackendEvent::Completed {
                sequence: identity,
                request: pending.request,
                payload,
                part: 0,
                submitted_tick: pending.submitted,
                usage: RequestUsage {
                    input_bytes: pending.input_bytes as u64,
                    output_bytes,
                    prompt_tokens: pending.input_tokens,
                    completion_tokens: output_tokens,
                },
                turn: pending.turn,
            });
        }
        None
    }

    fn metrics(&self) -> BackendMetrics {
        self.metrics
    }

    fn sequence_metrics(&self, identity: SequenceIdentity) -> Option<SequenceMetrics> {
        self.registry
            .get(identity)
            .ok()
            .map(|entry| entry.state.metrics)
    }
}

#[must_use]
pub(crate) const fn estimate_tokens(bytes: usize) -> u64 {
    bytes.saturating_add(3).saturating_div(4) as u64
}

/// Validate the versioned inference request before it reaches a model. The
/// deterministic reference backend alone accepts legacy payloads so the
/// pre-existing boot witness remains usable while guests migrate to KIR1.
pub(crate) struct DecodedRequest<'a> {
    pub(crate) turn: u64,
    pub(crate) input: &'a [u8],
    pub(crate) max_output_tokens: Option<u32>,
    pub(crate) timeout_ticks: Option<u64>,
}

pub(crate) fn decode_request(
    role: BackendRole,
    payload: &[u8],
    require_versioned: bool,
) -> Result<DecodedRequest<'_>, BackendError> {
    if let Some(request) = decode_inference_request(payload) {
        let expected = match role {
            BackendRole::Broker => InferenceRequestKind::BrokerPlan,
            BackendRole::Agent => InferenceRequestKind::AgentTurn,
            BackendRole::Workspace => return Err(BackendError::InvalidRole),
        };
        if request.kind != expected {
            return Err(BackendError::RoleMismatch);
        }
        return Ok(DecodedRequest {
            turn: request.turn,
            input: request.input,
            max_output_tokens: Some(request.max_output_tokens),
            timeout_ticks: Some(u64::from(request.timeout_ticks)),
        });
    }
    let looks_versioned = payload
        .get(..4)
        .is_some_and(|magic| magic == INFERENCE_REQUEST_MAGIC.to_le_bytes());
    if require_versioned || looks_versioned {
        return Err(BackendError::InvalidEncoding);
    }
    Ok(DecodedRequest {
        turn: 0,
        input: payload,
        max_output_tokens: None,
        timeout_ticks: None,
    })
}
