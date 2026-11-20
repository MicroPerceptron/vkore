//! Root allocates anonymous hugepage files; QEMU receives only those files.
//! Surplus pages return to ordinary RAM when the last mapping/FD is released.
use super::*;
use crate::hardware::{GuestNuma, PageSize};
use rustix::fs::{FallocateFlags, MemfdFlags, Mode, SealFlags};
use std::io::{Seek, SeekFrom};

fn base(page: PageSize) -> Result<PathBuf, String> {
    if page == PageSize::System {
        return Err("System pages do not need hugepage provisioning.".into());
    }
    let path = PathBuf::from(format!(
        "/sys/kernel/mm/hugepages/hugepages-{}kB",
        page.mib() * 1024
    ));
    if !path.is_dir() {
        return Err(format!(
            "{} is not supported by this host kernel.",
            page.label()
        ));
    }
    Ok(path)
}
fn number(path: impl AsRef<Path>) -> Result<u64, String> {
    read(path)?
        .parse()
        .map_err(|_| "Invalid kernel hugepage counter.".into())
}

pub(super) fn check(memory_mib: u32, page: PageSize, numa: GuestNuma) -> Result<(), String> {
    crate::hugepages::validate(memory_mib, page, numa)?;
    if page == PageSize::System {
        return Ok(());
    }
    let path = base(page)?;
    let free =
        number(path.join("free_hugepages"))?.saturating_sub(number(path.join("resv_hugepages"))?);
    let mut system = sysinfo::System::new().map_err(|e| e.to_string())?;
    system.refresh_memory();
    if u64::from(memory_mib)
        > (system.available_memory() >> 20).saturating_add(free * u64::from(page.mib()))
    {
        return Err(
            "Not enough available host RAM and unreserved hugepages for this guest.".into(),
        );
    }
    Ok(())
}

/// The journal is synced before the global policy changes. Normal errors restore
/// immediately; after SIGKILL, the next request for this size recovers it. Never
/// overwrite a policy value changed by an external host administrator.
struct Allowance {
    path: PathBuf,
    journal: File,
    change: Option<crate::hugepages::AllowanceChange>,
}
impl Allowance {
    fn new(page: PageSize, needed: u64) -> Result<Self, String> {
        let path = base(page)?;
        let journal = lease(&format!("hugepages-{}", page.mib()))?;
        Self::begin(path, journal, needed)
    }
    fn begin(path: PathBuf, journal: File, needed: u64) -> Result<Self, String> {
        let mut guard = Self {
            path,
            journal,
            change: None,
        };
        let mut bytes = Vec::new();
        Read::by_ref(&mut guard.journal)
            .take(257)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 256 {
            return Err("Invalid hugepage recovery journal.".into());
        }
        if !bytes.is_empty() {
            guard.change = Some(
                serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Invalid hugepage recovery journal: {e}"))?,
            );
            guard.restore()?;
        }
        let before = guard.current()?;
        let raised = crate::hugepages::allowance(
            before,
            number(guard.path.join("surplus_hugepages"))?,
            number(guard.path.join("free_hugepages"))?,
            number(guard.path.join("resv_hugepages"))?,
            needed,
        )?;
        if raised != before {
            let change = crate::hugepages::AllowanceChange { before, raised };
            guard
                .journal
                .seek(SeekFrom::Start(0))
                .map_err(|e| e.to_string())?;
            guard
                .journal
                .write_all(&serde_json::to_vec(&change).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            guard.journal.sync_all().map_err(|e| e.to_string())?;
            guard.change = Some(change);
            write(
                guard.path.join("nr_overcommit_hugepages"),
                &raised.to_string(),
            )?;
            if guard.current()? != raised {
                return Err("This kernel cannot provision the requested hugepage size dynamically. Reserve a pool at host boot, use 2 MiB pages, or choose Automatic.".into());
            }
        }
        Ok(guard)
    }
    fn current(&self) -> Result<u64, String> {
        number(self.path.join("nr_overcommit_hugepages"))
    }
    fn restore(&mut self) -> Result<(), String> {
        if let Some(change) = self.change {
            let current = self.current()?;
            if let Some(value) = change.restore_value(current) {
                write(
                    self.path.join("nr_overcommit_hugepages"),
                    &value.to_string(),
                )?;
            } else if current != change.before {
                eprintln!(
                    "vKore: hugepage policy changed externally; preserving its current value."
                );
            }
            self.journal.set_len(0).map_err(|e| e.to_string())?;
            self.journal.sync_all().map_err(|e| e.to_string())?;
            self.change = None;
        }
        Ok(())
    }
}
impl Drop for Allowance {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!(
                "vKore: HUGEPAGE POLICY RESTORATION FAILED: {error}; recovery journal retained."
            );
        }
    }
}

