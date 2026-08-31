//! Capability-rooted host adapter for kore workspace snapshots.

pub mod image;
pub mod live;

pub use image::{DEFAULT_IMAGE_BYTES, seed_image};
pub use live::{LiveRoot, ServedRange, WatchHints};

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

use cap_std::{ambient_authority, fs::Dir};
use kore_workspace_sync::{
    CanonicalPath, Change, ChangeKind, ChangeSet, ChangeSetBuilder, Entry, EntryKind, ImportSource,
    Manifest, ManifestBuilder, ManifestError, Policy, SnapshotHeader, SnapshotId, StagedFile,
    bounds::{MAX_ENTRIES, MAX_FILE_BYTES, MAX_SNAPSHOT_BYTES},
};
use zerocopy::{FromBytes, IntoBytes};

const ARTIFACT_MAGIC: &[u8; 8] = b"KWSNAP1\0";
const SOURCE_DOMAIN: &[u8] = b"kore.workspace.source.v1\0";

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Manifest(ManifestError),
    InvalidArtifact,
    UnsupportedType(String),
    UnsupportedMetadata(String),
    HardLink(String),
    SparseFile(String),
    NonUtf8Name,
    SourceChanged(String),
    Bounds,
    OutputExists,
    WrongParent,
    ApprovalMismatch,
    Conflict,
    Partial { applied: usize },
    Watch(String),
    Image(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O: {error}"),
            Self::Manifest(error) => write!(f, "manifest: {error:?}"),
            Self::InvalidArtifact => f.write_str("invalid workspace artifact"),
            Self::UnsupportedType(path) => write!(f, "unsupported filesystem object: {path}"),
            Self::UnsupportedMetadata(path) => {
                write!(f, "unsupported filesystem metadata: {path}")
            }
            Self::HardLink(path) => write!(f, "hard-linked file is unsupported: {path}"),
            Self::SparseFile(path) => write!(f, "sparse file is unsupported: {path}"),
            Self::NonUtf8Name => f.write_str("workspace contains a non-UTF-8 name"),
            Self::SourceChanged(path) => write!(f, "source changed during snapshot: {path}"),
            Self::Bounds => f.write_str("workspace exceeds frozen bounds"),
            Self::OutputExists => f.write_str("output already exists"),
            Self::WrongParent => f.write_str("desired snapshot does not descend from the base"),
            Self::ApprovalMismatch => f.write_str("approval does not name the exact change set"),
            Self::Conflict => f.write_str("host workspace no longer matches the approved base"),
            Self::Partial { applied } => {
                write!(f, "host filesystem stopped after {applied} applied changes")
            }
            Self::Watch(detail) => write!(f, "workspace watcher: {detail}"),
            Self::Image(detail) => write!(f, "Kore filesystem image: {detail}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportPlan {
    base: SnapshotId,
    desired: SnapshotId,
    changes: ChangeSet,
}

impl ExportPlan {
    pub fn between(base: &Artifact, desired: &Artifact) -> Result<Self, Error> {
        if desired.manifest.header().parent_snapshot != base.manifest.snapshot_id() {
            return Err(Error::WrongParent);
        }
        let mut builder = ChangeSetBuilder::new(base.manifest.snapshot_id());
        let base_entries = base
            .manifest
            .entries()
            .iter()
            .map(|entry| (&entry.path, entry))
            .collect::<BTreeMap<_, _>>();
        let desired_entries = desired
            .manifest
            .entries()
            .iter()
            .map(|entry| (&entry.path, entry))
            .collect::<BTreeMap<_, _>>();

        // A rename is inferred only when the same file shape is unique on
        // both sides. Ambiguous equal-content files remain explicit
        // delete/create operations rather than a guessed identity move.
        let mut renamed_from = BTreeSet::new();
        let mut renamed_to = BTreeSet::new();
        for entry in base.manifest.entries().iter().filter(|entry| {
            entry.kind == EntryKind::File && !desired_entries.contains_key(&entry.path)
        }) {
            let candidates = desired
                .manifest
                .entries()
                .iter()
                .filter(|candidate| {
                    candidate.kind == EntryKind::File
                        && !base_entries.contains_key(&candidate.path)
                        && candidate.digest == entry.digest
                        && candidate.content_len == entry.content_len
                        && candidate.flags == entry.flags
                })
                .collect::<Vec<_>>();
            let reverse_count = candidates.first().map_or(0, |candidate| {
                base.manifest
                    .entries()
                    .iter()
                    .filter(|source| {
                        source.kind == EntryKind::File
                            && !desired_entries.contains_key(&source.path)
                            && source.digest == candidate.digest
                            && source.content_len == candidate.content_len
                            && source.flags == candidate.flags
                    })
                    .count()
            });
            if candidates.len() == 1 && reverse_count == 1 {
                let target = candidates[0];
                builder.push(Change {
                    kind: ChangeKind::Rename,
                    path: entry.path.clone(),
                    target: Some(target.path.clone()),
                    expected: entry.digest,
                    replacement: [0; 32],
                    replacement_len: 0,
                    executable: false,
                })?;
                renamed_from.insert(entry.path.clone());
                renamed_to.insert(target.path.clone());
            }
        }

        for entry in base.manifest.entries() {
            match desired_entries.get(&entry.path) {
                None if !renamed_from.contains(&entry.path) => builder.push(Change {
                    kind: ChangeKind::Delete,
                    path: entry.path.clone(),
                    target: None,
                    expected: entry.digest,
                    replacement: [0; 32],
                    replacement_len: 0,
                    executable: false,
                })?,
                Some(next)
                    if entry.kind != next.kind
                        || entry.digest != next.digest
                        || entry.flags != next.flags
                        || entry.content_len != next.content_len =>
                {
                    if entry.kind != EntryKind::File || next.kind != EntryKind::File {
                        return Err(Error::Conflict);
                    }
                    builder.push(Change {
                        kind: ChangeKind::ModifyFile,
                        path: entry.path.clone(),
                        target: None,
                        expected: entry.digest,
                        replacement: next.digest,
                        replacement_len: next.content_len,
                        executable: next.flags.executable(),
                    })?;
                }
                Some(_) | None => {}
            }
        }
        for entry in desired.manifest.entries() {
            if !base_entries.contains_key(&entry.path) && !renamed_to.contains(&entry.path) {
                builder.push(Change {
                    kind: match entry.kind {
                        EntryKind::Directory => ChangeKind::CreateDirectory,
                        EntryKind::File => ChangeKind::CreateFile,
                    },
                    path: entry.path.clone(),
                    target: None,
                    expected: [0; 32],
                    replacement: if entry.kind == EntryKind::File {
                        entry.digest
                    } else {
                        [0; 32]
                    },
                    replacement_len: entry.content_len,
                    executable: entry.flags.executable(),
                })?;
            }
        }
        let changes = builder.finish()?;
        changes.verify()?;
        Ok(Self {
            base: base.manifest.snapshot_id(),
            desired: desired.manifest.snapshot_id(),
            changes,
        })
    }

    pub fn id(&self) -> [u8; 32] {
        self.changes.id
    }

    pub fn changes(&self) -> &[Change] {
        self.changes.changes()
    }

    pub fn apply(
        &self,
        root: &Path,
        base: &Artifact,
        desired: &Artifact,
        approval: [u8; 32],
        result: &Path,
    ) -> Result<ExportResult, Error> {
        if approval != self.id()
            || base.manifest.snapshot_id() != self.base
            || desired.manifest.snapshot_id() != self.desired
        {
            return Err(Error::ApprovalMismatch);
        }
        if result.exists() {
            return ExportResult::read(result, self.id());
        }
        let current = Artifact::scan(root, base.manifest.header().parent_snapshot)?;
        if current.manifest.snapshot_id() != self.base {
            return Err(Error::Conflict);
        }
        let root_dir = Dir::open_ambient_dir(root, ambient_authority())?;
        let mut applied = 0usize;
        for change in self
            .changes()
            .iter()
            .filter(|change| change.kind == ChangeKind::CreateDirectory)
        {
            if verify_precondition(&root_dir, change, base).is_err() {
                return Err(record_apply_failure(result, self.id(), applied)?);
            }
            if let Err(error) = create_directory(&root_dir, &change.path) {
                let outcome = ExportResult::partial(self.id(), applied);
                outcome.write_new(result)?;
                return Err(match error {
                    Error::Io(_) => Error::Partial { applied },
                    other => other,
                });
            }
            applied += 1;
        }
        for change in self
            .changes()
            .iter()
            .filter(|change| change.kind == ChangeKind::Rename)
        {
            if verify_precondition(&root_dir, change, base).is_err() {
                return Err(record_apply_failure(result, self.id(), applied)?);
            }
            let Some(target) = change.target.as_ref() else {
                return Err(Error::InvalidArtifact);
            };
            if rename_entry(&root_dir, &change.path, target).is_err() {
                let outcome = ExportResult::partial(self.id(), applied);
                outcome.write_new(result)?;
                return Err(Error::Partial { applied });
            }
            applied += 1;
        }
        for change in self
            .changes()
            .iter()
            .filter(|change| matches!(change.kind, ChangeKind::CreateFile | ChangeKind::ModifyFile))
        {
            if verify_precondition(&root_dir, change, base).is_err() {
                return Err(record_apply_failure(result, self.id(), applied)?);
            }
            let Some(content) = desired.content(&change.path) else {
                return Err(Error::InvalidArtifact);
            };
            if let Err(_error) =
                replace_file(&root_dir, &change.path, content, change.executable, applied)
            {
                let outcome = ExportResult::partial(self.id(), applied);
                outcome.write_new(result)?;
                return Err(Error::Partial { applied });
            }
            applied += 1;
        }
        for change in self
            .changes()
            .iter()
            .rev()
            .filter(|change| change.kind == ChangeKind::Delete)
        {
            if verify_precondition(&root_dir, change, base).is_err() {
                return Err(record_apply_failure(result, self.id(), applied)?);
            }
            if remove_entry(&root_dir, &change.path).is_err() {
                let outcome = ExportResult::partial(self.id(), applied);
                outcome.write_new(result)?;
                return Err(Error::Partial { applied });
            }
            applied += 1;
        }
        let observed = Artifact::scan(root, self.base)?;
        if observed.manifest.snapshot_id() != self.desired {
            let outcome = ExportResult::partial(self.id(), applied);
            outcome.write_new(result)?;
            return Err(Error::Partial { applied });
        }
        let outcome = ExportResult::complete(self.id(), self.desired, applied);
        outcome.write_new(result)?;
        Ok(outcome)
    }
}

fn record_apply_failure(path: &Path, change_set: [u8; 32], applied: usize) -> Result<Error, Error> {
    ExportResult::partial(change_set, applied).write_new(path)?;
    Ok(if applied == 0 {
        Error::Conflict
    } else {
        Error::Partial { applied }
    })
}

fn verify_precondition(root: &Dir, change: &Change, base: &Artifact) -> Result<(), Error> {
    let current = root.symlink_metadata(change.path.as_str());
    match change.kind {
        ChangeKind::CreateDirectory | ChangeKind::CreateFile => {
            return match current {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(Error::Conflict),
            };
        }
        ChangeKind::ModifyFile | ChangeKind::Delete | ChangeKind::Rename => {}
    }
    let expected = base
        .manifest
        .entries()
        .iter()
        .find(|entry| entry.path == change.path)
        .ok_or(Error::Conflict)?;
    if expected.digest != change.expected {
        return Err(Error::Conflict);
    }
    let metadata = current.map_err(|_| Error::Conflict)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::Conflict);
    }
    match expected.kind {
        EntryKind::Directory => {
            if !metadata.is_dir() {
                return Err(Error::Conflict);
            }
            let directory = root.open_dir(change.path.as_str())?;
            refuse_extended_metadata(&directory, change.path.as_str())
        }
        EntryKind::File => {
            if !metadata.is_file()
                || metadata.len() != u64::from(expected.content_len)
                || executable(&metadata) != expected.flags.executable()
            {
                return Err(Error::Conflict);
            }
            refuse_file_shape(&metadata, change.path.as_str())?;
            let mut file = root.open(change.path.as_str())?;
            refuse_extended_metadata(&file, change.path.as_str())?;
            let mut bytes = Vec::with_capacity(expected.content_len as usize);
            file.read_to_end(&mut bytes)?;
            let after = file.metadata()?;
            if metadata_fingerprint(&metadata) != metadata_fingerprint(&after)
                || bytes.len() != expected.content_len as usize
                || blake3::hash(&bytes).as_bytes() != &expected.digest
            {
                return Err(Error::Conflict);
            }
            Ok(())
        }
    }
}

