use super::*;
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

#[cfg(target_os = "linux")]
pub(super) fn discover() -> Inventory {
    let mut inventory = Inventory::empty(Platform::Linux);
    let mut enumerator = match udev::Enumerator::new() {
        Ok(e) => e,
        Err(e) => {
            return Inventory::unavailable(
                Platform::Linux,
                &format!("Cannot enumerate PCI devices: {e}"),
            );
        }
    };
    if let Err(e) = enumerator.match_subsystem("pci") {
        return Inventory::unavailable(Platform::Linux, &format!("Cannot query PCI devices: {e}"));
    }
    let devices = match enumerator.scan_devices() {
        Ok(d) => d,
        Err(e) => {
            return Inventory::unavailable(
                Platform::Linux,
                &format!("Cannot scan PCI devices: {e}"),
            );
        }
    };
    for (index, device) in devices.enumerate() {
        if index == 4096 {
            inventory.warn("PCI scan reached its 4096-device limit.");
            break;
        }
        match probe(device.syspath(), &mut inventory) {
            Ok(Some(npu)) => inventory.push(npu),
            Ok(None) => {}
            Err(e) => inventory.warn(&format!("{}: {e}", device.sysname().to_string_lossy())),
        }
    }
    inventory
}

/// Numeric IDs identify hardware, never its suitability for guest assignment.
/// References: Linux drivers/accel/{ivpu/ivpu_drv.h,amdxdna/amdxdna_pci_drv.c}.
fn identify(vendor: u16, device: u16, driver: Option<&str>) -> Option<(Vendor, &'static str)> {
    match (vendor, device) {
        (0x8086, 0x7d1d) => Some((Vendor::Intel, "Intel NPU · Meteor Lake")),
        (0x8086, 0xad1d) => Some((Vendor::Intel, "Intel NPU · Arrow Lake")),
        (0x8086, 0x643e) => Some((Vendor::Intel, "Intel NPU · Lunar Lake")),
        (0x8086, 0xb03e) => Some((Vendor::Intel, "Intel NPU · Panther Lake")),
        (0x8086, 0xfd3e) => Some((Vendor::Intel, "Intel NPU · Wildcat Lake")),
        (0x8086, 0xd71d) => Some((Vendor::Intel, "Intel NPU · Nova Lake")),
        (0x1022, 0x1502 | 0x17f0 | 0x17f2 | 0x17f3 | 0x1b0b | 0x1b0c) => {
            Some((Vendor::Amd, "AMD XDNA NPU"))
        }
        (0x8086, _) if matches!(driver, Some("ivpu" | "intel_vpu")) => {
            Some((Vendor::Intel, "Intel NPU"))
        }
        (0x1022, _) if driver == Some("amdxdna") => Some((Vendor::Amd, "AMD XDNA NPU")),
        _ => None,
    }
}

fn text(path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other("device attribute exceeds 4096 bytes"));
    }
    String::from_utf8(bytes)
        .map(|s| s.trim().to_owned())
        .map_err(io::Error::other)
}
fn hex(path: &Path) -> io::Result<u16> {
    u16::from_str_radix(text(path)?.trim_start_matches("0x"), 16).map_err(io::Error::other)
}
fn optional_text(path: &Path) -> io::Result<Option<String>> {
    match text(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn link_name(path: &Path) -> io::Result<Option<String>> {
    match fs::read_link(path) {
        Ok(p) => p
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| Some(bounded(s)))
            .ok_or_else(|| io::Error::other("invalid device link")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
fn address(s: &str) -> bool {
    s.len() == 12
        && s.as_bytes()[4] == b':'
        && s.as_bytes()[7] == b':'
        && s.as_bytes()[10] == b'.'
        && s.bytes()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7 | 10) || b.is_ascii_hexdigit())
        && u8::from_str_radix(&s[8..10], 16).is_ok_and(|n| n < 32)
        && u8::from_str_radix(&s[11..12], 16).is_ok_and(|n| n < 8)
}
fn number(path: &Path) -> io::Result<Option<u32>> {
    optional_text(path)?
        .map(|s| s.parse().map_err(io::Error::other))
        .transpose()
}
fn observed<T>(result: io::Result<T>, inventory: &mut Inventory, context: &str) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            inventory.warn(&format!("{context}: {error}"));
            None
        }
    }
}
fn probe(path: &Path, inventory: &mut Inventory) -> io::Result<Option<Npu>> {
    let vendor = hex(&path.join("vendor"))?;
    let device = hex(&path.join("device"))?;
    let driver = observed(link_name(&path.join("driver")), inventory, "NPU driver").flatten();
    let Some((family, name)) = identify(vendor, device, driver.as_deref()) else {
        return Ok(None);
    };
    let pci = pci(path, inventory)?;
    Ok(Some(Npu {
        id: format!("pci:{}", pci.address),
        name: name.into(),
        vendor: family,
        source: Source::LinuxPci,
        driver,
        core_count: None,
        pci: Some(pci),
    }))
}

