//! Capability-rooted workspace backend for the dedicated HostLink route.
//!
//! The selected host path is consumed by `LiveRoot::open`; this adapter sees
//! only canonical workspace RPC records thereafter. Inference routes remain
//! delegated to the deterministic MVP backend and cannot address workspace
//! objects.

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use kore_hostlink::{BulkId, RequestId, RouteId, WORKSPACE_REPLY};
use kore_workspace_sync::{
    ATTACH_MAGIC, AttachRequestV1, AttachResponseV1, BLOB_STORED_MAGIC, BlobStoredV1,
    DELTA_OP_REQUEST_MAGIC, DELTA_POLL_MAGIC, DELTA_RESPONSE_MAGIC, DeltaOpRequestV1, DeltaPollV1,
    DeltaResponseV1, ENTRY_REQUEST_MAGIC, ERROR_RESPONSE_MAGIC, FORMAT_VERSION, GrantOperations,
    ManifestEntryRequestV1, ManifestEntryResponseV1, Policy, RANGE_REQUEST_MAGIC,
    RANGE_RESPONSE_MAGIC, RangeRequestV1, RangeResponseV1, StandingGrant, TRANSACTION_RESULT_MAGIC,
    TopologyDelta, TransactionOutcome, TransactionResultV1, WORKSPACE_BLOB_CONTENT,
    WORKSPACE_TRANSACTION_CONTENT, WRITE_TRANSACTION_MAGIC, WorkspaceErrorV1, WorkspaceMode,
    WorkspaceStatus, decode_write_transaction, encode_delta_op,
};
use workspacectl::{Error as WorkspaceIoError, LiveRoot};
use zerocopy::byteorder::little_endian::{U16, U64};
use zerocopy::{FromBytes, IntoBytes};

use crate::backend::{
    BackendError, BackendEvent, BackendMetrics, BackendRole, DeterministicBackend,
    InferenceBackend, RequestUsage, SequenceIdentity, SequenceMetrics,
};

const EVENTS_MAX: usize = 16;
const BLOBS_MAX: usize = 4;

struct StagedBlob {
    bulk_id: BulkId,
    digest: [u8; 32],
    bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum WorkspaceBackendError {
    Workspace(WorkspaceIoError),
}

impl From<WorkspaceIoError> for WorkspaceBackendError {
    fn from(value: WorkspaceIoError) -> Self {
        Self::Workspace(value)
    }
}

pub struct WorkspaceBackend<B = DeterministicBackend> {
    inference: B,
    workspace: LiveRoot,
    workspace_route: Option<SequenceIdentity>,
    pending_delta: Option<TopologyDelta>,
    events: VecDeque<BackendEvent>,
    grant: Option<StandingGrant>,
    blobs: Vec<StagedBlob>,
}

impl WorkspaceBackend<DeterministicBackend> {
    pub fn open(root: &Path, latency_ticks: u64) -> Result<Self, WorkspaceBackendError> {
        Self::open_with_writeback(root, None, latency_ticks, false)
    }

    pub fn open_with_writeback(
        root: &Path,
        state: Option<&Path>,
        latency_ticks: u64,
        allow_writeback: bool,
    ) -> Result<Self, WorkspaceBackendError> {
        Self::open_with_inference(
            root,
            state,
            allow_writeback,
            DeterministicBackend::new(latency_ticks),
        )
    }
}

impl<B> WorkspaceBackend<B> {
    /// Compose the dedicated workspace route with any bounded inference
    /// backend. The workspace authority remains in `LiveRoot`; the inference
    /// backend receives only its broker/agent protocol inputs.
    pub fn open_with_inference(
        root: &Path,
        state: Option<&Path>,
        allow_writeback: bool,
        inference: B,
    ) -> Result<Self, WorkspaceBackendError> {
        let workspace = match state {
            Some(state) => LiveRoot::open_with_state(root, [0; 32], state)?,
            None if allow_writeback => {
                return Err(WorkspaceBackendError::Workspace(
                    WorkspaceIoError::InvalidArtifact,
                ));
            }
            None => LiveRoot::open(root, [0; 32])?,
        };
        let grant = if allow_writeback {
            Some(
                StandingGrant::new(
                    fresh_grant_id(workspace.manifest().snapshot_id()),
                    workspace.manifest().snapshot_id(),
                    GrantOperations::ALL,
                    kore_workspace_sync::bounds::MAX_CHANGES as u16,
                    kore_workspace_sync::bounds::MAX_OVERLAY_BYTES,
                    0,
                    &[],
                )
                .expect("frozen writeback grant geometry is valid"),
            )
        } else {
            None
        };
        Ok(Self {
            inference,
            workspace,
            workspace_route: None,
            pending_delta: None,
            events: VecDeque::with_capacity(EVENTS_MAX),
            grant,
            blobs: Vec::with_capacity(BLOBS_MAX),
        })
    }

