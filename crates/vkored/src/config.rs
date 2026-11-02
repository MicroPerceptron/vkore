//! A machine's configuration, kept under vKore's state directory.
//!
//! Paths in a configuration are ceilings, never authority: a workspace root
//! names what may be staged into the machine and the bounds of what may come
//! back, and nothing the guest does can widen either.

use std::{
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use vkore_vm::{AccelChoice, Vendor};

use crate::state::{State, private_dir};

/// The configuration format this vKore writes and reads.
pub const SCHEMA_VERSION: u32 = 2;

/// What happens to a project's changes when its machine stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Writeback {
    /// Nothing comes back; the machine's copy is its own.
    Off,
    /// Changes are collected and shown; `vkore machine apply` applies them.
    #[default]
    Review,
    /// Changes within the bounds are applied when the machine stops.
    Automatic,
}

/// A host project staged into a terminal's home.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    /// The project's directory name in the terminal's home.
    pub name: String,
    /// The terminal whose home receives it.
    #[serde(default)]
    pub face: u8,
    /// Names left out at any depth.
    pub exclude: Vec<String>,
    pub writeback: Writeback,
    /// The most files and bytes one stop may write back.
    pub max_files: usize,
    pub max_bytes: u64,
}

/// One machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineConfig {
    pub schema_version: u32,
    pub name: String,
    /// The Kore ISO it boots: the caller's artifact.
    pub iso: PathBuf,
    /// The SHA-256 the ISO had when the machine last booted it.
    #[serde(default)]
    pub iso_sha256: Option<String>,
    pub disk_mib: u64,
    pub cpus: u16,
    pub memory_mib: u32,
    /// The IOMMU vendor; absent, the host's own (so it can accelerate).
    #[serde(default)]
    pub vendor: Option<Vendor>,
    #[serde(default)]
    pub accel: AccelChoice,
    pub framebuffer: (u32, u32),
    #[serde(default)]
    pub workspace: Option<WorkspaceConfig>,
    /// UEFI firmware, when not the one QEMU's descriptors name.
    #[serde(default)]
    pub firmware: Option<FirmwareConfig>,
    /// Make sure terminal 0's home has `pictures/screenshots` before each
    /// boot: Kore's capture writer saves only into an existing folder, so
    /// without it PrintScreen saves nothing (the same as typing
    /// `make-directory pictures/screenshots` in the guest).
    #[serde(default = "yes")]
    pub screenshots_folder: bool,
}

fn yes() -> bool {
    true
}

/// An explicitly chosen UEFI firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareConfig {
    pub code: PathBuf,
    pub vars_template: PathBuf,
    pub format: String,
}

impl MachineConfig {
    /// A machine named `name` booting `iso`, with the gate's resources.
    pub fn new(name: impl Into<String>, iso: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            iso: iso.into(),
            iso_sha256: None,
            disk_mib: 4096,
            cpus: 4,
            memory_mib: 4096,
            vendor: None,
            accel: AccelChoice::Auto,
            framebuffer: (1280, 800),
            workspace: None,
            firmware: None,
            screenshots_folder: true,
        }
    }
}

