use hostlinkd::{
    BackendHarness, BackendLimits, CreateSession, DeterministicBackend, DeterministicGuest,
    EventKind, HarnessHandle, HarnessHttpServer, HarnessService, LlamaServerBackend,
    LlamaServerConfig, PromptRequest, PromptStatus, SessionState, SessionStatus, TaskRuntime,
};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn create(handle: &HarnessHandle, session: &str) -> SessionState {
    handle
        .create_session(CreateSession {
            api_version: 1,
            project_id: "project".into(),
            session_id: session.into(),
        })
        .unwrap()
}

fn prompt(id: &str, text: &str) -> PromptRequest {
    PromptRequest {
        api_version: 1,
        prompt_id: id.into(),
        idempotency_key: format!("key-{id}"),
        text: text.into(),
    }
}

fn wait_terminal(
    handle: &HarnessHandle,
    session: &str,
    prompt: &str,
) -> Vec<hostlinkd::HarnessEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = handle.events(session, 0).unwrap();
        if events.iter().any(|event| {
            event.prompt_id.as_deref() == Some(prompt) && event.kind == EventKind::PromptTerminal
        }) {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "prompt did not terminate: {events:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn prompt_runs_broker_agent_guest_tool_and_assistant_loop() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = BackendHarness::new(DeterministicBackend::new(2), DeterministicGuest::new());
    let service = HarnessService::start(runtime, temp.path());
    let handle = service.handle();
    create(&handle, "session-1");
    handle
        .submit_prompt("session-1", prompt("p1", "read X"))
        .unwrap();
    let events = wait_terminal(&handle, "session-1", "p1");
    assert!(events.iter().all(|event| event.timestamp_unix_ms > 0));
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].timestamp_unix_ms <= pair[1].timestamp_unix_ms)
    );
    for kind in [
        EventKind::UserPrompt,
        EventKind::BrokerProposal,
        EventKind::Capability,
        EventKind::ToolCall,
        EventKind::ToolResult,
        EventKind::Assistant,
        EventKind::PromptTerminal,
    ] {
        assert!(
            events.iter().any(|event| event.kind == kind),
            "missing {kind:?}"
        );
    }
    let tool = events
        .iter()
        .find(|event| event.kind == EventKind::ToolResult)
        .unwrap();
    assert_eq!(tool.payload["execution_domain"], "guest");
    assert!(
        tool.payload["text"]
            .as_str()
            .unwrap()
            .contains("capability-scoped")
    );
    let assistant = events
        .iter()
        .find(|event| event.kind == EventKind::Assistant)
        .unwrap();
    assert_eq!(assistant.payload["validated"], true);
    assert!(temp.path().join("session-1.jsonl").exists());
}

#[test]
fn sequential_prompts_reuse_conversation_and_idempotent_retry_does_not_repeat() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path(),
    );
    let handle = service.handle();
    create(&handle, "sequential");
    let first = prompt("p1", "read X");
    assert_eq!(
        handle.submit_prompt("sequential", first.clone()).unwrap(),
        "p1"
    );
    assert_eq!(handle.submit_prompt("sequential", first).unwrap(), "p1");
    wait_terminal(&handle, "sequential", "p1");
    handle
        .submit_prompt("sequential", prompt("p2", "read X"))
        .unwrap();
    let events = wait_terminal(&handle, "sequential", "p2");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == EventKind::BrokerProposal)
            .count(),
        1
    );
    assert_eq!(handle.state("sequential").unwrap().prompts.len(), 2);
}

#[test]
fn cross_session_ids_and_idempotency_conflicts_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path(),
    );
    let handle = service.handle();
    create(&handle, "one");
    create(&handle, "two");
    let request = prompt("p1", "read X");
    handle.submit_prompt("one", request.clone()).unwrap();
    let conflicting = PromptRequest {
        prompt_id: "other".into(),
        ..request
    };
    assert!(handle.submit_prompt("one", conflicting).is_err());
    let changed = PromptRequest {
        api_version: 1,
        prompt_id: "p1".into(),
        idempotency_key: "key-p1".into(),
        text: "read Y".into(),
    };
    assert!(handle.submit_prompt("one", changed).is_err());
    wait_terminal(&handle, "one", "p1");
    assert!(
        handle
            .events("two", 0)
            .unwrap()
            .iter()
            .all(|event| event.session_id == "two")
    );
}

