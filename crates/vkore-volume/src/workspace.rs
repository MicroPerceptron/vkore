//! A host project staged into a terminal's home for a machine, and what the
//! machine changed, applied back.
//!
//! - [`stage`] scans the project root as a capability (nothing above it is
//!   reachable, no link is followed), copies what Kore can hold into a
//!   private snapshot, and installs that snapshot with Kore's own
//!   all-or-nothing publication. The returned [`Manifest`] records every
//!   staged file by length and BLAKE3 digest, and every entry left out and
//!   why (a link, a special file, a name Kore cannot hold, an exclusion).
//! - [`collect`] mounts the volume after the machine has shut down, walks
//!   the project's directory and reads every file once, keeping only what
//!   differs from the manifest.
//! - [`apply`] writes those changes back inside the project root: refused
//!   whole if they exceed the caller's bounds, and refused whole if any host
//!   file it would replace or remove changed since it was staged. Each file
//!   is published by one rename; a new file never replaces one that
//!   appeared meanwhile.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use cap_fs_ext::DirExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use serde::{Deserialize, Serialize};

use crate::{
    Browser, Disk, DiskKind, EntryKind, Error, VolPath,
    bounds::{FORMAT_VERSION, MAX_DIR_ENTRIES, MAX_OBJECTS, MAX_PATH_DEPTH},
    path::check_name,
    transfer::{exists, open_dir_within, partial_name, publish, refuse_link},
};

/// The manifest format this crate writes and reads.
pub const MANIFEST_VERSION: u32 = 1;

/// Names left out at any depth unless the caller says otherwise: version
/// control and build output, which a machine's terminal has no use for and
/// which would exhaust the volume's object bound.
pub const DEFAULT_EXCLUDES: &[&str] = &[".git", ".hg", ".svn", "target", "node_modules", ".vkore"];

/// Levels a home takes below the volume's root (`work/face-N`).
const HOME_DEPTH: usize = 2;

/// How a project is staged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageOptions {
    /// The terminal whose home receives the project.
    pub face: u8,
    /// The project's directory name in that home.
    pub name: String,
    /// Entry names left out at any depth.
    pub exclude: BTreeSet<String>,
}

impl StageOptions {
    /// Stage as `name` in terminal 0's home, leaving out [`DEFAULT_EXCLUDES`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            face: 0,
            name: name.into(),
            exclude: DEFAULT_EXCLUDES
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        }
    }
}

/// A staged file: what the volume received.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    pub len: u64,
    /// BLAKE3, lowercase hex.
    pub blake3: String,
}

/// An entry staging left out, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

/// What [`stage`] put on the volume: the record [`collect`] and [`apply`]
/// compare against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub face: u8,
    pub name: String,
    /// The volume's root generation that names the staged project.
    pub generation: u64,
    /// The DagFS format version the install was committed in.
    pub format_version: u32,
    /// The project root it was staged from, resolved.
    pub workspace: PathBuf,
    pub files: BTreeMap<String, FileState>,
    pub directories: BTreeSet<String>,
    pub skipped: Vec<Skipped>,
}

impl Manifest {
    /// The project's directory on the volume.
    pub fn root(&self) -> Result<VolPath, Error> {
        VolPath::home(self.face)?.join(&self.name)
    }

    /// Write the manifest to `path`, replacing it in one rename.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|error| Error::Manifest(error.to_string()))?;
        let partial = path.with_extension("json.partial");
        fs::write(&partial, bytes).map_err(|error| Error::host(&partial, error))?;
        fs::rename(&partial, path).map_err(|error| Error::host(path, error))
    }

    /// Read a manifest [`Manifest::save`] wrote.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let bytes = fs::read(path).map_err(|error| Error::host(path, error))?;
        let manifest: Self =
            serde_json::from_slice(&bytes).map_err(|error| Error::Manifest(error.to_string()))?;
        if manifest.version != MANIFEST_VERSION {
            return Err(Error::Manifest(format!(
                "version {} (this vKore reads {MANIFEST_VERSION})",
                manifest.version
            )));
        }
        check_name(&manifest.name)?;
        Ok(manifest)
    }
}

/// What the scan of a project root found to stage.
#[derive(Default)]
struct Scan {
    files: Vec<(String, u64)>,
    directories: Vec<String>,
    skipped: Vec<Skipped>,
}

