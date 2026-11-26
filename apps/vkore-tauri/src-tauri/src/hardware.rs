//! A machine's hardware: what it asks of the host, checked and saved by the engine.
//!
//! Every rule lives in `vkored::hardware` (`validate_host`, `save_authorized`) and
//! `vkore-vm`, so the window cannot accept what the CLI or the iced desktop would
//! refuse. This module reads a machine and the host it would run on into plain
//! values, applies a draft to a copy of the machine's configuration, and hands the
//! copy back to the engine to check (without asking for anything) or to save (which
//! may ask polkit, through the desktop's authentication agent, for the permission
//! the memory or hugepage choice needs, and waits for the answer).
//!
//! What stays fixed: the disk's size and storage reservation (the engine does not
//! resize a disk after it is created) and assigned devices, which are shown, not
//! edited, until a device picker exists here. A saved draft keeps both as they are.

use serde::{Deserialize, Serialize};
use vkore_vm::hardware::{GuestNuma, HostResources, NetworkMode, PageSize};
use vkored::MachineConfig;

use crate::runtime::{Refusal, Runtime};
use crate::shown::shown;

/// The most cores a view lists.
const MAX_CORES: usize = 256;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreView {
    pub id: u32,
    pub kind: String,
    /// Host logical CPUs of this core: one, or more with SMT.
    pub threads: Vec<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolView {
    pub page: &'static str,
    pub free_mib: u64,
}

/// What this host can offer a machine's hardware.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostHardware {
    pub logical_cpus: usize,
    pub physical_cores: usize,
    pub cores: Vec<CoreView>,
    pub memory_mib: u64,
    pub available_memory_mib: u64,
    pub cpu_pinning: bool,
    pub memory_pinning: bool,
    pub hugetlb: bool,
    pub pools: Vec<PoolView>,
    /// Native CPU waits can be used here, and why not when they cannot.
    pub native_waits_error: Option<String>,
}

/// The parts of a machine's hardware the window edits.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub cpus: u16,
    pub memory_mib: u32,
    pub pin_cpus: bool,
    pub host_cpus: Vec<u32>,
    pub cpu_power_management: bool,
    pub pin_memory: bool,
    /// `"system"`, `"2m"` or `"1g"`.
    pub pages: String,
    /// `"isolated"` or `"e1000e"`.
    pub network: String,
    /// `"single"` or `"split"`.
    pub guest_numa: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareView {
    pub machine: String,
    pub draft: Draft,
    pub min_cpus: u16,
    pub min_memory_mib: u32,
    pub max_cpus: u16,
    pub disk_mib: u64,
    pub reserve_storage: bool,
    /// Assigned accelerators and GPU functions: shown, not edited here.
    pub devices: usize,
    pub host: HostHardware,
}

/// The engine's verdict on a draft, without asking for anything.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub ok: bool,
    pub message: String,
    /// Saving, and then launching, may ask the desktop's authentication agent.
    pub needs_authorization: bool,
}

fn pages_name(pages: PageSize) -> &'static str {
    match pages {
        PageSize::System => "system",
        PageSize::Huge2M => "2m",
        PageSize::Huge1G => "1g",
    }
}

fn pages_from(name: &str) -> Result<PageSize, Refusal> {
    match name {
        "system" => Ok(PageSize::System),
        "2m" => Ok(PageSize::Huge2M),
        "1g" => Ok(PageSize::Huge1G),
        _ => Err(Refusal::State("unknown page size".into())),
    }
}

fn draft_of(config: &MachineConfig) -> Draft {
    let r = &config.resources;
    Draft {
        cpus: config.cpus,
        memory_mib: config.memory_mib,
        pin_cpus: r.pin_cpus,
        host_cpus: r.host_cpus.clone(),
        cpu_power_management: r.cpu_power_management,
        pin_memory: r.pin_memory,
        pages: pages_name(r.pages).into(),
        network: match r.network {
            NetworkMode::Isolated => "isolated",
            NetworkMode::E1000e => "e1000e",
        }
        .into(),
        guest_numa: match r.guest_numa {
            GuestNuma::Single => "single",
            GuestNuma::Split => "split",
        }
        .into(),
    }
}