#[test]
fn service_cancellation_wins_before_late_completion() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(500), DeterministicGuest::new()),
        temp.path(),
    );
    let handle = service.handle();
    create(&handle, "cancel-race");
    handle
        .submit_prompt("cancel-race", prompt("p1", "read X"))
        .unwrap();
    handle
        .cancel_prompt(
            "cancel-race",
            hostlinkd::interactive::CancelRequest {
                api_version: 1,
                prompt_id: "p1".into(),
            },
        )
        .unwrap();
    let events = wait_terminal(&handle, "cancel-race", "p1");
    let terminal = events
        .iter()
        .find(|event| event.kind == EventKind::PromptTerminal)
        .unwrap();
    assert_eq!(terminal.payload["status"], "cancelled");
    assert!(
        !events
            .iter()
            .any(|event| event.kind == EventKind::Assistant)
    );
}

#[test]
fn caller_supplied_llama_backend_uses_the_same_harness_api() {
    let Some(python) = find_python() else { return };
    let temp = tempfile::tempdir().unwrap();
    let model = temp.path().join("model.gguf");
    std::fs::write(&model, b"stub").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let script = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/stub_llama_server.py");
    let limits = BackendLimits {
        sequences: 3,
        brokers: 1,
        agents: 2,
        max_output_tokens: 512,
        ..BackendLimits::default()
    };
    let backend = LlamaServerBackend::start(LlamaServerConfig {
        executable: python,
        executable_args: vec![script.into_os_string()],
        model,
        port,
        total_context_tokens: 8_192,
        threads: 1,
        startup_timeout: Duration::from_secs(10),
        io_timeout: Duration::from_millis(25),
        limits,
        trace: false,
    })
    .unwrap();
    let service = HarnessService::start(
        BackendHarness::new(backend, DeterministicGuest::new()),
        temp.path().join("transcripts"),
    );
    let handle = service.handle();
    create(&handle, "llama");
    handle
        .submit_prompt("llama", prompt("p1", "Read workspace/X using /X"))
        .unwrap();
    let events = wait_terminal(&handle, "llama", "p1");
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::BrokerProposal)
    );
    assert!(events.iter().any(|event| event.kind == EventKind::ToolCall));
    let tool_result = events
        .iter()
        .find(|event| event.kind == EventKind::ToolResult)
        .unwrap();
    assert_eq!(tool_result.payload["result"], "Output");
    assert_eq!(
        tool_result.payload["text"],
        "X: capability-scoped deterministic workspace object"
    );
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::Assistant)
    );
    let streamed = |role: &str, turn: u64| {
        events
            .iter()
            .filter(|event| {
                event.kind == EventKind::Inference
                    && event.payload["kind"] == "ModelOutputChunk"
                    && event.payload["role"] == role
                    && event.payload["turn"] == turn
            })
            .filter_map(|event| event.payload["payload"].as_str())
            .collect::<String>()
    };
    let broker_stream = streamed("Broker", 1);
    let agent_tool_stream = streamed("Agent", 1);
    let agent_response_stream = streamed("Agent", 2);
    assert!(
        broker_stream.contains(r#""action":"create_agent""#),
        "broker stream: {broker_stream}"
    );
    assert!(
        agent_tool_stream.contains(r#""action":"tool_call""#),
        "agent tool stream: {agent_tool_stream}; events: {events:#?}"
    );
    assert!(
        agent_response_stream.contains(r#""action":"respond""#),
        "agent response stream: {agent_response_stream}"
    );
    let first = |predicate: &dyn Fn(&hostlinkd::HarnessEvent) -> bool| {
        events.iter().position(predicate).unwrap()
    };
    let broker_chunk = first(&|event| {
        event.kind == EventKind::Inference
            && event.payload["kind"] == "ModelOutputChunk"
            && event.payload["role"] == "Broker"
    });
    let broker_proposal = first(&|event| event.kind == EventKind::BrokerProposal);
    let agent_chunk = first(&|event| {
        event.kind == EventKind::Inference
            && event.payload["kind"] == "ModelOutputChunk"
            && event.payload["role"] == "Agent"
            && event.payload["turn"] == 1
    });
    let tool_call = first(&|event| event.kind == EventKind::ToolCall);
    let tool_result = first(&|event| event.kind == EventKind::ToolResult);
    let final_chunk = first(&|event| {
        event.kind == EventKind::Inference
            && event.payload["kind"] == "ModelOutputChunk"
            && event.payload["role"] == "Agent"
            && event.payload["turn"] == 2
    });
    let assistant = first(&|event| event.kind == EventKind::Assistant);
    assert!(broker_chunk < broker_proposal);
    assert!(agent_chunk < tool_call);
    assert!(tool_call < tool_result && tool_result < final_chunk);
    assert!(final_chunk < assistant);
}

#[test]
fn cancellation_close_and_backend_restart_retire_live_work() {
    let session = SessionState {
        api_version: 1,
        project_id: "project".into(),
        session_id: "direct".into(),
        status: SessionStatus::Open,
        generation: 1,
        backend_incarnation: 0,
        last_event_sequence: 0,
        prompts: Default::default(),
    };
    let mut runtime = BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new());
    let mut events = Vec::new();
    let status = runtime.run_prompt(
        &session,
        &prompt("p1", "read X"),
        &mut |kind, _, _| events.push(kind),
        &|| false,
    );
    assert_eq!(status, PromptStatus::Completed);
    runtime.reset_backend();
    let status = runtime.run_prompt(
        &session,
        &prompt("p2", "read X"),
        &mut |kind, _, _| events.push(kind),
        &|| false,
    );
    assert_eq!(status, PromptStatus::Failed);
    assert!(events.contains(&EventKind::Error));

    let cancelled = runtime.run_prompt(
        &SessionState {
            session_id: "cancel".into(),
            ..session.clone()
        },
        &prompt("cancel", "read X"),
        &mut |_, _, _| {},
        &|| true,
    );
    assert_eq!(cancelled, PromptStatus::Cancelled);
}

#[test]
fn closing_session_emits_route_retirement_and_kv_erasure() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path(),
    );
    let handle = service.handle();
    create(&handle, "close");
    handle
        .submit_prompt("close", prompt("p1", "read X"))
        .unwrap();
    wait_terminal(&handle, "close", "p1");
    handle.close_session("close").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if handle.state("close").unwrap().status == SessionStatus::Closed {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    }
    let events = handle.events("close", 0).unwrap();
    let closed = events
        .iter()
        .find(|event| event.kind == EventKind::ConversationClosed)
        .unwrap();
    assert_eq!(closed.payload["kv_erased"], true);
    assert_eq!(closed.payload["route_retired"], true);
}

