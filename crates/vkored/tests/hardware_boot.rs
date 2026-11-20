//! Opt-in end-to-end resource path, with a private disk and no host workspace.
use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::{Duration, Instant},
};
use vkored::{MachineConfig, MachineStore, Session, State};

#[test]
fn minimum_hardware_reserves_a_disk_and_boots_through_the_session() {
    boot_and_edit_command(true);
}

#[test]
fn terminal_keyboard_edits_and_executes_in_a_real_guest() {
    boot_and_edit_command(false);
}

fn boot_and_edit_command(reserve_storage: bool) {
    let (Some(qemu), Some(iso)) = (
        std::env::var_os("VKORE_TEST_QEMU"),
        std::env::var_os("VKORE_TEST_ISO"),
    ) else {
        eprintln!("skipped: set VKORE_TEST_QEMU and VKORE_TEST_ISO for a production guest");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let store = MachineStore::new(State::open(root.path()).unwrap());
    let mut config = MachineConfig::new("hardware", PathBuf::from(iso));
    config.cpus = 2;
    config.memory_mib = 2048;
    config.disk_mib = 1024;
    config.resources.reserve_storage = reserve_storage;
    // This test accepts any production boot ISO. Let that guest create its
    // disk through its own engine; no host-format dependency or workspace.
    config.screenshots_folder = false;
    store.create(&config).unwrap();
    let mut console = None;
    let mut session = Session::start(
        &store,
        "hardware",
        &PathBuf::from(qemu),
        vkore_vm::Display::Headless,
        &mut |event| {
            if let vkored::Event::Console { socket } = event {
                console = Some(vkored::console::Console::connect(socket).unwrap());
            }
        },
    )
    .unwrap();
    assert!(
        vkored::hardware::save(&store, "hardware", 2, 2048, 1024, config.resources.clone())
            .unwrap_err()
            .contains("busy")
    );
    let disk = store.state().machine("hardware").disk;
    let metadata = std::fs::metadata(&disk).unwrap();
    assert_eq!(metadata.len(), 1 << 30);
    if reserve_storage {
        assert!(metadata.blocks() * 512 >= metadata.len());
    }
    let deadline = Instant::now() + Duration::from_secs(900);
    while session.stage() < vkore_vm::Stage::Ready && Instant::now() < deadline {
        if let Some(console) = &mut console {
            console.poll();
        }
        assert!(
            session
                .poll(Duration::from_millis(250), &mut |_| {})
                .unwrap(),
            "guest exited before ready"
        );
    }
    assert_eq!(
        session.stage(),
        vkore_vm::Stage::Ready,
        "{:?}",
        session.machine().readiness()
    );
    {
        let console = console
            .as_mut()
            .expect("guest must expose its shell console");
        // Match the existing Kore console gate: help's answer and the next
        // prompt prove input, execution and output, rather than just a socket.
        let deadline = Instant::now() + Duration::from_secs(60);
        while !console.transcript().contains("terminal:0> ") && Instant::now() < deadline {
            console.poll();
            assert!(
                session
                    .poll(Duration::from_millis(100), &mut |_| {})
                    .unwrap()
            );
        }
        assert!(console.transcript().contains("terminal:0> "));
        // Type an intentional typo, erase it and submit through the same raw
        // input path as the desktop. Kore owns the editing and echo.
        for bytes in [b"h".as_slice(), b"e", b"x", b"\x7f", b"l", b"p", b"\r"] {
            console.send_input(bytes).unwrap();
        }
        loop {
            console.poll();
            let output = console.transcript();
            if output
                .split_once("available commands for this task:")
                .is_some_and(|(_, tail)| tail.contains("terminal:0> "))
            {
                break;
            }
            assert!(Instant::now() < deadline, "no console round trip: {output}");
            assert!(
                session
                    .poll(Duration::from_millis(100), &mut |_| {})
                    .unwrap()
            );
        }
    }
    session
        .machine()
        .screendump(&root.path().join("screen.ppm"))
        .unwrap();
    session.stop(Duration::from_secs(10), &mut |_| {}).unwrap();
    session.finish(&store, &mut |_| {}).unwrap();
    // Lease is released, settings can be persisted, disk remains allocated.
    vkored::hardware::save(&store, "hardware", 2, 2048, 1024, config.resources).unwrap();
    if reserve_storage {
        assert!(std::fs::metadata(disk).unwrap().blocks() * 512 >= 1 << 30);
    }
}
