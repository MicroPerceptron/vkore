use hostlinkd::backend::BackendRole;
use hostlinkd::daemon::{BenchmarkEvent, LinkCore, SocketPlane};
use hostlinkd::harness::{Fault, FrameHarness, decode_envelope};
use hostlinkd::protocol::*;
use hostlinkd::{
    BackendError, BackendEvent, BackendMetrics, BenchmarkSink, DeterministicBackend,
    InferenceBackend, RequestUsage, SequenceIdentity, SequenceMetrics,
};
use kore_hostlink::{
    BulkId, BulkRefV1, LinkEpoch, RequestId, RouteId, TaskGeneration, derive_link_epoch,
};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zerocopy::IntoBytes;
use zerocopy::byteorder::little_endian::{U16, U32};

#[derive(Default)]
struct Samples(Vec<BenchmarkEvent>);
impl BenchmarkSink for Samples {
    fn observe(&mut self, e: BenchmarkEvent) {
        self.0.push(e)
    }
}
fn id(n: u8) -> [u8; 16] {
    [n; 16]
}
fn hello<B: InferenceBackend>(core: &mut LinkCore<B, Samples>) -> LinkEpoch {
    core.begin_negotiation();
    let frame = core.pop_control_frame().unwrap();
    assert_eq!(decode_envelope(&frame).kind.get(), HELLO);
    let host_nonce: [u8; 32] = frame[kore_hostlink::ENVELOPE_LEN..].try_into().unwrap();
    let guest_boot = [7; 32];
    let epoch = derive_link_epoch(&host_nonce, &guest_boot, negotiated_transcript());
    let mut payload = [0; HELLO_ACK_LEN];
    payload[..32].copy_from_slice(&guest_boot);
    payload[32..].copy_from_slice(&epoch.0);
    let ack = envelope(
        HELLO_ACK,
        LinkEpoch::ZERO,
        RouteId::ZERO,
        0,
        0,
        RequestId::ZERO,
        payload.len(),
    );
    core.receive(SocketPlane::Control, ack, &payload);
    assert_eq!(core.epoch(), Some(epoch));
    epoch
}

#[test]
fn hello_is_retried_with_the_same_nonce_until_acknowledged() {
    let mut core = LinkCore::new(DeterministicBackend::new(1), Samples::default());
    core.begin_negotiation();
    let first = core.pop_control_frame().unwrap();
    assert_eq!(decode_envelope(&first).kind.get(), HELLO);

    for _ in 0..999 {
        core.advance();
    }
    assert!(core.pop_control_frame().is_none());
    core.advance();

    let retry = core.pop_control_frame().unwrap();
    assert_eq!(decode_envelope(&retry).kind.get(), HELLO);
    assert_eq!(
        &retry[kore_hostlink::ENVELOPE_LEN..],
        &first[kore_hostlink::ENVELOPE_LEN..]
    );
}

fn open<B: InferenceBackend>(
    core: &mut LinkCore<B, Samples>,
    epoch: LinkEpoch,
    route: u8,
    role: u8,
    ordinal: u8,
) {
    open_generation(core, epoch, route, 9, role, ordinal)
}

fn open_generation<B: InferenceBackend>(
    core: &mut LinkCore<B, Samples>,
    epoch: LinkEpoch,
    route: u8,
    generation: u64,
    role: u8,
    ordinal: u8,
) {
    let mut payload = [0; ROUTE_OPEN_LEN];
    payload[..16].copy_from_slice(&id(route));
    payload[16..24].copy_from_slice(&generation.to_le_bytes());
    payload[24] = role;
    payload[25] = ordinal;
    let env = envelope(
        ROUTE_OPEN,
        epoch,
        RouteId::ZERO,
        0,
        0,
        RequestId::ZERO,
        payload.len(),
    );
    core.receive(SocketPlane::Control, env, &payload);
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        ROUTE_OPENED
    )
}

