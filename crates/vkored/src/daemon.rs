//! Nonblocking two-plane HostLink endpoint.

use std::collections::VecDeque;
#[cfg(unix)]
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kore_hostlink::{
    AdmitError, BulkChunkHeaderV1, BulkFinishV1, BulkFrameDecoder, BulkId, BulkRefV1,
    ControlFrameDecoder, EnvelopeV1, FrontendIntent, FrontendIntentId, FrontendPoll,
    FrontendPollPhase, FrontendResultKind, INFERENCE_EVENT_HEADER_LEN, INFERENCE_PART_MAX,
    INLINE_MAX, InferenceEventKind, InferenceEventView, InferenceRequestKind, LinkEpoch,
    RequestError, RequestId, RouteError, RouteId, RouteRole, RouteState, SequenceDisposition,
    TaskGeneration, decode_frontend_poll, decode_frontend_result, decode_inference_event,
    decode_inference_request, decode_refusal, derive_link_epoch,
};
use zerocopy::byteorder::little_endian::{U16, U32};
use zerocopy::{FromBytes, IntoBytes};

use crate::backend::{
    BackendError, BackendEvent, BackendFailure, BackendRole, InferenceBackend,
    InferenceObservation, ObservationKind, SEQUENCES_MAX, SequenceIdentity,
};
use crate::bulk::{
    BulkError, BulkManager, CompletedBulk, HOST_BULK_BYTE_CREDITS_PER_ROUTE,
    HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
};
pub use crate::frontend_updates::FrontendUpdateResultOwned;
use crate::frontend_updates::{FrontendUpdates, Receipt as UpdateReceipt};
use crate::protocol::*;

const ROUTES_MAX: usize = SEQUENCES_MAX + 1;
const QUEUE_FRAMES_MAX: usize = 64;
const QUEUE_BYTES_MAX: usize = 2 * 1024 * 1024;
const DATA_QUEUE_FRAMES_MAX: usize = 48;
const DATA_QUEUE_BYTES_MAX: usize = 3 * 1024 * 1024 / 2;
const TRANSCRIPT_MAX: usize = 4096;
const AUTHORITY_OBSERVATIONS_MAX: usize = 4096;
const FRONTEND_INTENTS_MAX: usize = 8;
const FRONTEND_POLLS_MAX: usize = 8;
const FRONTEND_RESULTS_MAX: usize = 8;
// Each terminal result can have its worker and its owning broker awaiting
// publication. They share one result, but retain independent route requests.
const FRONTEND_RECEIPTS_MAX: usize = FRONTEND_RESULTS_MAX * 2;
const READ_SIZE: usize = 16 * 1024;
const FRAMES_PER_TICK: usize = 64;
const COMPLETED_BULK_MAX: usize = 12;
const OUTBOUND_BULK_MAX: usize = 12;
const ACCEPTS_PER_TICK: usize = 8;
/// Fail closed if a peer stops draining after a sequence-poisoning refusal.
/// Normally the small control frame enters the socket in the same tick.
const REFUSAL_FLUSH_TICKS_MAX: u64 = 64;
// QEMU establishes the Unix chardev connection before a guest has necessarily
// posted its data queues or announced PORT_READY.  A Hello written in that
// interval is lawfully lost by the console backend, so negotiation must not
// rely on the socket-connect edge being the guest-ready edge.
const HELLO_RETRY_TICKS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketPlane {
    Control,
    Bulk,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub control_socket: PathBuf,
    pub bulk_socket: PathBuf,
}

/// Portable serial-plane listener for QEMU hosts without Unix-domain
/// sockets. Only loopback addresses are accepted: HostLink is a VM-local
/// authority channel, never a network service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TcpConfig {
    pub control: SocketAddr,
    pub bulk: SocketAddr,
}

