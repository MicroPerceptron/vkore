use std::{
    fs::File,
    io::{self, Write},
    sync::mpsc,
    thread,
};

use kore_host::volume::{self, Kind, Mount, VolError};
use serde::Serialize;

use crate::{Disk, Error, VolPath, claim::Claim};

/// What a listed object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
}

impl From<Kind> for EntryKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::File => Self::File,
            Kind::Directory => Self::Directory,
        }
    }
}

/// One entry of a listed directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// A file's length in bytes; 0 for a directory.
    pub len: u64,
}

/// What a path names on the volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Listing {
    /// A directory's entries, directories first, each group by name.
    Directory { entries: Vec<Entry> },
    /// A file, and its length in bytes.
    File { len: u64 },
    /// A terminal's home nothing has written to yet: the machine creates it
    /// on the boot that first roots a terminal there.
    EmptyHome,
}

/// What recovery found when the volume was mounted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct MountStatus {
    /// The root generation the mount reads.
    pub generation: u64,
    /// Recovery took the fallback root: Kore itself keeps this volume
    /// read-only, and nothing should write it.
    pub used_fallback_root: bool,
}

/// One object beneath a walked directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Walked {
    /// Its `/`-separated path below the walked directory.
    pub path: String,
    pub kind: EntryKind,
    /// A file's length in bytes; 0 for a directory.
    pub len: u64,
    /// Levels below the walked directory: 1 for its own entries.
    pub depth: usize,
}

type Job = Box<dyn FnOnce(&Mount) + Send>;

/// A volume mounted read-only on a thread of its own, answering requests
/// for as long as it is held.
///
/// The mount holds the image's shared record lock: a machine cannot boot
/// from the disk while a browser has it open, and a browser cannot open a
/// disk a machine is running on. Drop it (or [`Browser::close`] it) before
/// installing, removing or booting.
pub struct Browser {
    disk: Disk,
    status: MountStatus,
    jobs: Option<mpsc::Sender<Job>>,
    worker: Option<thread::JoinHandle<Result<(), VolError>>>,
    /// Released after the worker has joined, so only once the engine has
    /// closed the disk (fields drop after `Drop::drop`).
    _claim: Claim,
}

impl Browser {
    /// Mount `disk` read-only as the boot finds and recovers it.
    pub fn open(disk: &Disk) -> Result<Self, Error> {
        let claim = Claim::take(disk)?;
        let (jobs, queue) = mpsc::channel::<Job>();
        let (ready, mounted) = mpsc::sync_channel::<MountStatus>(1);
        let target = disk.clone();
        let worker = thread::Builder::new()
            .name("vkore-volume".into())
            .spawn(move || {
                volume::read(target.path(), target.block_size(), move |mount| {
                    let status = mount.status();
                    // A browser that stopped waiting is no reason to fail.
                    let _ = ready.send(MountStatus {
                        generation: status.generation,
                        used_fallback_root: status.used_fallback_root,
                    });
                    while let Ok(job) = queue.recv() {
                        job(mount);
                    }
                    Ok(())
                })
            })
            .map_err(|error| Error::host(disk.path(), error))?;
        match mounted.recv() {
            Ok(status) => Ok(Self {
                disk: disk.clone(),
                status,
                jobs: Some(jobs),
                worker: Some(worker),
                _claim: claim,
            }),
            // The mount never reached its session: the engine's refusal is
            // what the worker answered.
            Err(_) => {
                drop(jobs);
                match worker.join() {
                    Ok(Err(error)) => Err(error.into()),
                    Ok(Ok(())) | Err(_) => Err(Error::SessionEnded),
                }
            }
        }
    }

    pub fn disk(&self) -> &Disk {
        &self.disk
    }

    pub fn status(&self) -> MountStatus {
        self.status
    }