/// `config` with `draft` applied. Disk, storage reservation and devices are kept.
fn applied(config: &MachineConfig, draft: &Draft) -> Result<MachineConfig, Refusal> {
    let mut next = config.clone();
    next.cpus = draft.cpus;
    next.memory_mib = draft.memory_mib;
    let r = &mut next.resources;
    r.pin_cpus = draft.pin_cpus;
    r.host_cpus = if draft.pin_cpus {
        draft.host_cpus.clone()
    } else {
        Vec::new()
    };
    r.cpu_power_management = draft.cpu_power_management;
    r.pin_memory = draft.pin_memory;
    r.pages = pages_from(&draft.pages)?;
    r.network = match draft.network.as_str() {
        "isolated" => NetworkMode::Isolated,
        "e1000e" => NetworkMode::E1000e,
        _ => return Err(Refusal::State("unknown network mode".into())),
    };
    r.guest_numa = match draft.guest_numa.as_str() {
        "single" => GuestNuma::Single,
        "split" => GuestNuma::Split,
        _ => return Err(Refusal::State("unknown guest topology".into())),
    };
    Ok(next)
}

fn host_view(host: &HostResources) -> HostHardware {
    HostHardware {
        logical_cpus: host.logical_cpus,
        physical_cores: host.physical_cores,
        cores: host
            .cores
            .iter()
            .take(MAX_CORES)
            .map(|core| CoreView {
                id: core.id,
                kind: shown(&core.kind),
                threads: core.threads.clone(),
            })
            .collect(),
        memory_mib: host.memory_mib,
        available_memory_mib: host.available_memory_mib,
        cpu_pinning: host.cpu_pinning,
        memory_pinning: host.memory_pinning,
        hugetlb: host.hugetlb_supported,
        pools: host
            .hugepages
            .iter()
            .map(|pool| PoolView {
                page: pages_name(pool.page),
                free_mib: pool.free_mib,
            })
            .collect(),
        native_waits_error: host
            .cpu_power_management
            .clone()
            .err()
            .map(|error| shown(&error)),
    }
}

/// A machine's disk belongs to its worker while it runs, and so does its hardware.
fn stopped(runtime: &Runtime, name: &str) -> Result<(), Refusal> {
    if !vkored::config::valid_name(name) {
        return Err(Refusal::BadName);
    }
    if runtime.is_active(name) {
        return Err(Refusal::State(
            "stop the machine before changing its hardware".into(),
        ));
    }
    Ok(())
}

pub fn get(runtime: &Runtime, name: &str) -> Result<HardwareView, Refusal> {
    if !vkored::config::valid_name(name) {
        return Err(Refusal::BadName);
    }
    let config = runtime
        .store()?
        .get(name)
        .map_err(|error| Refusal::State(error.to_string()))?;
    let host = HostResources::probe(runtime.root()).map_err(Refusal::State)?;
    Ok(HardwareView {
        machine: shown(&config.name),
        draft: draft_of(&config),
        min_cpus: config.requirements.cpus,
        min_memory_mib: config.requirements.memory_mib,
        max_cpus: u16::try_from(host.logical_cpus.min(128)).unwrap_or(128),
        disk_mib: config.disk_mib,
        reserve_storage: config.resources.reserve_storage,
        devices: config.resources.passthrough.len() + config.resources.gpu_vfs.len(),
        host: host_view(&host),
    })
}

/// Whether `draft` would be accepted, and whether saving it needs permission.
pub fn check(runtime: &Runtime, name: &str, draft: &Draft) -> Result<Check, Refusal> {
    stopped(runtime, name)?;
    let config = runtime
        .store()?
        .get(name)
        .map_err(|error| Refusal::State(error.to_string()))?;
    let next = applied(&config, draft)?;
    let host = HostResources::probe(runtime.root()).map_err(Refusal::State)?;
    Ok(match vkored::hardware::validate_host(&next, &host) {
        Ok(()) => Check {
            ok: true,
            message: String::new(),
            needs_authorization: vkored::hardware::uses_guardian(&next)
                || vkored::hardware::needs_authorization(&next, &host),
        },
        Err(message) => Check {
            ok: false,
            message: shown(&message),
            needs_authorization: false,
        },
    })
}

