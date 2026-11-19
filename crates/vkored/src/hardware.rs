//! Resource policy shared by the desktop, imports, saved configurations and
//! launch preflight. The view edits values; it never defines admission policy.
use crate::{MachineConfig, MachineStore, config::StoreError};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};
use vkore_vm::hardware::{HostResources, PageSize, Resources};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirements {
    pub cpus: u16,
    pub memory_mib: u32,
    pub disk_mib: u64,
}
impl Default for Requirements {
    fn default() -> Self {
        Self {
            cpus: 2,
            memory_mib: 2048,
            disk_mib: 1024,
        }
    }
}
impl Requirements {
    pub fn validate(self) -> Result<(), String> {
        if !(2..=128).contains(&self.cpus)
            || !(2048..=1 << 20).contains(&self.memory_mib)
            || !(1024..=1 << 22).contains(&self.disk_mib)
        {
            return Err("image requirements must be within 2–128 CPUs, 2 GiB–1 TiB RAM and 1 GiB–4 TiB storage".into());
        }
        Ok(())
    }
}

pub fn defaults(host: &HostResources, minimum: Requirements) -> (u16, u32, u64, Resources) {
    let cpus = (host.physical_cores / 2).clamp(2, 128) as u16;
    let cpus = cpus.max(minimum.cpus);
    // Both guest NUMA nodes must be aligned to the selected physical page size.
    let memory = ((host.memory_mib / 2).clamp(2048, 1 << 20) as u32)
        .max(minimum.memory_mib)
        .div_ceil(4)
        * 4;
    let pages = if host.huge_available(PageSize::Huge2M, memory) {
        PageSize::Huge2M
    } else {
        PageSize::System
    };
    (
        cpus,
        memory,
        minimum.disk_mib.max(4096),
        Resources {
            pin_cpus: host.cpu_pinning,
            host_cpus: host.preferred_cpus(cpus),
            pin_memory: host.memory_pinning,
            pages,
            reserve_storage: true,
        },
    )
}

pub fn validate(config: &MachineConfig) -> Result<(), String> {
    let min = config.requirements;
    min.validate()?;
    if !(min.cpus..=128).contains(&config.cpus) {
        return Err(format!("this image needs {}–128 CPUs", min.cpus));
    }
    if !(min.memory_mib..=1 << 20).contains(&config.memory_mib)
        || !config.memory_mib.is_multiple_of(2)
    {
        return Err(format!(
            "this image needs at least {} MiB RAM, split evenly between two guest nodes",
            min.memory_mib
        ));
    }
    if !(min.disk_mib..=1 << 22).contains(&config.disk_mib) {
        return Err(format!(
            "this image needs at least {} MiB storage (maximum 4 TiB)",
            min.disk_mib
        ));
    }
    vkore_vm::hardware::validate_binding(&config.resources, config.cpus)?;
    let pages = config.resources.pages.mib();
    if pages != 0 && !config.memory_mib.is_multiple_of(pages * 2) {
        return Err(format!(
            "{} requires RAM in multiples of {} MiB for two guest nodes",
            config.resources.pages.label(),
            pages * 2
        ));
    }
    Ok(())
}

/// Recheck snapshots at use, including changes to allowed CPUs and memory pressure.
pub fn validate_host(config: &MachineConfig, host: &HostResources) -> Result<(), String> {
    validate(config)?;
    if usize::from(config.cpus) > host.logical_cpus {
        return Err("guest CPU count exceeds the host's available logical CPUs".into());
    }
    if u64::from(config.memory_mib) > host.memory_mib {
        return Err("guest RAM exceeds physical host memory".into());
    }
    if config.resources.pin_memory
        && config.resources.pages == PageSize::System
        && u64::from(config.memory_mib) > host.available_memory_mib
    {
        return Err(
            "not enough available host RAM; reduce the allocation or close other workloads".into(),
        );
    }
    let r = &config.resources;
    if r.pin_cpus {
        if !host.cpu_pinning {
            return Err(
                "this host does not support exact CPU pinning; turn off CPU pinning".into(),
            );
        }
        let allowed: std::collections::BTreeSet<_> =
            host.cores.iter().flat_map(|c| &c.threads).collect();
        if r.host_cpus.iter().any(|id| !allowed.contains(id)) {
            return Err("a selected host CPU is offline or no longer available; refresh the hardware selection".into());
        }
    }
    if r.pin_memory && !host.memory_pinning {
        return Err(
            "this host QEMU does not support memory locking; turn off memory pinning".into(),
        );
    }
    if r.pages != PageSize::System && !host.hugetlb_supported {
        return Err("explicit hugepage backing requires Linux; choose Automatic (system pages) on this host".into());
    }
    if !host.huge_available(r.pages, config.memory_mib) {
        return Err(format!(
            "not enough reserved {} on this host; choose system pages or provision the host hugepage pool",
            r.pages.label()
        ));
    }
    Ok(())
}