fn pci(path: &Path, inventory: &mut Inventory) -> io::Result<Pci> {
    let vendor = hex(&path.join("vendor"))?;
    let device = hex(&path.join("device"))?;
    let bdf = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| address(s))
        .ok_or_else(|| io::Error::other("invalid PCI address"))?
        .to_owned();
    let group = observed(
        link_name(&path.join("iommu_group")).and_then(|v| {
            v.map(|s| s.parse::<u32>().map_err(io::Error::other))
                .transpose()
        }),
        inventory,
        "PCI IOMMU group",
    )
    .flatten();
    let members = if group.is_some() {
        observed(group_members(path), inventory, "PCI IOMMU group members").unwrap_or_default()
    } else {
        Vec::new()
    };
    let pf = observed(
        link_name(&path.join("physfn")).and_then(|v| {
            if v.as_ref().is_some_and(|s| !address(s)) {
                Err(io::Error::other("invalid physical-function address"))
            } else {
                Ok(v)
            }
        }),
        inventory,
        "PCI physical function",
    )
    .flatten();
    let total_vfs = observed(
        number(&path.join("sriov_totalvfs")).map(|v| v.unwrap_or(0)),
        inventory,
        "PCI SR-IOV capacity",
    );
    let enabled_vfs = observed(
        number(&path.join("sriov_numvfs")),
        inventory,
        "PCI enabled VFs",
    )
    .flatten();
    let revision = observed(
        optional_text(&path.join("revision")).and_then(|v| {
            v.map(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).map_err(io::Error::other))
                .transpose()
        }),
        inventory,
        "PCI revision",
    )
    .flatten();
    Ok(Pci {
        address: bdf,
        vendor_id: vendor,
        device_id: device,
        revision,
        iommu_group: group,
        group_members: members,
        physical_function: pf,
        total_vfs,
        enabled_vfs,
    })
}

fn group_members(path: &Path) -> io::Result<Vec<String>> {
    let mut members = Vec::new();
    for (index, entry) in fs::read_dir(path.join("iommu_group/devices"))?.enumerate() {
        if index == MAX_DEVICES {
            return Err(io::Error::other("IOMMU group exceeds 64 devices"));
        }
        let member = entry?.file_name().to_string_lossy().into_owned();
        if !address(&member) {
            return Err(io::Error::other("invalid IOMMU member address"));
        }
        members.push(member);
    }
    members.sort();
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn classification_does_not_promote_generic_accelerators_or_gpu_ids() {
        assert!(identify(0x8086, 0x643e, None).is_some());
        assert!(identify(0x1022, 0x17f0, Some("vfio-pci")).is_some());
        assert!(identify(0x8086, 0xffff, Some("ivpu")).is_some());
        assert!(identify(0x8086, 0x1234, None).is_none());
        assert!(identify(0x1002, 0x17f0, None).is_none());
        assert!(identify(0x1234, 0x1234, Some("amdxdna")).is_none());
        assert!(!address("0000:00:20.0"));
        assert!(!address("0000:00:02.8"));
        assert!(address("0001:ab:1f.7"));
    }
    #[test]
    fn driverless_and_vfio_devices_keep_identity_and_probed_sriov_topology() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("0001:04:00.0");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("vendor"), "0x1022\n").unwrap();
        fs::write(path.join("device"), "0x17f0\n").unwrap();
        fs::write(path.join("revision"), "0x11\n").unwrap();
        let npu = probe(&path, &mut Inventory::empty(Platform::Linux))
            .unwrap()
            .unwrap();
        assert_eq!(npu.id, "pci:0001:04:00.0");
        assert_eq!(npu.driver, None);
        assert_eq!(npu.pci.unwrap().total_vfs, Some(0));
        symlink("/sys/bus/pci/drivers/vfio-pci", path.join("driver")).unwrap();
        let group = root.path().join("19");
        fs::create_dir_all(group.join("devices")).unwrap();
        fs::write(group.join("devices/0001:04:00.0"), "").unwrap();
        symlink(&group, path.join("iommu_group")).unwrap();
        fs::write(path.join("sriov_totalvfs"), "4\n").unwrap();
        fs::write(path.join("sriov_numvfs"), "2\n").unwrap();
        let npu = probe(&path, &mut Inventory::empty(Platform::Linux))
            .unwrap()
            .unwrap();
        assert_eq!(npu.driver.as_deref(), Some("vfio-pci"));
        let pci = npu.pci.unwrap();
        assert_eq!(pci.iommu_group, Some(19));
        assert_eq!(pci.group_members, ["0001:04:00.0"]);
        assert_eq!((pci.total_vfs, pci.enabled_vfs), (Some(4), Some(2)));
        fs::write(path.join("sriov_totalvfs"), "broken").unwrap();
        let mut partial = Inventory::empty(Platform::Linux);
        let npu = probe(&path, &mut partial).unwrap().unwrap();
        assert_eq!(npu.pci.unwrap().total_vfs, None);
        assert_eq!(partial.status, DiscoveryStatus::Partial);
        assert!(!partial.warnings.is_empty());
    }
}

