//! Resource policy shared by the desktop, imports, saved configurations and
//! launch preflight. The view edits values; it never defines admission policy.
use crate::{MachineConfig, MachineStore, config::StoreError};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};
use vkore_vm::hardware::{HostResources, PageSize, Resources};

/// Maximum virtual disk capacity, shared by admission and the desktop.
pub const MAX_DISK_MIB: u64 = 1 << 22;
/// New inference machines need room for model weights and working files.
pub const DEFAULT_DISK_MIB: u64 = 32 * 1024;

/// Reservation must fit the host; sparse disks may grow up to the format limit.
pub fn disk_limit(host: &HostResources, reserve: bool) -> u64 {
    if reserve {
        host.storage_available_mib.min(MAX_DISK_MIB)
    } else {
        MAX_DISK_MIB
    }
}

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
            || !(1024..=MAX_DISK_MIB).contains(&self.disk_mib)
        {
            return Err("image requirements must be within 2–128 CPUs, 2 GiB–1 TiB RAM and 1 GiB–4 TiB storage".into());
        }
        Ok(())
    }
}

/// Recommend native waits only when this configuration resolves to pinned KVM.
/// Explicit saved choices are validated separately and never replaced here.
pub fn recommended_cpu_waits(
    host: &HostResources,
    pin_cpus: bool,
    accel: vkore_vm::AccelChoice,
    vendor: Option<vkore_vm::Vendor>,
) -> bool {
    pin_cpus
        && host.cpu_pinning
        && host.cpu_power_management.is_ok()
        && host
            .acceleration
            .choose(
                accel,
                vendor.unwrap_or_else(|| host.acceleration.native_vendor()),
            )
            .is_ok_and(|accel| accel == vkore_vm::Accel::Kvm)
}

