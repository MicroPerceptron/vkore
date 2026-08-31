//! Offline adapter from a verified snapshot artifact to a Kore block image.
//!
//! The adapter implements the ordinary direct-block seam over one host file.
//! Formatting, import, atomic publication, and later recovery therefore use
//! exactly the same filesystem implementation as the founded VM service.

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use kore_block::{
    BlockBuffer, BlockChannel, BlockError, BlockGeometry, CompletionMode, DeviceCommand,
    DeviceCompletion, DirectBlockDevice,
};
use kore_svc_fs::{BlockProvider, SessionId, WorkspaceStore, block_on, persistence::PersistentFs};
use kore_workspace_sync::{Policy, SnapshotId, import_from};

use crate::{Artifact, Error, secure_output_options};

const BLOCK_BYTES: u64 = 512;
const MAX_TRANSFER_BLOCKS: u32 = 128;
const MIN_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const DEFAULT_IMAGE_BYTES: u64 = 1024 * 1024 * 1024;

struct FileBuffer(Vec<u8>);

impl BlockBuffer for FileBuffer {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn bytes(&self) -> &[u8] {
        &self.0
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

struct FileBlockDevice {
    file: File,
    geometry: BlockGeometry,
    completion: Option<DeviceCompletion>,
}

impl FileBlockDevice {
    fn from_file(file: File, bytes: u64) -> Result<Self, Error> {
        if bytes < MIN_IMAGE_BYTES || !bytes.is_multiple_of(BLOCK_BYTES) {
            return Err(Error::Image(
                "size must be at least 8 MiB and a multiple of 512 bytes",
            ));
        }
        let geometry = BlockGeometry {
            block_size: BLOCK_BYTES as u32,
            block_count: bytes / BLOCK_BYTES,
            max_transfer_blocks: MAX_TRANSFER_BLOCKS,
            atomic_write_blocks: 1,
            flush: true,
            discard: false,
            read_only: false,
        }
        .validate()
        .map_err(|_| Error::Image("invalid block geometry"))?;
        Ok(Self {
            file,
            geometry,
            completion: None,
        })
    }

    #[cfg(test)]
    fn open(path: &Path) -> Result<Self, Error> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let bytes = file.metadata()?.len();
        Self::from_file(file, bytes)
    }

    fn checked_range(&self, lba: u64, blocks: u32) -> Result<(u64, usize), BlockError> {
        let end = lba
            .checked_add(u64::from(blocks))
            .ok_or(BlockError::Range)?;
        if blocks == 0 || end > self.geometry.block_count {
            return Err(BlockError::Range);
        }
        let offset = lba.checked_mul(BLOCK_BYTES).ok_or(BlockError::Range)?;
        let bytes =
            usize::try_from(u64::from(blocks) * BLOCK_BYTES).map_err(|_| BlockError::Range)?;
        Ok((offset, bytes))
    }
}

impl DirectBlockDevice for FileBlockDevice {
    type Buffer = FileBuffer;

    fn geometry(&self) -> BlockGeometry {
        self.geometry
    }

    fn allocate_buffer(&mut self, bytes: usize) -> Result<Self::Buffer, BlockError> {
        if bytes == 0
            || bytes
                > usize::try_from(u64::from(self.geometry.max_transfer_blocks) * BLOCK_BYTES)
                    .map_err(|_| BlockError::TooLarge)?
        {
            return Err(BlockError::TooLarge);
        }
        Ok(FileBuffer(vec![0; bytes]))
    }

    fn submit(
        &mut self,
        command: DeviceCommand,
        mut buffer: Option<&mut Self::Buffer>,
        cookie: u64,
    ) -> Result<(), BlockError> {
        if self.completion.is_some() {
            return Err(BlockError::Backpressure);
        }
        let result = match command {
            DeviceCommand::Read {
                lba,
                blocks,
                buffer_offset,
            } => self.checked_range(lba, blocks).and_then(|(offset, bytes)| {
                let buffer = buffer.as_deref_mut().ok_or(BlockError::UnknownBuffer)?;
                let end = buffer_offset.checked_add(bytes).ok_or(BlockError::Range)?;
                let destination = buffer
                    .bytes_mut()
                    .get_mut(buffer_offset..end)
                    .ok_or(BlockError::Range)?;
                self.file
                    .seek(SeekFrom::Start(offset))
                    .and_then(|_| self.file.read_exact(destination))
                    .map_err(|_| BlockError::Media)
            }),
            DeviceCommand::Write {
                lba,
                blocks,
                buffer_offset,
            } => self.checked_range(lba, blocks).and_then(|(offset, bytes)| {
                let buffer = buffer.as_deref().ok_or(BlockError::UnknownBuffer)?;
                let end = buffer_offset.checked_add(bytes).ok_or(BlockError::Range)?;
                let source = buffer
                    .bytes()
                    .get(buffer_offset..end)
                    .ok_or(BlockError::Range)?;
                self.file
                    .seek(SeekFrom::Start(offset))
                    .and_then(|_| self.file.write_all(source))
                    .map_err(|_| BlockError::Media)
            }),
            DeviceCommand::Flush => self.file.sync_data().map_err(|_| BlockError::Media),
            DeviceCommand::Discard { .. } => Err(BlockError::Unsupported),
        };
        self.completion = Some(DeviceCompletion { cookie, result });
        Ok(())
    }