impl From<kore_workspace_sync::ChangeSetError> for Error {
    fn from(_: kore_workspace_sync::ChangeSetError) -> Self {
        Self::InvalidArtifact
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportResult {
    pub change_set: [u8; 32],
    pub snapshot: [u8; 32],
    pub applied: u16,
    pub complete: bool,
}

impl ExportResult {
    fn complete(change_set: [u8; 32], snapshot: [u8; 32], applied: usize) -> Self {
        Self {
            change_set,
            snapshot,
            applied: applied as u16,
            complete: true,
        }
    }

    fn partial(change_set: [u8; 32], applied: usize) -> Self {
        Self {
            change_set,
            snapshot: [0; 32],
            applied: applied as u16,
            complete: false,
        }
    }

    fn write_new(self, path: &Path) -> Result<(), Error> {
        let options = secure_output_options();
        let mut file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(Error::OutputExists);
            }
            Err(error) => return Err(Error::Io(error)),
        };
        file.write_all(b"KWR1")?;
        file.write_all(&[self.complete as u8, 0])?;
        file.write_all(&self.applied.to_le_bytes())?;
        file.write_all(&self.change_set)?;
        file.write_all(&self.snapshot)?;
        file.sync_all()?;
        Ok(())
    }

    fn read(path: &Path, expected: [u8; 32]) -> Result<Self, Error> {
        let mut bytes = [0; 72];
        let mut file = OpenOptions::new().read(true).open(path)?;
        file.read_exact(&mut bytes)?;
        let mut extra = [0; 1];
        if file.read(&mut extra)? != 0 || &bytes[..4] != b"KWR1" || bytes[5] != 0 {
            return Err(Error::InvalidArtifact);
        }
        let mut change_set = [0; 32];
        change_set.copy_from_slice(&bytes[8..40]);
        if change_set != expected {
            return Err(Error::Conflict);
        }
        let mut snapshot = [0; 32];
        snapshot.copy_from_slice(&bytes[40..72]);
        Ok(Self {
            change_set,
            snapshot,
            applied: u16::from_le_bytes([bytes[6], bytes[7]]),
            complete: bytes[4] == 1,
        })
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ManifestError> for Error {
    fn from(value: ManifestError) -> Self {
        Self::Manifest(value)
    }
}

#[derive(Clone, Debug)]
pub struct Artifact {
    manifest: Manifest,
    contents: BTreeMap<CanonicalPath, Vec<u8>>,
}

impl Artifact {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn content(&self, path: &CanonicalPath) -> Option<&[u8]> {
        self.contents.get(path).map(Vec::as_slice)
    }

    pub fn scan(root: &Path, parent: SnapshotId) -> Result<Self, Error> {
        let root = Dir::open_ambient_dir(root, ambient_authority())?;
        Self::scan_cap(&root, parent)
    }

    fn scan_cap(root: &Dir, parent: SnapshotId) -> Result<Self, Error> {
        refuse_extended_metadata(root, ".")?;
        let first = scan_tree(root)?;
        let scanned = scan_tree(root)?;
        if first != scanned {
            return Err(Error::SourceChanged(".".into()));
        }
        if scanned.len() > MAX_ENTRIES {
            return Err(Error::Bounds);
        }
        let mut builder = ManifestBuilder::new(parent, Policy::default());
        let mut contents = BTreeMap::new();
        for item in scanned {
            builder.push(item.entry.clone())?;
            if let Some(bytes) = item.content {
                contents.insert(item.entry.path, bytes);
            }
        }
        let manifest = builder.finish()?;
        manifest.verify(Policy::default())?;
        Ok(Self { manifest, contents })
    }

    pub fn write_new(&self, output: &Path) -> Result<(), Error> {
        let options = secure_output_options();
        let mut file = match options.open(output) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(Error::OutputExists);
            }
            Err(error) => return Err(Error::Io(error)),
        };
        file.write_all(ARTIFACT_MAGIC)?;
        file.write_all(self.manifest.header().as_bytes())?;
        for (index, entry) in self.manifest.entries().iter().enumerate() {
            file.write_all(entry.header(index as u16).as_bytes())?;
            file.write_all(entry.path.as_bytes())?;
            if entry.kind == EntryKind::File {
                file.write_all(self.content(&entry.path).ok_or(Error::InvalidArtifact)?)?;
            }
        }
        file.sync_all()?;
        Ok(())
    }