/// Stage `workspace` into `disk` as `options.name` in terminal
/// `options.face`'s home, replacing what a previous staging left there. A
/// blank image is formatted first, as the machine's first boot would; a
/// blank disk is its machine's to format. `scratch` holds the snapshot
/// while it is installed (a private directory of the caller's, outside the
/// project).
pub fn stage(
    disk: &Disk,
    workspace: &Path,
    scratch: &Path,
    options: &StageOptions,
) -> Result<Manifest, Error> {
    check_name(&options.name)?;
    let destination = VolPath::home(options.face)?.join(&options.name)?;
    let workspace = fs::canonicalize(workspace).map_err(|error| Error::host(workspace, error))?;
    let scratch_resolved =
        fs::canonicalize(scratch).map_err(|error| Error::host(scratch, error))?;
    if scratch_resolved.starts_with(&workspace) {
        return Err(Error::Path(format!(
            "{}: the staging scratch lies inside the project it stages",
            scratch_resolved.display()
        )));
    }
    let root = Dir::open_ambient_dir(&workspace, ambient_authority())
        .map_err(|error| Error::host(&workspace, error))?;
    let mut scan = Scan::default();
    let mut objects = 1;
    scan_dir(&root, &workspace, "", 1, options, &mut scan, &mut objects)?;

    let snapshot = private_dir(&scratch_resolved, "stage")?;
    let staged = snapshot.join(&options.name);
    let copied = copy_snapshot(&root, &workspace, &staged, &scan);
    let installed = copied.and_then(|files| {
        if disk.is_blank()? {
            if disk.kind() == DiskKind::Device {
                return Err(Error::Path(format!(
                    "{}: a blank disk is formatted by its machine's first boot",
                    disk.path().display()
                )));
            }
            disk.format()?;
        }
        let installed = disk.install(&staged, &destination, true)?;
        Ok((installed, files))
    });
    let _ = fs::remove_dir_all(&snapshot);
    let (installed, files) = installed?;
    Ok(Manifest {
        version: MANIFEST_VERSION,
        face: options.face,
        name: options.name.clone(),
        generation: installed.generation,
        format_version: FORMAT_VERSION,
        workspace,
        files,
        directories: scan.directories.into_iter().collect(),
        skipped: scan.skipped,
    })
}

/// Scan `relative` below `root`, at `depth` levels below the staged
/// directory, checking the volume's bounds before anything is copied.
fn scan_dir(
    root: &Dir,
    host: &Path,
    relative: &str,
    depth: usize,
    options: &StageOptions,
    scan: &mut Scan,
    objects: &mut usize,
) -> Result<(), Error> {
    let shown = host.join(relative);
    let dir = if relative.is_empty() {
        root.try_clone()
    } else {
        root.open_dir_nofollow(relative)
    }
    .map_err(|error| Error::host(&shown, error))?;
    let mut children = Vec::new();
    for entry in dir.entries().map_err(|error| Error::host(&shown, error))? {
        let entry = entry.map_err(|error| Error::host(&shown, error))?;
        let raw = entry.file_name();
        let Some(name) = raw.to_str() else {
            scan.skipped.push(Skipped {
                path: join(relative, &raw.to_string_lossy()),
                reason: "not UTF-8".into(),
            });
            continue;
        };
        let path = join(relative, name);
        if options.exclude.contains(name) {
            scan.skipped.push(Skipped {
                path,
                reason: "excluded".into(),
            });
            continue;
        }
        if let Err(error) = check_name(name) {
            scan.skipped.push(Skipped {
                path,
                reason: error.to_string(),
            });
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| Error::host(host.join(&path), error))?;
        if kind.is_symlink() {
            scan.skipped.push(Skipped {
                path,
                reason: "a symbolic link (not followed)".into(),
            });
        } else if kind.is_dir() {
            children.push((name.to_owned(), None));
        } else if kind.is_file() {
            let len = entry
                .metadata()
                .map_err(|error| Error::host(host.join(&path), error))?
                .len();
            children.push((name.to_owned(), Some(len)));
        } else {
            scan.skipped.push(Skipped {
                path,
                reason: "a special file".into(),
            });
        }
    }
    if children.len() > MAX_DIR_ENTRIES {
        return Err(Error::Bounds(format!(
            "{}: {} entries, and a kore directory holds at most {MAX_DIR_ENTRIES}",
            shown.display(),
            children.len()
        )));
    }
    if !children.is_empty() && HOME_DEPTH + depth + 1 > MAX_PATH_DEPTH {
        return Err(Error::Bounds(format!(
            "{}: deeper than kore's {MAX_PATH_DEPTH} levels below the volume's root",
            shown.display()
        )));
    }
    *objects += children.len();
    if *objects > MAX_OBJECTS {
        return Err(Error::Bounds(format!(
            "{}: more than the {MAX_OBJECTS} objects a kore volume holds (exclude \
             build output or version control, or stage a smaller root)",
            host.display()
        )));
    }
    children.sort();
    for (name, len) in children {
        let path = join(relative, &name);
        match len {
            Some(len) => scan.files.push((path, len)),
            None => {
                scan.directories.push(path.clone());
                scan_dir(root, host, &path, depth + 1, options, scan, objects)?;
            }
        }
    }
    Ok(())
}

