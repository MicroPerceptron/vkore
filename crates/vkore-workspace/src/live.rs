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
    CanonicalPath, ChangeKind, ChangeSet, ChangeSetBuilder, DeltaOp, Entry, EntryKind, Manifest,
    ManifestBuilder, Policy, RangeProof, RangeRequestV1, SourceCorrelation, StandingGrant,
    TopologyDelta, TransactionOutcome, bounds::MAX_TRANSACTION_RESULTS, build_range_proof,
    range_root,
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
            manifest: artifact.manifest,
            sources,
            next_delta_sequence: 1,
            results,
            journal,
        })
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
        // A directory rename carries every descendant with it. Check the
        // standing grant for each affected source and destination path too;
        // authorizing only the top-level label would bypass nested exclusions.
        for change in changes
            .changes()
            .iter()
            .filter(|change| change.kind == ChangeKind::Rename)
        {
            let target = change.target.as_ref().ok_or(Error::InvalidArtifact)?;
            for entry in self
                .manifest
                .entries()
                .iter()
                .filter(|entry| entry.path.is_within(&change.path))
            {
                let mut path = target.as_bytes().to_vec();
                path.extend_from_slice(&entry.path.as_bytes()[change.path.as_bytes().len()..]);
                let mut probe = ChangeSetBuilder::new(changes.base);
                probe.push(kore_workspace_sync::Change {
                    path: entry.path.clone(),
                    target: Some(CanonicalPath::new(&path).map_err(|_| Error::InvalidArtifact)?),
                    expected: entry.digest,
                    ..change.clone()
                })?;
                if grant.authorize(&probe.finish()?, now).is_err() {
                    return self.remember(transaction, changes.id, TransactionOutcome::Refused);
                }
            }
        }
        let actual = Artifact::scan_cap(&self.root, self.manifest.header().parent_snapshot)?;
        if actual.manifest.snapshot_id() != self.manifest.snapshot_id() {
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

        // Compare the complete requested result with the observed tree. A
        // reverse diff guesses renames from unique file contents; it cannot
        // faithfully verify an explicit directory rename or equal-content
        // files whose identities the caller selected through real handles.
        let desired = match expected_manifest(&self.manifest, changes, contents) {
            Ok(manifest) => manifest,
            Err(_) => return self.remember(transaction, changes.id, TransactionOutcome::Conflict),
        };

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
        if observed.manifest.snapshot_id() != desired.snapshot_id() {
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

/// Apply the submitted operations in the same order as the capability-rooted
/// host adapter, solely to derive the expected canonical namespace. Source
/// correlations are observations and are excluded from snapshot identity.
fn expected_manifest(
    base: &Manifest,
    changes: &ChangeSet,
    contents: &BTreeMap<CanonicalPath, Vec<u8>>,
) -> Result<Manifest, Error> {
    let mut entries = base
        .entries()
        .iter()
        .map(|entry| (entry.path.clone(), entry.clone()))
        .collect::<BTreeMap<_, _>>();
    for change in changes
        .changes()
        .iter()
        .filter(|change| change.kind == ChangeKind::CreateDirectory)
    {
        if entries
            .insert(
                change.path.clone(),
                Entry::directory(change.path.clone(), [0; 16]),
            )
            .is_some()
        {
            return Err(Error::Conflict);
        }
    }
    for change in changes
        .changes()
        .iter()
        .filter(|change| change.kind == ChangeKind::Rename)
    {
        let target = change.target.as_ref().ok_or(Error::InvalidArtifact)?;
        if !entries.contains_key(&change.path)
            || entries.contains_key(target)
            || target.is_within(&change.path)
        {
            return Err(Error::Conflict);
        }
        let moved = entries
            .values()
            .filter(|entry| entry.path.is_within(&change.path))
            .cloned()
            .collect::<Vec<_>>();
        for mut entry in moved {
            entries.remove(&entry.path);
            let mut path = target.as_bytes().to_vec();
            path.extend_from_slice(&entry.path.as_bytes()[change.path.as_bytes().len()..]);
            entry.path = CanonicalPath::new(&path).map_err(|_| Error::InvalidArtifact)?;
            if entries.insert(entry.path.clone(), entry).is_some() {
                return Err(Error::Conflict);
            }
        }
    }
    for change in changes
        .changes()
        .iter()
        .filter(|change| matches!(change.kind, ChangeKind::CreateFile | ChangeKind::ModifyFile))
    {
        if (change.kind == ChangeKind::CreateFile) == entries.contains_key(&change.path) {
            return Err(Error::Conflict);
        }
        let bytes = contents.get(&change.path).ok_or(Error::InvalidArtifact)?;
        let entry = Entry::file_with_range_root(
            change.path.clone(),
            change.executable,
            change.replacement_len,
            change.replacement,
            range_root(bytes).ok_or(Error::InvalidArtifact)?,
            [0; 16],
        )
        .map_err(|_| Error::InvalidArtifact)?;
        entries.insert(change.path.clone(), entry);
    }
    for change in changes
        .changes()
        .iter()
        .rev()
        .filter(|change| change.kind == ChangeKind::Delete)
    {
        // Deleting an entire subtree requires explicit child operations, so
        // the standing grant checks exclusions for every affected path.
        if entries
            .keys()
            .any(|path| path != &change.path && path.is_within(&change.path))
        {
            return Err(Error::Conflict);
        }
        if entries.remove(&change.path).is_none() {
            return Err(Error::Conflict);
        }
    }
    let mut builder = ManifestBuilder::new(changes.base, Policy::default());
    for entry in entries.into_values() {
        builder.push(entry).map_err(|_| Error::InvalidArtifact)?;
    }
    builder.finish().map_err(|_| Error::InvalidArtifact)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kore_workspace_sync::{Change, ChangeSetBuilder, GrantOperations, RANGE_REQUEST_MAGIC};

    fn namespace_change(live: &LiveRoot, path: &str, target: Option<&str>) -> Change {
        let entry = live
            .manifest()
            .entries()
            .iter()
            .find(|entry| entry.path.as_str() == path)
            .unwrap();
        Change {
            kind: if target.is_some() {
                ChangeKind::Rename
            } else {
                ChangeKind::Delete
            },
            path: entry.path.clone(),
            target: target.map(|path| CanonicalPath::new(path.as_bytes()).unwrap()),
            expected: entry.digest,
            replacement: [0; 32],
            replacement_len: 0,
            executable: false,
        }
    }

    fn namespace_changes(live: &LiveRoot, changes: impl IntoIterator<Item = Change>) -> ChangeSet {
        let mut builder = ChangeSetBuilder::new(live.manifest().snapshot_id());
        for change in changes {
            builder.push(change).unwrap();
        }
        builder.finish().unwrap()
    }

    fn namespace_grant(live: &LiveRoot, exclusions: &[CanonicalPath]) -> StandingGrant {
        StandingGrant::new(
            [1; 16],
            live.manifest().snapshot_id(),
            GrantOperations::ALL,
            16,
            4096,
            0,
            exclusions,
        )
        .unwrap()
    }

    fn namespace_tree() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src/private")).unwrap();
        std::fs::write(root.path().join("src/main"), b"same").unwrap();
        std::fs::write(root.path().join("src/private/data"), b"secret").unwrap();
        std::fs::write(root.path().join("other"), b"same").unwrap();
        root
    }

    #[test]
    fn created_directory_and_file_accept_later_content_without_changing_siblings() {
        let root = namespace_tree();
        let state = tempfile::tempdir().unwrap();
        let mut live = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        let mut grant = namespace_grant(&live, &[]);
        for (nonce, kind, path, content) in [
            (21, ChangeKind::CreateDirectory, "new", None),
            (22, ChangeKind::CreateFile, "new/file", Some(b"".as_slice())),
            (
                23,
                ChangeKind::ModifyFile,
                "new/file",
                Some(b"created".as_slice()),
            ),
        ] {
            let path = CanonicalPath::new(path.as_bytes()).unwrap();
            let expected = if kind == ChangeKind::ModifyFile {
                live.manifest()
                    .entries()
                    .iter()
                    .find(|entry| entry.path == path)
                    .unwrap()
                    .digest
            } else {
                [0; 32]
            };
            let changes = namespace_changes(
                &live,
                [Change {
                    kind,
                    path: path.clone(),
                    target: None,
                    expected,
                    replacement: content.map_or([0; 32], |bytes| *blake3::hash(bytes).as_bytes()),
                    replacement_len: content.map_or(0, |bytes| bytes.len() as u32),
                    executable: false,
                }],
            );
            let contents = content.map_or_else(BTreeMap::new, |bytes| {
                BTreeMap::from([(path, bytes.to_vec())])
            });
            let outcome = live
                .apply_transaction([nonce; 32], &changes, &contents, &mut grant, 1)
                .unwrap();
            assert!(
                matches!(outcome, TransactionOutcome::Applied { .. }),
                "{outcome:?}"
            );
            assert_eq!(
                live.apply_transaction([nonce; 32], &changes, &contents, &mut grant, 2)
                    .unwrap(),
                outcome
            );
        }
        assert_eq!(
            std::fs::read(root.path().join("new/file")).unwrap(),
            b"created"
        );
        assert_eq!(
            std::fs::read(root.path().join("src/main")).unwrap(),
            b"same"
        );
        let query = request(&live, "new/file", 0, 7);
        assert_eq!(live.read_range(&query).unwrap().bytes, b"created");
    }

    #[test]
    fn explicit_directory_rename_is_applied_and_replayed_after_restart() {
        let root = namespace_tree();
        let state = tempfile::tempdir().unwrap();
        let mut live = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        let changes = namespace_changes(&live, [namespace_change(&live, "src", Some("lib"))]);
        let mut grant = namespace_grant(&live, &[]);
        let outcome = live
            .apply_transaction([11; 32], &changes, &BTreeMap::new(), &mut grant, 1)
            .unwrap();
        assert!(
            matches!(outcome, TransactionOutcome::Applied { .. }),
            "{outcome:?}"
        );
        assert!(!root.path().join("src").exists());
        assert_eq!(
            std::fs::read(root.path().join("lib/main")).unwrap(),
            b"same"
        );
        assert_eq!(
            std::fs::read(root.path().join("lib/private/data")).unwrap(),
            b"secret"
        );
        assert_eq!(std::fs::read(root.path().join("other")).unwrap(), b"same");
        assert_eq!(
            live.apply_transaction([11; 32], &changes, &BTreeMap::new(), &mut grant, 2)
                .unwrap(),
            outcome
        );
        drop(live);
        let mut reopened = LiveRoot::open_with_state(root.path(), [0; 32], state.path()).unwrap();
        assert_eq!(
            reopened
                .apply_transaction([11; 32], &changes, &BTreeMap::new(), &mut grant, 3)
                .unwrap(),
            outcome
        );
    }

    #[test]
    fn explicit_file_rename_preserves_equal_content_sibling() {
        let root = namespace_tree();
        let mut live = LiveRoot::open(root.path(), [0; 32]).unwrap();
        let changes = namespace_changes(
            &live,
            [namespace_change(&live, "src/main", Some("src/new"))],
        );
        let mut grant = namespace_grant(&live, &[]);
        let outcome = live
            .apply_transaction([12; 32], &changes, &BTreeMap::new(), &mut grant, 1)
            .unwrap();
        assert!(
            matches!(outcome, TransactionOutcome::Applied { .. }),
            "{outcome:?}"
        );
        assert!(!root.path().join("src/main").exists());
        assert_eq!(std::fs::read(root.path().join("src/new")).unwrap(), b"same");
        assert_eq!(std::fs::read(root.path().join("other")).unwrap(), b"same");
    }

    #[test]
    fn directory_rename_checks_nested_source_and_destination_exclusions() {
        for exclusion in ["src/private", "lib/private"] {
            let root = namespace_tree();
            let mut live = LiveRoot::open(root.path(), [0; 32]).unwrap();
            let changes = namespace_changes(&live, [namespace_change(&live, "src", Some("lib"))]);
            let mut grant =
                namespace_grant(&live, &[CanonicalPath::new(exclusion.as_bytes()).unwrap()]);
            assert_eq!(
                live.apply_transaction([13; 32], &changes, &BTreeMap::new(), &mut grant, 1)
                    .unwrap(),
                TransactionOutcome::Refused
            );
            assert!(!root.path().join("lib").exists());
            assert_eq!(
                std::fs::read(root.path().join("src/main")).unwrap(),
                b"same"
            );
            assert_eq!(
                std::fs::read(root.path().join("src/private/data")).unwrap(),
                b"secret"
            );
        }
    }

    #[test]
    fn explicit_subtree_deletion_checks_every_path_before_any_effect() {
        for excluded in [false, true] {
            let root = namespace_tree();
            let mut live = LiveRoot::open(root.path(), [0; 32]).unwrap();
            let changes = namespace_changes(
                &live,
                ["src", "src/main", "src/private", "src/private/data"]
                    .map(|path| namespace_change(&live, path, None)),
            );
            let exclusions = if excluded {
                vec![CanonicalPath::new(b"src/private/data").unwrap()]
            } else {
                vec![]
            };
            let mut grant = namespace_grant(&live, &exclusions);
            let outcome = live
                .apply_transaction([14; 32], &changes, &BTreeMap::new(), &mut grant, 1)
                .unwrap();
            if excluded {
                assert_eq!(outcome, TransactionOutcome::Refused);
                assert_eq!(
                    std::fs::read(root.path().join("src/main")).unwrap(),
                    b"same"
                );
                assert_eq!(
                    std::fs::read(root.path().join("src/private/data")).unwrap(),
                    b"secret"
                );
            } else {
                assert!(
                    matches!(outcome, TransactionOutcome::Applied { .. }),
                    "{outcome:?}"
                );
                assert!(!root.path().join("src").exists());
            }
            assert_eq!(std::fs::read(root.path().join("other")).unwrap(), b"same");
        }
    }

    #[test]
    fn implicit_subtree_deletion_is_refused_before_earlier_file_deletion() {
        let root = namespace_tree();
        let mut live = LiveRoot::open(root.path(), [0; 32]).unwrap();
        let changes = namespace_changes(
            &live,
            ["src", "src/main"].map(|path| namespace_change(&live, path, None)),
        );
        let mut grant = namespace_grant(&live, &[]);
        assert_eq!(
            live.apply_transaction([15; 32], &changes, &BTreeMap::new(), &mut grant, 1)
                .unwrap(),
            TransactionOutcome::Conflict
        );
        assert_eq!(
            std::fs::read(root.path().join("src/main")).unwrap(),
            b"same"
        );
        assert_eq!(
            std::fs::read(root.path().join("src/private/data")).unwrap(),
            b"secret"
        );
    }

    #[test]
    fn directory_rename_detects_unexpected_host_child_before_effects() {
        let root = namespace_tree();
        let mut live = LiveRoot::open(root.path(), [0; 32]).unwrap();
        let changes = namespace_changes(&live, [namespace_change(&live, "src", Some("lib"))]);
        let mut grant = namespace_grant(&live, &[]);
        std::fs::write(root.path().join("src/new"), b"external").unwrap();
        assert_eq!(
            live.apply_transaction([16; 32], &changes, &BTreeMap::new(), &mut grant, 1)
                .unwrap(),
            TransactionOutcome::Conflict
        );
        assert!(!root.path().join("lib").exists());
        assert_eq!(
            std::fs::read(root.path().join("src/new")).unwrap(),
            b"external"
        );
        assert_eq!(
            std::fs::read(root.path().join("src/main")).unwrap(),
            b"same"
        );
    }

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
