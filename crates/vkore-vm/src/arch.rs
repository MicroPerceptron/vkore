//! The instruction set a Kore guest runs, and what follows from it.
//!
//! A Kore ISO names its architecture by the one removable-media boot file
//! it carries (UEFI §3.5.1.1: `\EFI\BOOT\BOOTX64.EFI` or `BOOTAA64.EFI`,
//! Limine's UEFI stage), so a machine's architecture is read from its
//! medium ([`crate::media`]), never configured. Everything that differs
//! per architecture — the emulator, the firmware's target, the packaged
//! runtime's files, whether this host can accelerate it — is answered
//! here, so no caller matches on an architecture to find a file name.

use std::fmt;

/// A Kore guest's instruction set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestArch {
    /// x86-64: the PC (`q35`) machine Kore's production gate boots.
    X86_64,
    /// AArch64 (ARMv8-A and later): QEMU's `virt` machine.
    Aarch64,
}

impl GuestArch {
    /// Every architecture vKore composes a machine for.
    pub const ALL: [Self; 2] = [Self::X86_64, Self::Aarch64];

    /// The architecture this host's CPUs execute, when vKore composes a
    /// guest for it: the only one hardware acceleration can run.
    pub fn host() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Aarch64)
        } else {
            None
        }
    }

    /// The name QEMU, its firmware descriptors, Rust and Kore's image
    /// catalogue all spell it with.
    pub fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    /// The architecture spelled `name`, as [`name`](Self::name) spells it.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|arch| arch.name() == name)
    }

    /// The QEMU system emulator for this architecture.
    pub fn qemu_binary(self) -> &'static str {
        match self {
            Self::X86_64 => "qemu-system-x86_64",
            Self::Aarch64 => "qemu-system-aarch64",
        }
    }

    /// The UEFI removable-media boot file a medium for this architecture
    /// carries under `EFI/BOOT`.
    pub(crate) fn efi_boot_file(self) -> &'static str {
        match self {
            Self::X86_64 => "BOOTX64.EFI",
            Self::Aarch64 => "BOOTAA64.EFI",
        }
    }

    /// The machine type the profile composes, as a firmware descriptor's
    /// `targets[].machines` globs name it (`pc-q35-*`, `virt-*`).
    pub(crate) fn firmware_machine_prefix(self) -> &'static str {
        match self {
            Self::X86_64 => "pc-q35-",
            Self::Aarch64 => "virt-",
        }
    }

    /// The probe's machine type: the bare name of the same machine.
    pub(crate) fn machine(self) -> &'static str {
        match self {
            Self::X86_64 => "q35",
            Self::Aarch64 => "virt",
        }
    }

    /// A packaged QEMU runtime's emulator and split firmware for this
    /// architecture, relative to its root (QEMU's own install names).
    pub(crate) fn bundle_files(self) -> [&'static str; 3] {
        match self {
            Self::X86_64 => [
                "bin/qemu-system-x86_64",
                "share/qemu/edk2-x86_64-code.fd",
                "share/qemu/edk2-i386-vars.fd",
            ],
            Self::Aarch64 => [
                "bin/qemu-system-aarch64",
                "share/qemu/edk2-aarch64-code.fd",
                "share/qemu/edk2-arm-vars.fd",
            ],
        }
    }
}

impl fmt::Display for GuestArch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_each_architecture_has_its_own_files() {
        for arch in GuestArch::ALL {
            assert_eq!(GuestArch::parse(arch.name()), Some(arch));
            assert!(arch.qemu_binary().ends_with(arch.name()));
            assert!(arch.bundle_files()[0].ends_with(arch.qemu_binary()));
        }
        assert_eq!(GuestArch::parse("arm64"), None);
        assert_ne!(
            GuestArch::X86_64.efi_boot_file(),
            GuestArch::Aarch64.efi_boot_file()
        );
        assert_eq!(
            serde_json::to_string(&GuestArch::Aarch64).unwrap(),
            "\"aarch64\""
        );
    }

    #[test]
    fn the_host_is_one_of_ours_on_every_supported_build() {
        if cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
            assert!(GuestArch::host().is_some());
        }
    }
}