/// Copy the scanned tree into `staged`, digesting each file as it is read.
fn copy_snapshot(
    root: &Dir,
    host: &Path,
    staged: &Path,
    scan: &Scan,
) -> Result<BTreeMap<String, FileState>, Error> {
    fs::create_dir(staged).map_err(|error| Error::host(staged, error))?;
    for directory in &scan.directories {
        let path = staged.join(directory);
        fs::create_dir(&path).map_err(|error| Error::host(&path, error))?;
    }
    let mut files = BTreeMap::new();
    let mut buffer = vec![0; 1 << 16];
    for (relative, _) in &scan.files {
        let shown = host.join(relative);
        let mut source = root
            .open(relative)
            .map_err(|error| Error::host(&shown, error))?
            .into_std();
        // The scan saw a regular file; refuse whatever replaced it since.
        if !source
            .metadata()
            .map_err(|error| Error::host(&shown, error))?
            .is_file()
        {
            return Err(Error::Path(format!(
                "{}: no longer a regular file",
                shown.display()
            )));
        }
        let target = staged.join(relative);
        let mut sink = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|error| Error::host(&target, error))?;
        let mut hasher = blake3::Hasher::new();
        let mut len = 0u64;
        loop {
            let read = match source.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(Error::host(&shown, error)),
            };
            hasher.update(&buffer[..read]);
            io::Write::write_all(&mut sink, &buffer[..read])
                .map_err(|error| Error::host(&target, error))?;
            len += read as u64;
        }
        files.insert(
            relative.clone(),
            FileState {
                len,
                blake3: hasher.finalize().to_hex().to_string(),
            },
        );
    }
    Ok(files)
}

/// A file the machine created or changed, held until applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub path: String,
    pub len: u64,
    /// BLAKE3, lowercase hex.
    pub blake3: String,
    #[serde(skip)]
    held: PathBuf,
}

/// What the machine changed in a staged project.
#[derive(Debug, Serialize)]
pub struct Changes {
    pub created: Vec<Change>,
    pub modified: Vec<Change>,
    pub deleted: Vec<String>,
    pub created_directories: Vec<String>,
    /// Deepest first.
    pub deleted_directories: Vec<String>,
    /// The volume generation the changes were read at.
    pub generation: u64,
    #[serde(skip)]
    held: Option<PathBuf>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty()
            && self.modified.is_empty()
            && self.deleted.is_empty()
            && self.created_directories.is_empty()
            && self.deleted_directories.is_empty()
    }

    /// Files written or removed if applied.
    pub fn file_count(&self) -> usize {
        self.created.len() + self.modified.len() + self.deleted.len()
    }

    /// Bytes written if applied.
    pub fn bytes(&self) -> u64 {
        self.created
            .iter()
            .chain(&self.modified)
            .map(|change| change.len)
            .sum()
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            let _ = fs::remove_dir_all(held);
        }
    }
}