    pub fn read(path: &Path) -> Result<Self, Error> {
        let mut bytes = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(path)?
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_SNAPSHOT_BYTES as usize + 64 * 1024 {
            return Err(Error::Bounds);
        }
        let rest = bytes
            .strip_prefix(ARTIFACT_MAGIC)
            .ok_or(Error::InvalidArtifact)?;
        let (header, mut rest) =
            SnapshotHeader::read_from_prefix(rest).map_err(|_| Error::InvalidArtifact)?;
        if !header.shape_is_valid() || usize::from(header.entry_count.get()) > MAX_ENTRIES {
            return Err(Error::InvalidArtifact);
        }
        let mut builder = ManifestBuilder::new(header.parent_snapshot, Policy::default());
        let mut contents = BTreeMap::new();
        for expected in 0..header.entry_count.get() {
            let (entry_head, tail) = kore_workspace_sync::wire::EntryHeader::read_from_prefix(rest)
                .map_err(|_| Error::InvalidArtifact)?;
            rest = tail;
            if entry_head.index.get() != expected {
                return Err(Error::InvalidArtifact);
            }
            let path_len = usize::from(entry_head.path_len.get());
            let path = rest.get(..path_len).ok_or(Error::InvalidArtifact)?;
            rest = &rest[path_len..];
            let entry = Entry::from_wire(&entry_head, path)?;
            if entry.kind == EntryKind::File {
                let len = entry.content_len as usize;
                let content = rest.get(..len).ok_or(Error::InvalidArtifact)?.to_vec();
                rest = &rest[len..];
                if blake3::hash(&content).as_bytes() != &entry.digest {
                    return Err(Error::InvalidArtifact);
                }
                contents.insert(entry.path.clone(), content);
            }
            builder.push(entry)?;
        }
        if !rest.is_empty() {
            return Err(Error::InvalidArtifact);
        }
        let manifest = builder.finish()?;
        if manifest.header() != &header {
            return Err(Error::InvalidArtifact);
        }
        manifest.verify(Policy::default())?;
        Ok(Self { manifest, contents })
    }
}

