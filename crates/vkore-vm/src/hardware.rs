//! Host resources, separate from the synthetic topology presented to Kore.
//! Discovery is an explicit snapshot, never a per-frame operation. Linux core
//! IDs come from sysfs and are restricted to this process's allowed CPU set.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSize {
    #[default]
    System,
    Huge2M,
    Huge1G,
}
impl PageSize {
    pub fn mib(self) -> u32 {
        match self {
            Self::System => 0,
            Self::Huge2M => 2,
            Self::Huge1G => 1024,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System pages",
            Self::Huge2M => "2 MiB hugepages",
            Self::Huge1G => "1 GiB hugepages",
        }
    }
}

/// Legacy configurations deserialize without changing their scheduling policy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    #[serde(default)]
    pub pin_cpus: bool,
    /// One distinct host logical CPU per vCPU, in guest CPU-index order.
    #[serde(default)]
    pub host_cpus: Vec<u32>,
    #[serde(default)]
    pub pin_memory: bool,
    #[serde(default)]
    pub pages: PageSize,
    #[serde(default)]
    pub reserve_storage: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Core {
    pub package: u32,
    pub id: u32,
    pub kind: String,
    pub threads: Vec<u32>,
    pub numa_node: Option<u32>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Storage {
    pub device: String,
    pub mount: PathBuf,
    pub filesystem: String,
    pub kind: String,
    pub total_mib: u64,
    pub available_mib: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct MemoryNode {
    pub id: u32,
    pub memory_mib: u64,
    pub cpus: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct HugePool {
    pub page: PageSize,
    pub free_mib: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct HostResources {
    /// Discovery failure is reported inside the inventory, never as missing RAM/CPU.
    pub npus: vkore_devices::Inventory,
    pub cpu_name: String,
    pub physical_cores: usize,
    pub logical_cpus: usize,
    pub cores: Vec<Core>,
    pub core_summary: String,
    pub memory_mib: u64,
    pub available_memory_mib: u64,
    pub free_memory_mib: u64,
    pub memory_nodes: Vec<MemoryNode>,
    pub storage: Vec<Storage>,
    pub storage_path: PathBuf,
    pub storage_available_mib: u64,
    pub cpu_pinning: bool,
    pub memory_pinning: bool,
    /// Native host page granularity; independent of Kore's guest page tables.
    pub system_page_bytes: usize,
    /// Whether this platform supports vKore's explicit Linux hugetlb backend.
    /// Pool accessibility and capacity are reported separately in `hugepages`.
    pub hugetlb_supported: bool,
    pub hugepages: Vec<HugePool>,
    pub memlock_bytes: Option<u64>,
}
impl HostResources {
    pub fn probe(storage_path: &Path) -> Result<Self, String> {
        let mut system = sysinfo::System::new().map_err(|e| format!("host discovery: {e}"))?;
        system.refresh_memory();
        system.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
        let mut path = storage_path.to_path_buf();
        while !path.exists() {
            if !path.pop() {
                return Err("cannot find the storage filesystem".into());
            }
        }
        let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        let stat = rustix::fs::statvfs(&path).map_err(|e| format!("storage capacity: {e}"))?;
        let disks = sysinfo::Disks::new_with_refreshed_list()
            .map_err(|e| format!("storage discovery: {e}"))?;
        let storage = disks
            .iter()
            .take(256)
            .map(|disk| Storage {
                device: disk.name().to_string_lossy().into_owned(),
                mount: disk.mount_point().to_path_buf(),
                filesystem: disk.file_system().to_string_lossy().into_owned(),
                kind: format!("{:?}", disk.kind()),
                total_mib: disk.total_space() >> 20,
                available_mib: disk.available_space() >> 20,
            })
            .collect();
        let cores = platform::cores()?;
        let physical_cores = if cores.is_empty() {
            sysinfo::System::physical_core_count().unwrap_or(0)
        } else {
            cores.len()
        };
        let logical_cpus = if cores.is_empty() {
            system.cpus().len()
        } else {
            cores.iter().map(|c| c.threads.len()).sum()
        };
        if physical_cores == 0 || system.total_memory() == 0 {
            return Err("host CPU or memory topology is unavailable".into());
        }
        Ok(Self {
            npus: vkore_devices::discover(),
            cpu_name: system
                .cpus()
                .first()
                .map(|c| c.brand().to_owned())
                .unwrap_or_default(),
            physical_cores,
            logical_cpus,
            cores,
            core_summary: platform::summary(),
            memory_nodes: platform::memory_nodes(),
            memory_mib: system.total_memory() >> 20,
            available_memory_mib: system.available_memory() >> 20,
            free_memory_mib: system.free_memory() >> 20,
            storage,
            storage_path: path,
            storage_available_mib: stat.f_bavail.saturating_mul(stat.f_frsize) >> 20,
            cpu_pinning: cfg!(target_os = "linux"),
            memory_pinning: cfg!(target_os = "linux"),
            system_page_bytes: rustix::param::page_size(),
            hugetlb_supported: cfg!(target_os = "linux"),
            hugepages: platform::hugepages(),
            memlock_bytes: rustix::process::getrlimit(rustix::process::Resource::Memlock).current,
        })
    }
    /// Spread over physical cores before using SMT siblings.
    pub fn preferred_cpus(&self, count: u16) -> Vec<u32> {
        let mut ids = Vec::new();
        for sibling in 0..self
            .cores
            .iter()
            .map(|c| c.threads.len())
            .max()
            .unwrap_or(0)
        {
            for core in &self.cores {
                if let Some(id) = core.threads.get(sibling) {
                    ids.push(*id);
                }
            }
        }
        ids.truncate(usize::from(count));
        ids
    }
    pub fn huge_available(&self, page: PageSize, memory_mib: u32) -> bool {
        page == PageSize::System
            || (self.hugetlb_supported
                && self
                    .hugepages
                    .iter()
                    .any(|pool| pool.page == page && pool.free_mib >= u64::from(memory_mib)))
    }
}

pub fn validate_binding(resources: &Resources, cpus: u16) -> Result<(), String> {
    if resources.host_cpus.len() > 128 {
        return Err("at most 128 host CPU selections".into());
    }
    if resources.pin_cpus
        && (resources.host_cpus.len() != usize::from(cpus)
            || resources.host_cpus.iter().collect::<BTreeSet<_>>().len() != usize::from(cpus))
    {
        return Err("select one distinct host logical CPU for every guest CPU".into());
    }
    Ok(())
}

/// Called only while guest CPUs are stopped. Verify QMP TIDs belong to this
/// QEMU child before using them; never trust a peer-supplied PID as authority.
pub fn bind_vcpus(pid: u32, threads: &[(i64, i64)], resources: &Resources) -> Result<(), String> {
    if !resources.pin_cpus {
        return Ok(());
    }
    platform::bind(pid, threads, &resources.host_cpus)
}

#[cfg(any(target_os = "linux", test))]
fn cpu_list_contains(list: &str, cpu: u32) -> bool {
    list.trim().split(',').any(|part| {
        if let Some((first, last)) = part.split_once('-') {
            matches!((first.parse::<u32>(),last.parse::<u32>()),(Ok(a),Ok(b)) if a<=cpu&&cpu<=b)
        } else {
            part.parse::<u32>() == Ok(cpu)
        }
    })
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use rustix::thread::{CpuSet, sched_getaffinity, sched_setaffinity};
    use std::{fs::File, io::Read};
    fn read(path: impl AsRef<Path>) -> Option<String> {
        let mut value = String::new();
        File::open(path)
            .ok()?
            .take(8192)
            .read_to_string(&mut value)
            .ok()?;
        Some(value.trim().to_owned())
    }
    fn number(path: impl AsRef<Path>) -> Option<u32> {
        read(path)?.parse().ok()
    }
    pub fn cores() -> Result<Vec<Core>, String> {
        let allowed = sched_getaffinity(None).map_err(|e| format!("CPU affinity: {e}"))?;
        let mut cores: Vec<Core> = Vec::new();
        let efficiency = read("/sys/bus/event_source/devices/cpu_atom/cpus").unwrap_or_default();
        let performance = read("/sys/bus/event_source/devices/cpu_core/cpus").unwrap_or_default();
        for id in 0..CpuSet::MAX_CPU {
            if !allowed.is_set(id) {
                continue;
            }
            let base = PathBuf::from(format!("/sys/devices/system/cpu/cpu{id}/topology"));
            let package = number(base.join("physical_package_id"))
                .ok_or("CPU package topology unavailable")?;
            let core_id = number(base.join("core_id")).ok_or("CPU core topology unavailable")?;
            // The kernel's Intel hybrid PMUs expose reliable per-type CPU
            // lists; do not infer P/E identity from frequency or SMT count.
            let kind = if cpu_list_contains(&efficiency, id as u32) {
                "Efficiency"
            } else if cpu_list_contains(&performance, id as u32) {
                "Performance"
            } else {
                "Core"
            }
            .to_owned();
            let numa_node = std::fs::read_dir(base.parent().unwrap())
                .ok()
                .into_iter()
                .flatten()
                .take(256)
                .filter_map(Result::ok)
                .find_map(|e| e.file_name().to_str()?.strip_prefix("node")?.parse().ok());
            if let Some(core) = cores
                .iter_mut()
                .find(|c| c.package == package && c.id == core_id)
            {
                core.threads.push(id as u32);
            } else {
                cores.push(Core {
                    package,
                    id: core_id,
                    kind,
                    threads: vec![id as u32],
                    numa_node,
                });
            }
        }
        Ok(cores)
    }
    pub fn memory_nodes() -> Vec<MemoryNode> {
        let mut nodes = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/sys/devices/system/node") {
            for entry in entries.take(4096).filter_map(Result::ok) {
                let Some(id) = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix("node"))
                    .and_then(|n| n.parse().ok())
                else {
                    continue;
                };
                let info = read(entry.path().join("meminfo")).unwrap_or_default();
                let memory_mib = info
                    .lines()
                    .find_map(|line| {
                        line.split_once("MemTotal:")
                            .and_then(|(_, n)| n.split_whitespace().next()?.parse::<u64>().ok())
                    })
                    .unwrap_or(0)
                    / 1024;
                nodes.push(MemoryNode {
                    id,
                    memory_mib,
                    cpus: read(entry.path().join("cpulist")).unwrap_or_default(),
                });
            }
        }
        nodes.sort_by_key(|n| n.id);
        nodes
    }
    pub fn summary() -> String {
        "Physical cores and SMT threads available to vKore".into()
    }
    pub fn hugepages() -> Vec<HugePool> {
        [PageSize::Huge2M, PageSize::Huge1G]
            .into_iter()
            .filter_map(|page| {
                let base = format!("/sys/kernel/mm/hugepages/hugepages-{}kB", page.mib() * 1024);
                use rustix::fs::{MemfdFlags, memfd_create};
                let size = if page == PageSize::Huge2M {
                    MemfdFlags::HUGE_2MB
                } else {
                    MemfdFlags::HUGE_1GB
                };
                // Tests privilege and kernel page-size support without consuming pages.
                let _fd = memfd_create(
                    c"vkore-page-probe",
                    MemfdFlags::CLOEXEC | MemfdFlags::HUGETLB | size,
                )
                .ok()?;
                let free = number(format!("{base}/free_hugepages"))?;
                let reserved = number(format!("{base}/resv_hugepages"))?;
                Some(HugePool {
                    page,
                    free_mib: u64::from(free.saturating_sub(reserved)) * u64::from(page.mib()),
                })
            })
            .collect()
    }
    pub fn bind(pid: u32, threads: &[(i64, i64)], ids: &[u32]) -> Result<(), String> {
        let allowed = sched_getaffinity(None).map_err(|e| e.to_string())?;
        let mut seen = BTreeSet::new();
        for &(index, tid) in threads {
            let index = usize::try_from(index).map_err(|_| "invalid QMP CPU index")?;
            let cpu = *ids.get(index).ok_or("unexpected QMP CPU index")? as usize;
            let tid = u32::try_from(tid).map_err(|_| "invalid QMP thread ID")?;
            if !seen.insert(index)
                || cpu >= CpuSet::MAX_CPU
                || !allowed.is_set(cpu)
                || !Path::new(&format!("/proc/{pid}/task/{tid}")).is_dir()
            {
                return Err("QEMU thread or selected host CPU is unavailable".into());
            }
            let tid = rustix::process::Pid::from_raw(tid as i32).ok_or("invalid thread ID")?;
            let mut mask = CpuSet::new();
            mask.set(cpu);
            sched_setaffinity(Some(tid), &mask).map_err(|e| format!("CPU pinning: {e}"))?;
            if sched_getaffinity(Some(tid)).map_err(|e| e.to_string())? != mask {
                return Err("CPU affinity verification failed".into());
            }
        }
        if seen.len() != ids.len() {
            return Err("QEMU did not report every selected CPU".into());
        }
        Ok(())
    }
}
#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;
    pub fn cores() -> Result<Vec<Core>, String> {
        Ok(Vec::new())
    }
    pub fn memory_nodes() -> Vec<MemoryNode> {
        Vec::new()
    }
    pub fn hugepages() -> Vec<HugePool> {
        Vec::new()
    }
    pub fn bind(_: u32, _: &[(i64, i64)], _: &[u32]) -> Result<(), String> {
        Err("exact host CPU pinning is not available on this operating system".into())
    }
    pub fn summary() -> String {
        let mut groups = Vec::new();
        #[cfg(target_os = "macos")]
        for level in 0..8 {
            let query = |field: &str| {
                let output = std::process::Command::new("/usr/sbin/sysctl")
                    .args(["-n", &format!("hw.perflevel{level}.{field}")])
                    .output()
                    .ok()?;
                output
                    .status
                    .success()
                    .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            };
            if let (Some(name), Some(count)) = (query("name"), query("physicalcpu"))
                && !name.is_empty()
                && count.parse::<u32>().is_ok_and(|n| n > 0)
            {
                groups.push(format!("{count} {name}"));
            }
        }
        if groups.is_empty() {
            "The system manages core placement".into()
        } else {
            groups.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hybrid_lists_handle_sparse_ids_and_ranges_without_expansion() {
        for cpu in [0, 2, 3, 4, 18] {
            assert!(cpu_list_contains("0,2-4,18", cpu));
        }
        assert!(!cpu_list_contains("0,2-4,18", 1));
        assert!(!cpu_list_contains("bad,8-2", 4));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn pins_and_verifies_a_real_owned_thread() {
        let (send, recv) = std::sync::mpsc::channel();
        let (done, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(rustix::thread::gettid().as_raw_nonzero().get())
                .unwrap();
            wait.recv().unwrap();
        });
        let tid = recv.recv().unwrap();
        let allowed = rustix::thread::sched_getaffinity(None).unwrap();
        let cpu = (0..rustix::thread::CpuSet::MAX_CPU)
            .find(|n| allowed.is_set(*n))
            .unwrap() as u32;
        let resources = Resources {
            pin_cpus: true,
            host_cpus: vec![cpu],
            ..Default::default()
        };
        let result = bind_vcpus(std::process::id(), &[(0, i64::from(tid))], &resources);
        done.send(()).unwrap();
        worker.join().unwrap();
        result.unwrap();
        assert!(bind_vcpus(std::process::id(), &[(0, -1)], &resources).is_err());
    }
    #[test]
    fn binding_requires_distinct_complete_assignments() {
        let mut r = Resources {
            pin_cpus: true,
            host_cpus: vec![0, 0],
            ..Resources::default()
        };
        assert!(validate_binding(&r, 2).is_err());
        r.host_cpus = vec![0, 5];
        assert!(validate_binding(&r, 2).is_ok());
        assert!(validate_binding(&r, 3).is_err());
    }
    #[test]
    fn real_host_snapshot_has_memory_and_storage() {
        let host = HostResources::probe(&std::env::temp_dir()).unwrap();
        assert!(host.memory_mib > 0 && host.physical_cores > 0);
        assert_eq!(host.system_page_bytes, rustix::param::page_size());
        assert!(host.system_page_bytes.is_power_of_two());
        assert!(host.huge_available(PageSize::System, u32::MAX));
        if !host.hugetlb_supported {
            assert!(!host.huge_available(PageSize::Huge2M, 2048));
            assert!(!host.huge_available(PageSize::Huge1G, 2048));
        }
        assert!(!host.storage.is_empty());
    }
}
