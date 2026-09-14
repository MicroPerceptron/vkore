//! Portable, capability-rooted live workspace adapter.
//!
//! The ambient host pathname is consumed exactly once by `open`. Every later
//! operation is relative to the retained `cap_std::fs::Dir`. Watcher events are
//! hints only; `rescan` reconstructs and validates the complete canonical tree.

use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::mpsc::{Receiver, TryRecvError, channel},
};

use cap_std::{
    ambient_authority,
    fs::{Dir, File, OpenOptions},
};
use kore_workspace_sync::{
    CanonicalPath, ChangeKind, ChangeSet, ChangeSetBuilder, DeltaOp, EntryKind, Manifest, Policy,
    RangeProof, RangeRequestV1, SourceCorrelation, StandingGrant, TopologyDelta,
    TransactionOutcome, bounds::MAX_TRANSACTION_RESULTS, build_range_proof, range_root,
};

use crate::{
    Artifact, Error, ExportPlan, create_directory, metadata_fingerprint, refuse_extended_metadata,
    refuse_file_shape, remove_entry, rename_entry, replace_file, verify_precondition,
};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

#[derive(Debug)]
pub struct ServedRange {
    pub bytes: Vec<u8>,
    pub proof: RangeProof,
}

pub struct LiveRoot {
    root: Dir,
    selected: Option<Vec<SelectedFile>>,
    manifest: Manifest,
    sources: BTreeMap<SourceCorrelation, CanonicalPath>,
    next_delta_sequence: u64,
    results: Vec<ApplyRecord>,
    journal: Option<TransactionJournal>,
}

#[derive(Clone, Copy)]
struct ApplyRecord {
    transaction: [u8; 32],
    changes: [u8; 32],
    outcome: TransactionOutcome,
}

const JOURNAL_NAME: &str = "workspace-transactions.v1";
const JOURNAL_MAGIC: [u8; 4] = *b"KWTJ";
const JOURNAL_VERSION: u16 = 1;
const JOURNAL_RECORD_BYTES: usize = 140;

struct TransactionJournal {
    file: File,
}

#[derive(Clone, Copy)]
enum JournalOutcome {
    Prepared,
    Terminal(TransactionOutcome),
}

impl TransactionJournal {
    fn open(state: &Path) -> Result<(Self, Vec<ApplyRecord>), Error> {
        let state = Dir::open_ambient_dir(state, ambient_authority())?;
        refuse_extended_metadata(&state, ".")?;
        let mut options = OpenOptions::new();
        options.read(true).append(true).create(true);
        let mut file = state.open_with(JOURNAL_NAME, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::InvalidArtifact);
        }
        if metadata.len() % JOURNAL_RECORD_BYTES as u64 != 0 {
            return Err(Error::InvalidArtifact);
        }
        // The guest's bounded result cache is not a host lifetime quota.
        // Validate the append-only history with fixed-size reads and retain
        // only recent identities; older retries are resolved from disk.
        let mut recent = Vec::with_capacity(MAX_TRANSACTION_RESULTS);
        for _ in 0..metadata.len() / JOURNAL_RECORD_BYTES as u64 {
            let mut bytes = [0; JOURNAL_RECORD_BYTES];
            file.read_exact(&mut bytes)?;
            let (transaction, _, _) = decode_journal_record(&bytes)?;
            if let Some(index) = recent.iter().position(|id| *id == transaction) {
                recent.remove(index);
            } else if recent.len() == MAX_TRANSACTION_RESULTS {
                recent.remove(0);
            }
            recent.push(transaction);
        }
        let mut journal = Self { file };
        let mut records = Vec::with_capacity(MAX_TRANSACTION_RESULTS);
        for transaction in recent {
            records.push(journal.lookup(transaction)?.ok_or(Error::InvalidArtifact)?);
        }
        Ok((journal, records))
    }

    fn lookup(&mut self, transaction: [u8; 32]) -> Result<Option<ApplyRecord>, Error> {
        let length = self.file.metadata()?.len();
        if length % JOURNAL_RECORD_BYTES as u64 != 0 {
            return Err(Error::InvalidArtifact);
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut found: Option<([u8; 32], Option<TransactionOutcome>)> = None;
        for _ in 0..length / JOURNAL_RECORD_BYTES as u64 {
            let mut bytes = [0; JOURNAL_RECORD_BYTES];
            self.file.read_exact(&mut bytes)?;
            let (id, changes, outcome) = decode_journal_record(&bytes)?;
            if id != transaction {
                continue;
            }
            match outcome {
                JournalOutcome::Prepared if found.is_none() => found = Some((changes, None)),
                JournalOutcome::Terminal(outcome)
                    if found.is_none_or(|(expected, terminal)| {
                        expected == changes && terminal.is_none()
                    }) =>
                {
                    found = Some((changes, Some(outcome)));
                }
                _ => return Err(Error::InvalidArtifact),
            }
        }
        Ok(found.map(|(changes, outcome)| ApplyRecord {
            transaction,
            changes,
            // An interrupted prepare never becomes an inferred success.
            outcome: outcome.unwrap_or(TransactionOutcome::Partial { applied: 0 }),
        }))
    }

    fn append(
        &mut self,
        transaction: [u8; 32],
        changes: [u8; 32],
        outcome: JournalOutcome,
    ) -> Result<(), Error> {
        let record = encode_journal_record(transaction, changes, outcome);
        self.file.write_all(&record)?;
        self.file.sync_all()?;
        Ok(())
    }
}

