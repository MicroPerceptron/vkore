//! Opt-in native lifecycle gate. This starts a real VM and llama.cpp server;
//! it is excluded from the ordinary host suite and never supplies fake guest
//! readiness. See docs/supervisor-acceptance.md for the isolated configuration.
use super::*;

#[test]
#[ignore = "requires an isolated real-stack configuration and exclusive QEMU use"]
fn three_supervisor_restarts_preserve_the_native_vm() {
    on_native_stack(run_native_supervisor_gate);
}

#[test]
#[ignore = "requires an isolated real-stack configuration and exclusive QEMU use"]
fn native_frontend_prompt_before_recovery() {
    on_native_stack(|| {
        let mut runtime = configured_runtime();
        let guest = runtime.runtime.as_mut().unwrap();
        let intent = guest
            .daemon
            .queue_frontend_intent("What is 17 plus 25? Answer with the number only.")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut published = false;
        loop {
            guest.daemon.tick().unwrap();
            guest.drain_events(&mut |kind, _, event| eprintln!("{kind:?} {event}"));
            if let Some(result) = guest.daemon.take_frontend_result() {
                assert_eq!(result.intent_id, intent);
                eprintln!(
                    "{}",
                    json!({"gate":"native_frontend", "kind":format!("{:?}",result.kind),"text":result.text})
                );
                assert_eq!(
                    result.kind,
                    FrontendResultKind::Completed,
                    "{}",
                    result.text
                );
                assert_eq!(result.text.trim(), "42");
                published = true;
            }
            if published {
                guest.daemon.acknowledge_frontend_result(intent);
                if guest.daemon.core_mut().finish_frontend_intent(intent) {
                    assert!(!guest.vm_suspended);
                    assert_eq!(guest.boot_attempt, 1);
                    eprintln!(
                        "{}",
                        json!({"gate":"native_frontend","result":"ready_after_receipt"})
                    );
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "fresh native prompt did not complete and retire"
            );
            std::thread::sleep(DEFAULT_TICK);
        }
    });
}

#[test]
#[ignore = "writes test.txt; requires an isolated backed-up project and exclusive QEMU use"]
fn native_frontend_file_roundtrip() {
    on_native_stack(|| run_file_gate(false));
}

#[test]
#[ignore = "requires a backed-up existing test.txt fixture and exclusive real guest/model"]
fn native_existing_file_collision_preserves_bytes() {
    on_native_stack(|| run_file_gate(true));
}

fn run_file_gate(existing: bool) {
    let file = std::env::var_os("VKORE_SUPERVISOR_GATE_CONFIG").unwrap();
    let input: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let project_id = input["file_project_id"].as_str().expect("file_project_id");
    let backup = PathBuf::from(input["fixture_backup"].as_str().expect("fixture_backup"));
    assert!(
        backup.is_dir(),
        "back up the isolated fixture before this gate"
    );
    let projects = PathBuf::from(input["project_root"].as_str().unwrap());
    let project = ProjectStore::open(&projects)
        .unwrap()
        .get(project_id)
        .unwrap();
    assert_eq!(
        project.workspace.access,
        WorkspaceAccess::AutomaticWriteback
    );
    assert!(
        project.workspace.state.is_dir(),
        "project transaction state must exist (create the fixture through ProjectStore)"
    );
    let target = project.workspace.root.join("test.txt");
    if existing {
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
        assert_eq!(std::fs::read(backup.join("test.txt")).unwrap(), b"hello");
    } else {
        assert!(
            !target.exists() && !backup.join("test.txt").exists(),
            "test.txt must be absent; never delete a preexisting fixture"
        );
    }
    assert!(!project.workspace.root.join("missing-file.txt").exists());
    let mut runtime = configured_runtime();
    let session = SessionState {
        api_version: 1,
        project_id: project_id.into(),
        session_id: "native-file-gate".into(),
        status: crate::SessionStatus::Open,
        generation: 1,
        backend_incarnation: 0,
        last_event_sequence: 0,
        prompts: BTreeMap::new(),
    };
    let collision_prompts = [
        "Use the create tool to create a new empty file named test.txt. If creation fails because the file exists, report that error and leave its contents unchanged.",
        "What is 17 plus 25? Answer with the number only.",
    ];
    let file_prompts = [
        "Create a file named test.txt containing the word hello.",
        "Read test.txt and reply with its exact contents.",
        "Read missing-file.txt and reply with its contents.",
        "What is 17 plus 25? Answer with the number only.",
    ];
    let prompts = if existing {
        &collision_prompts[..]
    } else {
        &file_prompts[..]
    };
    let mut pid = None;
    for (index, text) in prompts.iter().copied().enumerate() {
        let prompt = PromptRequest {
            api_version: 1,
            prompt_id: format!("file-gate-{index}"),
            idempotency_key: format!("file-gate-{index}"),
            text: text.into(),
        };
        let mut events = Vec::new();
        let status = runtime.run_prompt(&session, &prompt, &mut |kind, public, event| {
                eprintln!("{}", json!({"gate":"native_files","prompt":index,"kind":format!("{kind:?}"),"public":public,"event":event}));
                events.push((kind, event));
            }, &|| false, &|| true);
        eprintln!(
            "{}",
            json!({"gate":"native_files","prompt":index,"status":format!("{status:?}")})
        );
        let text = events
            .iter()
            .filter(|(kind, _)| {
                matches!(
                    kind,
                    EventKind::Assistant | EventKind::Error | EventKind::ToolResult
                )
            })
            .map(|(_, event)| event.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        if (existing && index == 1) || (!existing && index != 2) {
            assert_eq!(status, PromptStatus::Completed, "{text}");
        }
        assert_eq!(
            std::fs::read(&target).expect("real host writeback"),
            b"hello"
        );
        let current_pid = runtime
            .runtime
            .as_ref()
            .unwrap()
            .process
            .as_ref()
            .unwrap()
            .id();
        assert_eq!(*pid.get_or_insert(current_pid), current_pid);
        if existing && index == 0 {
            assert!(
                events.iter().any(|(kind, event)| {
                    *kind == EventKind::ToolResult
                        && event
                            .to_string()
                            .to_ascii_lowercase()
                            .contains("already exists")
                }),
                "native create must report the actual existing-name refusal: {text}"
            );
        }
        match if existing {
            if index == 1 { 3 } else { usize::MAX }
        } else {
            index
        } {
            1 => assert!(text.contains("hello"), "{text}"),
            2 => {
                let lower = text.to_ascii_lowercase();
                assert!(
                    lower.contains("not found")
                        || lower.contains("does not exist")
                        || lower.contains("no such file"),
                    "missing-file refusal: {text}"
                );
            }
            3 => assert!(
                events
                    .iter()
                    .any(|(kind, event)| *kind == EventKind::Assistant
                        && event["text"]
                            .as_str()
                            .is_some_and(|text| text.trim() == "42")),
                "{text}"
            ),
            _ => {}
        }
        assert!(!runtime.runtime.as_ref().unwrap().vm_suspended);
    }
}

#[test]
#[ignore = "mutates an isolated backed-up project; requires real guest/model and exclusive QEMU"]
fn native_transaction_reply_faults_preserve_effects_and_vm() {
    on_native_stack(|| {
        use std::sync::atomic::Ordering;
        let input: Value = serde_json::from_slice(
            &std::fs::read(std::env::var_os("VKORE_SUPERVISOR_GATE_CONFIG").unwrap()).unwrap(),
        )
        .unwrap();
        let project_id = input["file_project_id"].as_str().unwrap();
        let project = ProjectStore::open(input["project_root"].as_str().unwrap())
            .unwrap()
            .get(project_id)
            .unwrap();
        assert!(PathBuf::from(input["fixture_backup"].as_str().unwrap()).is_dir());
        assert!(project.workspace.state.is_dir());
        let names = ["interrupted-disconnect.txt", "interrupted-stop.txt"];
        for name in names {
            assert!(!project.workspace.root.join(name).exists());
        }
        let mut runtime = configured_runtime();
        runtime
            .ensure_project(project_id, "", &mut |_, _, e| eprintln!("{e}"))
            .unwrap();
        runtime
            .prepare(&mut |e| eprintln!("{e}"), &|| false)
            .unwrap();
        let pid = runtime
            .runtime
            .as_ref()
            .unwrap()
            .process
            .as_ref()
            .unwrap()
            .id();
        let journal = project.workspace.state.join("workspace-transactions.v1");
        for (index, name) in names.into_iter().enumerate() {
            let previous = runtime.supervisor.unwrap();
            let guest = runtime.runtime.as_mut().unwrap();
            let gate = guest
                .daemon
                .with_idle_backend(|backend| backend.gate_next_transaction_reply())
                .unwrap();
            let intent = guest
                .daemon
                .queue_frontend_intent(&format!("Create an empty file named {name}."))
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(300);
            while gate.load(Ordering::SeqCst) != 1 {
                guest.daemon.tick().unwrap();
                guest.drain_events(&mut |kind, _, e| eprintln!("{kind:?} {e}"));
                if let Some(result) = guest.daemon.take_frontend_result() {
                    eprintln!(
                        "{}",
                        json!({"gate":"native_transaction_fault",
                        "file":name,"intent":format!("{intent:?}"),
                        "result_intent":format!("{:?}",result.intent_id),
                        "kind":format!("{:?}",result.kind),"text":result.text})
                    );
                    panic!("prompt ended before held transaction reply");
                }
                assert!(
                    Instant::now() < deadline,
                    "transaction fault boundary not reached"
                );
                std::thread::sleep(DEFAULT_TICK);
            }
            assert_eq!(
                std::fs::read(project.workspace.root.join(name)).unwrap(),
                b""
            );
            let committed = std::fs::read(&journal).unwrap();
            eprintln!(
                "{}",
                json!({"gate":"native_transaction_fault", "file":name,
                "boundary":"host committed; reply withheld", "qemu_pid":pid})
            );
            if index == 0 {
                guest.daemon.force_disconnect();
            }
            guest.daemon.cancel_frontend_intent(intent);
            guest.recover_supervisor(&mut |_, _, e| eprintln!("{e}"));
            // The stop has been requested while the real operation is awaiting
            // its reply. Release that original outcome so cleanup can resolve
            // actual custody; never replace it with a fabricated cancellation.
            gate.store(2, Ordering::SeqCst);
            runtime
                .prepare(&mut |e| eprintln!("{e}"), &|| false)
                .unwrap();
            let next = runtime.supervisor.unwrap();
            assert_eq!(next.generation.get(), previous.generation.get() + 1);
            assert_ne!(next.route, previous.route);
            let guest = runtime.runtime.as_mut().unwrap();
            assert_eq!(guest.process.as_ref().unwrap().id(), pid);
            assert!(
                guest
                    .process
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .is_none()
            );
            assert!(!guest.vm_suspended);
            assert!(!guest.recovering);
            assert!(guest.daemon.take_frontend_result().is_none());
            assert_eq!(
                std::fs::read(project.workspace.root.join(name)).unwrap(),
                b""
            );
            assert_eq!(
                std::fs::read(&journal).unwrap(),
                committed,
                "retirement replayed a write transaction"
            );
            eprintln!(
                "{}",
                json!({"gate":"native_transaction_fault", "file":name,
                "generation":next.generation.get(), "result":"ready; original effect retained without replay"})
            );
        }
    });
}

#[test]
#[ignore = "requires a pinned KORE_AGENT_SUPERVISOR_PROJECT=0 image and exclusive QEMU"]
fn native_no_project_refuses_files_and_answers_arithmetic() {
    on_native_stack(|| {
        let mut runtime = configured_runtime();
        let guest = runtime.runtime.as_mut().unwrap();
        let pid = guest.process.as_ref().unwrap().id();
        for (text, expected) in [
            (
                "Create a file named forbidden.txt containing the word secret.",
                FrontendResultKind::Refused,
            ),
            (
                "What is 17 plus 25? Answer with the number only.",
                FrontendResultKind::Completed,
            ),
        ] {
            let intent = guest.daemon.queue_frontend_intent(text).unwrap();
            let deadline = Instant::now() + Duration::from_secs(300);
            let mut published = false;
            loop {
                guest.daemon.tick().unwrap();
                guest.drain_events(&mut |kind, _, e| eprintln!("{kind:?} {e}"));
                if let Some(result) = guest.daemon.take_frontend_result() {
                    assert_eq!(result.intent_id, intent);
                    assert_eq!(result.kind, expected, "{}", result.text);
                    if expected == FrontendResultKind::Completed {
                        assert_eq!(result.text.trim(), "42");
                    }
                    eprintln!(
                        "{}",
                        json!({"gate":"native_no_project", "text":result.text, "kind":format!("{:?}", result.kind)})
                    );
                    published = true;
                }
                if published {
                    guest.daemon.acknowledge_frontend_result(intent);
                    if guest.daemon.core_mut().finish_frontend_intent(intent) {
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "no-project prompt did not retire"
                );
                std::thread::sleep(DEFAULT_TICK);
            }
            assert_eq!(guest.process.as_ref().unwrap().id(), pid);
            assert!(!guest.vm_suspended);
        }
    });
}

#[test]
#[ignore = "requires isolated authority.txt fixture, real model and exclusive QEMU"]
fn native_live_authority_revocation_preserves_vm() {
    on_native_stack(|| {
        use std::cell::Cell;
        let input: Value = serde_json::from_slice(
            &std::fs::read(std::env::var_os("VKORE_SUPERVISOR_GATE_CONFIG").unwrap()).unwrap(),
        )
        .unwrap();
        let project_id = input["file_project_id"].as_str().unwrap();
        let project = ProjectStore::open(input["project_root"].as_str().unwrap())
            .unwrap()
            .get(project_id)
            .unwrap();
        let fixture = project.workspace.root.join("authority.txt");
        let before = std::fs::read(&fixture).unwrap();
        assert_eq!(before, b"authority fixture");
        assert!(
            PathBuf::from(input["fixture_backup"].as_str().unwrap())
                .join("authority.txt")
                .is_file()
        );
        let mut runtime = configured_runtime();
        runtime
            .ensure_project(project_id, "", &mut |_, _, e| eprintln!("{e}"))
            .unwrap();
        runtime
            .prepare(&mut |e| eprintln!("{e}"), &|| false)
            .unwrap();
        let pid = runtime
            .runtime
            .as_ref()
            .unwrap()
            .process
            .as_ref()
            .unwrap()
            .id();
        let session = SessionState {
            api_version: 1,
            project_id: project_id.into(),
            session_id: "native-authority-gate".into(),
            status: crate::SessionStatus::Open,
            generation: 1,
            backend_incarnation: 0,
            last_event_sequence: 0,
            prompts: BTreeMap::new(),
        };
        let prompt = PromptRequest { api_version:1, prompt_id:"authority-revoke".into(), idempotency_key:"authority-revoke".into(),
            text:"Read authority.txt five times using five separate read tool calls, then report the last contents.".into() };
        let worker_started = Cell::new(false);
        let sent = Cell::new(false);
        let applied = Cell::new(false);
        let installed_file = Cell::new(false);
        let revoked_file = Cell::new(false);
        let status = runtime.run_prompt_with_updates(&session, &prompt, &mut |kind, public, event| {
            eprintln!("{}", json!({"gate":"native_authority", "kind":format!("{kind:?}"), "public":public, "event":event}));
            if event["kind"] == "Input" && event["role"] == "Agent" { worker_started.set(true); }
            if kind == EventKind::AuthorityUpdate && event["status"] == "applied" { applied.set(true); }
            if kind == EventKind::Capability && matches!(event["kind"].as_str(), Some("View" | "Object")) {
                if event["state"] == "Installed" { installed_file.set(true); }
                if event["state"] == "Revoked" { revoked_file.set(true); }
            }
        }, &|| false, &|| true, &|| {
            if worker_started.get() && !sent.replace(true) {
                Some(AuthorityUpdateRequest { api_version:1, prompt_id:prompt.prompt_id.clone(), update_id:"remove-all-files-and-tools".into(),
                    text:"Revoke all filesystem resources and all executable tools from this agent. Use an empty resource set, empty executable set and zero tool calls. Finish conversationally without accessing any files.".into() })
            } else { None }
        });
        assert!(
            sent.get() && installed_file.get(),
            "did not reach a real endowed worker"
        );
        assert!(
            applied.get() && revoked_file.get(),
            "native revoke and resumed-update receipt were not confirmed: {status:?}"
        );
        let guest = runtime.runtime.as_ref().unwrap();
        assert_eq!(guest.process.as_ref().unwrap().id(), pid);
        assert!(!guest.vm_suspended && !guest.recovering);
        assert_eq!(std::fs::read(&fixture).unwrap(), before);
        let mut answer = None;
        let status = runtime.run_prompt(
            &session,
            &PromptRequest {
                api_version: 1,
                prompt_id: "authority-followup".into(),
                idempotency_key: "authority-followup".into(),
                text: "What is 17 plus 25? Answer with the number only.".into(),
            },
            &mut |kind, _, event| {
                eprintln!("{kind:?} {event}");
                if kind == EventKind::Assistant {
                    answer = event["text"].as_str().map(str::to_owned);
                }
            },
            &|| false,
            &|| true,
        );
        assert_eq!(status, PromptStatus::Completed);
        assert_eq!(answer.as_deref().map(str::trim), Some("42"));
        assert_eq!(
            runtime
                .runtime
                .as_ref()
                .unwrap()
                .process
                .as_ref()
                .unwrap()
                .id(),
            pid
        );
        eprintln!(
            "{}",
            json!({"gate":"native_authority", "result":"revoked, resumed and followup completed", "qemu_pid":pid})
        );
    });
}

fn on_native_stack(run: fn()) {
    // Debug workspace-image construction carries large bounded temporaries.
    // Match the workspace image tests instead of overflowing libtest's small
    // default stack before a VM has even been started.
    std::thread::Builder::new()
        .name("native-supervisor-gate".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}

fn configured_runtime() -> KoreVmRuntime {
    let file = std::env::var_os("VKORE_SUPERVISOR_GATE_CONFIG")
        .expect("set VKORE_SUPERVISOR_GATE_CONFIG to the gate JSON file");
    let input: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let path = |key: &str| PathBuf::from(input[key].as_str().expect(key));
    let mut runtime = KoreVmRuntime::new(KoreVmRuntimeConfig {
        qemu_executable: path("qemu_executable"),
        project_root: path("project_root"),
        qemu_arguments: input["qemu_arguments"]
            .as_array()
            .expect("qemu_arguments must be an array")
            .iter()
            .map(|arg| OsString::from(arg.as_str().expect("QEMU argument must be text")))
            .collect(),
        require_real: true,
        headless_park: false,
    })
    .unwrap();
    let profile = runtime.application_profile().unwrap();
    let iso = profile
        .inference
        .kore_image
        .as_ref()
        .expect("configured ISO");
    let digest = blake3::hash(&std::fs::read(iso).unwrap())
        .to_hex()
        .to_string();
    assert_eq!(digest, input["iso_blake3"].as_str().expect("iso_blake3"));
    eprintln!(
        "{}",
        json!({"gate":"native_supervisor", "iso":iso, "blake3":digest})
    );
    runtime
        .prepare(&mut |status| eprintln!("{status}"), &|| false)
        .unwrap();
    runtime
}

fn run_native_supervisor_gate() {
    let mut runtime = configured_runtime();
    let pid = runtime
        .runtime
        .as_ref()
        .unwrap()
        .process
        .as_ref()
        .unwrap()
        .id();
    let mut previous = runtime.supervisor.unwrap();
    for cycle in 1..=3 {
        runtime
            .runtime
            .as_mut()
            .unwrap()
            .recover_supervisor(&mut |_, _, event| eprintln!("{event}"));
        runtime
            .prepare(&mut |status| eprintln!("{status}"), &|| false)
            .unwrap();
        let next = runtime.supervisor.unwrap();
        assert_eq!(
            next.generation.get(),
            previous.generation.get().checked_add(1).unwrap()
        );
        // HostLink derives a fresh broker route from each successor
        // generation after acknowledging closure of the old route.
        assert_ne!(
            next.route, previous.route,
            "recovery reused a retired route"
        );
        let guest = runtime.runtime.as_mut().unwrap();
        let process = guest.process.as_mut().unwrap();
        assert_eq!(process.id(), pid, "supervisor recovery replaced the OS");
        assert!(
            process.try_wait().unwrap().is_none(),
            "OS exited during recovery"
        );
        assert_eq!(guest.boot_attempt, 1, "recovery booted another VM");
        assert!(!guest.vm_suspended, "interactive desktop was parked");
        assert!(!guest.recovering);
        eprintln!(
            "{}",
            json!({"gate":"native_supervisor", "cycle":cycle,
            "qemu_pid":pid,"generation":next.generation.get(),"result":"ready"})
        );
        previous = next;
    }
    let previous_epoch = runtime
        .runtime
        .as_ref()
        .unwrap()
        .daemon
        .core()
        .epoch()
        .unwrap();
    runtime.runtime.as_mut().unwrap().daemon.force_disconnect();
    runtime
        .prepare(&mut |status| eprintln!("{status}"), &|| false)
        .unwrap();
    let next = runtime.supervisor.unwrap();
    assert_eq!(next.generation.get(), previous.generation.get() + 1);
    assert_ne!(next.route, previous.route);
    let guest = runtime.runtime.as_mut().unwrap();
    assert_ne!(guest.daemon.core().epoch(), Some(previous_epoch));
    let process = guest.process.as_mut().unwrap();
    assert_eq!(process.id(), pid, "transport loss replaced the OS");
    assert!(process.try_wait().unwrap().is_none());
    assert_eq!(guest.boot_attempt, 1);
    assert!(!guest.vm_suspended);
    assert!(!guest.recovering);
    eprintln!(
        "{}",
        json!({"gate":"native_supervisor", "fault":"host_disconnect",
        "qemu_pid":pid,"generation":next.generation.get(),"result":"ready"})
    );
    // Fault only after observing real progress. A deadline is a failure,
    // never a substitute for reaching the requested boundary.
    for fault in ["inference_disconnect", "response_before_receipt_stop"] {
        let previous = runtime.supervisor.unwrap();
        let guest = runtime.runtime.as_mut().unwrap();
        let intent = guest
            .daemon
            .queue_frontend_intent("What is 17 plus 25? Answer with the number only.")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            guest.daemon.tick().unwrap();
            guest.drain_events(&mut |kind, _, event| eprintln!("{kind:?} {event}"));
            if fault == "inference_disconnect" {
                if guest.daemon.core().backend_metrics().pending > 0 {
                    guest.daemon.force_disconnect();
                    assert!(guest.daemon.core().frontend_interrupted(intent));
                    break;
                }
                if let Some(result) = guest.daemon.take_frontend_result() {
                    panic!(
                        "prompt ended before the inference fault boundary: {:?}: {}",
                        result.kind, result.text
                    );
                }
            } else if let Some(result) = guest.daemon.take_frontend_result() {
                assert_eq!(result.intent_id, intent);
                assert_eq!(result.kind, FrontendResultKind::Completed);
                assert_eq!(result.text.trim(), "42");
                // Preserve the observed response, but deliberately withhold
                // its receipt so native worker retirement is still pending.
                eprintln!("{}", json!({"boundary":fault,"text":result.text}));
                assert!(!guest.daemon.core().frontend_ready(intent));
                break;
            }
            assert!(Instant::now() < deadline, "did not reach {fault}");
            std::thread::sleep(DEFAULT_TICK);
        }
        // Match the runtime's terminalization/containment order. Never
        // requeue the interrupted intent, even after successful recovery.
        guest.daemon.cancel_frontend_intent(intent);
        guest.recover_supervisor(&mut |_, _, event| eprintln!("{event}"));
        runtime
            .prepare(&mut |status| eprintln!("{status}"), &|| false)
            .unwrap();
        let next = runtime.supervisor.unwrap();
        assert_eq!(next.generation.get(), previous.generation.get() + 1);
        assert_ne!(next.route, previous.route);
        let guest = runtime.runtime.as_mut().unwrap();
        let process = guest.process.as_mut().unwrap();
        assert_eq!(process.id(), pid, "{fault} replaced the OS");
        assert!(process.try_wait().unwrap().is_none());
        assert_eq!(guest.boot_attempt, 1);
        assert!(!guest.vm_suspended);
        assert!(!guest.recovering);
        assert!(
            guest.daemon.take_frontend_result().is_none(),
            "stale result after {fault}"
        );
        eprintln!(
            "{}",
            json!({"gate":"native_supervisor","fault":fault,
            "qemu_pid":pid,"generation":next.generation.get(),"result":"ready"})
        );
    }
    // ProjectRuntime's Drop reaps only the VM owned by this test. Inference
    // and receipt boundaries do not certify in-flight filesystem effects,
    // desktop input or MM reclamation.
}