fn versioned_inference(role: u8, turn: u64, byte: u8) -> Vec<u8> {
    let mut payload = vec![0; INFERENCE_REQUEST_HEADER_LEN + 1];
    let length = InferenceRequestView {
        kind: if role == 0 {
            InferenceRequestKind::BrokerPlan
        } else {
            InferenceRequestKind::AgentTurn
        },
        turn,
        max_output_tokens: 64,
        timeout_ticks: 100,
        input: &[byte],
    }
    .encode(&mut payload)
    .unwrap();
    payload.truncate(length);
    payload
}

#[test]
fn broker_and_two_agents_are_isolated_and_bounded() {
    let mut core = LinkCore::new(DeterministicBackend::new(1), Samples::default());
    let epoch = hello(&mut core);
    open(&mut core, epoch, 2, 0, 0);
    open(&mut core, epoch, 3, 1, 0);
    open(&mut core, epoch, 4, 1, 1);
    for (route, role, byte) in [(2, 0, 10), (3, 1, 11), (4, 1, 12)] {
        let payload = versioned_inference(role, 1, byte);
        let env = envelope(
            INFER_REQUEST,
            epoch,
            RouteId(id(route)),
            9,
            0,
            RequestId(id(byte)),
            payload.len(),
        );
        core.receive(SocketPlane::Control, env, &payload);
    }
    core.advance();
    let mut replies = Vec::new();
    while let Some(frame) = core.pop_control_frame() {
        if decode_envelope(&frame).kind.get() == INFER_REPLY {
            replies.push(frame)
        }
    }
    assert_eq!(replies.len(), 3);
    let bodies: Vec<_> = replies
        .iter()
        .map(|f| {
            kore_hostlink::decode_inference_event(&f[kore_hostlink::ENVELOPE_LEN..])
                .expect("versioned inference completion")
                .payload
        })
        .collect();
    assert_eq!((bodies[0][0], bodies[0][1]), (0, 0));
    assert_eq!((bodies[1][0], bodies[1][1]), (1, 1));
    assert_eq!((bodies[2][0], bodies[2][1]), (1, 2));
    assert_ne!(&bodies[0][42..], &bodies[1][42..]);
}

#[test]
fn legacy_deterministic_boot_request_keeps_its_terminal_wire_shape() {
    let mut core = LinkCore::new(DeterministicBackend::new(1), Samples::default());
    let epoch = hello(&mut core);
    open(&mut core, epoch, 2, 0, 0);
    let request = RequestId(id(13));
    core.receive(
        SocketPlane::Control,
        envelope(INFER_REQUEST, epoch, RouteId(id(2)), 9, 0, request, 1),
        &[13],
    );
    core.advance();
    let frame = core.pop_control_frame().unwrap();
    assert_eq!(decode_envelope(&frame).kind.get(), INFER_REPLY);
    let payload = &frame[kore_hostlink::ENVELOPE_LEN..];
    assert!(kore_hostlink::decode_inference_event(payload).is_none());
    assert_eq!(payload.len(), 74);
    assert_eq!((payload[0], payload[1]), (0, 0));
}

#[test]
fn cancellation_replay_and_new_epoch_are_deterministic() {
    let mut core = LinkCore::new(DeterministicBackend::new(5), Samples::default());
    let epoch = hello(&mut core);
    open(&mut core, epoch, 2, 1, 0);
    let request = RequestId(id(8));
    let submit = envelope(INFER_REQUEST, epoch, RouteId(id(2)), 9, 0, request, 1);
    core.receive(SocketPlane::Control, submit, &[1]);
    core.receive(SocketPlane::Control, submit, &[1]);
    assert_eq!(core.metrics().replay_or_order, 1);
    assert_eq!(
        refusal_code(&core.pop_control_frame().unwrap()),
        RefusalCode::Duplicate as u16
    );
    let cancel = envelope(CANCEL, epoch, RouteId(id(2)), 9, 1, request, 0);
    core.receive(SocketPlane::Control, cancel, &[]);
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        CANCELLED
    );
    let unknown = envelope(CANCEL, epoch, RouteId(id(2)), 9, 2, RequestId(id(90)), 0);
    core.receive(SocketPlane::Control, unknown, &[]);
    assert_eq!(
        refusal_code(&core.pop_control_frame().unwrap()),
        RefusalCode::SelectorUnknown as u16
    );
    let replacement = envelope(
        INFER_REQUEST,
        epoch,
        RouteId(id(2)),
        9,
        2,
        RequestId(id(91)),
        1,
    );
    core.receive(SocketPlane::Control, replacement, &[2]);
    assert_eq!(core.metrics().submitted, 2);
    core.disconnect();
    assert!(core.epoch().is_none());
    let next = hello(&mut core);
    assert_ne!(epoch, next);
}