#[derive(Debug)]
pub enum DaemonError {
    Io(io::Error),
    InvalidSocketPath(PathBuf),
    SocketInUse(PathBuf),
    UnsafeTcpAddress(SocketAddr),
}
impl From<io::Error> for DaemonError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Metrics {
    pub epochs: u64,
    pub disconnects: u64,
    pub control_frames_in: u64,
    pub bulk_frames_in: u64,
    pub frames_out: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub malformed: u64,
    pub replay_or_order: u64,
    pub stale_route: u64,
    pub backpressure: u64,
    pub submitted: u64,
    pub completed: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub streamed_parts: u64,
    pub stale_backend_events: u64,
    pub max_queued_bytes: usize,
    /// Successful nonzero socket reads; a syscall/wakeup proxy, not a copy
    /// count for QEMU's internal chardev path.
    pub io_read_calls: u64,
    /// Successful nonzero socket writes; a syscall/wakeup proxy, not a copy
    /// count for QEMU's internal chardev path.
    pub io_write_calls: u64,
    pub partial_write_calls: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BenchmarkEvent {
    Submitted {
        route: RouteId,
        request: RequestId,
        tick: u64,
    },
    Completed {
        route: RouteId,
        request: RequestId,
        ticks: u64,
    },
    FirstToken {
        route: RouteId,
        request: RequestId,
        ticks: u64,
    },
    Cancelled {
        route: RouteId,
        request: RequestId,
        ticks: u64,
    },
    Failed {
        route: RouteId,
        request: RequestId,
        ticks: u64,
        failure: BackendFailure,
    },
    Disconnected {
        tick: u64,
    },
}

pub trait BenchmarkSink {
    fn observe(&mut self, event: BenchmarkEvent);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityObservation {
    pub state: kore_hostlink::AuthorityState,
    pub kind: kore_hostlink::AuthorityKind,
    pub agent_slot: u16,
    pub generation: u64,
    pub rights: u32,
    pub label: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrontendQueueError {
    InvalidIntent,
    Duplicate,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrontendCancelOutcome {
    QueuedRemoved,
    Requested,
    AlreadySettled,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontendResultOwned {
    pub intent_id: FrontendIntentId,
    pub kind: FrontendResultKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrontendIntentState {
    Queued,
    BrokerClaimed,
    AgentClaimed(SequenceIdentity),
    /// Delivery may have produced guest effects. A new link cannot replay it.
    Disconnected,
}

struct FrontendIntentOwned {
    intent_id: FrontendIntentId,
    text: String,
    state: FrontendIntentState,
    broker: Option<SequenceIdentity>,
    cancel_requested: bool,
    inference_failed: bool,
}

#[derive(Clone, Copy)]
struct PendingFrontendPoll {
    identity: SequenceIdentity,
    request: RequestId,
    turn: u64,
    poll: FrontendPoll,
    submitted_tick: u64,
}

struct PendingFrontendReceipt {
    intent_id: FrontendIntentId,
    identity: SequenceIdentity,
    request: RequestId,
    turn: u64,
    submitted_tick: u64,
}

struct FrontendCompletion {
    intent_id: FrontendIntentId,
    source: SequenceIdentity,
    broker: Option<SequenceIdentity>,
    worker: Option<SequenceIdentity>,
    published: bool,
    broker_joined: bool,
    retired: bool,
    ready: bool,
    cancel_requested: bool,
    connection_lost: bool,
}

impl FrontendCompletion {
    fn accepts_fallback(&self, identity: SequenceIdentity, result: FrontendResultKind) -> bool {
        !self.connection_lost
            && self.source.role == BackendRole::Agent
            && identity.role == BackendRole::Broker
            && self.broker == Some(identity)
            && (result == FrontendResultKind::Failed
                || (self.cancel_requested && result == FrontendResultKind::Cancelled))
            && !self.broker_joined
    }
}
impl BenchmarkSink for () {
    fn observe(&mut self, _: BenchmarkEvent) {}
}

struct Route {
    state: RouteState,
    identity: SequenceIdentity,
    stream_parts: Vec<(RequestId, u16)>,
    // A cancellation can arrive after the terminal model reply was queued but
    // before the guest consumed it. Retain bounded exact-generation evidence
    // so that race neither disconnects the link nor emits a second terminal.
    completed_requests: VecDeque<RequestId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutboundBulk {
    route: RouteId,
    request: RequestId,
    bulk_id: BulkId,
    /// BEGIN plus every CHUNK must be acknowledged before READY is valid.
    accepted_expected: u16,
    accepted: u16,
}

#[derive(Default)]
struct BoundedQueue {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
    offset: usize,
}
impl BoundedQueue {
    fn push(&mut self, frame: Vec<u8>) -> Result<(), Vec<u8>> {
        self.push_with_priority(frame, true)
    }

    fn push_with_priority(&mut self, frame: Vec<u8>, priority: bool) -> Result<(), Vec<u8>> {
        let frame_limit = if priority {
            QUEUE_FRAMES_MAX
        } else {
            DATA_QUEUE_FRAMES_MAX
        };
        let byte_limit = if priority {
            QUEUE_BYTES_MAX
        } else {
            DATA_QUEUE_BYTES_MAX
        };
        if self.frames.len() == frame_limit || self.bytes.saturating_add(frame.len()) > byte_limit {
            return Err(frame);
        }
        self.bytes += frame.len();
        self.frames.push_back(frame);
        Ok(())
    }
    fn front_remaining(&self) -> Option<&[u8]> {
        self.frames.front().map(|f| &f[self.offset..])
    }
    fn consume(&mut self, n: usize) {
        self.offset += n;
        self.bytes -= n;
        if self.frames.front().is_some_and(|f| self.offset == f.len()) {
            self.frames.pop_front();
            self.offset = 0;
        }
    }

    fn can_accept(&self, frames: usize, bytes: usize) -> bool {
        self.frames.len().saturating_add(frames) <= QUEUE_FRAMES_MAX
            && self
                .bytes
                .checked_add(bytes)
                .is_some_and(|total| total <= QUEUE_BYTES_MAX)
    }
}

impl<B: InferenceBackend, S: BenchmarkSink> LinkCore<B, S> {
    fn receive_bulk(&mut self, envelope_in: EnvelopeV1, payload: &[u8]) {
        let mut backend_bulk = None;
        let Some(index) = self
            .routes
            .iter()
            .position(|route| route.state.route_id() == envelope_in.route_id)
        else {
            self.refuse(
                &envelope_in,
                RefusalCode::RouteUnknown,
                RetryClass::NewRequest,
                b"",
            );
            return;
        };
        if let Err(error) = self.routes[index].state.check_received(&envelope_in) {
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope_in, code, retry, b"");
            return;
        }

        let result = match envelope_in.kind.get() {
            BULK_BEGIN => {
                let reference = match BulkRefV1::read_from_bytes(payload) {
                    Ok(reference) => reference,
                    Err(_) => {
                        self.refuse(
                            &envelope_in,
                            RefusalCode::InvalidLength,
                            RetryClass::Never,
                            b"",
                        );
                        return;
                    }
                };
                let opened_request = match self.routes[index]
                    .state
                    .open_request(envelope_in.request_id)
                {
                    Ok(()) => true,
                    Err(RequestError::Duplicate) => false,
                    Err(_) => {
                        self.refuse(
                            &envelope_in,
                            RefusalCode::Backpressure,
                            RetryClass::SameEpoch,
                            b"",
                        );
                        return;
                    }
                };
                match self.bulk.begin(
                    envelope_in.route_id,
                    envelope_in.request_id,
                    reference,
                    opened_request,
                ) {
                    Ok(()) => Ok((BULK_ACCEPTED, reference.bulk_id.0.to_vec())),
                    Err(error) => {
                        if opened_request {
                            let _ = self.routes[index]
                                .state
                                .close_request(envelope_in.request_id);
                        }
                        Err(error)
                    }
                }
            }
            BULK_CHUNK => {
                let data_len = payload.len().saturating_sub(BULK_CHUNK_HEADER_LEN);
                if self.routes[index]
                    .state
                    .consume_bulk(data_len as u64, 1)
                    .is_err()
                {
                    self.refuse(
                        &envelope_in,
                        RefusalCode::Backpressure,
                        RetryClass::SameEpoch,
                        b"",
                    );
                    return;
                }
                match self
                    .bulk
                    .chunk(envelope_in.route_id, envelope_in.request_id, payload)
                {
                    Ok(_) => Ok((BULK_ACCEPTED, payload[..16].to_vec())),
                    Err(error) => {
                        let _ = self.routes[index].state.replenish_bulk(data_len as u64, 1);
                        Err(error)
                    }
                }
            }
            BULK_FINISH => {
                if self.completed_bulk.len() == COMPLETED_BULK_MAX {
                    Err(BulkError::Backpressure)
                } else {
                    match self
                        .bulk
                        .finish(envelope_in.route_id, envelope_in.request_id, payload)
                    {
                        Ok((completed, bytes, chunks, _opened_request)) => {
                            let id = completed.bulk_id.0.to_vec();
                            let _ = self.routes[index].state.replenish_bulk(bytes, chunks);
                            // FINISH is transport-terminal, but a completed
                            // upload is still awaiting its role-specific
                            // backend's typed application result. Keep the
                            // request live until `finish` emits that result;
                            // otherwise the generation-aware backend event is
                            // discarded as late and the guest sees READY with
                            // no WORKSPACE_REPLY.
                            backend_bulk = Some(completed.clone());
                            self.completed_bulk.push_back(completed);
                            Ok((BULK_READY, id))
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            BULK_ABORT => {
                match self
                    .bulk
                    .abort(envelope_in.route_id, envelope_in.request_id, payload)
                {
                    Ok((id, bytes, chunks, opened_request)) => {
                        let _ = self.routes[index].state.replenish_bulk(bytes, chunks);
                        if opened_request {
                            let _ = self.routes[index]
                                .state
                                .close_request(envelope_in.request_id);
                        }
                        Ok((BULK_ABORTED, id.0.to_vec()))
                    }
                    Err(error) => Err(error),
                }
            }
            _ => unreachable!(),
        };

        let (reply_kind, reply_payload) = match result {
            Ok(result) => result,
            Err(error) => {
                let (code, retry) = bulk_refusal(error);
                self.refuse(&envelope_in, code, retry, b"");
                return;
            }
        };
        if let Err(error) = self.routes[index].state.commit_received(&envelope_in) {
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope_in, code, retry, b"");
            return;
        }
        // Bulk acknowledgements are control records. Keeping them on the
        // ordered control stream prevents a later backend reply from
        // overtaking the terminal READY across the independent sockets.
        self.reply_routed(
            SocketPlane::Control,
            reply_kind,
            envelope_in.route_id,
            envelope_in.request_id,
            &reply_payload,
        );
        if let Some(completed) = backend_bulk {
            let submitted = self.backend.submit_bulk(
                self.routes[index].identity,
                completed.request,
                completed.bulk_id,
                completed.content_kind,
                &completed.bytes,
                self.tick,
            );
            if let Err(error) = submitted {
                let _ = self.routes[index]
                    .state
                    .close_request(envelope_in.request_id);
                let (code, retry) = backend_refusal(error);
                self.refuse(
                    &envelope_in,
                    code,
                    retry,
                    format!("typed bulk backend {error:?}").as_bytes(),
                );
            } else {
                if matches!(
                    self.routes[index].state.role(),
                    RouteRole::Broker | RouteRole::Agent
                ) {
                    self.routes[index]
                        .stream_parts
                        .push((envelope_in.request_id, 0));
                }
                self.metrics.submitted = self.metrics.submitted.saturating_add(1);
                self.sink.observe(BenchmarkEvent::Submitted {
                    route: envelope_in.route_id,
                    request: envelope_in.request_id,
                    tick: self.tick,
                });
            }
        }
    }

    fn refuse(&mut self, source: &EnvelopeV1, code: RefusalCode, retry: RetryClass, detail: &[u8]) {
        self.log(format!(
            "refused kind={} route={} generation={} request={} code={code:?} retry={retry:?} detail={}",
            source.kind.get(),
            hex_bytes(&source.route_id.0),
            source.task_generation.get(),
            hex_bytes(&source.request_id.0),
            String::from_utf8_lossy(detail),
        ));
        let refusal = RefusalView {
            for_family: FAMILY_LINK,
            for_kind: source.kind.get(),
            code,
            retry,
            retry_after_ms: 0,
            safe_detail: &detail[..detail.len().min(SAFE_DETAIL_MAX)],
        };
        let mut payload = [0; REFUSAL_HEADER_LEN + SAFE_DETAIL_MAX];
        let payload_len = refusal
            .encode(&mut payload)
            .expect("fixed refusal output is large enough");
        let payload = &payload[..payload_len];
        if self.epoch == Some(source.link_epoch)
            && self
                .routes
                .iter()
                .any(|route| route.state.route_id() == source.route_id)
        {
            let queued = self.reply_routed(
                SocketPlane::Control,
                REFUSED,
                source.route_id,
                source.request_id,
                payload,
            );
            // This refusal deliberately leaves the peer's input sequence
            // unconsumed. The peer has already advanced after transport
            // acceptance, so no subsequent routed frame is safe in this
            // epoch unless it retained the exact frame and sequence.
            self.require_reconnect(queued);
            return;
        }
        let reply = envelope(
            REFUSED,
            self.epoch.unwrap_or(LinkEpoch::ZERO),
            RouteId::ZERO,
            0,
            0,
            source.request_id,
            payload.len(),
        );
        self.enqueue(SocketPlane::Control, frame(&reply, payload));
    }

    fn require_reconnect(&mut self, flush_control: bool) {
        self.reconnect_after_control_flush |= flush_control;
        let deadline = self.tick.saturating_add(REFUSAL_FLUSH_TICKS_MAX);
        self.reconnect_deadline_tick = if self.reconnect_deadline_tick == 0 {
            deadline
        } else {
            self.reconnect_deadline_tick.min(deadline)
        };
    }

    pub(crate) fn reconnect_required(&self) -> bool {
        self.reconnect_deadline_tick != 0
    }

    pub(crate) fn reconnect_requires_control_flush(&self) -> bool {
        self.reconnect_after_control_flush
    }

    pub(crate) fn reconnect_is_ready_with_pending_control(&self, pending_control: bool) -> bool {
        self.reconnect_deadline_tick != 0
            && (!self.reconnect_after_control_flush
                || (self.control_out.frames.is_empty() && !pending_control)
                || self.tick >= self.reconnect_deadline_tick)
    }

    fn retire_bulk(&mut self, route_id: RouteId, request: RequestId) -> bool {
        let retired = self.bulk.cancel_request(route_id, request);
        if retired.is_empty() {
            return false;
        }
        if let Some(route) = self.route_mut(route_id) {
            for (_, bytes, chunks, _) in &retired {
                let _ = route.state.replenish_bulk(*bytes, *chunks);
            }
        }
        true
    }
}

impl BoundedQueue {
    fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
        self.offset = 0;
    }
}

pub struct LinkCore<B, S = ()> {
    backend: B,
    sink: S,
    epoch: Option<LinkEpoch>,
    negotiation_nonce: Option<[u8; 32]>,
    next_hello_retry_tick: u64,
    epoch_counter: u64,
    routes: Vec<Route>,
    bulk: BulkManager,
    completed_bulk: VecDeque<CompletedBulk>,
    outbound_bulk: Vec<OutboundBulk>,
    control_out: BoundedQueue,
    bulk_out: BoundedQueue,
    transcript: VecDeque<String>,
    authority_observations: VecDeque<AuthorityObservation>,
    frontend_intents: VecDeque<FrontendIntentOwned>,
    frontend_updates: FrontendUpdates,
    frontend_polls: VecDeque<PendingFrontendPoll>,
    frontend_results: VecDeque<FrontendResultOwned>,
    frontend_receipts: VecDeque<PendingFrontendReceipt>,
    frontend_completions: VecDeque<FrontendCompletion>,
    next_frontend_intent: u128,
    metrics: Metrics,
    tick: u64,
    reconnect_after_control_flush: bool,
    reconnect_deadline_tick: u64,
}

impl<B: InferenceBackend, S: BenchmarkSink> LinkCore<B, S> {
    #[must_use]
    pub fn new(backend: B, sink: S) -> Self {
        Self {
            backend,
            sink,
            epoch: None,
            negotiation_nonce: None,
            next_hello_retry_tick: 0,
            epoch_counter: 0,
            routes: Vec::with_capacity(ROUTES_MAX),
            bulk: BulkManager::default(),
            completed_bulk: VecDeque::with_capacity(COMPLETED_BULK_MAX),
            outbound_bulk: Vec::with_capacity(OUTBOUND_BULK_MAX),
            control_out: BoundedQueue::default(),
            bulk_out: BoundedQueue::default(),
            transcript: VecDeque::with_capacity(TRANSCRIPT_MAX),
            authority_observations: VecDeque::with_capacity(AUTHORITY_OBSERVATIONS_MAX),
            frontend_intents: VecDeque::with_capacity(FRONTEND_INTENTS_MAX),
            frontend_updates: FrontendUpdates::default(),
            frontend_polls: VecDeque::with_capacity(FRONTEND_POLLS_MAX),
            frontend_results: VecDeque::with_capacity(FRONTEND_RESULTS_MAX),
            frontend_receipts: VecDeque::with_capacity(FRONTEND_RECEIPTS_MAX),
            frontend_completions: VecDeque::with_capacity(FRONTEND_RESULTS_MAX),
            next_frontend_intent: 0,
            metrics: Metrics::default(),
            tick: 0,
            reconnect_after_control_flush: false,
            reconnect_deadline_tick: 0,
        }
    }
    #[must_use]
    pub const fn metrics(&self) -> Metrics {
        self.metrics
    }
    #[must_use]
    pub fn backend_metrics(&self) -> crate::backend::BackendMetrics {
        self.backend.metrics()
    }
    #[must_use]
    pub fn backend_sequence_metrics(
        &self,
        sequence: SequenceIdentity,
    ) -> Option<crate::backend::SequenceMetrics> {
        self.backend.sequence_metrics(sequence)
    }
    #[must_use]
    pub fn epoch(&self) -> Option<LinkEpoch> {
        self.epoch
    }
    pub fn transcript(&self) -> impl Iterator<Item = &str> {
        self.transcript.iter().map(String::as_str)
    }
    pub fn pop_authority_observation(&mut self) -> Option<AuthorityObservation> {
        self.authority_observations.pop_front()
    }
    pub fn queue_frontend_intent(
        &mut self,
        text: impl Into<String>,
    ) -> Result<FrontendIntentId, FrontendQueueError> {
        let text = text.into();
        if text.is_empty()
            || text.len() > kore_hostlink::FRONTEND_TEXT_MAX
            || text.as_bytes().contains(&0)
        {
            return Err(FrontendQueueError::InvalidIntent);
        }
        if self.frontend_intents.len() == FRONTEND_INTENTS_MAX {
            return Err(FrontendQueueError::Full);
        }
        self.next_frontend_intent = self
            .next_frontend_intent
            .checked_add(1)
            .ok_or(FrontendQueueError::Full)?;
        let intent_id = FrontendIntentId::from_sequence(self.next_frontend_intent)
            .ok_or(FrontendQueueError::InvalidIntent)?;
        self.frontend_intents.push_back(FrontendIntentOwned {
            intent_id,
            text,
            state: FrontendIntentState::Queued,
            broker: None,
            cancel_requested: false,
            inference_failed: false,
        });
        self.service_frontend_polls();
        Ok(intent_id)
    }
    pub fn pop_frontend_result(&mut self) -> Option<FrontendResultOwned> {
        self.frontend_results.pop_front()
    }

    pub fn queue_frontend_update(
        &mut self,
        intent_id: FrontendIntentId,
        text: impl Into<String>,
    ) -> Result<u32, FrontendQueueError> {
        let broker = self
            .frontend_intents
            .iter()
            .find(|intent| {
                intent.intent_id == intent_id
                    && matches!(intent.state, FrontendIntentState::AgentClaimed(_))
                    && !intent.cancel_requested
                    && !intent.inference_failed
            })
            .and_then(|intent| intent.broker)
            .ok_or(FrontendQueueError::InvalidIntent)?;
        let revision = self
            .frontend_updates
            .queue(intent_id, broker, text.into())?;
        self.service_frontend_polls();
        Ok(revision)
    }

    pub fn pop_frontend_update_result(&mut self) -> Option<FrontendUpdateResultOwned> {
        self.frontend_updates.pop()
    }

    pub fn pop_frontend_update_result_for(
        &mut self,
        intent: FrontendIntentId,
        revision: u32,
    ) -> Option<FrontendUpdateResultOwned> {
        self.frontend_updates.pop_for(intent, revision)
    }
    /// Failed/cancelled work and accepted terminal outcomes need guest CPU to
    /// settle their native ownership, even if another model sequence is busy.
    pub fn frontend_requires_progress(&self) -> bool {
        self.frontend_updates.requires_progress()
            || self
                .frontend_intents
                .iter()
                .any(|intent| intent.cancel_requested || intent.inference_failed)
            || self
                .frontend_completions
                .iter()
                .any(|completion| !completion.ready)
    }
    /// Cancel only this host intent. Claimed work is settled by its owning
    /// guest broker; this operation neither kills the VM nor guesses which
    /// inference requests on a shared broker sequence belong to the intent.
    pub fn request_frontend_cancellation(&mut self, id: FrontendIntentId) -> FrontendCancelOutcome {
        if self
            .frontend_completions
            .iter()
            .any(|completion| completion.intent_id == id)
        {
            return FrontendCancelOutcome::AlreadySettled;
        }
        let Some(index) = self
            .frontend_intents
            .iter()
            .position(|intent| intent.intent_id == id)
        else {
            return FrontendCancelOutcome::Unknown;
        };
        if self.frontend_intents[index].state == FrontendIntentState::Queued {
            self.frontend_intents.remove(index);
            return FrontendCancelOutcome::QueuedRemoved;
        }
        self.frontend_intents[index].cancel_requested = true;
        self.frontend_updates.cancel_undelivered(id);
        FrontendCancelOutcome::Requested
    }

    fn frontend_control(
        &self,
        identity: SequenceIdentity,
        id: FrontendIntentId,
    ) -> Option<kore_hostlink::FrontendControl> {
        use kore_hostlink::FrontendControl;
        if let Some(intent) = self
            .frontend_intents
            .iter()
            .find(|intent| intent.intent_id == id && intent.broker == Some(identity))
        {
            return Some(if intent.cancel_requested {
                FrontendControl::CancelRequested
            } else {
                FrontendControl::Active
            });
        }
        self.frontend_completions
            .iter()
            .any(|completion| completion.intent_id == id && completion.broker == Some(identity))
            .then_some(FrontendControl::Settled)
    }
    pub fn frontend_ready(&self, intent_id: FrontendIntentId) -> bool {
        self.frontend_completions.iter().any(|completion| {
            completion.intent_id == intent_id
                && !completion.connection_lost
                && completion.published
                && completion.retired
                && completion.ready
        })
    }

    /// This prompt crossed into the guest, but its connection ended before
    /// execution/cleanup was settled. The VM owner must contain that guest;
    /// reconnecting transport is not proof that its effects can be replayed.
    pub fn frontend_interrupted(&self, intent_id: FrontendIntentId) -> bool {
        self.frontend_intents.iter().any(|intent| {
            intent.intent_id == intent_id && intent.state == FrontendIntentState::Disconnected
        }) || self
            .frontend_completions
            .iter()
            .any(|completion| completion.intent_id == intent_id && completion.connection_lost)
    }
    /// The runtime releases its bounded completion record only after observing
    /// this intent's native retirement and subsequent broker readiness.
    pub fn finish_frontend_intent(&mut self, intent_id: FrontendIntentId) -> bool {
        if !self.frontend_ready(intent_id) {
            return false;
        }
        self.frontend_completions
            .retain(|completion| completion.intent_id != intent_id);
        true
    }
    /// Release the guest's result acknowledgement only after the harness has
    /// published the final response. Until then its worker remains alive.
    pub fn acknowledge_frontend_result(&mut self, intent_id: FrontendIntentId) -> bool {
        let Some(completion) = self
            .frontend_completions
            .iter_mut()
            .find(|completion| completion.intent_id == intent_id)
        else {
            return false;
        };
        if completion.connection_lost {
            return false;
        }
        completion.published = true;
        let mut acknowledged = false;
        let mut index = 0;
        while index < self.frontend_receipts.len() {
            let receipt = &self.frontend_receipts[index];
            if receipt.intent_id == intent_id
                && self.finish_frontend_request(
                    receipt.identity,
                    receipt.request,
                    receipt.turn,
                    receipt.submitted_tick,
                    &[],
                )
            {
                self.frontend_receipts.remove(index);
                acknowledged = true;
            } else {
                index += 1;
            }
        }
        if acknowledged {
            self.log("frontend response published; teardown acknowledged");
        }
        acknowledged
    }
    pub fn cancel_frontend_intent(&mut self, intent_id: FrontendIntentId) -> bool {
        let before = self.frontend_intents.len();
        self.frontend_intents
            .retain(|intent| intent.intent_id != intent_id);
        self.frontend_polls
            .retain(|pending| pending.poll.intent_id != intent_id);
        self.frontend_results
            .retain(|result| result.intent_id != intent_id);
        self.frontend_receipts
            .retain(|receipt| receipt.intent_id != intent_id);
        self.frontend_completions
            .retain(|completion| completion.intent_id != intent_id);
        self.frontend_intents.len() != before
    }
    pub fn pop_control_frame(&mut self) -> Option<Vec<u8>> {
        let frame = self.control_out.frames.pop_front()?;
        self.control_out.bytes -= frame.len();
        Some(frame)
    }
    pub fn pop_bulk_frame(&mut self) -> Option<Vec<u8>> {
        let frame = self.bulk_out.frames.pop_front()?;
        self.bulk_out.bytes -= frame.len();
        Some(frame)
    }
    pub fn pop_completed_bulk(&mut self) -> Option<CompletedBulk> {
        self.completed_bulk.pop_front()
    }

    /// Start the host-to-guest negotiation required by #537. Calling this on
    /// an active link retires the prior epoch before emitting a fresh Hello.
    pub fn begin_negotiation(&mut self) {
        self.reset_session(true);
        self.epoch_counter = self.epoch_counter.wrapping_add(1).max(1);
        let nonce = fresh_nonce(self.epoch_counter, self.tick);
        self.negotiation_nonce = Some(nonce);
        self.enqueue_pending_hello(nonce);
        self.next_hello_retry_tick = self.tick.saturating_add(HELLO_RETRY_TICKS);
        self.log("hello sent");
    }

    fn enqueue_pending_hello(&mut self, nonce: [u8; 32]) {
        let hello = envelope(
            HELLO,
            LinkEpoch::ZERO,
            RouteId::ZERO,
            0,
            0,
            RequestId::ZERO,
            HELLO_LEN,
        );
        self.enqueue(
            SocketPlane::Control,
            frame(&hello, HelloV1 { host_nonce: nonce }.as_bytes()),
        );
    }

    fn log(&mut self, message: impl Into<String>) {
        if self.transcript.len() == TRANSCRIPT_MAX {
            self.transcript.pop_front();
        }
        self.transcript
            .push_back(format!("{} {}", self.tick, message.into()));
    }

    fn record_inference_observation(&mut self, observation: &InferenceObservation) {
        let role = match observation.role {
            BackendRole::Broker => "broker",
            BackendRole::Agent => "agent",
            BackendRole::Workspace => "workspace",
        };
        let kind = match observation.kind {
            ObservationKind::Input => "input",
            ObservationKind::ModelPrompt => "model_prompt",
            ObservationKind::ModelOutputChunk => "model_output_chunk",
            ObservationKind::RawModelCompletion => "raw_model_completion",
            ObservationKind::ReturnedModelFields => "returned_model_fields",
            ObservationKind::Validation => "validation",
            ObservationKind::AssistantMessage => "assistant_message",
            ObservationKind::ToolCall => "tool_call",
            ObservationKind::ToolResult => "tool_result",
            ObservationKind::BrokerProposal => "broker_proposal",
        };
        self.log(format!(
            "inference-observation {}",
            serde_json::json!({
                "role": role,
                "route": hex_bytes(&observation.route.0),
                "generation": observation.generation.get(),
                "request": hex_bytes(&observation.request.0),
                "turn": observation.turn,
                "kind": kind,
                "payload": observation.payload,
                "accepted": observation.accepted,
                "detail": observation.detail,
            })
        ));
    }

    fn record_authority_observation(&mut self, observation: &AuthorityObservation) {
        let state = match observation.state {
            kore_hostlink::AuthorityState::Installed => "installed",
            kore_hostlink::AuthorityState::Revoked => "revoked",
            kore_hostlink::AuthorityState::Teardown => "teardown",
        };
        let kind = match observation.kind {
            kore_hostlink::AuthorityKind::Object => "object",
            kore_hostlink::AuthorityKind::View => "view",
            kore_hostlink::AuthorityKind::Executable => "executable",
            kore_hostlink::AuthorityKind::Budget => "budget",
            kore_hostlink::AuthorityKind::Agent => "agent",
        };
        self.log(format!(
            "authority-observation {}",
            serde_json::json!({
                "state": state,
                "kind": kind,
                "agent_slot": observation.agent_slot,
                "generation": observation.generation,
                "rights": observation.rights,
                "label": observation.label,
            })
        ));
    }
    fn enqueue(&mut self, plane: SocketPlane, frame: Vec<u8>) -> bool {
        self.enqueue_with_priority(plane, frame, true)
    }
    fn enqueue_with_priority(
        &mut self,
        plane: SocketPlane,
        frame: Vec<u8>,
        priority: bool,
    ) -> bool {
        let queue = match plane {
            SocketPlane::Control => &mut self.control_out,
            SocketPlane::Bulk => &mut self.bulk_out,
        };
        let result = if priority {
            queue.push(frame)
        } else {
            queue.push_with_priority(frame, false)
        };
        if result.is_err() {
            self.metrics.backpressure += 1;
            return false;
        }
        self.metrics.max_queued_bytes = self.metrics.max_queued_bytes.max(queue.bytes);
        true
    }

    fn receive_frontend(
        &mut self,
        route_index: usize,
        envelope: EnvelopeV1,
        payload: &[u8],
    ) -> bool {
        let Some(request) = decode_inference_request(payload) else {
            return false;
        };
        if !matches!(
            request.kind,
            InferenceRequestKind::FrontendPoll
                | InferenceRequestKind::FrontendResult
                | InferenceRequestKind::FrontendUpdateResult
        ) {
            return false;
        }
        let identity = self.routes[route_index].identity;
        let accepted = match request.kind {
            InferenceRequestKind::FrontendUpdateResult => {
                let Some(result) = kore_hostlink::decode_frontend_update_result(request.input)
                else {
                    self.refuse(
                        &envelope,
                        RefusalCode::InvalidEncoding,
                        RetryClass::NewRequest,
                        b"frontend update result",
                    );
                    return true;
                };
                if identity.role != BackendRole::Broker
                    || !self.frontend_updates.accepts(identity, result)
                {
                    self.refuse(
                        &envelope,
                        RefusalCode::SelectorUnknown,
                        RetryClass::Never,
                        b"frontend update owner",
                    );
                    return true;
                }
                None
            }
            InferenceRequestKind::FrontendPoll => {
                let Some(poll) = decode_frontend_poll(request.input) else {
                    self.refuse(
                        &envelope,
                        RefusalCode::InvalidEncoding,
                        RetryClass::NewRequest,
                        b"frontend poll",
                    );
                    return true;
                };
                let role_matches = matches!(
                    (identity.role, poll.phase),
                    (BackendRole::Broker, FrontendPollPhase::BrokerNext)
                        | (BackendRole::Broker, FrontendPollPhase::BrokerReadyNext)
                        | (BackendRole::Broker, FrontendPollPhase::BrokerRetired)
                        | (BackendRole::Broker, FrontendPollPhase::BrokerControl)
                        | (BackendRole::Agent, FrontendPollPhase::AgentClaim)
                );
                if poll.phase == FrontendPollPhase::BrokerReadyNext
                    && !self.frontend_completions.iter().any(|completion| {
                        completion.intent_id == poll.intent_id
                            && completion.broker == Some(identity)
                            && completion.published
                            && completion.retired
                            && !completion.ready
                    })
                {
                    self.refuse(
                        &envelope,
                        RefusalCode::SelectorUnknown,
                        RetryClass::Never,
                        b"frontend readiness",
                    );
                    return true;
                }
                if poll.phase == FrontendPollPhase::BrokerControl
                    && (self.frontend_control(identity, poll.intent_id).is_none()
                        || self.frontend_polls.iter().any(|pending| {
                            pending.poll.phase == poll.phase
                                && pending.poll.intent_id == poll.intent_id
                        }))
                {
                    self.refuse(
                        &envelope,
                        RefusalCode::SelectorUnknown,
                        RetryClass::Never,
                        b"frontend control",
                    );
                    return true;
                }
                if poll.phase == FrontendPollPhase::BrokerRetired
                    && !self.frontend_completions.iter().any(|completion| {
                        completion.intent_id == poll.intent_id
                            && completion.broker == Some(identity)
                            && completion.published
                            && !completion.retired
                            && !self.frontend_polls.iter().any(|pending| {
                                pending.poll.phase == FrontendPollPhase::BrokerRetired
                                    && pending.poll.intent_id == poll.intent_id
                            })
                            && !completion.worker.is_some_and(|worker| {
                                self.routes.iter().any(|route| route.identity == worker)
                            })
                            && !self
                                .frontend_receipts
                                .iter()
                                .any(|receipt| receipt.intent_id == poll.intent_id)
                    })
                {
                    self.refuse(
                        &envelope,
                        RefusalCode::SelectorUnknown,
                        RetryClass::Never,
                        b"frontend retirement",
                    );
                    return true;
                }
                if !role_matches || self.frontend_polls.len() == FRONTEND_POLLS_MAX {
                    self.refuse(
                        &envelope,
                        if role_matches {
                            RefusalCode::Backpressure
                        } else {
                            RefusalCode::InvalidEncoding
                        },
                        RetryClass::NewRequest,
                        b"frontend poll role",
                    );
                    return true;
                }
                Some(PendingFrontendPoll {
                    identity,
                    request: envelope.request_id,
                    turn: request.turn,
                    poll,
                    submitted_tick: self.tick,
                })
            }
            InferenceRequestKind::FrontendResult => {
                let Some(result) = decode_frontend_result(request.input) else {
                    self.refuse(
                        &envelope,
                        RefusalCode::InvalidEncoding,
                        RetryClass::NewRequest,
                        b"frontend result",
                    );
                    return true;
                };
                let joining = self.frontend_completions.iter().any(|completion| {
                    completion.intent_id == result.intent_id
                        && completion.accepts_fallback(identity, result.kind)
                });
                let route_matches = joining
                    || self.frontend_intents.iter().any(|intent| {
                        intent.intent_id == result.intent_id
                            && match identity.role {
                                BackendRole::Agent => {
                                    intent.state == FrontendIntentState::AgentClaimed(identity)
                                }
                                // Only the resident broker can report a worker's startup
                                // failure when that worker cannot produce its own result.
                                BackendRole::Broker => {
                                    (result.kind == FrontendResultKind::Failed
                                        || (intent.cancel_requested
                                            && result.kind == FrontendResultKind::Cancelled))
                                        && intent.broker == Some(identity)
                                        && matches!(
                                            intent.state,
                                            FrontendIntentState::BrokerClaimed
                                                | FrontendIntentState::AgentClaimed(_)
                                        )
                                }
                                _ => false,
                            }
                    });
                if !route_matches
                    || self.frontend_receipts.len() == FRONTEND_RECEIPTS_MAX
                    || (!joining
                        && (self.frontend_results.len() == FRONTEND_RESULTS_MAX
                            || self.frontend_completions.len() == FRONTEND_RESULTS_MAX))
                {
                    self.refuse(
                        &envelope,
                        if route_matches {
                            RefusalCode::Backpressure
                        } else {
                            RefusalCode::SelectorUnknown
                        },
                        RetryClass::NewRequest,
                        b"frontend result route",
                    );
                    return true;
                }
                None
            }
            _ => unreachable!(),
        };
        if let Err(error) = self.routes[route_index]
            .state
            .open_request(envelope.request_id)
        {
            let code = match error {
                RequestError::Duplicate => RefusalCode::Duplicate,
                RequestError::Zero => RefusalCode::InvalidEncoding,
                RequestError::CapacityExhausted => RefusalCode::Backpressure,
                RequestError::Unknown => RefusalCode::SelectorUnknown,
            };
            self.refuse(&envelope, code, RetryClass::SameEpoch, b"");
            return true;
        }
        if let Err(error) = self.routes[route_index].state.commit_received(&envelope) {
            let _ = self.routes[route_index]
                .state
                .close_request(envelope.request_id);
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope, code, retry, b"");
            return true;
        }
        self.routes[route_index]
            .stream_parts
            .push((envelope.request_id, 0));
        self.metrics.submitted = self.metrics.submitted.saturating_add(1);
        self.sink.observe(BenchmarkEvent::Submitted {
            route: identity.route,
            request: envelope.request_id,
            tick: self.tick,
        });
        if let Some(pending) = accepted {
            if pending.poll.phase == FrontendPollPhase::BrokerReadyNext {
                for completion in &mut self.frontend_completions {
                    if completion.intent_id == pending.poll.intent_id
                        && completion.broker == Some(identity)
                        && completion.retired
                    {
                        completion.ready = true;
                    }
                }
            }
            self.frontend_polls.push_back(pending);
            self.service_frontend_polls();
            if matches!(
                pending.poll.phase,
                FrontendPollPhase::BrokerNext | FrontendPollPhase::BrokerReadyNext
            ) && self.frontend_polls.iter().any(|candidate| {
                candidate.identity == pending.identity && candidate.request == pending.request
            }) {
                self.log("frontend broker poll parked");
            }
        } else if request.kind == InferenceRequestKind::FrontendUpdateResult {
            let result = kore_hostlink::decode_frontend_update_result(request.input)
                .expect("validated update result");
            let accepted = self.frontend_updates.accept(
                UpdateReceipt {
                    identity,
                    request: envelope.request_id,
                    turn: request.turn,
                    submitted_tick: self.tick,
                },
                result,
            );
            debug_assert!(accepted);
            self.service_frontend_update_receipts();
        } else {
            let result = decode_frontend_result(request.input)
                .expect("frontend result was validated before request admission");
            let published = if let Some(completion) = self
                .frontend_completions
                .iter_mut()
                .find(|completion| completion.intent_id == result.intent_id)
            {
                completion.broker_joined = true;
                completion.published
            } else {
                let index = self
                    .frontend_intents
                    .iter()
                    .position(|intent| intent.intent_id == result.intent_id)
                    .expect("frontend result retained its claimed intent");
                let intent = self.frontend_intents.remove(index).unwrap();
                self.frontend_updates.cancel_undelivered(intent.intent_id);
                self.frontend_completions.push_back(FrontendCompletion {
                    intent_id: result.intent_id,
                    source: identity,
                    broker: intent.broker,
                    worker: match intent.state {
                        FrontendIntentState::AgentClaimed(worker) => Some(worker),
                        _ => None,
                    },
                    published: false,
                    broker_joined: identity.role == BackendRole::Broker,
                    retired: false,
                    ready: false,
                    cancel_requested: intent.cancel_requested,
                    connection_lost: false,
                });
                self.frontend_results.push_back(FrontendResultOwned {
                    intent_id: result.intent_id,
                    // Cancellation accepted before a terminal result fixes
                    // the outcome. A late worker completion cannot undo it.
                    kind: if intent.cancel_requested {
                        FrontendResultKind::Cancelled
                    } else {
                        result.kind
                    },
                    text: if intent.cancel_requested {
                        "Prompt cancelled.".to_owned()
                    } else {
                        result.text.to_owned()
                    },
                });
                false
            };
            self.frontend_receipts.push_back(PendingFrontendReceipt {
                intent_id: result.intent_id,
                identity,
                request: envelope.request_id,
                turn: request.turn,
                submitted_tick: self.tick,
            });
            self.log("frontend result accepted");
            if published {
                self.acknowledge_frontend_result(result.intent_id);
            }
        }
        true
    }

    fn service_frontend_polls(&mut self) {
        self.service_frontend_update_receipts();
        let mut index = 0;
        while index < self.frontend_polls.len() {
            let pending = self.frontend_polls[index];
            if pending.poll.phase == FrontendPollPhase::BrokerControl {
                let mut wire = [0; INFERENCE_PART_MAX];
                let planning = self
                    .frontend_intents
                    .iter()
                    .any(|intent| intent.state == FrontendIntentState::BrokerClaimed)
                    || !self.backend.broker_prompt_ready(pending.identity);
                let update = self
                    .frontend_updates
                    .pending(pending.identity, pending.poll.intent_id)
                    .filter(|_| !planning)
                    .and_then(|update| Some((update.revision, update.encode(&mut wire)?)));
                if let Some((revision, len)) = update {
                    if self.finish_frontend_request(
                        pending.identity,
                        pending.request,
                        pending.turn,
                        pending.submitted_tick,
                        &wire[..len],
                    ) {
                        self.frontend_updates
                            .delivered(pending.poll.intent_id, revision);
                        self.frontend_polls.remove(index);
                    } else {
                        index += 1;
                    }
                    continue;
                }
                if let Some(control) =
                    self.frontend_control(pending.identity, pending.poll.intent_id)
                    && self.finish_frontend_request(
                        pending.identity,
                        pending.request,
                        pending.turn,
                        pending.submitted_tick,
                        &[control as u8],
                    )
                {
                    self.frontend_polls.remove(index);
                } else {
                    index += 1;
                }
                continue;
            }
            if pending.poll.phase == FrontendPollPhase::BrokerRetired {
                // Drain the broker's earlier control exchange first. Its reply
                // is one byte larger and can hit backpressure even when the
                // empty retirement acknowledgement would fit.
                if self.frontend_polls.iter().any(|other| {
                    other.poll.phase == FrontendPollPhase::BrokerControl
                        && other.poll.intent_id == pending.poll.intent_id
                }) {
                    index += 1;
                    continue;
                }
                // Admission checked the exact broker, published outcome and
                // closed worker route. Output backpressure retains the request.
                if self.finish_frontend_request(
                    pending.identity,
                    pending.request,
                    pending.turn,
                    pending.submitted_tick,
                    &[],
                ) {
                    if let Some(completion) = self
                        .frontend_completions
                        .iter_mut()
                        .find(|completion| completion.intent_id == pending.poll.intent_id)
                    {
                        completion.retired = true;
                    }
                    self.frontend_polls.remove(index);
                    self.log("frontend native retirement confirmed");
                } else {
                    index += 1;
                }
                continue;
            }
            let intent_index = match pending.poll.phase {
                FrontendPollPhase::BrokerNext | FrontendPollPhase::BrokerReadyNext => {
                    // Worker lanes share one broker inference sequence. Keep
                    // planning/founding serial until the worker claims its
                    // intent, then allow the next plan alongside that worker.
                    // Backend cancellation can acknowledge before cleanup, so
                    // require the sequence owner to be idle as well.
                    if self.frontend_updates.requires_progress()
                        || !self.backend.broker_prompt_ready(pending.identity)
                        || self.frontend_intents.iter().any(|intent| {
                            intent.broker == Some(pending.identity)
                                && intent.state == FrontendIntentState::BrokerClaimed
                        })
                    {
                        None
                    } else {
                        self.frontend_intents
                            .iter()
                            .position(|intent| intent.state == FrontendIntentState::Queued)
                    }
                }
                FrontendPollPhase::AgentClaim => self.frontend_intents.iter().position(|intent| {
                    intent.intent_id == pending.poll.intent_id
                        && intent.state == FrontendIntentState::BrokerClaimed
                }),
                FrontendPollPhase::BrokerRetired | FrontendPollPhase::BrokerControl => {
                    unreachable!()
                }
            };
            let Some(intent_index) = intent_index else {
                index += 1;
                continue;
            };
            let intent_id = self.frontend_intents[intent_index].intent_id;
            let text = self.frontend_intents[intent_index].text.clone();
            let mut payload = [0; INFERENCE_PART_MAX];
            let Some(length) = (FrontendIntent {
                intent_id,
                text: &text,
            })
            .encode(&mut payload) else {
                index += 1;
                continue;
            };
            if !self.finish_frontend_request(
                pending.identity,
                pending.request,
                pending.turn,
                pending.submitted_tick,
                &payload[..length],
            ) {
                index += 1;
                continue;
            }
            self.frontend_intents[intent_index].state = match pending.poll.phase {
                FrontendPollPhase::BrokerNext | FrontendPollPhase::BrokerReadyNext => {
                    self.frontend_intents[intent_index].broker = Some(pending.identity);
                    FrontendIntentState::BrokerClaimed
                }
                FrontendPollPhase::AgentClaim => {
                    FrontendIntentState::AgentClaimed(pending.identity)
                }
                FrontendPollPhase::BrokerRetired | FrontendPollPhase::BrokerControl => {
                    unreachable!()
                }
            };
            self.frontend_polls.remove(index);
            self.log(match pending.poll.phase {
                FrontendPollPhase::BrokerNext | FrontendPollPhase::BrokerReadyNext => {
                    "frontend intent claimed by broker"
                }
                FrontendPollPhase::AgentClaim => "frontend intent claimed by agent",
                FrontendPollPhase::BrokerRetired | FrontendPollPhase::BrokerControl => {
                    unreachable!()
                }
            });
        }
    }

    fn service_frontend_update_receipts(&mut self) {
        while let Some((revision, receipt)) = self.frontend_updates.receipt() {
            if !self.finish_frontend_request(
                receipt.identity,
                receipt.request,
                receipt.turn,
                receipt.submitted_tick,
                &[],
            ) {
                break;
            }
            self.frontend_updates.acknowledged(revision);
        }
    }

    fn finish_frontend_request(
        &mut self,
        identity: SequenceIdentity,
        request: RequestId,
        turn: u64,
        submitted_tick: u64,
        payload: &[u8],
    ) -> bool {
        // A delayed receipt must never be sent to a replacement generation.
        if !self.routes.iter().any(|route| route.identity == identity) {
            return false;
        }
        let mut encoded = [0; INFERENCE_EVENT_HEADER_LEN + INFERENCE_PART_MAX];
        let Some(length) = (InferenceEventView {
            kind: InferenceEventKind::TurnFinished,
            terminal: true,
            part: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            turn,
            payload,
        })
        .encode(&mut encoded) else {
            return false;
        };
        if !self.reply_routed_with_priority(
            SocketPlane::Control,
            INFER_REPLY,
            identity.route,
            request,
            &encoded[..length],
            true,
        ) {
            return false;
        }
        let Some(route) = self
            .routes
            .iter_mut()
            .find(|route| route.identity == identity)
        else {
            return false;
        };
        let _ = route.state.close_request(request);
        route
            .stream_parts
            .retain(|(candidate, _)| *candidate != request);
        self.metrics.completed = self.metrics.completed.saturating_add(1);
        self.sink.observe(BenchmarkEvent::Completed {
            route: identity.route,
            request,
            ticks: self.tick.saturating_sub(submitted_tick),
        });
        true
    }
    fn route_mut(&mut self, id: RouteId) -> Option<&mut Route> {
        self.routes.iter_mut().find(|r| r.state.route_id() == id)
    }

    pub fn receive(&mut self, plane: SocketPlane, envelope: EnvelopeV1, payload: &[u8]) {
        match plane {
            SocketPlane::Control => self.metrics.control_frames_in += 1,
            SocketPlane::Bulk => self.metrics.bulk_frames_in += 1,
        }
        self.metrics.bytes_in += (kore_hostlink::ENVELOPE_LEN + payload.len()) as u64;
        if envelope.family.get() != kore_hostlink::FAMILY_LINK
            || envelope.schema_minor.get() != 0
            || envelope.payload_len.get() as usize != payload.len()
        {
            self.metrics.malformed += 1;
            self.log("refused malformed-envelope");
            self.refuse(&envelope, RefusalCode::Malformed, RetryClass::Never, b"");
            return;
        }
        if envelope.kind.get() == REFUSED {
            // Never answer a refusal with another refusal. It is either a
            // negative acknowledgement of one of our already-advanced routed
            // sends or a failed negotiation; both require a fresh epoch.
            if let Some(refusal) = decode_refusal(payload) {
                self.log(format!(
                    "peer refused for_kind={} request={} code={:?} retry={:?} detail={}",
                    refusal.for_kind,
                    hex_bytes(&envelope.request_id.0),
                    refusal.code,
                    refusal.retry,
                    String::from_utf8_lossy(refusal.safe_detail)
                ));
            } else {
                self.log("peer refused with malformed payload");
            }
            self.require_reconnect(false);
            return;
        }
        if plane == SocketPlane::Control && envelope.kind.get() == HELLO_ACK {
            self.hello_ack(envelope, payload);
            return;
        }
        if envelope.kind.get() == HELLO {
            self.metrics.malformed += 1;
            self.refuse(
                &envelope,
                RefusalCode::UnknownKind,
                RetryClass::Never,
                b"wrong direction",
            );
            return;
        }
        let Some(epoch) = self.epoch else {
            self.metrics.stale_route += 1;
            self.log("refused no-epoch");
            self.refuse(
                &envelope,
                RefusalCode::EpochStale,
                RetryClass::AfterReconnect,
                b"",
            );
            return;
        };
        if envelope.link_epoch != epoch {
            self.metrics.stale_route += 1;
            self.log("refused stale-epoch");
            self.refuse(
                &envelope,
                RefusalCode::EpochStale,
                RetryClass::AfterReconnect,
                b"",
            );
            return;
        }
        if plane == SocketPlane::Control && envelope.kind.get() == ROUTE_OPEN {
            self.open_route(envelope, payload);
            return;
        }
        let kind = envelope.kind.get();
        if plane == SocketPlane::Bulk
            && matches!(kind, BULK_BEGIN | BULK_CHUNK | BULK_FINISH | BULK_ABORT)
        {
            self.receive_bulk(envelope, payload);
            return;
        }
        if plane == SocketPlane::Control && kind == CANCEL {
            self.receive_cancel(envelope, payload);
            return;
        }
        if plane == SocketPlane::Control && matches!(kind, BULK_ACCEPTED | BULK_READY) {
            self.receive_outbound_bulk_ack(envelope, payload);
            return;
        }
        let Some(index) = self
            .routes
            .iter()
            .position(|route| route.state.route_id() == envelope.route_id)
        else {
            self.metrics.stale_route += 1;
            self.log("refused unknown-route");
            self.refuse(
                &envelope,
                RefusalCode::RouteUnknown,
                RetryClass::NewRequest,
                b"",
            );
            return;
        };
        if let Err(error) = self.routes[index].state.check_received(&envelope) {
            match error {
                AdmitError::Sequence(
                    SequenceDisposition::Duplicate | SequenceDisposition::OutOfOrder,
                ) => self.metrics.replay_or_order += 1,
                _ => self.metrics.stale_route += 1,
            }
            self.log(format!("refused admission {error:?}"));
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope, code, retry, b"");
            return;
        }
        let request_kind = match self.routes[index].state.role() {
            RouteRole::Importer | RouteRole::Exporter => WORKSPACE_REQUEST,
            RouteRole::Broker | RouteRole::Agent => INFER_REQUEST,
        };
        match (plane, kind) {
            (SocketPlane::Control, kore_hostlink::AUTHORITY_REPORT)
                if self.routes[index].state.role() == RouteRole::Broker =>
            {
                let Some(report) = kore_hostlink::decode_authority_report(payload) else {
                    self.metrics.malformed = self.metrics.malformed.saturating_add(1);
                    self.refuse(
                        &envelope,
                        RefusalCode::InvalidEncoding,
                        RetryClass::NewRequest,
                        b"authority report",
                    );
                    return;
                };
                if let Err(error) = self.routes[index].state.commit_received(&envelope) {
                    let (code, retry) = admission_refusal(error);
                    self.refuse(&envelope, code, retry, b"");
                    return;
                }
                if self.authority_observations.len() == AUTHORITY_OBSERVATIONS_MAX {
                    self.authority_observations.pop_front();
                }
                self.authority_observations.push_back(AuthorityObservation {
                    state: report.state,
                    kind: report.kind,
                    agent_slot: report.agent_slot,
                    generation: report.generation,
                    rights: report.rights,
                    label: report.label.to_owned(),
                });
                self.log("authority report accepted");
            }
            (SocketPlane::Control, actual)
                if actual == request_kind && !payload.is_empty() && payload.len() <= INLINE_MAX =>
            {
                if self.receive_frontend(index, envelope, payload) {
                    return;
                }
                if let Err(error) = self.routes[index].state.open_request(envelope.request_id) {
                    let code = match error {
                        RequestError::Duplicate => RefusalCode::Duplicate,
                        RequestError::Zero => RefusalCode::InvalidEncoding,
                        RequestError::CapacityExhausted => RefusalCode::Backpressure,
                        RequestError::Unknown => RefusalCode::SelectorUnknown,
                    };
                    self.metrics.backpressure += 1;
                    self.refuse(&envelope, code, RetryClass::SameEpoch, b"");
                    return;
                }
                let identity = self.routes[index].identity;
                if let Err(error) =
                    self.backend
                        .submit(identity, envelope.request_id, payload, self.tick)
                {
                    let _ = self.routes[index].state.close_request(envelope.request_id);
                    let (code, retry) = backend_refusal(error);
                    self.metrics.backpressure += 1;
                    self.log(format!("backend refused submit {error:?}"));
                    self.refuse(&envelope, code, retry, b"");
                    return;
                }
                self.routes[index]
                    .stream_parts
                    .push((envelope.request_id, 0));
                if let Err(error) = self.routes[index].state.commit_received(&envelope) {
                    let _ = self
                        .backend
                        .cancel(identity, envelope.request_id, self.tick);
                    let _ = self.routes[index].state.close_request(envelope.request_id);
                    self.routes[index]
                        .stream_parts
                        .retain(|(request, _)| *request != envelope.request_id);
                    let (code, retry) = admission_refusal(error);
                    self.refuse(&envelope, code, retry, b"");
                    return;
                }
                self.metrics.submitted += 1;
                self.sink.observe(BenchmarkEvent::Submitted {
                    route: envelope.route_id,
                    request: envelope.request_id,
                    tick: self.tick,
                });
                self.log(format!(
                    "submitted role={:?} route={} generation={} request={}",
                    identity.role,
                    hex_bytes(&identity.route.0),
                    identity.generation.get(),
                    hex_bytes(&envelope.request_id.0),
                ));
            }
            (SocketPlane::Control, ROUTE_CLOSE) if payload.is_empty() => {
                if let Err(error) = self.routes[index].state.commit_received(&envelope) {
                    let (code, retry) = admission_refusal(error);
                    self.refuse(&envelope, code, retry, b"");
                    return;
                }
                if !self.reply_routed(
                    SocketPlane::Control,
                    ROUTE_CLOSED,
                    envelope.route_id,
                    envelope.request_id,
                    &[],
                ) {
                    self.disconnect();
                    return;
                }
                let identity = self.routes[index].identity;
                let _ = self.backend.close(identity);
                self.frontend_updates.close_broker(identity);
                self.routes.swap_remove(index);
                // A confirmed route close ends only this generation's pending
                // receipt delivery. Never redirect it to a replacement route.
                self.frontend_receipts
                    .retain(|receipt| receipt.identity != identity);
                self.log("route closed");
            }
            _ => {
                self.metrics.malformed += 1;
                self.log("refused kind-or-shape");
                self.refuse(&envelope, RefusalCode::UnknownKind, RetryClass::Never, b"");
            }
        }
    }

    fn receive_cancel(&mut self, envelope_in: EnvelopeV1, payload: &[u8]) {
        if !payload.is_empty() {
            self.refuse(&envelope_in, RefusalCode::Malformed, RetryClass::Never, b"");
            return;
        }
        let Some(index) = self
            .routes
            .iter()
            .position(|route| route.state.route_id() == envelope_in.route_id)
        else {
            self.refuse(
                &envelope_in,
                RefusalCode::RouteUnknown,
                RetryClass::NewRequest,
                b"",
            );
            return;
        };
        if let Err(error) = self.routes[index].state.check_received(&envelope_in) {
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope_in, code, retry, b"");
            return;
        }

        let retired = self.retire_bulk(envelope_in.route_id, envelope_in.request_id);
        let identity = self.routes[index].identity;
        if let Some(pending) = self.frontend_polls.iter().position(|pending| {
            pending.identity == identity && pending.request == envelope_in.request_id
        }) {
            self.frontend_polls.remove(pending);
            if let Err(error) = self.routes[index].state.commit_received(&envelope_in) {
                let (code, retry) = admission_refusal(error);
                self.refuse(&envelope_in, code, retry, b"");
                return;
            }
            if !self.reply_routed_with_priority(
                SocketPlane::Control,
                CANCELLED,
                envelope_in.route_id,
                envelope_in.request_id,
                &[],
                true,
            ) {
                self.disconnect();
                return;
            }
            let _ = self.routes[index]
                .state
                .close_request(envelope_in.request_id);
            self.routes[index]
                .stream_parts
                .retain(|(request, _)| *request != envelope_in.request_id);
            self.metrics.cancelled = self.metrics.cancelled.saturating_add(1);
            self.log("frontend poll cancelled");
            return;
        }
        let backend_event = self
            .backend
            .cancel(identity, envelope_in.request_id, self.tick)
            .ok();
        if !retired && backend_event.is_none() {
            if self.routes[index]
                .completed_requests
                .contains(&envelope_in.request_id)
            {
                if let Err(error) = self.routes[index].state.commit_received(&envelope_in) {
                    let (code, retry) = admission_refusal(error);
                    self.refuse(&envelope_in, code, retry, b"");
                }
                // The original terminal reply is the completion of this
                // request. Sending CANCELLED as well would replay its terminal.
                return;
            }
            self.refuse(
                &envelope_in,
                RefusalCode::SelectorUnknown,
                RetryClass::Never,
                b"",
            );
            return;
        }
        if let Err(error) = self.routes[index].state.commit_received(&envelope_in) {
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope_in, code, retry, b"");
            return;
        }
        if let Some(event) = backend_event {
            self.finish(event);
        } else {
            if !self.reply_routed_with_priority(
                SocketPlane::Control,
                CANCELLED,
                envelope_in.route_id,
                envelope_in.request_id,
                &[],
                true,
            ) {
                self.disconnect();
                return;
            }
            let _ = self.routes[index]
                .state
                .close_request(envelope_in.request_id);
            self.metrics.cancelled += 1;
        }
    }

    fn receive_outbound_bulk_ack(&mut self, envelope_in: EnvelopeV1, payload: &[u8]) {
        let Some(index) = self
            .routes
            .iter()
            .position(|route| route.state.route_id() == envelope_in.route_id)
        else {
            self.refuse(
                &envelope_in,
                RefusalCode::RouteUnknown,
                RetryClass::NewRequest,
                b"",
            );
            return;
        };
        if let Err(error) = self.routes[index].state.check_received(&envelope_in) {
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope_in, code, retry, b"");
            return;
        }
        let Ok(bulk_id) = <[u8; 16]>::try_from(payload) else {
            self.refuse(
                &envelope_in,
                RefusalCode::InvalidLength,
                RetryClass::Never,
                b"",
            );
            return;
        };
        let Some(outbound_index) = self.outbound_bulk.iter().position(|bulk| {
            bulk.route == envelope_in.route_id
                && bulk.request == envelope_in.request_id
                && bulk.bulk_id == BulkId(bulk_id)
        }) else {
            self.refuse(
                &envelope_in,
                RefusalCode::SelectorUnknown,
                RetryClass::Never,
                b"",
            );
            return;
        };
        let terminal = match envelope_in.kind.get() {
            BULK_ACCEPTED
                if self.outbound_bulk[outbound_index].accepted
                    < self.outbound_bulk[outbound_index].accepted_expected =>
            {
                self.outbound_bulk[outbound_index].accepted += 1;
                false
            }
            BULK_READY
                if self.outbound_bulk[outbound_index].accepted
                    == self.outbound_bulk[outbound_index].accepted_expected =>
            {
                true
            }
            _ => {
                self.refuse(
                    &envelope_in,
                    RefusalCode::OutOfOrder,
                    RetryClass::Never,
                    b"",
                );
                return;
            }
        };
        if let Err(error) = self.routes[index].state.commit_received(&envelope_in) {
            let (code, retry) = admission_refusal(error);
            self.refuse(&envelope_in, code, retry, b"");
            return;
        }
        if terminal {
            self.outbound_bulk.swap_remove(outbound_index);
            let _ = self.routes[index]
                .state
                .close_request(envelope_in.request_id);
        }
    }

    fn hello_ack(&mut self, ack: EnvelopeV1, payload: &[u8]) {
        let Some(host_nonce) = self.negotiation_nonce else {
            self.refuse(
                &ack,
                RefusalCode::EpochStale,
                RetryClass::AfterReconnect,
                b"hello not pending",
            );
            return;
        };
        if payload.len() != HELLO_ACK_LEN
            || !ack.link_epoch.is_zero()
            || !ack.route_id.is_zero()
            || ack.task_generation.get() != 0
            || ack.sequence.get() != 0
            || !ack.request_id.is_zero()
        {
            self.metrics.malformed += 1;
            self.log("refused hello-ack-shape");
            self.refuse(&ack, RefusalCode::Malformed, RetryClass::Never, b"");
            return;
        }
        let hello_ack = match HelloAckV1::read_from_bytes(payload) {
            Ok(hello_ack) => hello_ack,
            Err(_) => {
                self.refuse(&ack, RefusalCode::InvalidEncoding, RetryClass::Never, b"");
                return;
            }
        };
        let epoch = derive_link_epoch(&host_nonce, &hello_ack.guest_boot, negotiated_transcript());
        if hello_ack.epoch != epoch {
            self.metrics.malformed += 1;
            self.refuse(
                &ack,
                RefusalCode::InvalidEncoding,
                RetryClass::AfterReconnect,
                b"epoch mismatch",
            );
            return;
        }
        self.epoch = Some(epoch);
        self.negotiation_nonce = None;
        self.metrics.epochs += 1;
        self.log("epoch established");
    }

    fn open_route(&mut self, envelope_in: EnvelopeV1, payload: &[u8]) {
        if payload.len() != ROUTE_OPEN_LEN
            || !envelope_in.route_id.is_zero()
            || envelope_in.task_generation.get() != 0
            || envelope_in.sequence.get() != 0
            || envelope_in.request_id != RequestId::ZERO
        {
            self.metrics.malformed += 1;
            self.log("refused route-open-shape");
            self.refuse(&envelope_in, RefusalCode::Malformed, RetryClass::Never, b"");
            return;
        }
        if self.routes.len() == ROUTES_MAX {
            self.metrics.backpressure = self.metrics.backpressure.saturating_add(1);
            self.refuse(
                &envelope_in,
                RefusalCode::CapacityExhausted,
                RetryClass::AfterReconnect,
                b"",
            );
            return;
        }
        let route_open = match RouteOpenV1::read_from_bytes(payload) {
            Ok(route_open) => route_open,
            Err(_) => {
                self.refuse(
                    &envelope_in,
                    RefusalCode::InvalidEncoding,
                    RetryClass::NewRequest,
                    b"",
                );
                return;
            }
        };
        let route_id = route_open.route_id;
        let generation_value = route_open.task_generation.get();
        let Some(generation) = TaskGeneration::new(generation_value) else {
            self.metrics.malformed += 1;
            self.refuse(
                &envelope_in,
                RefusalCode::InvalidEncoding,
                RetryClass::NewRequest,
                b"",
            );
            return;
        };
        if route_id.is_zero() {
            self.refuse(
                &envelope_in,
                RefusalCode::InvalidEncoding,
                RetryClass::NewRequest,
                b"",
            );
            return;
        }
        let backend_role = match (route_open.role, route_open.ordinal) {
            (0, 0) => BackendRole::Broker,
            (1, _) => BackendRole::Agent,
            (2, 0) => BackendRole::Workspace,
            _ => {
                self.metrics.malformed += 1;
                self.log("refused sequence-key");
                self.refuse(
                    &envelope_in,
                    RefusalCode::InvalidEncoding,
                    RetryClass::NewRequest,
                    b"",
                );
                return;
            }
        };
        let role = match backend_role {
            BackendRole::Broker => RouteRole::Broker,
            BackendRole::Agent => RouteRole::Agent,
            BackendRole::Workspace => RouteRole::Importer,
        };
        let identity = SequenceIdentity {
            route: route_id,
            generation,
            role: backend_role,
        };
        let Some(state) = RouteState::new(
            self.epoch.unwrap(),
            route_id,
            generation,
            role,
            16,
            HOST_BULK_BYTE_CREDITS_PER_ROUTE,
            HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
        ) else {
            return;
        };
        if self.routes.iter().any(|r| r.state.route_id() == route_id) {
            self.metrics.stale_route += 1;
            self.log("refused duplicate route-open");
            self.refuse(
                &envelope_in,
                RefusalCode::Duplicate,
                RetryClass::NewRequest,
                b"",
            );
            return;
        }
        if let Err(error) = self.backend.open(identity) {
            let (code, retry) = backend_refusal(error);
            self.log(format!("backend refused route-open {error:?}"));
            self.refuse(&envelope_in, code, retry, b"");
            return;
        }
        let out = envelope(
            ROUTE_OPENED,
            self.epoch.unwrap(),
            RouteId::ZERO,
            0,
            0,
            RequestId::ZERO,
            payload.len(),
        );
        self.routes.push(Route {
            state,
            identity,
            stream_parts: Vec::with_capacity(16),
            completed_requests: VecDeque::new(),
        });
        self.enqueue(SocketPlane::Control, frame(&out, payload));
        self.log(format!(
            "route opened {:?} caller-ordinal={}",
            backend_role, route_open.ordinal
        ));
    }

    fn reply_routed(
        &mut self,
        plane: SocketPlane,
        kind: u16,
        route_id: RouteId,
        request: RequestId,
        payload: &[u8],
    ) -> bool {
        let priority = !matches!(kind, INFER_REPLY | BULK_ACCEPTED | BULK_READY);
        self.reply_routed_with_priority(plane, kind, route_id, request, payload, priority)
    }

    fn reply_routed_with_priority(
        &mut self,
        plane: SocketPlane,
        kind: u16,
        route_id: RouteId,
        request: RequestId,
        payload: &[u8],
        priority: bool,
    ) -> bool {
        let Some(index) = self
            .routes
            .iter()
            .position(|route| route.state.route_id() == route_id)
        else {
            return false;
        };
        let Ok(sequence) = self.routes[index].state.send_sequence() else {
            return false;
        };
        let generation = self.routes[index].state.generation().get();
        let epoch = self.epoch.unwrap();
        let out = envelope(
            kind,
            epoch,
            route_id,
            generation,
            sequence,
            request,
            payload.len(),
        );
        if kind == INFER_REPLY {
            let event = decode_inference_event(payload);
            self.log(format!(
                "infer reply request={} sequence={} bytes={} event={:?}",
                hex_bytes(&request.0),
                sequence,
                payload.len(),
                event.map(|event| (event.kind, event.terminal, event.turn, event.part))
            ));
        }
        if !self.enqueue_with_priority(plane, frame(&out, payload), priority) {
            return false;
        }
        self.routes[index].state.commit_sent(sequence).is_ok()
    }

    fn reply_bulk(
        &mut self,
        route_id: RouteId,
        request: RequestId,
        bulk_id: BulkId,
        content_kind: u16,
        payload: &[u8],
    ) -> bool {
        let Some(route_index) = self
            .routes
            .iter()
            .position(|route| route.state.route_id() == route_id)
        else {
            return false;
        };
        let Some(payloads) = outbound_bulk_payloads(bulk_id, content_kind, payload) else {
            return false;
        };
        if self.outbound_bulk.len() == OUTBOUND_BULK_MAX
            || self.outbound_bulk.iter().any(|bulk| {
                bulk.route == route_id && (bulk.request == request || bulk.bulk_id == bulk_id)
            })
        {
            return false;
        }
        let accepted_expected = match u16::try_from(payloads.len().saturating_sub(1)) {
            Ok(expected) if expected != 0 => expected,
            _ => return false,
        };
        let queued_bytes = payloads.iter().try_fold(0usize, |total, (_, payload)| {
            total.checked_add(kore_hostlink::ENVELOPE_LEN + payload.len())
        });
        let Some(queued_bytes) = queued_bytes else {
            return false;
        };
        if !self.bulk_out.can_accept(payloads.len(), queued_bytes) {
            self.metrics.backpressure = self.metrics.backpressure.saturating_add(1);
            return false;
        }
        let epoch = self.epoch.unwrap();
        let generation = self.routes[route_index].state.generation().get();
        let Ok(first_sequence) = self.routes[route_index].state.send_sequence() else {
            return false;
        };
        for (index, (kind, payload)) in payloads.into_iter().enumerate() {
            let Some(sequence) = first_sequence.checked_add(index as u64) else {
                return false;
            };
            let out = envelope(
                kind,
                epoch,
                route_id,
                generation,
                sequence,
                request,
                payload.len(),
            );
            let encoded = frame(&out, &payload);
            self.bulk_out
                .push(encoded)
                .expect("bulk reply was preflighted against the bounded queue");
            self.routes[route_index]
                .state
                .commit_sent(sequence)
                .expect("bulk reply sequences are consecutive");
        }
        self.outbound_bulk.push(OutboundBulk {
            route: route_id,
            request,
            bulk_id,
            accepted_expected,
            accepted: 0,
        });
        self.metrics.max_queued_bytes = self.metrics.max_queued_bytes.max(self.bulk_out.bytes);
        true
    }
    fn finish(&mut self, event: BackendEvent) {
        let identity = event.sequence();
        let Some(index) = self
            .routes
            .iter()
            .position(|route| route.identity == identity)
        else {
            self.metrics.stale_backend_events = self.metrics.stale_backend_events.saturating_add(1);
            self.log("discarded stale backend event");
            return;
        };
        let request = event.request();
        if !matches!(&event, BackendEvent::Chunk { .. })
            && matches!(identity.role, BackendRole::Broker | BackendRole::Agent)
            && self.routes[index].state.has_request(request)
        {
            let history = &mut self.routes[index].completed_requests;
            if history.len() == crate::backend::REQUEST_HISTORY_MAX {
                history.pop_front();
            }
            history.push_back(request);
        }
        match event {
            BackendEvent::Chunk {
                sequence,
                request,
                payload,
                part,
                submitted_tick,
                turn,
            } => {
                if !self.routes[index].state.has_request(request) {
                    self.metrics.stale_backend_events =
                        self.metrics.stale_backend_events.saturating_add(1);
                    self.log("discarded backend chunk for terminal request");
                    return;
                }
                let Some(expected) = self.routes[index]
                    .stream_parts
                    .iter_mut()
                    .find_map(|(candidate, expected)| (*candidate == request).then_some(expected))
                else {
                    self.metrics.stale_backend_events =
                        self.metrics.stale_backend_events.saturating_add(1);
                    return;
                };
                if part != *expected {
                    self.metrics.replay_or_order = self.metrics.replay_or_order.saturating_add(1);
                    self.disconnect();
                    return;
                }
                *expected = expected.saturating_add(1);
                self.metrics.streamed_parts = self.metrics.streamed_parts.saturating_add(1);
                if part == 0 {
                    self.sink.observe(BenchmarkEvent::FirstToken {
                        route: sequence.route,
                        request,
                        ticks: self.tick.saturating_sub(submitted_tick),
                    });
                }
                let mut encoded = [0u8; INFERENCE_EVENT_HEADER_LEN + INFERENCE_PART_MAX];
                let Some(length) = (InferenceEventView {
                    kind: InferenceEventKind::AssistantPart,
                    terminal: false,
                    part,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    turn,
                    payload: &payload,
                })
                .encode(&mut encoded) else {
                    self.metrics.malformed = self.metrics.malformed.saturating_add(1);
                    self.disconnect();
                    return;
                };
                if !self.reply_routed_with_priority(
                    SocketPlane::Control,
                    INFER_REPLY,
                    sequence.route,
                    request,
                    &encoded[..length],
                    false,
                ) {
                    self.disconnect();
                    return;
                }
                self.log("streamed");
            }
            BackendEvent::Completed {
                sequence,
                request,
                payload,
                part,
                submitted_tick,
                usage,
                turn,
            } => {
                if !self.routes[index].state.has_request(request) {
                    self.metrics.stale_backend_events =
                        self.metrics.stale_backend_events.saturating_add(1);
                    self.log("discarded duplicate backend completion");
                    return;
                }
                let reply_kind = match self.routes[index].state.role() {
                    RouteRole::Importer | RouteRole::Exporter => WORKSPACE_REPLY,
                    RouteRole::Broker | RouteRole::Agent => INFER_REPLY,
                };
                if reply_kind == WORKSPACE_REPLY {
                    // Workspace uploads open their request with BULK_BEGIN,
                    // so they have no inference stream-part row.  Their
                    // application result is nevertheless a strictly bounded
                    // single terminal part.
                    if part != 0 || turn != 0 {
                        self.metrics.replay_or_order =
                            self.metrics.replay_or_order.saturating_add(1);
                        self.disconnect();
                        return;
                    }
                } else {
                    let expected =
                        self.routes[index]
                            .stream_parts
                            .iter()
                            .find_map(|(candidate, expected)| {
                                (*candidate == request).then_some(*expected)
                            });
                    if expected != Some(part) {
                        self.metrics.replay_or_order =
                            self.metrics.replay_or_order.saturating_add(1);
                        self.disconnect();
                        return;
                    }
                }
                let mut encoded = [0u8; INFERENCE_EVENT_HEADER_LEN + INFERENCE_PART_MAX];
                let terminal_payload = if reply_kind == WORKSPACE_REPLY {
                    payload.as_slice()
                } else if turn == 0 && part == 0 {
                    // KIR1 reserves turn zero. Preserve the pre-KIR1
                    // deterministic boot witness byte-for-byte so existing
                    // founded clients do not need a flag-day protocol change.
                    if payload.len() > INFERENCE_PART_MAX {
                        self.metrics.malformed = self.metrics.malformed.saturating_add(1);
                        self.disconnect();
                        return;
                    }
                    payload.as_slice()
                } else {
                    let Some(length) = (InferenceEventView {
                        kind: InferenceEventKind::TurnFinished,
                        terminal: true,
                        part,
                        prompt_tokens: u32::try_from(usage.prompt_tokens).unwrap_or(u32::MAX),
                        completion_tokens: u32::try_from(usage.completion_tokens)
                            .unwrap_or(u32::MAX),
                        turn,
                        payload: &payload,
                    })
                    .encode(&mut encoded) else {
                        self.metrics.malformed = self.metrics.malformed.saturating_add(1);
                        self.disconnect();
                        return;
                    };
                    &encoded[..length]
                };
                if !self.reply_routed_with_priority(
                    SocketPlane::Control,
                    reply_kind,
                    sequence.route,
                    request,
                    terminal_payload,
                    true,
                ) {
                    self.disconnect();
                    return;
                }
                if self.routes[index].state.close_request(request).is_err() {
                    self.disconnect();
                    return;
                }
                self.routes[index]
                    .stream_parts
                    .retain(|(candidate, _)| *candidate != request);
                self.metrics.completed += 1;
                if part == 0 {
                    self.sink.observe(BenchmarkEvent::FirstToken {
                        route: sequence.route,
                        request,
                        ticks: self.tick.saturating_sub(submitted_tick),
                    });
                }
                self.sink.observe(BenchmarkEvent::Completed {
                    route: sequence.route,
                    request,
                    ticks: self.tick.saturating_sub(submitted_tick),
                });
                self.log(format!(
                    "completed role={:?} route={} generation={} request={}",
                    sequence.role,
                    hex_bytes(&sequence.route.0),
                    sequence.generation.get(),
                    hex_bytes(&request.0),
                ));
            }
            BackendEvent::BulkCompleted {
                sequence,
                request,
                bulk_id,
                content_kind,
                payload,
                submitted_tick,
            } => {
                if self.reply_bulk(sequence.route, request, bulk_id, content_kind, &payload) {
                    self.metrics.completed = self.metrics.completed.saturating_add(1);
                    self.sink.observe(BenchmarkEvent::Completed {
                        route: sequence.route,
                        request,
                        ticks: self.tick.saturating_sub(submitted_tick),
                    });
                    self.log("bulk completed");
                } else {
                    // A backend event is terminal and cannot be re-polled.
                    // Refuse explicitly and force a fresh epoch rather than
                    // dropping it or exposing a partial transfer.
                    let source = envelope(
                        WORKSPACE_REQUEST,
                        self.epoch.unwrap_or(LinkEpoch::ZERO),
                        sequence.route,
                        self.route_mut(sequence.route)
                            .map_or(0, |r| r.state.generation().get()),
                        0,
                        request,
                        0,
                    );
                    self.refuse(
                        &source,
                        RefusalCode::Backpressure,
                        RetryClass::AfterReconnect,
                        b"",
                    );
                }
            }
            BackendEvent::Cancelled {
                sequence,
                request,
                submitted_tick,
                ..
            } => {
                if !self.routes[index].state.has_request(request) {
                    self.metrics.stale_backend_events =
                        self.metrics.stale_backend_events.saturating_add(1);
                    self.log("discarded duplicate backend cancellation");
                    return;
                }
                if !self.reply_routed_with_priority(
                    SocketPlane::Control,
                    CANCELLED,
                    sequence.route,
                    request,
                    &[],
                    true,
                ) {
                    self.disconnect();
                    return;
                }
                if self.routes[index].state.close_request(request).is_err() {
                    self.disconnect();
                    return;
                }
                self.routes[index]
                    .stream_parts
                    .retain(|(candidate, _)| *candidate != request);
                self.metrics.cancelled += 1;
                self.sink.observe(BenchmarkEvent::Cancelled {
                    route: sequence.route,
                    request,
                    ticks: self.tick.saturating_sub(submitted_tick),
                });
                self.log("cancelled");
            }
            BackendEvent::Failed {
                sequence,
                request,
                submitted_tick,
                failure,
                turn,
            } => {
                if !self.routes[index].state.has_request(request) {
                    self.metrics.stale_backend_events =
                        self.metrics.stale_backend_events.saturating_add(1);
                    self.log("discarded duplicate backend failure");
                    return;
                }
                let part = self.routes[index]
                    .stream_parts
                    .iter()
                    .find_map(|(candidate, part)| (*candidate == request).then_some(*part))
                    .unwrap_or(0);
                if !self.reply_backend_failure(sequence, request, failure, turn, part) {
                    self.disconnect();
                    return;
                }
                if self.routes[index].state.close_request(request).is_err() {
                    self.disconnect();
                    return;
                }
                self.routes[index]
                    .stream_parts
                    .retain(|(candidate, _)| *candidate != request);
                self.metrics.failed = self.metrics.failed.saturating_add(1);
                for intent in &mut self.frontend_intents {
                    if intent.state == FrontendIntentState::AgentClaimed(sequence)
                        || (intent.state == FrontendIntentState::BrokerClaimed
                            && intent.broker == Some(sequence))
                    {
                        intent.inference_failed = true;
                    }
                }
                self.sink.observe(BenchmarkEvent::Failed {
                    route: sequence.route,
                    request,
                    ticks: self.tick.saturating_sub(submitted_tick),
                    failure,
                });
                self.log(format!("backend failed {failure:?}"));
            }
        }
    }

    fn reply_backend_failure(
        &mut self,
        sequence: SequenceIdentity,
        request: RequestId,
        failure: BackendFailure,
        turn: u64,
        part: u16,
    ) -> bool {
        let code = match failure {
            BackendFailure::TimedOut => RefusalCode::TimedOut,
            BackendFailure::HostDisconnected | BackendFailure::BackendRestarted => {
                RefusalCode::HostDisconnected
            }
            BackendFailure::MalformedResponse => RefusalCode::InvalidEncoding,
            BackendFailure::ModelFailed => RefusalCode::ModelFailed,
            BackendFailure::OutputLimit => RefusalCode::TooLarge,
            BackendFailure::ContextLimit => RefusalCode::BudgetExceeded,
        };
        let refusal = RefusalView {
            for_family: FAMILY_LINK,
            for_kind: INFER_REQUEST,
            code,
            retry: RetryClass::NewRequest,
            retry_after_ms: 0,
            safe_detail: &[],
        };
        let mut payload = [0; REFUSAL_HEADER_LEN];
        let len = refusal
            .encode(&mut payload)
            .expect("fixed refusal header is large enough");
        if matches!(sequence.role, BackendRole::Broker | BackendRole::Agent) {
            let mut event = [0; INFERENCE_EVENT_HEADER_LEN + REFUSAL_HEADER_LEN];
            let Some(length) = (InferenceEventView {
                kind: InferenceEventKind::TurnFailed,
                terminal: true,
                part,
                prompt_tokens: 0,
                completion_tokens: 0,
                turn,
                payload: &payload[..len],
            })
            .encode(&mut event) else {
                return false;
            };
            return self.reply_routed_with_priority(
                SocketPlane::Control,
                INFER_REPLY,
                sequence.route,
                request,
                &event[..length],
                true,
            );
        }
        self.reply_routed_with_priority(
            SocketPlane::Control,
            REFUSED,
            sequence.route,
            request,
            &payload[..len],
            true,
        )
    }
    pub fn advance(&mut self) {
        self.tick = self.tick.saturating_add(1);
        self.poll_backend();
    }

    /// Poll host inference without consuming a guest-visible logical tick.
    ///
    /// The real Kore runtime uses this while QEMU virtual time is stopped for
    /// a slow host model turn. Transport and model progress continue, but the
    /// bounded timeout carried by Kore does not expire while Kore itself
    /// cannot run.
    pub fn poll_backend(&mut self) {
        if self.reconnect_required() {
            // Only advance the bounded flush deadline. No backend completion
            // may append another same-epoch frame after a routed refusal.
            return;
        }
        self.service_frontend_polls();
        if self.epoch.is_none()
            && self.tick >= self.next_hello_retry_tick
            && let Some(nonce) = self.negotiation_nonce
        {
            // Do not stack retries behind a frame the socket has not accepted
            // yet. Once it has, resend the same nonce: a delayed HelloAck then
            // still derives the one pending epoch.
            if self.control_out.frames.is_empty() {
                self.enqueue_pending_hello(nonce);
                self.log("hello retried");
            }
            self.next_hello_retry_tick = self.tick.saturating_add(HELLO_RETRY_TICKS);
        }
        for _ in 0..FRAMES_PER_TICK {
            let Some(event) = self.backend.poll(self.tick) else {
                break;
            };
            self.finish(event);
        }
    }
    pub fn disconnect(&mut self) {
        self.metrics.disconnects += 1;
        self.sink
            .observe(BenchmarkEvent::Disconnected { tick: self.tick });
        self.reset_session(true);
        self.log("disconnected");
    }
    fn reset_session(&mut self, clear_output: bool) {
        self.frontend_updates.disconnect();
        self.backend.reset();
        self.bulk.reset();
        self.completed_bulk.clear();
        self.outbound_bulk.clear();
        self.routes.clear();
        self.frontend_polls.clear();
        self.frontend_receipts.clear();
        for completion in &mut self.frontend_completions {
            if !completion.ready {
                completion.connection_lost = true;
                completion.broker = None;
            }
        }
        for intent in &mut self.frontend_intents {
            if intent.state != FrontendIntentState::Queued {
                intent.state = FrontendIntentState::Disconnected;
            }
            intent.broker = None;
        }
        self.epoch = None;
        self.negotiation_nonce = None;
        self.next_hello_retry_tick = 0;
        self.reconnect_after_control_flush = false;
        self.reconnect_deadline_tick = 0;
        if clear_output {
            self.control_out.clear();
            self.bulk_out.clear();
        }
    }
}

fn outbound_bulk_payloads(
    bulk_id: BulkId,
    content_kind: u16,
    payload: &[u8],
) -> Option<Vec<(u16, Vec<u8>)>> {
    if payload.is_empty() || payload.len() > crate::bulk::BULK_DATA_MAX || bulk_id.is_zero() {
        return None;
    }
    let total_len = u32::try_from(payload.len()).ok()?;
    let chunks = payload
        .len()
        .div_ceil(kore_hostlink::FOUNDED_BULK_WIRE_CHUNK);
    let chunk_count = u16::try_from(chunks).ok()?;
    if chunk_count == 0 || chunk_count > kore_hostlink::BULK_CHUNKS_MAX {
        return None;
    }
    let digest = *blake3::hash(payload).as_bytes();
    let reference = BulkRefV1 {
        bulk_id,
        total_len: U32::new(total_len),
        chunk_count: U16::new(chunk_count),
        content_kind: U16::new(content_kind),
        digest_blake3: digest,
    };
    let mut frames = Vec::with_capacity(chunks + 2);
    frames.push((BULK_BEGIN, reference.as_bytes().to_vec()));
    for (index, chunk) in payload
        .chunks(kore_hostlink::FOUNDED_BULK_WIRE_CHUNK)
        .enumerate()
    {
        let offset =
            u32::try_from(index.checked_mul(kore_hostlink::FOUNDED_BULK_WIRE_CHUNK)?).ok()?;
        let header = BulkChunkHeaderV1 {
            bulk_id,
            offset: U32::new(offset),
            digest_blake3: *blake3::hash(chunk).as_bytes(),
        };
        let mut framed = Vec::with_capacity(header.as_bytes().len() + chunk.len());
        framed.extend_from_slice(header.as_bytes());
        framed.extend_from_slice(chunk);
        frames.push((BULK_CHUNK, framed));
    }
    let finish = BulkFinishV1 {
        bulk_id,
        final_total: U32::new(total_len),
        digest_blake3: digest,
    };
    frames.push((BULK_FINISH, finish.as_bytes().to_vec()));
    Some(frames)
}

fn admission_refusal(error: AdmitError) -> (RefusalCode, RetryClass) {
    match error {
        AdmitError::Route(RouteError::EpochStale) => {
            (RefusalCode::EpochStale, RetryClass::AfterReconnect)
        }
        AdmitError::Route(RouteError::RouteUnknown) => {
            (RefusalCode::RouteUnknown, RetryClass::NewRequest)
        }
        AdmitError::Route(RouteError::GenerationStale) => {
            (RefusalCode::GenerationStale, RetryClass::NewRequest)
        }
        AdmitError::Route(RouteError::RouteClosed) => {
            (RefusalCode::RouteClosed, RetryClass::NewRequest)
        }
        AdmitError::Route(RouteError::LinkControlOnRoute) => {
            (RefusalCode::Malformed, RetryClass::Never)
        }
        AdmitError::Sequence(SequenceDisposition::Duplicate) => {
            (RefusalCode::Duplicate, RetryClass::SameEpoch)
        }
        AdmitError::Sequence(SequenceDisposition::OutOfOrder) => {
            (RefusalCode::OutOfOrder, RetryClass::SameEpoch)
        }
        AdmitError::Sequence(SequenceDisposition::Exhausted) => {
            (RefusalCode::CapacityExhausted, RetryClass::NewRequest)
        }
        AdmitError::Sequence(SequenceDisposition::Next) => {
            (RefusalCode::Internal, RetryClass::Never)
        }
    }
}

fn bulk_refusal(error: BulkError) -> (RefusalCode, RetryClass) {
    match error {
        BulkError::InvalidLength => (RefusalCode::InvalidLength, RetryClass::Never),
        BulkError::InvalidShape | BulkError::RequestMismatch | BulkError::Digest => {
            (RefusalCode::InvalidEncoding, RetryClass::Never)
        }
        BulkError::TooLarge => (RefusalCode::TooLarge, RetryClass::NewRequest),
        BulkError::TooMany => (RefusalCode::TooMany, RetryClass::SameEpoch),
        BulkError::ArithmeticOverflow => (RefusalCode::ArithmeticOverflow, RetryClass::Never),
        BulkError::Duplicate => (RefusalCode::Duplicate, RetryClass::SameEpoch),
        BulkError::OutOfOrder => (RefusalCode::OutOfOrder, RetryClass::SameEpoch),
        BulkError::Unknown => (RefusalCode::SelectorUnknown, RetryClass::Never),
        BulkError::Backpressure => (RefusalCode::Backpressure, RetryClass::SameEpoch),
    }
}

fn backend_refusal(error: BackendError) -> (RefusalCode, RetryClass) {
    match error {
        BackendError::InvalidLimits | BackendError::InvalidIdentity => {
            (RefusalCode::InvalidEncoding, RetryClass::Never)
        }
        BackendError::InvalidRole | BackendError::RoleMismatch => {
            (RefusalCode::PolicyDenied, RetryClass::NewRequest)
        }
        BackendError::SequenceCapacity | BackendError::RoleCapacity | BackendError::Full => {
            (RefusalCode::Backpressure, RetryClass::SameEpoch)
        }
        BackendError::RouteOccupied | BackendError::Duplicate => {
            (RefusalCode::Duplicate, RetryClass::NewRequest)
        }
        BackendError::OutOfOrder => (RefusalCode::OutOfOrder, RetryClass::NewRequest),
        BackendError::RouteUnknown => (RefusalCode::RouteUnknown, RetryClass::NewRequest),
        BackendError::GenerationStale => (RefusalCode::GenerationStale, RetryClass::NewRequest),
        BackendError::SequenceClosed => (RefusalCode::RouteClosed, RetryClass::NewRequest),
        BackendError::RequestUnknown => (RefusalCode::SelectorUnknown, RetryClass::Never),
        BackendError::InputTooLarge | BackendError::OutputLimit => {
            (RefusalCode::TooLarge, RetryClass::NewRequest)
        }
        BackendError::InvalidEncoding => (RefusalCode::InvalidEncoding, RetryClass::Never),
        BackendError::ContextLimit | BackendError::TokenLimit | BackendError::TimeoutLimit => {
            (RefusalCode::BudgetExceeded, RetryClass::NewRequest)
        }
        BackendError::BackendUnavailable => {
            (RefusalCode::HostDisconnected, RetryClass::AfterReconnect)
        }
    }
}

enum Decoder {
    Control(ControlFrameDecoder, Box<[u8]>),
    Bulk(BulkFrameDecoder, Box<[u8]>),
}
type DecodedFrame = (EnvelopeV1, Vec<u8>);
type DecodeResult = Result<(usize, Option<DecodedFrame>), kore_hostlink::DecodeError>;
impl Decoder {
    fn new(plane: SocketPlane) -> Self {
        match plane {
            SocketPlane::Control => Self::Control(
                ControlFrameDecoder::new(),
                vec![0; 64 * 1024].into_boxed_slice(),
            ),
            SocketPlane::Bulk => Self::Bulk(
                BulkFrameDecoder::new(),
                vec![0; 1024 * 1024].into_boxed_slice(),
            ),
        }
    }
    fn push(&mut self, input: &[u8]) -> DecodeResult {
        fn decode<const N: usize>(
            d: &mut kore_hostlink::FramedDecoder<N>,
            s: &mut [u8],
            input: &[u8],
        ) -> DecodeResult {
            let p = d.push(input, s)?;
            if !p.complete {
                return Ok((p.consumed, None));
            }
            let env = *d.envelope().unwrap();
            let payload = d.payload(s).unwrap().to_vec();
            d.reset();
            Ok((p.consumed, Some((env, payload))))
        }
        match self {
            Self::Control(d, s) => decode(d, s, input),
            Self::Bulk(d, s) => decode(d, s, input),
        }
    }
}

enum ConnectionStream {
    #[cfg(unix)]
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl ConnectionStream {
    fn set_nonblocking(&self, enabled: bool) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.set_nonblocking(enabled),
            Self::Tcp(stream) => stream.set_nonblocking(enabled),
        }
    }
}

impl Read for ConnectionStream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.read(bytes),
            Self::Tcp(stream) => stream.read(bytes),
        }
    }
}

