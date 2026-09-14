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
fn response_is_published_before_reclaim_and_cleanup_cannot_reply_twice() {
    // Harness contract test, not a substitute for the real Kore/Qwen test.
    struct ResponseLifecycle;
    impl TaskRuntime for ResponseLifecycle {
        fn run_prompt(
            &mut self,
            _: &SessionState,
            _: &PromptRequest,
            emit: &mut dyn FnMut(EventKind, bool, Value),
            _: &dyn Fn() -> bool,
            response_published: &dyn Fn() -> bool,
        ) -> PromptStatus {
            assert!(!response_published());
            emit(
                EventKind::Error,
                true,
                serde_json::json!({"message": "worker exited", "stage": "agent_failure"}),
            );
            assert!(response_published());
            emit(
                EventKind::Capability,
                false,
                serde_json::json!({"state": "Teardown"}),
            );
            emit(
                EventKind::Error,
                true,
                serde_json::json!({"message": "cleanup diagnostic"}),
            );
            assert!(response_published());
            PromptStatus::Failed
        }
        fn close(&mut self, _: &SessionState, _: &mut dyn FnMut(EventKind, bool, Value)) {}
    }
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(ResponseLifecycle, temp.path());
    let handle = service.handle();
    create(&handle, "receipt-session");
    handle
        .submit_prompt("receipt-session", prompt("p1", "hello"))
        .unwrap();
    let events = wait_terminal(&handle, "receipt-session", "p1");
    let finals = events
        .iter()
        .filter(|event| {
            event.public && matches!(event.kind, EventKind::Error | EventKind::Assistant)
        })
        .collect::<Vec<_>>();
    assert_eq!(finals.len(), 1);
    let reclaim = events
        .iter()
        .find(|event| event.kind == EventKind::Capability)
        .unwrap();
    assert!(finals[0].sequence < reclaim.sequence);
    assert!(
        events
            .iter()
            .any(|event| !event.public && event.payload["message"] == "cleanup diagnostic")
    );
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
fn conversational_prompt_founds_a_tool_inert_agent_and_answers_directly() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path(),
    );
    let handle = service.handle();
    create(&handle, "conversation");
    handle
        .submit_prompt("conversation", prompt("hello", "hello"))
        .unwrap();

    let events = wait_terminal(&handle, "conversation", "hello");
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::Assistant)
    );
    assert!(events.iter().all(|event| event.kind != EventKind::ToolCall));
    let installed = events
        .iter()
        .find(|event| event.kind == EventKind::Capability && event.payload["state"] == "installed")
        .unwrap();
    assert_eq!(installed.payload["tools"], serde_json::json!([]));
}

#[test]
fn greeting_then_tool_prompt_is_rebrokered_with_a_fresh_prompt_lease() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path(),
    );
    let handle = service.handle();
    create(&handle, "sequential");
    let first = prompt("p1", "hello");
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
        2
    );
    for prompt_id in ["p1", "p2"] {
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.kind == EventKind::BrokerProposal
                        && event.prompt_id.as_deref() == Some(prompt_id)
                })
                .count(),
            1,
            "each prompt must have its own broker decision"
        );
    }
    let installed = events
        .iter()
        .filter(|event| {
            event.kind == EventKind::Capability && event.payload["state"] == "installed"
        })
        .collect::<Vec<_>>();
    assert_eq!(installed.len(), 2);
    assert_eq!(installed[0].payload["tools"], serde_json::json!([]));
    assert_eq!(installed[0].payload["capabilities"], serde_json::json!([]));
    assert_eq!(installed[1].payload["tools"], serde_json::json!(["cat"]));
    assert_eq!(
        installed[1].payload["capabilities"],
        serde_json::json!([
            {"kind":"object", "label":"workspace/X"},
            {"kind":"executable", "label":"cat"}
        ])
    );
    assert_ne!(installed[0].payload["agent"], installed[1].payload["agent"]);
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event.kind == EventKind::Capability && event.payload["state"] == "teardown"
            )
            .count(),
        2
    );
    assert!(events.iter().any(|event| event.kind == EventKind::ToolCall));
    assert!(
        events
            .iter()
            .any(|event| event.kind == EventKind::ToolResult)
    );
    let worker_turns = events
        .iter()
        .filter(|event| event.kind == EventKind::Inference && event.payload["state"] == "submitted")
        .filter_map(|event| event.payload["turn"].as_u64())
        .collect::<Vec<_>>();
    assert_eq!(worker_turns.first(), Some(&1));
    assert!(worker_turns.iter().any(|turn| *turn > 1));
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
        &|| true,
    );
    assert_eq!(status, PromptStatus::Completed);
    runtime.reset_backend();
    let status = runtime.run_prompt(
        &session,
        &prompt("p2", "read X"),
        &mut |kind, _, _| events.push(kind),
        &|| false,
        &|| true,
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
    let server = HarnessHttpServer::bind_with_state(
        "127.0.0.1:0".parse().unwrap(),
        service.handle(),
        temp.path().join("projects"),
    )
    .unwrap();
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

#[test]
fn loopback_http_api_accepts_host_headers_without_ports() {
    let temp = tempfile::tempdir().unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path(),
    );
    let server = HarnessHttpServer::bind_with_state(
        "127.0.0.1:0".parse().unwrap(),
        service.handle(),
        temp.path().join("projects"),
    )
    .unwrap();
    let address = server.local_addr();

    for host in ["127.0.0.1", "localhost", "LOCALHOST", "[::1]"] {
        let (tx, rx) = mpsc::channel();
        let request_host = host.to_owned();
        let client = thread::spawn(move || {
            tx.send(http_with_host(address, "GET", "/", "", &request_host))
                .unwrap();
        });
        server.poll(Duration::from_secs(1)).unwrap();
        let response = rx.recv().unwrap();
        client.join().unwrap();
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "host {host:?} was rejected: {response}"
        );
    }
}