#[test]
fn malformed_faults_are_constructible() {
    let mut h = FrameHarness::default();
    let env = envelope(
        HELLO_ACK,
        LinkEpoch::ZERO,
        RouteId::ZERO,
        0,
        0,
        RequestId::ZERO,
        HELLO_ACK_LEN,
    );
    let bytes = h.valid(&env, &[0; HELLO_ACK_LEN]);
    let bad = h.inject(bytes.clone(), Fault::BadMagic);
    assert_ne!(&bad[..4], &bytes[..4]);
    assert_eq!(
        h.inject(bytes, Fault::Replay),
        h.inject(Vec::new(), Fault::Replay)
    );
}

#[test]
fn hello_direction_and_generation_refusals_fail_closed() {
    let mut core = LinkCore::new(DeterministicBackend::new(1), Samples::default());
    let wrong_direction = envelope(
        HELLO,
        LinkEpoch::ZERO,
        RouteId::ZERO,
        0,
        0,
        RequestId::ZERO,
        HELLO_LEN,
    );
    core.receive(SocketPlane::Control, wrong_direction, &[0; HELLO_LEN]);
    assert_eq!(
        refusal_code(&core.pop_control_frame().unwrap()),
        RefusalCode::UnknownKind as u16
    );
    assert!(core.epoch().is_none());

    let epoch = hello(&mut core);
    open(&mut core, epoch, 2, 1, 0);
    let stale = envelope(
        INFER_REQUEST,
        epoch,
        RouteId(id(2)),
        10,
        0,
        RequestId(id(70)),
        1,
    );
    core.receive(SocketPlane::Control, stale, &[1]);
    assert_eq!(
        refusal_code(&core.pop_control_frame().unwrap()),
        RefusalCode::GenerationStale as u16
    );
    let valid = envelope(
        INFER_REQUEST,
        epoch,
        RouteId(id(2)),
        9,
        0,
        RequestId(id(71)),
        1,
    );
    core.receive(SocketPlane::Control, valid, &[1]);
    assert_eq!(core.metrics().submitted, 1);
}

#[derive(Default)]
struct InjectedState {
    sequences: RefCell<Vec<SequenceIdentity>>,
    events: RefCell<VecDeque<BackendEvent>>,
}

struct InjectedBackend(Rc<InjectedState>);

impl InferenceBackend for InjectedBackend {
    fn reset(&mut self) {
        self.0.sequences.borrow_mut().clear();
    }

    fn open(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError> {
        self.0.sequences.borrow_mut().push(sequence);
        Ok(())
    }

    fn close(&mut self, sequence: SequenceIdentity) -> Result<(), BackendError> {
        self.0
            .sequences
            .borrow_mut()
            .retain(|installed| *installed != sequence);
        Ok(())
    }

    fn submit(
        &mut self,
        sequence: SequenceIdentity,
        _: RequestId,
        _: &[u8],
        _: u64,
    ) -> Result<(), BackendError> {
        self.0
            .sequences
            .borrow()
            .contains(&sequence)
            .then_some(())
            .ok_or(BackendError::GenerationStale)
    }

    fn cancel(
        &mut self,
        _: SequenceIdentity,
        _: RequestId,
        _: u64,
    ) -> Result<BackendEvent, BackendError> {
        Err(BackendError::RequestUnknown)
    }

    fn poll(&mut self, _: u64) -> Option<BackendEvent> {
        self.0.events.borrow_mut().pop_front()
    }

    fn metrics(&self) -> BackendMetrics {
        BackendMetrics::default()
    }

    fn sequence_metrics(&self, _: SequenceIdentity) -> Option<SequenceMetrics> {
        None
    }

    fn broker_prompt_ready(&self, _: SequenceIdentity) -> bool {
        // This late-event fixture does not model prompt ownership or cleanup.
        false
    }
}

#[test]
fn late_backend_completion_cannot_enter_a_reused_route_or_generation() {
    let injected = Rc::new(InjectedState::default());
    let mut core = LinkCore::new(InjectedBackend(Rc::clone(&injected)), Samples::default());
    let epoch = hello(&mut core);
    open_generation(&mut core, epoch, 2, 9, 1, 0);
    let request = RequestId(id(72));
    core.receive(
        SocketPlane::Control,
        envelope(INFER_REQUEST, epoch, RouteId(id(2)), 9, 0, request, 1),
        b"a",
    );
    core.receive(
        SocketPlane::Control,
        envelope(ROUTE_CLOSE, epoch, RouteId(id(2)), 9, 1, RequestId::ZERO, 0),
        &[],
    );
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        ROUTE_CLOSED
    );

