//! Linux host owner for HostLink's fixed ivshmem bulk window.
//!
//! The mapping carries only complete `kore-hostlink` bulk frames through the
//! allocation-free `kore-hostlink-shmem` ABI. Serial remains the control and
//! wake plane. A backing is create-new and mode 0600; an existing path is
//! evidence from another/live VM and is never truncated or reused.

use std::io;
use std::path::Path;

use kore_hostlink::{BULK_FRAME_MAX, LinkEpoch, SHMEM_QUEUE_CAPACITY, SHMEM_SLOT_PAYLOAD_BYTES};
use kore_hostlink_shmem::{Epoch, GeometryError, Layout, ReceiveError, SendError, WindowError};

/// Power-of-two BAR/backing size used by the QEMU production topology. The
/// shared ABI occupies a checked prefix and leaves the remainder untouched.
pub const IVSHMEM_BACKING_BYTES: usize = 32 << 20;
const _: () = assert!(IVSHMEM_BACKING_BYTES.is_power_of_two());

#[derive(Debug)]
pub enum IvshmemError {
    UnsupportedPlatform,
    UnsafePath,
    Io(io::Error),
    Geometry(GeometryError),
    Window(WindowError),
    Send(SendError),
    Receive(ReceiveError),
    Inactive,
    Epoch,
}

impl From<io::Error> for IvshmemError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<GeometryError> for IvshmemError {
    fn from(value: GeometryError) -> Self {
        Self::Geometry(value)
    }
}

impl From<WindowError> for IvshmemError {
    fn from(value: WindowError) -> Self {
        Self::Window(value)
    }
}

impl From<SendError> for IvshmemError {
    fn from(value: SendError) -> Self {
        Self::Send(value)
    }
}