    fn call<T: Send + 'static>(
        &self,
        job: impl FnOnce(&Mount) -> T + Send + 'static,
    ) -> Result<T, Error> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.jobs
            .as_ref()
            .ok_or(Error::SessionEnded)?
            .send(Box::new(move |mount| {
                let _ = reply.send(job(mount));
            }))
            .map_err(|_| Error::SessionEnded)?;
        answer.recv().map_err(|_| Error::SessionEnded)
    }

    /// What `path` names: a directory's entries (directories first, each
    /// group by name), a file's length, or a home not yet created.
    pub fn list(&self, path: &VolPath) -> Result<Listing, Error> {
        let face = path.face();
        let engine_path = path.engine_path();
        let listing = self.call(move |mount| mount.list(face, engine_path.as_deref()))??;
        Ok(match listing {
            volume::Listing::Directory(entries) => {
                let mut entries = entries
                    .into_iter()
                    .map(|entry| Entry {
                        name: entry.name,
                        kind: entry.kind.into(),
                        len: entry.len,
                    })
                    .collect::<Vec<_>>();
                entries.sort_by(|a, b| {
                    (a.kind != EntryKind::Directory, &a.name)
                        .cmp(&(b.kind != EntryKind::Directory, &b.name))
                });
                Listing::Directory { entries }
            }
            volume::Listing::File { len } => Listing::File { len },
            volume::Listing::EmptyHome => Listing::EmptyHome,
        })
    }

    /// A file's bytes, refusing one longer than `limit` without reading
    /// past it.
    pub fn read(&self, path: &VolPath, limit: u64) -> Result<Vec<u8>, Error> {
        let (face, engine_path) = file_path(path)?;
        let shown = path.to_string();
        let mut sink = Bounded {
            bytes: Vec::new(),
            limit,
            exceeded: false,
        };
        let (read, sink) = self.call(move |mount| {
            let read = mount.read_to(face, &engine_path, &mut sink);
            (read, sink)
        })?;
        match read {
            Ok(_) => Ok(sink.bytes),
            Err(VolError::Output(_)) if sink.exceeded => {
                Err(Error::TooLarge { what: shown, limit })
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Up to `limit` leading bytes of the file at `path`. Reading stops at
    /// the limit: a preview of a large file costs what it shows.
    pub fn head(&self, path: &VolPath, limit: usize) -> Result<Vec<u8>, Error> {
        let (face, engine_path) = file_path(path)?;
        let mut sink = Head {
            bytes: Vec::with_capacity(limit.min(64 << 10)),
            limit,
            full: false,
        };
        let (read, sink) = self.call(move |mount| {
            let read = mount.read_to(face, &engine_path, &mut sink);
            (read, sink)
        })?;
        match read {
            Ok(_) => Ok(sink.bytes),
            // The sink stopped the copy, as asked: the engine has closed the
            // file all the same.
            Err(VolError::Output(_)) if sink.full => Ok(sink.bytes),
            Err(error) => Err(error.into()),
        }
    }

    /// Stream a file into a host file, synced to its medium, and answer its
    /// length and BLAKE3 digest.
    pub fn copy_into(&self, path: &VolPath, sink: File) -> Result<(u64, blake3::Hash), Error> {
        let (len, digest, file) = self.copy_to(path, sink)?;
        file.sync_all()
            .map_err(|error| Error::host(self.disk.path(), error))?;
        Ok((len, digest))
    }

    /// Stream a file into `sink` without holding it in memory, and answer
    /// its length, BLAKE3 digest, and the sink. A sink that refuses ends the
    /// copy with [`Error::Volume`] carrying the output's error.
    pub fn copy_to<W: Write + Send + 'static>(
        &self,
        path: &VolPath,
        sink: W,
    ) -> Result<(u64, blake3::Hash, W), Error> {
        let (face, engine_path) = file_path(path)?;
        let mut sink = Hashing {
            sink,
            hasher: blake3::Hasher::new(),
        };
        let (copied, sink) = self.call(move |mount| {
            let copied = mount.read_to(face, &engine_path, &mut sink);
            (copied, sink)
        })?;
        let len = copied?;
        Ok((len, sink.hasher.finalize(), sink.sink))
    }

    /// Every object beneath the directory at `path`, depth first, each
    /// directory before what it holds. Bounded by the volume itself: it
    /// holds at most [`crate::bounds::MAX_OBJECTS`] objects.
    pub fn walk(&self, path: &VolPath) -> Result<Vec<Walked>, Error> {
        let face = path.face();
        let engine_path = path.engine_path();
        self.call(move |mount| {
            let mut walked = Vec::new();
            match engine_path {
                Some(engine_path) => {
                    collect(mount, face, &engine_path, "", 0, &mut walked)?;
                }
                // A home is walked through its entries: the engine's paths
                // below a home name at least one component.
                None => match mount.list(face, None)? {
                    volume::Listing::Directory(entries) => {
                        for entry in entries {
                            walked.push(Walked {
                                path: entry.name.clone(),
                                kind: entry.kind.into(),
                                len: entry.len,
                                depth: 1,
                            });
                            if entry.kind == Kind::Directory {
                                collect(mount, face, &entry.name, &entry.name, 1, &mut walked)?;
                            }
                        }
                    }
                    volume::Listing::EmptyHome | volume::Listing::File { .. } => {}
                },
            }
            Ok::<_, VolError>(walked)
        })?
        .map_err(Error::from)
    }

    /// Unmount, releasing the disk's lock, and report how the session ended.
    pub fn close(mut self) -> Result<(), Error> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Result<(), Error> {
        drop(self.jobs.take());
        match self.worker.take().map(thread::JoinHandle::join) {
            Some(Ok(Ok(()))) | None => Ok(()),
            Some(Ok(Err(error))) => Err(error.into()),
            Some(Err(_)) => Err(Error::SessionEnded),
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn file_path(path: &VolPath) -> Result<(u8, String), Error> {
    match path.engine_path() {
        Some(engine_path) if !path.is_top() => Ok((path.face(), engine_path)),
        _ => Err(Error::Path(format!("{path} is a directory, not a file"))),
    }
}

/// Walk `engine_path` (relative below `face`'s home, or absolute), naming
/// what it finds below `prefix` at `depth` levels beneath the walk's start.
fn collect(
    mount: &Mount,
    face: u8,
    engine_path: &str,
    prefix: &str,
    depth: usize,
    walked: &mut Vec<Walked>,
) -> Result<(), VolError> {
    mount.walk(face, engine_path, &mut |entry| {
        walked.push(Walked {
            path: if prefix.is_empty() {
                entry.path.to_owned()
            } else {
                format!("{prefix}/{}", entry.path)
            },
            kind: entry.kind.into(),
            len: entry.len,
            depth: depth + entry.depth,
        });
        Ok(())
    })
}

/// A sink that refuses to grow past its bound.
struct Bounded {
    bytes: Vec<u8>,
    limit: u64,
    exceeded: bool,
}

impl Write for Bounded {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.bytes.len() as u64 + buf.len() as u64 > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("bound reached"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A sink that keeps the first `limit` bytes, then stops the copy.
struct Head {
    bytes: Vec<u8>,
    limit: usize,
    full: bool,
}

impl Write for Head {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = self.limit - self.bytes.len();
        if room == 0 {
            self.full = true;
            return Err(io::Error::other("the head is read"));
        }
        let taken = room.min(buf.len());
        self.bytes.extend_from_slice(&buf[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A sink that digests what passes through it.
struct Hashing<W> {
    sink: W,
    hasher: blake3::Hasher,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.sink.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}