    open_generation(&mut core, epoch, 2, 10, 1, 0);
    core.receive(
        SocketPlane::Control,
        envelope(INFER_REQUEST, epoch, RouteId(id(2)), 10, 0, request, 1),
        b"b",
    );
    injected
        .events
        .borrow_mut()
        .push_back(BackendEvent::Completed {
            sequence: SequenceIdentity {
                route: RouteId(id(2)),
                generation: TaskGeneration::new(9).unwrap(),
                role: BackendRole::Agent,
            },
            request,
            payload: b"old".to_vec(),
            part: 0,
            submitted_tick: 0,
            usage: RequestUsage::default(),
            turn: 1,
        });
    core.advance();
    assert!(core.pop_control_frame().is_none());
    assert_eq!(core.metrics().stale_backend_events, 1);

    injected
        .events
        .borrow_mut()
        .push_back(BackendEvent::Completed {
            sequence: SequenceIdentity {
                route: RouteId(id(2)),
                generation: TaskGeneration::new(10).unwrap(),
                role: BackendRole::Agent,
            },
            request,
            payload: b"new".to_vec(),
            part: 0,
            submitted_tick: 0,
            usage: RequestUsage::default(),
            turn: 1,
        });
    core.advance();
    let frame = core.pop_control_frame().unwrap();
    assert_eq!(decode_envelope(&frame).kind.get(), INFER_REPLY);
    let event = kore_hostlink::decode_inference_event(&frame[kore_hostlink::ENVELOPE_LEN..])
        .expect("versioned terminal inference event");
    assert!(event.terminal);
    assert_eq!(event.payload, b"new");
}

#[test]
fn out_of_order_backend_stream_part_fails_the_epoch_closed() {
    let injected = Rc::new(InjectedState::default());
    let mut core = LinkCore::new(InjectedBackend(Rc::clone(&injected)), Samples::default());
    let epoch = hello(&mut core);
    open_generation(&mut core, epoch, 2, 9, 1, 0);
    let request = RequestId(id(73));
    core.receive(
        SocketPlane::Control,
        envelope(INFER_REQUEST, epoch, RouteId(id(2)), 9, 0, request, 1),
        b"a",
    );
    injected.events.borrow_mut().push_back(BackendEvent::Chunk {
        sequence: SequenceIdentity {
            route: RouteId(id(2)),
            generation: TaskGeneration::new(9).unwrap(),
            role: BackendRole::Agent,
        },
        request,
        payload: b"skipped".to_vec(),
        part: 1,
        submitted_tick: 0,
        turn: 0,
    });
    core.advance();
    assert!(core.epoch().is_none());
    assert_eq!(core.metrics().replay_or_order, 1);
    assert!(core.pop_control_frame().is_none());
}

