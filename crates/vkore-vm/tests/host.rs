//! Against this host's real QEMU and a real Kore ISO. Opt in with
//! `VKORE_TEST_QEMU` (the `qemu-system-x86_64` or `qemu-system-aarch64`
//! for the ISO) and `VKORE_TEST_ISO` (a Kore ISO of either architecture,
//! read from the medium); without them each test says what it skipped.

use std::path::PathBuf;

use vkore_vm::{
    Accel, BootMedium, Display, GuestArch, HostCaps, Machine, MachineSpec, Paging, Platform,
    QemuCaps, Stage, Vendor, firmware, machine::private_vars,
};

/// The machine a medium of `arch` boots, with Intel's IOMMU on x86.
fn platform(arch: GuestArch, vendor: Vendor) -> Platform {
    match arch {
        GuestArch::X86_64 => Platform::X86_64(vendor),
        GuestArch::Aarch64 => Platform::Aarch64,
    }
}

/// The framebuffer the platform's display presents: the profile's mode on
/// x86, the firmware's `ramfb` mode on aarch64.
fn framebuffer(arch: GuestArch) -> (u32, u32) {
    match arch {
        GuestArch::X86_64 => (1280, 800),
        GuestArch::Aarch64 => (800, 600),
    }
}

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
    // The emulator is for exactly one of the architectures: the other's
    // probe refuses it in words.
    let probed: Vec<_> = GuestArch::ALL
        .into_iter()
        .filter_map(|arch| {
            QemuCaps::probe(&qemu, platform(arch, Vendor::Intel), Accel::Tcg)
                .ok()
                .map(|caps| (arch, caps))
        })
        .collect();
    let [(arch, caps)] = probed.as_slice() else {
        panic!(
            "{} is an emulator for {} architectures",
            qemu.display(),
            probed.len()
        );
    };
    assert!(caps.version.0 >= 8, "{caps:?}");
    match arch {
        GuestArch::X86_64 => {
            assert_eq!(caps.generic_initiators, caps.version.0 >= 9);
            assert!(matches!(
                caps.vtd_scalable_property,
                "scalable-mode" | "x-scalable-mode"
            ));
        }
        GuestArch::Aarch64 => assert!(caps.aarch64_virt, "{caps:?}"),
    }
    let host = HostCaps::probe();
    assert_eq!(
        host.choose(vkore_vm::AccelChoice::Tcg, platform(*arch, Vendor::Intel))
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
    assert!(medium.arch.is_some(), "{medium:?}");
    assert_eq!(medium.sha256.len(), 64);
}

/// A full production boot under TCG takes minutes; this one waits for the
/// shell and the seeded store, then stops the machine.
#[test]
fn a_real_kore_iso_boots_to_ready() {
    boot_to_ready(vkore_vm::hardware::GuestNuma::Split, false);
}
#[test]
fn a_real_kore_iso_boots_with_one_memory_node() {
    boot_to_ready(vkore_vm::hardware::GuestNuma::Single, false);
}
/// Opt in separately: this changes host idle behavior and requires native KVM.
#[test]
fn a_real_kore_iso_boots_with_native_cpu_waits() {
    if std::env::var("VKORE_TEST_CPU_PM").as_deref() != Ok("1") {
        eprintln!("skipped: set VKORE_TEST_CPU_PM=1 on a supported Linux KVM host");
        return;
    }
    vkore_vm::cpu_pm::available()
        .expect("native CPU waits must be supported when explicitly requested");
    boot_to_ready(vkore_vm::hardware::GuestNuma::Single, true);
}
fn boot_to_ready(topology: vkore_vm::hardware::GuestNuma, cpu_pm: bool) {
    let (Some(qemu), Some(iso)) = (env_path("VKORE_TEST_QEMU"), env_path("VKORE_TEST_ISO")) else {
        return;
    };
    let arch = BootMedium::inspect(&iso)
        .unwrap()
        .arch
        .expect("a UEFI Kore ISO");
    if arch == GuestArch::Aarch64 && topology != vkore_vm::hardware::GuestNuma::Single {
        eprintln!("skipped: an aarch64 machine is one NUMA node");
        return;
    }
    let (dir, mut machine) = start_machine(&qemu, iso, topology, cpu_pm);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    while machine.readiness().stage() < Stage::Ready && std::time::Instant::now() < deadline {
        machine.poll(std::time::Duration::from_millis(500)).unwrap();
        if !machine.running() {
            let console = machine.console().unwrap_or_default();
            let console = String::from_utf8_lossy(&console);
            let tail = console
                .lines()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            panic!(
                "QEMU exited: {:?}\nLast console lines:\n{tail}",
                machine.readiness()
            );
        }
    }
    let readiness = machine.readiness().clone();
    assert_eq!(readiness.stage(), Stage::Ready, "{readiness:?}");
    assert!(
        readiness
            .version
            .as_deref()
            .is_some_and(|version| !version.is_empty())
    );
    let shot = dir.path().join("screen.ppm");
    machine.screendump(&shot).unwrap();
    let (width, height) = framebuffer(arch);
    assert!(std::fs::metadata(&shot).unwrap().len() > u64::from(width * height * 3));
    machine.stop(std::time::Duration::from_secs(5)).unwrap();
    assert!(!machine.running());
}