fn encode_journal_record(
    transaction: [u8; 32],
    changes: [u8; 32],
    outcome: JournalOutcome,
) -> [u8; JOURNAL_RECORD_BYTES] {
    let mut record = [0; JOURNAL_RECORD_BYTES];
    record[..4].copy_from_slice(&JOURNAL_MAGIC);
    record[4..6].copy_from_slice(&JOURNAL_VERSION.to_le_bytes());
    record[8..40].copy_from_slice(&transaction);
    record[40..72].copy_from_slice(&changes);
    let (tag, applied, snapshot) = match outcome {
        JournalOutcome::Prepared => (0, 0, [0; 32]),
        JournalOutcome::Terminal(TransactionOutcome::Applied { snapshot }) => (1, 0, snapshot),
        JournalOutcome::Terminal(TransactionOutcome::Conflict) => (2, 0, [0; 32]),
        JournalOutcome::Terminal(TransactionOutcome::Partial { applied }) => (3, applied, [0; 32]),
        JournalOutcome::Terminal(TransactionOutcome::Refused) => (4, 0, [0; 32]),
    };
    record[72] = tag;
    record[74..76].copy_from_slice(&applied.to_le_bytes());
    record[76..108].copy_from_slice(&snapshot);
    let digest = blake3::hash(&record[..108]);
    record[108..140].copy_from_slice(digest.as_bytes());
    record
}

fn decode_journal_record(record: &[u8]) -> Result<([u8; 32], [u8; 32], JournalOutcome), Error> {
    if record.len() != JOURNAL_RECORD_BYTES
        || record[..4] != JOURNAL_MAGIC
        || u16::from_le_bytes(
            record[4..6]
                .try_into()
                .map_err(|_| Error::InvalidArtifact)?,
        ) != JOURNAL_VERSION
        || record[6..8] != [0; 2]
        || record[73] != 0
        || blake3::hash(&record[..108]).as_bytes() != &record[108..140]
    {
        return Err(Error::InvalidArtifact);
    }
    let transaction = record[8..40]
        .try_into()
        .map_err(|_| Error::InvalidArtifact)?;
    let changes = record[40..72]
        .try_into()
        .map_err(|_| Error::InvalidArtifact)?;
    if transaction == [0; 32] || changes == [0; 32] {
        return Err(Error::InvalidArtifact);
    }
    let applied = u16::from_le_bytes(
        record[74..76]
            .try_into()
            .map_err(|_| Error::InvalidArtifact)?,
    );
    let snapshot: [u8; 32] = record[76..108]
        .try_into()
        .map_err(|_| Error::InvalidArtifact)?;
    let outcome = match record[72] {
        0 if applied == 0 && snapshot == [0; 32] => JournalOutcome::Prepared,
        1 if applied == 0 && snapshot != [0; 32] => {
            JournalOutcome::Terminal(TransactionOutcome::Applied { snapshot })
        }
        2 if applied == 0 && snapshot == [0; 32] => {
            JournalOutcome::Terminal(TransactionOutcome::Conflict)
        }
        3 if snapshot == [0; 32] => {
            JournalOutcome::Terminal(TransactionOutcome::Partial { applied })
        }
        4 if applied == 0 && snapshot == [0; 32] => {
            JournalOutcome::Terminal(TransactionOutcome::Refused)
        }
        _ => return Err(Error::InvalidArtifact),
    };
    Ok((transaction, changes, outcome))
}

/// Cross-platform watcher whose output is deliberately collapsed to a hint.
/// The event's host paths never enter the semantic protocol; callers invoke
/// `LiveRoot::rescan` to obtain one validated canonical transaction.
pub struct WatchHints {
    _watcher: RecommendedWatcher,
    events: Receiver<notify::Result<Event>>,
}

