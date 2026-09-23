//! Opt-in native lifecycle gate. This starts a real VM and llama.cpp server;
//! it is excluded from the ordinary host suite and never supplies fake guest
//! readiness. See docs/supervisor-acceptance.md for the isolated configuration.
use super::*;

#[test]
#[ignore = "requires an isolated real-stack configuration and exclusive QEMU use"]
fn three_supervisor_restarts_preserve_the_native_vm() {
    // Debug workspace-image construction carries large bounded temporaries.
    // Match the workspace image tests instead of overflowing libtest's small
    // default stack before a VM has even been started.
    std::thread::Builder::new()
        .name("native-supervisor-gate".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(run_native_supervisor_gate)
        .unwrap()
        .join()
        .unwrap();
}

fn run_native_supervisor_gate() {
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
    // ProjectRuntime's Drop reaps only the VM owned by this test. This gate
    // does not certify desktop input, in-flight tools or MM reclamation.
}
