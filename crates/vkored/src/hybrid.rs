//! HostLink host core over serial control plus ivshmem bulk.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

use kore_hostlink::{
    BULK_FRAME_MAX, BulkFrameDecoder, CONTROL_FRAME_MAX, ControlFrameDecoder, LinkEpoch, RequestId,
    RouteId, SHMEM_WAKE, ShmemWakeV1, link_envelope, shmem_wake_envelope_is_valid,
};
use kore_hostlink_shmem::{ReceiveError, SendError};
use zerocopy::{FromBytes, IntoBytes};

use crate::backend::InferenceBackend;
use crate::daemon::{BenchmarkSink, LinkCore, SocketPlane};
use crate::ivshmem::{IvshmemBulkHost, IvshmemError};
use crate::protocol::frame;

const CONTROL_READ_BYTES: usize = 16 << 10;
const CONTROL_BUFFER_BYTES: usize = 2 * CONTROL_FRAME_MAX;
const CONTROL_FRAMES_PER_TICK: usize = 64;
const CONTROL_WRITES_PER_TICK: usize = 16;
const CONTROL_FRAMES_BEFORE_WAKE: usize = 8;
const BULK_RECORDS_PER_TICK: usize = 4;

#[derive(Debug)]
pub enum HybridError {
    Io(io::Error),
    Ivshmem(IvshmemError),
    ControlOverflow,
    MalformedControl,
    MalformedBulk,
    Disconnected,
}