impl From<ReceiveError> for IvshmemError {
    fn from(value: ReceiveError) -> Self {
        Self::Receive(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IvshmemMetrics {
    pub publications: u64,
    pub records_received: u64,
    pub bytes_published: u64,
    pub bytes_received: u64,
    pub full: u64,
    pub empty: u64,
    pub resets: u64,
    /// One host copy into or out of the shared slot per record.
    pub copies: u64,
}

/// The one reviewed geometry. A payload slot includes the complete HostLink
/// envelope, not merely the largest benchmark body.
pub fn frozen_layout() -> Result<Layout, IvshmemError> {
    let layout = Layout::new(SHMEM_QUEUE_CAPACITY, SHMEM_SLOT_PAYLOAD_BYTES)?;
    debug_assert_eq!(layout.payload_bytes() as usize, BULK_FRAME_MAX);
    Ok(layout)
}

/// Exact QEMU `ivshmem-plain` arguments for an already-created backing.
pub fn qemu_arguments(path: &Path, bytes: usize) -> Result<Vec<String>, IvshmemError> {
    let path = path.to_str().ok_or(IvshmemError::UnsafePath)?;
    if path.is_empty() || path.contains([',', '\n', '\r', '\0']) || bytes != IVSHMEM_BACKING_BYTES {
        return Err(IvshmemError::UnsafePath);
    }
    Ok(vec![
        "-object".into(),
        format!("memory-backend-file,id=hostlink_shmem,mem-path={path},size={bytes},share=on"),
        "-device".into(),
        "ivshmem-plain,id=hostlink_bulk,memdev=hostlink_shmem".into(),
    ])
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs::{self, File, OpenOptions};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::PathBuf;

    use kore_hostlink_shmem::{Side, endpoint, init, reset};
    use memmap2::{MmapMut, MmapOptions};

    use super::*;
    use kore_hostlink::derive_shmem_epoch;

    pub struct IvshmemBulkHost {
        _file: File,
        map: MmapMut,
        path: PathBuf,
        dev: u64,
        ino: u64,
        layout: Layout,
        active: Option<Epoch>,
        publication: u64,
        metrics: IvshmemMetrics,
    }

    impl IvshmemBulkHost {
        pub fn create(path: &Path) -> Result<Self, IvshmemError> {
            if path.as_os_str().is_empty() {
                return Err(IvshmemError::UnsafePath);
            }
            let layout = frozen_layout()?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
            file.set_len(IVSHMEM_BACKING_BYTES as u64)?;
            let metadata = file.metadata()?;
            if !metadata.file_type().is_file() || metadata.mode() & 0o777 != 0o600 {
                return Err(IvshmemError::UnsafePath);
            }
            // SAFETY: this owner created, sized, and exclusively initializes
            // the regular file before QEMU is launched or another mapping is
            // handed out. The mapping never outlives the retained file.
            let mut map = unsafe {
                MmapOptions::new()
                    .len(IVSHMEM_BACKING_BYTES)
                    .map_mut(&file)?
            };
            let bootstrap = Epoch::new(1).expect("one is a nonzero bootstrap epoch");
            init(&mut map, layout, bootstrap)?;
            map.flush()?;
            Ok(Self {
                _file: file,
                map,
                path: path.to_owned(),
                dev: metadata.dev(),
                ino: metadata.ino(),
                layout,
                active: None,
                publication: 0,
                metrics: IvshmemMetrics::default(),
            })
        }

        pub fn activate(&mut self, link_epoch: LinkEpoch) -> Result<Epoch, IvshmemError> {
            let value = derive_shmem_epoch(link_epoch).ok_or(IvshmemError::Epoch)?;
            let epoch = Epoch::new(value).ok_or(IvshmemError::Epoch)?;
            match reset(&mut self.map, epoch) {
                Ok(_) => {}
                Err(WindowError::SameEpoch) => {}
                Err(error) => return Err(error.into()),
            }
            self.active = Some(epoch);
            self.publication = 0;
            self.metrics.resets = self.metrics.resets.saturating_add(1);
            Ok(epoch)
        }

        pub fn deactivate(&mut self) {
            self.active = None;
            self.publication = 0;
        }

        pub fn try_publish(&mut self, frame: &[u8]) -> Result<u64, IvshmemError> {
            if frame.len() > BULK_FRAME_MAX {
                return Err(IvshmemError::Send(SendError::TooLarge {
                    requested: frame.len(),
                    maximum: BULK_FRAME_MAX,
                }));
            }
            let epoch = self.active.ok_or(IvshmemError::Inactive)?;
            let result = {
                let mut side = endpoint(&mut self.map, epoch, Side::B)?;
                side.tx().try_push(frame)
            };
            match result {
                Ok(()) => {}
                Err(SendError::Full) => {
                    self.metrics.full = self.metrics.full.saturating_add(1);
                    return Err(IvshmemError::Send(SendError::Full));
                }
                Err(error) => return Err(error.into()),
            }
            self.publication = self.publication.checked_add(1).ok_or(IvshmemError::Epoch)?;
            self.metrics.publications = self.metrics.publications.saturating_add(1);
            self.metrics.bytes_published = self
                .metrics
                .bytes_published
                .saturating_add(frame.len() as u64);
            self.metrics.copies = self.metrics.copies.saturating_add(1);
            Ok(self.publication)
        }

        pub fn try_receive(&mut self, output: &mut [u8]) -> Result<Option<usize>, IvshmemError> {
            let epoch = self.active.ok_or(IvshmemError::Inactive)?;
            let result = {
                let mut side = endpoint(&mut self.map, epoch, Side::B)?;
                side.rx().try_pop(output)
            };
            match result {
                Ok(bytes) => {
                    self.metrics.records_received = self.metrics.records_received.saturating_add(1);
                    self.metrics.bytes_received =
                        self.metrics.bytes_received.saturating_add(bytes as u64);
                    self.metrics.copies = self.metrics.copies.saturating_add(1);
                    Ok(Some(bytes))
                }
                Err(ReceiveError::Empty) => {
                    self.metrics.empty = self.metrics.empty.saturating_add(1);
                    Ok(None)
                }
                Err(error) => Err(error.into()),
            }
        }

        pub const fn layout(&self) -> Layout {
            self.layout
        }

        pub const fn epoch(&self) -> Option<Epoch> {
            self.active
        }

        pub const fn publication(&self) -> u64 {
            self.publication
        }

        pub const fn metrics(&self) -> IvshmemMetrics {
            self.metrics
        }

        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for IvshmemBulkHost {
        fn drop(&mut self) {
            if let Ok(metadata) = fs::symlink_metadata(&self.path)
                && metadata.dev() == self.dev
                && metadata.ino() == self.ino
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    pub struct IvshmemBulkHost;

    impl IvshmemBulkHost {
        pub fn create(_: &Path) -> Result<Self, IvshmemError> {
            Err(IvshmemError::UnsupportedPlatform)
        }

        pub fn activate(&mut self, _: LinkEpoch) -> Result<Epoch, IvshmemError> {
            Err(IvshmemError::UnsupportedPlatform)
        }

        pub fn deactivate(&mut self) {}

        pub fn try_publish(&mut self, _: &[u8]) -> Result<u64, IvshmemError> {
            Err(IvshmemError::UnsupportedPlatform)
        }

        pub fn try_receive(&mut self, _: &mut [u8]) -> Result<Option<usize>, IvshmemError> {
            Err(IvshmemError::UnsupportedPlatform)
        }

        pub fn layout(&self) -> Layout {
            frozen_layout().expect("the frozen geometry is compile-time bounded")
        }

        pub const fn epoch(&self) -> Option<Epoch> {
            None
        }

        pub const fn publication(&self) -> u64 {
            0
        }

        pub const fn metrics(&self) -> IvshmemMetrics {
            IvshmemMetrics {
                publications: 0,
                records_received: 0,
                bytes_published: 0,
                bytes_received: 0,
                full: 0,
                empty: 0,
                resets: 0,
                copies: 0,
            }
        }

        pub fn path(&self) -> &Path {
            Path::new("")
        }
    }
}

pub use platform::IvshmemBulkHost;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_shape_is_exact_and_paths_are_not_injected() {
        let layout = frozen_layout().unwrap();
        assert_eq!(layout.capacity(), SHMEM_QUEUE_CAPACITY);
        assert_eq!(layout.payload_bytes(), SHMEM_SLOT_PAYLOAD_BYTES);
        let args = qemu_arguments(
            Path::new("/run/user/1000/kore-vm/bulk.mem"),
            IVSHMEM_BACKING_BYTES,
        )
        .unwrap();
        assert_eq!(args.len(), 4);
        assert!(args[1].contains(&format!("size={IVSHMEM_BACKING_BYTES}")));
        assert!(args[3].contains("ivshmem-plain"));
        assert!(qemu_arguments(Path::new("/tmp/bad,path"), 1).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn backing_is_create_new_0600_and_bidirectional() {
        use std::fs;
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        use std::time::{SystemTime, UNIX_EPOCH};

        use kore_hostlink_shmem::{Side, endpoint};
        use memmap2::MmapOptions;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "kore-hostlink-ivshmem-{}-{unique}",
            std::process::id()
        ));
        fs::DirBuilder::new()
            .recursive(false)
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let path = directory.join("bulk.mem");
        let mut host = IvshmemBulkHost::create(&path).unwrap();
        assert!(IvshmemBulkHost::create(&path).is_err());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let link = LinkEpoch([7; 16]);
        let epoch = host.activate(link).unwrap();
        let peer_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        // SAFETY: the host initialized the exact file size and both mappings
        // obey the SPSC side split.
        let mut peer_map = unsafe {
            MmapOptions::new()
                .len(host.layout().window_bytes())
                .map_mut(&peer_file)
                .unwrap()
        };
        let publication = host.try_publish(b"host-to-guest").unwrap();
        assert_eq!(publication, 1);
        let mut bytes = [0; 32];
        {
            let mut peer = endpoint(&mut peer_map, epoch, Side::A).unwrap();
            let received = peer.rx().try_pop(&mut bytes).unwrap();
            assert_eq!(&bytes[..received], b"host-to-guest");
            peer.tx().try_push(b"guest-to-host").unwrap();
        }
        assert_eq!(host.try_receive(&mut bytes).unwrap(), Some(13));
        assert_eq!(&bytes[..13], b"guest-to-host");
        drop(host);
        assert!(!path.exists());
        fs::remove_dir(&directory).unwrap();
    }
}
