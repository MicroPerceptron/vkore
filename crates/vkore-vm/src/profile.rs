//! The machine a Kore guest boots on: the topology Kore's production gate
//! boots (`Makefile`, `QEMU_X86_CMD` with the gate's framebuffer), composed
//! from a typed specification rather than a shell string.
//!
//! What the topology carries, and why Kore needs it:
//!
//! - **A remapping IOMMU** (Intel VT-d or AMD-Vi) with interrupt remapping:
//!   Kore refuses DMA without one and interrupts without remapping, and
//!   cross-checks the CPU vendor against the ACPI table, so the CPU model
//!   always names the IOMMU's vendor.
//! - **Two NUMA nodes with an HMAT** and a PCIe expander bridge: the second
//!   root complex the device broker serves, and the measured per-node
//!   bandwidth Kore's placement reads.
//! - **NVMe** for the user volume, **two xHCI** controllers (terminal 0's
//!   keyboard and mouse at the guest-visible ports Kore binds terminals to,
//!   a non-HID stick, and the network row's own controller), an **e1000e**,
//!   and a **32-bpp framebuffer** the compositor founds over.
//! - **UEFI only**: Limine boots from the ISO's EFI image; Secure Boot off.
//!
//! The guest's networks are QEMU user-mode and isolated (`restrict=on`):
//! Kore has no IP stack, and nothing it does needs a packet to leave the
//! host.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use crate::probe::{Accel, QemuCaps};

/// The IOMMU (and so the CPU vendor) a machine presents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    /// Intel VT-d, a `GenuineIntel` CPU.
    Intel,
    /// AMD-Vi, an `AuthenticAMD` CPU.
    Amd,
}

impl Vendor {
    /// The CPUID vendor string Kore checks against the remapping table.
    pub fn cpuid(self) -> &'static str {
        match self {
            Self::Intel => "GenuineIntel",
            Self::Amd => "AuthenticAMD",
        }
    }
}

/// The paging mode an ISO's Limine configuration names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Paging {
    /// `paging_mode: 4level` (Kore's `4k48` profile).
    FourLevel,
    /// `paging_mode: 5level` (`4k57`): the CPU must realize LA57.
    FiveLevel,
}

/// Where the guest's screen goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Display {
    /// No display backend; the framebuffer device is still present, as the
    /// compositor founds over it. QMP `screendump` still reads it.
    Headless,
    /// QEMU's own window (its default UI backend on this host).
    Window,
    /// A VNC server on a private Unix socket.
    Vnc(PathBuf),
}

/// Everything one machine needs.
#[derive(Clone, Debug)]
pub struct MachineSpec {
    /// The machine's name, shown by QEMU.
    pub name: String,
    pub iso: PathBuf,
    /// The machine's NVMe disk: a raw image holding its user volume.
    pub disk: PathBuf,
    /// A blank raw image for the USB stick Kore's input enumeration expects.
    pub usb_stick: PathBuf,
    pub firmware_code: PathBuf,
    /// The machine's own writable copy of the firmware's variable store.
    pub firmware_vars: PathBuf,
    /// `raw` or `qcow2`, as the firmware descriptor names it.
    pub firmware_format: String,
    pub vendor: Vendor,
    pub accel: Accel,
    pub paging: Paging,
    pub cpus: u16,
    pub memory_mib: u32,
    pub display: Display,
    /// The framebuffer mode the compositor founds over.
    pub framebuffer: (u32, u32),
    pub serial_log: PathBuf,
    pub debugcon_log: PathBuf,
    pub qmp_socket: PathBuf,
}