#[test]
fn saturated_stream_reserves_delivery_for_terminal_completion() {
    let injected = Rc::new(InjectedState::default());
    let mut core = LinkCore::new(InjectedBackend(Rc::clone(&injected)), Samples::default());
    let epoch = hello(&mut core);
    open_generation(&mut core, epoch, 2, 9, 1, 0);
    let request = RequestId(id(74));
    core.receive(
        SocketPlane::Control,
        envelope(INFER_REQUEST, epoch, RouteId(id(2)), 9, 0, request, 1),
        b"a",
    );
    let sequence = SequenceIdentity {
        route: RouteId(id(2)),
        generation: TaskGeneration::new(9).unwrap(),
        role: BackendRole::Agent,
    };
    for part in 0..48u16 {
        injected.events.borrow_mut().push_back(BackendEvent::Chunk {
            sequence,
            request,
            payload: b"x".to_vec(),
            part,
            submitted_tick: 0,
            turn: 0,
        });
    }
    injected
        .events
        .borrow_mut()
        .push_back(BackendEvent::Completed {
            sequence,
            request,
            payload: Vec::new(),
            part: 48,
            submitted_tick: 0,
            usage: RequestUsage::default(),
            turn: 0,
        });
    core.advance();
    let mut count = 0;
    let mut terminal = false;
    while let Some(frame) = core.pop_control_frame() {
        count += 1;
        terminal |= kore_hostlink::decode_inference_event(&frame[kore_hostlink::ENVELOPE_LEN..])
            .is_some_and(|event| event.terminal);
    }
    assert_eq!(count, 49);
    assert!(terminal);
    assert_eq!(core.metrics().completed, 1);
}

#[test]
fn qemu_topology_is_modern_and_exact() {
    let args =
        hostlinkd::qemu::arguments(Path::new("/tmp/control.sock"), Path::new("/tmp/bulk.sock"))
            .unwrap();
    let joined = args.join(" ");
    assert!(joined.contains("virtio-serial-pci,id=hostlink"));
    assert!(joined.contains("disable-legacy=on"));
    assert!(joined.contains("iommu_platform=on"));
    assert!(joined.contains("max_ports=3"));
    assert!(joined.contains("nr=1,name=org.microperceptron.hostlink.control"));
    assert!(joined.contains("nr=2,name=org.microperceptron.hostlink.bulk"));
}

#[test]
fn backend_generalizes_broker_and_agent_sequences() {
    use hostlinkd::InferenceBackend;
    let mut b = DeterministicBackend::new(1);
    for (route, role) in [
        (1, BackendRole::Broker),
        (2, BackendRole::Agent),
        (3, BackendRole::Agent),
        (4, BackendRole::Agent),
    ] {
        assert!(
            b.open(SequenceIdentity {
                route: RouteId(id(route)),
                generation: TaskGeneration::new(9).unwrap(),
                role,
            })
            .is_ok()
        );
    }
}

#[test]
fn typed_backpressure_does_not_consume_the_receive_sequence() {
    let mut core = LinkCore::new(DeterministicBackend::new(100), Samples::default());
    let epoch = hello(&mut core);
    open(&mut core, epoch, 2, 1, 0);
    for sequence in 0..16 {
        let request = RequestId(id(sequence as u8 + 1));
        let submit = envelope(
            INFER_REQUEST,
            epoch,
            RouteId(id(2)),
            9,
            sequence,
            request,
            1,
        );
        core.receive(SocketPlane::Control, submit, &[1]);
    }
    let refused = envelope(
        INFER_REQUEST,
        epoch,
        RouteId(id(2)),
        9,
        16,
        RequestId(id(40)),
        1,
    );
    core.receive(SocketPlane::Control, refused, &[1]);
    let refusal = core.pop_control_frame().unwrap();
    assert_eq!(decode_envelope(&refusal).kind.get(), REFUSED);
    assert_eq!(refusal_code(&refusal), RefusalCode::Backpressure as u16);

    // The refused request did not advance receive sequence 16.
    let cancel = envelope(CANCEL, epoch, RouteId(id(2)), 9, 16, RequestId(id(1)), 0);
    core.receive(SocketPlane::Control, cancel, &[]);
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        CANCELLED
    );
}