fn http(address: std::net::SocketAddr, method: &str, path: &str, body: &str) -> String {
    http_with_host(address, method, path, body, &address.to_string())
}

fn http_with_host(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: &str,
    host: &str,
) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
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

#[test]
fn preparation_is_async_and_queued_projects_keep_their_scope_and_fair_turns() {
    use std::sync::{Arc, Mutex};
    struct Prepared {
        release: mpsc::Receiver<()>,
        entered: mpsc::Sender<()>,
        seen: Arc<Mutex<Vec<(String, String)>>>,
    }
    impl TaskRuntime for Prepared {
        fn prepare(
            &mut self,
            status: &mut dyn FnMut(Value),
            _: &dyn Fn() -> bool,
        ) -> Result<(), String> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            status(serde_json::json!({"phase":"broker_ready"}));
            Ok(())
        }
        fn run_prompt(
            &mut self,
            session: &SessionState,
            prompt: &PromptRequest,
            _: &mut dyn FnMut(EventKind, bool, Value),
            _: &dyn Fn() -> bool,
            _: &dyn Fn() -> bool,
        ) -> PromptStatus {
            self.seen
                .lock()
                .unwrap()
                .push((session.project_id.clone(), prompt.prompt_id.clone()));
            PromptStatus::Completed
        }
        fn close(&mut self, _: &SessionState, _: &mut dyn FnMut(EventKind, bool, Value)) {}
    }
    let temp = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (release, wait) = mpsc::channel();
    let (entered, entered_wait) = mpsc::channel();
    let service = HarnessService::start(
        Prepared {
            release: wait,
            entered,
            seen: seen.clone(),
        },
        temp.path(),
    );
    entered_wait.recv_timeout(Duration::from_secs(2)).unwrap();
    let handle = service.handle();
    for name in ["alpha", "beta", "closed"] {
        handle
            .create_session(CreateSession {
                api_version: 1,
                project_id: name.into(),
                session_id: name.into(),
            })
            .unwrap();
    }
    for (name, id) in [
        ("alpha", "a1"),
        ("alpha", "a2"),
        ("beta", "b1"),
        ("closed", "c1"),
    ] {
        handle.submit_prompt(name, prompt(id, "hello")).unwrap();
    }
    handle.close_session("closed").unwrap();
    assert!(seen.lock().unwrap().is_empty());
    release.send(()).unwrap();
    wait_terminal(&handle, "alpha", "a2");
    wait_terminal(&handle, "closed", "c1");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("alpha".into(), "a1".into()),
            ("beta".into(), "b1".into()),
            ("alpha".into(), "a2".into())
        ]
    );
    assert_eq!(
        handle.state("closed").unwrap().prompts["c1"],
        PromptStatus::Cancelled
    );
    service.shutdown();
}

