//! GPU intent names a physical function and VF ordinal, not an assumed PCI BDF.
//! Linux's virtfn links resolve the address after provisioning. Never assign a PF.
use super::Selection;
use serde::{Deserialize, Serialize};
use vkore_devices::{Gpu, GpuInventory};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub physical_function: String,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Zero-based Linux virtfn ordinal; the UI displays one-based VF numbers.
    pub index: u32,
}
impl Assignment {
    pub fn new(gpu: &Gpu, index: u32) -> Self {
        Self {
            physical_function: gpu.pci.address.clone(),
            vendor_id: gpu.pci.vendor_id,
            device_id: gpu.pci.device_id,
            index,
        }
    }
    pub fn parent<'a>(&self, inventory: &'a GpuInventory) -> Result<&'a Gpu, String> {
        let gpu = inventory
            .devices
            .iter()
            .find(|g| {
                g.pci.address == self.physical_function
                    && g.pci.vendor_id == self.vendor_id
                    && g.pci.device_id == self.device_id
            })
            .ok_or_else(|| {
                format!(
                    "GPU {} is missing or its identity changed.",
                    self.physical_function
                )
            })?;
        eligible_parent(gpu)?;
        if self.index >= gpu.pci.total_vfs.unwrap_or(0).min(64) {
            return Err("Selected VF exceeds the GPU's reported capacity.".into());
        }
        let enabled = gpu.pci.enabled_vfs.ok_or("GPU VF count is unavailable.")?;
        if enabled != 0 && self.index >= enabled {
            return Err("This VF pool is already in use. Select an existing VF; vKore will not resize or remove the pool.".into());
        }
        Ok(gpu)
    }
    pub fn resolve(&self, inventory: &GpuInventory) -> Result<Selection, String> {
        let gpu = self.parent(inventory)?;
        if gpu.pci.enabled_vfs == Some(0) {
            return Err("GPU virtual functions need provisioning. Finish hardware setup with Done/Save hardware (also after a host reboot).".into());
        }
        let link = gpu.virtual_functions.iter().find(|v| v.index == self.index)
            .ok_or("GPU virtual functions need provisioning. Finish hardware setup with Done/Save hardware (also after a host reboot).")?;
        let vf = inventory
            .devices
            .iter()
            .find(|g| g.pci.address == link.address)
            .ok_or("The GPU VF has not appeared in host discovery; refresh hardware.")?;
        let selection = Selection {
            address: vf.pci.address.clone(),
            vendor_id: vf.pci.vendor_id,
            device_id: vf.pci.device_id,
            physical_function: Some(self.physical_function.clone()),
        };
        validate_selection(&selection, inventory)?;
        Ok(selection)
    }
}

pub fn eligible_parent(gpu: &Gpu) -> Result<(), String> {
    if gpu.pci.physical_function.is_some()
        || gpu.pci.vendor_id != 0x8086
        || gpu.driver.as_deref() != Some("xe")
    {
        return Err(
            "Managed GPU SR-IOV requires an Intel GPU physical function using the Linux xe driver."
                .into(),
        );
    }
    if !gpu.pci.total_vfs.is_some_and(|n| n > 0) || gpu.pci.enabled_vfs.is_none() {
        return Err("The host driver exposes no usable SR-IOV capacity. Enable SR-IOV support in the host's xe configuration, then refresh.".into());
    }
    Ok(())
}

pub fn validate(selected: &[Assignment], other_devices: usize) -> Result<(), String> {
    if selected.len().saturating_add(other_devices) > super::MAX_DEVICES {
        return Err("At most four accelerators can be assigned to one guest.".into());
    }
    let mut parents = std::collections::BTreeSet::new();
    for s in selected {
        if !super::valid_address(&s.physical_function)
            || s.index >= 64
            || !parents.insert(&s.physical_function)
        {
            return Err("Select at most one VF per GPU, with a valid physical-function address and VF index.".into());
        }
    }
    Ok(())
}