#[test]
fn bulk_begin_chunk_finish_and_abort_are_digest_checked_and_replay_safe() {
    let mut core = LinkCore::new(DeterministicBackend::new(10), Samples::default());
    let epoch = hello(&mut core);
    open(&mut core, epoch, 2, 1, 0);
    let route = RouteId(id(2));
    let request = RequestId(id(50));
    let bytes = b"bounded bulk bytes";
    let reference = bulk_reference(60, bytes);

    // A lowered host limit is a typed refusal and does not consume sequence 0.
    let oversized = BulkRefV1 {
        bulk_id: BulkId(id(61)),
        total_len: U32::new((crate_object_limit() + 1) as u32),
        chunk_count: U16::new(9),
        content_kind: U16::new(1),
        digest_blake3: [1; 32],
    };
    send_bulk(
        &mut core,
        epoch,
        route,
        0,
        request,
        BULK_BEGIN,
        oversized.as_bytes(),
    );
    assert_eq!(
        refusal_code(&core.pop_control_frame().unwrap()),
        RefusalCode::TooLarge as u16
    );

    // An admitted transfer can be explicitly aborted without becoming ready.
    let aborted_request = RequestId(id(51));
    let aborted = bulk_reference(62, b"discard me");
    send_bulk(
        &mut core,
        epoch,
        route,
        0,
        aborted_request,
        BULK_BEGIN,
        aborted.as_bytes(),
    );
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        BULK_ACCEPTED
    );
    let mut abort = Vec::from(aborted.bulk_id.0);
    abort.extend_from_slice(&(RefusalCode::Cancelled as u16).to_le_bytes());
    send_bulk(
        &mut core,
        epoch,
        route,
        1,
        aborted_request,
        BULK_ABORT,
        &abort,
    );
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        BULK_ABORTED
    );
    assert!(core.pop_completed_bulk().is_none());

    send_bulk(
        &mut core,
        epoch,
        route,
        2,
        request,
        BULK_BEGIN,
        reference.as_bytes(),
    );
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        BULK_ACCEPTED
    );

    let mut bad_chunk = chunk_payload(reference.bulk_id, 0, bytes);
    bad_chunk[20] ^= 1;
    send_bulk(&mut core, epoch, route, 3, request, BULK_CHUNK, &bad_chunk);
    assert_eq!(
        refusal_code(&core.pop_control_frame().unwrap()),
        RefusalCode::InvalidEncoding as u16
    );

    let chunk = chunk_payload(reference.bulk_id, 0, bytes);
    send_bulk(&mut core, epoch, route, 3, request, BULK_CHUNK, &chunk);
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        BULK_ACCEPTED
    );
    let finish = finish_payload(reference.bulk_id, bytes);
    send_bulk(&mut core, epoch, route, 4, request, BULK_FINISH, &finish);
    assert_eq!(
        decode_envelope(&core.pop_control_frame().unwrap())
            .kind
            .get(),
        BULK_READY
    );
    assert_eq!(core.pop_completed_bulk().unwrap().bytes, bytes);
}

#[test]
fn two_socket_disconnect_requires_reconnect_and_new_epoch() {
    let root = unique_temp();
    fs::create_dir(&root).unwrap();
    let control_path = root.join("control.sock");
    let bulk_path = root.join("bulk.sock");
    let mut daemon = hostlinkd::HostLinkDaemon::bind(
        hostlinkd::Config {
            control_socket: control_path.clone(),
            bulk_socket: bulk_path.clone(),
        },
        DeterministicBackend::new(1),
        (),
    )
    .unwrap();
    let mut control = UnixStream::connect(&control_path).unwrap();
    let bulk = UnixStream::connect(&bulk_path).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let first = socket_hello(&mut daemon, &mut control, 21);
    drop(bulk);
    for _ in 0..3 {
        daemon.tick().unwrap();
    }
    assert!(daemon.core().epoch().is_none());
    drop(control);
    let mut control = UnixStream::connect(&control_path).unwrap();
    let _bulk = UnixStream::connect(&bulk_path).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let second = socket_hello(&mut daemon, &mut control, 21);
    assert_ne!(first, second);
    assert_eq!(daemon.core().metrics().disconnects, 1);
    drop(daemon);
    fs::remove_dir(&root).unwrap();
}