impl Write for ConnectionStream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.write(bytes),
            Self::Tcp(stream) => stream.write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.flush(),
            Self::Tcp(stream) => stream.flush(),
        }
    }
}

struct Connection {
    stream: ConnectionStream,
    decoder: Decoder,
}

enum BoundListener {
    #[cfg(unix)]
    Unix {
        listener: UnixListener,
        path: PathBuf,
        dev: u64,
        ino: u64,
    },
    Tcp(TcpListener),
}
impl Drop for BoundListener {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Self::Unix { path, dev, ino, .. } = self
            && let Ok(metadata) = fs::symlink_metadata(&*path)
            && metadata.dev() == *dev
            && metadata.ino() == *ino
        {
            let _ = fs::remove_file(&*path);
        }
    }
}

pub struct HostLinkDaemon<B, S = ()> {
    core: LinkCore<B, S>,
    control_listener: BoundListener,
    bulk_listener: BoundListener,
    control: Option<Connection>,
    bulk: Option<Connection>,
    control_write_chunk: usize,
    bulk_write_chunk: usize,
}
impl<B: InferenceBackend, S: BenchmarkSink> HostLinkDaemon<B, S> {
    #[cfg(unix)]
    pub fn bind(config: Config, backend: B, sink: S) -> Result<Self, DaemonError> {
        Self::bind_with_write_chunks(config, backend, sink, usize::MAX, usize::MAX)
    }
    #[cfg(unix)]
    pub fn bind_with_write_chunks(
        config: Config,
        backend: B,
        sink: S,
        control_write_chunk: usize,
        bulk_write_chunk: usize,
    ) -> Result<Self, DaemonError> {
        let control_listener = bind_listener(&config.control_socket)?;
        let bulk_listener = bind_listener(&config.bulk_socket)?;
        Ok(Self {
            core: LinkCore::new(backend, sink),
            control_listener,
            bulk_listener,
            control: None,
            bulk: None,
            control_write_chunk: control_write_chunk.max(1),
            bulk_write_chunk: bulk_write_chunk.max(1),
        })
    }