fn secure_output_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

impl ImportSource for Artifact {
    type Error = Error;

    async fn read(
        &mut self,
        file: &StagedFile,
        offset: u32,
        destination: &mut [u8],
    ) -> Result<usize, Self::Error> {
        let content = self.content(&file.path).ok_or(Error::InvalidArtifact)?;
        let start = usize::try_from(offset).map_err(|_| Error::Bounds)?;
        let tail = content.get(start..).ok_or(Error::InvalidArtifact)?;
        let count = tail.len().min(destination.len());
        destination[..count].copy_from_slice(&tail[..count]);
        Ok(count)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Scanned {
    entry: Entry,
    content: Option<Vec<u8>>,
}

fn scan_tree(root: &Dir) -> Result<Vec<Scanned>, Error> {
    let mut scanned = Vec::new();
    let mut total_bytes = 0u64;
    scan_dir(root, None, &mut scanned, &mut total_bytes)?;
    Ok(scanned)
}

fn scan_dir(
    dir: &Dir,
    prefix: Option<&CanonicalPath>,
    out: &mut Vec<Scanned>,
    total_bytes: &mut u64,
) -> Result<(), Error> {
    let mut entries = dir.entries()?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_unstable_by_key(|entry| entry.file_name());
    for item in entries {
        if out.len() == MAX_ENTRIES {
            return Err(Error::Bounds);
        }
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| Error::NonUtf8Name)?;
        let joined = match prefix {
            Some(prefix) => format!("{}/{name}", prefix.as_str()),
            None => name,
        };
        let path = CanonicalPath::new(joined.as_bytes()).map_err(ManifestError::Path)?;
        let kind = item.file_type()?;
        if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
            return Err(Error::UnsupportedType(joined));
        }
        if kind.is_dir() {
            let child = item.open_dir()?;
            refuse_extended_metadata(&child, &joined)?;
            out.push(Scanned {
                entry: Entry::directory(path.clone(), source_id(&path, 0, &[0; 32])),
                content: None,
            });
            scan_dir(&child, Some(&path), out, total_bytes)?;
            continue;
        }
        let before = item.metadata()?;
        if before.len() > MAX_FILE_BYTES {
            return Err(Error::Bounds);
        }
        *total_bytes = total_bytes
            .checked_add(before.len())
            .filter(|total| *total <= MAX_SNAPSHOT_BYTES)
            .ok_or(Error::Bounds)?;
        refuse_file_shape(&before, &joined)?;
        let executable = executable(&before);
        let mut file = item.open()?;
        refuse_extended_metadata(&file, &joined)?;
        let mut content = Vec::with_capacity(before.len() as usize);
        file.read_to_end(&mut content)?;
        let after = item.metadata()?;
        if metadata_fingerprint(&before) != metadata_fingerprint(&after)
            || content.len() as u64 != before.len()
        {
            return Err(Error::SourceChanged(joined));
        }
        let digest = *blake3::hash(&content).as_bytes();
        // A metadata recheck catches replacement/truncation, while a second
        // read catches same-length writes with coarse timestamp granularity.
        // Accept only two identical complete observations.
        let mut verify = Vec::with_capacity(content.len());
        let mut second = item.open()?;
        second.read_to_end(&mut verify)?;
        let final_metadata = item.metadata()?;
        if verify != content
            || metadata_fingerprint(&after) != metadata_fingerprint(&final_metadata)
        {
            return Err(Error::SourceChanged(joined));
        }
        let range_root = kore_workspace_sync::range_root(&content).ok_or(Error::Bounds)?;
        let entry = Entry::file_with_range_root(
            path.clone(),
            executable,
            content.len() as u32,
            digest,
            range_root,
            source_id(&path, before.len(), &digest),
        )?;
        out.push(Scanned {
            entry,
            content: Some(content),
        });
    }
    Ok(())
}

