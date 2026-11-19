//! Opt-in end-to-end resource path, with a private disk and no host workspace.
use std::{
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::{Duration, Instant},
};
use vkored::{MachineConfig, MachineStore, Session, State};

#[test]
fn minimum_hardware_reserves_a_disk_and_boots_through_the_session() {
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
    config.resources.reserve_storage = true;
    // This test accepts any production boot ISO. Let that guest create its
    // disk through its own engine; no host-format dependency or workspace.
    config.screenshots_folder = false;
    store.create(&config).unwrap();
    let mut session = Session::start(
        &store,
        "hardware",
        &PathBuf::from(qemu),
        vkore_vm::Display::Headless,
        &mut |_| {},
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
    assert!(metadata.blocks() * 512 >= metadata.len());
    let deadline = Instant::now() + Duration::from_secs(900);
    while session.stage() < vkore_vm::Stage::Ready && Instant::now() < deadline {
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
    session
        .machine()
        .screendump(&root.path().join("screen.ppm"))
        .unwrap();
    session.stop(Duration::from_secs(10), &mut |_| {}).unwrap();
    session.finish(&store, &mut |_| {}).unwrap();
    // Lease is released, settings can be persisted, disk remains allocated.
    vkored::hardware::save(&store, "hardware", 2, 2048, 1024, config.resources).unwrap();
    assert!(std::fs::metadata(disk).unwrap().blocks() * 512 >= 1 << 30);
}