    pub fn bind_tcp(config: TcpConfig, backend: B, sink: S) -> Result<Self, DaemonError> {
        if !config.control.ip().is_loopback() || config.control.port() == 0 {
            return Err(DaemonError::UnsafeTcpAddress(config.control));
        }
        if !config.bulk.ip().is_loopback()
            || config.bulk.port() == 0
            || config.bulk == config.control
        {
            return Err(DaemonError::UnsafeTcpAddress(config.bulk));
        }
        let control_listener = bind_tcp_listener(config.control)?;
        let bulk_listener = bind_tcp_listener(config.bulk)?;
        Ok(Self {
            core: LinkCore::new(backend, sink),
            control_listener,
            bulk_listener,
            control: None,
            bulk: None,
            control_write_chunk: usize::MAX,
            bulk_write_chunk: usize::MAX,
        })
    }
    pub fn core(&self) -> &LinkCore<B, S> {
        &self.core
    }
    pub fn core_mut(&mut self) -> &mut LinkCore<B, S> {
        &mut self.core
    }
    pub fn tick(&mut self) -> Result<(), DaemonError> {
        self.service(true)
    }

    fn service(&mut self, advance_clock: bool) -> Result<(), DaemonError> {
        let control_new = accept(
            &self.control_listener,
            &mut self.control,
            SocketPlane::Control,
        )?;
        let bulk_new = accept(&self.bulk_listener, &mut self.bulk, SocketPlane::Bulk)?;
        if (control_new || bulk_new)
            && self.control.is_some()
            && self.bulk.is_some()
            && self.core.epoch.is_none()
            && self.core.negotiation_nonce.is_none()
        {
            self.core.begin_negotiation();
        }
        let mut disconnected = false;
        disconnected |= service_read(&mut self.control, SocketPlane::Control, &mut self.core)?;
        if self.core.reconnect_deadline_tick == 0 {
            disconnected |= service_read(&mut self.bulk, SocketPlane::Bulk, &mut self.core)?;
        }
        if advance_clock {
            self.core.advance();
        } else {
            self.core.poll_backend();
        }
        if !self.core.reconnect_required() || self.core.reconnect_requires_control_flush() {
            disconnected |= service_write(
                &mut self.control,
                &mut self.core.control_out,
                &mut self.core.metrics,
                self.control_write_chunk,
            )?;
            if !self.core.reconnect_required() {
                disconnected |= service_write(
                    &mut self.bulk,
                    &mut self.core.bulk_out,
                    &mut self.core.metrics,
                    self.bulk_write_chunk,
                )?;
            }
        }
        disconnected |= self.core.reconnect_is_ready_with_pending_control(false);
        if disconnected {
            self.control = None;
            self.bulk = None;
            self.core.disconnect();
        }
        Ok(())
    }

