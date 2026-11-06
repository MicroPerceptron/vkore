//! One owner per disk in this process.
//!
//! Kore's image device locks a disk with a POSIX record lock, which belongs
//! to the process and is dropped when *any* of its descriptors of the file
//! closes. Two opens of one disk from this process never refuse each other,
//! and the first to close drops the other's lock — after which a machine
//! could start on a disk a [`crate::Browser`] still reads, or an install
//! could write under it. The engine cannot see this; a process can, so every
//! operation that opens a disk first takes its claim here and holds it until
//! the engine has closed the disk again.
//!
//! The registry is touched only when a disk is opened or released, never on
//! a read or a write, so it serializes nothing a volume does.

use std::{
    collections::BTreeSet,
    sync::{Mutex, PoisonError},
};

use crate::{Disk, Error};

/// The (device, inode) of each disk this process has open.
static CLAIMED: Mutex<BTreeSet<(u64, u64)>> = Mutex::new(BTreeSet::new());

/// The claim on one disk; released when dropped.
#[derive(Debug)]
pub(crate) struct Claim {
    identity: (u64, u64),
}

impl Claim {
    /// Claim `disk` for this process, refusing a disk this process already
    /// has open.
    pub(crate) fn take(disk: &Disk) -> Result<Self, Error> {
        let identity = disk.identity();
        let mut claimed = CLAIMED.lock().unwrap_or_else(PoisonError::into_inner);
        if !claimed.insert(identity) {
            return Err(Error::Path(format!(
                "{} is already open in this process: close its browser (or let its \
                 operation finish) first",
                disk.path().display()
            )));
        }
        Ok(Self { identity })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        CLAIMED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.identity);
    }
}
