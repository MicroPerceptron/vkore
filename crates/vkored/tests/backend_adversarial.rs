use hostlinkd::{
    BackendError, BackendEvent, BackendFailure, BackendLimits, BackendRole, DeterministicBackend,
    InferenceBackend, SequenceIdentity,
};
use kore_hostlink::{
    INFERENCE_PART_MAX, INFERENCE_REQUEST_HEADER_LEN, InferenceRequestKind, InferenceRequestView,
    RequestId, RouteId, TaskGeneration,
};

fn id(byte: u8) -> [u8; 16] {
    [byte; 16]
}

fn sequence(route: u8, generation: u64, role: BackendRole) -> SequenceIdentity {
    SequenceIdentity {
        route: RouteId(id(route)),
        generation: TaskGeneration::new(generation).unwrap(),
        role,
    }
}

fn limits(sequences: usize, brokers: usize, agents: usize) -> BackendLimits {
    BackendLimits {
        sequences,
        brokers,
        agents,
        pending_per_sequence: 4,
        request_history: 16,
        context_bytes: 4096,
        output_bytes: 4096,
        tokens_per_sequence: 4096,
        max_output_tokens: 64,
        request_timeout_ticks: 20,
    }
}

fn agent_turn(turn: u64, input: &[u8]) -> Vec<u8> {
    let mut output = vec![0; INFERENCE_REQUEST_HEADER_LEN + INFERENCE_PART_MAX];
    let length = InferenceRequestView {
        kind: InferenceRequestKind::AgentTurn,
        turn,
        max_output_tokens: 32,
        timeout_ticks: 10,
        input,
    }
    .encode(&mut output)
    .unwrap();
    output.truncate(length);
    output
}

#[test]
fn interleaved_sequences_keep_context_pending_and_cancellation_isolated() {
    let mut backend = DeterministicBackend::with_limits(2, limits(4, 1, 3)).unwrap();
    let sequences = [
        sequence(1, 7, BackendRole::Broker),
        sequence(2, 7, BackendRole::Agent),
        sequence(3, 8, BackendRole::Agent),
        sequence(4, 9, BackendRole::Agent),
    ];
    for sequence in sequences {
        backend.open(sequence).unwrap();
    }
    for (index, sequence) in sequences.into_iter().enumerate() {
        backend
            .submit(
                sequence,
                RequestId(id(10 + index as u8)),
                &[index as u8 + 1],
                0,
            )
            .unwrap();
    }
    let cancelled = backend.cancel(sequences[2], RequestId(id(12)), 1).unwrap();
    assert!(matches!(
        cancelled,
        BackendEvent::Cancelled {
            sequence: owner,
            request,
            ..
        } if owner == sequences[2] && request == RequestId(id(12))
    ));

    let mut completed = Vec::new();
    while let Some(event) = backend.poll(2) {
        if let BackendEvent::Completed {
            sequence, payload, ..
        } = event
        {
            completed.push((sequence, payload));
        }
    }
    assert_eq!(completed.len(), 3);
    assert_eq!(completed[0].0, sequences[0]);
    assert_eq!(completed[1].0, sequences[1]);
    assert_eq!(completed[2].0, sequences[3]);
    assert_eq!(&completed[0].1[..2], &[0, 0]);
    assert_eq!(&completed[1].1[..2], &[1, 1]);
    assert_eq!(&completed[2].1[..2], &[1, 3]);
    assert_ne!(&completed[1].1[42..], &completed[2].1[42..]);
    assert_eq!(backend.metrics().pending, 0);
}

#[test]
fn route_generation_role_and_request_replay_are_exact_and_terminal() {
    let mut backend = DeterministicBackend::with_limits(1, limits(1, 0, 1)).unwrap();
    let old = sequence(1, 7, BackendRole::Agent);
    backend.open(old).unwrap();
    assert_eq!(
        backend.open(sequence(1, 8, BackendRole::Agent)),
        Err(BackendError::GenerationStale)
    );
    assert_eq!(
        backend.submit(
            sequence(1, 7, BackendRole::Broker),
            RequestId(id(2)),
            b"wrong-role",
            0
        ),
        Err(BackendError::RoleMismatch)
    );
    let request = RequestId(id(3));
    backend.submit(old, request, b"once", 0).unwrap();
    assert!(matches!(
        backend.poll(1),
        Some(BackendEvent::Completed { .. })
    ));
    assert_eq!(
        backend.submit(old, request, b"replay", 2),
        Err(BackendError::Duplicate)
    );
    backend.close(old).unwrap();
    let replacement = sequence(1, 8, BackendRole::Agent);
    backend.open(replacement).unwrap();
    assert_eq!(
        backend.submit(replacement, request, &agent_turn(2, b"skipped turn"), 3),
        Err(BackendError::OutOfOrder)
    );
    backend
        .submit(replacement, request, &agent_turn(1, b"new generation"), 3)
        .unwrap();
}

#[test]
fn pending_context_history_and_timeout_limits_refuse_with_typed_errors() {
    let mut bounded = limits(1, 0, 1);
    bounded.context_bytes = 100;
    bounded.tokens_per_sequence = 100;
    bounded.request_history = 2;
    bounded.request_timeout_ticks = 3;
    let mut backend = DeterministicBackend::with_limits(10, bounded).unwrap();
    let sequence = sequence(1, 1, BackendRole::Agent);
    backend.open(sequence).unwrap();
    backend
        .submit(sequence, RequestId(id(1)), b"twenty-byte-input!!!", 0)
        .unwrap();
    assert_eq!(
        backend.submit(sequence, RequestId(id(2)), b"would-overcommit", 0),
        Err(BackendError::ContextLimit)
    );
    assert!(matches!(
        backend.poll(3),
        Some(BackendEvent::Failed {
            failure: BackendFailure::TimedOut,
            ..
        })
    ));
    backend
        .submit(sequence, RequestId(id(2)), b"small", 4)
        .unwrap();
    assert_eq!(
        backend.submit(sequence, RequestId(id(3)), b"history-full", 4),
        Err(BackendError::Full)
    );
}
