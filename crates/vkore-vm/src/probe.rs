//! What this host and its QEMU can realize for a Kore machine.
//!
//! Version numbers lie about device properties (distributions backport and
//! strip), so a property is probed the way Kore's Makefile probes it: QEMU
//! is asked to realize the candidate device ahead of a deliberately invalid
//! sentinel device, and the candidate is realizable exactly when QEMU gets
//! as far as refusing the sentinel. Only the acpi-generic-initiator floor
//! is a version check, because QEMU 8 *lists* that object type and then
//! refuses to create it.

use std::{
    fmt, fs,
    path::Path,
    process::{Command, Stdio},
};

use crate::profile::Vendor;

const SENTINEL: &str = "vkore-probe-sentinel";

/// How a machine's CPUs execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Accel {
    /// Linux KVM: `-cpu host`, only when the host's vendor is the IOMMU's.
    Kvm,
    /// macOS Hypervisor.framework, on an Intel Mac.
    Hvf,
    /// QEMU's portable translator: a CPU model of the IOMMU's vendor.
    Tcg,
}

/// What the caller asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccelChoice {
    /// The fastest accelerator that keeps Kore's vendor check coherent.
    #[default]
    Auto,
    Kvm,
    Hvf,
    Tcg,
}

/// What the probed QEMU realizes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct QemuCaps {
    pub version: (u32, u32, u32),
    /// `scalable-mode` (QEMU 11 and later) or `x-scalable-mode`.
    pub vtd_scalable_property: &'static str,
    /// VT-d PASID and first-level translation (`x-pasid-mode`, `x-flts`).
    pub vtd_sva: bool,
    /// `acpi-generic-initiator` objects (QEMU 9 and later).
    pub generic_initiators: bool,
    /// AMD-Vi DMA remapping (`dma-remap`, QEMU 10.1 and later).
    pub amd_dma_remap: bool,
    /// Five-level paging on the chosen accelerator and CPU.
    pub la57: bool,
}

/// What this host offers.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HostCaps {
    /// The CPU vendor, when it is Intel or AMD.
    pub vendor: Option<Vendor>,
    /// `/dev/kvm` opens for reading and writing.
    pub kvm: bool,
    /// macOS on an Intel CPU.
    pub hvf: bool,
}

/// What a probe could not establish.
#[derive(Debug)]
pub enum ProbeError {
    /// The QEMU executable did not run.
    Qemu(String),
    /// A choice this host cannot honour.
    Accel(String),
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Qemu(message) | Self::Accel(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Run `qemu` with `arguments` and answer its combined output, or why it
/// did not run.
fn run(qemu: &Path, arguments: &[&str]) -> Result<String, ProbeError> {
    let output = Command::new(qemu)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| ProbeError::Qemu(format!("{}: {error}", qemu.display())))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

/// Whether QEMU realizes everything in `arguments` ahead of the sentinel.
fn realizes(qemu: &Path, arguments: &[&str]) -> Result<bool, ProbeError> {
    let mut all = vec!["-display", "none", "-no-user-config", "-nodefaults", "-S"];
    all.extend_from_slice(arguments);
    all.extend_from_slice(&["-device", SENTINEL]);
    Ok(run(qemu, &all)?.contains(SENTINEL))
}

/// `QEMU emulator version 8.2.2 (Debian ...)` → `(8, 2, 2)`.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let rest = text.split("version ").nth(1)?;
    let mut numbers = rest
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u32>().ok());
    Some((
        numbers.next()??,
        numbers.next()??,
        numbers.next().flatten().unwrap_or(0),
    ))
}

impl QemuCaps {
    /// Probe `qemu` for a machine of `vendor` running on `accel`.
    pub fn probe(qemu: &Path, vendor: Vendor, accel: Accel) -> Result<Self, ProbeError> {
        let version = parse_version(&run(qemu, &["--version"])?).ok_or_else(|| {
            ProbeError::Qemu(format!(
                "{}: not a QEMU this probe understands",
                qemu.display()
            ))
        })?;
        let help = run(qemu, &["-device", "intel-iommu,help"])?;
        let vtd_scalable_property = if help
            .lines()
            .any(|line| line.trim_start().starts_with("scalable-mode="))
        {
            "scalable-mode"
        } else {
            "x-scalable-mode"
        };
        let vtd_sva = vendor == Vendor::Intel
            && realizes(
                qemu,
                &[
                    "-machine",
                    "q35",
                    "-device",
                    &format!(
                        "intel-iommu,intremap=on,caching-mode=on,device-iotlb=on,{vtd_scalable_property}=on,x-pasid-mode=on,x-flts=on,snoop-control=on,dma-translation=on,aw-bits=48"
                    ),
                ],
            )?;
        let amd_dma_remap = vendor == Vendor::Amd
            && realizes(
                qemu,
                &[
                    "-machine",
                    "q35",
                    "-device",
                    "amd-iommu,intremap=on,device-iotlb=on,xtsup=on,dma-remap=on",
                ],
            )?;
        let la57_cpu = match (accel, vendor) {
            (Accel::Kvm | Accel::Hvf, _) => "host,+la57,enforce",
            (Accel::Tcg, Vendor::Intel) => "max,+x2apic,+arat,vendor=GenuineIntel,+la57,enforce",
            (Accel::Tcg, Vendor::Amd) => "EPYC-v4,+la57,enforce",
        };
        let accel_argument = match accel {
            Accel::Kvm => "kvm",
            Accel::Hvf => "hvf",
            Accel::Tcg => "tcg",
        };
        let la57 = realizes(
            qemu,
            &[
                "-machine",
                "q35",
                "-accel",
                accel_argument,
                "-cpu",
                la57_cpu,
                "-m",
                "128",
            ],
        )?;
        Ok(Self {
            version,
            vtd_scalable_property,
            vtd_sva,
            generic_initiators: version.0 >= 9,
            amd_dma_remap,
            la57,
        })
    }
}