    /// Service HostLink and host inference while leaving Kore's logical tick
    /// frozen. This is valid only while the paired VM is suspended.
    pub fn tick_frozen(&mut self) -> Result<(), DaemonError> {
        self.service(false)
    }
    /// Deliberately sever both transport planes for the measurement fault
    /// phase. QEMU's reconnecting chardevs must establish a fresh epoch.
    pub fn force_disconnect(&mut self) {
        if self.control.is_none()
            && self.bulk.is_none()
            && self.core.epoch.is_none()
            && self.core.negotiation_nonce.is_none()
        {
            return;
        }
        self.control = None;
        self.bulk = None;
        self.core.disconnect();
    }
    pub fn write_transcript(&self, path: &Path) -> io::Result<()> {
        let mut f = File::create(path)?;
        for line in self.core.transcript() {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }

    pub fn take_inference_observation(&mut self) -> Option<crate::backend::InferenceObservation> {
        let observation = self.core.backend.take_observation()?;
        self.core.record_inference_observation(&observation);
        Some(observation)
    }

    pub fn take_authority_observation(&mut self) -> Option<AuthorityObservation> {
        let observation = self.core.pop_authority_observation()?;
        self.core.record_authority_observation(&observation);
        Some(observation)
    }

    pub fn bind_dynamic_tcp(backend: B, sink: S) -> Result<(Self, TcpConfig), DaemonError> {
        let control = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let bulk = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        control.set_nonblocking(true)?;
        bulk.set_nonblocking(true)?;
        let config = TcpConfig {
            control: control.local_addr()?,
            bulk: bulk.local_addr()?,
        };
        Ok((
            Self {
                core: LinkCore::new(backend, sink),
                control_listener: BoundListener::Tcp(control),
                bulk_listener: BoundListener::Tcp(bulk),
                control: None,
                bulk: None,
                control_write_chunk: usize::MAX,
                bulk_write_chunk: usize::MAX,
            },
            config,
        ))
    }

    pub fn queue_frontend_intent(
        &mut self,
        text: impl Into<String>,
    ) -> Result<FrontendIntentId, FrontendQueueError> {
        self.core.queue_frontend_intent(text)
    }

    pub fn set_conversation_context(
        &mut self,
        context: &[(bool, String)],
    ) -> Result<(), BackendError> {
        self.core.backend.set_conversation_context(context)
    }

    pub fn take_frontend_result(&mut self) -> Option<FrontendResultOwned> {
        self.core.pop_frontend_result()
    }

    pub fn queue_frontend_update(
        &mut self,
        intent_id: FrontendIntentId,
        text: impl Into<String>,
    ) -> Result<u32, FrontendQueueError> {
        self.core.queue_frontend_update(intent_id, text)
    }

    pub fn take_frontend_update_result(&mut self) -> Option<FrontendUpdateResultOwned> {
        self.core.pop_frontend_update_result()
    }

    pub fn acknowledge_frontend_result(&mut self, intent_id: FrontendIntentId) -> bool {
        self.core.acknowledge_frontend_result(intent_id)
    }

    pub fn cancel_frontend_intent(&mut self, intent_id: FrontendIntentId) -> bool {
        self.core.cancel_frontend_intent(intent_id)
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(unix)]
fn bind_listener(path: &Path) -> Result<BoundListener, DaemonError> {
    if path.as_os_str().is_empty() {
        return Err(DaemonError::InvalidSocketPath(path.into()));
    }
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(DaemonError::InvalidSocketPath(path.into()));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(DaemonError::SocketInUse(path.into()));
        }
        fs::remove_file(path)?;
    }
    let listener = UnixListener::bind(path)?;
    listener.set_nonblocking(true)?;
    let meta = fs::symlink_metadata(path)?;
    Ok(BoundListener::Unix {
        listener,
        path: path.into(),
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

fn bind_tcp_listener(address: SocketAddr) -> io::Result<BoundListener> {
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    Ok(BoundListener::Tcp(listener))
}

fn accept(
    listener: &BoundListener,
    slot: &mut Option<Connection>,
    plane: SocketPlane,
) -> io::Result<bool> {
    let mut installed = false;
    for _ in 0..ACCEPTS_PER_TICK {
        let accepted = match listener {
            #[cfg(unix)]
            BoundListener::Unix { listener, .. } => listener
                .accept()
                .map(|(stream, _)| ConnectionStream::Unix(stream)),
            BoundListener::Tcp(listener) => listener.accept().and_then(|(stream, _)| {
                // HostLink's epoch, route, cancellation, and streaming
                // records are intentionally small control frames. Do not let
                // Nagle delay a route-open acknowledgement past the guest's
                // bounded activation window.
                stream.set_nodelay(true)?;
                Ok(ConnectionStream::Tcp(stream))
            }),
        };
        match accepted {
            Ok(stream) => {
                stream.set_nonblocking(true)?;
                if slot.is_none() {
                    *slot = Some(Connection {
                        stream,
                        decoder: Decoder::new(plane),
                    });
                    installed = true;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }
    Ok(installed)
}
fn service_read<B: InferenceBackend, S: BenchmarkSink>(
    slot: &mut Option<Connection>,
    plane: SocketPlane,
    core: &mut LinkCore<B, S>,
) -> io::Result<bool> {
    let Some(c) = slot else { return Ok(false) };
    let mut buf = [0u8; READ_SIZE];
    match c.stream.read(&mut buf) {
        Ok(0) => return Ok(true),
        Ok(n) => {
            core.metrics.io_read_calls = core.metrics.io_read_calls.saturating_add(1);
            let mut at = 0;
            let mut frames = 0;
            while at < n && frames < FRAMES_PER_TICK {
                match c.decoder.push(&buf[at..n]) {
                    Ok((used, frame)) => {
                        if used == 0 {
                            return Ok(true);
                        }
                        at += used;
                        if let Some((e, p)) = frame {
                            core.receive(plane, e, &p);
                            frames += 1;
                            if core.reconnect_deadline_tick != 0 {
                                // Discard any coalesced suffix with the epoch
                                // that is about to be retired. Processing it
                                // could admit traffic after the sequence gap.
                                return Ok(false);
                            }
                        }
                    }
                    Err(_) => {
                        core.metrics.malformed += 1;
                        return Ok(true);
                    }
                }
            }
            if at < n {
                return Ok(true);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
            ) =>
        {
            return Ok(true);
        }
        Err(e) => return Err(e),
    }
    Ok(false)
}
fn service_write(
    slot: &mut Option<Connection>,
    queue: &mut BoundedQueue,
    metrics: &mut Metrics,
    maximum_write: usize,
) -> io::Result<bool> {
    let Some(c) = slot else { return Ok(false) };
    while let Some(bytes) = queue.front_remaining() {
        let offered = bytes.len().min(maximum_write);
        match c.stream.write(&bytes[..offered]) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                metrics.io_write_calls = metrics.io_write_calls.saturating_add(1);
                if n < bytes.len() {
                    metrics.partial_write_calls = metrics.partial_write_calls.saturating_add(1);
                }
                queue.consume(n);
                metrics.bytes_out += n as u64;
                if queue.offset == 0 {
                    metrics.frames_out += 1;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(true);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

fn fresh_nonce(counter: u64, tick: u64) -> [u8; 32] {
    let mut nonce = [0u8; 32];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut nonce))
        .is_ok()
    {
        return nonce;
    }
    nonce[..8].copy_from_slice(&counter.to_le_bytes());
    nonce[8..16].copy_from_slice(&tick.to_le_bytes());
    nonce[16..20].copy_from_slice(&std::process::id().to_le_bytes());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_le_bytes();
    nonce[20..].copy_from_slice(&nanos[..12]);
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontend_request(kind: InferenceRequestKind, turn: u64, inner: &[u8]) -> Vec<u8> {
        let mut output = [0; INLINE_MAX];
        let length = kore_hostlink::InferenceRequestView {
            kind,
            turn,
            max_output_tokens: 1,
            timeout_ticks: 1_000,
            input: inner,
        }
        .encode(&mut output)
        .unwrap();
        output[..length].to_vec()
    }

    fn install_frontend_route(
        core: &mut LinkCore<crate::backend::DeterministicBackend>,
        epoch: LinkEpoch,
        route: RouteId,
        generation: TaskGeneration,
        role: RouteRole,
        backend_role: BackendRole,
    ) {
        core.routes.push(Route {
            state: RouteState::new(
                epoch,
                route,
                generation,
                role,
                16,
                HOST_BULK_BYTE_CREDITS_PER_ROUTE,
                HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
            )
            .unwrap(),
            identity: SequenceIdentity {
                route,
                generation,
                role: backend_role,
            },
            stream_parts: Vec::new(),
            completed_requests: VecDeque::new(),
        });
    }

    #[test]
    fn frozen_backend_poll_does_not_consume_kore_timeout_ticks() {
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.advance();
        let running_tick = core.tick;
        for _ in 0..100_000 {
            core.poll_backend();
        }
        assert_eq!(core.tick, running_tick);
        core.advance();
        assert_eq!(core.tick, running_tick + 1);
    }

    #[test]
    fn repeated_frontend_text_gets_distinct_opaque_ids() {
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        let first = core.queue_frontend_intent("read X").unwrap();
        let second = core.queue_frontend_intent("read X").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn idle_broker_poll_records_the_ready_for_next_chat_barrier() {
        let epoch = LinkEpoch([41; 16]);
        let broker_route = RouteId([42; 16]);
        let broker_generation = TaskGeneration::new(2).unwrap();
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.epoch = Some(epoch);
        install_frontend_route(
            &mut core,
            epoch,
            broker_route,
            broker_generation,
            RouteRole::Broker,
            BackendRole::Broker,
        );

        let mut inner = [0; INFERENCE_PART_MAX];
        let length = FrontendPoll {
            phase: FrontendPollPhase::BrokerNext,
            intent_id: FrontendIntentId::ZERO,
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendPoll, 1, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                broker_route,
                broker_generation.get(),
                0,
                RequestId([43; 16]),
                payload.len(),
            ),
            &payload,
        );

        assert_eq!(core.frontend_polls.len(), 1);
        assert!(
            core.transcript()
                .any(|entry| entry.ends_with("frontend broker poll parked"))
        );
    }

    #[test]
    fn only_the_claiming_broker_can_report_worker_failure_once() {
        for (claiming, kind, accepted) in [
            (true, FrontendResultKind::Failed, true),
            (false, FrontendResultKind::Failed, false),
            (true, FrontendResultKind::Completed, false),
        ] {
            let epoch = LinkEpoch([61; 16]);
            let route = RouteId([62; 16]);
            let generation = TaskGeneration::new(2).unwrap();
            let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
            core.epoch = Some(epoch);
            install_frontend_route(
                &mut core,
                epoch,
                route,
                generation,
                RouteRole::Broker,
                BackendRole::Broker,
            );
            let intent_id = core.queue_frontend_intent("hello").unwrap();
            let intent = &mut core.frontend_intents[0];
            intent.state = FrontendIntentState::BrokerClaimed;
            intent.broker = Some(SequenceIdentity {
                route: if claiming { route } else { RouteId([63; 16]) },
                generation,
                role: BackendRole::Broker,
            });
            let mut inner = [0; INFERENCE_PART_MAX];
            let length = kore_hostlink::FrontendResult {
                intent_id,
                kind,
                text: "Agent failed before responding: RunnerExited(41)",
            }
            .encode(&mut inner)
            .unwrap();
            let payload =
                frontend_request(InferenceRequestKind::FrontendResult, 1, &inner[..length]);
            core.control_out.clear();
            core.receive(
                SocketPlane::Control,
                envelope(
                    INFER_REQUEST,
                    epoch,
                    route,
                    generation.get(),
                    0,
                    RequestId([64; 16]),
                    payload.len(),
                ),
                &payload,
            );
            assert_eq!(core.frontend_results.len(), usize::from(accepted));
            if accepted {
                assert!(core.control_out.frames.is_empty());
                core.pop_frontend_result().unwrap();
                // A duplicate failure cannot produce a second final response.
                core.receive(
                    SocketPlane::Control,
                    envelope(
                        INFER_REQUEST,
                        epoch,
                        route,
                        generation.get(),
                        1,
                        RequestId([65; 16]),
                        payload.len(),
                    ),
                    &payload,
                );
                assert!(core.frontend_results.is_empty());
                assert_eq!(core.frontend_receipts.len(), 1);
                assert!(core.acknowledge_frontend_result(intent_id));
                assert!(!core.acknowledge_frontend_result(intent_id));
            }
        }
    }

    #[test]
    fn frontend_updates_deliver_on_exact_broker_and_ack_separately_from_prompt_results() {
        use kore_hostlink::{FrontendUpdateOutcome, FrontendUpdateResult};
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        let queued = core.queue_frontend_intent("unclaimed").unwrap();
        assert_eq!(
            core.queue_frontend_update(queued, "change authority"),
            Err(FrontendQueueError::InvalidIntent)
        );
        let revision = core.queue_frontend_update(intent, "Revoke input.").unwrap();
        assert!(core.frontend_requires_progress());
        assert_eq!(
            core.queue_frontend_update(intent, "second update"),
            Err(FrontendQueueError::Duplicate)
        );
        assert!(core.frontend_updates.pending(worker, intent).is_none());
        core.control_out.bytes = QUEUE_BYTES_MAX;
        submit_frontend_lifecycle(
            &mut core,
            broker,
            0,
            FrontendPollPhase::BrokerControl,
            intent,
        );
        assert!(core.frontend_updates.pending(broker, intent).is_some());
        core.control_out.bytes = 0;
        core.service_frontend_polls();
        let frame = core.pop_control_frame().unwrap();
        let reply = decode_inference_event(&frame[ENVELOPE_LEN..]).unwrap();
        let update = kore_hostlink::decode_frontend_update(reply.payload).unwrap();
        assert_eq!(
            (update.intent_id, update.revision, update.text),
            (intent, revision, "Revoke input.")
        );
        assert!(core.frontend_updates.pending(broker, intent).is_none());
        let result = FrontendUpdateResult {
            intent_id: intent,
            revision,
            outcome: FrontendUpdateOutcome::Applied,
            text: "Input revoked.",
        };
        let mut inner = [0; INFERENCE_PART_MAX];
        let len = result.encode(&mut inner).unwrap();
        let payload =
            frontend_request(InferenceRequestKind::FrontendUpdateResult, 1, &inner[..len]);
        // The worker cannot submit testimony on behalf of the owning broker.
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                core.epoch.unwrap(),
                worker.route,
                worker.generation.get(),
                0,
                RequestId([110; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert!(core.pop_frontend_update_result().is_none());
        core.control_out.clear();
        core.control_out.bytes = QUEUE_BYTES_MAX;
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                core.epoch.unwrap(),
                broker.route,
                broker.generation.get(),
                1,
                RequestId([111; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert!(
            core.pop_frontend_update_result().is_none(),
            "backpressure retains result/receipt custody"
        );
        assert!(core.frontend_results.is_empty() && core.frontend_completions.is_empty());
        assert_eq!(core.frontend_intents.len(), 2);
        core.control_out.bytes = 0;
        core.service_frontend_polls();
        let frame = core.pop_control_frame().unwrap();
        assert!(
            decode_inference_event(&frame[ENVELOPE_LEN..])
                .unwrap()
                .payload
                .is_empty()
        );
        let actual = core.pop_frontend_update_result().unwrap();
        assert_eq!(
            (
                actual.intent_id,
                actual.revision,
                actual.outcome,
                actual.text.as_str()
            ),
            (
                intent,
                revision,
                FrontendUpdateOutcome::Applied,
                "Input revoked."
            )
        );
        assert!(core.pop_frontend_update_result().is_none());
        let next = core
            .queue_frontend_update(intent, "Grant input again.")
            .unwrap();
        assert!(next > revision);
        assert!(!core.frontend_updates.accepts(broker, result));
        assert!(core.frontend_results.is_empty() && core.frontend_completions.is_empty());
    }

    #[test]
    fn frontend_updates_wait_for_existing_planning_and_hold_new_claims_until_receipt() {
        use kore_hostlink::{FrontendUpdateOutcome, FrontendUpdateResult};
        let (mut core, intent, broker, _) = frontend_race_fixture();
        let second = core.queue_frontend_intent("already claimed").unwrap();
        core.frontend_intents[1].state = FrontendIntentState::BrokerClaimed;
        core.frontend_intents[1].broker = Some(broker);
        let third = core.queue_frontend_intent("wait for update").unwrap();
        let revision = core.queue_frontend_update(intent, "Revoke input.").unwrap();
        submit_frontend_lifecycle(
            &mut core,
            broker,
            0,
            FrontendPollPhase::BrokerControl,
            intent,
        );
        let frame = core.pop_control_frame().unwrap();
        assert_eq!(
            kore_hostlink::FrontendControl::decode(
                decode_inference_event(&frame[ENVELOPE_LEN..])
                    .unwrap()
                    .payload
            ),
            Some(kore_hostlink::FrontendControl::Active)
        );
        assert!(core.frontend_updates.pending(broker, intent).is_some());
        let other = SequenceIdentity {
            route: RouteId([74; 16]),
            generation: TaskGeneration::new(3).unwrap(),
            role: BackendRole::Agent,
        };
        install_frontend_route(
            &mut core,
            LinkEpoch([71; 16]),
            other.route,
            other.generation,
            RouteRole::Agent,
            BackendRole::Agent,
        );
        core.frontend_intents[1].state = FrontendIntentState::AgentClaimed(other);
        submit_frontend_lifecycle(
            &mut core,
            broker,
            1,
            FrontendPollPhase::BrokerNext,
            FrontendIntentId::ZERO,
        );
        assert!(core.control_out.frames.is_empty());
        assert_eq!(core.frontend_intents[2].state, FrontendIntentState::Queued);
        submit_frontend_lifecycle(
            &mut core,
            broker,
            2,
            FrontendPollPhase::BrokerControl,
            intent,
        );
        let frame = core.pop_control_frame().unwrap();
        assert!(
            kore_hostlink::decode_frontend_update(
                decode_inference_event(&frame[ENVELOPE_LEN..])
                    .unwrap()
                    .payload
            )
            .is_some()
        );
        let mut inner = [0; INFERENCE_PART_MAX];
        let len = FrontendUpdateResult {
            intent_id: intent,
            revision,
            outcome: FrontendUpdateOutcome::Applied,
            text: "done",
        }
        .encode(&mut inner)
        .unwrap();
        let payload =
            frontend_request(InferenceRequestKind::FrontendUpdateResult, 1, &inner[..len]);
        core.control_out.bytes = QUEUE_BYTES_MAX;
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                core.epoch.unwrap(),
                broker.route,
                broker.generation.get(),
                3,
                RequestId([119; 16]),
                payload.len(),
            ),
            &payload,
        );
        core.service_frontend_polls();
        assert_eq!(core.frontend_intents[2].state, FrontendIntentState::Queued);
        core.control_out.bytes = 0;
        core.service_frontend_polls();
        assert_eq!(
            core.frontend_intents
                .iter()
                .find(|row| row.intent_id == third)
                .unwrap()
                .state,
            FrontendIntentState::BrokerClaimed
        );
        assert_eq!(
            core.frontend_intents
                .iter()
                .find(|row| row.intent_id == second)
                .unwrap()
                .state,
            FrontendIntentState::AgentClaimed(other)
        );
        assert_eq!(
            core.pop_frontend_update_result().unwrap().outcome,
            FrontendUpdateOutcome::Applied
        );
        assert!(core.frontend_results.is_empty() && core.frontend_completions.is_empty());
    }

    #[test]
    fn authority_update_runtime_bridge_waits_for_exact_receipt_and_preserves_other_results() {
        use crate::{AuthorityUpdateRequest, EventKind, kore_runtime::LiveUpdateBridge};
        use kore_hostlink::{FrontendUpdateOutcome, FrontendUpdateResult};
        let (mut core, intent, broker, _) = frontend_race_fixture();
        let mut bridge = LiveUpdateBridge::new(intent);
        let request = std::cell::RefCell::new(Some(AuthorityUpdateRequest {
            api_version: 1,
            prompt_id: "prompt".into(),
            update_id: "update".into(),
            text: "Revoke input.".into(),
        }));
        let events = std::cell::RefCell::new(Vec::new());
        let mut emit = |kind, public, value| events.borrow_mut().push((kind, public, value));
        bridge.submit(&mut core, &|| request.borrow_mut().take(), &mut emit);
        assert!(bridge.pending());
        bridge.submit(
            &mut core,
            &|| panic!("must not drain another API request"),
            &mut emit,
        );
        submit_frontend_lifecycle(
            &mut core,
            broker,
            0,
            FrontendPollPhase::BrokerControl,
            intent,
        );
        let frame = core.pop_control_frame().unwrap();
        let reply = decode_inference_event(&frame[ENVELOPE_LEN..]).unwrap();
        let update = kore_hostlink::decode_frontend_update(reply.payload).unwrap();
        assert_eq!(update.text, "Revoke input.");
        let revision = update.revision;
        let mut inner = [0; INFERENCE_PART_MAX];
        let len = FrontendUpdateResult {
            intent_id: intent,
            revision,
            outcome: FrontendUpdateOutcome::Applied,
            text: "Input revoked.",
        }
        .encode(&mut inner)
        .unwrap();
        let payload =
            frontend_request(InferenceRequestKind::FrontendUpdateResult, 1, &inner[..len]);
        core.control_out.bytes = QUEUE_BYTES_MAX;
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                core.epoch.unwrap(),
                broker.route,
                broker.generation.get(),
                1,
                RequestId([121; 16]),
                payload.len(),
            ),
            &payload,
        );
        bridge.receive(&mut core, &mut emit);
        assert!(bridge.pending() && events.borrow().is_empty());
        core.control_out.bytes = 0;
        core.service_frontend_polls();
        // Another intent's terminal result must remain in its own custody.
        let other = FrontendIntentId([122; 16]);
        let other_revision = core
            .frontend_updates
            .queue(other, broker, "other".into())
            .unwrap();
        core.frontend_updates.cancel_undelivered(other);
        bridge.receive(&mut core, &mut emit);
        bridge.receive(&mut core, &mut emit);
        assert!(!bridge.pending());
        let events = events.borrow();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, EventKind::AuthorityUpdate);
        assert!(events[0].1);
        assert_eq!(events[0].2["update_id"], "update");
        assert_eq!(events[0].2["revision"], revision);
        assert_eq!(events[0].2["status"], "applied");
        assert!(core.frontend_results.is_empty() && core.frontend_completions.is_empty());
        assert_eq!(
            core.pop_frontend_update_result_for(other, other_revision)
                .unwrap()
                .outcome,
            FrontendUpdateOutcome::Cancelled
        );
    }

    fn frontend_race_fixture() -> (
        LinkCore<crate::backend::DeterministicBackend>,
        FrontendIntentId,
        SequenceIdentity,
        SequenceIdentity,
    ) {
        let epoch = LinkEpoch([71; 16]);
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.epoch = Some(epoch);
        for (route, role, backend_role) in [
            (RouteId([72; 16]), RouteRole::Broker, BackendRole::Broker),
            (RouteId([73; 16]), RouteRole::Agent, BackendRole::Agent),
        ] {
            install_frontend_route(
                &mut core,
                epoch,
                route,
                TaskGeneration::new(3).unwrap(),
                role,
                backend_role,
            );
        }
        let broker = core.routes[0].identity;
        let worker = core.routes[1].identity;
        core.backend.open(broker).unwrap();
        let intent_id = core.queue_frontend_intent("hello").unwrap();
        core.frontend_intents[0].state = FrontendIntentState::AgentClaimed(worker);
        core.frontend_intents[0].broker = Some(broker);
        (core, intent_id, broker, worker)
    }

    fn submit_frontend_race_result(
        core: &mut LinkCore<crate::backend::DeterministicBackend>,
        intent_id: FrontendIntentId,
        identity: SequenceIdentity,
        sequence: u64,
        request: RequestId,
    ) {
        let mut inner = [0; INFERENCE_PART_MAX];
        let (kind, text) = if identity.role == BackendRole::Broker {
            (FrontendResultKind::Failed, "Agent deadline exceeded.")
        } else {
            (
                FrontendResultKind::Completed,
                "Finished before the deadline.",
            )
        };
        let length = kore_hostlink::FrontendResult {
            intent_id,
            kind,
            text,
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendResult, 1, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                core.epoch.unwrap(),
                identity.route,
                identity.generation.get(),
                sequence,
                request,
                payload.len(),
            ),
            &payload,
        );
    }

    #[test]
    fn frontend_deadline_race_shares_publication_without_replacing_the_answer() {
        for publish_before_deadline in [false, true] {
            let (mut core, intent, broker, worker) = frontend_race_fixture();
            let worker_request = RequestId([74; 16]);
            let broker_request = RequestId([75; 16]);
            submit_frontend_race_result(&mut core, intent, worker, 0, worker_request);
            assert_eq!(
                core.pop_frontend_result().unwrap().kind,
                FrontendResultKind::Completed
            );
            assert!(core.control_out.frames.is_empty());
            if publish_before_deadline {
                assert!(core.acknowledge_frontend_result(intent));
            }
            submit_frontend_race_result(&mut core, intent, broker, 0, broker_request);
            assert!(
                core.frontend_results.is_empty(),
                "a deadline cannot replace the answer"
            );
            if !publish_before_deadline {
                assert!(
                    core.control_out.frames.is_empty(),
                    "both requests wait for publication"
                );
                assert_eq!(core.frontend_receipts.len(), 2);
                core.control_out.bytes = QUEUE_BYTES_MAX;
                assert!(!core.acknowledge_frontend_result(intent));
                assert_eq!(core.frontend_receipts.len(), 2);
                core.control_out.bytes = 0;
                assert!(core.acknowledge_frontend_result(intent));
            }
            assert!(core.frontend_receipts.is_empty());
            let receipts: Vec<_> = core
                .control_out
                .frames
                .iter()
                .map(|frame| {
                    let envelope = EnvelopeV1::read_from_bytes(&frame[..ENVELOPE_LEN]).unwrap();
                    assert_eq!(envelope.kind.get(), INFER_REPLY);
                    let event = decode_inference_event(&frame[ENVELOPE_LEN..]).unwrap();
                    assert!(event.terminal && event.payload.is_empty());
                    assert_eq!(event.kind, InferenceEventKind::TurnFinished);
                    (envelope.route_id, envelope.request_id)
                })
                .collect();
            assert_eq!(
                receipts,
                vec![
                    (worker.route, worker_request),
                    (broker.route, broker_request)
                ]
            );
            assert!(!core.acknowledge_frontend_result(intent));
            // Even a fresh request from the real broker cannot join twice.
            submit_frontend_race_result(&mut core, intent, broker, 1, RequestId([76; 16]));
            assert!(core.frontend_results.is_empty() && core.frontend_receipts.is_empty());
        }
    }

    #[test]
    fn frontend_deadline_winner_rejects_a_late_worker_answer() {
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        submit_frontend_race_result(&mut core, intent, broker, 0, RequestId([74; 16]));
        submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([75; 16]));
        assert_eq!(core.frontend_results.len(), 1);
        assert_eq!(
            core.pop_frontend_result().unwrap().kind,
            FrontendResultKind::Failed
        );
        assert_eq!(core.frontend_receipts.len(), 1);
        assert!(core.acknowledge_frontend_result(intent));
        assert!(core.frontend_receipts.is_empty());
    }

    #[test]
    fn frontend_deadline_join_rejects_other_brokers_and_replacement_generations() {
        for replacement in [false, true] {
            let (mut core, intent, broker, worker) = frontend_race_fixture();
            submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
            let intruder = SequenceIdentity {
                route: if replacement {
                    broker.route
                } else {
                    RouteId([79; 16])
                },
                generation: TaskGeneration::new(4).unwrap(),
                role: BackendRole::Broker,
            };
            if replacement {
                core.routes.remove(0);
            }
            let epoch = core.epoch.unwrap();
            install_frontend_route(
                &mut core,
                epoch,
                intruder.route,
                intruder.generation,
                RouteRole::Broker,
                BackendRole::Broker,
            );
            submit_frontend_race_result(&mut core, intent, intruder, 0, RequestId([75; 16]));
            assert!(!core.frontend_completions[0].broker_joined);
            assert_eq!(core.frontend_receipts.len(), 1);
            assert_eq!(core.frontend_results.len(), 1);
            assert_eq!(
                EnvelopeV1::read_from_bytes(
                    &core.control_out.frames.back().unwrap()[..ENVELOPE_LEN]
                )
                .unwrap()
                .kind
                .get(),
                REFUSED
            );
        }
    }

    #[test]
    fn frontend_completion_survives_concurrent_poll_until_exact_worker_route_closes() {
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
        core.pop_frontend_result().unwrap();
        assert!(core.acknowledge_frontend_result(intent));
        let mut inner = [0; INFERENCE_PART_MAX];
        let length = FrontendPoll {
            phase: FrontendPollPhase::BrokerNext,
            intent_id: FrontendIntentId::ZERO,
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendPoll, 1, &inner[..length]);
        let epoch = core.epoch.unwrap();
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                broker.route,
                broker.generation.get(),
                0,
                RequestId([75; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert_eq!(
            core.frontend_completions.len(),
            1,
            "a concurrent poll is not worker retirement"
        );
        core.receive(
            SocketPlane::Control,
            envelope(
                ROUTE_CLOSE,
                epoch,
                worker.route,
                worker.generation.get(),
                1,
                RequestId([76; 16]),
                0,
            ),
            &[],
        );
        assert!(!core.routes.iter().any(|route| route.identity == worker));
        // A replacement generation does not keep its predecessor's completion
        // alive, and can never receive the predecessor's delayed receipt.
        install_frontend_route(
            &mut core,
            epoch,
            worker.route,
            TaskGeneration::new(4).unwrap(),
            RouteRole::Agent,
            BackendRole::Agent,
        );
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                broker.route,
                broker.generation.get(),
                1,
                RequestId([77; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert_eq!(core.frontend_completions.len(), 1);
        assert!(!core.frontend_ready(intent));
        assert!(!core.finish_frontend_intent(intent));
        assert!(!core.acknowledge_frontend_result(intent));
    }

    #[test]
    fn frontend_worker_route_close_preserves_the_brokers_unpublished_receipt() {
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
        submit_frontend_race_result(&mut core, intent, broker, 0, RequestId([75; 16]));
        let epoch = core.epoch.unwrap();
        core.receive(
            SocketPlane::Control,
            envelope(
                ROUTE_CLOSE,
                epoch,
                worker.route,
                worker.generation.get(),
                1,
                RequestId([76; 16]),
                0,
            ),
            &[],
        );
        assert_eq!(core.frontend_receipts.len(), 1);
        assert_eq!(core.frontend_receipts[0].identity, broker);
        assert!(!core.frontend_completions[0].published);
        core.control_out.clear();
        install_frontend_route(
            &mut core,
            epoch,
            worker.route,
            TaskGeneration::new(4).unwrap(),
            RouteRole::Agent,
            BackendRole::Agent,
        );
        core.pop_frontend_result().unwrap();
        assert!(core.acknowledge_frontend_result(intent));
        assert_eq!(core.control_out.frames.len(), 1);
        let reply =
            EnvelopeV1::read_from_bytes(&core.control_out.frames[0][..ENVELOPE_LEN]).unwrap();
        assert_eq!(reply.route_id, broker.route);
        assert_eq!(reply.kind.get(), INFER_REPLY);
        core.disconnect();
        assert!(core.frontend_receipts.is_empty());
        assert!(core.frontend_interrupted(intent));
        assert!(!core.frontend_ready(intent));
        assert!(!core.acknowledge_frontend_result(intent));
    }

    #[test]
    fn frontend_disconnect_never_replays_claimed_work_on_a_new_epoch() {
        for broker_claimed in [true, false] {
            let (mut core, first, broker, _) = frontend_race_fixture();
            if broker_claimed {
                core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
            }
            // The second iteration exercises the fixture's AgentClaimed state.
            let queued = core.queue_frontend_intent("not delivered yet").unwrap();
            core.disconnect();
            core.disconnect(); // repeated reset cannot erase the uncertainty
            assert!(core.frontend_interrupted(first));
            assert!(!core.frontend_interrupted(queued));
            assert!(core.frontend_results.is_empty());
            assert!(!core.frontend_ready(first));
            let epoch = LinkEpoch([81; 16]);
            core.epoch = Some(epoch);
            // Reuse the same route/generation to prove a fresh epoch alone
            // cannot turn the old claim into a new delivery.
            install_frontend_route(
                &mut core,
                epoch,
                broker.route,
                broker.generation,
                RouteRole::Broker,
                BackendRole::Broker,
            );
            core.backend.open(broker).unwrap();
            submit_frontend_lifecycle(
                &mut core,
                broker,
                0,
                FrontendPollPhase::BrokerNext,
                FrontendIntentId::ZERO,
            );
            assert!(core.frontend_interrupted(first));
            let next = core
                .frontend_intents
                .iter()
                .find(|intent| intent.intent_id == queued)
                .unwrap();
            assert_eq!(next.state, FrontendIntentState::BrokerClaimed);
            assert_eq!(
                core.frontend_intents
                    .iter()
                    .filter(|intent| intent.state == FrontendIntentState::BrokerClaimed)
                    .count(),
                1
            );
            assert!(core.cancel_frontend_intent(first));
            assert!(!core.frontend_interrupted(first));
            assert!(
                core.frontend_intents
                    .iter()
                    .any(|intent| intent.intent_id == queued)
            );
        }
    }

    #[test]
    fn frontend_disconnect_preserves_a_result_without_inventing_cleanup() {
        let (mut core, intent, _, worker) = frontend_race_fixture();
        submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
        core.disconnect();
        let result = core.pop_frontend_result().unwrap();
        assert_eq!(result.intent_id, intent);
        assert_eq!(result.kind, FrontendResultKind::Completed);
        assert!(core.frontend_interrupted(intent));
        assert!(!core.frontend_ready(intent));
        assert!(!core.finish_frontend_intent(intent));
        assert!(!core.acknowledge_frontend_result(intent));
        assert!(core.control_out.frames.is_empty());
        core.cancel_frontend_intent(intent);
        assert!(core.frontend_completions.is_empty());
        assert!(!core.frontend_interrupted(intent));
    }

    #[test]
    fn frontend_disconnect_does_not_erase_already_confirmed_readiness() {
        let (mut core, intent, broker, _) = frontend_race_fixture();
        core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
        submit_frontend_race_result(&mut core, intent, broker, 0, RequestId([74; 16]));
        core.pop_frontend_result().unwrap();
        assert!(core.acknowledge_frontend_result(intent));
        submit_frontend_lifecycle(
            &mut core, broker, 1, FrontendPollPhase::BrokerRetired, intent,
        );
        submit_frontend_lifecycle(
            &mut core, broker, 2, FrontendPollPhase::BrokerReadyNext, intent,
        );
        assert!(core.frontend_ready(intent));
        core.disconnect();
        assert!(!core.frontend_interrupted(intent));
        assert!(core.frontend_ready(intent));
        assert!(core.finish_frontend_intent(intent));
        assert!(core.frontend_completions.is_empty());
    }

    #[test]
    fn frontend_refusal_without_worker_retires_and_reuses_bounded_capacity() {
        let (mut core, mut intent, broker, _) = frontend_race_fixture();
        core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
        for cycle in 0..(FRONTEND_RESULTS_MAX as u64 * 3) {
            submit_frontend_race_result(
                &mut core,
                intent,
                broker,
                cycle * 3,
                RequestId([(100 + cycle * 3) as u8; 16]),
            );
            assert_eq!(
                core.pop_frontend_result().unwrap().kind,
                FrontendResultKind::Failed
            );
            assert!(!core.frontend_ready(intent));
            assert!(core.acknowledge_frontend_result(intent));
            submit_frontend_lifecycle(
                &mut core,
                broker,
                cycle * 3 + 1,
                FrontendPollPhase::BrokerRetired,
                intent,
            );
            assert!(core.frontend_completions[0].retired);
            assert!(
                !core.frontend_ready(intent),
                "retirement alone is not broker readiness"
            );
            let next = core.queue_frontend_intent("next prompt").unwrap();
            submit_frontend_lifecycle(
                &mut core,
                broker,
                cycle * 3 + 2,
                FrontendPollPhase::BrokerReadyNext,
                intent,
            );
            assert!(core.frontend_ready(intent));
            assert!(!core.frontend_ready(next));
            assert!(core.finish_frontend_intent(intent));
            assert!(!core.finish_frontend_intent(intent));
            assert!(core.frontend_completions.is_empty() && core.frontend_receipts.is_empty());
            assert!(core.frontend_polls.is_empty());
            assert_eq!(
                core.frontend_intents[0].state,
                FrontendIntentState::BrokerClaimed
            );
            core.control_out.clear();
            intent = next;
        }
    }

    fn submit_frontend_lifecycle(
        core: &mut LinkCore<crate::backend::DeterministicBackend>,
        identity: SequenceIdentity,
        sequence: u64,
        phase: FrontendPollPhase,
        intent_id: FrontendIntentId,
    ) {
        let mut inner = [0; INFERENCE_PART_MAX];
        let length = FrontendPoll { phase, intent_id }
            .encode(&mut inner)
            .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendPoll, 1, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                core.epoch.unwrap(),
                identity.route,
                identity.generation.get(),
                sequence,
                RequestId([(100 + sequence) as u8; 16]),
                payload.len(),
            ),
            &payload,
        );
    }

    #[test]
    fn frontend_concurrent_lanes_serialize_plans_and_overlap_worker_execution() {
        let (mut core, first, broker, worker) = frontend_race_fixture();
        core.frontend_intents[0].state = FrontendIntentState::Queued;
        core.frontend_intents[0].broker = None;
        let second = core.queue_frontend_intent("second worker").unwrap();
        for sequence in 0..2 {
            submit_frontend_lifecycle(
                &mut core,
                broker,
                sequence,
                FrontendPollPhase::BrokerNext,
                FrontendIntentId::ZERO,
            );
        }
        assert_eq!(
            core.frontend_intents[0].state,
            FrontendIntentState::BrokerClaimed
        );
        assert_eq!(core.frontend_intents[1].state, FrontendIntentState::Queued);
        assert_eq!(core.frontend_polls.len(), 1);
        // No planning request has arrived yet: idle backend alone must not
        // admit both claims into the same turn-one broker sequence.
        assert!(core.backend.broker_prompt_ready(broker));
        core.backend.open(worker).unwrap();
        core.backend
            .submit(worker, RequestId([87; 16]), b"running worker", 0)
            .unwrap();
        submit_frontend_lifecycle(&mut core, worker, 0, FrontendPollPhase::AgentClaim, first);
        core.service_frontend_polls();
        assert_eq!(
            core.frontend_intents[0].state,
            FrontendIntentState::AgentClaimed(worker)
        );
        assert_eq!(core.frontend_intents[1].intent_id, second);
        assert_eq!(
            core.frontend_intents[1].state,
            FrontendIntentState::BrokerClaimed
        );
        assert!(core.frontend_polls.is_empty());
        assert_eq!(
            core.backend.metrics().pending,
            1,
            "worker remains active during next plan"
        );
    }

    #[test]
    fn frontend_broker_claim_waits_for_backend_ownership_to_clear() {
        let (mut core, _, broker, _) = frontend_race_fixture();
        let next = core
            .queue_frontend_intent("waiting for broker cleanup")
            .unwrap();
        let request = RequestId([88; 16]);
        core.backend
            .submit(broker, request, b"pending plan", 0)
            .unwrap();
        assert!(!core.backend.broker_prompt_ready(broker));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            0,
            FrontendPollPhase::BrokerNext,
            FrontendIntentId::ZERO,
        );
        assert_eq!(core.frontend_intents[1].state, FrontendIntentState::Queued);
        assert_eq!(core.frontend_polls.len(), 1);
        core.backend.cancel(broker, request, 1).unwrap();
        assert!(core.backend.broker_prompt_ready(broker));
        core.service_frontend_polls();
        assert_eq!(core.frontend_intents[1].intent_id, next);
        assert_eq!(
            core.frontend_intents[1].state,
            FrontendIntentState::BrokerClaimed
        );
        assert!(core.frontend_polls.is_empty());
        core.backend.close(broker).unwrap();
        assert!(!core.backend.broker_prompt_ready(broker));
    }

    #[test]
    fn frontend_lane_readiness_is_not_delayed_by_another_lanes_plan() {
        let (mut core, first, broker, _) = frontend_race_fixture();
        core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
        submit_frontend_race_result(&mut core, first, broker, 0, RequestId([89; 16]));
        core.pop_frontend_result().unwrap();
        assert!(core.acknowledge_frontend_result(first));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            1,
            FrontendPollPhase::BrokerRetired,
            first,
        );
        let second = core.queue_frontend_intent("another lane's plan").unwrap();
        submit_frontend_lifecycle(
            &mut core,
            broker,
            2,
            FrontendPollPhase::BrokerNext,
            FrontendIntentId::ZERO,
        );
        assert_eq!(core.frontend_intents[0].intent_id, second);
        assert_eq!(
            core.frontend_intents[0].state,
            FrontendIntentState::BrokerClaimed
        );
        core.backend
            .submit(broker, RequestId([90; 16]), b"planning", 0)
            .unwrap();
        let third = core.queue_frontend_intent("next work").unwrap();
        submit_frontend_lifecycle(
            &mut core,
            broker,
            3,
            FrontendPollPhase::BrokerReadyNext,
            first,
        );
        assert!(core.frontend_ready(first));
        assert!(core.finish_frontend_intent(first));
        assert_eq!(core.frontend_polls.len(), 1, "only the new claim waits");
        assert_eq!(core.frontend_intents[1].intent_id, third);
        assert_eq!(core.frontend_intents[1].state, FrontendIntentState::Queued);
        assert_eq!(core.backend.metrics().pending, 1);
    }

    #[test]
    fn frontend_concurrent_lane_readiness_only_completes_its_named_intent() {
        let (mut core, first, broker, _) = frontend_race_fixture();
        core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
        let second = core.queue_frontend_intent("second lane").unwrap();
        core.frontend_intents[1].state = FrontendIntentState::BrokerClaimed;
        core.frontend_intents[1].broker = Some(broker);
        for (sequence, intent) in [(0, first), (1, second)] {
            submit_frontend_race_result(
                &mut core,
                intent,
                broker,
                sequence,
                RequestId([(70 + sequence) as u8; 16]),
            );
            assert_eq!(core.pop_frontend_result().unwrap().intent_id, intent);
            assert!(core.acknowledge_frontend_result(intent));
        }
        submit_frontend_lifecycle(
            &mut core,
            broker,
            2,
            FrontendPollPhase::BrokerRetired,
            first,
        );
        submit_frontend_lifecycle(
            &mut core,
            broker,
            3,
            FrontendPollPhase::BrokerRetired,
            second,
        );
        assert!(
            core.frontend_completions
                .iter()
                .all(|completion| completion.retired)
        );
        // A fresh lane's unrelated poll cannot acknowledge either old lane.
        submit_frontend_lifecycle(
            &mut core,
            broker,
            4,
            FrontendPollPhase::BrokerNext,
            FrontendIntentId::ZERO,
        );
        assert!(!core.frontend_ready(first) && !core.frontend_ready(second));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            5,
            FrontendPollPhase::BrokerReadyNext,
            first,
        );
        assert!(core.frontend_ready(first));
        assert!(!core.frontend_ready(second));
        assert!(core.finish_frontend_intent(first));
        assert!(!core.finish_frontend_intent(second));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            6,
            FrontendPollPhase::BrokerReadyNext,
            second,
        );
        assert!(core.finish_frontend_intent(second));
        assert!(core.frontend_completions.is_empty());
    }

    #[test]
    fn frontend_lane_readiness_refuses_unretired_foreign_and_repeated_claims() {
        let (mut core, intent, broker, _) = frontend_race_fixture();
        core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
        submit_frontend_race_result(&mut core, intent, broker, 0, RequestId([79; 16]));
        core.pop_frontend_result().unwrap();
        assert!(core.acknowledge_frontend_result(intent));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            1,
            FrontendPollPhase::BrokerReadyNext,
            intent,
        );
        assert!(!core.frontend_ready(intent));
        assert!(core.frontend_polls.is_empty());
        // The refused request did not consume receive order or a request row.
        submit_frontend_lifecycle(
            &mut core,
            broker,
            1,
            FrontendPollPhase::BrokerRetired,
            intent,
        );
        assert!(core.frontend_completions[0].retired);
        let epoch = core.epoch.unwrap();
        install_frontend_route(
            &mut core,
            epoch,
            RouteId([80; 16]),
            TaskGeneration::new(3).unwrap(),
            RouteRole::Broker,
            BackendRole::Broker,
        );
        let foreign = core.routes.last().unwrap().identity;
        submit_frontend_lifecycle(
            &mut core,
            foreign,
            0,
            FrontendPollPhase::BrokerReadyNext,
            intent,
        );
        assert!(!core.frontend_ready(intent));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            2,
            FrontendPollPhase::BrokerReadyNext,
            intent,
        );
        assert!(core.frontend_ready(intent));
        let pending = core.frontend_polls.len();
        submit_frontend_lifecycle(
            &mut core,
            broker,
            3,
            FrontendPollPhase::BrokerReadyNext,
            intent,
        );
        assert_eq!(core.frontend_polls.len(), pending);
        assert!(core.finish_frontend_intent(intent));
    }

    #[test]
    fn frontend_retirement_requires_publication_exact_broker_and_closed_worker() {
        for case in 0..4 {
            let (mut core, intent, broker, worker) = frontend_race_fixture();
            submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
            let reporter = if case == 2 { worker } else { broker };
            if case != 0 {
                core.acknowledge_frontend_result(intent);
            }
            if case != 1 {
                let epoch = core.epoch.unwrap();
                core.receive(
                    SocketPlane::Control,
                    envelope(
                        ROUTE_CLOSE,
                        epoch,
                        worker.route,
                        worker.generation.get(),
                        1,
                        RequestId([76; 16]),
                        0,
                    ),
                    &[],
                );
            }
            if case == 3 {
                core.frontend_completions[0].broker = Some(SequenceIdentity {
                    generation: TaskGeneration::new(2).unwrap(),
                    ..broker
                });
            }
            core.control_out.clear();
            submit_frontend_lifecycle(
                &mut core,
                reporter,
                if case == 2 { 2 } else { 0 },
                FrontendPollPhase::BrokerRetired,
                intent,
            );
            assert!(!core.frontend_completions[0].retired);
            assert!(!core.frontend_ready(intent));
            assert_eq!(core.control_out.frames.len(), 1);
            assert_eq!(
                EnvelopeV1::read_from_bytes(&core.control_out.frames[0][..ENVELOPE_LEN])
                    .unwrap()
                    .kind
                    .get(),
                REFUSED
            );
        }
    }

    #[test]
    fn frontend_retirement_backpressure_waits_before_broker_readiness() {
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
        core.pop_frontend_result().unwrap();
        assert!(core.acknowledge_frontend_result(intent));
        let epoch = core.epoch.unwrap();
        core.receive(
            SocketPlane::Control,
            envelope(
                ROUTE_CLOSE,
                epoch,
                worker.route,
                worker.generation.get(),
                1,
                RequestId([76; 16]),
                0,
            ),
            &[],
        );
        core.control_out.clear();
        core.control_out.bytes = QUEUE_BYTES_MAX;
        submit_frontend_lifecycle(
            &mut core,
            broker,
            0,
            FrontendPollPhase::BrokerRetired,
            intent,
        );
        assert_eq!(core.frontend_polls.len(), 1);
        assert!(!core.frontend_completions[0].retired);
        assert!(!core.frontend_ready(intent));
        core.control_out.bytes = 0;
        core.poll_backend();
        assert!(core.frontend_completions[0].retired);
        assert!(core.frontend_polls.is_empty());
        assert_eq!(core.control_out.frames.len(), 1);
        assert!(!core.frontend_ready(intent));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            1,
            FrontendPollPhase::BrokerReadyNext,
            intent,
        );
        assert!(core.frontend_ready(intent));
        assert!(core.finish_frontend_intent(intent));
    }