#[test]
fn loopback_http_api_returns_sse_events_and_reconnect_state() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path(),
    );
    let server = HarnessHttpServer::bind("127.0.0.1:0".parse().unwrap(), service.handle()).unwrap();
    let address = server.local_addr();
    let (tx, rx) = mpsc::channel();
    let client = thread::spawn(move || {
        let response = http(
            address,
            "POST",
            "/api/v1/sessions",
            r#"{"api_version":1,"project_id":"http","session_id":"web"}"#,
        );
        tx.send(response).unwrap();
    });
    server.poll(Duration::from_secs(1)).unwrap();
    let response = rx.recv().unwrap();
    client.join().unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));

    let (tx, rx) = mpsc::channel();
    let client = thread::spawn(move || {
        tx.send(http(
            address,
            "GET",
            "/api/v1/sessions/web/events?after=0",
            "",
        ))
        .unwrap();
    });
    server.poll(Duration::from_secs(1)).unwrap();
    let response = rx.recv().unwrap();
    client.join().unwrap();
    assert!(response.contains("text/event-stream"));
    assert!(response.contains("session_opened"));
    let data = response
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap();
    let event: Value = serde_json::from_str(data).unwrap();
    assert_eq!(event["session_id"], "web");
}

fn http(address: std::net::SocketAddr, method: &str, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn find_python() -> Option<std::path::PathBuf> {
    for candidate in ["python", "python3"] {
        if std::process::Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            return Some(candidate.into());
        }
    }
    None
}