#[test]
fn shared_failure_invalidates_other_queued_generations_before_running_them() {
    struct FailsOnce {
        incarnation: u64,
        failed: bool,
    }
    impl TaskRuntime for FailsOnce {
        fn run_prompt(
            &mut self,
            _: &SessionState,
            prompt: &PromptRequest,
            _: &mut dyn FnMut(EventKind, bool, Value),
            _: &dyn Fn() -> bool,
            _: &dyn Fn() -> bool,
        ) -> PromptStatus {
            assert_ne!(prompt.prompt_id, "stale");
            self.failed = true;
            PromptStatus::Failed
        }
        fn close(&mut self, _: &SessionState, _: &mut dyn FnMut(EventKind, bool, Value)) {}
        fn runtime_status(&mut self) -> Option<Value> {
            if self.failed && self.incarnation == 1 {
                self.incarnation = 2;
            }
            None
        }
        fn backend_incarnation(&self) -> u64 {
            self.incarnation
        }
    }
    // A preparation barrier makes both submissions definitely precede failure.
    struct Barrier {
        inner: FailsOnce,
        release: mpsc::Receiver<()>,
    }
    impl TaskRuntime for Barrier {
        fn prepare(
            &mut self,
            _: &mut dyn FnMut(Value),
            _: &dyn Fn() -> bool,
        ) -> Result<(), String> {
            self.release.recv().unwrap();
            Ok(())
        }
        fn run_prompt(
            &mut self,
            s: &SessionState,
            p: &PromptRequest,
            e: &mut dyn FnMut(EventKind, bool, Value),
            c: &dyn Fn() -> bool,
            r: &dyn Fn() -> bool,
        ) -> PromptStatus {
            self.inner.run_prompt(s, p, e, c, r)
        }
        fn close(&mut self, s: &SessionState, e: &mut dyn FnMut(EventKind, bool, Value)) {
            self.inner.close(s, e);
        }
        fn runtime_status(&mut self) -> Option<Value> {
            self.inner.runtime_status()
        }
        fn backend_incarnation(&self) -> u64 {
            self.inner.incarnation
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let (send, release) = mpsc::channel();
    let service = HarnessService::start(
        Barrier {
            inner: FailsOnce {
                incarnation: 1,
                failed: false,
            },
            release,
        },
        temp.path(),
    );
    let handle = service.handle();
    let old = create(&handle, "first");
    create(&handle, "second");
    handle
        .submit_prompt("first", prompt("failure", "hello"))
        .unwrap();
    handle
        .submit_prompt("second", prompt("stale", "hello"))
        .unwrap();
    send.send(()).unwrap();
    wait_terminal(&handle, "second", "stale");
    assert_eq!(
        handle.state("second").unwrap().prompts["stale"],
        PromptStatus::Cancelled
    );
    assert_ne!(handle.state("first").unwrap().generation, old.generation);
    assert_eq!(handle.state("second").unwrap().backend_incarnation, 2);
    service.shutdown();
}

#[test]
fn app_shutdown_cancels_preparation_and_terminalizes_queued_work() {
    struct Starting(mpsc::Sender<()>);
    impl TaskRuntime for Starting {
        fn prepare(
            &mut self,
            _: &mut dyn FnMut(Value),
            cancelled: &dyn Fn() -> bool,
        ) -> Result<(), String> {
            self.0.send(()).unwrap();
            while !cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            Err("startup cancelled".into())
        }
        fn run_prompt(
            &mut self,
            _: &SessionState,
            _: &PromptRequest,
            _: &mut dyn FnMut(EventKind, bool, Value),
            _: &dyn Fn() -> bool,
            _: &dyn Fn() -> bool,
        ) -> PromptStatus {
            panic!("shutdown must not run queued work");
        }
        fn close(&mut self, _: &SessionState, _: &mut dyn FnMut(EventKind, bool, Value)) {}
    }
    let temp = tempfile::tempdir().unwrap();
    let (entered, wait) = mpsc::channel();
    let service = HarnessService::start(Starting(entered), temp.path());
    let handle = service.handle();
    wait.recv_timeout(Duration::from_secs(2)).unwrap();
    create(&handle, "queued");
    for index in 0..32 {
        handle
            .submit_prompt("queued", prompt(&format!("p{index}"), "hello"))
            .unwrap();
    }
    handle.close_session("queued").unwrap();
    service.shutdown();
    assert_eq!(
        handle.state("queued").unwrap().prompts["p0"],
        PromptStatus::Cancelled
    );
    assert_eq!(
        handle
            .events("queued", 0)
            .unwrap()
            .iter()
            .filter(|e| e.kind == EventKind::PromptTerminal)
            .count(),
        32
    );
}
