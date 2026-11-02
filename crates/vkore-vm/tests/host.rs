//! Against this host's real QEMU and a real Kore ISO. Opt in with
//! `VKORE_TEST_QEMU` (a `qemu-system-x86_64`) and `VKORE_TEST_ISO` (a Kore
//! x86_64 ISO); without them each test says what it skipped.

use std::path::PathBuf;

use vkore_vm::{
    Accel, BootMedium, Display, HostCaps, Machine, MachineSpec, Paging, QemuCaps, Stage, Vendor,
    firmware, machine::private_vars,
};

fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name).map(PathBuf::from);
    if value.is_none() {
        eprintln!("skipped: set {name} to run this test");
    }
    value
}

#[test]
fn this_host_s_qemu_is_probed_not_guessed() {
    let Some(qemu) = env_path("VKORE_TEST_QEMU") else {
        return;
    };
    let caps = QemuCaps::probe(&qemu, Vendor::Intel, Accel::Tcg).unwrap();
    assert!(caps.version.0 >= 8, "{caps:?}");
    assert_eq!(caps.generic_initiators, caps.version.0 >= 9);
    assert!(matches!(
        caps.vtd_scalable_property,
        "scalable-mode" | "x-scalable-mode"
    ));
    let host = HostCaps::probe();
    assert_eq!(
        host.choose(vkore_vm::AccelChoice::Tcg, Vendor::Intel)
            .unwrap(),
        Accel::Tcg
    );
}

#[test]
fn a_real_kore_iso_names_its_paging_mode() {
    let Some(iso) = env_path("VKORE_TEST_ISO") else {
        return;
    };
    let medium = BootMedium::inspect(&iso).unwrap();
    assert!(medium.paging.is_some(), "{medium:?}");
    assert_eq!(medium.sha256.len(), 64);
}

/// A full production boot under TCG takes minutes; this one waits for the
/// shell and the seeded store, then stops the machine.
#[test]
fn a_real_kore_iso_boots_to_ready() {
    let (Some(qemu), Some(iso)) = (env_path("VKORE_TEST_QEMU"), env_path("VKORE_TEST_ISO")) else {
        return;
    };
    let firmware = firmware::discover().expect("an installed UEFI firmware descriptor");
    let dir = tempfile::tempdir().unwrap();
    let disk = dir.path().join("disk.raw");
    std::fs::File::create(&disk)
        .unwrap()
        .set_len(1 << 30)
        .unwrap();
    let stick = dir.path().join("usb.raw");
    std::fs::File::create(&stick)
        .unwrap()
        .set_len(16 << 20)
        .unwrap();
    let vars = dir.path().join("vars.fd");
    private_vars(&firmware.vars_template, &vars).unwrap();
    let medium = BootMedium::inspect(&iso).unwrap();
    let accel = HostCaps::probe()
        .choose(vkore_vm::AccelChoice::Auto, Vendor::Intel)
        .unwrap();
    let caps = QemuCaps::probe(&qemu, Vendor::Intel, accel).unwrap();
    let spec = MachineSpec {
        name: "test".into(),
        iso,
        disk,
        usb_stick: stick,
        firmware_code: firmware.code.clone(),
        firmware_vars: vars,
        firmware_format: firmware.format.clone(),
        vendor: Vendor::Intel,
        accel,
        paging: medium.paging.unwrap_or(Paging::FourLevel),
        cpus: 4,
        memory_mib: 4096,
        display: Display::Headless,
        framebuffer: (1280, 800),
        serial_log: dir.path().join("serial.log"),
        debugcon_log: dir.path().join("debugcon.log"),
        qmp_socket: dir.path().join("qmp.sock"),
    };
    let mut machine = Machine::start(&qemu, &caps, &spec, &dir.path().join("qemu.log")).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    while machine.readiness().stage() < Stage::Ready && std::time::Instant::now() < deadline {
        machine.poll(std::time::Duration::from_millis(500)).unwrap();
        assert!(machine.running(), "QEMU exited: {:?}", machine.readiness());
    }
    let readiness = machine.readiness().clone();
    assert_eq!(readiness.stage(), Stage::Ready, "{readiness:?}");
    assert_eq!(readiness.version.as_deref(), Some("0.40.0"));
    let shot = dir.path().join("screen.ppm");
    machine.screendump(&shot).unwrap();
    assert!(std::fs::metadata(&shot).unwrap().len() > 1280 * 800 * 3);
    machine.stop(std::time::Duration::from_secs(5)).unwrap();
    assert!(!machine.running());
}
