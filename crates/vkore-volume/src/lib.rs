//! A Kore machine's DagFS volume, from the host.
//!
//! No host kernel mounts DagFS. A booted Kore does, and this crate reads and
//! writes a volume with exactly that engine: libkore's `kore-host` drives the
//! filesystem row's own locate, recovery and commit (the exported `kore-vol`
//! procedure), so what vKore shows is what Kore sees, and what vKore writes is
//! what the machine's next boot mounts. Nothing here parses volume bytes.
//!
//! - [`Disk`] names a raw disk image or, on Linux, a machine's own disk.
//! - [`Browser`] mounts it read-only on a thread of its own and answers
//!   listings, walks and bounded reads as data, for as long as it is held.
//! - [`Browser::export`] copies a file or tree out to a host directory:
//!   root-confined, never through a symbolic link, each file published
//!   atomically and never over an existing one unless asked.
//! - [`Disk::install`], [`Disk::remove`] and [`Disk::format`] change the
//!   volume through Kore's all-or-nothing publication.
//! - [`preview`] turns bytes into an image (with the capture metadata Kore's
//!   screenshots carry), text, or a binary summary.
//! - [`workspace`] stages a host project into a terminal's home before a
//!   machine boots, and collects and applies what the machine changed after
//!   it shuts down.
//!
//! **Offline only.** A running machine keeps its free extents and its next
//! root slot in memory; Kore's image device takes a record lock and refuses
//! an image another process holds, as QEMU holds the disks it runs. A
//! [`Browser`] holds a shared lock for its lifetime: drop it before a machine
//! boots from the same disk.

#![cfg(unix)]
#![forbid(unsafe_code)]

mod browse;
mod claim;
mod disk;
mod error;
mod path;
pub mod preview;
mod transfer;
pub mod workspace;

pub use browse::{Browser, Entry, EntryKind, Listing, MountStatus, Walked};
pub use disk::{Disk, DiskKind};
pub use error::Error;
pub use path::{Place, VolPath};
pub use preview::{Capture, Preview};
pub use transfer::{ExportOptions, Exported, Installed, Removed};

/// The volume bounds Kore's engine enforces, checked before anything is
/// written.
pub mod bounds {
    pub use kore_host::volume::{FORMAT_VERSION, MAX_DIR_ENTRIES, MAX_OBJECTS, MAX_PATH_DEPTH};
    /// The longest name a Kore directory entry holds, in UTF-8 bytes.
    pub const MAX_NAME_BYTES: usize = 32;
}

/// Where Kore's screen captures land, from a terminal's home.
pub const SCREENSHOTS: &str = "pictures/screenshots";