/// A specification QEMU cannot be given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// A path QEMU's option syntax cannot carry (not UTF-8, a comma, a
    /// control character) or a socket path longer than `sun_path`.
    Path(PathBuf, &'static str),
    /// Two NUMA nodes need at least two CPUs; Kore admits at most 128.
    Cpus(u16),
    /// Memory splits evenly over two nodes, at least 2 GiB in all.
    Memory(u32),
    /// A framebuffer mode outside what Kore's display path takes.
    Framebuffer(u32, u32),
    /// A machine name QEMU's `-name` cannot carry.
    Name(String),
    /// The probed QEMU cannot realize this topology.
    Unsupported(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(path, why) => write!(f, "{}: {why}", path.display()),
            Self::Cpus(cpus) => write!(
                f,
                "{cpus} CPUs: a Kore machine has two NUMA nodes, so 2 to 128 CPUs"
            ),
            Self::Memory(mib) => write!(
                f,
                "{mib} MiB: at least 2048 MiB, an even number split over two nodes"
            ),
            Self::Framebuffer(width, height) => write!(
                f,
                "a {width}x{height} framebuffer: 640x480 to 7680x4320, an even width"
            ),
            Self::Name(name) => write!(
                f,
                "{name:?}: a machine name is 1 to 64 letters, digits, '-' or '_'"
            ),
            Self::Unsupported(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ProfileError {}

const MIN_MEMORY_MIB: u32 = 2048;
const MAX_MEMORY_MIB: u32 = 1 << 20;
const MAX_CPUS: u16 = 128;
const SUN_PATH_MAX: usize = 107;

/// Check `path` for QEMU's option syntax.
fn option_path(path: &Path) -> Result<&str, ProfileError> {
    let text = path
        .to_str()
        .ok_or(ProfileError::Path(path.to_path_buf(), "not UTF-8"))?;
    if text.is_empty() || text.contains(',') || text.chars().any(char::is_control) {
        return Err(ProfileError::Path(
            path.to_path_buf(),
            "QEMU's option syntax cannot carry this path (a comma or a control character)",
        ));
    }
    Ok(text)
}

/// The NUMA split Kore's gate topology uses: node 0 takes two thirds of
/// the CPUs (at least one), node 1 the rest; memory splits evenly; HMAT
/// latencies and bandwidths as the Makefile states them.
fn numa(cpus: u16, memory_mib: u32) -> Vec<String> {
    let nodes = 2u16;
    let per_node = memory_mib / u32::from(nodes);
    let mut arguments = Vec::new();
    let mut first = 0u16;
    for node in 0..nodes {
        let remaining = nodes - node;
        let left = cpus - first;
        let count = if node == 0 && cpus >= 3 {
            (cpus * 2).div_ceil(3)
        } else {
            left.div_ceil(remaining)
        }
        .max(1);
        let last = first + count - 1;
        arguments.push("-numa".into());
        arguments.push(format!(
            "node,nodeid={node},cpus={first}-{last},memdev=ram{node}"
        ));
        arguments.push("-object".into());
        arguments.push(format!("memory-backend-ram,id=ram{node},size={per_node}M"));
        first = last + 1;
    }
    for source in 0..nodes {
        for destination in 0..nodes {
            if source != destination {
                arguments.push("-numa".into());
                arguments.push(format!("dist,src={source},dst={destination},val=21"));
            }
        }
    }
    for source in 0..nodes {
        for target in 0..nodes {
            let (latency, bandwidth) = match (source == target, target) {
                (true, 0) => (10, 200),
                (true, _) => (10, 100),
                (false, _) => (20, 50),
            };
            arguments.push("-numa".into());
            arguments.push(format!(
                "hmat-lb,initiator={source},target={target},hierarchy=memory,data-type=access-latency,latency={latency}"
            ));
            arguments.push("-numa".into());
            arguments.push(format!(
                "hmat-lb,initiator={source},target={target},hierarchy=memory,data-type=access-bandwidth,bandwidth={bandwidth}G"
            ));
        }
    }
    arguments
}

/// The smallest power-of-two VRAM, at least 16 MiB, that holds the mode at
/// 32 bits per pixel (the gate's `vgamem`).
fn vgamem_mib(width: u32, height: u32) -> u64 {
    let needed = u64::from(width) * u64::from(height) * 4;
    let mut mib = 16u64;
    while mib << 20 < needed {
        mib *= 2;
    }
    mib
}

impl MachineSpec {
    /// QEMU's arguments for this machine on the probed `qemu`, excluding
    /// the executable itself.
    pub fn arguments(&self, qemu: &QemuCaps) -> Result<Vec<String>, ProfileError> {
        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ProfileError::Name(self.name.clone()));
        }
        if !(2..=MAX_CPUS).contains(&self.cpus) {
            return Err(ProfileError::Cpus(self.cpus));
        }
        if !(MIN_MEMORY_MIB..=MAX_MEMORY_MIB).contains(&self.memory_mib)
            || !self.memory_mib.is_multiple_of(2)
        {
            return Err(ProfileError::Memory(self.memory_mib));
        }
        let (width, height) = self.framebuffer;
        if !(640..=7680).contains(&width) || !(480..=4320).contains(&height) || width % 2 != 0 {
            return Err(ProfileError::Framebuffer(width, height));
        }
        if !matches!(self.firmware_format.as_str(), "raw" | "qcow2") {
            return Err(ProfileError::Unsupported(format!(
                "firmware in {} format: raw or qcow2",
                self.firmware_format
            )));
        }
        let iso = option_path(&self.iso)?;
        let disk = option_path(&self.disk)?;
        let stick = option_path(&self.usb_stick)?;
        let code = option_path(&self.firmware_code)?;
        let vars = option_path(&self.firmware_vars)?;
        let serial = option_path(&self.serial_log)?;
        let debugcon = option_path(&self.debugcon_log)?;
        let qmp = option_path(&self.qmp_socket)?;
        if qmp.len() > SUN_PATH_MAX {
            return Err(ProfileError::Path(
                self.qmp_socket.clone(),
                "longer than a Unix socket path can be",
            ));
        }

        let paging_cpu = match self.paging {
            Paging::FourLevel => "",
            Paging::FiveLevel => ",+la57,enforce",
        };
        if self.paging == Paging::FiveLevel && !qemu.la57 {
            return Err(ProfileError::Unsupported(
                "the ISO boots five-level paging, and this QEMU cannot realize LA57 for this \
                 accelerator"
                    .into(),
            ));
        }
        let (machine, accel, cpu) = match self.accel {
            Accel::Kvm => (
                "q35,kernel_irqchip=split,hmat=on",
                "kvm".to_owned(),
                format!("host{paging_cpu}"),
            ),
            Accel::Hvf => ("q35,hmat=on", "hvf".to_owned(), format!("host{paging_cpu}")),
            Accel::Tcg => (
                "q35,hmat=on",
                "tcg,thread=multi".to_owned(),
                format!(
                    "{}{paging_cpu}",
                    match self.vendor {
                        // TCG's `max` reports AuthenticAMD; Kore cross-checks
                        // the vendor against the remapping table.
                        Vendor::Intel => "max,+x2apic,+arat,vendor=GenuineIntel",
                        Vendor::Amd => {
                            "EPYC-v4,+x2apic,+arat,-fxsr-opt,-misalignsse,-osvw,-topoext,\
                             -perfctr-core,-clzero,-ibpb,-nrip-save,-xsavec,-xsaves"
                        }
                    }
                ),
            ),
        };
        let iommu = match self.vendor {
            Vendor::Intel => format!(
                "intel-iommu,intremap=on,caching-mode=on,device-iotlb=on,{}=on,{}snoop-control=on,dma-translation=on,aw-bits=48",
                qemu.vtd_scalable_property,
                if qemu.vtd_sva {
                    "x-pasid-mode=on,x-flts=on,"
                } else {
                    ""
                }
            ),
            Vendor::Amd if qemu.amd_dma_remap => {
                "amd-iommu,intremap=on,device-iotlb=on,xtsup=on,dma-remap=on".to_owned()
            }
            Vendor::Amd => {
                return Err(ProfileError::Unsupported(
                    "this QEMU cannot realize AMD-Vi DMA remapping (QEMU 10.1 or later)".into(),
                ));
            }
        };

        let mut arguments: Vec<String> = vec![
            "-name".into(),
            format!("kore-{}", self.name),
            "-machine".into(),
            machine.into(),
            "-accel".into(),
            accel,
            "-cpu".into(),
            cpu,
            "-m".into(),
            self.memory_mib.to_string(),
            "-smp".into(),
            format!("{0},cores={0},threads=1,sockets=1", self.cpus),
        ];
        arguments.extend(numa(self.cpus, self.memory_mib));
        arguments.extend(
            [
                "-no-user-config",
                "-nodefaults",
                "-no-reboot",
                "-rtc",
                "base=utc,clock=host",
                "-boot",
                "menu=off,strict=on",
                "-monitor",
                "none",
            ]
            .map(String::from),
        );
        match &self.display {
            Display::Headless => arguments.extend(["-display".into(), "none".into()]),
            // QEMU's default UI backend on this host.
            Display::Window => {}
            Display::Vnc(socket) => {
                let socket = option_path(socket)?;
                if socket.len() > SUN_PATH_MAX {
                    return Err(ProfileError::Path(
                        PathBuf::from(socket),
                        "longer than a Unix socket path can be",
                    ));
                }
                arguments.extend(["-display".into(), format!("vnc=unix:{socket}")]);
            }
        }
        arguments.extend([
            "-debugcon".into(),
            format!("file:{debugcon}"),
            "-global".into(),
            "isa-debugcon.iobase=0xe9".into(),
            "-device".into(),
            "pxb-pcie,id=pxb1,numa_node=1,bus_nr=128,bus=pcie.0".into(),
            "-device".into(),
            "pcie-root-port,id=rp0,bus=pcie.0,chassis=1,slot=1".into(),
            "-device".into(),
            "pcie-root-port,id=rp1,bus=pxb1,chassis=4,slot=1".into(),
            "-device".into(),
            "pcie-root-port,id=rp2,bus=pxb1,chassis=5,slot=2".into(),
            "-device".into(),
            "pcie-root-port,id=rp3,bus=pxb1,chassis=6,slot=3".into(),
            "-device".into(),
            iommu,
            "-netdev".into(),
            "user,id=net0,ipv6=off,restrict=on".into(),
            "-device".into(),
            "e1000e,id=nic0,netdev=net0,bus=rp0".into(),
            "-drive".into(),
            format!("file={disk},if=none,id=nvme0,format=raw,cache=none,discard=unmap"),
            "-device".into(),
            format!(
                "nvme,id=nvme_dev0,serial=nvme0,drive=nvme0,bus=rp1,max_ioqpairs={},msix_qsize=65",
                self.cpus
            ),
            // `p3=0`: no SuperSpeed bank, so the command line's port numbers
            // are the guest-visible ones Kore binds terminals to.
            "-device".into(),
            "qemu-xhci,id=xhci,bus=rp2,p3=0".into(),
            "-device".into(),
            "qemu-xhci,id=xhci_net,bus=rp3,p3=0".into(),
            "-netdev".into(),
            "user,id=netusb0,ipv6=off,restrict=on".into(),
            "-device".into(),
            "usb-net,bus=xhci_net.0,port=1,netdev=netusb0".into(),
            "-device".into(),
            "usb-kbd,id=primary-kbd,bus=xhci.0,port=1".into(),
            "-device".into(),
            "usb-mouse,id=primary-mouse,bus=xhci.0,port=3".into(),
            "-drive".into(),
            format!("if=none,id=usbstick,file={stick},format=raw"),
            "-device".into(),
            "usb-storage,bus=xhci.0,drive=usbstick,removable=on".into(),
        ]);
        if qemu.generic_initiators {
            for (id, device, node) in [
                ("gi_nic0", "nic0", 0),
                ("gi_nvme0", "nvme_dev0", 1),
                ("gi_xhci", "xhci", 1),
            ] {
                arguments.push("-object".into());
                arguments.push(format!(
                    "acpi-generic-initiator,id={id},pci-dev={device},node={node}"
                ));
            }
        }
        arguments.extend([
            "-device".into(),
            "pcie-root-port,id=rp_fb,bus=pxb1,chassis=8,slot=4".into(),
            "-device".into(),
            format!(
                "bochs-display,bus=rp_fb,xres={width},yres={height},vgamem={}M",
                vgamem_mib(width, height)
            ),
            "-drive".into(),
            format!(
                "if=pflash,format={},readonly=on,file={code}",
                self.firmware_format
            ),
            "-drive".into(),
            format!("if=pflash,format={},file={vars}", self.firmware_format),
            "-serial".into(),
            format!("file:{serial}"),
            "-qmp".into(),
            format!("unix:{qmp},server=on,wait=off"),
            "-cdrom".into(),
            iso.into(),
        ]);
        Ok(arguments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> QemuCaps {
        QemuCaps {
            version: (8, 2, 2),
            vtd_scalable_property: "x-scalable-mode",
            vtd_sva: false,
            generic_initiators: false,
            amd_dma_remap: false,
            la57: true,
        }
    }

    fn spec() -> MachineSpec {
        MachineSpec {
            name: "demo".into(),
            iso: "/m/kore.iso".into(),
            disk: "/m/disk.raw".into(),
            usb_stick: "/m/usb.raw".into(),
            firmware_code: "/usr/share/OVMF/OVMF_CODE_4M.fd".into(),
            firmware_vars: "/m/vars.fd".into(),
            firmware_format: "raw".into(),
            vendor: Vendor::Intel,
            accel: Accel::Tcg,
            paging: Paging::FourLevel,
            cpus: 4,
            memory_mib: 4096,
            display: Display::Headless,
            framebuffer: (1280, 800),
            serial_log: "/m/serial.log".into(),
            debugcon_log: "/m/debugcon.log".into(),
            qmp_socket: "/m/qmp.sock".into(),
        }
    }

    fn joined(arguments: &[String]) -> String {
        arguments.join(" ")
    }

    #[test]
    fn the_gate_topology_on_qemu_8_under_tcg() {
        let arguments = spec().arguments(&caps()).unwrap();
        let line = joined(&arguments);
        // What `make -n exec-boot-gate-x86 PROFILE=intel ACCEL=tcg CPUS=4
        // MEM=4096` composes on a QEMU 8 host, item for item.
        for expected in [
            "-machine q35,hmat=on -accel tcg,thread=multi -cpu max,+x2apic,+arat,vendor=GenuineIntel -m 4096 -smp 4,cores=4,threads=1,sockets=1",
            "-numa node,nodeid=0,cpus=0-2,memdev=ram0 -object memory-backend-ram,id=ram0,size=2048M -numa node,nodeid=1,cpus=3-3,memdev=ram1 -object memory-backend-ram,id=ram1,size=2048M",
            "-numa dist,src=0,dst=1,val=21 -numa dist,src=1,dst=0,val=21",
            "-numa hmat-lb,initiator=0,target=0,hierarchy=memory,data-type=access-bandwidth,bandwidth=200G",
            "-numa hmat-lb,initiator=1,target=1,hierarchy=memory,data-type=access-bandwidth,bandwidth=100G",
            "-no-user-config -nodefaults -no-reboot -rtc base=utc,clock=host -boot menu=off,strict=on -monitor none -display none",
            "-device intel-iommu,intremap=on,caching-mode=on,device-iotlb=on,x-scalable-mode=on,snoop-control=on,dma-translation=on,aw-bits=48",
            "-drive file=/m/disk.raw,if=none,id=nvme0,format=raw,cache=none,discard=unmap -device nvme,id=nvme_dev0,serial=nvme0,drive=nvme0,bus=rp1,max_ioqpairs=4,msix_qsize=65",
            "-device usb-kbd,id=primary-kbd,bus=xhci.0,port=1 -device usb-mouse,id=primary-mouse,bus=xhci.0,port=3",
            "-device bochs-display,bus=rp_fb,xres=1280,yres=800,vgamem=16M",
            "-drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd -drive if=pflash,format=raw,file=/m/vars.fd",
            "-serial file:/m/serial.log -qmp unix:/m/qmp.sock,server=on,wait=off -cdrom /m/kore.iso",
        ] {
            assert!(line.contains(expected), "missing {expected:?} in {line}");
        }
        assert!(!line.contains("acpi-generic-initiator"));
        assert!(line.contains("user,id=net0,ipv6=off,restrict=on"));
    }

    #[test]
    fn newer_qemu_kvm_amd_and_five_level_paging() {
        let mut caps = caps();
        caps.version = (10, 1, 0);
        caps.vtd_scalable_property = "scalable-mode";
        caps.vtd_sva = true;
        caps.generic_initiators = true;
        caps.amd_dma_remap = true;
        let mut spec = spec();
        spec.accel = Accel::Kvm;
        spec.paging = Paging::FiveLevel;
        let line = joined(&spec.arguments(&caps).unwrap());
        assert!(line.contains(
            "-machine q35,kernel_irqchip=split,hmat=on -accel kvm -cpu host,+la57,enforce"
        ));
        assert!(line.contains("scalable-mode=on,x-pasid-mode=on,x-flts=on,snoop-control=on"));
        assert!(
            line.contains("-object acpi-generic-initiator,id=gi_nvme0,pci-dev=nvme_dev0,node=1")
        );
        spec.vendor = Vendor::Amd;
        spec.accel = Accel::Tcg;
        let line = joined(&spec.arguments(&caps).unwrap());
        assert!(line.contains("-cpu EPYC-v4,+x2apic,+arat,-fxsr-opt"));
        assert!(line.contains(",-xsaves,+la57,enforce"));
        assert!(
            line.contains("-device amd-iommu,intremap=on,device-iotlb=on,xtsup=on,dma-remap=on")
        );
    }

    #[test]
    fn what_qemu_or_kore_cannot_take_is_refused() {
        let mut spec = spec();
        spec.cpus = 1;
        assert_eq!(spec.arguments(&caps()), Err(ProfileError::Cpus(1)));
        spec.cpus = 2;
        spec.memory_mib = 3001;
        assert_eq!(spec.arguments(&caps()), Err(ProfileError::Memory(3001)));
        spec.memory_mib = 2048;
        spec.iso = "/m/a,b.iso".into();
        assert!(matches!(
            spec.arguments(&caps()),
            Err(ProfileError::Path(..))
        ));
        spec.iso = "/m/kore.iso".into();
        spec.qmp_socket = PathBuf::from(format!("/{}/q.sock", "x".repeat(110)));
        assert!(matches!(
            spec.arguments(&caps()),
            Err(ProfileError::Path(..))
        ));
        let mut spec = self::spec();
        spec.vendor = Vendor::Amd;
        assert!(matches!(
            spec.arguments(&caps()),
            Err(ProfileError::Unsupported(_))
        ));
        let mut spec = self::spec();
        spec.paging = Paging::FiveLevel;
        let mut no_la57 = caps();
        no_la57.la57 = false;
        assert!(matches!(
            spec.arguments(&no_la57),
            Err(ProfileError::Unsupported(_))
        ));
        spec.name = "a b".into();
        assert!(matches!(
            spec.arguments(&caps()),
            Err(ProfileError::Name(_))
        ));
    }

    #[test]
    fn the_numa_split_and_vram_match_the_gate() {
        let two = joined(&numa(2, 2048));
        assert!(two.contains("cpus=0-0") && two.contains("cpus=1-1") && two.contains("size=1024M"));
        let eight = joined(&numa(8, 8192));
        assert!(eight.contains("cpus=0-5") && eight.contains("cpus=6-7"));
        assert_eq!(vgamem_mib(1280, 800), 16);
        assert_eq!(vgamem_mib(1920, 1200), 16);
        assert_eq!(vgamem_mib(2560, 1600), 16);
        assert_eq!(vgamem_mib(3840, 2400), 64);
    }
}