fn start_machine(
    qemu: &std::path::Path,
    iso: PathBuf,
    topology: vkore_vm::hardware::GuestNuma,
    cpu_pm: bool,
) -> (tempfile::TempDir, Machine) {
    let medium = BootMedium::inspect(&iso).unwrap();
    let arch = medium.arch.expect("a UEFI Kore ISO");
    let firmware = vkore_vm::runtime::Runtime::for_qemu(qemu, arch)
        .expect("valid QEMU runtime")
        .map(|runtime| runtime.firmware)
        .or_else(|| firmware::discover(arch))
        .expect("bundled firmware or an installed UEFI firmware descriptor");
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
    let host = HostCaps::probe();
    let vendor = if cpu_pm {
        host.vendor.expect("host vendor")
    } else {
        Vendor::Intel
    };
    let accel = host
        .choose(
            if cpu_pm {
                vkore_vm::AccelChoice::Kvm
            } else {
                vkore_vm::AccelChoice::Auto
            },
            platform(arch, vendor),
        )
        .unwrap();
    let caps = QemuCaps::probe(qemu, platform(arch, vendor), accel).unwrap();
    let spec = MachineSpec {
        name: "test".into(),
        iso,
        disk,
        usb_stick: stick,
        firmware_code: firmware.code.clone(),
        firmware_vars: vars,
        firmware_format: firmware.format.clone(),
        platform: platform(arch, vendor),
        accel,
        paging: medium.paging.unwrap_or(Paging::FourLevel),
        cpus: 2,
        memory_mib: 2048,
        resources: vkore_vm::hardware::Resources {
            guest_numa: topology,
            cpu_power_management: cpu_pm,
            pin_cpus: cpu_pm,
            host_cpus: if cpu_pm {
                vkore_vm::hardware::HostResources::probe(dir.path())
                    .unwrap()
                    .preferred_cpus(2)
            } else {
                Vec::new()
            },
            ..Default::default()
        },
        display: Display::Headless,
        framebuffer: framebuffer(arch),
        serial_log: dir.path().join("serial.log"),
        debugcon_log: dir.path().join("debugcon.log"),
        qmp_socket: dir.path().join("qmp.sock"),
        console_socket: medium.console.then(|| dir.path().join("console.sock")),
    };
    let machine = Machine::start(qemu, &caps, &spec, &dir.path().join("qemu.log")).unwrap();
    (dir, machine)
}

/// Kernel self-test images intentionally omit the production runtime and shut
/// down after PASS. Keep their acceptance separate from shell/volume readiness.
#[test]
fn a_kernel_self_test_iso_passes_and_powers_off() {
    let (Some(qemu), Some(iso)) = (env_path("VKORE_TEST_QEMU"), env_path("VKORE_TEST_BOOT_ISO"))
    else {
        return;
    };
    let topology = match BootMedium::inspect(&iso).unwrap().arch {
        Some(GuestArch::Aarch64) => vkore_vm::hardware::GuestNuma::Single,
        _ => vkore_vm::hardware::GuestNuma::Split,
    };
    let (dir, mut machine) = start_machine(&qemu, iso, topology, false);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
    let mut captured = false;
    while std::time::Instant::now() < deadline {
        machine.poll(std::time::Duration::from_millis(200)).unwrap();
        if machine.readiness().display.is_some() && !captured && machine.running() {
            let shot = dir.path().join("boot.ppm");
            machine.screendump(&shot).unwrap();
            assert!(std::fs::metadata(shot).unwrap().len() > 1280 * 800 * 3);
            captured = true;
        }
        if !machine.running() {
            break;
        }
    }
    assert!(!machine.running(), "kernel self-test did not power off");
    let bytes = machine.console().unwrap();
    let console = String::from_utf8_lossy(&bytes);
    assert!(
        console.contains("boot_test: PASS"),
        "kernel self-test did not pass"
    );
    assert!(
        console.contains("power: S5"),
        "guest did not request power off"
    );
    assert!(captured, "no guest framebuffer was captured through QMP");
}