/// QEMU locks more than guest RAM (firmware, devices, emulator mappings).
/// Allow a bounded 1 GiB margin, and still surface any QEMU lock failure.
pub fn lock_budget(memory_mib: u32) -> u64 {
    (u64::from(memory_mib) + 1024) << 20
}
pub fn needs_authorization(config: &MachineConfig, host: &HostResources) -> bool {
    config.resources.pin_memory
        && host.memory_pinning
        && host
            .memlock_bytes
            .is_some_and(|n| n < lock_budget(config.memory_mib))
}

/// Raise only this user's vKore process limit. QEMU stays unprivileged and
/// inherits it. No shell, privileged QEMU, arbitrary helper, or password input.
/// pkexec uses the desktop's authentication agent and handles cancellation.
pub fn authorize_memory(memory_mib: u32) -> Result<(), String> {
    if !(2048..=1 << 20).contains(&memory_mib) {
        return Err("invalid memory authorization size".into());
    }
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        let budget = lock_budget(memory_mib);
        let limit = getrlimit(Resource::Memlock);
        if limit.current.is_none_or(|n| n >= budget) {
            return Ok(());
        }
        if limit.maximum.is_none_or(|n| n >= budget) {
            return setrlimit(
                Resource::Memlock,
                Rlimit {
                    current: Some(budget),
                    maximum: limit.maximum,
                },
            )
            .map_err(|e| e.to_string());
        }
        let mut child = std::process::Command::new("/usr/bin/pkexec")
            .args(["--disable-internal-agent", "/usr/bin/prlimit", "--pid", &std::process::id().to_string(), &format!("--memlock={budget}:{budget}")])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().map_err(|_| "memory authorization requires polkit, a desktop authentication agent and /usr/bin/prlimit")?;
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if status.success()
                    && getrlimit(Resource::Memlock)
                        .current
                        .is_none_or(|n| n >= budget)
                {
                    return Ok(());
                }
                return Err("memory authorization was cancelled or denied; retry or turn off memory pinning".into());
            }
            if started.elapsed() > std::time::Duration::from_secs(120) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "memory authorization timed out; retry or turn off memory pinning".into(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    #[cfg(not(target_os = "linux"))]
    Err("memory pinning authorization is unavailable on this host".into())
}

/// Serialize launch and hardware edits across desktop/CLI processes. Held for
/// the complete session, including offline staging and writeback.
pub(crate) fn lease(dir: &Path) -> Result<File, StoreError> {
    let path = dir.join("session.lock");
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| StoreError::Io(path.clone(), e))?;
    file.try_lock().map_err(|_| {
        StoreError::Invalid("this machine is busy; stop it before changing its hardware".into())
    })?;
    Ok(file)
}

