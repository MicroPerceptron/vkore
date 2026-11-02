//! Moving files between a volume and the host.
//!
//! Out (`Browser::export`): into a host directory the caller names, and
//! never outside it. The directory is opened as a capability (`cap-std`), so
//! no path below it can climb out; no symbolic link below it is followed;
//! every name is checked as one a host directory holds safely; each file is
//! written beside its destination and published by one rename, so a reader
//! never sees half of it; and an existing file is replaced only when asked.
//!
//! In (`Disk::install`): through Kore's own all-or-nothing publication,
//! which refuses links, special files and names Kore cannot hold before it
//! writes anything, and reads every file back after it commits.

use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use cap_fs_ext::DirExt;
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use kore_host::volume;
use serde::Serialize;

use crate::{Browser, Disk, EntryKind, Error, Listing, VolPath, claim::Claim, path::check_name};

/// How an export treats what the destination already holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Replace existing files of the same name. Directories are merged; a
    /// file never replaces a directory, nor a directory a file.
    pub replace: bool,
}

/// What an export wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Exported {
    /// The host path of the exported file or directory.
    pub destination: PathBuf,
    pub files: usize,
    pub directories: usize,
    pub bytes: u64,
}

/// What an install put on the volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Installed {
    pub files: usize,
    pub directories: usize,
    pub bytes: u64,
    /// The root generation that names the install.
    pub generation: u64,
}

/// What a removal took off the volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Removed {
    /// The root generation that no longer names it.
    pub generation: u64,
}

impl Disk {
    /// Install the host file or tree `source` at `destination` below a
    /// terminal's home; with `replace`, in place of what is there. The
    /// volume must be offline: no machine running on the disk, and no
    /// [`Browser`] of it open in this process.
    pub fn install(
        &self,
        source: &Path,
        destination: &VolPath,
        replace: bool,
    ) -> Result<Installed, Error> {
        let path = destination.install_path()?;
        let _claim = Claim::take(self)?;
        let installed = volume::put(
            self.path(),
            self.block_size(),
            destination.face(),
            source,
            &path,
            replace,
        )?;
        Ok(Installed {
            files: installed.files,
            directories: installed.directories,
            bytes: installed.bytes,
            generation: installed.after,
        })
    }

    /// Remove `path` (and what it holds) from below a terminal's home.
    pub fn remove(&self, path: &VolPath) -> Result<Removed, Error> {
        let engine_path = path.install_path()?;
        let _claim = Claim::take(self)?;
        let removed = volume::rm(self.path(), self.block_size(), path.face(), &engine_path)?;
        Ok(Removed {
            generation: removed.after,
        })
    }
}

impl Browser {
    /// Copy `source` — a file, or a directory with everything below it —
    /// into the host directory `destination` under its own name (a home
    /// exports as `face-N`, the root as `volume`), and answer what was
    /// written.
    pub fn export(
        &self,
        source: &VolPath,
        destination: &Path,
        options: ExportOptions,
    ) -> Result<Exported, Error> {
        let name = match (source.name(), source.place()) {
            (Some(name), _) => name.to_owned(),
            (None, crate::Place::Home(face)) => format!("face-{face}"),
            (None, crate::Place::Root) => "volume".to_owned(),
        };
        check_name(&name)?;
        let into = Dir::open_ambient_dir(destination, ambient_authority())
            .map_err(|error| Error::host(destination, error))?;
        let mut exported = Exported {
            destination: destination.join(&name),
            ..Exported::default()
        };
        match self.list(source)? {
            Listing::File { len } => {
                exported.bytes =
                    self.export_file(source, len, &into, &name, destination, options)?;
                exported.files = 1;
            }
            Listing::Directory { .. } | Listing::EmptyHome => {
                let top = open_or_create_dir(&into, &name, destination)?;
                exported.directories = 1;
                let below = destination.join(&name);
                for entry in self.walk(source)? {
                    let parts = entry.path.split('/').collect::<Vec<_>>();
                    for part in &parts {
                        check_name(part)?;
                    }
                    let (last, parents) = parts.split_last().ok_or_else(|| {
                        Error::Path(format!("{source}: an entry with an empty path"))
                    })?;
                    let parent_path = below.join(parents.join("/"));
                    let parent = if parents.is_empty() {
                        top.try_clone()
                            .map_err(|error| Error::host(&below, error))?
                    } else {
                        open_dir_within(&top, parents, &below)?
                    };
                    let mut volume_path = source.clone();
                    for part in &parts {
                        volume_path = volume_path.join(part)?;
                    }
                    match entry.kind {
                        EntryKind::Directory => {
                            open_or_create_dir(&parent, last, &parent_path)?;
                            exported.directories += 1;
                        }
                        EntryKind::File => {
                            exported.bytes += self.export_file(
                                &volume_path,
                                entry.len,
                                &parent,
                                last,
                                &parent_path,
                                options,
                            )?;
                            exported.files += 1;
                        }
                    }
                }
            }
        }
        Ok(exported)
    }