fn open_parent(root: &Dir, path: &CanonicalPath) -> Result<(Dir, String), Error> {
    let mut parent = root.try_clone()?;
    let components = path.as_str().split('/').collect::<Vec<_>>();
    let (name, ancestors) = components.split_last().ok_or(Error::InvalidArtifact)?;
    for component in ancestors {
        parent = parent.open_dir(component)?;
    }
    Ok((parent, (*name).into()))
}

fn create_directory(root: &Dir, path: &CanonicalPath) -> Result<(), Error> {
    let (parent, name) = open_parent(root, path)?;
    parent.create_dir(&name)?;
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        let mut permissions = parent.metadata(&name)?.permissions();
        permissions.set_mode(0o700);
        parent.set_permissions(&name, permissions)?;
    }
    Ok(())
}

fn replace_file(
    root: &Dir,
    path: &CanonicalPath,
    content: &[u8],
    executable: bool,
    sequence: usize,
) -> Result<(), Error> {
    #[cfg(not(unix))]
    let _ = executable;
    let (parent, name) = open_parent(root, path)?;
    let temporary = format!(".kore-export-{sequence:04x}");
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = parent.open_with(&temporary, &options)?;
    file.write_all(content)?;
    #[cfg(unix)]
    {
        use cap_std::fs::PermissionsExt;
        let mut permissions = file.metadata()?.permissions();
        permissions.set_mode(if executable { 0o700 } else { 0o600 });
        file.set_permissions(permissions)?;
    }
    file.sync_all()?;
    drop(file);
    if parent.rename(&temporary, &parent, &name).is_err() {
        #[cfg(windows)]
        {
            let existing = parent
                .symlink_metadata(&name)
                .map_err(|_| Error::Conflict)?;
            if !existing.is_file() || existing.file_type().is_symlink() {
                let _ = parent.remove_file(&temporary);
                return Err(Error::Conflict);
            }
            let backup = format!(".kore-export-backup-{sequence:04x}");
            if parent.rename(&name, &parent, &backup).is_err() {
                let _ = parent.remove_file(&temporary);
                return Err(Error::Conflict);
            }
            if parent.rename(&temporary, &parent, &name).is_err() {
                let _ = parent.rename(&backup, &parent, &name);
                let _ = parent.remove_file(&temporary);
                return Err(Error::Conflict);
            }
            if parent.remove_file(&backup).is_err() {
                return Err(Error::Partial { applied: sequence });
            }
        }
        #[cfg(not(windows))]
        {
            let _ = parent.remove_file(&temporary);
            return Err(Error::Conflict);
        }
    }
    Ok(())
}