/// What the store refused.
#[derive(Debug)]
pub enum StoreError {
    Invalid(String),
    NotFound(String),
    Exists(String),
    Io(PathBuf, std::io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::NotFound(name) => write!(f, "no machine named {name:?}"),
            Self::Exists(name) => write!(f, "a machine named {name:?} exists"),
            Self::Io(path, error) => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for StoreError {}

/// Machines, by name.
pub struct MachineStore {
    state: State,
}

/// Whether `name` can name a machine (and so a directory and a QEMU name).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 48
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !name.starts_with('-')
}

/// A warning for a workspace root that exposes more than a project should:
/// a filesystem root, a whole home directory, or a credential store.
pub fn broad_root_warning(root: &Path) -> Option<String> {
    if root.parent().is_none() {
        return Some("a filesystem root exposes an unusually broad scope".into());
    }
    if std::env::var_os("HOME").is_some_and(|home| home == root.as_os_str()) {
        return Some("an entire home directory may hold credentials and private data".into());
    }
    let sensitive = [".ssh", ".aws", ".azure", ".gnupg", ".kube", ".config"];
    if root.components().any(|component| {
        matches!(component, Component::Normal(value)
            if sensitive.iter().any(|name| value.eq_ignore_ascii_case(name)))
    }) {
        return Some("this directory commonly holds authentication credentials".into());
    }
    None
}

impl MachineStore {
    pub fn new(state: State) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Every machine's configuration, by name.
    pub fn list(&self) -> Result<Vec<MachineConfig>, StoreError> {
        let dir = self.state.machines();
        let mut machines = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|error| StoreError::Io(dir.clone(), error))? {
            let entry = entry.map_err(|error| StoreError::Io(dir.clone(), error))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if valid_name(&name) && self.state.machine(&name).config.is_file() {
                machines.push(self.get(&name)?);
            }
        }
        machines.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(machines)
    }

    pub fn get(&self, name: &str) -> Result<MachineConfig, StoreError> {
        if !valid_name(name) {
            return Err(StoreError::NotFound(name.into()));
        }
        let path = self.state.machine(name).config;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound(name.into()));
            }
            Err(error) => return Err(StoreError::Io(path, error)),
        };
        let config: MachineConfig = serde_json::from_slice(&bytes)
            .map_err(|error| StoreError::Invalid(format!("{}: {error}", path.display())))?;
        if config.name != name {
            return Err(StoreError::Invalid(format!(
                "{}: holds {:?}",
                path.display(),
                config.name
            )));
        }
        // The file is the user's to edit: what is read is held to what is
        // written.
        validate(&config).map_err(|error| match error {
            StoreError::Invalid(message) => {
                StoreError::Invalid(format!("{}: {message}", path.display()))
            }
            other => other,
        })?;
        Ok(config)
    }

    /// Create a machine; its name must be new.
    pub fn create(&self, config: &MachineConfig) -> Result<(), StoreError> {
        validate(config)?;
        let paths = self.state.machine(&config.name);
        if paths.config.exists() {
            return Err(StoreError::Exists(config.name.clone()));
        }
        private_dir(&paths.dir).map_err(|error| StoreError::Io(paths.dir.clone(), error))?;
        self.save(config)
    }

    /// Replace a machine's configuration in one rename.
    pub fn save(&self, config: &MachineConfig) -> Result<(), StoreError> {
        validate(config)?;
        let paths = self.state.machine(&config.name);
        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        let partial = paths.dir.join(".machine.json.partial");
        fs::write(&partial, bytes).map_err(|error| StoreError::Io(partial.clone(), error))?;
        fs::rename(&partial, &paths.config).map_err(|error| StoreError::Io(paths.config, error))
    }

    /// Remove a machine and everything vKore kept for it, its disk included.
    pub fn remove(&self, name: &str) -> Result<(), StoreError> {
        self.get(name)?;
        let paths = self.state.machine(name);
        let _ = fs::remove_file(&paths.qmp);
        fs::remove_dir_all(&paths.dir).map_err(|error| StoreError::Io(paths.dir, error))
    }
}