impl WatchHints {
    pub fn start(path: &Path) -> Result<Self, Error> {
        let (send, events) = channel();
        let mut watcher =
            notify::recommended_watcher(send).map_err(|error| Error::Watch(error.to_string()))?;
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|error| Error::Watch(error.to_string()))?;
        Ok(Self {
            _watcher: watcher,
            events,
        })
    }

    pub fn poll(&self) -> Result<bool, Error> {
        let mut changed = false;
        loop {
            match self.events.try_recv() {
                Ok(Ok(_)) => changed = true,
                Ok(Err(error)) => return Err(Error::Watch(error.to_string())),
                Err(TryRecvError::Empty) => return Ok(changed),
                Err(TryRecvError::Disconnected) => {
                    return Err(Error::Watch("watcher disconnected".into()));
                }
            }
        }
    }
}

/// A text representation of one explicitly authorized backing file. The open
/// directory capability and digest bind every range read to that same source.
pub struct SelectedFile {
    directory: Dir,
    name: std::path::PathBuf,
    pub alias: CanonicalPath,
    pub backing_path: std::path::PathBuf,
    original_digest: [u8; 32],
    bytes: Vec<u8>,
}

impl SelectedFile {
    pub fn new(
        directory: Dir,
        name: std::path::PathBuf,
        alias: CanonicalPath,
        backing_path: std::path::PathBuf,
        original_digest: [u8; 32],
        bytes: Vec<u8>,
    ) -> Result<Self, Error> {
        if bytes.len() as u64 > kore_workspace_sync::bounds::MAX_FILE_BYTES {
            return Err(Error::Bounds);
        }
        let selected = Self {
            directory,
            name,
            alias,
            backing_path,
            original_digest,
            bytes,
        };
        selected.verify()?;
        Ok(selected)
    }