pub(super) fn allocate(
    parent: u32,
    memory_mib: u32,
    page: PageSize,
    numa: GuestNuma,
) -> Result<Vec<File>, String> {
    check(memory_mib, page, numa)?;
    if page == PageSize::System {
        return Ok(Vec::new());
    }
    let identity = caller(parent)?;
    let mut allowance = Allowance::new(page, u64::from(memory_mib / page.mib()))?;
    let result = (|| {
        let mut files = Vec::new();
        let bytes = u64::from(memory_mib / numa.nodes()) << 20;
        let chunk = (256_u64 << 20).max(u64::from(page.mib()) << 20);
        for _ in 0..numa.nodes() {
            let size = match page {
                PageSize::Huge2M => MemfdFlags::HUGE_2MB,
                PageSize::Huge1G => MemfdFlags::HUGE_1GB,
                PageSize::System => unreachable!(),
            };
            let file = File::from(
                rustix::fs::memfd_create(
                    c"vkore-guest-ram",
                    MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::HUGETLB | size,
                )
                .map_err(|e| e.to_string())?,
            );
            file.set_len(bytes).map_err(|e| e.to_string())?;
            // Populate before driver handoff and before QEMU maps anything. A
            // failed allocation is an errno, never a SIGBUS in the guardian.
            let mut offset = 0;
            while offset < bytes {
                if cancelled()? {
                    return Err("Hugepage allocation was cancelled.".into());
                }
                let len = chunk.min(bytes - offset);
                rustix::fs::fallocate(&file, FallocateFlags::empty(), offset, len).map_err(|e| format!("Cannot allocate {} MiB using {}: {e}. Free RAM may be fragmented or this page size may require a pool reserved at host boot. Reduce guest RAM, use 2 MiB pages, or choose Automatic.", memory_mib, page.label()))?;
                offset += len;
            }
            rustix::fs::fchown(
                &file,
                Some(rustix::process::Uid::from_raw(identity.uid)),
                Some(rustix::process::Gid::from_raw(identity.gid)),
            )
            .map_err(|e| e.to_string())?;
            rustix::fs::fchmod(&file, Mode::RUSR | Mode::WUSR).map_err(|e| e.to_string())?;
            rustix::fs::fcntl_add_seals(
                &file,
                SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
            )
            .map_err(|e| e.to_string())?;
            files.push(file);
        }
        Ok(files)
    })();
    // Do not leave a larger surplus allowance for unrelated future allocations.
    // Lowering it does not evict the pages already owned by these files.
    allowance.restore()?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, value) in [
            ("nr_overcommit_hugepages", "20"),
            ("free_hugepages", "0"),
            ("resv_hugepages", "0"),
            ("surplus_hugepages", "0"),
        ] {
            fs::write(dir.path().join(name), value).unwrap();
        }
        dir
    }
    fn begin(path: &Path, needed: u64) -> Result<Allowance, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join("journal"))
            .unwrap();
        Allowance::begin(path.to_owned(), file, needed)
    }
    #[test]
    fn restores_after_failed_allocation_and_preserves_external_policy() {
        let dir = fixture();
        let path = dir.path().join("nr_overcommit_hugepages");
        let allocation: Result<(), String> = (|| {
            let _allowance = begin(dir.path(), 4096)?;
            assert_eq!(number(&path)?, 4096);
            Err("injected partial allocation failure".into())
        })();
        assert!(allocation.is_err());
        assert_eq!(number(&path).unwrap(), 20);
        assert_eq!(fs::read(dir.path().join("journal")).unwrap(), b"");
        let allowance = begin(dir.path(), 4096).unwrap();
        fs::write(&path, "8192").unwrap();
        drop(allowance);
        assert_eq!(number(path).unwrap(), 8192);
    }
    #[test]
    fn recovers_interrupted_policy_before_the_next_allocation() {
        let dir = fixture();
        let path = dir.path().join("nr_overcommit_hugepages");
        fs::write(&path, "4096").unwrap();
        fs::write(
            dir.path().join("journal"),
            br#"{"before":20,"raised":4096}"#,
        )
        .unwrap();
        let mut guard = begin(dir.path(), 1024).unwrap();
        assert_eq!(number(&path).unwrap(), 1024);
        guard.restore().unwrap();
        assert_eq!(number(path).unwrap(), 20);
    }
    #[test]
    fn failed_restoration_keeps_the_recovery_record() {
        let dir = fixture();
        let path = dir.path().join("nr_overcommit_hugepages");
        let mut guard = begin(dir.path(), 4096).unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(guard.restore().is_err());
        assert!(!fs::read(dir.path().join("journal")).unwrap().is_empty());
        fs::remove_dir(&path).unwrap();
        fs::write(&path, "4096").unwrap();
        drop(guard);
        assert_eq!(number(path).unwrap(), 20);
        assert!(fs::read(dir.path().join("journal")).unwrap().is_empty());
    }
    #[test]
    fn malformed_recovery_record_does_not_change_the_host() {
        let dir = fixture();
        fs::write(dir.path().join("journal"), [b'x'; 257]).unwrap();
        assert!(begin(dir.path(), 4096).is_err());
        assert_eq!(
            number(dir.path().join("nr_overcommit_hugepages")).unwrap(),
            20
        );
    }
}
