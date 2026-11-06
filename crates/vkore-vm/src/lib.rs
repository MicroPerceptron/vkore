//! Kore machines under QEMU.
//!
//! A Kore guest is not a generic VM: the kernel refuses DMA without an IOMMU
//! and interrupts without remapping, cross-checks the CPU vendor against the
//! ACPI remapping table, founds its compositor only over a 32-bpp aperture,
//! and boots through Limine in exactly the paging mode its ISO names. This
//! crate composes the topology Kore's own production gate boots
//! ([`profile`]), probes what this host's QEMU can realize instead of
//! guessing from version strings ([`probe`]), finds UEFI firmware the way
//! QEMU documents ([`firmware`]), reads what a boot medium asks for
//! ([`media`]), and follows a boot through Kore's serial witnesses
//! ([`readiness`]) and QEMU's machine protocol ([`qmp`]). [`machine`] puts
//! them together for one supervised guest.
//!
//! No Kore checkout is needed: the ISO is a caller-supplied artifact.

pub mod firmware;
pub mod machine;
pub mod media;
pub mod probe;
pub mod profile;
pub mod qmp;
pub mod readiness;

pub use firmware::Firmware;
pub use machine::{Machine, MachineError, MachineEvent, StopOutcome};
pub use media::BootMedium;
pub use probe::{Accel, AccelChoice, HostCaps, QemuCaps};
pub use profile::{Display, MachineSpec, Paging, ProfileError, Vendor};
pub use readiness::{Readiness, Stage};