    fn verify(&self) -> Result<(), Error> {
        let before = self.directory.symlink_metadata(&self.name)?;
        if !before.is_file() || before.file_type().is_symlink() || before.len() > 32 * 1024 * 1024 {
            return Err(Error::InvalidArtifact);
        }
        let mut file = self.directory.open(&self.name)?;
        if metadata_fingerprint(&before) != metadata_fingerprint(&file.metadata()?) {
            return Err(Error::Conflict);
        }
        let mut hash = blake3::Hasher::new();
        let mut buffer = [0u8; 8192];
        let mut remaining = before.len();
        while remaining > 0 {
            let take = remaining.min(buffer.len() as u64) as usize;
            let read = file.read(&mut buffer[..take])?;
            if read == 0 {
                return Err(Error::Conflict);
            }
            hash.update(&buffer[..read]);
            remaining -= read as u64;
        }
        let after = self.directory.symlink_metadata(&self.name)?;
        if metadata_fingerprint(&before) != metadata_fingerprint(&after)
            || hash.finalize().as_bytes() != &self.original_digest
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

impl LiveRoot {
    pub fn open(path: &Path, parent: [u8; 32]) -> Result<Self, Error> {
        Self::open_inner(path, parent, None)
    }

    /// Open a live root with a separately selected durable transaction state
    /// directory. The state directory is never part of the exported namespace.
    pub fn open_with_state(path: &Path, parent: [u8; 32], state: &Path) -> Result<Self, Error> {
        Self::open_inner(path, parent, Some(state))
    }

    fn open_inner(path: &Path, parent: [u8; 32], state: Option<&Path>) -> Result<Self, Error> {
        let root = Dir::open_ambient_dir(path, ambient_authority())?;
        refuse_extended_metadata(&root, ".")?;
        let artifact = Artifact::scan_cap(&root, parent)?;
        let sources = source_map(artifact.manifest())?;
        let (journal, results) = match state {
            Some(state) => {
                let (journal, results) = TransactionJournal::open(state)?;
                (Some(journal), results)
            }
            None => (None, Vec::with_capacity(MAX_TRANSACTION_RESULTS)),
        };
        Ok(Self {
            root,
            selected: None,
            manifest: artifact.manifest,
            sources,
            next_delta_sequence: 1,
            results,
            journal,
        })
    }

    /// Sparse, read-only virtual topology. Files remain at their authorized
    /// host locations; aliases identify bounded text views and not host paths.
    pub fn open_selected(empty_root: &Path, selected: Vec<SelectedFile>) -> Result<Self, Error> {
        use kore_workspace_sync::{Entry, ManifestBuilder};
        if selected.len() > kore_workspace_sync::bounds::MAX_ENTRIES {
            return Err(Error::Bounds);
        }
        let mut root = Self::open(empty_root, [0; 32])?;
        let mut builder = ManifestBuilder::new([0; 32], Policy::default());
        for file in &selected {
            file.verify()?;
            let digest = *blake3::hash(&file.bytes).as_bytes();
            let mut source = blake3::Hasher::new();
            source.update(file.backing_path.as_os_str().as_encoded_bytes());
            source.update(&file.original_digest);
            let mut identity = [0u8; 16];
            identity.copy_from_slice(&source.finalize().as_bytes()[..16]);
            builder.push(Entry::file_with_range_root(
                file.alias.clone(),
                false,
                file.bytes.len() as u32,
                digest,
                range_root(&file.bytes).ok_or(Error::Bounds)?,
                identity,
            )?)?;
        }
        root.manifest = builder.finish()?;
        root.sources = source_map(&root.manifest)?;
        root.selected = Some(selected);
        Ok(root)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    #[must_use]
    pub const fn next_delta_sequence(&self) -> u64 {
        self.next_delta_sequence
    }

    pub fn read_range(&self, request: &RangeRequestV1) -> Result<ServedRange, Error> {
        request.validate().map_err(|_| Error::InvalidArtifact)?;
        if request.snapshot != self.manifest.snapshot_id() {
            return Err(Error::Conflict);
        }
        let path = self.sources.get(&request.source).ok_or(Error::Conflict)?;
        let entry = self
            .manifest
            .entries()
            .iter()
            .find(|entry| entry.path == *path && entry.kind == EntryKind::File)
            .ok_or(Error::Conflict)?;
        if entry.digest != request.version_digest
            || entry.range_root != request.range_root
            || entry.content_len != request.file_len.get()
        {
            return Err(Error::Conflict);
        }
        if let Some(selected) = &self.selected {
            let file = selected
                .iter()
                .find(|file| file.alias == *path)
                .ok_or(Error::Conflict)?;
            file.verify()?;
            let leaf =
                (request.offset.get() / kore_workspace_sync::bounds::RANGE_BYTES as u32) as u16;
            let proof = build_range_proof(&file.bytes, leaf).ok_or(Error::Bounds)?;
            let start = usize::from(leaf) * kore_workspace_sync::bounds::RANGE_BYTES;
            let end = (start + kore_workspace_sync::bounds::RANGE_BYTES).min(file.bytes.len());
            return Ok(ServedRange {
                bytes: file.bytes.get(start..end).ok_or(Error::Bounds)?.to_vec(),
                proof,
            });
        }
        let before = self.root.symlink_metadata(path.as_str())?;
        if !before.is_file()
            || before.file_type().is_symlink()
            || before.len() != u64::from(entry.content_len)
        {
            return Err(Error::Conflict);
        }
        refuse_file_shape(&before, path.as_str())?;
        let mut file = self.root.open(path.as_str())?;
        refuse_extended_metadata(&file, path.as_str())?;
        let mut content = Vec::with_capacity(entry.content_len as usize);
        file.read_to_end(&mut content)?;
        let after = self.root.symlink_metadata(path.as_str())?;
        if metadata_fingerprint(&before) != metadata_fingerprint(&after)
            || content.len() != entry.content_len as usize
            || blake3::hash(&content).as_bytes() != &entry.digest
            || range_root(&content) != Some(entry.range_root)
        {
            return Err(Error::SourceChanged(path.as_str().into()));
        }
        let leaf_index =
            (request.offset.get() / kore_workspace_sync::bounds::RANGE_BYTES as u32) as u16;
        let proof = build_range_proof(&content, leaf_index).ok_or(Error::Bounds)?;
        let start = usize::from(leaf_index) * kore_workspace_sync::bounds::RANGE_BYTES;
        let end = start
            .checked_add(kore_workspace_sync::bounds::RANGE_BYTES)
            .map(|end| end.min(content.len()))
            .ok_or(Error::Bounds)?;
        Ok(ServedRange {
            bytes: content[start..end].to_vec(),
            proof,
        })
    }

    /// Apply one kore-authored copy-on-write transaction beneath the retained
    /// capability root. Every precondition and standing-grant rule is checked
    /// before the first mutation. Host files are replaced atomically one by
    /// one; platforms without transaction-wide atomicity report `Partial`.
    pub fn apply_transaction(
        &mut self,
        transaction: [u8; 32],
        changes: &ChangeSet,
        contents: &BTreeMap<CanonicalPath, Vec<u8>>,
        grant: &mut StandingGrant,
        now: u64,
    ) -> Result<TransactionOutcome, Error> {
        if self.selected.is_some() {
            return Ok(TransactionOutcome::Refused);
        }
        if transaction == [0; 32] || changes.verify().is_err() {
            return Err(Error::InvalidArtifact);
        }
        if let Some(record) = self
            .results
            .iter()
            .find(|record| record.transaction == transaction)
        {
            return if record.changes == changes.id {
                Ok(record.outcome)
            } else {
                Err(Error::ApprovalMismatch)
            };
        }
        if let Some(journal) = self.journal.as_mut() {
            if let Some(record) = journal.lookup(transaction)? {
                return if record.changes == changes.id {
                    Ok(record.outcome)
                } else {
                    Err(Error::ApprovalMismatch)
                };
            }
        } else if self.results.len() == MAX_TRANSACTION_RESULTS {
            // Without durable history, refuse BEFORE the first host effect.
            return Err(Error::Bounds);
        }
        if grant.authorize(changes, now).is_err() {
            return self.remember(transaction, changes.id, TransactionOutcome::Refused);
        }
        if changes.base != self.manifest.snapshot_id() {
            return self.remember(transaction, changes.id, TransactionOutcome::Conflict);
        }
        for change in changes.changes() {
            if matches!(change.kind, ChangeKind::CreateFile | ChangeKind::ModifyFile) {
                let Some(content) = contents.get(&change.path) else {
                    return self.remember(transaction, changes.id, TransactionOutcome::Refused);
                };
                if content.len() != change.replacement_len as usize
                    || blake3::hash(content).as_bytes() != &change.replacement
                {
                    return self.remember(transaction, changes.id, TransactionOutcome::Refused);
                }
            }
        }
        let base = Artifact {
            manifest: self.manifest.clone(),
            contents: BTreeMap::new(),
        };
        // Revalidate the complete affected set before the first effect. This
        // prevents an early operation from obscuring a later stale base.
        if changes
            .changes()
            .iter()
            .any(|change| verify_precondition(&self.root, change, &base).is_err())
        {
            return self.remember(transaction, changes.id, TransactionOutcome::Conflict);
        }

        // A normal write can finish with Resize(current_length), or replace
        // bytes with identical bytes. Those are authorized, preconditioned
        // operations, but they produce no content diff. Compare the observed
        // tree against only their effective changes, while journaling and
        // acknowledging the original exact transaction. Otherwise a no-op
        // is incorrectly reported as a partial write after reaching the host.
        let mut effective = ChangeSetBuilder::new(changes.base);
        for change in changes.changes() {
            let unchanged = change.kind == ChangeKind::ModifyFile
                && change.expected == change.replacement
                && self.manifest.entries().iter().any(|entry| {
                    entry.path == change.path
                        && entry.kind == EntryKind::File
                        && entry.content_len == change.replacement_len
                        && entry.flags.executable() == change.executable
                });
            if !unchanged {
                effective
                    .push(change.clone())
                    .map_err(|_| Error::InvalidArtifact)?;
            }
        }
        let effective = effective.finish().map_err(|_| Error::InvalidArtifact)?;

        if let Some(journal) = self.journal.as_mut() {
            journal.append(transaction, changes.id, JournalOutcome::Prepared)?;
        }

        let mut applied = 0u16;
        for change in changes
            .changes()
            .iter()
            .filter(|change| change.kind == ChangeKind::CreateDirectory)
        {
            if create_directory(&self.root, &change.path).is_err() {
                return self.remember(
                    transaction,
                    changes.id,
                    TransactionOutcome::Partial { applied },
                );
            }
            applied = applied.checked_add(1).ok_or(Error::Bounds)?;
        }
        for change in changes
            .changes()
            .iter()
            .filter(|change| change.kind == ChangeKind::Rename)
        {
            let target = change.target.as_ref().ok_or(Error::InvalidArtifact)?;
            if rename_entry(&self.root, &change.path, target).is_err() {
                return self.remember(
                    transaction,
                    changes.id,
                    TransactionOutcome::Partial { applied },
                );
            }
            applied = applied.checked_add(1).ok_or(Error::Bounds)?;
        }
        for change in changes
            .changes()
            .iter()
            .filter(|change| matches!(change.kind, ChangeKind::CreateFile | ChangeKind::ModifyFile))
        {
            let content = contents.get(&change.path).ok_or(Error::InvalidArtifact)?;
            if replace_file(
                &self.root,
                &change.path,
                content,
                change.executable,
                usize::from(applied),
            )
            .is_err()
            {
                return self.remember(
                    transaction,
                    changes.id,
                    TransactionOutcome::Partial { applied },
                );
            }
            applied = applied.checked_add(1).ok_or(Error::Bounds)?;
        }
        for change in changes
            .changes()
            .iter()
            .rev()
            .filter(|change| change.kind == ChangeKind::Delete)
        {
            if remove_entry(&self.root, &change.path).is_err() {
                return self.remember(
                    transaction,
                    changes.id,
                    TransactionOutcome::Partial { applied },
                );
            }
            applied = applied.checked_add(1).ok_or(Error::Bounds)?;
        }

        let observed = Artifact::scan_cap(&self.root, changes.base)?;
        let observed_plan = ExportPlan::between(&base, &observed)?;
        if observed_plan.id() != effective.id {
            return self.remember(
                transaction,
                changes.id,
                TransactionOutcome::Partial { applied },
            );
        }
        let next = observed.manifest.snapshot_id();
        grant
            .advance(changes.base, next)
            .map_err(|_| Error::ApprovalMismatch)?;
        self.sources = source_map(observed.manifest())?;
        self.manifest = observed.manifest;
        self.remember(
            transaction,
            changes.id,
            TransactionOutcome::Applied { snapshot: next },
        )
    }

    fn remember(
        &mut self,
        transaction: [u8; 32],
        changes: [u8; 32],
        outcome: TransactionOutcome,
    ) -> Result<TransactionOutcome, Error> {
        if self.journal.is_none() && self.results.len() == MAX_TRANSACTION_RESULTS {
            return Err(Error::Bounds);
        }
        if let Some(journal) = self.journal.as_mut() {
            journal.append(transaction, changes, JournalOutcome::Terminal(outcome))?;
        }
        if self.results.len() == MAX_TRANSACTION_RESULTS {
            self.results.remove(0);
        }
        self.results.push(ApplyRecord {
            transaction,
            changes,
            outcome,
        });
        Ok(outcome)
    }

    /// Securely rescan after any watcher hint and produce one canonical,
    /// parent-bound delta. Returning `None` means the hint was spurious.
    pub fn rescan(&mut self, origin_transaction: [u8; 32]) -> Result<Option<TopologyDelta>, Error> {
        if let Some(selected) = &self.selected {
            for file in selected {
                file.verify()?;
            }
            return Ok(None);
        }
        let desired = Artifact::scan_cap(&self.root, self.manifest.snapshot_id())?;
        // The second scan's parent makes its identity advance even when the
        // tree is unchanged; compare the committed root first.
        if desired.manifest.header().root_digest == self.manifest.header().root_digest {
            return Ok(None);
        }
        let base_artifact = Artifact {
            manifest: self.manifest.clone(),
            contents: BTreeMap::new(),
        };
        let plan = ExportPlan::between(&base_artifact, &desired)?;
        let mut ops = Vec::with_capacity(plan.changes().len());
        for change in plan.changes() {
            let replacement = match change.kind {
                kore_workspace_sync::ChangeKind::CreateDirectory
                | kore_workspace_sync::ChangeKind::CreateFile
                | kore_workspace_sync::ChangeKind::ModifyFile => desired
                    .manifest
                    .entries()
                    .iter()
                    .find(|entry| entry.path == change.path)
                    .cloned(),
                kore_workspace_sync::ChangeKind::Delete
                | kore_workspace_sync::ChangeKind::Rename => None,
            };
            ops.push(DeltaOp {
                change: change.clone(),
                replacement,
            });
        }
        let changes = plan.changes.clone();
        let delta = TopologyDelta::new(
            self.next_delta_sequence,
            &changes,
            desired.manifest.snapshot_id(),
            origin_transaction,
            &ops,
        )
        .map_err(|_| Error::InvalidArtifact)?;
        self.next_delta_sequence = self
            .next_delta_sequence
            .checked_add(1)
            .ok_or(Error::Bounds)?;
        self.sources = source_map(desired.manifest())?;
        self.manifest = desired.manifest;
        Ok(Some(delta))
    }
}

fn source_map(manifest: &Manifest) -> Result<BTreeMap<SourceCorrelation, CanonicalPath>, Error> {
    manifest.verify(Policy::default())?;
    let mut sources = BTreeMap::new();
    for entry in manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        if entry.source == [0; 16] || sources.insert(entry.source, entry.path.clone()).is_some() {
            return Err(Error::InvalidArtifact);
        }
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kore_workspace_sync::{Change, ChangeSetBuilder, GrantOperations, RANGE_REQUEST_MAGIC};

    #[test]
    fn durable_history_outlives_the_cache_and_replays_old_results_after_restart() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), b"initial").unwrap();
        let mut live = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        let mut grant = StandingGrant::new(
            [1; 16],
            live.manifest().snapshot_id(),
            GrantOperations::ALL,
            4,
            4096,
            0,
            &[],
        )
        .unwrap();
        let mut first = None;
        for nonce in 1..=32u8 {
            let content = format!("value-{nonce}").into_bytes();
            let entry = live.manifest().entries()[0].clone();
            let mut builder = ChangeSetBuilder::new(live.manifest().snapshot_id());
            builder
                .push(Change {
                    kind: ChangeKind::ModifyFile,
                    path: entry.path.clone(),
                    target: None,
                    expected: entry.digest,
                    replacement: *blake3::hash(&content).as_bytes(),
                    replacement_len: content.len() as u32,
                    executable: false,
                })
                .unwrap();
            let changes = builder.finish().unwrap();
            let contents = BTreeMap::from([(entry.path, content)]);
            let outcome = live
                .apply_transaction([nonce; 32], &changes, &contents, &mut grant, 1)
                .unwrap();
            assert!(matches!(outcome, TransactionOutcome::Applied { .. }));
            assert!(live.results.len() <= MAX_TRANSACTION_RESULTS);
            if first.is_none() {
                first = Some((changes, contents, outcome));
            }
        }
        let (changes, contents, expected) = first.unwrap();
        assert_eq!(
            live.apply_transaction([1; 32], &changes, &contents, &mut grant, 1)
                .unwrap(),
            expected
        );
        drop(live);
        let mut reopened = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        assert!(reopened.results.len() <= MAX_TRANSACTION_RESULTS);
        assert_eq!(
            reopened
                .apply_transaction([1; 32], &changes, &contents, &mut grant, 1)
                .unwrap(),
            expected
        );
        assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"value-32");
    }

    #[test]
    fn identical_replacement_is_applied_and_does_not_poison_the_next_write() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), b"same").unwrap();
        let mut live = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        let mut grant = StandingGrant::new(
            [1; 16],
            live.manifest().snapshot_id(),
            GrantOperations::ALL,
            4,
            4096,
            0,
            &[],
        )
        .unwrap();
        for (nonce, replacement) in [
            (1, b"same".as_slice()),
            (2, b"same".as_slice()),
            (3, b"changed".as_slice()),
        ] {
            let entry = live.manifest().entries()[0].clone();
            let mut builder = ChangeSetBuilder::new(live.manifest().snapshot_id());
            builder
                .push(Change {
                    kind: ChangeKind::ModifyFile,
                    path: entry.path.clone(),
                    target: None,
                    expected: entry.digest,
                    replacement: *blake3::hash(replacement).as_bytes(),
                    replacement_len: replacement.len() as u32,
                    executable: false,
                })
                .unwrap();
            let changes = builder.finish().unwrap();
            let contents = BTreeMap::from([(entry.path, replacement.to_vec())]);
            let outcome = live
                .apply_transaction([nonce; 32], &changes, &contents, &mut grant, 1)
                .unwrap();
            assert!(
                matches!(outcome, TransactionOutcome::Applied { .. }),
                "{outcome:?}"
            );
            assert_eq!(std::fs::read(root.path().join("a")).unwrap(), replacement);
        }
    }
    use zerocopy::byteorder::little_endian::{U16, U32};

    fn request(root: &LiveRoot, path: &str, offset: u32, length: u16) -> RangeRequestV1 {
        let entry = root
            .manifest()
            .entries()
            .iter()
            .find(|entry| entry.path.as_str() == path)
            .unwrap();
        RangeRequestV1 {
            magic: RANGE_REQUEST_MAGIC,
            version: U16::new(kore_workspace_sync::bounds::FORMAT_VERSION),
            flags: U16::ZERO,
            snapshot: root.manifest().snapshot_id(),
            source: entry.source,
            version_digest: entry.digest,
            range_root: entry.range_root,
            file_len: U32::new(entry.content_len),
            offset: U32::new(offset),
            length: U16::new(length),
            retry: 0,
            reserved: 0,
        }
    }

    #[test]
    fn lazy_range_is_verified_and_host_mutation_is_stale() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), vec![7; 5000]).unwrap();
        let live = LiveRoot::open(root.path(), [0; 32]).unwrap();
        let served = live.read_range(&request(&live, "a", 4097, 3)).unwrap();
        assert_eq!(served.bytes, vec![7; 904]);
        assert!(kore_workspace_sync::verify_range(
            live.manifest().entries()[0].range_root,
            &served.proof,
            &served.bytes,
        ));
        std::fs::write(root.path().join("a"), vec![8; 5000]).unwrap();
        assert!(matches!(
            live.read_range(&request(&live, "a", 0, 1)),
            Err(Error::SourceChanged(_) | Error::Conflict)
        ));
    }

    #[test]
    fn selected_document_reads_projection_only_while_backing_is_current() {
        let host = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        std::fs::write(host.path().join("deck.pptx"), b"original bytes").unwrap();
        std::fs::write(host.path().join("unrelated.txt"), b"not authorized").unwrap();
        let selected = SelectedFile::new(
            Dir::open_ambient_dir(host.path(), ambient_authority()).unwrap(),
            "deck.pptx".into(),
            CanonicalPath::new(b"deck-text.txt").unwrap(),
            host.path().join("deck.pptx"),
            *blake3::hash(b"original bytes").as_bytes(),
            b"Slide 2: Growth 17.8%".to_vec(),
        )
        .unwrap();
        let live = LiveRoot::open_selected(empty.path(), vec![selected]).unwrap();
        assert_eq!(live.manifest().entries().len(), 1);
        let request = request(&live, "deck-text.txt", 0, 1);
        assert_eq!(
            live.read_range(&request).unwrap().bytes,
            b"Slide 2: Growth 17.8%"
        );
        std::fs::write(host.path().join("deck.pptx"), b"modified bytes").unwrap();
        assert!(live.read_range(&request).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_replacement_never_escapes_the_retained_root() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), b"safe").unwrap();
        let live = LiveRoot::open(root.path(), [0; 32]).unwrap();
        let request = request(&live, "a", 0, 1);
        std::fs::remove_file(root.path().join("a")).unwrap();
        symlink("/etc/passwd", root.path().join("a")).unwrap();
        assert!(live.read_range(&request).is_err());
    }

    #[test]
    fn standing_grant_applies_without_prompt_and_retry_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), b"old").unwrap();
        let mut live = LiveRoot::open(root.path(), [0; 32]).unwrap();
        let base = live.manifest().snapshot_id();
        let expected = live.manifest().entries()[0].digest;
        let replacement = b"new".to_vec();
        let replacement_digest = *blake3::hash(&replacement).as_bytes();
        let path = CanonicalPath::new(b"a").unwrap();
        let mut builder = ChangeSetBuilder::new(base);
        builder
            .push(Change {
                kind: ChangeKind::ModifyFile,
                path: path.clone(),
                target: None,
                expected,
                replacement: replacement_digest,
                replacement_len: replacement.len() as u32,
                executable: false,
            })
            .unwrap();
        let changes = builder.finish().unwrap();
        let mut contents = BTreeMap::new();
        contents.insert(path, replacement);
        let mut grant =
            StandingGrant::new([1; 16], base, GrantOperations::ALL, 4, 4096, 0, &[]).unwrap();
        let transaction = [9; 32];
        let first = live
            .apply_transaction(transaction, &changes, &contents, &mut grant, 1)
            .unwrap();
        assert!(matches!(first, TransactionOutcome::Applied { .. }));
        assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"new");
        assert_eq!(
            live.apply_transaction(transaction, &changes, &contents, &mut grant, 2)
                .unwrap(),
            first
        );
    }

    #[test]
    fn durable_result_survives_adapter_restart_and_prepared_never_infers_success() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), b"old").unwrap();
        let mut live = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        let base = live.manifest().snapshot_id();
        let entry = live.manifest().entries()[0].clone();
        let mut builder = ChangeSetBuilder::new(base);
        builder
            .push(Change {
                kind: ChangeKind::ModifyFile,
                path: entry.path.clone(),
                target: None,
                expected: entry.digest,
                replacement: *blake3::hash(b"new").as_bytes(),
                replacement_len: 3,
                executable: false,
            })
            .unwrap();
        let changes = builder.finish().unwrap();
        let mut contents = BTreeMap::new();
        contents.insert(entry.path, b"new".to_vec());
        let mut grant =
            StandingGrant::new([1; 16], base, GrantOperations::ALL, 4, 4096, 0, &[]).unwrap();
        let transaction = [9; 32];
        let outcome = live
            .apply_transaction(transaction, &changes, &contents, &mut grant, 1)
            .unwrap();
        drop(live);

        let mut reopened = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        assert_eq!(
            reopened
                .apply_transaction(transaction, &changes, &contents, &mut grant, 2)
                .unwrap(),
            outcome
        );

        let state2 = tempfile::tempdir().unwrap();
        let mut journal = TransactionJournal::open(state2.path()).unwrap().0;
        journal
            .append([7; 32], [8; 32], JournalOutcome::Prepared)
            .unwrap();
        drop(journal);
        let (_, records) = TransactionJournal::open(state2.path()).unwrap();
        assert_eq!(
            records[0].outcome,
            TransactionOutcome::Partial { applied: 0 }
        );
    }

    #[test]
    fn excluded_or_stale_transaction_touches_nothing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("private")).unwrap();
        std::fs::write(root.path().join("private/a"), b"old").unwrap();
        let mut live = LiveRoot::open(root.path(), [0; 32]).unwrap();
        let base = live.manifest().snapshot_id();
        let entry = live
            .manifest()
            .entries()
            .iter()
            .find(|entry| entry.path.as_str() == "private/a")
            .unwrap();
        let mut builder = ChangeSetBuilder::new(base);
        builder
            .push(Change {
                kind: ChangeKind::ModifyFile,
                path: entry.path.clone(),
                target: None,
                expected: entry.digest,
                replacement: *blake3::hash(b"new").as_bytes(),
                replacement_len: 3,
                executable: false,
            })
            .unwrap();
        let changes = builder.finish().unwrap();
        let mut contents = BTreeMap::new();
        contents.insert(CanonicalPath::new(b"private/a").unwrap(), b"new".to_vec());
        let mut grant = StandingGrant::new(
            [1; 16],
            base,
            GrantOperations::ALL,
            4,
            4096,
            0,
            &[CanonicalPath::new(b"private").unwrap()],
        )
        .unwrap();
        assert_eq!(
            live.apply_transaction([2; 32], &changes, &contents, &mut grant, 1)
                .unwrap(),
            TransactionOutcome::Refused
        );
        assert_eq!(
            std::fs::read(root.path().join("private/a")).unwrap(),
            b"old"
        );
    }
}