pub fn validate_selection(selection: &Selection, inventory: &GpuInventory) -> Result<(), String> {
    let parent = selection
        .physical_function
        .as_ref()
        .ok_or("GPU assignment requires a VF.")?;
    let pf = inventory
        .devices
        .iter()
        .find(|g| &g.pci.address == parent)
        .ok_or("GPU physical function disappeared.")?;
    eligible_parent(pf)?;
    if !pf
        .virtual_functions
        .iter()
        .any(|v| v.address == selection.address)
    {
        return Err("GPU VF no longer belongs to the selected physical function.".into());
    }
    let vf = inventory
        .devices
        .iter()
        .find(|g| g.pci.address == selection.address)
        .ok_or("GPU VF disappeared.")?;
    if vf.pci.vendor_id != selection.vendor_id
        || vf.pci.device_id != selection.device_id
        || vf.pci.physical_function.as_ref() != Some(parent)
        || vf.pci.vendor_id != pf.pci.vendor_id
    {
        return Err("GPU VF identity or physical-function link changed.".into());
    }
    if vf.pci.iommu_group.is_none() || vf.pci.group_members != [selection.address.clone()] {
        return Err("GPU VF passthrough needs an IOMMU group containing only that VF.".into());
    }
    if !matches!(vf.driver.as_deref(), None | Some("xe" | "vfio-pci")) {
        return Err("Unsupported GPU VF host driver.".into());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
pub(super) trait Pool {
    fn count(&self) -> Result<u32, String>;
    fn autoprobe(&self) -> Result<bool, String>;
    fn set_autoprobe(&mut self, enabled: bool) -> Result<(), String>;
    fn create(&mut self, count: u32) -> Result<(), String>;
}
#[cfg(any(target_os = "linux", test))]
pub(super) fn provision(pool: &mut impl Pool, needed: u32) -> Result<(), String> {
    if !(1..=64).contains(&needed) {
        return Err("Invalid VF pool size.".into());
    }
    let count = pool.count()?;
    if count >= needed {
        return Ok(());
    }
    if count != 0 {
        return Err("Existing GPU VF pools cannot be resized by vKore.".into());
    }
    let autoprobe = pool.autoprobe()?;
    pool.set_autoprobe(false)?;
    let result: Result<(), String> = (|| {
        // Recheck after changing autoprobe; another host manager may have acted.
        if pool.count()? != 0 {
            return Err("GPU VF pool changed during preparation.".into());
        }
        pool.create(needed)?;
        if pool.count()? != needed {
            return Err("The GPU driver did not create the requested VF pool.".into());
        }
        Ok(())
    })();
    // Restore host policy even on failed/partial creation. Never tear down VFs:
    // another process may already have opened one. Surface partial preparation.
    let restored = pool.set_autoprobe(autoprobe);
    match (result, restored) {
        (Ok(()), Ok(())) => Ok(()),
        (result, restored) => Err(format!(
            "GPU preparation failed: {}; autoprobe restoration: {}. Refresh host discovery; any created VFs are retained.",
            result.err().unwrap_or_else(|| "VFs created".into()),
            restored.err().unwrap_or_else(|| "restored".into())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inventory() -> GpuInventory {
        let pf = Gpu {
            name: "Intel GPU".into(),
            driver: Some("xe".into()),
            pci: vkore_devices::Pci {
                address: "0000:00:02.0".into(),
                vendor_id: 0x8086,
                device_id: 0xb080,
                revision: None,
                iommu_group: Some(1),
                group_members: vec!["0000:00:02.0".into()],
                physical_function: None,
                total_vfs: Some(7),
                enabled_vfs: Some(1),
            },
            virtual_functions: vec![vkore_devices::VirtualFunction {
                index: 0,
                address: "0000:03:00.1".into(),
            }],
        };
        let mut vf = pf.clone();
        vf.pci.address = "0000:03:00.1".into();
        vf.pci.physical_function = Some(pf.pci.address.clone());
        vf.pci.iommu_group = Some(2);
        vf.pci.group_members = vec![vf.pci.address.clone()];
        vf.pci.total_vfs = Some(0);
        vf.pci.enabled_vfs = None;
        vf.virtual_functions.clear();
        vf.driver = None;
        GpuInventory {
            devices: vec![pf, vf],
            warnings: vec![],
        }
    }
    #[test]
    fn vf_resolution_checks_parent_identity_links_isolation_and_driver() {
        let original = inventory();
        let intent = Assignment::new(&original.devices[0], 0);
        let resolved = intent.resolve(&original).unwrap();
        assert_eq!(resolved.address, "0000:03:00.1");
        assert_eq!(resolved.physical_function.as_deref(), Some("0000:00:02.0"));
        for failure in 0..8 {
            let mut changed = original.clone();
            match failure {
                0 => changed.devices[0].pci.device_id += 1,
                1 => changed.devices[0].virtual_functions.clear(),
                2 => changed.devices[1].pci.physical_function = None,
                3 => changed.devices[1]
                    .pci
                    .group_members
                    .push("0000:00:02.0".into()),
                4 => changed.devices[1].pci.iommu_group = None,
                5 => changed.devices[0].driver = Some("i915".into()),
                6 => changed.devices[1].driver = Some("another-driver".into()),
                _ => changed.devices[0].pci.enabled_vfs = Some(0),
            }
            assert!(intent.resolve(&changed).is_err(), "failure {failure}");
        }
        assert!(
            Assignment::new(&original.devices[0], 1)
                .parent(&original)
                .is_err()
        );
        assert!(validate(&[intent.clone(), intent], 0).is_err());
        assert!(
            eligible_parent(&original.devices[1]).is_err(),
            "never assign a PF as a VF"
        );
    }
    struct FakePool {
        count: u32,
        autoprobe: bool,
        fail_create: bool,
        fail_restore: bool,
        writes: Vec<String>,
    }
    impl Pool for FakePool {
        fn count(&self) -> Result<u32, String> {
            Ok(self.count)
        }
        fn autoprobe(&self) -> Result<bool, String> {
            Ok(self.autoprobe)
        }
        fn set_autoprobe(&mut self, enabled: bool) -> Result<(), String> {
            self.writes.push(format!("autoprobe={enabled}"));
            if enabled && self.fail_restore {
                return Err("restore denied".into());
            }
            self.autoprobe = enabled;
            Ok(())
        }
        fn create(&mut self, count: u32) -> Result<(), String> {
            assert!(!self.autoprobe);
            self.writes.push(format!("create={count}"));
            if self.fail_create {
                return Err("driver rejected".into());
            }
            self.count = count;
            Ok(())
        }
    }
    fn pool(count: u32) -> FakePool {
        FakePool {
            count,
            autoprobe: true,
            fail_create: false,
            fail_restore: false,
            writes: vec![],
        }
    }
    #[test]
    fn provisioning_preserves_existing_pools_and_restores_autoprobe_on_failure() {
        let mut existing = pool(2);
        provision(&mut existing, 1).unwrap();
        assert!(provision(&mut existing, 3).is_err());
        assert!(existing.writes.is_empty());
        let mut created = pool(0);
        provision(&mut created, 2).unwrap();
        assert_eq!(
            created.writes,
            ["autoprobe=false", "create=2", "autoprobe=true"]
        );
        assert_eq!(created.count, 2);
        for fail_restore in [false, true] {
            let mut failed = pool(0);
            failed.fail_create = true;
            failed.fail_restore = fail_restore;
            assert!(provision(&mut failed, 1).is_err());
            assert_eq!(failed.writes.last().unwrap(), "autoprobe=true");
            if !fail_restore {
                assert!(failed.autoprobe);
            }
        }
        assert!(provision(&mut pool(0), 0).is_err());
        assert!(provision(&mut pool(0), 65).is_err());
    }
}
