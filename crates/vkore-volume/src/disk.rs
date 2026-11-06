use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

use kore_host::volume;

use crate::{Error, claim::Claim};

/// What a [`Disk`] is on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskKind {
    /// A regular file holding a raw disk image: what vKore's machines boot,
    /// or a copy of a machine's disk.
    Image,
    /// A block device: a machine's own disk (or the partition holding its
    /// user volume), attached to this host with that machine shut down.
    /// Linux only; needs read (and, to write, write) permission on the node.
    Device,
}

/// A Kore machine's disk, as the host names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    path: PathBuf,
    kind: DiskKind,
    block_size: Option<u32>,
    /// The (device, inode) the path resolved to: what this process's claim
    /// on the disk is keyed by, whatever name reached it.
    identity: (u64, u64),
}

impl Disk {
    /// The disk at `path`: a raw image file, or on Linux a block device.
    /// Symbolic links are resolved (`/dev/disk/by-id/…` names a device that
    /// way), and the disk is then named by what they resolve to.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let resolved = fs::canonicalize(path).map_err(|error| Error::host(path, error))?;
        let metadata = fs::metadata(&resolved).map_err(|error| Error::host(&resolved, error))?;
        let kind = if metadata.is_file() {
            DiskKind::Image
        } else if metadata.file_type().is_block_device() && cfg!(target_os = "linux") {
            DiskKind::Device
        } else {
            return Err(Error::NotADisk(resolved));
        };
        Ok(Self {
            path: resolved,
            kind,
            block_size: None,
            identity: (metadata.dev(), metadata.ino()),
        })
    }

    /// Read the disk with `bytes`-sized logical blocks (512 or 4096) instead
    /// of the default: an image's is QEMU's 512, a device's its own.
    pub fn with_block_size(mut self, bytes: u32) -> Result<Self, Error> {
        if !matches!(bytes, 512 | 4096) {
            return Err(Error::Path(format!(
                "a block size of {bytes} bytes: 512 or 4096"
            )));
        }
        self.block_size = Some(bytes);
        Ok(self)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> DiskKind {
        self.kind
    }

    pub fn block_size(&self) -> Option<u32> {
        self.block_size
    }

    pub(crate) fn identity(&self) -> (u64, u64) {
        self.identity
    }

    /// Create a blank, sparse raw image of `bytes` at `path` — blank roots,
    /// as `qemu-img create` makes one — refusing a path that exists. The
    /// machine formats it at its first boot, or [`Disk::format`] does now.
    pub fn create_image(path: impl AsRef<Path>, bytes: u64) -> Result<Self, Error> {
        let path = path.as_ref();
        const MIB: u64 = 1 << 20;
        if bytes < 16 * MIB || !bytes.is_multiple_of(MIB) {
            return Err(Error::Path(format!(
                "a disk image of {bytes} bytes: at least 16 MiB, in whole MiB"
            )));
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| Error::host(path, error))?;
        file.set_len(bytes)
            .map_err(|error| Error::host(path, error))?;
        file.sync_all().map_err(|error| Error::host(path, error))?;
        Self::new(path)
    }

    /// Format blank roots as Kore's first boot would. Only an image is ever
    /// formatted here: a machine formats its own disk at its first boot.
    pub fn format(&self) -> Result<u64, Error> {
        let _claim = Claim::take(self)?;
        Ok(volume::format(&self.path, self.block_size)?.generation)
    }

    /// Whether the disk's roots are blank: a volume nothing has formatted.
    pub fn is_blank(&self) -> Result<bool, Error> {
        let _claim = Claim::take(self)?;
        match volume::read(&self.path, self.block_size, |_| Ok(())) {
            Ok(()) => Ok(false),
            Err(volume::VolError::Blank) => Ok(true),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_is_a_regular_file_and_nothing_else_is_a_disk() {
        let dir = tempfile::tempdir().unwrap();
        let disk = Disk::create_image(dir.path().join("a.raw"), 16 << 20).unwrap();
        assert_eq!(disk.kind(), DiskKind::Image);
        assert!(disk.is_blank().unwrap());
        assert!(matches!(
            Disk::create_image(dir.path().join("a.raw"), 16 << 20),
            Err(Error::Host { .. })
        ));
        assert!(Disk::create_image(dir.path().join("b.raw"), (16 << 20) + 1).is_err());
        assert!(matches!(Disk::new(dir.path()), Err(Error::NotADisk(_))));
        assert!(disk.clone().with_block_size(1024).is_err());
        assert_eq!(disk.with_block_size(4096).unwrap().block_size(), Some(4096));
    }
}