#[test]
fn routed_refusal_is_observable_before_both_planes_reset_for_a_new_epoch() {
    let root = unique_temp();
    fs::create_dir(&root).unwrap();
    let control_path = root.join("control.sock");
    let bulk_path = root.join("bulk.sock");
    let mut daemon = hostlinkd::HostLinkDaemon::bind(
        hostlinkd::Config {
            control_socket: control_path.clone(),
            bulk_socket: bulk_path.clone(),
        },
        DeterministicBackend::new(1),
        (),
    )
    .unwrap();
    let mut control = UnixStream::connect(&control_path).unwrap();
    let bulk = UnixStream::connect(&bulk_path).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let first = socket_hello(&mut daemon, &mut control, 31);
    socket_open(&mut daemon, &mut control, first, 2);

    // Empty inference input is a semantic refusal after sequence admission.
    let rejected = envelope(
        INFER_REQUEST,
        first,
        RouteId(id(2)),
        9,
        0,
        RequestId(id(90)),
        0,
    );
    control.write_all(rejected.as_bytes()).unwrap();
    daemon.tick().unwrap();

    let refusal = read_socket_frame(&mut control);
    assert_eq!(decode_envelope(&refusal).kind.get(), REFUSED);
    assert_eq!(refusal_code(&refusal), RefusalCode::UnknownKind as u16);
    let mut byte = [0; 1];
    assert_eq!(control.read(&mut byte).unwrap(), 0);
    assert!(daemon.core().epoch().is_none());
    assert_eq!(daemon.core().metrics().disconnects, 1);
    daemon.force_disconnect();
    assert_eq!(daemon.core().metrics().disconnects, 1);

    drop(control);
    drop(bulk);
    let mut control = UnixStream::connect(&control_path).unwrap();
    let _bulk = UnixStream::connect(&bulk_path).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let second = socket_hello(&mut daemon, &mut control, 31);
    assert_ne!(first, second);

    drop(daemon);
    fs::remove_dir(&root).unwrap();
}

#[test]
fn peer_refusal_closes_without_a_refusal_ping_pong() {
    let root = unique_temp();
    fs::create_dir(&root).unwrap();
    let control_path = root.join("control.sock");
    let bulk_path = root.join("bulk.sock");
    let mut daemon = hostlinkd::HostLinkDaemon::bind(
        hostlinkd::Config {
            control_socket: control_path.clone(),
            bulk_socket: bulk_path.clone(),
        },
        DeterministicBackend::new(1),
        (),
    )
    .unwrap();
    let mut control = UnixStream::connect(&control_path).unwrap();
    let _bulk = UnixStream::connect(&bulk_path).unwrap();
    control
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let epoch = socket_hello(&mut daemon, &mut control, 32);
    socket_open(&mut daemon, &mut control, epoch, 2);

    let refusal = RefusalView {
        for_family: kore_hostlink::FAMILY_LINK,
        for_kind: INFER_REPLY,
        code: RefusalCode::Backpressure,
        retry: RetryClass::AfterReconnect,
        retry_after_ms: 0,
        safe_detail: &[],
    };
    let mut payload = [0; REFUSAL_HEADER_LEN];
    let payload_len = refusal.encode(&mut payload).unwrap();
    let env = envelope(
        REFUSED,
        epoch,
        RouteId(id(2)),
        9,
        0,
        RequestId(id(91)),
        payload_len,
    );
    control.write_all(env.as_bytes()).unwrap();
    control.write_all(&payload[..payload_len]).unwrap();
    daemon.tick().unwrap();

    let mut byte = [0; 1];
    assert_eq!(control.read(&mut byte).unwrap(), 0);
    assert!(daemon.core().epoch().is_none());
    assert_eq!(daemon.core().metrics().frames_out, 2); // Hello + RouteOpened only.

    drop(daemon);
    fs::remove_dir(&root).unwrap();
}