    #[test]
    fn frontend_cancellation_orders_against_results_without_replacing_a_settled_answer() {
        for cancel_first in [true, false] {
            let (mut core, intent, _, worker) = frontend_race_fixture();
            if cancel_first {
                assert_eq!(
                    core.request_frontend_cancellation(intent),
                    FrontendCancelOutcome::Requested
                );
                assert_eq!(
                    core.request_frontend_cancellation(intent),
                    FrontendCancelOutcome::Requested
                );
            }
            submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
            assert_eq!(
                core.request_frontend_cancellation(intent),
                FrontendCancelOutcome::AlreadySettled
            );
            let result = core.pop_frontend_result().unwrap();
            assert_eq!(
                result.kind,
                if cancel_first {
                    FrontendResultKind::Cancelled
                } else {
                    FrontendResultKind::Completed
                }
            );
            assert_eq!(
                result.text,
                if cancel_first {
                    "Prompt cancelled."
                } else {
                    "Finished before the deadline."
                }
            );
            assert!(
                core.control_out.frames.is_empty(),
                "cancellation does not waive publication"
            );
            assert_eq!(core.frontend_receipts.len(), 1);
            assert!(core.acknowledge_frontend_result(intent));
            assert!(
                !core.frontend_ready(intent),
                "publication does not waive native retirement"
            );
        }
    }