    fn enqueue(&mut self, event: BackendEvent) -> Result<(), BackendError> {
        if self.events.len() == EVENTS_MAX {
            return Err(BackendError::Full);
        }
        self.events.push_back(event);
        Ok(())
    }

    fn inline(
        &mut self,
        route: RouteId,
        request: RequestId,
        payload: Vec<u8>,
        tick: u64,
    ) -> Result<(), BackendError> {
        let sequence = self.workspace_identity(route)?;
        self.enqueue(BackendEvent::Completed {
            sequence,
            request,
            payload,
            part: 0,
            submitted_tick: tick,
            usage: RequestUsage::default(),
            turn: 0,
        })
    }

    fn workspace_identity(&self, route: RouteId) -> Result<SequenceIdentity, BackendError> {
        self.workspace_route
            .filter(|identity| identity.route == route)
            .ok_or(BackendError::InvalidRole)
    }

    fn error(
        &mut self,
        route: RouteId,
        request: RequestId,
        request_magic: [u8; 4],
        status: WorkspaceStatus,
        tick: u64,
    ) -> Result<(), BackendError> {
        self.inline(
            route,
            request,
            WorkspaceErrorV1 {
                magic: ERROR_RESPONSE_MAGIC,
                version: U16::new(FORMAT_VERSION),
                status: U16::new(status as u16),
                request_magic,
                reserved: zerocopy::byteorder::little_endian::U32::ZERO,
            }
            .as_bytes()
            .to_vec(),
            tick,
        )
    }