fn socket_hello(
    daemon: &mut hostlinkd::HostLinkDaemon<DeterministicBackend>,
    control: &mut UnixStream,
    boot: u8,
) -> LinkEpoch {
    for _ in 0..2 {
        daemon.tick().unwrap();
    }
    let mut hello = vec![0u8; kore_hostlink::ENVELOPE_LEN + HELLO_LEN];
    control.read_exact(&mut hello).unwrap();
    assert_eq!(decode_envelope(&hello).kind.get(), HELLO);
    let host_nonce: [u8; 32] = hello[kore_hostlink::ENVELOPE_LEN..].try_into().unwrap();
    let guest_boot = [boot; 32];
    let epoch = derive_link_epoch(&host_nonce, &guest_boot, negotiated_transcript());
    let mut payload = [0; HELLO_ACK_LEN];
    payload[..32].copy_from_slice(&guest_boot);
    payload[32..].copy_from_slice(&epoch.0);
    let env = envelope(
        HELLO_ACK,
        LinkEpoch::ZERO,
        RouteId::ZERO,
        0,
        0,
        RequestId::ZERO,
        payload.len(),
    );
    control.write_all(env.as_bytes()).unwrap();
    control.write_all(&payload).unwrap();
    for _ in 0..3 {
        daemon.tick().unwrap();
    }
    assert_eq!(daemon.core().epoch(), Some(epoch));
    epoch
}

fn socket_open(
    daemon: &mut hostlinkd::HostLinkDaemon<DeterministicBackend>,
    control: &mut UnixStream,
    epoch: LinkEpoch,
    route: u8,
) {
    let mut payload = [0; ROUTE_OPEN_LEN];
    payload[..16].copy_from_slice(&id(route));
    payload[16..24].copy_from_slice(&9u64.to_le_bytes());
    payload[24] = 1;
    payload[25] = 0;
    let env = envelope(
        ROUTE_OPEN,
        epoch,
        RouteId::ZERO,
        0,
        0,
        RequestId::ZERO,
        payload.len(),
    );
    control.write_all(env.as_bytes()).unwrap();
    control.write_all(&payload).unwrap();
    daemon.tick().unwrap();
    let opened = read_socket_frame(control);
    assert_eq!(decode_envelope(&opened).kind.get(), ROUTE_OPENED);
}

fn read_socket_frame(stream: &mut UnixStream) -> Vec<u8> {
    let mut header = vec![0; kore_hostlink::ENVELOPE_LEN];
    stream.read_exact(&mut header).unwrap();
    let payload_len = decode_envelope(&header).payload_len.get() as usize;
    header.resize(kore_hostlink::ENVELOPE_LEN + payload_len, 0);
    stream
        .read_exact(&mut header[kore_hostlink::ENVELOPE_LEN..])
        .unwrap();
    header
}

fn unique_temp() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ordinal = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "hostlinkd-{}-{nonce}-{ordinal}",
        std::process::id()
    ))
}

fn send_bulk(
    core: &mut LinkCore<DeterministicBackend, Samples>,
    epoch: LinkEpoch,
    route: RouteId,
    sequence: u64,
    request: RequestId,
    kind: u16,
    payload: &[u8],
) {
    let env = envelope(kind, epoch, route, 9, sequence, request, payload.len());
    core.receive(SocketPlane::Bulk, env, payload);
}

fn bulk_reference(byte: u8, bytes: &[u8]) -> BulkRefV1 {
    BulkRefV1 {
        bulk_id: BulkId(id(byte)),
        total_len: U32::new(bytes.len() as u32),
        chunk_count: U16::new(1),
        content_kind: U16::new(kore_workspace_sync::WORKSPACE_BLOB_CONTENT),
        digest_blake3: *blake3::hash(bytes).as_bytes(),
    }
}

fn chunk_payload(id: BulkId, offset: u32, bytes: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&id.0);
    payload.extend_from_slice(&offset.to_le_bytes());
    payload.extend_from_slice(blake3::hash(bytes).as_bytes());
    payload.extend_from_slice(bytes);
    payload
}

fn finish_payload(id: BulkId, bytes: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&id.0);
    payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    payload.extend_from_slice(blake3::hash(bytes).as_bytes());
    payload
}

fn refusal_code(frame: &[u8]) -> u16 {
    let at = kore_hostlink::ENVELOPE_LEN + 4;
    u16::from_le_bytes(frame[at..at + 2].try_into().unwrap())
}

fn crate_object_limit() -> usize {
    hostlinkd::bulk::HOST_BULK_OBJECT_MAX
}