/// Read what the machine left in the staged project, keeping what differs
/// from `manifest`. The machine must have shut down: a disk it runs on is
/// refused as in use. `scratch` holds changed files until applied.
pub fn collect(disk: &Disk, manifest: &Manifest, scratch: &Path) -> Result<Changes, Error> {
    let root = manifest.root()?;
    let browser = Browser::open(disk)?;
    let held = private_dir(scratch, "collect")?;
    let mut changes = Changes {
        created: Vec::new(),
        modified: Vec::new(),
        deleted: Vec::new(),
        created_directories: Vec::new(),
        deleted_directories: Vec::new(),
        generation: browser.status().generation,
        held: Some(held.clone()),
    };
    let walked = match browser.walk(&root) {
        Ok(walked) => walked,
        // The machine removed the project's directory itself.
        Err(error) if error.is_not_found() => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();
    for (index, entry) in walked.into_iter().enumerate() {
        match entry.kind {
            EntryKind::Directory => {
                if !manifest.directories.contains(&entry.path) {
                    changes.created_directories.push(entry.path.clone());
                }
                directories.insert(entry.path);
            }
            EntryKind::File => {
                let path = join_volume(&root, &entry.path)?;
                let target = held.join(format!("{index}"));
                let sink = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)
                    .map_err(|error| Error::host(&target, error))?;
                let (len, digest) = browser.copy_into(&path, sink)?;
                let digest = digest.to_hex().to_string();
                match manifest.files.get(&entry.path) {
                    Some(state) if state.len == len && state.blake3 == digest => {
                        let _ = fs::remove_file(&target);
                    }
                    staged => {
                        let change = Change {
                            path: entry.path.clone(),
                            len,
                            blake3: digest,
                            held: target,
                        };
                        if staged.is_some() {
                            changes.modified.push(change);
                        } else {
                            changes.created.push(change);
                        }
                    }
                }
                files.insert(entry.path);
            }
        }
    }
    browser.close()?;
    changes.deleted = manifest
        .files
        .keys()
        .filter(|path| !files.contains(*path))
        .cloned()
        .collect();
    let mut gone = manifest
        .directories
        .iter()
        .filter(|path| !directories.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    gone.sort_by_key(|path| std::cmp::Reverse(path.split('/').count()));
    changes.deleted_directories = gone;
    Ok(changes)
}

/// How much an apply may write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApplyLimits {
    pub max_files: usize,
    pub max_bytes: u64,
}

/// What [`apply`] did to the project.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Applied {
    pub written: usize,
    pub deleted: usize,
    pub bytes: u64,
    pub directories_created: usize,
    pub directories_removed: usize,
    /// Directories the machine removed that still hold host files, left in
    /// place.
    pub directories_kept: Vec<String>,
}

/// Apply `changes` inside the project root `manifest` was staged from:
/// refused whole beyond `limits`, refused whole if any host file to be
/// replaced or removed changed since staging (or a new file's name is taken
/// by different content), and otherwise written file by file, each in one
/// rename.
pub fn apply(
    changes: &Changes,
    manifest: &Manifest,
    limits: ApplyLimits,
) -> Result<Applied, Error> {
    if changes.file_count() > limits.max_files || changes.bytes() > limits.max_bytes {
        return Err(Error::Bounds(format!(
            "{} files and {} bytes to write back, beyond the bound of {} files and {} bytes",
            changes.file_count(),
            changes.bytes(),
            limits.max_files,
            limits.max_bytes
        )));
    }
    let host = &manifest.workspace;
    let root = Dir::open_ambient_dir(host, ambient_authority())
        .map_err(|error| Error::host(host, error))?;

    // Every precondition before anything is written.
    let mut conflicts = Vec::new();
    for path in changes
        .modified
        .iter()
        .map(|change| &change.path)
        .chain(&changes.deleted)
    {
        let staged = manifest
            .files
            .get(path)
            .ok_or_else(|| Error::Manifest(format!("{path} was not staged")))?;
        if host_digest(&root, host, path)?.as_deref() != Some(staged.blake3.as_str()) {
            conflicts.push(path.clone());
        }
    }
    let mut already = BTreeSet::new();
    for change in &changes.created {
        match host_digest(&root, host, &change.path)? {
            None => {}
            Some(digest) if digest == change.blake3 => {
                already.insert(change.path.clone());
            }
            Some(_) => conflicts.push(change.path.clone()),
        }
    }
    for directory in &changes.created_directories {
        let (parent, name) = open_parent(&root, host, directory)?;
        if let Some(parent) = parent {
            match parent.symlink_metadata(&name) {
                Ok(metadata) if !metadata.is_dir() => conflicts.push(directory.clone()),
                _ => {}
            }
        }
    }
    if !conflicts.is_empty() {
        return Err(Error::Conflict(conflicts));
    }

    let mut applied = Applied::default();
    let mut created = changes.created_directories.clone();
    created.sort_by_key(|path| path.split('/').count());
    for directory in &created {
        let (parent, name) = open_parent(&root, host, directory)?;
        let parent = parent.ok_or_else(|| {
            Error::Path(format!(
                "{}: its parent is missing",
                host.join(directory).display()
            ))
        })?;
        match parent.create_dir(&name) {
            Ok(()) => applied.directories_created += 1,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(Error::host(host.join(directory), error)),
        }
    }
    for (change, replace) in changes
        .modified
        .iter()
        .map(|change| (change, true))
        .chain(changes.created.iter().map(|change| (change, false)))
    {
        if already.contains(&change.path) {
            continue;
        }
        write_back(&root, host, change, replace)?;
        applied.written += 1;
        applied.bytes += change.len;
    }
    for path in &changes.deleted {
        let (parent, name) = open_parent(&root, host, path)?;
        if let Some(parent) = parent {
            match parent.remove_file(&name) {
                Ok(()) => applied.deleted += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::host(host.join(path), error)),
            }
        }
    }
    for directory in &changes.deleted_directories {
        let (parent, name) = open_parent(&root, host, directory)?;
        let Some(parent) = parent else { continue };
        match parent.remove_dir(&name) {
            Ok(()) => applied.directories_removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            // Host files the machine never saw keep the directory alive.
            Err(_) => applied.directories_kept.push(directory.clone()),
        }
    }
    Ok(applied)
}