fn remove_entry(root: &Dir, path: &CanonicalPath) -> Result<(), Error> {
    let (parent, name) = open_parent(root, path)?;
    let metadata = parent.symlink_metadata(&name)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::Conflict);
    }
    if metadata.is_dir() {
        parent.remove_dir(&name)?;
    } else if metadata.is_file() {
        parent.remove_file(&name)?;
    } else {
        return Err(Error::Conflict);
    }
    Ok(())
}

fn rename_entry(root: &Dir, source: &CanonicalPath, target: &CanonicalPath) -> Result<(), Error> {
    let (source_parent, source_name) = open_parent(root, source)?;
    let (target_parent, target_name) = open_parent(root, target)?;
    if target_parent.symlink_metadata(&target_name).is_ok() {
        return Err(Error::Conflict);
    }
    source_parent.rename(&source_name, &target_parent, &target_name)?;
    Ok(())
}

fn source_id(path: &CanonicalPath, len: u64, digest: &[u8; 32]) -> [u8; 16] {
    let mut hash = blake3::Hasher::new();
    hash.update(SOURCE_DOMAIN);
    hash.update(path.as_bytes());
    hash.update(&len.to_le_bytes());
    hash.update(digest);
    let mut id = [0; 16];
    id.copy_from_slice(&hash.finalize().as_bytes()[..16]);
    id
}