/// Only hardware fields can be edited. Existing disks cannot be resized by
/// truncating a file; doing so requires an engine-owned resize protocol.
pub fn save(
    store: &MachineStore,
    name: &str,
    cpus: u16,
    memory_mib: u32,
    disk_mib: u64,
    resources: Resources,
) -> Result<(), String> {
    store.get(name).map_err(|e| e.to_string())?;
    let paths = store.state().machine(name);
    let _lease = lease(&paths.dir).map_err(|e| e.to_string())?;
    let mut config = store.get(name).map_err(|e| e.to_string())?;
    if paths.disk.exists()
        && (disk_mib != config.disk_mib
            || resources.reserve_storage != config.resources.reserve_storage)
    {
        return Err("storage allocation is fixed after the disk is created".into());
    }
    config.cpus = cpus;
    config.memory_mib = memory_mib;
    config.disk_mib = disk_mib;
    config.resources = resources;
    let host = HostResources::probe(store.state().root())?;
    validate_host(&config, &host)?;
    if !paths.disk.exists()
        && config.resources.reserve_storage
        && disk_mib > host.storage_available_mib
    {
        return Err("not enough free storage for this reservation".into());
    }
    store.save(&config).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host() -> HostResources {
        HostResources {
            cpu_name: "fixture".into(),
            physical_cores: 8,
            logical_cpus: 16,
            cores: (0..8)
                .map(|id| vkore_vm::hardware::Core {
                    package: 0,
                    id,
                    kind: "Core".into(),
                    threads: vec![id, id + 8],
                    numa_node: Some(0),
                })
                .collect(),
            core_summary: String::new(),
            memory_mib: 32768,
            available_memory_mib: 24576,
            free_memory_mib: 8192,
            memory_nodes: vec![],
            storage: vec![],
            storage_path: "/".into(),
            storage_available_mib: 102400,
            cpu_pinning: true,
            memory_pinning: true,
            system_page_bytes: 4096,
            hugetlb_supported: true,
            hugepages: vec![vkore_vm::hardware::HugePool {
                page: PageSize::Huge2M,
                free_mib: 16384,
            }],
            memlock_bytes: Some(65536),
        }
    }
    #[test]
    fn unsupported_page_backing_is_distinct_from_an_exhausted_pool() {
        let mut h = host();
        let mut c = MachineConfig::new("pages", "/fixture.iso");
        c.resources.pages = PageSize::Huge2M;
        h.hugetlb_supported = false;
        assert!(
            validate_host(&c, &h)
                .unwrap_err()
                .contains("requires Linux")
        );
        c.resources.pages = PageSize::System;
        validate_host(&c, &h).unwrap();
        h.hugetlb_supported = true;
        h.hugepages.clear();
        c.resources.pages = PageSize::Huge2M;
        assert!(validate_host(&c, &h).unwrap_err().contains("pool"));
    }
    #[test]
    fn recommendations_use_physical_cores_and_half_physical_ram() {
        let h = host();
        let (cpu, ram, disk, r) = defaults(&h, Requirements::default());
        assert_eq!((cpu, ram, disk), (4, 16384, 4096));
        assert_eq!(r.host_cpus, vec![0, 1, 2, 3]);
        assert!(r.pin_cpus && r.pin_memory && r.reserve_storage);
        assert_eq!(r.pages, PageSize::Huge2M);
        let mut h = h;
        h.hugepages.clear();
        h.cpu_pinning = false;
        h.memory_pinning = false;
        let (_, _, _, r) = defaults(&h, Requirements::default());
        assert_eq!(r.pages, PageSize::System);
        assert!(!r.pin_cpus && !r.pin_memory);
    }
    #[test]
    fn changed_host_resources_are_rejected_without_silent_fallback() {
        let mut h = host();
        let mut c = MachineConfig::new("demo", "/image.iso");
        (c.cpus, c.memory_mib, c.disk_mib, c.resources) = defaults(&h, c.requirements);
        validate_host(&c, &h).unwrap();
        assert!(needs_authorization(&c, &h));
        h.cores.remove(0);
        assert!(validate_host(&c, &h).is_err());
        h = host();
        h.hugepages.clear();
        assert!(validate_host(&c, &h).is_err());
        c.resources.pages = PageSize::System;
        h.available_memory_mib = 1024;
        assert!(validate_host(&c, &h).is_err());
    }
    #[test]
    fn existing_disk_and_busy_machine_are_not_reconfigured() {
        let dir = tempfile::tempdir().unwrap();
        let store = MachineStore::new(crate::State::open(dir.path()).unwrap());
        let c = MachineConfig::new("demo", "/image.iso");
        store.create(&c).unwrap();
        let paths = store.state().machine("demo");
        let held = lease(&paths.dir).unwrap();
        assert!(
            save(&store, "demo", 2, 2048, 4096, Default::default())
                .unwrap_err()
                .contains("busy")
        );
        drop(held);
        std::fs::write(&paths.disk, b"preserved").unwrap();
        assert!(
            save(&store, "demo", 2, 2048, 8192, Default::default())
                .unwrap_err()
                .contains("fixed")
        );
        assert_eq!(store.get("demo").unwrap(), c);
        assert_eq!(std::fs::read(paths.disk).unwrap(), b"preserved");
    }
    #[test]
    fn minimums_and_hugepage_alignment_are_enforced() {
        let mut c = MachineConfig::new("demo", "/image.iso");
        for (cpu, ram, disk) in [(1, 2048, 1024), (2, 2046, 1024), (2, 2048, 1023)] {
            c.cpus = cpu;
            c.memory_mib = ram;
            c.disk_mib = disk;
            assert!(validate(&c).is_err());
        }
        c.cpus = 2;
        c.memory_mib = 2048;
        c.disk_mib = 1024;
        c.resources.pages = PageSize::Huge1G;
        assert!(validate(&c).is_ok());
        c.memory_mib = 3072;
        assert!(validate(&c).is_err());
    }
    #[test]
    fn launch_and_edits_share_an_exclusive_lease() {
        let dir = tempfile::tempdir().unwrap();
        let first = lease(dir.path()).unwrap();
        assert!(lease(dir.path()).is_err());
        drop(first);
        assert!(lease(dir.path()).is_ok());
    }
}
