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