    #[test]
    fn frontend_queued_cancellation_removes_only_the_unclaimed_intent() {
        let (mut core, active, broker, _) = frontend_race_fixture();
        let cancelled = core.queue_frontend_intent("cancel before claim").unwrap();
        let other = core.queue_frontend_intent("keep queued").unwrap();
        assert_eq!(
            core.request_frontend_cancellation(cancelled),
            FrontendCancelOutcome::QueuedRemoved
        );
        assert_eq!(
            core.request_frontend_cancellation(cancelled),
            FrontendCancelOutcome::Unknown
        );
        assert_eq!(
            core.frontend_control(broker, active),
            Some(kore_hostlink::FrontendControl::Active)
        );
        assert_eq!(core.frontend_intents.len(), 2);
        assert_eq!(core.frontend_intents[1].intent_id, other);
        assert!(core.frontend_results.is_empty() && core.frontend_completions.is_empty());
        assert_eq!(core.routes.len(), 2);
    }

    #[test]
    fn frontend_control_is_exact_broker_owned_and_rechecks_backpressured_cancellation() {
        use kore_hostlink::FrontendControl;
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        submit_frontend_lifecycle(
            &mut core,
            worker,
            0,
            FrontendPollPhase::BrokerControl,
            intent,
        );
        assert_eq!(
            EnvelopeV1::read_from_bytes(&core.control_out.frames.back().unwrap()[..ENVELOPE_LEN])
                .unwrap()
                .kind
                .get(),
            REFUSED
        );
        assert_eq!(
            core.frontend_control(
                SequenceIdentity {
                    generation: TaskGeneration::new(4).unwrap(),
                    ..broker
                },
                intent
            ),
            None
        );
        assert_eq!(
            core.frontend_control(broker, FrontendIntentId::from_sequence(999).unwrap()),
            None
        );
        let (mut core, intent, broker, _) = frontend_race_fixture();
        core.control_out.clear();
        core.control_out.bytes = QUEUE_BYTES_MAX;
        submit_frontend_lifecycle(
            &mut core,
            broker,
            0,
            FrontendPollPhase::BrokerControl,
            intent,
        );
        assert_eq!(core.frontend_polls.len(), 1);
        assert_eq!(
            core.request_frontend_cancellation(intent),
            FrontendCancelOutcome::Requested
        );
        core.control_out.bytes = 0;
        core.poll_backend();
        assert!(core.frontend_polls.is_empty());
        let reply = decode_inference_event(&core.control_out.frames[0][ENVELOPE_LEN..]).unwrap();
        assert_eq!(
            FrontendControl::decode(reply.payload),
            Some(FrontendControl::CancelRequested)
        );
    }

    #[test]
    fn frontend_cancelled_worker_closure_leaves_other_inference_alive() {
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        let epoch = core.epoch.unwrap();
        let sibling = SequenceIdentity {
            route: RouteId([79; 16]),
            ..worker
        };
        install_frontend_route(
            &mut core,
            epoch,
            sibling.route,
            sibling.generation,
            RouteRole::Agent,
            BackendRole::Agent,
        );
        // Exercise the daemon's supported raw inference admission and actual
        // backend sequence owners; no cancelled intent sweeps shared requests.
        for identity in [worker, sibling] {
            core.backend.open(identity).unwrap();
            core.receive(
                SocketPlane::Control,
                envelope(
                    INFER_REQUEST,
                    epoch,
                    identity.route,
                    identity.generation.get(),
                    0,
                    RequestId([90; 16]),
                    4,
                ),
                b"work",
            );
        }
        assert_eq!(core.backend_metrics().pending, 2);
        assert_eq!(
            core.request_frontend_cancellation(intent),
            FrontendCancelOutcome::Requested
        );
        assert_eq!(
            core.backend_metrics().pending,
            2,
            "the broker owns cancellation of its requests"
        );
        submit_frontend_race_result(&mut core, intent, broker, 0, RequestId([74; 16]));
        assert_eq!(
            core.pop_frontend_result().unwrap().kind,
            FrontendResultKind::Cancelled
        );
        assert!(core.acknowledge_frontend_result(intent));
        core.receive(
            SocketPlane::Control,
            envelope(
                ROUTE_CLOSE,
                epoch,
                worker.route,
                worker.generation.get(),
                1,
                RequestId([76; 16]),
                0,
            ),
            &[],
        );
        assert_eq!(core.backend_metrics().pending, 1);
        assert!(core.routes.iter().any(|route| route.identity == sibling));
        submit_frontend_lifecycle(
            &mut core,
            broker,
            1,
            FrontendPollPhase::BrokerRetired,
            intent,
        );
        submit_frontend_lifecycle(
            &mut core,
            broker,
            2,
            FrontendPollPhase::BrokerReadyNext,
            intent,
        );
        assert!(core.finish_frontend_intent(intent));
        assert_eq!(core.backend_metrics().pending, 1);
    }

