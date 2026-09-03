use hostlinkd::{
    BackendEvent, BackendRole, DeterministicBackend, InferenceBackend, SequenceIdentity,
};
use kore_hostlink::{
    AgentEventKind, AgentRequestKind, AgentRequestView, InferenceRequestKind, InferenceRequestView,
    RequestId, RouteId, TaskGeneration, ToolResultKind, decode_agent_event,
};

fn id(byte: u8) -> [u8; 16] {
    [byte; 16]
}

fn sequence(route: u8, generation: u64) -> SequenceIdentity {
    SequenceIdentity {
        route: RouteId(id(route)),
        generation: TaskGeneration::new(generation).unwrap(),
        role: BackendRole::Agent,
    }
}

fn request(
    turn: u64,
    kind: AgentRequestKind,
    call_id: u64,
    result: Option<ToolResultKind>,
    text: &str,
) -> Vec<u8> {
    let mut inner = [0; kore_hostlink::INFERENCE_PART_MAX];
    let inner_len = AgentRequestView {
        kind,
        turn,
        call_id,
        result,
        tools: "",
        text,
    }
    .encode(&mut inner)
    .unwrap();
    let mut outer = [0; kore_hostlink::INLINE_MAX];
    let outer_len = InferenceRequestView {
        kind: InferenceRequestKind::AgentTurn,
        turn,
        max_output_tokens: 64,
        timeout_ticks: 100,
        input: &inner[..inner_len],
    }
    .encode(&mut outer)
    .unwrap();
    outer[..outer_len].to_vec()
}

fn completed(backend: &mut DeterministicBackend, tick: u64) -> BackendEvent {
    backend.poll(tick).expect("deterministic event is ready")
}

#[test]
fn two_isolated_agent_tool_rounds_reuse_capacity_and_reject_stale_generation() {
    let mut backend = DeterministicBackend::new(1);
    let agents = [sequence(1, 1), sequence(2, 1)];
    for agent in agents {
        backend.open(agent).unwrap();
        backend
            .submit(
                agent,
                RequestId(id(agent.route.0[0] + 10)),
                &request(1, AgentRequestKind::UserText, 0, None, "identify"),
                0,
            )
            .unwrap();
    }

    let mut calls = [(
        SequenceIdentity {
            route: RouteId::ZERO,
            generation: TaskGeneration::new(1).unwrap(),
            role: BackendRole::Agent,
        },
        0,
    ); 2];
    for row in &mut calls {
        let BackendEvent::Completed {
            sequence,
            request: _,
            payload,
            turn: 1,
            ..
        } = completed(&mut backend, 1)
        else {
            panic!("expected terminal tool call");
        };
        let event = decode_agent_event(&payload).unwrap();
        assert_eq!(event.kind, AgentEventKind::ToolCall);
        assert_eq!(event.command, "cat");
        *row = (sequence, event.call_id);
    }
    assert_ne!(calls[0].0.route, calls[1].0.route);

    for (sequence, call_id) in calls {
        backend
            .submit(
                sequence,
                RequestId(id(sequence.route.0[0] + 20)),
                &request(
                    2,
                    AgentRequestKind::ToolResult,
                    call_id,
                    Some(ToolResultKind::Output),
                    if sequence.route.0[0] == 1 {
                        "agent-a"
                    } else {
                        "agent-b"
                    },
                ),
                2,
            )
            .unwrap();
    }
    for _ in 0..2 {
        let BackendEvent::Completed {
            payload, turn: 2, ..
        } = completed(&mut backend, 3)
        else {
            panic!("expected completion");
        };
        assert_eq!(
            decode_agent_event(&payload).unwrap().kind,
            AgentEventKind::Completed
        );
    }

    for agent in agents {
        backend.close(agent).unwrap();
    }
    let replacement = sequence(1, 2);
    backend.open(replacement).unwrap();
    assert_eq!(
        backend.submit(
            agents[0],
            RequestId(id(99)),
            &request(1, AgentRequestKind::UserText, 0, None, "stale"),
            4,
        ),
        Err(hostlinkd::BackendError::GenerationStale)
    );
}

#[test]
fn malformed_inner_record_does_not_advance_backend_turn() {
    let mut backend = DeterministicBackend::new(1);
    let agent = sequence(1, 1);
    backend.open(agent).unwrap();
    let mut malformed = request(1, AgentRequestKind::UserText, 0, None, "hello");
    let inner_magic = kore_hostlink::INFERENCE_REQUEST_HEADER_LEN;
    malformed[inner_magic + 7] = 0x80;
    assert_eq!(
        backend.submit(agent, RequestId(id(1)), &malformed, 0),
        Err(hostlinkd::BackendError::InvalidEncoding)
    );
    backend
        .submit(
            agent,
            RequestId(id(1)),
            &request(1, AgentRequestKind::UserText, 0, None, "hello"),
            0,
        )
        .unwrap();
}
