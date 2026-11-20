//! Partially huge shmem backing without changing the guest's NUMA topology.
use crate::hugepages::{Promotion, promote};
use rustix::{
    fs::{FallocateFlags, MemfdFlags},
    mm::{Advice, MapFlags, ProtFlags},
};
use std::{fs::File, time::Instant};

const HUGE: usize = 2 << 20;
const CHUNK_MIB: u32 = 256;

/// Own the full reservation, including alignment padding. The file overlay is
/// always wholly inside it; no existing process mapping can be overwritten.
struct Mapping {
    reservation: *mut std::ffi::c_void,
    reserved: usize,
    data: *mut std::ffi::c_void,
    len: usize,
}
impl Mapping {
    fn new(file: &File, offset: u64, len: usize) -> Result<Self, String> {
        let reserved = len.checked_add(HUGE).ok_or("Mapping size overflow")?;
        // SAFETY: no requested address, no memory access through this reservation.
        let reservation = unsafe {
            rustix::mm::mmap_anonymous(
                std::ptr::null_mut(),
                reserved,
                ProtFlags::empty(),
                MapFlags::PRIVATE,
            )
        }
        .map_err(|e| e.to_string())?;
        let data = ((reservation as usize).next_multiple_of(HUGE)) as *mut std::ffi::c_void;
        let mapping = Self {
            reservation,
            reserved,
            data,
            len,
        };
        // SAFETY: aligned data..data+len is contained in our exclusive reservation.
        // The owned file is populated, and its size cannot change during this map.
        unsafe {
            rustix::mm::mmap(
                data,
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED | MapFlags::FIXED,
                file,
                offset,
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(mapping)
    }
    fn populate(&self) -> Result<bool, String> {
        // SAFETY: both calls cover exactly this live writable mapping. Populate
        // reports faults as errno instead of risking SIGBUS from pointer writes.
        unsafe {
            let _ = rustix::mm::madvise(self.data, self.len, Advice::LinuxHugepage);
            match rustix::mm::madvise(self.data, self.len, Advice::LinuxPopulateWrite) {
                Ok(()) => Ok(true),
                Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS) => Ok(false),
                Err(error) => Err(format!("Cannot populate guest memory: {error}")),
            }
        }
    }
    fn collapse(&self, offset_mib: u32) -> Promotion {
        let offset = (offset_mib as usize) << 20;
        assert!(offset + HUGE <= self.len);
        // SAFETY: the selected 2 MiB range is aligned and wholly within the live
        // writable file mapping. MADV_COLLAPSE does not invalidate its addresses.
        let rc = unsafe { libc::madvise(self.data.byte_add(offset), HUGE, libc::MADV_COLLAPSE) };
        if rc == 0 {
            return Promotion::Huge;
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ENOMEM | libc::EAGAIN | libc::EBUSY) => Promotion::Base,
            _ => Promotion::Stop,
        }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: this is the original reservation, including our file overlay;
        // it is owned only here and no pointers or borrowed slices escape.
        let _ = unsafe { rustix::mm::munmap(self.reservation, self.reserved) };
    }
}