    fn submit_workspace(
        &mut self,
        route: RouteId,
        request: RequestId,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        let magic = payload
            .get(..4)
            .and_then(|bytes| bytes.try_into().ok())
            .unwrap_or([0; 4]);
        match magic {
            ATTACH_MAGIC => {
                let Ok(request_wire) = AttachRequestV1::read_from_bytes(payload) else {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                };
                if request_wire.validate() != Ok(WorkspaceMode::Live)
                    || request_wire.policy_digest != Policy::default().digest()
                    || (request_wire.known_snapshot != [0; 32]
                        && request_wire.known_snapshot != self.workspace.manifest().snapshot_id())
                {
                    return self.error(route, request, magic, WorkspaceStatus::Stale, tick);
                }
                let header = self.workspace.manifest().header();
                let (
                    write_grant,
                    write_operations,
                    write_max_operations,
                    write_max_bytes,
                    write_expires_at,
                ) = self.grant.as_ref().map_or(([0; 16], 0, 0, 0, 0), |grant| {
                    (
                        grant.id(),
                        grant.operations().bits(),
                        grant.max_operations(),
                        grant.max_bytes(),
                        grant.expires_at(),
                    )
                });
                self.inline(
                    route,
                    request,
                    AttachResponseV1 {
                        magic: ATTACH_MAGIC,
                        version: U16::new(FORMAT_VERSION),
                        status: U16::new(WorkspaceStatus::Ok as u16),
                        snapshot: header.snapshot_id,
                        parent_snapshot: header.parent_snapshot,
                        root_digest: header.root_digest,
                        policy_digest: header.policy_digest,
                        entry_count: header.entry_count,
                        directory_count: header.directory_count,
                        total_bytes: header.total_bytes,
                        delta_sequence: U64::new(self.workspace.next_delta_sequence()),
                        write_grant,
                        write_operations,
                        reserved0: 0,
                        write_max_operations: U16::new(write_max_operations),
                        write_max_bytes: U64::new(write_max_bytes),
                        write_expires_at: U64::new(write_expires_at),
                    }
                    .as_bytes()
                    .to_vec(),
                    tick,
                )
            }
            ENTRY_REQUEST_MAGIC => {
                let Ok(request_wire) = ManifestEntryRequestV1::read_from_bytes(payload) else {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                };
                if request_wire.validate().is_err()
                    || request_wire.snapshot != self.workspace.manifest().snapshot_id()
                {
                    return self.error(route, request, magic, WorkspaceStatus::Stale, tick);
                }
                let index = usize::from(request_wire.index.get());
                let Some(entry) = self.workspace.manifest().entries().get(index) else {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                };
                let Some(response) = ManifestEntryResponseV1::from_entry(
                    self.workspace.manifest().snapshot_id(),
                    request_wire.index.get(),
                    entry,
                ) else {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                };
                self.inline(route, request, response.as_bytes().to_vec(), tick)
            }
            RANGE_REQUEST_MAGIC => {
                let Ok(request_wire) = RangeRequestV1::read_from_bytes(payload) else {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                };
                let served = match self.workspace.read_range(&request_wire) {
                    Ok(served) => served,
                    Err(WorkspaceIoError::Conflict | WorkspaceIoError::SourceChanged(_)) => {
                        return self.error(route, request, magic, WorkspaceStatus::Stale, tick);
                    }
                    Err(_) => {
                        return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                    }
                };
                let bulk_id = range_bulk_id(request, &request_wire);
                let mut siblings = [[0; 32]; kore_workspace_sync::MAX_RANGE_PROOF_DEPTH];
                siblings[..served.proof.siblings().len()].copy_from_slice(served.proof.siblings());
                let response = RangeResponseV1 {
                    magic: RANGE_RESPONSE_MAGIC,
                    version: U16::new(FORMAT_VERSION),
                    status: U16::new(WorkspaceStatus::Ok as u16),
                    snapshot: request_wire.snapshot,
                    source: request_wire.source,
                    version_digest: request_wire.version_digest,
                    bulk_id: bulk_id.0,
                    file_len: request_wire.file_len,
                    range_index: U16::new(served.proof.leaf_index),
                    range_count: U16::new(served.proof.leaf_count),
                    proof_depth: served.proof.siblings().len() as u8,
                    reserved: [0; 3],
                    siblings,
                };
                let mut bytes = Vec::with_capacity(response.as_bytes().len() + served.bytes.len());
                bytes.extend_from_slice(response.as_bytes());
                bytes.extend_from_slice(&served.bytes);
                self.enqueue(BackendEvent::BulkCompleted {
                    sequence: self.workspace_identity(route)?,
                    request,
                    bulk_id,
                    content_kind: WORKSPACE_REPLY,
                    payload: bytes,
                    submitted_tick: tick,
                })
            }
            DELTA_POLL_MAGIC => {
                let Ok(request_wire) = DeltaPollV1::read_from_bytes(payload) else {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                };
                if request_wire.validate().is_err()
                    || request_wire.snapshot != self.workspace.manifest().snapshot_id()
                    || request_wire.next_sequence.get() != self.workspace.next_delta_sequence()
                {
                    return self.error(route, request, magic, WorkspaceStatus::Stale, tick);
                }
                match self.workspace.rescan([0; 32]) {
                    Ok(Some(delta)) => {
                        let response = DeltaResponseV1 {
                            magic: DELTA_RESPONSE_MAGIC,
                            version: U16::new(FORMAT_VERSION),
                            status: U16::new(WorkspaceStatus::Ok as u16),
                            parent: delta.parent,
                            next: delta.next,
                            sequence: U64::new(delta.sequence),
                            delta_id: delta.id,
                            origin_transaction: delta.origin_transaction,
                            change_count: U16::new(delta.ops().len() as u16),
                            reserved: U16::ZERO,
                        };
                        self.pending_delta = Some(delta);
                        self.inline(route, request, response.as_bytes().to_vec(), tick)
                    }
                    Ok(None) => {
                        self.error(route, request, magic, WorkspaceStatus::Unavailable, tick)
                    }
                    Err(_) => self.error(route, request, magic, WorkspaceStatus::Stale, tick),
                }
            }
            DELTA_OP_REQUEST_MAGIC => {
                let Ok(request_wire) = DeltaOpRequestV1::read_from_bytes(payload) else {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                };
                if request_wire.validate().is_err() {
                    return self.error(route, request, magic, WorkspaceStatus::Refused, tick);
                }
                let Some(delta) = self.pending_delta.as_ref() else {
                    return self.error(route, request, magic, WorkspaceStatus::Stale, tick);
                };
                if request_wire.sequence.get() != delta.sequence
                    || request_wire.delta_id != delta.id
                    || usize::from(request_wire.index.get()) >= delta.ops().len()
                {
                    return self.error(route, request, magic, WorkspaceStatus::Stale, tick);
                }
                let mut bytes = [0; 1024];
                let len = encode_delta_op(delta, request_wire.index.get(), &mut bytes)
                    .map_err(|_| BackendError::Full)?;
                self.enqueue(BackendEvent::BulkCompleted {
                    sequence: self.workspace_identity(route)?,
                    request,
                    bulk_id: delta_op_bulk_id(request, delta.id, request_wire.index.get()),
                    content_kind: WORKSPACE_REPLY,
                    payload: bytes[..len].to_vec(),
                    submitted_tick: tick,
                })
            }
            _ => self.error(route, request, magic, WorkspaceStatus::Refused, tick),
        }
    }
}

fn fresh_grant_id(snapshot: [u8; 32]) -> [u8; 16] {
    let mut hash = blake3::Hasher::new();
    hash.update(b"kore.host-workspace.grant.v1\0");
    hash.update(&snapshot);
    hash.update(&std::process::id().to_le_bytes());
    hash.update(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    );
    let mut id = [0; 16];
    id.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    if id == [0; 16] {
        id[0] = 1;
    }
    id
}

impl<B: InferenceBackend> InferenceBackend for WorkspaceBackend<B> {
    fn reset(&mut self) {
        self.inference.reset();
        self.workspace_route = None;
        self.pending_delta = None;
        self.events.clear();
        self.blobs.clear();
    }

