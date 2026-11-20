//! Bounded hugepage backing contract shared by the launcher and its helper.
use crate::hardware::{GuestNuma, PageSize};

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AllowanceChange {
    pub before: u64,
    pub raised: u64,
}
impl AllowanceChange {
    pub fn restore_value(self, current: u64) -> Option<u64> {
        (current == self.raised && self.raised > self.before).then_some(self.before)
    }
}

pub(crate) fn validate(memory_mib: u32, page: PageSize, numa: GuestNuma) -> Result<(), String> {
    if !(2048..=1 << 20).contains(&memory_mib) || !memory_mib.is_multiple_of(numa.alignment(page)) {
        return Err("Invalid hugepage memory size or guest NUMA alignment.".into());
    }
    Ok(())
}

/// Only replace the exact backends emitted by profile::numa. Shared mappings
/// reuse the helper's populated pages instead of allocating private COW pages.
pub(crate) fn fd_arguments(
    args: &mut [String],
    memory_mib: u32,
    page: PageSize,
    numa: GuestNuma,
    fds: &[i32],
) -> Result<(), String> {
    validate(memory_mib, page, numa)?;
    if page == PageSize::System {
        return if fds.is_empty() {
            Ok(())
        } else {
            Err("Unexpected memory descriptors.".into())
        };
    }
    if fds.len() != numa.nodes() as usize || fds.iter().any(|fd| *fd < 3) {
        return Err("Invalid hugepage descriptor set.".into());
    }
    let per_node = memory_mib / numa.nodes();
    let mut positions = Vec::new();
    for node in 0..numa.nodes() {
        let expected = format!(
            "memory-backend-memfd,id=ram{node},size={per_node}M,hugetlb=on,hugetlbsize={},prealloc=on",
            u64::from(page.mib()) << 20
        );
        let found: Vec<_> = args
            .windows(2)
            .enumerate()
            .filter_map(|(i, pair)| (pair[0] == "-object" && pair[1] == expected).then_some(i + 1))
            .collect();
        if found.len() != 1 {
            return Err("QEMU hugepage arguments are missing or ambiguous.".into());
        }
        positions.push(found[0]);
    }
    for (node, (position, fd)) in positions.into_iter().zip(fds).enumerate() {
        args[position] = format!(
            "memory-backend-file,id=ram{node},size={per_node}M,mem-path=/proc/self/fd/{fd},share=on,prealloc=on"
        );
    }
    Ok(())
}

/// Add only the missing pages to the existing surplus allowance. A lowered
/// allowance may already be below current usage; account for that usage too.
pub(crate) fn allowance(
    current: u64,
    surplus: u64,
    free: u64,
    reserved: u64,
    needed: u64,
) -> Result<u64, String> {
    let missing = needed.saturating_sub(free.saturating_sub(reserved));
    if missing == 0 {
        return Ok(current);
    }
    Ok(current.max(
        surplus
            .checked_add(missing)
            .ok_or("Hugepage allowance overflow.")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_only_our_policy_and_preserve_external_changes() {
        let change = AllowanceChange {
            before: 20,
            raised: 4096,
        };
        assert_eq!(change.restore_value(4096), Some(20));
        assert_eq!(change.restore_value(20), None); // Interrupted before writing.
        assert_eq!(change.restore_value(8192), None); // Host administrator.
        assert_eq!(change.restore_value(0), None);
    }
    #[test]
    fn replaces_exact_single_and_split_backends_without_changing_ram() {
        for numa in [GuestNuma::Single, GuestNuma::Split] {
            for page in [PageSize::Huge2M, PageSize::Huge1G] {
                let mut args = crate::profile::numa(4, 8192, page, numa);
                let fds: Vec<_> = (7..7 + numa.nodes() as i32).collect();
                fd_arguments(&mut args, 8192, page, numa, &fds).unwrap();
                for (node, fd) in fds.iter().enumerate() {
                    assert!(args.contains(&format!("memory-backend-file,id=ram{node},size={}M,mem-path=/proc/self/fd/{fd},share=on,prealloc=on", 8192 / numa.nodes())));
                }
                assert!(!args.iter().any(|a| a.contains("hugetlb=on")));
            }
        }
    }
    #[test]
    fn refuses_mismatched_size_duplicates_alignment_and_descriptors() {
        let original = crate::profile::numa(4, 8192, PageSize::Huge2M, GuestNuma::Split);
        for (size, fds) in [
            (4096, vec![7, 8]),
            (8192, vec![7]),
            (8192, vec![0, 8]),
            (8193, vec![7, 8]),
        ] {
            let mut args = original.clone();
            assert!(
                fd_arguments(&mut args, size, PageSize::Huge2M, GuestNuma::Split, &fds).is_err()
            );
            assert_eq!(args, original);
        }
        let mut args = [original.clone(), original].concat();
        assert!(
            fd_arguments(&mut args, 8192, PageSize::Huge2M, GuestNuma::Split, &[7, 8]).is_err()
        );
        for size in [0, 2047, (1 << 20) + 1, u32::MAX] {
            assert!(validate(size, PageSize::Huge2M, GuestNuma::Single).is_err());
        }
    }
    #[test]
    fn allowance_preserves_existing_policy_and_reservations() {
        assert_eq!(allowance(0, 0, 0, 0, 4096).unwrap(), 4096);
        assert_eq!(allowance(100, 200, 100, 40, 1000).unwrap(), 1140);
        assert_eq!(allowance(5000, 0, 0, 0, 4096).unwrap(), 5000);
        assert_eq!(allowance(0, 200, 4096, 0, 4096).unwrap(), 0);
        assert!(allowance(0, u64::MAX, 0, 0, 1).is_err());
    }
}