/// Publish a held change as `change.path` below `root`.
fn write_back(root: &Dir, host: &Path, change: &Change, replace: bool) -> Result<(), Error> {
    let shown = host.join(&change.path);
    let (parent, name) = open_parent(root, host, &change.path)?;
    let parent =
        parent.ok_or_else(|| Error::Path(format!("{}: its parent is missing", shown.display())))?;
    refuse_link(&parent, &name, &shown)?;
    let partial = partial_name(&name);
    let written = (|| {
        let mut sink = parent
            .open_with(&partial, OpenOptions::new().write(true).create_new(true))
            .map_err(|error| Error::host(&shown, error))?
            .into_std();
        let mut source =
            fs::File::open(&change.held).map_err(|error| Error::host(&change.held, error))?;
        let copied =
            io::copy(&mut source, &mut sink).map_err(|error| Error::host(&shown, error))?;
        if copied != change.len {
            return Err(Error::Path(format!(
                "{}: {copied} bytes held, {} collected",
                shown.display(),
                change.len
            )));
        }
        sink.sync_all()
            .map_err(|error| Error::host(&shown, error))?;
        publish(&parent, &partial, &name, &shown, replace)
    })();
    if written.is_err() {
        let _ = parent.remove_file(&partial);
    }
    written
}

/// The BLAKE3 hex digest of the regular file at `path` below `root`, or
/// `None` when nothing is there. A link or a non-file there is a conflict
/// the caller reports as a digest no staged file has.
fn host_digest(root: &Dir, host: &Path, path: &str) -> Result<Option<String>, Error> {
    let shown = host.join(path);
    let (parent, name) = open_parent(root, host, path)?;
    let Some(parent) = parent else {
        return Ok(None);
    };
    let metadata = match parent.symlink_metadata(&name) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::host(&shown, error)),
    };
    if !metadata.is_file() {
        return Ok(Some(String::from("not a regular file")));
    }
    let mut file = parent
        .open(&name)
        .map_err(|error| Error::host(&shown, error))?
        .into_std();
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher).map_err(|error| Error::host(&shown, error))?;
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

/// The directory holding `path` below `root` (`None` when a directory on
/// the way is missing), and the last name, every component checked and no
/// link followed.
fn open_parent(root: &Dir, host: &Path, path: &str) -> Result<(Option<Dir>, String), Error> {
    let parts = path.split('/').collect::<Vec<_>>();
    for part in &parts {
        check_name(part)?;
    }
    let (last, parents) = parts
        .split_last()
        .ok_or_else(|| Error::Path("an empty path".into()))?;
    if parents.iter().enumerate().any(|(depth, _)| {
        let prefix = parents[..=depth].join("/");
        !exists(root, &prefix, host).unwrap_or(false)
    }) {
        return Ok((None, (*last).to_owned()));
    }
    Ok((
        Some(open_dir_within(root, parents, host)?),
        (*last).to_owned(),
    ))
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

/// `relative` (`/`-separated, as a walk names it) below `root`.
fn join_volume(root: &VolPath, relative: &str) -> Result<VolPath, Error> {
    relative
        .split('/')
        .try_fold(root.clone(), |path, part| path.join(part))
}

/// A fresh private directory below `scratch`.
fn private_dir(scratch: &Path, purpose: &str) -> Result<PathBuf, Error> {
    use std::os::unix::fs::DirBuilderExt;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = scratch.join(format!(
        "{purpose}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|error| Error::host(&path, error))?;
    Ok(path)
}