    fn open(&mut self, identity: SequenceIdentity) -> Result<(), BackendError> {
        if identity.role == BackendRole::Workspace {
            if self.workspace_route.is_some() {
                return Err(BackendError::RoleCapacity);
            }
            self.workspace_route = Some(identity);
        } else {
            self.inference.open(identity)?;
        }
        Ok(())
    }

    fn close(&mut self, identity: SequenceIdentity) -> Result<(), BackendError> {
        if self.workspace_route == Some(identity) {
            self.events.retain(|event| event.sequence() != identity);
            self.workspace_route = None;
            Ok(())
        } else {
            self.inference.close(identity)
        }
    }

    fn submit(
        &mut self,
        identity: SequenceIdentity,
        request: RequestId,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        if self.workspace_route == Some(identity) {
            self.submit_workspace(identity.route, request, payload, tick)
        } else {
            self.inference.submit(identity, request, payload, tick)
        }
    }

    fn submit_bulk(
        &mut self,
        identity: SequenceIdentity,
        request: RequestId,
        bulk_id: BulkId,
        content_kind: u16,
        payload: &[u8],
        tick: u64,
    ) -> Result<(), BackendError> {
        if self.workspace_route != Some(identity) {
            if content_kind != kore_hostlink::INFERENCE_REQUEST_CONTENT {
                return Err(BackendError::InvalidRole);
            }
            return self.inference.submit(identity, request, payload, tick);
        }
        let route = identity.route;
        match content_kind {
            WORKSPACE_BLOB_CONTENT => {
                if payload.is_empty()
                    || payload.len() as u64 > kore_workspace_sync::bounds::MAX_FILE_BYTES
                {
                    return self.error(
                        route,
                        request,
                        WRITE_TRANSACTION_MAGIC,
                        WorkspaceStatus::Refused,
                        tick,
                    );
                }
                let digest = *blake3::hash(payload).as_bytes();
                if let Some(existing) = self.blobs.iter().find(|blob| blob.bulk_id == bulk_id) {
                    if existing.digest != digest || existing.bytes != payload {
                        return self.error(
                            route,
                            request,
                            WRITE_TRANSACTION_MAGIC,
                            WorkspaceStatus::Conflict,
                            tick,
                        );
                    }
                } else {
                    if self.blobs.len() == BLOBS_MAX {
                        return self.error(
                            route,
                            request,
                            WRITE_TRANSACTION_MAGIC,
                            WorkspaceStatus::Refused,
                            tick,
                        );
                    }
                    self.blobs.push(StagedBlob {
                        bulk_id,
                        digest,
                        bytes: payload.to_vec(),
                    });
                }
                self.inline(
                    route,
                    request,
                    BlobStoredV1 {
                        magic: BLOB_STORED_MAGIC,
                        version: U16::new(FORMAT_VERSION),
                        status: U16::new(WorkspaceStatus::Ok as u16),
                        bulk_id: bulk_id.0,
                        digest,
                        length: zerocopy::byteorder::little_endian::U32::new(payload.len() as u32),
                        reserved: zerocopy::byteorder::little_endian::U32::ZERO,
                    }
                    .as_bytes()
                    .to_vec(),
                    tick,
                )
            }
            WORKSPACE_TRANSACTION_CONTENT => {
                let transaction = match decode_write_transaction(payload) {
                    Ok(transaction) => transaction,
                    Err(_) => {
                        return self.error(
                            route,
                            request,
                            WRITE_TRANSACTION_MAGIC,
                            WorkspaceStatus::Refused,
                            tick,
                        );
                    }
                };
                let Some(grant) = self.grant.as_mut() else {
                    return self.error(
                        route,
                        request,
                        WRITE_TRANSACTION_MAGIC,
                        WorkspaceStatus::Refused,
                        tick,
                    );
                };
                if grant.id() != transaction.grant {
                    return self.error(
                        route,
                        request,
                        WRITE_TRANSACTION_MAGIC,
                        WorkspaceStatus::Refused,
                        tick,
                    );
                }
                let mut contents = BTreeMap::new();
                for change in transaction.changes.changes().iter().filter(|change| {
                    matches!(
                        change.kind,
                        kore_workspace_sync::ChangeKind::CreateFile
                            | kore_workspace_sync::ChangeKind::ModifyFile
                    )
                }) {
                    if change.replacement_len == 0
                        && change.replacement == *blake3::hash(&[]).as_bytes()
                    {
                        contents.insert(change.path.clone(), Vec::new());
                        continue;
                    }
                    if let Some(blob) = self.blobs.iter().find(|blob| {
                        blob.digest == change.replacement
                            && blob.bytes.len() == change.replacement_len as usize
                    }) {
                        contents.insert(change.path.clone(), blob.bytes.clone());
                    }
                }
                // `LiveRoot` consults its durable terminal-result journal
                // before validating content. Therefore a byte-identical
                // transaction retry succeeds after its staging blob has been
                // retired, while a new transaction with a missing blob still
                // fails closed during content validation.
                let outcome = self
                    .workspace
                    .apply_transaction(transaction.id, &transaction.changes, &contents, grant, tick)
                    .unwrap_or(TransactionOutcome::Refused);
                let (status, snapshot, applied) = match outcome {
                    TransactionOutcome::Applied { snapshot } => (
                        WorkspaceStatus::Ok,
                        snapshot,
                        transaction.changes.changes().len() as u16,
                    ),
                    TransactionOutcome::Conflict => (WorkspaceStatus::Conflict, [0; 32], 0),
                    TransactionOutcome::Partial { applied } => {
                        (WorkspaceStatus::Partial, [0; 32], applied)
                    }
                    TransactionOutcome::Refused => (WorkspaceStatus::Refused, [0; 32], 0),
                };
                if status == WorkspaceStatus::Ok {
                    self.blobs.retain(|blob| {
                        !transaction
                            .changes
                            .changes()
                            .iter()
                            .any(|change| change.replacement == blob.digest)
                    });
                }
                self.inline(
                    route,
                    request,
                    TransactionResultV1 {
                        magic: TRANSACTION_RESULT_MAGIC,
                        version: U16::new(FORMAT_VERSION),
                        status: U16::new(status as u16),
                        transaction: transaction.id,
                        changes: transaction.changes.id,
                        snapshot,
                        applied: U16::new(applied),
                        reserved: U16::ZERO,
                    }
                    .as_bytes()
                    .to_vec(),
                    tick,
                )
            }
            _ => self.error(
                route,
                request,
                WRITE_TRANSACTION_MAGIC,
                WorkspaceStatus::Refused,
                tick,
            ),
        }
    }