impl HostCaps {
    /// What this host offers, read without privileges.
    pub fn probe() -> Self {
        let vendor = cpu_vendor();
        let kvm = cfg!(target_os = "linux")
            && fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/kvm")
                .is_ok();
        let hvf = cfg!(target_os = "macos") && cfg!(target_arch = "x86_64");
        Self { vendor, kvm, hvf }
    }

    /// The accelerator for a machine of `vendor`: hardware only when the
    /// host's CPU vendor is the IOMMU's (`-cpu host` copies it, and Kore
    /// checks it against the remapping table), else the translator.
    pub fn choose(&self, choice: AccelChoice, vendor: Vendor) -> Result<Accel, ProbeError> {
        let coherent = self.vendor == Some(vendor);
        match choice {
            AccelChoice::Auto if self.kvm && coherent => Ok(Accel::Kvm),
            AccelChoice::Auto if self.hvf && coherent => Ok(Accel::Hvf),
            AccelChoice::Auto | AccelChoice::Tcg => Ok(Accel::Tcg),
            AccelChoice::Kvm if !self.kvm => Err(ProbeError::Accel(
                "KVM was asked for, and /dev/kvm does not open for reading and writing".into(),
            )),
            AccelChoice::Hvf if !self.hvf => Err(ProbeError::Accel(
                "HVF was asked for, and this is not macOS on an Intel CPU".into(),
            )),
            AccelChoice::Kvm | AccelChoice::Hvf if !coherent => Err(ProbeError::Accel(format!(
                "hardware acceleration runs the host's own CPU, and Kore checks the CPU \
                 vendor against the {} IOMMU: use TCG, or the {} profile on this host",
                vendor.cpuid(),
                match vendor {
                    Vendor::Intel => "amd",
                    Vendor::Amd => "intel",
                }
            ))),
            AccelChoice::Kvm => Ok(Accel::Kvm),
            AccelChoice::Hvf => Ok(Accel::Hvf),
        }
    }

    /// The IOMMU profile that lets this host accelerate: its own vendor's,
    /// Intel otherwise.
    pub fn native_vendor(&self) -> Vendor {
        self.vendor.unwrap_or(Vendor::Intel)
    }
}

fn cpu_vendor() -> Option<Vendor> {
    #[cfg(target_arch = "x86_64")]
    {
        // CPUID leaf 0: the vendor string in EBX, EDX, ECX.
        let leaf = std::arch::x86_64::__cpuid(0);
        let mut bytes = [0u8; 12];
        bytes[..4].copy_from_slice(&leaf.ebx.to_le_bytes());
        bytes[4..8].copy_from_slice(&leaf.edx.to_le_bytes());
        bytes[8..].copy_from_slice(&leaf.ecx.to_le_bytes());
        match &bytes {
            b"GenuineIntel" => Some(Vendor::Intel),
            b"AuthenticAMD" => Some(Vendor::Amd),
            _ => None,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_as_qemu_prints_them() {
        assert_eq!(
            parse_version("QEMU emulator version 8.2.2 (Debian 1:8.2.2+ds-0ubuntu1.18)\n"),
            Some((8, 2, 2))
        );
        assert_eq!(
            parse_version("QEMU emulator version 10.1.0\nCopyright"),
            Some((10, 1, 0))
        );
        assert_eq!(
            parse_version("QEMU emulator version 11.0"),
            Some((11, 0, 0))
        );
        assert_eq!(parse_version("not qemu"), None);
    }

    #[test]
    fn acceleration_keeps_the_vendor_check_coherent() {
        let intel_kvm = HostCaps {
            vendor: Some(Vendor::Intel),
            kvm: true,
            hvf: false,
        };
        assert_eq!(
            intel_kvm.choose(AccelChoice::Auto, Vendor::Intel).unwrap(),
            Accel::Kvm
        );
        assert_eq!(
            intel_kvm.choose(AccelChoice::Auto, Vendor::Amd).unwrap(),
            Accel::Tcg
        );
        assert!(intel_kvm.choose(AccelChoice::Kvm, Vendor::Amd).is_err());
        assert!(intel_kvm.choose(AccelChoice::Hvf, Vendor::Intel).is_err());
        let bare = HostCaps {
            vendor: Some(Vendor::Amd),
            kvm: false,
            hvf: false,
        };
        assert_eq!(
            bare.choose(AccelChoice::Auto, Vendor::Amd).unwrap(),
            Accel::Tcg
        );
        assert!(bare.choose(AccelChoice::Kvm, Vendor::Amd).is_err());
        assert_eq!(bare.native_vendor(), Vendor::Amd);
    }
}