/// Save `draft`. This holds the machine's session lease through validation,
/// authorization (which can wait on a person) and persistence, and a refused request
/// never changes what is saved.
pub fn save(runtime: &Runtime, name: &str, draft: &Draft) -> Result<(), Refusal> {
    stopped(runtime, name)?;
    let store = runtime.store()?;
    let config = store
        .get(name)
        .map_err(|error| Refusal::State(error.to_string()))?;
    let next = applied(&config, draft)?;
    vkored::hardware::save_authorized(
        &store,
        name,
        next.cpus,
        next.memory_mib,
        config.disk_mib,
        next.resources,
    )
    .map_err(Refusal::State)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(tag: &str) -> (std::path::PathBuf, Runtime) {
        let root =
            std::env::temp_dir().join(format!("vkore-tauri-hw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = Runtime::new(root.clone(), None);
        runtime
            .store()
            .unwrap()
            .create(&MachineConfig::new("kore-dev", "/images/kore.iso"))
            .unwrap();
        (root, runtime)
    }

    #[test]
    fn a_machine_and_its_host_are_read_as_the_page_expects() {
        let (root, runtime) = machine("view");
        let view = get(&runtime, "kore-dev").unwrap();
        let wire = serde_json::to_value(&view).unwrap();
        crate::assert_camel_keys(&wire);
        assert_eq!(wire["draft"]["cpus"], 4);
        assert_eq!(wire["draft"]["pages"], "system");
        assert_eq!(wire["draft"]["network"], "isolated");
        assert!(wire["host"]["logicalCpus"].as_u64().unwrap() >= 1);
        assert_eq!(view.devices, 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_engine_decides_what_a_draft_may_ask_for() {
        let (root, runtime) = machine("check");
        let draft = get(&runtime, "kore-dev").unwrap().draft;
        let ok = check(&runtime, "kore-dev", &draft).unwrap();
        crate::assert_camel_keys(&serde_json::to_value(&ok).unwrap());
        assert!(ok.ok, "{}", ok.message);
        assert!(!ok.needs_authorization);

        // More guest CPUs than the host has, fewer than the image needs, and pinning
        // with no host CPU chosen are each refused, in the engine's words.
        let mut bad = draft.clone();
        bad.cpus = 128;
        let refused = check(&runtime, "kore-dev", &bad).unwrap();
        assert!(!refused.ok && !refused.message.is_empty());
        let mut bad = draft.clone();
        bad.pin_cpus = true;
        bad.host_cpus = vec![];
        assert!(!check(&runtime, "kore-dev", &bad).unwrap().ok);
        let mut bad = draft.clone();
        bad.pages = "huge".into();
        assert!(matches!(
            check(&runtime, "kore-dev", &bad),
            Err(Refusal::State(_))
        ));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_saved_draft_changes_only_the_hardware_it_names() {
        let (root, runtime) = machine("save");
        let before = runtime.store().unwrap().get("kore-dev").unwrap();
        let mut draft = draft_of(&before);
        draft.cpus = 2;
        draft.memory_mib = 2048;
        draft.network = "e1000e".into();
        save(&runtime, "kore-dev", &draft).unwrap();
        let after = runtime.store().unwrap().get("kore-dev").unwrap();
        assert_eq!((after.cpus, after.memory_mib), (2, 2048));
        assert_eq!(after.resources.network, NetworkMode::E1000e);
        // Everything it did not name is as it was.
        assert_eq!(after.disk_mib, before.disk_mib);
        assert_eq!(after.iso, before.iso);
        assert_eq!(
            after.resources.reserve_storage,
            before.resources.reserve_storage
        );
        assert_eq!(after.name, before.name);

        // A refused draft changes nothing.
        let mut bad = draft.clone();
        bad.cpus = 128;
        assert!(save(&runtime, "kore-dev", &bad).is_err());
        assert_eq!(runtime.store().unwrap().get("kore-dev").unwrap().cpus, 2);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_running_machine_cannot_have_its_hardware_changed() {
        let (root, runtime) = machine("busy");
        let draft = get(&runtime, "kore-dev").unwrap().draft;
        runtime.mark_active_for_test("kore-dev");
        assert!(matches!(
            check(&runtime, "kore-dev", &draft),
            Err(Refusal::State(_))
        ));
        assert!(matches!(
            save(&runtime, "kore-dev", &draft),
            Err(Refusal::State(_))
        ));
        assert_eq!(get(&runtime, "../x").unwrap_err(), Refusal::BadName);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