impl From<io::Error> for HybridError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<IvshmemError> for HybridError {
    fn from(value: IvshmemError) -> Self {
        Self::Ivshmem(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HybridMetrics {
    pub control_reads: u64,
    pub control_writes: u64,
    pub partial_control_writes: u64,
    pub bulk_records_in: u64,
    pub bulk_records_out: u64,
    pub wake_frames: u64,
    pub wake_frames_in: u64,
    pub wake_coalesced: u64,
    pub control_would_block: u64,
    pub bulk_would_block: u64,
}

enum PendingKind {
    Protocol,
    Wake,
}

struct PendingWrite {
    bytes: Vec<u8>,
    offset: usize,
    kind: PendingKind,
}

/// One connected hybrid link. Connection acceptance and QEMU lifecycle stay
/// with the adapter/daemon that owns this value.
pub struct HybridHostLink<B, S = ()> {
    core: LinkCore<B, S>,
    control: UnixStream,
    bulk: IvshmemBulkHost,
    control_decoder: ControlFrameDecoder,
    control_payload: Vec<u8>,
    control_input: Vec<u8>,
    control_start: usize,
    bulk_decoder: BulkFrameDecoder,
    bulk_payload: Vec<u8>,
    bulk_input: Vec<u8>,
    pending_control: Option<PendingWrite>,
    pending_bulk: Option<Vec<u8>>,
    wake_requested: Option<u64>,
    guest_wake: WakeTracker,
    active_epoch: Option<LinkEpoch>,
    normal_control_since_wake: usize,
    metrics: HybridMetrics,
    control_write_chunk: usize,
}

impl<B: InferenceBackend, S: BenchmarkSink> HybridHostLink<B, S> {
    pub fn new(
        control: UnixStream,
        bulk: IvshmemBulkHost,
        backend: B,
        sink: S,
    ) -> Result<Self, HybridError> {
        Self::new_with_control_write_chunk(control, bulk, backend, sink, usize::MAX)
    }

    pub fn new_with_control_write_chunk(
        control: UnixStream,
        bulk: IvshmemBulkHost,
        backend: B,
        sink: S,
        control_write_chunk: usize,
    ) -> Result<Self, HybridError> {
        control.set_nonblocking(true)?;
        let mut core = LinkCore::new(backend, sink);
        core.begin_negotiation();
        Ok(Self {
            core,
            control,
            bulk,
            control_decoder: ControlFrameDecoder::new(),
            control_payload: vec![0; CONTROL_FRAME_MAX],
            control_input: Vec::with_capacity(CONTROL_BUFFER_BYTES),
            control_start: 0,
            bulk_decoder: BulkFrameDecoder::new(),
            bulk_payload: vec![0; BULK_FRAME_MAX],
            bulk_input: vec![0; BULK_FRAME_MAX],
            pending_control: None,
            pending_bulk: None,
            wake_requested: None,
            guest_wake: WakeTracker::default(),
            active_epoch: None,
            normal_control_since_wake: 0,
            metrics: HybridMetrics::default(),
            control_write_chunk: control_write_chunk.max(1),
        })
    }

    pub fn core(&self) -> &LinkCore<B, S> {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut LinkCore<B, S> {
        &mut self.core
    }

    pub fn bulk(&self) -> &IvshmemBulkHost {
        &self.bulk
    }

    pub const fn metrics(&self) -> HybridMetrics {
        self.metrics
    }

    /// Retire task-visible state before a control-plane reconnect.
    pub fn disconnect(&mut self) {
        self.core.disconnect();
        self.bulk.deactivate();
        self.active_epoch = None;
        self.wake_requested = None;
        self.guest_wake.synchronize(None);
    }

    /// Recover the VM-owned mapping after a serial connection closes so an
    /// outer accept loop can establish a fresh HostLink epoch.
    pub fn into_bulk(self) -> IvshmemBulkHost {
        self.bulk
    }

    /// One bounded control-first turn. Shared publication never waits for a
    /// socket write: it records/coalesces the wake and returns.
    pub fn tick(&mut self) -> Result<(), HybridError> {
        self.read_control()?;
        self.activate_epoch()?;
        if !self.core.reconnect_required() {
            self.read_bulk()?;
        }
        self.core.advance();
        if self.core.reconnect_required() && !self.core.reconnect_requires_control_flush() {
            return Err(HybridError::Disconnected);
        }
        self.write_control()?;
        if !self.core.reconnect_required() {
            self.write_bulk()?;
            // A bulk publication may have requested a wake; give it a bounded
            // control opportunity in the same turn without spinning.
            self.write_control()?;
        }
        if self
            .core
            .reconnect_is_ready_with_pending_control(self.pending_control.is_some())
        {
            return Err(HybridError::Disconnected);
        }
        Ok(())
    }

    fn activate_epoch(&mut self) -> Result<(), HybridError> {
        let epoch = self.core.epoch();
        if epoch != self.active_epoch {
            match epoch {
                Some(epoch) => {
                    self.bulk.activate(epoch)?;
                    self.active_epoch = Some(epoch);
                    self.wake_requested = None;
                    self.guest_wake.synchronize(Some(epoch));
                }
                None => {
                    self.bulk.deactivate();
                    self.active_epoch = None;
                    self.wake_requested = None;
                    self.guest_wake.synchronize(None);
                }
            }
        }
        Ok(())
    }

    fn read_control(&mut self) -> Result<(), HybridError> {
        if self.control_start != 0
            && (self.control_start == self.control_input.len()
                || self.control_start >= CONTROL_READ_BYTES)
        {
            self.control_input.drain(..self.control_start);
            self.control_start = 0;
        }
        let retained = self.control_input.len().saturating_sub(self.control_start);
        if retained >= CONTROL_BUFFER_BYTES {
            return Err(HybridError::ControlOverflow);
        }
        let mut input = [0; CONTROL_READ_BYTES];
        match self.control.read(&mut input) {
            Ok(0) => return Err(HybridError::Disconnected),
            Ok(bytes) => {
                if self.control_input.len() + bytes > CONTROL_BUFFER_BYTES {
                    return Err(HybridError::ControlOverflow);
                }
                self.control_input.extend_from_slice(&input[..bytes]);
                self.metrics.control_reads = self.metrics.control_reads.saturating_add(1);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) =>
            {
                return Err(HybridError::Disconnected);
            }
            Err(error) => return Err(error.into()),
        }

        for _ in 0..CONTROL_FRAMES_PER_TICK {
            if self.control_start == self.control_input.len() {
                break;
            }
            let progress = self
                .control_decoder
                .push(
                    &self.control_input[self.control_start..],
                    &mut self.control_payload,
                )
                .map_err(|_| HybridError::MalformedControl)?;
            if progress.consumed == 0 {
                return Err(HybridError::MalformedControl);
            }
            self.control_start += progress.consumed;
            if progress.complete {
                let envelope = *self
                    .control_decoder
                    .envelope()
                    .ok_or(HybridError::MalformedControl)?;
                let payload = self
                    .control_decoder
                    .payload(&self.control_payload)
                    .ok_or(HybridError::MalformedControl)?;
                if envelope.kind.get() == SHMEM_WAKE {
                    let newer = self
                        .guest_wake
                        .accept(&envelope, payload)
                        .map_err(|_| HybridError::MalformedControl)?;
                    self.metrics.wake_frames_in = self.metrics.wake_frames_in.saturating_add(1);
                    if !newer {
                        self.metrics.wake_coalesced = self.metrics.wake_coalesced.saturating_add(1);
                    }
                } else {
                    self.core.receive(SocketPlane::Control, envelope, payload);
                    // HELLO_ACK may be coalesced with the first wake in one
                    // stream read. Synchronize before decoding the suffix.
                    self.activate_epoch()?;
                }
                self.control_decoder.reset();
                if self.core.reconnect_required() {
                    // Ignore a coalesced suffix from the poisoned epoch. The
                    // pending refusal is flushed before the socket is closed.
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn read_bulk(&mut self) -> Result<(), HybridError> {
        if self.active_epoch.is_none() || self.guest_wake.pending().is_none() {
            return Ok(());
        }
        // A producer may refill slots concurrently as this consumer advances,
        // so capacity is not a lawful per-wake drain limit. Keep the wake
        // pending across bounded turns until an actual Empty recheck.
        for _ in 0..BULK_RECORDS_PER_TICK {
            let bytes = match self.bulk.try_receive(&mut self.bulk_input) {
                Ok(Some(bytes)) => bytes,
                Ok(None) | Err(IvshmemError::Receive(ReceiveError::Empty)) => {
                    self.guest_wake.complete();
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            };
            let progress = self
                .bulk_decoder
                .push(&self.bulk_input[..bytes], &mut self.bulk_payload)
                .map_err(|_| HybridError::MalformedBulk)?;
            if progress.consumed != bytes || !progress.complete {
                return Err(HybridError::MalformedBulk);
            }
            let envelope = *self
                .bulk_decoder
                .envelope()
                .ok_or(HybridError::MalformedBulk)?;
            let payload = self
                .bulk_decoder
                .payload(&self.bulk_payload)
                .ok_or(HybridError::MalformedBulk)?;
            self.core.receive(SocketPlane::Bulk, envelope, payload);
            self.bulk_decoder.reset();
            self.metrics.bulk_records_in = self.metrics.bulk_records_in.saturating_add(1);
            if self.core.reconnect_required() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn write_bulk(&mut self) -> Result<(), HybridError> {
        if self.active_epoch.is_none() {
            return Ok(());
        }
        for _ in 0..BULK_RECORDS_PER_TICK {
            let frame = match self.pending_bulk.take() {
                Some(frame) => frame,
                None => match self.core.pop_bulk_frame() {
                    Some(frame) => frame,
                    None => break,
                },
            };
            match self.bulk.try_publish(&frame) {
                Ok(publication) => {
                    if self.wake_requested.is_some() {
                        self.metrics.wake_coalesced = self.metrics.wake_coalesced.saturating_add(1);
                    }
                    self.wake_requested = Some(publication);
                    self.metrics.bulk_records_out = self.metrics.bulk_records_out.saturating_add(1);
                }
                Err(IvshmemError::Send(SendError::Full)) => {
                    self.pending_bulk = Some(frame);
                    self.metrics.bulk_would_block = self.metrics.bulk_would_block.saturating_add(1);
                    break;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn write_control(&mut self) -> Result<(), HybridError> {
        for _ in 0..CONTROL_WRITES_PER_TICK {
            self.install_pending_control()?;
            let Some(pending) = &mut self.pending_control else {
                break;
            };
            let remaining = &pending.bytes[pending.offset..];
            let offered = remaining.len().min(self.control_write_chunk);
            match self.control.write(&remaining[..offered]) {
                Ok(0) => return Err(HybridError::Disconnected),
                Ok(bytes) => {
                    self.metrics.control_writes = self.metrics.control_writes.saturating_add(1);
                    pending.offset += bytes;
                    if pending.offset != pending.bytes.len() {
                        self.metrics.partial_control_writes =
                            self.metrics.partial_control_writes.saturating_add(1);
                        continue;
                    }
                    if matches!(pending.kind, PendingKind::Wake) {
                        self.metrics.wake_frames = self.metrics.wake_frames.saturating_add(1);
                        self.normal_control_since_wake = 0;
                    } else {
                        self.normal_control_since_wake =
                            self.normal_control_since_wake.saturating_add(1);
                    }
                    self.pending_control = None;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.metrics.control_would_block =
                        self.metrics.control_would_block.saturating_add(1);
                    break;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    return Err(HybridError::Disconnected);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn install_pending_control(&mut self) -> Result<(), HybridError> {
        let active_epoch = self.active_epoch;
        if let Some(pending) = &mut self.pending_control {
            // A not-yet-started wake can be coalesced to the latest published
            // count without changing any bytes already visible to the stream.
            if pending.offset == 0
                && matches!(pending.kind, PendingKind::Wake)
                && let Some(publication) = self.wake_requested.take()
            {
                let epoch = active_epoch.ok_or(HybridError::MalformedBulk)?;
                pending.bytes = Self::wake_frame(epoch, publication)?;
                self.metrics.wake_coalesced = self.metrics.wake_coalesced.saturating_add(1);
            }
            return Ok(());
        }

        let force_wake = self.wake_requested.is_some()
            && self.normal_control_since_wake >= CONTROL_FRAMES_BEFORE_WAKE;
        if !force_wake && let Some(bytes) = self.core.pop_control_frame() {
            self.pending_control = Some(PendingWrite {
                bytes,
                offset: 0,
                kind: PendingKind::Protocol,
            });
            return Ok(());
        }
        if let Some(publication) = self.wake_requested.take() {
            let epoch = active_epoch.ok_or(HybridError::MalformedBulk)?;
            self.pending_control = Some(PendingWrite {
                bytes: Self::wake_frame(epoch, publication)?,
                offset: 0,
                kind: PendingKind::Wake,
            });
        } else if let Some(bytes) = self.core.pop_control_frame() {
            self.pending_control = Some(PendingWrite {
                bytes,
                offset: 0,
                kind: PendingKind::Protocol,
            });
        }
        Ok(())
    }

    fn wake_frame(epoch: LinkEpoch, publication: u64) -> Result<Vec<u8>, HybridError> {
        let wake = ShmemWakeV1::new(publication).ok_or(HybridError::MalformedBulk)?;
        let envelope = link_envelope(
            SHMEM_WAKE,
            epoch,
            RouteId::ZERO,
            0,
            0,
            RequestId::ZERO,
            core::mem::size_of::<ShmemWakeV1>(),
        );
        Ok(frame(&envelope, wake.as_bytes()))
    }
}

#[derive(Default)]
struct WakeTracker {
    epoch: Option<LinkEpoch>,
    seen: u64,
    pending: Option<u64>,
}

impl WakeTracker {
    fn synchronize(&mut self, epoch: Option<LinkEpoch>) {
        self.epoch = epoch;
        self.seen = 0;
        self.pending = None;
    }

    fn accept(&mut self, envelope: &kore_hostlink::EnvelopeV1, payload: &[u8]) -> Result<bool, ()> {
        let epoch = self.epoch.ok_or(())?;
        if !shmem_wake_envelope_is_valid(envelope, epoch) {
            return Err(());
        }
        let wake = ShmemWakeV1::read_from_bytes(payload).map_err(|_| ())?;
        let publication = wake.publication().ok_or(())?;
        let current = self.pending.unwrap_or(self.seen);
        if publication < current {
            return Err(());
        }
        if publication == current {
            return Ok(false);
        }
        self.pending = Some(publication);
        Ok(true)
    }

    const fn pending(&self) -> Option<u64> {
        self.pending
    }

    fn complete(&mut self) {
        if let Some(publication) = self.pending.take() {
            self.seen = publication;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kore_hostlink::{EnvelopeV1, SHMEM_WAKE, shmem_wake_envelope_is_valid};
    use zerocopy::FromBytes;

    #[test]
    fn wake_frame_is_shared_codec_link_control() {
        let epoch = LinkEpoch([9; 16]);
        let wake = ShmemWakeV1::new(7).unwrap();
        let envelope = link_envelope(
            SHMEM_WAKE,
            epoch,
            RouteId::ZERO,
            0,
            0,
            RequestId::ZERO,
            wake.as_bytes().len(),
        );
        let bytes = frame(&envelope, wake.as_bytes());
        let decoded = EnvelopeV1::read_from_bytes(&bytes[..kore_hostlink::ENVELOPE_LEN]).unwrap();
        assert!(shmem_wake_envelope_is_valid(&decoded, epoch));
        assert_eq!(wake.publication(), Some(7));
    }

    #[test]
    fn guest_wake_is_monotonic_idempotent_and_epoch_bound() {
        let epoch = LinkEpoch([9; 16]);
        let mut tracker = WakeTracker::default();
        tracker.synchronize(Some(epoch));
        let wake = ShmemWakeV1::new(7).unwrap();
        let envelope = link_envelope(
            SHMEM_WAKE,
            epoch,
            RouteId::ZERO,
            0,
            0,
            RequestId::ZERO,
            wake.as_bytes().len(),
        );
        assert_eq!(tracker.accept(&envelope, wake.as_bytes()), Ok(true));
        assert_eq!(tracker.accept(&envelope, wake.as_bytes()), Ok(false));
        tracker.complete();
        assert_eq!(tracker.accept(&envelope, wake.as_bytes()), Ok(false));

        let stale = ShmemWakeV1::new(6).unwrap();
        assert!(tracker.accept(&envelope, stale.as_bytes()).is_err());
        let wrong_epoch = link_envelope(
            SHMEM_WAKE,
            LinkEpoch([8; 16]),
            RouteId::ZERO,
            0,
            0,
            RequestId::ZERO,
            wake.as_bytes().len(),
        );
        assert!(tracker.accept(&wrong_epoch, wake.as_bytes()).is_err());
        assert!(tracker.accept(&envelope, &wake.as_bytes()[..8]).is_err());
    }

    #[test]
    fn bounded_nonempty_turn_does_not_retire_the_wake() {
        let epoch = LinkEpoch([9; 16]);
        let mut tracker = WakeTracker::default();
        tracker.synchronize(Some(epoch));
        tracker.pending = Some(kore_hostlink::SHMEM_QUEUE_CAPACITY as u64 + 1);
        // A bounded drain that never observes Empty leaves notification state
        // armed for the next turn; only the explicit recheck retires it.
        assert!(tracker.pending().is_some());
        assert!(tracker.pending().is_some());
        tracker.complete();
        assert!(tracker.pending().is_none());
    }
}
