use std::{fmt, io, path::PathBuf};

use kore_host::volume::VolError;

/// What a volume operation refused, and why.
#[derive(Debug)]
pub enum Error {
    /// What Kore's engine refused, in its own words: a blank or foreign
    /// disk, a volume a machine holds, a newer format, a name it cannot hold.
    Volume(VolError),
    /// A host file or directory, and what it met.
    Host { path: PathBuf, error: io::Error },
    /// Neither a regular file nor (on Linux) a block device.
    NotADisk(PathBuf),
    /// A name Kore cannot hold (UTF-8, at most 32 bytes, no NUL), or one a
    /// host directory cannot hold safely (`.`, `..`, a separator).
    Name(String),
    /// A path that names no terminal's home, or one a write cannot reach.
    Path(String),
    /// More than the caller's bound.
    TooLarge { what: String, limit: u64 },
    /// The browsing session's thread has stopped.
    SessionEnded,
    /// A host destination exists and replacing it was not asked.
    Exists(PathBuf),
    /// Host files changed since they were staged; nothing was written.
    Conflict(Vec<String>),
    /// A tree the volume's bounds cannot hold, refused before it is copied.
    Bounds(String),
    /// A staging record that does not match what it describes.
    Manifest(String),
}

impl Error {
    pub(crate) fn host(path: impl Into<PathBuf>, error: io::Error) -> Self {
        Self::Host {
            path: path.into(),
            error,
        }
    }

    /// Whether the volume is held by a running machine or another tool.
    pub fn is_in_use(&self) -> bool {
        matches!(self, Self::Volume(error) if error.is_in_use())
    }

    /// Whether the disk carries blank roots: a volume Kore formats at its
    /// first boot.
    pub fn is_blank(&self) -> bool {
        matches!(self, Self::Volume(VolError::Blank))
    }

    /// Whether the engine found nothing at the path asked for.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Volume(VolError::NotFound(_)))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Volume(error) => write!(f, "{error}"),
            Self::Host { path, error } => write!(f, "{}: {error}", path.display()),
            Self::NotADisk(path) => write!(
                f,
                "{}: neither a disk image nor a disk's block device",
                path.display()
            ),
            Self::Name(message) | Self::Path(message) | Self::Bounds(message) => {
                f.write_str(message)
            }
            Self::TooLarge { what, limit } => {
                write!(f, "{what} is larger than the {limit}-byte bound")
            }
            Self::SessionEnded => f.write_str("the volume session has ended"),
            Self::Exists(path) => write!(
                f,
                "{} exists, and replacing it was not asked",
                path.display()
            ),
            Self::Conflict(paths) => write!(
                f,
                "{} host file(s) changed since they were staged ({}); nothing was written",
                paths.len(),
                paths.join(", ")
            ),
            Self::Manifest(message) => write!(f, "staging record: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Volume(error) => Some(error),
            Self::Host { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl From<VolError> for Error {
    fn from(error: VolError) -> Self {
        Self::Volume(error)
    }
}