    fn poll(&mut self) -> Option<DeviceCompletion> {
        self.completion.take()
    }

    fn quiesce(&mut self) -> Result<(), BlockError> {
        if self.completion.is_some() {
            Err(BlockError::Draining)
        } else {
            self.file.sync_data().map_err(|_| BlockError::Media)
        }
    }
}

type ImageProvider = BlockProvider<FileBlockDevice, 2, 8>;

fn provider(device: FileBlockDevice) -> Result<ImageProvider, Error> {
    let channel = BlockChannel::<_, 2, 8>::new(device, CompletionMode::Poll)
        .map_err(|_| Error::Image("could not create direct block channel"))?;
    BlockProvider::new(channel).map_err(|_| Error::Image("could not create filesystem provider"))
}

/// Create a new raw Kore filesystem image and atomically publish `artifact`
/// inside it. The output is never overwritten.
fn seed_image_inner(
    artifact: &mut Artifact,
    output: &Path,
    image_bytes: u64,
) -> Result<SnapshotId, Error> {
    let file = match secure_output_options().read(true).open(output) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Error::OutputExists);
        }
        Err(error) => return Err(Error::Io(error)),
    };
    if let Err(error) = file.set_len(image_bytes) {
        drop(file);
        let _ = std::fs::remove_file(output);
        return Err(Error::Io(error));
    }

    let result = (|| {
        let device = FileBlockDevice::from_file(file, image_bytes)?;
        let importer = SessionId::new(0);
        let broker = SessionId::new(1);
        let mut volume = block_on(PersistentFs::format(provider(device)?, importer))
            .map_err(|_| Error::Image("filesystem format failed"))?;
        block_on(volume.engine().attach_session(broker))
            .map_err(|_| Error::Image("broker session attach failed"))?;
        let manifest = artifact.manifest().clone();
        let store = WorkspaceStore::new(volume.engine(), importer, broker);
        let (_, commit) = block_on(import_from(store, manifest, Policy::default(), artifact))
            .map_err(|_| Error::Image("workspace import failed"))?;
        block_on(volume.commit(importer))
            .map_err(|_| Error::Image("filesystem publication failed"))?;
        Ok(commit.snapshot)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

pub fn seed_image(
    artifact: &mut Artifact,
    output: &Path,
    image_bytes: u64,
) -> Result<SnapshotId, Error> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("workspace-image-import".into())
            .stack_size(64 * 1024 * 1024)
            .spawn_scoped(scope, || seed_image_inner(artifact, output, image_bytes))
            .map_err(Error::Io)?
            .join()
            .map_err(|_| Error::Image("image worker panicked"))?
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kore_svc_fs::{Name, Rights};

    #[test]
    fn raw_image_recovers_snapshot_and_fresh_broker_authority() {
        std::thread::Builder::new()
            .name("workspace-image-recovery".into())
            .stack_size(64 * 1024 * 1024)
            .spawn(|| {
                let source = tempfile::tempdir().unwrap();
                std::fs::create_dir(source.path().join("face-0")).unwrap();
                std::fs::create_dir(source.path().join("face-1")).unwrap();
                std::fs::write(source.path().join("face-0/identity"), b"seat-a\n").unwrap();
                std::fs::write(source.path().join("face-1/identity"), b"seat-b\n").unwrap();
                let mut artifact = Artifact::scan(source.path(), [0; 32]).unwrap();
                let expected = artifact.manifest().snapshot_id();
                let destination = tempfile::tempdir().unwrap();
                let output = destination.path().join("workspace.raw");
                assert_eq!(
                    seed_image(&mut artifact, &output, DEFAULT_IMAGE_BYTES).unwrap(),
                    expected
                );

                let device = FileBlockDevice::open(&output).unwrap();
                let importer = SessionId::new(0);
                let broker = SessionId::new(1);
                let volume =
                    block_on(PersistentFs::recover(provider(device).unwrap(), importer)).unwrap();
                block_on(volume.engine().attach_session(broker)).unwrap();
                let recovered = block_on(
                    WorkspaceStore::new(volume.engine(), importer, broker).recover_current(),
                )
                .unwrap()
                .unwrap();
                assert_eq!(recovered.snapshot, expected);
                let root = volume
                    .engine()
                    .open_grant(broker, recovered.broker_root, Rights::READ)
                    .unwrap();
                let face = volume
                    .engine()
                    .open_at(
                        broker,
                        root.handle,
                        &Name::new("face-0").unwrap(),
                        Rights::READ,
                    )
                    .unwrap();
                let identity = volume
                    .engine()
                    .open_at(
                        broker,
                        face.handle,
                        &Name::new("identity").unwrap(),
                        Rights::READ,
                    )
                    .unwrap();
                let mut bytes = [0; 7];
                assert_eq!(
                    block_on(volume.engine().read(broker, identity.handle, 0, &mut bytes)).unwrap(),
                    bytes.len()
                );
                assert_eq!(&bytes, b"seat-a\n");
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
