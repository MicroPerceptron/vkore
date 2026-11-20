//! Whole-device NPU assignment. Saved intent is not authority: the helper
//! independently rediscovers the device before touching the host driver.
use serde::{Deserialize, Serialize};
use vkore_devices::{Inventory, Npu, Source, Vendor};

pub const MAX_DEVICES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub address: String,
    pub vendor_id: u16,
    pub device_id: u16,
}
impl Selection {
    pub fn from_device(device: &Npu) -> Option<Self> {
        let pci = device.pci.as_ref()?;
        Some(Self {
            address: pci.address.clone(),
            vendor_id: pci.vendor_id,
            device_id: pci.device_id,
        })
    }
}

pub fn valid_address(s: &str) -> bool {
    s.len() == 12
        && s.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b':',
            10 => b == b'.',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
        && u8::from_str_radix(&s[8..10], 16).is_ok_and(|n| n < 32)
        && u8::from_str_radix(&s[11..12], 16).is_ok_and(|n| n < 8)
}
pub fn validate_selections(selected: &[Selection]) -> Result<(), String> {
    if selected.len() > MAX_DEVICES {
        return Err("At most four NPUs can be assigned to one guest.".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for device in selected {
        if !valid_address(&device.address) || !seen.insert(&device.address) {
            return Err("Invalid or duplicate NPU PCI address.".into());
        }
    }
    Ok(())
}

/// Inventory-level admission shared by views, configuration and privileged use.
/// Reset, kernel and QEMU capabilities are checked separately before detachment.
pub fn eligible(device: &Npu) -> Result<(), String> {
    if device.source != Source::LinuxPci || !matches!(device.vendor, Vendor::Intel | Vendor::Amd) {
        return Err(
            "Whole-device NPU passthrough is available on Linux for Intel and AMD PCI NPUs.".into(),
        );
    }
    let pci = device.pci.as_ref().ok_or("PCI identity is unavailable.")?;
    if pci.physical_function.is_some() {
        return Err(
            "This is a virtual function; whole-device assignment requires the physical NPU.".into(),
        );
    }
    if pci.enabled_vfs.is_some_and(|n| n > 0) {
        return Err(
            "Disable the NPU's virtual functions before assigning the whole device.".into(),
        );
    }
    if pci.total_vfs.is_none()
        || (pci.total_vfs.is_some_and(|n| n > 0) && pci.enabled_vfs.is_none())
    {
        return Err("Cannot verify that the NPU has no active virtual functions; refresh host discovery before assigning it.".into());
    }
    if pci.iommu_group.is_none() || pci.group_members != [pci.address.clone()] {
        return Err("Passthrough requires an IOMMU group containing only this NPU; refresh after changing host isolation.".into());
    }
    if !matches!(
        device.driver.as_deref(),
        None | Some("ivpu" | "intel_vpu" | "amdxdna" | "vfio-pci")
    ) {
        return Err("This NPU's host driver is not supported for managed handoff.".into());
    }
    Ok(())
}

pub fn validate_inventory(selected: &[Selection], inventory: &Inventory) -> Result<(), String> {
    validate_selections(selected)?;
    for selection in selected {
        let device = inventory.devices.iter().find(|n| Selection::from_device(n).as_ref() == Some(selection))
            .ok_or_else(|| format!("Selected NPU {} is missing or its identity changed. Turn off its passthrough selection or refresh the host.", selection.address))?;
        eligible(device)?;
    }
    Ok(())
}

/// Guest-side PCIe ports and the iommufd object. The helper substitutes already
/// opened FDs for host paths; no VFIO permissions or ACLs are relaxed globally.
pub(crate) fn arguments(selected: &[Selection]) -> Vec<String> {
    let mut args = Vec::new();
    if !selected.is_empty() {
        args.extend(["-object".into(), "iommufd,id=vkore_iommu".into()]);
    }
    for (i, device) in selected.iter().enumerate() {
        args.extend([
            "-device".into(),
            format!(
                "pcie-root-port,id=npu_port{i},bus=pcie.0,chassis={},slot={}",
                10 + i,
                10 + i
            ),
            "-device".into(),
            format!(
                "vfio-pci,host={},id=npu{i},bus=npu_port{i},iommufd=vkore_iommu",
                device.address
            ),
        ]);
    }
    args
}

#[cfg(any(target_os = "linux", test))]
fn fd_arguments(args: &mut [String], selected: &[Selection], fds: &[i32]) -> Result<(), String> {
    if fds.len() != selected.len() + 1 || fds.iter().any(|n| *n < 3) {
        return Err("Invalid passthrough descriptor set.".into());
    }
    let replace = |args: &mut [String], expected: &str, replacement: String| {
        let positions: Vec<_> = args
            .iter()
            .enumerate()
            .filter_map(|(i, a)| (a == expected).then_some(i))
            .collect();
        if positions.len() != 1 {
            return Err("QEMU passthrough arguments are missing or ambiguous.".to_owned());
        }
        args[positions[0]] = replacement;
        Ok(())
    };
    replace(
        args,
        "iommufd,id=vkore_iommu",
        format!("iommufd,id=vkore_iommu,fd={}", fds[0]),
    )?;
    for (i, selection) in selected.iter().enumerate() {
        replace(
            args,
            &format!(
                "vfio-pci,host={},id=npu{i},bus=npu_port{i},iommufd=vkore_iommu",
                selection.address
            ),
            format!(
                "vfio-pci,fd={},id=npu{i},bus=npu_port{i},iommufd=vkore_iommu",
                fds[i + 1]
            ),
        )?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
mod handoff;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", test))]
mod supervision;
#[cfg(target_os = "linux")]
pub(crate) use linux::launch;

/// Called before CLI/desktop initialization, only for the private helper mode.
pub fn helper_entry() -> Option<i32> {
    let mode = std::env::args_os().nth(1)?;
    if mode != "--vkore-device-helper" && mode != "--vkore-memory-helper" {
        return None;
    }
    #[cfg(target_os = "linux")]
    return Some(if mode == "--vkore-memory-helper" {
        linux::memory_entry()
    } else {
        linux::entry()
    });
    #[cfg(not(target_os = "linux"))]
    Some(1)
}

/// Private polkit mode with an application-specific prompt. The existing
/// memory-only flow raises the calling vKore process's limit, not QEMU as root.
#[cfg(target_os = "linux")]
pub fn authorize_memory_limit(memory_mib: u32) -> Result<(), String> {
    linux::authorize_memory_limit(memory_mib)
}

/// Batch the device checks and optional memory-limit request at Done/Save.
/// No device is detached by this operation. Launch rechecks with fresh authority.
pub fn authorize(selected: &[Selection], memory_bytes: Option<u64>) -> Result<(), String> {
    validate_selections(selected)?;
    #[cfg(target_os = "linux")]
    return linux::authorize(selected, memory_bytes);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = memory_bytes;
        Err("NPU passthrough requires Linux.".into())
    }
}

/// Unprivileged checks that should fail before showing an authentication dialog.
pub fn check_host(selected: &[Selection]) -> Result<(), String> {
    if selected.is_empty() {
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    return linux::check_host(selected);
    #[cfg(not(target_os = "linux"))]
    Err("NPU passthrough requires Linux.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(crate) fn fixture() -> Npu {
        Npu {
            id: "pci:0000:00:0b.0".into(),
            name: "Intel NPU".into(),
            vendor: Vendor::Intel,
            source: Source::LinuxPci,
            driver: Some("intel_vpu".into()),
            core_count: None,
            pci: Some(vkore_devices::Pci {
                address: "0000:00:0b.0".into(),
                vendor_id: 0x8086,
                device_id: 0x643e,
                revision: Some(1),
                iommu_group: Some(2),
                group_members: vec!["0000:00:0b.0".into()],
                physical_function: None,
                total_vfs: Some(0),
                enabled_vfs: None,
            }),
        }
    }
    #[test]
    fn assignments_require_matching_identity_and_exclusive_physical_devices() {
        let device = fixture();
        let selection = Selection::from_device(&device).unwrap();
        let mut inventory = Inventory::empty(vkore_devices::Platform::Linux);
        inventory.devices.push(device);
        validate_inventory(std::slice::from_ref(&selection), &inventory).unwrap();
        inventory.devices[0].pci.as_mut().unwrap().device_id += 1;
        assert!(validate_inventory(std::slice::from_ref(&selection), &inventory).is_err());
        for change in 0..7 {
            let mut device = fixture();
            let pci = device.pci.as_mut().unwrap();
            match change {
                0 => pci.group_members.push("0000:00:0a.0".into()),
                1 => pci.group_members.clear(),
                2 => pci.physical_function = Some("0000:00:0a.0".into()),
                3 => pci.enabled_vfs = Some(1),
                4 => pci.iommu_group = None,
                5 => pci.total_vfs = None,
                _ => pci.total_vfs = Some(4),
            }
            assert!(eligible(&device).is_err());
        }
        inventory.devices.clear();
        assert!(validate_inventory(&[selection], &inventory).is_err());
    }
    #[test]
    fn guest_ports_are_distinct_and_no_device_is_implicitly_assigned() {
        assert!(arguments(&[]).is_empty());
        let a = Selection::from_device(&fixture()).unwrap();
        let mut b = a.clone();
        b.address = "0001:04:00.1".into();
        let args = arguments(&[a, b]);
        assert_eq!(
            args.iter().filter(|a| a.starts_with("iommufd,id=")).count(),
            1
        );
        assert!(args.iter().any(|a| a.contains("bus=npu_port0")));
        assert!(args.iter().any(|a| a.contains("bus=npu_port1")));
    }
    #[test]
    fn descriptor_substitution_requires_every_selected_device_exactly_once() {
        let selection = Selection::from_device(&fixture()).unwrap();
        let selected = std::slice::from_ref(&selection);
        let mut args = arguments(selected);
        fd_arguments(&mut args, selected, &[20, 21]).unwrap();
        assert!(args.iter().any(|a| a == "iommufd,id=vkore_iommu,fd=20"));
        assert!(args.iter().any(|a| a.starts_with("vfio-pci,fd=21,")));
        assert!(!args.iter().any(|a| a.contains("host=")));
        assert!(fd_arguments(&mut [], selected, &[20, 21]).is_err());
        let mut duplicated = arguments(selected);
        duplicated.extend(arguments(selected));
        assert!(fd_arguments(&mut duplicated, selected, &[20, 21]).is_err());
        assert!(fd_arguments(&mut arguments(selected), selected, &[0, 1]).is_err());
    }
    #[test]
    fn addresses_are_canonical_and_cannot_inject_qemu_options_or_paths() {
        for s in ["0000:00:0b.0", "0001:ff:1f.7"] {
            assert!(valid_address(s));
        }
        for s in [
            "../devices/x",
            "0000:00:0b.0,romfile=x",
            "0000:00:20.0",
            "0000:00:00.8",
            "0000:00:0B.0",
            "界界界界",
        ] {
            assert!(!valid_address(s));
        }
        let s = Selection {
            address: "0000:00:0b.0".into(),
            vendor_id: 0x8086,
            device_id: 0x643e,
        };
        assert!(validate_selections(&[s.clone(), s]).is_err());
    }
}
