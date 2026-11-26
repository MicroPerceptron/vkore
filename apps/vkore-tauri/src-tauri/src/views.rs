//! What the window is told about the host and its machines.
//!
//! Every field is a plain value: numbers, enums the engine owns, and strings
//! that went through [`shown`]. Paths are reduced to file names, so the window
//! learns neither the user's directory layout nor anything it could send back.

use std::path::Path;

use serde::Serialize;
use vkore_vm::{AccelChoice, hardware::HostResources};
use vkored::{MachineConfig, config::Writeback};

use crate::runtime::{Line, Refusal, Runtime, Snapshot};
use crate::shown::shown;

/// The most machines one listing returns.
pub const MAX_MACHINES: usize = 256;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    pub cpu_name: String,
    pub physical_cores: usize,
    pub logical_cpus: usize,
    pub memory_mib: u64,
    pub available_memory_mib: u64,
    pub storage_available_mib: u64,
    /// The accelerator this host can offer: `kvm`, `hvf` or `tcg`.
    pub accel: &'static str,
    /// The QEMU in use, by file name; `None` when none was found.
    pub qemu: Option<String>,
    pub running: usize,
    pub max_running: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    pub name: String,
    pub writeback: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineView {
    pub name: String,
    pub iso: String,
    pub iso_sha256: Option<String>,
    pub disk_mib: u64,
    pub cpus: u16,
    pub memory_mib: u32,
    pub framebuffer: (u32, u32),
    pub accel: &'static str,
    pub workspace: Option<WorkspaceView>,
    pub run: Snapshot,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineDetail {
    #[serde(flatten)]
    pub machine: MachineView,
    pub lines: Vec<Line>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| shown(&name.to_string_lossy()))
}

fn accel_name(choice: AccelChoice) -> &'static str {
    match choice {
        AccelChoice::Auto => "auto",
        AccelChoice::Kvm => "kvm",
        AccelChoice::Hvf => "hvf",
        AccelChoice::Tcg => "tcg",
    }
}

pub fn host(runtime: &Runtime) -> Result<HostView, Refusal> {
    let probe = HostResources::probe(runtime.root()).map_err(Refusal::State)?;
    let accel = if probe.acceleration.kvm {
        "kvm"
    } else if probe.acceleration.hvf {
        "hvf"
    } else {
        "tcg"
    };
    Ok(HostView {
        cpu_name: shown(&probe.cpu_name),
        physical_cores: probe.physical_cores,
        logical_cpus: probe.logical_cpus,
        memory_mib: probe.memory_mib,
        available_memory_mib: probe.available_memory_mib,
        storage_available_mib: probe.storage_available_mib,
        accel,
        qemu: runtime.qemu().map(file_name),
        running: runtime.running(),
        max_running: crate::runtime::MAX_RUNNING,
    })
}

fn view(config: &MachineConfig, run: Snapshot) -> MachineView {
    MachineView {
        name: shown(&config.name),
        iso: file_name(&config.iso),
        iso_sha256: config
            .iso_sha256
            .as_deref()
            .map(|digest| shown(digest.get(..12).unwrap_or(digest))),
        disk_mib: config.disk_mib,
        cpus: config.cpus,
        memory_mib: config.memory_mib,
        framebuffer: config.framebuffer,
        accel: accel_name(config.accel),
        workspace: config.workspace.as_ref().map(|workspace| WorkspaceView {
            name: shown(&workspace.name),
            writeback: match workspace.writeback {
                Writeback::Off => "off",
                Writeback::Review => "review",
                Writeback::Automatic => "automatic",
            },
        }),
        run,
    }
}

pub fn machines(runtime: &Runtime) -> Result<Vec<MachineView>, Refusal> {
    let store = runtime.store()?;
    let configs = store
        .list()
        .map_err(|error| Refusal::State(error.to_string()))?;
    Ok(configs
        .iter()
        .take(MAX_MACHINES)
        .map(|config| view(config, runtime.snapshot(&config.name)))
        .collect())
}

pub fn machine(runtime: &Runtime, name: &str) -> Result<MachineDetail, Refusal> {
    if !vkored::config::valid_name(name) {
        return Err(Refusal::BadName);
    }
    let config = runtime
        .store()?
        .get(name)
        .map_err(|error| Refusal::State(error.to_string()))?;
    Ok(MachineDetail {
        machine: view(&config, runtime.snapshot(name)),
        lines: runtime.lines(name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_sees_a_file_name_not_a_path() {
        let mut config = MachineConfig::new("kore", "/Users/someone/private/kore\u{202E}.iso");
        config.workspace = None;
        let shown = view(&config, Snapshot::idle());
        assert_eq!(shown.iso, "kore\\u{202E}.iso");
        let json = serde_json::to_string(&shown).unwrap();
        assert!(
            !json.contains("someone") && !json.contains("private"),
            "{json}"
        );
    }

    #[test]
    fn the_digest_is_a_prefix() {
        let mut config = MachineConfig::new("kore", "kore.iso");
        config.iso_sha256 = Some("0123456789abcdef0123456789abcdef".into());
        assert_eq!(
            view(&config, Snapshot::idle()).iso_sha256.as_deref(),
            Some("0123456789ab")
        );
    }

    #[test]
    fn a_machine_name_is_validated_before_the_store_is_opened() {
        let runtime = Runtime::new("/nonexistent/state".into(), None);
        assert_eq!(machine(&runtime, "../x").unwrap_err(), Refusal::BadName);
    }

    #[test]
    fn a_stored_machine_is_listed_and_fetched_as_the_window_sees_it() {
        let root = std::env::temp_dir().join(format!("vkore-tauri-views-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.clone(), None);
        let store = runtime.store().unwrap();
        let mut config = MachineConfig::new("kore-dev", "/images/kore.iso");
        config.cpus = 2;
        config.memory_mib = 2048;
        store.create(&config).unwrap();

        let listed = machines(&runtime).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "kore-dev");
        assert_eq!(listed[0].iso, "kore.iso");
        assert_eq!(listed[0].run.phase, crate::runtime::Phase::Idle);

        let detail = machine(&runtime, "kore-dev").unwrap();
        assert_eq!(detail.machine.memory_mib, 2048);
        assert!(detail.lines.is_empty());
        let json = serde_json::to_value(&detail).unwrap();
        // The wire shape the front end reads: camelCase, run nested, log beside it.
        assert_eq!(json["memoryMib"], 2048);
        assert_eq!(json["run"]["phase"], "idle");
        assert!(json["lines"].as_array().unwrap().is_empty());

        assert!(matches!(machine(&runtime, "ghost"), Err(Refusal::State(_))));
        // No QEMU, so nothing can start, and nothing is remembered for it.
        assert_eq!(runtime.start("kore-dev", |_| {}), Err(Refusal::NoQemu));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