/// Linux GPU discovery shares the same bounded PCI snapshot as NPU discovery.
#[cfg(target_os = "linux")]
pub(super) fn discover_gpus() -> GpuInventory {
    let mut diagnostics = Inventory::empty(Platform::Linux);
    let mut devices = Vec::new();
    let result = (|| -> io::Result<()> {
        let mut enumerator = udev::Enumerator::new()?;
        enumerator.match_subsystem("pci")?;
        for (index, device) in enumerator.scan_devices()?.enumerate() {
            if index == 4096 {
                return Err(io::Error::other("PCI scan exceeds 4096 devices"));
            }
            match gpu(device.syspath(), &mut diagnostics) {
                Ok(Some(gpu)) if devices.len() < MAX_DEVICES => devices.push(gpu),
                Ok(Some(_)) => diagnostics.warn("GPU inventory exceeds 64 devices"),
                Ok(None) => (),
                Err(e) => diagnostics.warn(&format!("GPU discovery: {e}")),
            }
        }
        Ok(())
    })();
    if let Err(e) = result {
        diagnostics.warn(&format!("GPU discovery: {e}"));
    }
    devices.sort_by(|a, b| a.pci.address.cmp(&b.pci.address));
    GpuInventory {
        devices,
        warnings: diagnostics.warnings,
    }
}
fn gpu(path: &Path, inventory: &mut Inventory) -> io::Result<Option<Gpu>> {
    let class = u32::from_str_radix(text(&path.join("class"))?.trim_start_matches("0x"), 16)
        .map_err(io::Error::other)?;
    if class >> 16 != 0x03 {
        return Ok(None);
    }
    let pci = pci(path, inventory)?;
    let driver = link_name(&path.join("driver"))?;
    let mut virtual_functions = Vec::new();
    for index in 0..pci.enabled_vfs.unwrap_or(0).min(MAX_DEVICES as u32) {
        let address = link_name(&path.join(format!("virtfn{index}")))?
            .filter(|s| address(s))
            .ok_or_else(|| io::Error::other("missing or invalid VF link"))?;
        virtual_functions.push(VirtualFunction { index, address });
    }
    Ok(Some(Gpu {
        name: format!(
            "{} GPU · {:04x}",
            match pci.vendor_id {
                0x8086 => "Intel",
                0x1002 => "AMD",
                0x10de => "NVIDIA",
                _ => "PCI",
            },
            pci.device_id
        ),
        pci,
        driver,
        virtual_functions,
    }))
}

#[cfg(test)]
#[test]
fn display_class_discovery_preserves_pf_vf_links_without_assuming_bdfs() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let pf = root.path().join("0000:00:02.0");
    let vf = root.path().join("0000:03:00.1");
    for path in [&pf, &vf] {
        fs::create_dir(path).unwrap();
        for (key, value) in [
            ("vendor", "0x8086"),
            ("device", "0xb080"),
            ("class", "0x038000"),
        ] {
            fs::write(path.join(key), value).unwrap();
        }
    }
    fs::write(pf.join("sriov_totalvfs"), "7").unwrap();
    fs::write(pf.join("sriov_numvfs"), "1").unwrap();
    symlink(&vf, pf.join("virtfn0")).unwrap();
    symlink(&pf, vf.join("physfn")).unwrap();
    let mut inventory = Inventory::empty(Platform::Linux);
    let parent = gpu(&pf, &mut inventory).unwrap().unwrap();
    assert_eq!(parent.virtual_functions[0].address, "0000:03:00.1");
    assert_eq!(
        gpu(&vf, &mut inventory)
            .unwrap()
            .unwrap()
            .pci
            .physical_function
            .as_deref(),
        Some("0000:00:02.0")
    );
    fs::write(vf.join("class"), "0x120000").unwrap();
    assert!(gpu(&vf, &mut inventory).unwrap().is_none());
    fs::remove_file(pf.join("virtfn0")).unwrap();
    assert!(gpu(&pf, &mut inventory).is_err());
}