    fn cancel(
        &mut self,
        identity: SequenceIdentity,
        request: RequestId,
        tick: u64,
    ) -> Result<BackendEvent, BackendError> {
        if self.workspace_route == Some(identity) {
            let Some(index) = self
                .events
                .iter()
                .position(|event| event.sequence() == identity && event.request() == request)
            else {
                return Err(BackendError::RequestUnknown);
            };
            let event = self
                .events
                .remove(index)
                .expect("located queued workspace event");
            return Ok(BackendEvent::Cancelled {
                sequence: identity,
                request,
                submitted_tick: event_tick(&event),
                turn: 0,
            });
        }
        self.inference.cancel(identity, request, tick)
    }

    fn poll(&mut self, tick: u64) -> Option<BackendEvent> {
        self.events
            .pop_front()
            .or_else(|| self.inference.poll(tick))
    }

    fn metrics(&self) -> BackendMetrics {
        self.inference.metrics()
    }

    fn sequence_metrics(&self, identity: SequenceIdentity) -> Option<SequenceMetrics> {
        if self.workspace_route == Some(identity) {
            Some(SequenceMetrics::default())
        } else {
            self.inference.sequence_metrics(identity)
        }
    }

    fn take_observation(&mut self) -> Option<crate::backend::InferenceObservation> {
        self.inference.take_observation()
    }
}

fn range_bulk_id(request: RequestId, range: &RangeRequestV1) -> BulkId {
    let mut hash = blake3::Hasher::new();
    hash.update(b"kore.workspace.range-bulk.v1\0");
    hash.update(&request.0);
    hash.update(&range.snapshot);
    hash.update(&range.source);
    hash.update(&range.offset.get().to_le_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    if id == [0; 16] {
        id[0] = 1;
    }
    BulkId(id)
}

fn delta_op_bulk_id(request: RequestId, delta: [u8; 32], index: u16) -> BulkId {
    let mut hash = blake3::Hasher::new();
    hash.update(b"kore.workspace.delta-bulk.v1\0");
    hash.update(&request.0);
    hash.update(&delta);
    hash.update(&index.to_le_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    if id == [0; 16] {
        id[0] = 1;
    }
    BulkId(id)
}

fn event_tick(event: &BackendEvent) -> u64 {
    match event {
        BackendEvent::Chunk { submitted_tick, .. }
        | BackendEvent::Completed { submitted_tick, .. }
        | BackendEvent::BulkCompleted { submitted_tick, .. }
        | BackendEvent::Cancelled { submitted_tick, .. }
        | BackendEvent::Failed { submitted_tick, .. } => *submitted_tick,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kore_workspace_sync::{
        Change, ChangeKind, ChangeSetBuilder, ENTRY_RESPONSE_MAGIC, ManifestEntryResponseV1,
        TransactionResultV1, WriteTransaction, encode_write_transaction,
    };

    fn workspace_sequence(route: RouteId) -> SequenceIdentity {
        SequenceIdentity {
            route,
            generation: kore_hostlink::TaskGeneration::new(1).unwrap(),
            role: BackendRole::Workspace,
        }
    }

    #[test]
    fn workspace_route_gets_manifest_and_verified_bulk_while_agent_route_cannot() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), vec![7; 5000]).unwrap();
        let mut backend = WorkspaceBackend::open(root.path(), 1).unwrap();
        let workspace_route = RouteId([1; 16]);
        let workspace = workspace_sequence(workspace_route);
        backend.open(workspace).unwrap();
        let attach = AttachRequestV1 {
            magic: ATTACH_MAGIC,
            version: U16::new(FORMAT_VERSION),
            mode: WorkspaceMode::Live as u8,
            flags: 0,
            policy_digest: Policy::default().digest(),
            known_snapshot: [0; 32],
        };
        backend
            .submit(workspace, RequestId([2; 16]), attach.as_bytes(), 1)
            .unwrap();
        let BackendEvent::Completed { payload, .. } = backend.poll(1).unwrap() else {
            panic!("attach must be inline");
        };
        let response = AttachResponseV1::read_from_bytes(&payload).unwrap();
        assert_eq!(response.validate(), Ok(WorkspaceStatus::Ok));

        let entry_request = ManifestEntryRequestV1 {
            magic: ENTRY_REQUEST_MAGIC,
            version: U16::new(FORMAT_VERSION),
            index: U16::ZERO,
            snapshot: response.snapshot,
        };
        backend
            .submit(workspace, RequestId([3; 16]), entry_request.as_bytes(), 2)
            .unwrap();
        let BackendEvent::Completed { payload, .. } = backend.poll(2).unwrap() else {
            panic!("entry must be inline");
        };
        assert_eq!(&payload[..4], &ENTRY_RESPONSE_MAGIC);
        let entry_response = ManifestEntryResponseV1::read_from_bytes(&payload).unwrap();
        let (_, entry) = entry_response.validate().unwrap();
        assert_eq!(entry.unwrap().path.as_str(), "file");
    }

    #[test]
    fn workspace_bulk_applies_and_replays_one_idempotent_standing_grant_write() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), b"before").unwrap();
        let mut backend =
            WorkspaceBackend::open_with_writeback(root.path(), Some(state.path()), 1, true)
                .unwrap();
        let route = RouteId([1; 16]);
        let workspace = workspace_sequence(route);
        backend.open(workspace).unwrap();
        let attach = AttachRequestV1 {
            magic: ATTACH_MAGIC,
            version: U16::new(FORMAT_VERSION),
            mode: WorkspaceMode::Live as u8,
            flags: 0,
            policy_digest: Policy::default().digest(),
            known_snapshot: [0; 32],
        };
        backend
            .submit(workspace, RequestId([2; 16]), attach.as_bytes(), 1)
            .unwrap();
        let BackendEvent::Completed { payload, .. } = backend.poll(1).unwrap() else {
            panic!("attach must complete");
        };
        let response = AttachResponseV1::read_from_bytes(&payload).unwrap();
        assert_ne!(response.write_grant, [0; 16]);

        let replacement = b"after automatic writeback";
        let replacement_digest = *blake3::hash(replacement).as_bytes();
        let blob_id = BulkId([4; 16]);
        backend
            .submit_bulk(
                workspace,
                RequestId([3; 16]),
                blob_id,
                WORKSPACE_BLOB_CONTENT,
                replacement,
                2,
            )
            .unwrap();
        assert!(matches!(
            backend.poll(2),
            Some(BackendEvent::Completed { .. })
        ));

        let entry = backend.workspace.manifest().entries()[0].clone();
        let mut changes = ChangeSetBuilder::new(response.snapshot);
        changes
            .push(Change {
                kind: ChangeKind::ModifyFile,
                path: entry.path,
                target: None,
                expected: entry.digest,
                replacement: replacement_digest,
                replacement_len: replacement.len() as u32,
                executable: false,
            })
            .unwrap();
        let transaction = WriteTransaction {
            id: [9; 32],
            grant: response.write_grant,
            changes: changes.finish().unwrap(),
        };
        let mut encoded = [0u8; kore_workspace_sync::MAX_WRITE_TRANSACTION_BYTES];
        let len = encode_write_transaction(&transaction, &mut encoded).unwrap();
        for request in [RequestId([5; 16]), RequestId([6; 16])] {
            backend
                .submit_bulk(
                    workspace,
                    request,
                    BulkId([7; 16]),
                    WORKSPACE_TRANSACTION_CONTENT,
                    &encoded[..len],
                    3,
                )
                .unwrap();
            let BackendEvent::Completed { payload, .. } = backend.poll(3).unwrap() else {
                panic!("transaction must complete inline");
            };
            let result = TransactionResultV1::read_from_bytes(&payload)
                .unwrap_or_else(|_| panic!("unexpected transaction reply: {payload:?}"));
            assert_eq!(result.validate(), Ok(WorkspaceStatus::Ok));
        }
        assert_eq!(
            std::fs::read(root.path().join("file")).unwrap(),
            replacement
        );
    }
}