pub(super) fn file(
    memory_mib: u32,
    deadline: Instant,
    mut cancelled: impl FnMut() -> Result<bool, String>,
) -> Result<(File, u32), String> {
    if memory_mib == 0 || memory_mib > 1 << 20 || !memory_mib.is_multiple_of(2) {
        return Err("Invalid transparent memory size.".into());
    }
    let file = File::from(
        rustix::fs::memfd_create(
            c"vkore-guest-mixed",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .map_err(|e| e.to_string())?,
    );
    file.set_len(u64::from(memory_mib) << 20)
        .map_err(|e| e.to_string())?;
    let mut confirmed = 0;
    let mut try_huge = true;
    for offset in (0..memory_mib).step_by(CHUNK_MIB as usize) {
        if cancelled()? {
            return Err("Memory preparation was cancelled.".into());
        }
        let chunk = CHUNK_MIB.min(memory_mib - offset);
        let len = (chunk as usize) << 20;
        // The full configured RAM must still be backed, even after the optional
        // promotion budget expires. This failure is a real allocation failure.
        rustix::fs::fallocate(
            &file,
            FallocateFlags::empty(),
            u64::from(offset) << 20,
            len as u64,
        )
        .map_err(|e| format!("Cannot allocate guest RAM, even with system pages: {e}"))?;
        if !try_huge || Instant::now() >= deadline {
            continue;
        }
        let mapping = match Mapping::new(&file, u64::from(offset) << 20, len) {
            Ok(mapping) => mapping,
            Err(error) => {
                eprintln!(
                    "vKore: transparent promotion mapping unavailable ({error}); retaining base-page backing."
                );
                try_huge = false;
                continue;
            }
        };
        match mapping.populate() {
            Ok(true) => {}
            result => {
                eprintln!(
                    "vKore: transparent promotion prefault unavailable ({result:?}); retaining allocated backing."
                );
                try_huge = false;
                continue;
            }
        }
        confirmed += promote(chunk, |offset| {
            if cancelled()? {
                return Err("Memory preparation was cancelled.".into());
            }
            if Instant::now() >= deadline {
                return Ok(Promotion::Stop);
            }
            let result = mapping.collapse(offset);
            if matches!(result, Promotion::Stop) {
                try_huge = false;
            }
            Ok(result)
        })?;
    }
    Ok((file, confirmed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::FileExt, time::Duration};
    #[test]
    fn unprivileged_qemu_maps_the_complete_fallback_file() {
        use std::{
            os::{
                fd::{AsRawFd, OwnedFd},
                unix::{net::UnixStream, process::CommandExt},
            },
            process::{Command, Stdio},
        };
        let Some(qemu) = std::env::var_os("VKORE_TEST_MEMORY_QEMU").filter(|s| !s.is_empty())
        else {
            return;
        };
        for deadline in [Instant::now(), Instant::now() + Duration::from_secs(5)] {
            let (memory, promoted) = file(128, deadline, || Ok(false)).unwrap();
            assert!(promoted <= 128);
            rustix::fs::fcntl_add_seals(
                &memory,
                rustix::fs::SealFlags::GROW
                    | rustix::fs::SealFlags::SHRINK
                    | rustix::fs::SealFlags::SEAL,
            )
            .unwrap();
            let (stream, emulator) = UnixStream::pair().unwrap();
            let mut command = Command::new(&qemu);
            command.args(["-machine", "q35", "-accel", "tcg", "-S", "-nodefaults", "-display", "none", "-m", "128M", "-object"])
                .arg(format!("memory-backend-file,id=ram0,size=128M,mem-path=/proc/self/fd/{},share=on,prealloc=on,align=2097152", memory.as_raw_fd()))
                .args(["-numa", "node,nodeid=0,memdev=ram0", "-qmp", "stdio"])
                .stdin(Stdio::from(OwnedFd::from(emulator.try_clone().unwrap())))
                .stdout(Stdio::from(OwnedFd::from(emulator))).stderr(Stdio::inherit());
            // SAFETY: child-only fcntl syscall; the owned FD is prepared before fork.
            unsafe {
                command.pre_exec(move || {
                    rustix::io::fcntl_setfd(&memory, rustix::io::FdFlags::empty())
                        .map_err(Into::into)
                });
            }
            let child = command.spawn().unwrap();
            drop(command);
            crate::passthrough::supervision::probe_paused(child, stream).unwrap();
        }
    }
    #[test]
    fn real_shmem_remains_complete_when_promotion_is_unavailable_or_partial() {
        for deadline in [Instant::now(), Instant::now() + Duration::from_secs(5)] {
            let (file, promoted) = file(8, deadline, || Ok(false)).unwrap();
            assert_eq!(file.metadata().unwrap().len(), 8 << 20);
            assert!(promoted <= 8 && promoted.is_multiple_of(2));
            let mut bytes = [1; 4];
            file.read_exact_at(&mut bytes, (8 << 20) - 4).unwrap();
            assert_eq!(bytes, [0; 4]);
            file.write_all_at(b"Kore", (8 << 20) - 4).unwrap();
            file.read_exact_at(&mut bytes, (8 << 20) - 4).unwrap();
            assert_eq!(&bytes, b"Kore");
        }
    }
    #[test]
    fn cancellation_does_not_become_fallback() {
        assert!(
            file(8, Instant::now(), || Ok(true))
                .unwrap_err()
                .contains("cancelled")
        );
    }
}