/// What a configuration must hold before it is written.
pub fn validate(config: &MachineConfig) -> Result<(), StoreError> {
    let invalid = |message: String| Err(StoreError::Invalid(message));
    if config.schema_version != SCHEMA_VERSION {
        return invalid(format!(
            "schema {} (this vKore writes {SCHEMA_VERSION})",
            config.schema_version
        ));
    }
    if !valid_name(&config.name) {
        return invalid(format!(
            "{:?}: a machine name is 1 to 48 letters, digits, '-' or '_'",
            config.name
        ));
    }
    if !config.iso.is_absolute() {
        return invalid(format!(
            "{}: name the ISO by an absolute path",
            config.iso.display()
        ));
    }
    if !(16..=1 << 22).contains(&config.disk_mib) {
        return invalid(format!("a {} MiB disk: 16 MiB to 4 TiB", config.disk_mib));
    }
    if let Some(workspace) = &config.workspace {
        if !workspace.root.is_absolute() {
            return invalid(format!(
                "{}: name the workspace by an absolute path",
                workspace.root.display()
            ));
        }
        if vkore_volume::VolPath::home(workspace.face)
            .and_then(|home| home.join(&workspace.name))
            .is_err()
        {
            return invalid(format!(
                "{:?} in terminal {}'s home: a kore name (at most 32 bytes) in a terminal's home",
                workspace.name, workspace.face
            ));
        }
        if workspace.writeback != Writeback::Off
            && (workspace.max_files == 0 || workspace.max_bytes == 0)
        {
            return invalid("writeback needs nonzero file and byte bounds".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, MachineStore) {
        let dir = tempfile::tempdir().unwrap();
        let state = State::open(dir.path().join("state")).unwrap();
        (dir, MachineStore::new(state))
    }

    #[test]
    fn machines_are_created_listed_replaced_and_removed() {
        let (_dir, store) = store();
        let mut config = MachineConfig::new("demo", "/isos/kore.iso");
        store.create(&config).unwrap();
        assert!(matches!(store.create(&config), Err(StoreError::Exists(_))));
        config.cpus = 2;
        store.save(&config).unwrap();
        assert_eq!(store.get("demo").unwrap().cpus, 2);
        assert_eq!(store.list().unwrap(), [config]);
        store.remove("demo").unwrap();
        assert!(matches!(store.get("demo"), Err(StoreError::NotFound(_))));
        assert!(store.list().unwrap().is_empty());
    }

    #[test]
    fn what_cannot_name_or_bound_a_machine_is_refused() {
        let (_dir, store) = store();
        for name in ["", "a b", "../up", "-flag", &"x".repeat(49)] {
            assert!(
                store.create(&MachineConfig::new(name, "/k.iso")).is_err(),
                "{name:?}"
            );
        }
        assert!(
            store
                .create(&MachineConfig::new("rel", "kore.iso"))
                .is_err()
        );
        let mut config = MachineConfig::new("ws", "/k.iso");
        config.workspace = Some(WorkspaceConfig {
            root: "/projects/demo".into(),
            name: "a-name-of-thirty-three-bytes-long".into(),
            face: 0,
            exclude: Vec::new(),
            writeback: Writeback::Review,
            max_files: 10,
            max_bytes: 10,
        });
        assert!(store.create(&config).is_err());
        let workspace = config.workspace.as_mut().unwrap();
        workspace.name = "demo".into();
        workspace.max_files = 0;
        assert!(store.create(&config).is_err());
        config.workspace.as_mut().unwrap().writeback = Writeback::Off;
        store.create(&config).unwrap();
    }

    #[test]
    fn a_configuration_edited_out_of_bounds_is_refused_when_read() {
        let (_dir, store) = store();
        store
            .create(&MachineConfig::new("demo", "/isos/kore.iso"))
            .unwrap();
        let path = store.state().machine("demo").config;
        let edit = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            change(&mut value);
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        };
        edit(&|value| value["iso"] = "relative.iso".into());
        assert!(matches!(store.get("demo"), Err(StoreError::Invalid(_))));
        // And a listing says so, rather than handing it on.
        assert!(store.list().is_err());
        edit(&|value| {
            value["iso"] = "/isos/kore.iso".into();
            value["disk_mib"] = (1u64 << 40).into();
        });
        assert!(matches!(store.get("demo"), Err(StoreError::Invalid(_))));
        edit(&|value| value["disk_mib"] = 4096.into());
        store.get("demo").unwrap();
        edit(&|value| value["name"] = "other".into());
        assert!(matches!(store.get("demo"), Err(StoreError::Invalid(_))));
    }

    #[test]
    fn broad_roots_are_named() {
        assert!(broad_root_warning(Path::new("/")).is_some());
        assert!(broad_root_warning(Path::new("/home/me/.ssh")).is_some());
        assert!(broad_root_warning(Path::new("/home/me/projects/kore")).is_none());
    }
}
