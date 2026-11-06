//! Where vKore keeps its state: one private directory per machine.
//!
//! ```text
//! <state>/machines/<name>/machine.json   the configuration
//!                         disk.raw       the machine's NVMe: its user volume
//!                         usb.raw        the blank stick Kore enumerates
//!                         vars.fd        its own UEFI variable store
//!                         serial.log     the last boot's console
//!                         debugcon.log   the kernel's emergency port
//!                         qemu.log       what QEMU said
//!                         manifest.json  the staged project's record
//!                         runs/          one event transcript per boot
//!                         scratch/       staging and collected changes
//! ```
//!
//! QMP sockets live under the runtime directory when the host has one
//! (`$XDG_RUNTIME_DIR/vkore`), so their paths stay within `sun_path` however
//! deep the state directory is.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
};

/// vKore's state directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    root: PathBuf,
}

impl State {
    /// The state directory at `root`, created private (0700) if absent.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        private_dir(&root)?;
        private_dir(&root.join("machines"))?;
        Ok(Self { root })
    }

    /// The default state directory: `$VKORE_STATE`, else
    /// `$XDG_STATE_HOME/vkore`, else `~/.local/state/vkore` (Linux) or
    /// `~/Library/Application Support/vkore` (macOS).
    pub fn default_root() -> Option<PathBuf> {
        if let Some(state) = std::env::var_os("VKORE_STATE") {
            return Some(state.into());
        }
        if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
            return Some(PathBuf::from(state).join("vkore"));
        }
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(if cfg!(target_os = "macos") {
            home.join("Library/Application Support/vkore")
        } else {
            home.join(".local/state/vkore")
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn machines(&self) -> PathBuf {
        self.root.join("machines")
    }

    /// The paths of machine `name` (which the store has validated).
    pub fn machine(&self, name: &str) -> MachinePaths {
        let dir = self.machines().join(name);
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(|runtime| PathBuf::from(runtime).join("vkore"))
            .filter(|runtime| private_dir(runtime).is_ok())
            .unwrap_or_else(|| dir.clone());
        MachinePaths {
            config: dir.join("machine.json"),
            disk: dir.join("disk.raw"),
            usb_stick: dir.join("usb.raw"),
            vars: dir.join("vars.fd"),
            serial: dir.join("serial.log"),
            debugcon: dir.join("debugcon.log"),
            qemu_log: dir.join("qemu.log"),
            manifest: dir.join("manifest.json"),
            runs: dir.join("runs"),
            scratch: dir.join("scratch"),
            qmp: runtime.join(format!("{name}.qmp")),
            dir,
        }
    }
}

/// One machine's files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachinePaths {
    pub dir: PathBuf,
    pub config: PathBuf,
    pub disk: PathBuf,
    pub usb_stick: PathBuf,
    pub vars: PathBuf,
    pub serial: PathBuf,
    pub debugcon: PathBuf,
    pub qemu_log: PathBuf,
    pub manifest: PathBuf,
    pub runs: PathBuf,
    pub scratch: PathBuf,
    pub qmp: PathBuf,
}

/// Create `path` (and its parents) and make it private to this user.
pub fn private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_s_files_live_in_its_own_private_directory() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::open(dir.path().join("state")).unwrap();
        let mode = fs::metadata(state.machines()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let paths = state.machine("demo");
        assert_eq!(paths.disk, state.machines().join("demo/disk.raw"));
        assert!(paths.qmp.to_str().unwrap().ends_with("demo.qmp"));
    }
}