#[cfg(unix)]
fn metadata_fingerprint(metadata: &cap_std::fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    use cap_std::fs::MetadataExt;
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

#[cfg(not(unix))]
fn metadata_fingerprint(
    metadata: &cap_std::fs::Metadata,
) -> (u64, Option<cap_std::time::SystemTime>) {
    (metadata.len(), metadata.modified().ok())
}

#[cfg(unix)]
fn refuse_file_shape(metadata: &cap_std::fs::Metadata, path: &str) -> Result<(), Error> {
    use cap_std::fs::MetadataExt;
    if metadata.nlink() != 1 {
        return Err(Error::HardLink(path.into()));
    }
    if metadata.len() != 0 && metadata.blocks().saturating_mul(512) < metadata.len() {
        return Err(Error::SparseFile(path.into()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn refuse_file_shape(_: &cap_std::fs::Metadata, _: &str) -> Result<(), Error> {
    Ok(())
}

#[cfg(unix)]
fn refuse_extended_metadata<F: std::os::fd::AsFd>(file: &F, path: &str) -> Result<(), Error> {
    let mut names = Vec::with_capacity(4096);
    let mut result = rustix::fs::flistxattr(file, rustix::buffer::spare_capacity(&mut names));
    #[cfg(target_os = "linux")]
    if result == Err(rustix::io::Errno::BADF) {
        // cap-std retains directories as O_PATH descriptors on Linux.
        let readable = rustix::fs::openat(
            file,
            ".",
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        );
        result = readable.and_then(|readable| {
            rustix::fs::flistxattr(&readable, rustix::buffer::spare_capacity(&mut names))
        });
    }
    match result {
        Ok(0) => Ok(()),
        Ok(_) if ignorable_host_xattrs(&names) => Ok(()),
        Ok(_) | Err(rustix::io::Errno::RANGE) => Err(Error::UnsupportedMetadata(path.into())),
        Err(error) if error == rustix::io::Errno::NOTSUP => Ok(()),
        Err(error) => Err(Error::Io(std::io::Error::from_raw_os_error(
            error.raw_os_error(),
        ))),
    }
}

#[cfg(target_os = "macos")]
fn ignorable_host_xattrs(names: &[u8]) -> bool {
    // Darwin attaches this non-user-authored process provenance marker to
    // newly created files. It is excluded (never represented), while every
    // user/security metadata xattr remains a deterministic refusal.
    names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .all(|name| name == b"com.apple.provenance")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn ignorable_host_xattrs(_: &[u8]) -> bool {
    false
}

#[cfg(not(unix))]
fn refuse_extended_metadata<F>(_: &F, _: &str) -> Result<(), Error> {
    Ok(())
}

#[cfg(unix)]
fn executable(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &cap_std::fs::Metadata) -> bool {
    false
}

pub fn snapshot_hex(id: &SnapshotId) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in id {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

pub fn parse_digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0; 32];
    for (at, byte) in digest.iter_mut().enumerate() {
        let high = text.as_bytes()[at * 2];
        let low = text.as_bytes()[at * 2 + 1];
        *byte = hex_nibble(high)?.checked_shl(4)? | hex_nibble(low)?;
    }
    Some(digest)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use core::{future::Future, pin::pin, task::Context};

    use super::*;
    use kore_workspace_sync::{BeginOutcome, CommitFailure, ImportStore, import_from};

    fn run<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(waker);
        for _ in 0..256 {
            if let core::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
        }
        panic!("ready-only artifact import parked")
    }

    #[derive(Default)]
    struct CaptureStore {
        files: BTreeMap<CanonicalPath, Vec<u8>>,
        aborted: bool,
    }

    impl ImportStore for CaptureStore {
        type Error = ();
        type Staging = ();
        type File = CanonicalPath;
        type Commit = ();

        async fn begin(&mut self, _: &SnapshotHeader) -> Result<BeginOutcome<(), ()>, ()> {
            Ok(BeginOutcome::Staging(()))
        }
        async fn create_directory(&mut self, _: &mut (), _: &Entry) -> Result<(), ()> {
            Ok(())
        }
        async fn begin_file(&mut self, _: &mut (), entry: &Entry) -> Result<Self::File, ()> {
            self.files.insert(entry.path.clone(), Vec::new());
            Ok(entry.path.clone())
        }
        async fn write_file(
            &mut self,
            _: &mut (),
            file: &mut Self::File,
            _: u32,
            bytes: &[u8],
        ) -> Result<(), ()> {
            self.files.get_mut(file).unwrap().extend_from_slice(bytes);
            Ok(())
        }
        async fn finish_file(&mut self, _: &mut (), _: Self::File, _: &Entry) -> Result<(), ()> {
            Ok(())
        }
        async fn commit(&mut self, _: (), _: &SnapshotHeader) -> Result<(), CommitFailure<(), ()>> {
            Ok(())
        }
        async fn abort(&mut self, _: ()) {
            self.aborted = true;
        }
    }

    #[test]
    fn artifact_round_trip_and_mutation_change_identity() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), b"one").unwrap();
        let first = Artifact::scan(root.path(), [0; 32]).unwrap();
        let artifact_dir = tempfile::tempdir().unwrap();
        let encoded = artifact_dir.path().join("snapshot.kws");
        first.write_new(&encoded).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&encoded).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let decoded = Artifact::read(&encoded).unwrap();
        assert_eq!(first.manifest.snapshot_id(), decoded.manifest.snapshot_id());

        std::fs::write(root.path().join("src/lib.rs"), b"two").unwrap();
        let second = Artifact::scan(root.path(), first.manifest.snapshot_id()).unwrap();
        assert_ne!(first.manifest.snapshot_id(), second.manifest.snapshot_id());
    }

    #[test]
    fn two_whole_tree_observations_refuse_a_mixed_snapshot() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("first"), b"before").unwrap();
        std::fs::write(source.path().join("second"), b"stable").unwrap();
        let root = Dir::open_ambient_dir(source.path(), ambient_authority()).unwrap();
        let first = scan_tree(&root).unwrap();
        std::fs::write(source.path().join("first"), b"after").unwrap();
        let second = scan_tree(&root).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn offline_artifact_uses_the_transport_neutral_import_transaction() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("payload"), vec![0x5a; 9_000]).unwrap();
        let mut artifact = Artifact::scan(root.path(), [0; 32]).unwrap();
        let manifest = artifact.manifest().clone();
        let (store, ()) = run(import_from(
            CaptureStore::default(),
            manifest,
            Policy::default(),
            &mut artifact,
        ))
        .unwrap();
        assert_eq!(
            store.files[&CanonicalPath::new(b"payload").unwrap()],
            vec![0x5a; 9_000]
        );
        assert!(!store.aborted);
    }

    #[test]
    fn symlink_is_refused_without_following_it() {
        #[cfg(unix)]
        {
            let root = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink("/", root.path().join("escape")).unwrap();
            assert!(matches!(
                Artifact::scan(root.path(), [0; 32]),
                Err(Error::UnsupportedType(_))
            ));
        }
    }

    #[test]
    fn malformed_truncated_and_overfull_artifacts_are_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), b"bytes").unwrap();
        let artifact = Artifact::scan(root.path(), [0; 32]).unwrap();
        let output = tempfile::tempdir().unwrap();
        let valid = output.path().join("valid.kws");
        artifact.write_new(&valid).unwrap();
        let bytes = std::fs::read(&valid).unwrap();

        let malformed = output.path().join("malformed.kws");
        let mut changed = bytes.clone();
        // SnapshotHeader.reserved begins at byte 12 after the artifact magic.
        changed[8 + 12] = 1;
        std::fs::write(&malformed, changed).unwrap();
        assert!(matches!(
            Artifact::read(&malformed),
            Err(Error::InvalidArtifact)
        ));

        let truncated = output.path().join("truncated.kws");
        std::fs::write(&truncated, &bytes[..bytes.len() - 1]).unwrap();
        assert!(matches!(
            Artifact::read(&truncated),
            Err(Error::InvalidArtifact)
        ));

        let overfull = tempfile::tempdir().unwrap();
        for index in 0..=MAX_ENTRIES {
            std::fs::write(overfull.path().join(format!("f{index:02}")), b"").unwrap();
        }
        assert!(matches!(
            Artifact::scan(overfull.path(), [0; 32]),
            Err(Error::Bounds)
        ));
    }

    #[test]
    fn hardlinks_are_refused() {
        #[cfg(unix)]
        {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("first"), b"same inode").unwrap();
            std::fs::hard_link(root.path().join("first"), root.path().join("second")).unwrap();
            assert!(matches!(
                Artifact::scan(root.path(), [0; 32]),
                Err(Error::HardLink(_))
            ));
        }
    }

    #[test]
    fn approved_export_is_preconditioned_atomic_per_object_and_retry_safe() {
        let base_root = tempfile::tempdir().unwrap();
        std::fs::create_dir(base_root.path().join("old")).unwrap();
        std::fs::write(base_root.path().join("edit.txt"), b"before").unwrap();
        std::fs::write(base_root.path().join("rename.txt"), b"same").unwrap();
        std::fs::write(base_root.path().join("old/remove.txt"), b"gone").unwrap();
        let base = Artifact::scan(base_root.path(), [0; 32]).unwrap();

        let desired_root = tempfile::tempdir().unwrap();
        std::fs::create_dir(desired_root.path().join("new")).unwrap();
        std::fs::write(desired_root.path().join("edit.txt"), b"after").unwrap();
        std::fs::write(desired_root.path().join("moved.txt"), b"same").unwrap();
        std::fs::write(desired_root.path().join("new/create.txt"), b"created").unwrap();
        let desired = Artifact::scan(desired_root.path(), base.manifest.snapshot_id()).unwrap();
        let plan = ExportPlan::between(&base, &desired).unwrap();
        assert!(plan.changes().iter().any(|change| {
            change.kind == ChangeKind::Rename
                && change.path.as_str() == "rename.txt"
                && change.target.as_ref().map(CanonicalPath::as_str) == Some("moved.txt")
        }));

        let applied_root = tempfile::tempdir().unwrap();
        std::fs::create_dir(applied_root.path().join("old")).unwrap();
        std::fs::write(applied_root.path().join("edit.txt"), b"before").unwrap();
        std::fs::write(applied_root.path().join("rename.txt"), b"same").unwrap();
        std::fs::write(applied_root.path().join("old/remove.txt"), b"gone").unwrap();
        let result_dir = tempfile::tempdir().unwrap();
        let result = result_dir.path().join("export.kwr");
        let first = plan
            .apply(applied_root.path(), &base, &desired, plan.id(), &result)
            .unwrap();
        assert!(first.complete);
        assert_eq!(first.snapshot, desired.manifest.snapshot_id());
        let second = plan
            .apply(applied_root.path(), &base, &desired, plan.id(), &result)
            .unwrap();
        assert_eq!(second, first, "retry returns the recorded result");
    }

    #[test]
    fn export_refuses_wrong_approval_and_changed_base_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), b"base").unwrap();
        let base = Artifact::scan(root.path(), [0; 32]).unwrap();
        let desired_root = tempfile::tempdir().unwrap();
        std::fs::write(desired_root.path().join("file"), b"desired").unwrap();
        let desired = Artifact::scan(desired_root.path(), base.manifest.snapshot_id()).unwrap();
        let plan = ExportPlan::between(&base, &desired).unwrap();
        let result_dir = tempfile::tempdir().unwrap();
        let result = result_dir.path().join("result.kwr");
        assert!(matches!(
            plan.apply(root.path(), &base, &desired, [9; 32], &result),
            Err(Error::ApprovalMismatch)
        ));
        assert_eq!(std::fs::read(root.path().join("file")).unwrap(), b"base");

        std::fs::write(root.path().join("file"), b"concurrent").unwrap();
        assert!(matches!(
            plan.apply(root.path(), &base, &desired, plan.id(), &result),
            Err(Error::Conflict)
        ));
        assert_eq!(
            std::fs::read(root.path().join("file")).unwrap(),
            b"concurrent"
        );
    }
}