    #[test]
    fn frontend_planning_cancel_after_terminal_does_not_replay_or_disconnect() {
        let (mut core, _, broker, _) = frontend_race_fixture();
        let epoch = core.epoch.unwrap();
        let request = RequestId([91; 16]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                broker.route,
                broker.generation.get(),
                0,
                request,
                4,
            ),
            b"plan",
        );
        core.advance();
        assert_eq!(core.backend_metrics().pending, 0);
        assert!(core.routes[0].completed_requests.contains(&request));
        let before = core.control_out.frames.len();
        core.receive(
            SocketPlane::Control,
            envelope(
                CANCEL,
                epoch,
                broker.route,
                broker.generation.get(),
                1,
                request,
                0,
            ),
            &[],
        );
        assert_eq!(
            core.control_out.frames.len(),
            before,
            "keep the original terminal reply"
        );
        assert!(!core.reconnect_required());
        assert_eq!(core.epoch(), Some(epoch));
        // The history belongs to the full route generation, not a bare ID.
        core.routes.remove(0);
        let next = TaskGeneration::new(4).unwrap();
        install_frontend_route(
            &mut core,
            epoch,
            broker.route,
            next,
            RouteRole::Broker,
            BackendRole::Broker,
        );
        core.receive(
            SocketPlane::Control,
            envelope(CANCEL, epoch, broker.route, next.get(), 0, request, 0),
            &[],
        );
        assert_eq!(
            EnvelopeV1::read_from_bytes(&core.control_out.frames.back().unwrap()[..ENVELOPE_LEN])
                .unwrap()
                .kind
                .get(),
            REFUSED
        );
    }

    #[test]
    fn frontend_broker_cancelled_result_requires_a_host_cancellation() {
        for cancel in [false, true] {
            let (mut core, intent, broker, _) = frontend_race_fixture();
            core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
            if cancel {
                core.request_frontend_cancellation(intent);
            }
            let mut inner = [0; INFERENCE_PART_MAX];
            let len = kore_hostlink::FrontendResult {
                intent_id: intent,
                kind: FrontendResultKind::Cancelled,
                text: "Prompt cancelled.",
            }
            .encode(&mut inner)
            .unwrap();
            let payload = frontend_request(InferenceRequestKind::FrontendResult, 1, &inner[..len]);
            core.receive(
                SocketPlane::Control,
                envelope(
                    INFER_REQUEST,
                    core.epoch.unwrap(),
                    broker.route,
                    broker.generation.get(),
                    0,
                    RequestId([74; 16]),
                    payload.len(),
                ),
                &payload,
            );
            assert_eq!(core.frontend_results.len(), usize::from(cancel));
            if cancel {
                assert_eq!(
                    core.pop_frontend_result().unwrap().kind,
                    FrontendResultKind::Cancelled
                );
                assert!(core.acknowledge_frontend_result(intent));
                submit_frontend_lifecycle(
                    &mut core,
                    broker,
                    1,
                    FrontendPollPhase::BrokerRetired,
                    intent,
                );
                submit_frontend_lifecycle(
                    &mut core,
                    broker,
                    2,
                    FrontendPollPhase::BrokerReadyNext,
                    intent,
                );
                assert!(core.finish_frontend_intent(intent));
            }
        }
    }

    #[test]
    fn frontend_retirement_cannot_overtake_a_backpressured_control_reply() {
        let (mut core, intent, broker, worker) = frontend_race_fixture();
        submit_frontend_race_result(&mut core, intent, worker, 0, RequestId([74; 16]));
        core.pop_frontend_result().unwrap();
        core.acknowledge_frontend_result(intent);
        let receipt_len = core.control_out.frames[0].len();
        let epoch = core.epoch.unwrap();
        core.receive(
            SocketPlane::Control,
            envelope(
                ROUTE_CLOSE,
                epoch,
                worker.route,
                worker.generation.get(),
                1,
                RequestId([76; 16]),
                0,
            ),
            &[],
        );
        core.control_out.clear();
        core.control_out.bytes = QUEUE_BYTES_MAX - receipt_len;
        submit_frontend_lifecycle(
            &mut core,
            broker,
            0,
            FrontendPollPhase::BrokerControl,
            intent,
        );
        submit_frontend_lifecycle(
            &mut core,
            broker,
            1,
            FrontendPollPhase::BrokerRetired,
            intent,
        );
        assert_eq!(core.frontend_polls.len(), 2);
        assert!(!core.frontend_completions[0].retired);
        assert!(core.control_out.frames.is_empty());
        core.control_out.bytes = 0;
        core.poll_backend();
        assert!(core.frontend_polls.is_empty());
        assert!(core.frontend_completions[0].retired);
        assert_eq!(core.control_out.frames.len(), 2);
        assert_eq!(core.control_out.frames[0].len(), receipt_len + 1);
        assert_eq!(core.control_out.frames[1].len(), receipt_len);
    }

    #[test]
    fn frontend_inference_failure_is_terminal_for_one_request_and_recovers() {
        for (failure, code) in [
            (BackendFailure::TimedOut, RefusalCode::TimedOut),
            (
                BackendFailure::HostDisconnected,
                RefusalCode::HostDisconnected,
            ),
            (
                BackendFailure::BackendRestarted,
                RefusalCode::HostDisconnected,
            ),
            (
                BackendFailure::MalformedResponse,
                RefusalCode::InvalidEncoding,
            ),
            (BackendFailure::ModelFailed, RefusalCode::ModelFailed),
            (BackendFailure::OutputLimit, RefusalCode::TooLarge),
            (BackendFailure::ContextLimit, RefusalCode::BudgetExceeded),
        ] {
            let (mut core, intent, broker, worker) = frontend_race_fixture();
            core.frontend_intents[0].state = FrontendIntentState::BrokerClaimed;
            let epoch = core.epoch.unwrap();
            let request = RequestId([92; 16]);
            for identity in [broker, worker] {
                if identity == worker {
                    core.backend.open(identity).unwrap();
                }
                core.receive(
                    SocketPlane::Control,
                    envelope(
                        INFER_REQUEST,
                        epoch,
                        identity.route,
                        identity.generation.get(),
                        0,
                        request,
                        4,
                    ),
                    b"work",
                );
            }
            assert_eq!(core.backend_metrics().pending, 2);
            assert!(!core.frontend_requires_progress());
            // Replace this one real pending backend completion with a fault.
            core.backend.cancel(broker, request, 0).unwrap();
            core.control_out.clear();
            core.finish(BackendEvent::Failed {
                sequence: broker,
                request,
                submitted_tick: 0,
                turn: 1,
                failure,
            });
            assert_eq!(core.epoch(), Some(epoch));
            assert!(!core.reconnect_required());
            assert!(
                core.frontend_requires_progress(),
                "another inference cannot keep the failed guest asleep"
            );
            assert_eq!(core.backend_metrics().pending, 1);
            let frame = &core.control_out.frames[0];
            assert_eq!(
                EnvelopeV1::read_from_bytes(&frame[..ENVELOPE_LEN])
                    .unwrap()
                    .kind
                    .get(),
                INFER_REPLY
            );
            let event = decode_inference_event(&frame[ENVELOPE_LEN..]).unwrap();
            assert_eq!(event.kind, InferenceEventKind::TurnFailed);
            assert_eq!(event.turn, 1);
            assert_eq!(event.part, 0);
            assert_eq!(
                kore_hostlink::decode_inference_failure(event.payload)
                    .unwrap()
                    .code,
                code
            );
            assert!(
                core.frontend_results.is_empty(),
                "the guest still owns its terminal frontend result"
            );
            submit_frontend_race_result(&mut core, intent, broker, 1, RequestId([74; 16]));
            assert_eq!(
                core.pop_frontend_result().unwrap().kind,
                FrontendResultKind::Failed
            );
            core.acknowledge_frontend_result(intent);
            submit_frontend_lifecycle(
                &mut core,
                broker,
                2,
                FrontendPollPhase::BrokerRetired,
                intent,
            );
            submit_frontend_lifecycle(
                &mut core,
                broker,
                3,
                FrontendPollPhase::BrokerReadyNext,
                intent,
            );
            assert!(core.finish_frontend_intent(intent));
            assert!(!core.frontend_requires_progress());
            let next = core.queue_frontend_intent("try again").unwrap();
            assert_eq!(core.frontend_intents[0].intent_id, next);
            assert_eq!(
                core.frontend_intents[0].state,
                FrontendIntentState::BrokerClaimed
            );
            assert_eq!(core.backend_metrics().pending, 1);
        }
    }

    #[test]
    fn consecutive_frontend_intents_use_fresh_agent_generations() {
        let epoch = LinkEpoch([51; 16]);
        let broker_route = RouteId([52; 16]);
        let agent_route = RouteId([53; 16]);
        let broker_generation = TaskGeneration::new(2).unwrap();
        let agent_generation = TaskGeneration::new(3).unwrap();
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.epoch = Some(epoch);
        core.backend
            .open(SequenceIdentity {
                route: broker_route,
                generation: broker_generation,
                role: BackendRole::Broker,
            })
            .unwrap();
        install_frontend_route(
            &mut core,
            epoch,
            broker_route,
            broker_generation,
            RouteRole::Broker,
            BackendRole::Broker,
        );
        install_frontend_route(
            &mut core,
            epoch,
            agent_route,
            agent_generation,
            RouteRole::Agent,
            BackendRole::Agent,
        );
        let intent_id = core.queue_frontend_intent("read X").unwrap();

        let mut inner = [0; INFERENCE_PART_MAX];
        let length = FrontendPoll {
            phase: FrontendPollPhase::BrokerNext,
            intent_id: FrontendIntentId::ZERO,
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendPoll, 1, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                broker_route,
                broker_generation.get(),
                0,
                RequestId([54; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert_eq!(
            core.frontend_intents[0].state,
            FrontendIntentState::BrokerClaimed
        );

        let length = FrontendPoll {
            phase: FrontendPollPhase::AgentClaim,
            intent_id,
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendPoll, 1, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                agent_route,
                agent_generation.get(),
                0,
                RequestId([55; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert_eq!(
            core.frontend_intents[0].state,
            FrontendIntentState::AgentClaimed(SequenceIdentity {
                route: agent_route,
                generation: agent_generation,
                role: BackendRole::Agent,
            })
        );

        let length = kore_hostlink::FrontendResult {
            intent_id,
            kind: FrontendResultKind::Completed,
            text: "ordinary X content",
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendResult, 2, &inner[..length]);
        core.control_out.clear();
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                agent_route,
                agent_generation.get(),
                1,
                RequestId([56; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert!(core.frontend_intents.is_empty());
        assert!(
            core.control_out.frames.is_empty(),
            "receipt must wait for publication"
        );
        assert_eq!(core.frontend_receipts.len(), 1);
        assert_eq!(
            core.pop_frontend_result(),
            Some(FrontendResultOwned {
                intent_id,
                kind: FrontendResultKind::Completed,
                text: "ordinary X content".into(),
            })
        );
        assert!(
            core.control_out.frames.is_empty(),
            "taking the result is not publication"
        );
        core.control_out.bytes = QUEUE_BYTES_MAX;
        assert!(!core.acknowledge_frontend_result(intent_id));
        assert_eq!(
            core.frontend_receipts.len(),
            1,
            "backpressure retains the receipt"
        );
        core.control_out.bytes = 0;
        assert!(core.acknowledge_frontend_result(intent_id));
        assert!(!core.control_out.frames.is_empty());
        assert!(
            !core.acknowledge_frontend_result(intent_id),
            "only one receipt"
        );

        // Kore tears the prompt-scoped runner down after returning its
        // HostLink pair, then opens the same logical agent route at the next
        // generation. A later chat must still reach the resident broker and
        // may only be claimed by that new exact generation.
        core.routes
            .retain(|route| route.identity.role != BackendRole::Agent);
        let next_agent_generation = TaskGeneration::new(4).unwrap();
        install_frontend_route(
            &mut core,
            epoch,
            agent_route,
            next_agent_generation,
            RouteRole::Agent,
            BackendRole::Agent,
        );
        let next_intent_id = core.queue_frontend_intent("hello").unwrap();

        let length = FrontendPoll {
            phase: FrontendPollPhase::BrokerNext,
            intent_id: FrontendIntentId::ZERO,
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendPoll, 1, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                broker_route,
                broker_generation.get(),
                1,
                RequestId([57; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert_eq!(
            core.frontend_intents[0].state,
            FrontendIntentState::BrokerClaimed
        );

        let length = FrontendPoll {
            phase: FrontendPollPhase::AgentClaim,
            intent_id: next_intent_id,
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendPoll, 1, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                agent_route,
                next_agent_generation.get(),
                0,
                RequestId([58; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert_eq!(
            core.frontend_intents[0].state,
            FrontendIntentState::AgentClaimed(SequenceIdentity {
                route: agent_route,
                generation: next_agent_generation,
                role: BackendRole::Agent,
            })
        );

        let length = kore_hostlink::FrontendResult {
            intent_id: next_intent_id,
            kind: FrontendResultKind::Completed,
            text: "hello",
        }
        .encode(&mut inner)
        .unwrap();
        let payload = frontend_request(InferenceRequestKind::FrontendResult, 2, &inner[..length]);
        core.receive(
            SocketPlane::Control,
            envelope(
                INFER_REQUEST,
                epoch,
                agent_route,
                next_agent_generation.get(),
                1,
                RequestId([59; 16]),
                payload.len(),
            ),
            &payload,
        );
        assert!(core.frontend_intents.is_empty());
        assert_eq!(
            core.pop_frontend_result(),
            Some(FrontendResultOwned {
                intent_id: next_intent_id,
                kind: FrontendResultKind::Completed,
                text: "hello".into(),
            })
        );
        assert!(core.acknowledge_frontend_result(next_intent_id));
    }

    #[test]
    fn live_transcript_records_exact_raw_and_validated_assistant_observations() {
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        for (kind, payload) in [
            (
                ObservationKind::RawModelCompletion,
                r#"{"action":"respond","text":"hello\nworld"}"#,
            ),
            (ObservationKind::AssistantMessage, "hello\nworld"),
        ] {
            core.record_inference_observation(&InferenceObservation {
                role: BackendRole::Agent,
                route: RouteId([9; 16]),
                generation: TaskGeneration::new(2).unwrap(),
                request: RequestId([10; 16]),
                turn: 1,
                kind,
                payload: payload.into(),
                accepted: Some(true),
                detail: "accepted".into(),
            });
        }
        let records: Vec<_> = core
            .transcript()
            .map(|line| {
                let (_, json) = line.split_once("inference-observation ").unwrap();
                serde_json::from_str::<serde_json::Value>(json).unwrap()
            })
            .collect();
        assert_eq!(
            records[0]["payload"],
            r#"{"action":"respond","text":"hello\nworld"}"#
        );
        assert_eq!(records[1]["payload"], "hello\nworld");
        assert_eq!(records[1]["kind"], "assistant_message");
    }

    #[test]
    fn broker_authority_reports_are_observed_only_after_strict_ordered_validation() {
        let epoch = LinkEpoch([41; 16]);
        let route_id = RouteId([42; 16]);
        let generation = TaskGeneration::new(3).unwrap();
        let state = RouteState::new(
            epoch,
            route_id,
            generation,
            RouteRole::Broker,
            16,
            HOST_BULK_BYTE_CREDITS_PER_ROUTE,
            HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
        )
        .unwrap();
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.epoch = Some(epoch);
        core.routes.push(Route {
            state,
            identity: SequenceIdentity {
                route: route_id,
                generation,
                role: BackendRole::Broker,
            },
            stream_parts: Vec::new(),
            completed_requests: VecDeque::new(),
        });
        let report = kore_hostlink::AuthorityReportView {
            state: kore_hostlink::AuthorityState::Installed,
            kind: kore_hostlink::AuthorityKind::Executable,
            agent_slot: 2,
            generation: 7,
            rights: 1,
            label: "cat",
        };
        let mut payload =
            [0; kore_hostlink::AUTHORITY_REPORT_HEADER_LEN + kore_hostlink::AUTHORITY_LABEL_MAX];
        let length = report.encode(&mut payload).unwrap();
        core.receive(
            SocketPlane::Control,
            envelope(
                kore_hostlink::AUTHORITY_REPORT,
                epoch,
                route_id,
                generation.get(),
                0,
                RequestId([43; 16]),
                length,
            ),
            &payload[..length],
        );
        assert_eq!(
            core.pop_authority_observation(),
            Some(AuthorityObservation {
                state: kore_hostlink::AuthorityState::Installed,
                kind: kore_hostlink::AuthorityKind::Executable,
                agent_slot: 2,
                generation: 7,
                rights: 1,
                label: "cat".into(),
            })
        );

        let mut malformed = payload[..length].to_vec();
        malformed[10] = 1;
        core.receive(
            SocketPlane::Control,
            envelope(
                kore_hostlink::AUTHORITY_REPORT,
                epoch,
                route_id,
                generation.get(),
                1,
                RequestId([44; 16]),
                length,
            ),
            &malformed,
        );
        assert!(core.pop_authority_observation().is_none());
        core.receive(
            SocketPlane::Control,
            envelope(
                kore_hostlink::AUTHORITY_REPORT,
                epoch,
                route_id,
                generation.get(),
                1,
                RequestId([44; 16]),
                length,
            ),
            &payload[..length],
        );
        assert!(core.pop_authority_observation().is_some());
    }

    #[test]
    fn outbound_bulk_uses_the_founded_four_kib_chunk_profile() {
        let bulk_id = BulkId([7; 16]);
        let payload = vec![0xA5; kore_hostlink::FOUNDED_BULK_WIRE_CHUNK + 17];
        let frames = outbound_bulk_payloads(bulk_id, 9, &payload).unwrap();
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[0].0, BULK_BEGIN);
        let reference = BulkRefV1::read_from_bytes(frames[0].1.as_slice()).unwrap();
        assert_eq!(reference.chunk_count.get(), 2);
        assert_eq!(reference.total_len.get(), payload.len() as u32);
        for (index, expected_len) in [kore_hostlink::FOUNDED_BULK_WIRE_CHUNK, 17]
            .into_iter()
            .enumerate()
        {
            assert_eq!(frames[index + 1].0, BULK_CHUNK);
            let header = BulkChunkHeaderV1::read_from_prefix(frames[index + 1].1.as_slice())
                .unwrap()
                .0;
            let chunk = &frames[index + 1].1[kore_hostlink::BULK_CHUNK_HEADER_LEN..];
            assert_eq!(
                header.offset.get(),
                (index * kore_hostlink::FOUNDED_BULK_WIRE_CHUNK) as u32
            );
            assert_eq!(chunk.len(), expected_len);
            assert_eq!(header.digest_blake3, *blake3::hash(chunk).as_bytes());
        }
        assert_eq!(frames[3].0, BULK_FINISH);
    }

    #[test]
    fn outbound_bulk_request_retires_only_after_ordered_terminal_ready() {
        let epoch = LinkEpoch([1; 16]);
        let route_id = RouteId([2; 16]);
        let request = RequestId([3; 16]);
        let bulk_id = BulkId([4; 16]);
        let mut state = RouteState::new(
            epoch,
            route_id,
            TaskGeneration::new(1).unwrap(),
            RouteRole::Importer,
            16,
            HOST_BULK_BYTE_CREDITS_PER_ROUTE,
            HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
        )
        .unwrap();
        state.open_request(request).unwrap();
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.epoch = Some(epoch);
        core.routes.push(Route {
            state,
            identity: SequenceIdentity {
                route: route_id,
                generation: TaskGeneration::new(1).unwrap(),
                role: BackendRole::Workspace,
            },
            stream_parts: Vec::new(),
            completed_requests: VecDeque::new(),
        });
        core.outbound_bulk.push(OutboundBulk {
            route: route_id,
            request,
            bulk_id,
            accepted_expected: 3,
            accepted: 0,
        });

        for sequence in 0..3 {
            core.receive(
                SocketPlane::Control,
                envelope(BULK_ACCEPTED, epoch, route_id, 1, sequence, request, 16),
                &bulk_id.0,
            );
            assert_eq!(core.outbound_bulk.len(), 1);
        }
        core.receive(
            SocketPlane::Control,
            envelope(BULK_READY, epoch, route_id, 1, 3, request, 16),
            &bulk_id.0,
        );
        assert!(core.outbound_bulk.is_empty());
        assert_eq!(
            core.routes[0].state.close_request(request),
            Err(RequestError::Unknown)
        );
    }

    #[test]
    fn inbound_bulk_ack_uses_the_ordered_control_plane() {
        let epoch = LinkEpoch([11; 16]);
        let route_id = RouteId([12; 16]);
        let request = RequestId([13; 16]);
        let bulk_id = BulkId([14; 16]);
        let state = RouteState::new(
            epoch,
            route_id,
            TaskGeneration::new(1).unwrap(),
            RouteRole::Importer,
            16,
            HOST_BULK_BYTE_CREDITS_PER_ROUTE,
            HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
        )
        .unwrap();
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.epoch = Some(epoch);
        core.routes.push(Route {
            state,
            identity: SequenceIdentity {
                route: route_id,
                generation: TaskGeneration::new(1).unwrap(),
                role: BackendRole::Workspace,
            },
            stream_parts: Vec::new(),
            completed_requests: VecDeque::new(),
        });
        let reference = BulkRefV1 {
            bulk_id,
            total_len: U32::new(1),
            chunk_count: U16::new(1),
            content_kind: U16::new(kore_workspace_sync::WORKSPACE_BLOB_CONTENT),
            digest_blake3: *blake3::hash(b"x").as_bytes(),
        };

        core.receive(
            SocketPlane::Bulk,
            envelope(
                BULK_BEGIN,
                epoch,
                route_id,
                1,
                0,
                request,
                reference.as_bytes().len(),
            ),
            reference.as_bytes(),
        );

        let control = core.pop_control_frame().expect("bulk ACK on control");
        let ack = EnvelopeV1::read_from_bytes(&control[..ENVELOPE_LEN]).unwrap();
        assert_eq!(ack.kind.get(), BULK_ACCEPTED);
        assert!(core.pop_bulk_frame().is_none());
    }

    #[test]
    fn founded_broker_bulk_kir1_larger_than_inline_client_buffer_reaches_reply() {
        const FOUNDED_CLIENT_BUFFER: usize = 512;

        let epoch = LinkEpoch([31; 16]);
        let route_id = RouteId([32; 16]);
        let request = RequestId([33; 16]);
        let bulk_id = BulkId([34; 16]);
        let generation = TaskGeneration::new(1).unwrap();
        let identity = SequenceIdentity {
            route: route_id,
            generation,
            role: BackendRole::Broker,
        };
        let state = RouteState::new(
            epoch,
            route_id,
            generation,
            RouteRole::Broker,
            16,
            HOST_BULK_BYTE_CREDITS_PER_ROUTE,
            HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
        )
        .unwrap();
        let mut core = LinkCore::new(crate::backend::DeterministicBackend::new(1), ());
        core.backend.open(identity).unwrap();
        core.epoch = Some(epoch);
        core.routes.push(Route {
            state,
            identity,
            stream_parts: Vec::new(),
            completed_requests: VecDeque::new(),
        });

        let mut offers = vec![
            kore_hostlink::BrokerOffer {
                reference: [1; 16],
                kind: kore_hostlink::BrokerOfferKind::Object,
                permitted_ops: 1,
                max_uses: 0,
                label: "workspace/X",
            },
            kore_hostlink::BrokerOffer {
                reference: [2; 16],
                kind: kore_hostlink::BrokerOfferKind::Executable,
                permitted_ops: 4,
                max_uses: 2,
                label: "ls",
            },
            kore_hostlink::BrokerOffer {
                reference: [3; 16],
                kind: kore_hostlink::BrokerOfferKind::Executable,
                permitted_ops: 4,
                max_uses: 2,
                label: "find",
            },
            kore_hostlink::BrokerOffer {
                reference: [4; 16],
                kind: kore_hostlink::BrokerOfferKind::Executable,
                permitted_ops: 4,
                max_uses: 2,
                label: "stat",
            },
            kore_hostlink::BrokerOffer {
                reference: [5; 16],
                kind: kore_hostlink::BrokerOfferKind::Executable,
                permitted_ops: 4,
                max_uses: 2,
                label: "cat",
            },
        ];
        for reference in 6..=15 {
            offers.push(kore_hostlink::BrokerOffer {
                reference: [reference; 16],
                kind: kore_hostlink::BrokerOfferKind::Object,
                permitted_ops: 1,
                max_uses: 0,
                label: "workspace/decoy",
            });
        }
        let mut planning = [0; INLINE_MAX];
        let planning_len = kore_hostlink::TaskIntentRequest {
            plan: *b"bulk-plan-000001",
            intent: "read X",
            ceiling: kore_hostlink::TaskBudget {
                tool_calls: 2,
                ..kore_hostlink::TaskBudget::ZERO
            },
            offers: &offers,
            attachments: &[],
        }
        .encode(&mut planning)
        .unwrap();
        let mut kir1 = [0; INLINE_MAX];
        let kir1_len = kore_hostlink::InferenceRequestView {
            kind: kore_hostlink::InferenceRequestKind::BrokerPlan,
            turn: 1,
            max_output_tokens: 128,
            timeout_ticks: 100,
            input: &planning[..planning_len],
        }
        .encode(&mut kir1)
        .unwrap();
        assert!(kir1_len > FOUNDED_CLIENT_BUFFER);

        let frames = outbound_bulk_payloads(
            bulk_id,
            kore_hostlink::INFERENCE_REQUEST_CONTENT,
            &kir1[..kir1_len],
        )
        .unwrap();
        for (sequence, (kind, payload)) in frames.into_iter().enumerate() {
            core.receive(
                SocketPlane::Bulk,
                envelope(
                    kind,
                    epoch,
                    route_id,
                    generation.get(),
                    sequence as u64,
                    request,
                    payload.len(),
                ),
                &payload,
            );
        }

        assert!(core.routes[0].state.has_request(request));
        assert_eq!(core.routes[0].stream_parts, vec![(request, 0)]);
        core.advance();
        assert!(!core.routes[0].state.has_request(request));
        assert!(core.routes[0].stream_parts.is_empty());
        let kinds: Vec<u16> = core
            .control_out
            .frames
            .iter()
            .map(|frame| {
                EnvelopeV1::read_from_bytes(&frame[..ENVELOPE_LEN])
                    .unwrap()
                    .kind
                    .get()
            })
            .collect();
        assert_eq!(
            kinds,
            vec![BULK_ACCEPTED, BULK_ACCEPTED, BULK_READY, INFER_REPLY]
        );
    }

    #[test]
    fn workspace_upload_retains_request_through_typed_application_reply() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), b"seed").unwrap();
        let backend = crate::workspace::WorkspaceBackend::open(root.path(), 1).unwrap();
        let epoch = LinkEpoch([21; 16]);
        let route_id = RouteId([22; 16]);
        let request = RequestId([23; 16]);
        let bulk_id = BulkId([24; 16]);
        let generation = TaskGeneration::new(1).unwrap();
        let identity = SequenceIdentity {
            route: route_id,
            generation,
            role: BackendRole::Workspace,
        };
        let state = RouteState::new(
            epoch,
            route_id,
            generation,
            RouteRole::Importer,
            16,
            HOST_BULK_BYTE_CREDITS_PER_ROUTE,
            HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
        )
        .unwrap();
        let mut core = LinkCore::new(backend, ());
        core.backend.open(identity).unwrap();
        core.epoch = Some(epoch);
        core.routes.push(Route {
            state,
            identity,
            stream_parts: Vec::new(),
            completed_requests: VecDeque::new(),
        });

        let bytes = b"x";
        let digest = *blake3::hash(bytes).as_bytes();
        let reference = BulkRefV1 {
            bulk_id,
            total_len: U32::new(bytes.len() as u32),
            chunk_count: U16::new(1),
            content_kind: U16::new(kore_workspace_sync::WORKSPACE_BLOB_CONTENT),
            digest_blake3: digest,
        };
        core.receive(
            SocketPlane::Bulk,
            envelope(
                BULK_BEGIN,
                epoch,
                route_id,
                generation.get(),
                0,
                request,
                reference.as_bytes().len(),
            ),
            reference.as_bytes(),
        );
        let chunk = BulkChunkHeaderV1 {
            bulk_id,
            offset: U32::ZERO,
            digest_blake3: digest,
        };
        let mut chunk_payload = chunk.as_bytes().to_vec();
        chunk_payload.extend_from_slice(bytes);
        core.receive(
            SocketPlane::Bulk,
            envelope(
                BULK_CHUNK,
                epoch,
                route_id,
                generation.get(),
                1,
                request,
                chunk_payload.len(),
            ),
            &chunk_payload,
        );
        let finish = BulkFinishV1 {
            bulk_id,
            final_total: U32::new(bytes.len() as u32),
            digest_blake3: digest,
        };
        core.receive(
            SocketPlane::Bulk,
            envelope(
                BULK_FINISH,
                epoch,
                route_id,
                generation.get(),
                2,
                request,
                finish.as_bytes().len(),
            ),
            finish.as_bytes(),
        );

        assert!(core.routes[0].state.has_request(request));
        core.advance();
        assert!(!core.routes[0].state.has_request(request));
        let kinds: Vec<u16> = core
            .control_out
            .frames
            .iter()
            .map(|frame| {
                EnvelopeV1::read_from_bytes(&frame[..ENVELOPE_LEN])
                    .unwrap()
                    .kind
                    .get()
            })
            .collect();
        assert_eq!(
            kinds,
            vec![BULK_ACCEPTED, BULK_ACCEPTED, BULK_READY, WORKSPACE_REPLY]
        );
    }

    #[test]
    fn route_credit_covers_four_max_chunk_count_transfers_until_terminal() {
        let epoch = LinkEpoch([1; 16]);
        let route = RouteId([2; 16]);
        let generation = TaskGeneration::new(1).unwrap();
        let mut state = RouteState::new(
            epoch,
            route,
            generation,
            RouteRole::Broker,
            16,
            HOST_BULK_BYTE_CREDITS_PER_ROUTE,
            HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
        )
        .unwrap();

        for _ in 0..crate::bulk::BULK_TRANSFERS_PER_ROUTE_MAX {
            for _ in 0..kore_hostlink::BULK_CHUNKS_MAX {
                state.consume_bulk(1, 1).unwrap();
            }
        }
        assert_eq!(
            state.consume_bulk(1, 1),
            Err(kore_hostlink::CreditError::Backpressure)
        );
        state
            .replenish_bulk(
                HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
                HOST_BULK_CHUNK_CREDITS_PER_ROUTE,
            )
            .unwrap();
    }

    #[test]
    fn output_queue_rejects_frames_and_bytes_beyond_its_bounds() {
        let mut queue = BoundedQueue::default();
        for _ in 0..QUEUE_FRAMES_MAX {
            queue.push(vec![0]).unwrap();
        }
        assert!(queue.push(vec![0]).is_err());

        let mut queue = BoundedQueue::default();
        queue.push(vec![0; QUEUE_BYTES_MAX]).unwrap();
        assert!(queue.push(vec![0]).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn configured_control_fragment_is_counted_without_os_short_write() {
        let (writer, _reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        let mut connection = Some(Connection {
            stream: ConnectionStream::Unix(writer),
            decoder: Decoder::new(SocketPlane::Control),
        });
        let mut queue = BoundedQueue::default();
        queue.push(vec![0; 128]).unwrap();
        let mut metrics = Metrics::default();
        assert!(!service_write(&mut connection, &mut queue, &mut metrics, 64).unwrap());
        assert_eq!(metrics.frames_out, 1);
        assert!(metrics.partial_write_calls >= 1);
    }
}