    /// Write one file beside its destination, then publish it by one
    /// rename; answer its length.
    fn export_file(
        &self,
        source: &VolPath,
        len: u64,
        into: &Dir,
        name: &str,
        host: &Path,
        options: ExportOptions,
    ) -> Result<u64, Error> {
        let shown = host.join(name);
        refuse_link(into, name, &shown)?;
        if !options.replace && exists(into, name, &shown)? {
            return Err(Error::Exists(shown));
        }
        let partial = partial_name(name);
        let file = into
            .open_with(&partial, OpenOptions::new().write(true).create_new(true))
            .map_err(|error| Error::host(host.join(&partial), error))?
            .into_std();
        let published = self.copy_into(source, file).and_then(|(written, _)| {
            if written != len {
                return Err(Error::Path(format!(
                    "{source}: {written} bytes read, {len} listed"
                )));
            }
            publish(into, &partial, name, &shown, options.replace)?;
            Ok(written)
        });
        if published.is_err() {
            let _ = into.remove_file(&partial);
        }
        published
    }
}

/// A name for a file being written beside `name`, unique in this process.
pub(crate) fn partial_name(name: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        ".{name}.vkore-{}-{}.partial",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

pub(crate) fn exists(dir: &Dir, name: &str, shown: &Path) -> Result<bool, Error> {
    match dir.symlink_metadata(name) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::host(shown, error)),
    }
}

/// Refuse a destination entry that is a symbolic link: an export writes
/// into the directory it was given, never where a link points.
pub(crate) fn refuse_link(dir: &Dir, name: &str, shown: &Path) -> Result<(), Error> {
    match dir.symlink_metadata(name) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(Error::Path(format!(
            "{} is a symbolic link; an export does not follow links",
            shown.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::host(shown, error)),
    }
}

/// Publish `partial` as `name`: a replacing rename when asked, else a hard
/// link that refuses an existing `name` (then the partial is removed), so a
/// file that appeared meanwhile is never clobbered. A filesystem without
/// hard links falls back to a checked rename.
pub(crate) fn publish(
    dir: &Dir,
    partial: &str,
    name: &str,
    shown: &Path,
    replace: bool,
) -> Result<(), Error> {
    if replace {
        return dir
            .rename(partial, dir, name)
            .map_err(|error| Error::host(shown, error));
    }
    match dir.hard_link(partial, dir, name) {
        Ok(()) => dir
            .remove_file(partial)
            .map_err(|error| Error::host(shown, error)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(Error::Exists(shown.to_path_buf()))
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
            ) =>
        {
            if exists(dir, name, shown)? {
                return Err(Error::Exists(shown.to_path_buf()));
            }
            dir.rename(partial, dir, name)
                .map_err(|error| Error::host(shown, error))
        }
        Err(error) => Err(Error::host(shown, error)),
    }
}

/// Open the directory `name` in `dir`, creating it if absent (directories
/// merge). A link or a non-directory of that name is refused.
fn open_or_create_dir(dir: &Dir, name: &str, host: &Path) -> Result<Dir, Error> {
    let shown = host.join(name);
    refuse_link(dir, name, &shown)?;
    match dir.create_dir(name) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = dir
                .symlink_metadata(name)
                .map_err(|error| Error::host(&shown, error))?;
            if !metadata.is_dir() {
                return Err(Error::Exists(shown));
            }
        }
        Err(error) => return Err(Error::host(&shown, error)),
    }
    dir.open_dir_nofollow(name)
        .map_err(|error| Error::host(&shown, error))
}

/// Open `parts` below `top`, one directory at a time, following no link.
pub(crate) fn open_dir_within(top: &Dir, parts: &[&str], host: &Path) -> Result<Dir, Error> {
    let mut at = top.try_clone().map_err(|error| Error::host(host, error))?;
    let mut shown = host.to_path_buf();
    for part in parts {
        shown.push(part);
        at = at
            .open_dir_nofollow(part)
            .map_err(|error| Error::host(&shown, error))?;
    }
    Ok(at)
}
