use std::fmt;

use kore_host::volume::{Name, is_face};

use crate::{Error, bounds::MAX_NAME_BYTES};

/// Where a [`VolPath`] starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// Terminal `face`'s home (`work/face-N` on the volume): what that
    /// terminal reads and writes, and where installs land.
    Home(u8),
    /// The volume's root. Read-only from the host: Kore installs only under
    /// a terminal's home.
    Root,
}

/// A path on a Kore volume: a [`Place`] and the names below it, each one a
/// name Kore can hold.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VolPath {
    place: Place,
    names: Vec<String>,
}

/// Check `name` as one path component: a name Kore can hold that also
/// addresses exactly one entry through the engine's `/`-separated paths and
/// cannot climb a host directory when copied out.
pub(crate) fn check_name(name: &str) -> Result<(), Error> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(Error::Name(format!(
            "{name:?} is not a name a path can hold (empty, `.`, `..`, or a separator)"
        )));
    }
    if name.len() > MAX_NAME_BYTES || Name::new(name).is_err() {
        return Err(Error::Name(format!(
            "{name:?} is not a kore name (UTF-8, at most {MAX_NAME_BYTES} bytes, no NUL)"
        )));
    }
    Ok(())
}

impl VolPath {
    /// Terminal `face`'s home.
    pub fn home(face: u8) -> Result<Self, Error> {
        if !is_face(face) {
            return Err(Error::Path(format!("face {face} is not a terminal's")));
        }
        Ok(Self {
            place: Place::Home(face),
            names: Vec::new(),
        })
    }

    /// The volume's root.
    pub fn root() -> Self {
        Self {
            place: Place::Root,
            names: Vec::new(),
        }
    }

    /// `text` as terminal `face` types it: below its home, or from the
    /// volume's root when it starts with `/`. Empty, `.` and `/`-only texts
    /// name the home or the root themselves.
    pub fn parse(face: u8, text: &str) -> Result<Self, Error> {
        let (mut path, rest) = match text.strip_prefix('/') {
            Some(rest) => (Self::root(), rest),
            None => (Self::home(face)?, text),
        };
        for part in rest.split('/').filter(|part| !part.is_empty()) {
            if part == "." {
                continue;
            }
            path = path.join(part)?;
        }
        Ok(path)
    }

    /// This path with `name` below it.
    pub fn join(&self, name: &str) -> Result<Self, Error> {
        check_name(name)?;
        if self.names.len() >= crate::bounds::MAX_PATH_DEPTH {
            return Err(Error::Path(format!(
                "{self}/{name} is deeper than kore's {} levels",
                crate::bounds::MAX_PATH_DEPTH
            )));
        }
        let mut names = self.names.clone();
        names.push(name.to_owned());
        Ok(Self {
            place: self.place,
            names,
        })
    }

    /// The directory holding this path, if it is not a home or the root.
    pub fn parent(&self) -> Option<Self> {
        let (_, parents) = self.names.split_last()?;
        Some(Self {
            place: self.place,
            names: parents.to_vec(),
        })
    }

    /// The last name, if this is not a home or the root.
    pub fn name(&self) -> Option<&str> {
        self.names.last().map(String::as_str)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn place(&self) -> Place {
        self.place
    }

    /// Whether this is a home or the root itself.
    pub fn is_top(&self) -> bool {
        self.names.is_empty()
    }

    /// The face a home path belongs to; the root reads the same from every
    /// face.
    pub(crate) fn face(&self) -> u8 {
        match self.place {
            Place::Home(face) => face,
            Place::Root => 0,
        }
    }

    /// The path the engine takes: relative below a home (`None` for the home
    /// itself), absolute from the root.
    pub(crate) fn engine_path(&self) -> Option<String> {
        match self.place {
            Place::Home(_) if self.names.is_empty() => None,
            Place::Home(_) => Some(self.names.join("/")),
            Place::Root => Some(format!("/{}", self.names.join("/"))),
        }
    }

    /// The path the engine takes for a write, which only reaches below a
    /// home.
    pub(crate) fn install_path(&self) -> Result<String, Error> {
        match self.place {
            Place::Home(_) if !self.names.is_empty() => Ok(self.names.join("/")),
            Place::Home(_) => Err(Error::Path(
                "a write names a path below the home, not the home itself".into(),
            )),
            Place::Root => Err(Error::Path(
                "the volume's root is read-only from the host: write below a terminal's home"
                    .into(),
            )),
        }
    }
}

impl fmt::Display for VolPath {
    /// Below a home as terminal N types it (`.` for the home itself); from
    /// the root with a leading `/`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.place {
            Place::Home(_) if self.names.is_empty() => f.write_str("."),
            Place::Home(_) => f.write_str(&self.names.join("/")),
            Place::Root => write!(f, "/{}", self.names.join("/")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_parse_as_a_terminal_types_them() {
        let home = VolPath::parse(0, "").unwrap();
        assert!(home.is_top() && home.place() == Place::Home(0));
        assert_eq!(
            VolPath::parse(0, "./pictures//screenshots/")
                .unwrap()
                .to_string(),
            "pictures/screenshots"
        );
        let root = VolPath::parse(3, "/work/face-0").unwrap();
        assert_eq!(root.place(), Place::Root);
        assert_eq!(root.engine_path().as_deref(), Some("/work/face-0"));
        assert_eq!(VolPath::root().engine_path().as_deref(), Some("/"));
        assert_eq!(home.engine_path(), None);
        assert!(VolPath::parse(0, "a/../b").is_err());
        assert!(VolPath::parse(0, "a\\b").is_err());
        assert!(VolPath::parse(0, "a-name-of-thirty-three-bytes.txt0").is_err());
        assert!(VolPath::home(255).is_err());
    }

    #[test]
    fn writes_reach_below_a_home_only() {
        let home = VolPath::home(1).unwrap();
        assert!(home.install_path().is_err());
        assert_eq!(home.join("js").unwrap().install_path().unwrap(), "js");
        assert!(
            VolPath::root()
                .join("work")
                .unwrap()
                .install_path()
                .is_err()
        );
        let file = home.join("a").unwrap().join("b.txt").unwrap();
        assert_eq!(file.parent().unwrap().to_string(), "a");
        assert_eq!(file.name(), Some("b.txt"));
        assert_eq!(file.face(), 1);
    }

    #[test]
    fn depth_is_bounded_as_the_engine_bounds_it() {
        let mut path = VolPath::home(0).unwrap();
        for _ in 0..crate::bounds::MAX_PATH_DEPTH {
            path = path.join("d").unwrap();
        }
        assert!(matches!(path.join("d"), Err(Error::Path(_))));
    }
}