pub fn defaults(host: &HostResources, minimum: Requirements) -> (u16, u32, u64, Resources) {
    let cpus = (host.physical_cores / 2).clamp(2, 128) as u16;
    let cpus = cpus.max(minimum.cpus);
    // New machines keep one memory node; align it for the preferred 2 MiB pages.
    let memory = ((host.memory_mib / 2).clamp(2048, 1 << 20) as u32)
        .max(minimum.memory_mib)
        .div_ceil(2)
        * 2;
    let pages = if host.huge_available(PageSize::Huge2M, memory) {
        PageSize::Huge2M
    } else {
        PageSize::System
    };
    (
        cpus,
        memory,
        minimum.disk_mib.max(DEFAULT_DISK_MIB),
        Resources {
            gpu_vfs: Vec::new(),
            guest_numa: vkore_vm::hardware::GuestNuma::Single,
            passthrough: Vec::new(),
            pin_cpus: host.cpu_pinning,
            cpu_power_management: recommended_cpu_waits(
                host,
                host.cpu_pinning,
                vkore_vm::AccelChoice::Auto,
                None,
            ),
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
        || !config
            .memory_mib
            .is_multiple_of(config.resources.guest_numa.nodes())
    {
        return Err(format!(
            "this image needs at least {} MiB RAM, aligned to the selected guest topology",
            min.memory_mib
        ));
    }
    if !(min.disk_mib..=MAX_DISK_MIB).contains(&config.disk_mib) {
        return Err(format!(
            "this image needs at least {} MiB storage (maximum 4 TiB)",
            min.disk_mib
        ));
    }
    vkore_vm::hardware::validate_binding(&config.resources, config.cpus)?;
    let pages = config.resources.pages.mib();
    if pages != 0
        && !config.memory_mib.is_multiple_of(
            config
                .resources
                .guest_numa
                .alignment(config.resources.pages),
        )
    {
        return Err(format!(
            "{} requires RAM in multiples of {} MiB for the selected guest topology",
            config.resources.pages.label(),
            config
                .resources
                .guest_numa
                .alignment(config.resources.pages)
        ));
    }
    Ok(())
}

/// Recheck snapshots at use, including changes to allowed CPUs and memory pressure.
pub fn validate_host(config: &MachineConfig, host: &HostResources) -> Result<(), String> {
    validate(config)?;
    if config.resources.cpu_power_management {
        host.cpu_power_management.clone()?;
        if !recommended_cpu_waits(host, config.resources.pin_cpus, config.accel, config.vendor) {
            return Err("Native CPU waits require KVM acceleration.".into());
        }
    }
    if usize::from(config.cpus) > host.logical_cpus {
        return Err("guest CPU count exceeds the host's available logical CPUs".into());
    }
    if u64::from(config.memory_mib) > host.memory_mib {
        return Err("guest RAM exceeds physical host memory".into());
    }
    if (config.resources.pin_memory || has_devices(config))
        && config.resources.pages == PageSize::System
        && u64::from(config.memory_mib) > host.available_memory_mib
    {
        return Err(
            "not enough available host RAM; reduce the allocation or close other workloads".into(),
        );
    }
    let r = &config.resources;
    vkore_vm::passthrough::validate_inventory(&r.passthrough, &host.npus, &host.gpus)?;
    for gpu in &r.gpu_vfs {
        let parent = gpu.parent(&host.gpus)?;
        if parent.pci.enabled_vfs.is_some_and(|n| n > 0) {
            let selection = gpu.resolve(&host.gpus)?;
            vkore_vm::passthrough::check_host(&[selection])?;
        }
    }
    vkore_vm::passthrough::check_host(&r.passthrough)?;
    if has_devices(config) && !cfg!(target_os = "linux") {
        return Err(
            "Accelerator passthrough requires Linux; remove the saved device selections on this host."
                .into(),
        );
    }
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
    if !host.huge_supported(r.pages) {
        return Err(format!(
            "{} is not supported by this host kernel; choose Automatic",
            r.pages.label()
        ));
    }
    if r.pages != PageSize::System {
        let pool = host.hugepages.iter().find(|p| p.page == r.pages).unwrap();
        if u64::from(config.memory_mib) > pool.free_mib.saturating_add(host.available_memory_mib) {
            return Err("not enough available RAM and unreserved hugepages; reduce the allocation or close other workloads".into());
        }
    }
    Ok(())
}

pub fn uses_guardian(config: &MachineConfig) -> bool {
    has_devices(config) || config.resources.pages != PageSize::System
}

pub fn has_devices(config: &MachineConfig) -> bool {
    !config.resources.passthrough.is_empty() || !config.resources.gpu_vfs.is_empty()
}
/// Resolve persisted GPU intent immediately before launch; setup can provision absent VFs.
pub fn assigned_devices(
    config: &MachineConfig,
    host: &HostResources,
) -> Result<Vec<vkore_vm::passthrough::Selection>, String> {
    let mut selected = config.resources.passthrough.clone();
    for gpu in &config.resources.gpu_vfs {
        selected.push(gpu.resolve(&host.gpus)?);
    }
    vkore_vm::passthrough::validate_inventory(&selected, &host.npus, &host.gpus)?;
    vkore_vm::passthrough::check_host(&selected)?;
    Ok(selected)
}

pub use vkore_vm::passthrough::lock_budget;
pub fn needs_authorization(config: &MachineConfig, host: &HostResources) -> bool {
    (config.resources.pin_memory || has_devices(config))
        && host.memory_pinning
        && host.memlock_bytes.is_some_and(|n| {
            lock_budget(
                config.memory_mib,
                config.resources.passthrough.len() + config.resources.gpu_vfs.len(),
            )
            .is_ok_and(|budget| n < budget)
        })
}

/// Validate the complete selection before requesting its permissions once.
/// Desktop completion and launch share this entry point; noninteractive callers
/// continue to use validation without invoking an authentication agent.
pub fn authorize_configuration(config: &MachineConfig, host: &HostResources) -> Result<(), String> {
    if uses_guardian(config) {
        validate_host(config, host)?;
        return vkore_vm::passthrough::authorize(
            &config.resources.passthrough,
            &config.resources.gpu_vfs,
            config.memory_mib,
            config.resources.pages,
            config.resources.guest_numa,
        );
    }
    authorize_with(config, host, authorize_memory)
}

fn authorize_with(
    config: &MachineConfig,
    host: &HostResources,
    authorize: impl FnOnce(u32) -> Result<(), String>,
) -> Result<(), String> {
    validate_host(config, host)?;
    if needs_authorization(config, host) {
        authorize(config.memory_mib)?;
    }
    Ok(())
}

/// Finish a new machine's hardware draft. Storage admission precedes any
/// authorization; no image or machine needs to exist yet.
pub fn prepare(config: &MachineConfig, root: &Path) -> Result<HostResources, String> {
    let host = HostResources::probe(root)?;
    validate_host(config, &host)?;
    if config.resources.reserve_storage && config.disk_mib > host.storage_available_mib {
        return Err("not enough free storage for this reservation".into());
    }
    authorize_configuration(config, &host)?;
    let host = HostResources::probe(root)?;
    validate_host(config, &host)?;
    if config.resources.reserve_storage && config.disk_mib > host.storage_available_mib {
        return Err("not enough free storage for this reservation".into());
    }
    Ok(host)
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
        let budget = lock_budget(memory_mib, 0)?;
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
        vkore_vm::passthrough::authorize_memory_limit(memory_mib)?;
        if getrlimit(Resource::Memlock)
            .current
            .is_none_or(|n| n >= budget)
        {
            Ok(())
        } else {
            Err("Memory authorization did not provide the requested limit.".into())
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
    save_with(
        store,
        name,
        cpus,
        memory_mib,
        disk_mib,
        resources,
        |_, _| Ok(()),
    )
}

/// Desktop save: hold the session lease through validation, authorization and
/// persistence. A denied request never changes the saved configuration.
pub fn save_authorized(
    store: &MachineStore,
    name: &str,
    cpus: u16,
    memory_mib: u32,
    disk_mib: u64,
    resources: Resources,
) -> Result<(), String> {
    save_with(
        store,
        name,
        cpus,
        memory_mib,
        disk_mib,
        resources,
        authorize_configuration,
    )
}

fn save_with(
    store: &MachineStore,
    name: &str,
    cpus: u16,
    memory_mib: u32,
    disk_mib: u64,
    resources: Resources,
    authorize: impl FnOnce(&MachineConfig, &HostResources) -> Result<(), String>,
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
    authorize(&config, &host)?;
    // Authorization can wait for the user; don't persist a now-invalid choice.
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
            npus: vkore_vm::devices::Inventory::empty(vkore_vm::devices::Platform::Linux),
            gpus: Default::default(),
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
            acceleration: vkore_vm::HostCaps {
                vendor: Some(vkore_vm::Vendor::Intel),
                kvm: true,
                hvf: false,
            },
            cpu_power_management: Ok(()),
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
    fn automatic_native_waits_require_supported_pinned_kvm() {
        use vkore_vm::{AccelChoice, Vendor};
        let mut host = host();
        for vendor in [Vendor::Intel, Vendor::Amd] {
            host.acceleration.vendor = Some(vendor);
            for choice in [AccelChoice::Auto, AccelChoice::Kvm] {
                assert!(recommended_cpu_waits(&host, true, choice, Some(vendor)));
                assert!(!recommended_cpu_waits(&host, false, choice, Some(vendor)));
            }
            for choice in [AccelChoice::Tcg, AccelChoice::Hvf] {
                assert!(!recommended_cpu_waits(&host, true, choice, Some(vendor)));
            }
        }
        assert!(!recommended_cpu_waits(
            &host,
            true,
            AccelChoice::Auto,
            Some(Vendor::Intel)
        ));
        host.cpu_power_management = Err("unsupported".into());
        assert!(
            !defaults(&host, Requirements::default())
                .3
                .cpu_power_management
        );
        host.cpu_power_management = Ok(());
        host.acceleration.kvm = false;
        assert!(
            !defaults(&host, Requirements::default())
                .3
                .cpu_power_management
        );
        host.acceleration.kvm = true;
        host.cpu_pinning = false;
        assert!(
            !defaults(&host, Requirements::default())
                .3
                .cpu_power_management
        );
    }

    #[test]
    fn native_waits_are_recommended_and_revalidated_against_the_host() {
        let mut host = host();
        let mut config = MachineConfig::new("waits", "/image.iso");
        (
            config.cpus,
            config.memory_mib,
            config.disk_mib,
            config.resources,
        ) = defaults(&host, Requirements::default());
        assert!(config.resources.cpu_power_management);
        assert!(validate_host(&config, &host).is_ok());
        host.cpu_power_management = Err("KVM does not allow native MONITOR/MWAIT".into());
        assert!(
            validate_host(&config, &host)
                .unwrap_err()
                .contains("MONITOR/MWAIT")
        );
        host.cpu_power_management = Ok(());
        config.accel = vkore_vm::AccelChoice::Tcg;
        assert!(validate_host(&config, &host).unwrap_err().contains("KVM"));
        config.accel = vkore_vm::AccelChoice::Kvm;
        config.resources.pin_cpus = false;
        assert!(
            validate_host(&config, &host)
                .unwrap_err()
                .contains("pinning")
        );
        let mut legacy = serde_json::to_value(&config.resources).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("cpu_power_management");
        assert!(
            !serde_json::from_value::<Resources>(legacy)
                .unwrap()
                .cpu_power_management
        );
    }

    #[test]
    fn permission_check_counts_gpu_vfs_as_well_as_dedicated_devices() {
        let mut h = host();
        h.memlock_bytes = Some(9 << 30);
        let mut c = MachineConfig::new("demo", "/image.iso");
        c.memory_mib = 8192;
        c.resources.pin_memory = false;
        c.resources
            .passthrough
            .push(vkore_vm::passthrough::Selection {
                address: "0000:00:0b.0".into(),
                vendor_id: 0x8086,
                device_id: 0xb03e,
                physical_function: None,
            });
        assert!(!needs_authorization(&c, &h));
        c.resources
            .gpu_vfs
            .push(vkore_vm::passthrough::gpu::Assignment {
                physical_function: "0000:00:02.0".into(),
                vendor_id: 0x8086,
                device_id: 0xb080,
                index: 0,
            });
        assert!(needs_authorization(&c, &h));
        h.memlock_bytes = Some(17 << 30);
        assert!(!needs_authorization(&c, &h));
    }

    #[test]
    fn storage_defaults_and_limits_match_reservation_policy() {
        let mut h = host();
        let min = Requirements::default();
        assert_eq!(defaults(&h, min).2, 32 * 1024);
        h.storage_available_mib = 7 * 1024 + 512;
        assert_eq!(defaults(&h, min).2, DEFAULT_DISK_MIB); // Do not silently shrink.
        assert_eq!(disk_limit(&h, true), h.storage_available_mib);
        assert_eq!(disk_limit(&h, false), MAX_DISK_MIB);
        h.storage_available_mib = 512;
        assert_eq!(defaults(&h, min).2, DEFAULT_DISK_MIB); // Admission reports insufficient space.
        h.storage_available_mib = u64::MAX;
        assert_eq!(disk_limit(&h, true), MAX_DISK_MIB);
    }
    #[test]
    fn authorization_is_batched_after_validation_and_skips_unneeded_requests() {
        let mut h = host();
        let mut c = MachineConfig::new("demo", "/image.iso");
        (c.cpus, c.memory_mib, c.disk_mib, c.resources) = defaults(&h, c.requirements);
        let mut calls = 0;
        authorize_with(&c, &h, |mib| {
            calls += 1;
            assert_eq!(mib, c.memory_mib);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(
            authorize_with(&c, &h, |_| Err("denied".into())),
            Err("denied".into())
        );
        c.cpus = 1;
        assert!(authorize_with(&c, &h, |_| panic!("invalid draft prompted")).is_err());
        c.cpus = 4;
        h.memlock_bytes = Some(lock_budget(c.memory_mib, 0).unwrap());
        authorize_with(&c, &h, |_| panic!("sufficient limit prompted")).unwrap();
        h.memlock_bytes = None;
        authorize_with(&c, &h, |_| panic!("unlimited lock budget prompted")).unwrap();
        h.memlock_bytes = Some(0);
        c.resources.pin_memory = false;
        authorize_with(&c, &h, |_| panic!("unpinned memory prompted")).unwrap();
        h.memory_pinning = false;
        authorize_with(&c, &h, |_| panic!("unsupported host prompted")).unwrap();
    }

    #[test]
    fn denied_save_preserves_config_and_releases_session_lease() {
        let dir = tempfile::tempdir().unwrap();
        let store = MachineStore::new(crate::State::open(dir.path()).unwrap());
        let c = MachineConfig::new("demo", "/image.iso");
        store.create(&c).unwrap();
        let result = save_with(&store, "demo", 2, 2048, 1024, Default::default(), |_, _| {
            assert!(lease(&store.state().machine("demo").dir).is_err());
            Err("cancelled".into())
        });
        assert_eq!(result, Err("cancelled".into()));
        assert_eq!(store.get("demo").unwrap(), c);
        assert!(lease(&store.state().machine("demo").dir).is_ok());
        assert!(
            save_with(&store, "demo", 1, 2048, 1024, Default::default(), |_, _| {
                panic!("invalid settings prompted")
            })
            .is_err()
        );
    }

    #[test]
    fn empty_hugepage_pool_can_be_provisioned_without_devices_or_memory_pinning() {
        let mut h = host();
        h.hugepages[0].free_mib = 0;
        let mut c = MachineConfig::new("pages", "/fixture.iso");
        c.resources.pages = PageSize::Huge2M;
        c.resources.pin_memory = false;
        c.memory_mib = 8192;
        assert!(uses_guardian(&c));
        assert!(!has_devices(&c));
        validate_host(&c, &h).unwrap();
        h.available_memory_mib = 4096;
        assert!(
            validate_host(&c, &h)
                .unwrap_err()
                .contains("not enough available")
        );
        h.hugepages[0].free_mib = 4096;
        validate_host(&c, &h).unwrap();
        c.resources.pages = PageSize::System;
        assert!(!uses_guardian(&c));
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
        assert!(validate_host(&c, &h).unwrap_err().contains("not supported"));
    }
    #[test]
    fn recommendations_use_physical_cores_and_half_physical_ram() {
        let h = host();
        let (cpu, ram, disk, r) = defaults(&h, Requirements::default());
        assert_eq!((cpu, ram, disk), (4, 16384, 32768));
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
            save_with(
                &store,
                "demo",
                2,
                2048,
                4096,
                Default::default(),
                |_, _| panic!("busy machine prompted")
            )
            .unwrap_err()
            .contains("busy")
        );
        drop(held);
        std::fs::write(&paths.disk, b"preserved").unwrap();
        assert!(
            save_with(
                &store,
                "demo",
                2,
                2048,
                8192,
                Default::default(),
                |_, _| panic!("fixed disk change prompted")
            )
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

/// Discovery and guest support are distinct. No route is selectable until its
/// complete launch/ownership implementation exists, irrespective of PCI identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessStatus {
    Configurable,
    Pending,
    NeedsSetup,
    Unavailable,
    Unknown,
}
impl AccessStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Configurable => "Can configure",
            Self::Pending => "Not connected yet",
            Self::NeedsSetup => "Needs host setup",
            Self::Unavailable => "Unavailable",
            Self::Unknown => "Not verified",
        }
    }
}
#[derive(Clone, Debug)]
pub struct AccessRoute {
    pub name: &'static str,
    pub status: AccessStatus,
    pub detail: &'static str,
}

pub fn npu_routes(device: &vkore_vm::devices::Npu) -> [AccessRoute; 3] {
    use AccessStatus::*;
    use vkore_vm::devices::{Source, Vendor};
    let route = |name, status, detail| AccessRoute {
        name,
        status,
        detail,
    };
    let direct = match &device.pci {
        Some(pci) if pci.physical_function.is_some() => route(
            "Dedicated device",
            Unavailable,
            "This is a virtual function; use the SR-IOV route.",
        ),
        Some(pci) if pci.iommu_group.is_none() => route(
            "Dedicated device",
            NeedsSetup,
            "No IOMMU group was reported. Host isolation must be available before assignment.",
        ),
        Some(_) if vkore_vm::passthrough::eligible(device).is_ok() => route(
            "Dedicated device",
            Configurable,
            "Enable Full passthrough to assign this NPU at launch. Function-level reset, Linux KVM and QEMU IOMMUFD support are required; guest driver support must be tested with your image.",
        ),
        Some(_) => route(
            "Dedicated device",
            Unavailable,
            "Whole-device assignment requires a supported physical NPU in its own IOMMU group, with no enabled VFs.",
        ),
        None if device.source == Source::CoreMl => route(
            "Dedicated device",
            Unavailable,
            "vKore cannot pass the Apple Neural Engine directly to a guest.",
        ),
        None => route(
            "Dedicated device",
            Unavailable,
            "vKore has no direct device-assignment backend on this host.",
        ),
    };
    let sriov = match &device.pci {
        Some(pci) if pci.physical_function.is_some() => route(
            "Virtual function · SR-IOV",
            Pending,
            "The host reports an existing VF. Guest assignment is not connected yet.",
        ),
        Some(pci) if pci.total_vfs.is_some_and(|n| n > 0) => route(
            "Virtual function · SR-IOV",
            Pending,
            "The host reports SR-IOV capacity. VF provisioning and guest compatibility are not verified.",
        ),
        Some(pci) if pci.total_vfs == Some(0) => route(
            "Virtual function · SR-IOV",
            Unavailable,
            "The host does not expose SR-IOV capacity for this device.",
        ),
        None if device.source == Source::CoreMl => route(
            "Virtual function · SR-IOV",
            Unavailable,
            "Core ML does not expose an SR-IOV virtual function.",
        ),
        _ => route(
            "Virtual function · SR-IOV",
            Unknown,
            "This discovery source does not report SR-IOV capabilities.",
        ),
    };
    let shared = match (device.vendor, device.source, device.driver.as_deref()) {
        (_, Source::LinuxPci, Some("vfio-pci")) => route(
            "Shared · virtio-accel",
            NeedsSetup,
            "The device is bound to VFIO. A host runtime needs its native driver.",
        ),
        (_, Source::LinuxPci, None) => route(
            "Shared · virtio-accel",
            NeedsSetup,
            "No host driver is bound. A compatible driver and runtime are required.",
        ),
        (Vendor::Apple, Source::CoreMl, _) => route(
            "Shared · virtio-accel",
            Pending,
            "Core ML reports this Neural Engine. The guest connection is not implemented yet; Core ML may also use the CPU.",
        ),
        (Vendor::Intel, _, _) => route(
            "Shared · virtio-accel",
            Unknown,
            "OpenVINO is the candidate backend. Its runtime and device access have not been checked.",
        ),
        (Vendor::Amd, _, _) => route(
            "Shared · virtio-accel",
            Unknown,
            "The XDNA backend needs a compatible host runtime. Runtime access has not been checked.",
        ),
        (Vendor::Qualcomm, _, _) => route(
            "Shared · virtio-accel",
            Unknown,
            "The Hexagon backend needs a compatible QAIRT runtime. Runtime access has not been checked.",
        ),
        _ => route(
            "Shared · virtio-accel",
            Unknown,
            "No compatible host backend has been verified for this NPU.",
        ),
    };
    [direct, sriov, shared]
}

#[cfg(test)]
mod npu_tests {
    use super::*;
    use vkore_vm::devices::{Npu, Pci, Source, Vendor};
    fn device() -> Npu {
        Npu {
            id: "pci:0000:00:0b.0".into(),
            name: "Intel NPU".into(),
            vendor: Vendor::Intel,
            source: Source::LinuxPci,
            driver: Some("ivpu".into()),
            core_count: None,
            pci: Some(Pci {
                address: "0000:00:0b.0".into(),
                vendor_id: 0x8086,
                device_id: 0xb03e,
                revision: Some(1),
                iommu_group: Some(12),
                group_members: vec!["0000:00:0b.0".into()],
                physical_function: None,
                total_vfs: Some(0),
                enabled_vfs: None,
            }),
        }
    }
    #[test]
    fn identity_never_implies_sriov_or_runtime_readiness() {
        let mut npu = device();
        let routes = npu_routes(&npu);
        assert_eq!(routes[0].status, AccessStatus::Configurable);
        assert_eq!(routes[1].status, AccessStatus::Unavailable);
        assert_eq!(routes[2].status, AccessStatus::Unknown);
        npu.pci.as_mut().unwrap().total_vfs = Some(4);
        assert_eq!(npu_routes(&npu)[1].status, AccessStatus::Pending);
        npu.pci.as_mut().unwrap().total_vfs = None;
        assert_eq!(npu_routes(&npu)[1].status, AccessStatus::Unknown);
        npu.driver = Some("vfio-pci".into());
        assert_eq!(npu_routes(&npu)[2].status, AccessStatus::NeedsSetup);
        npu.pci.as_mut().unwrap().physical_function = Some("0000:00:0a.0".into());
        assert_eq!(npu_routes(&npu)[0].status, AccessStatus::Unavailable);
        assert_eq!(npu_routes(&npu)[1].status, AccessStatus::Pending);
    }
    #[test]
    fn native_host_discovery_does_not_grant_guest_access() {
        let mut npu = device();
        npu.source = Source::CoreMl;
        npu.vendor = Vendor::Apple;
        npu.pci = None;
        npu.driver = None;
        let routes = npu_routes(&npu);
        assert_eq!(routes[0].status, AccessStatus::Unavailable);
        assert_eq!(routes[1].status, AccessStatus::Unavailable);
        assert_eq!(routes[2].status, AccessStatus::Pending);
        npu.source = Source::WindowsDxCore;
        npu.vendor = Vendor::Qualcomm;
        let routes = npu_routes(&npu);
        assert_eq!(routes[0].status, AccessStatus::Unavailable);
        assert_eq!(routes[1].status, AccessStatus::Unknown);
        assert_eq!(routes[2].status, AccessStatus::Unknown);
    }
}
