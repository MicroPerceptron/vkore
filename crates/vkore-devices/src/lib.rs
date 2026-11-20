//! Read-only NPU and Linux GPU inventory. A detected device is not permission to assign it.
//! Platform handles stay inside a probe; snapshots own only bounded values.
//! There are no runtime downloads, model loads, driver changes or guest ABI here.
use serde::Serialize;

pub const MAX_DEVICES: usize = 64;
const MAX_WARNINGS: usize = 16;
const MAX_TEXT: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Linux,
    Windows,
    MacOs,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStatus {
    Complete,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
pub struct Inventory {
    pub platform: Platform,
    pub status: DiscoveryStatus,
    pub devices: Vec<Npu>,
    pub warnings: Vec<String>,
}
impl Inventory {
    pub fn empty(platform: Platform) -> Self {
        Self {
            platform,
            status: DiscoveryStatus::Complete,
            devices: Vec::new(),
            warnings: Vec::new(),
        }
    }
    pub fn unavailable(platform: Platform, reason: &str) -> Self {
        let mut result = Self::empty(platform);
        result.warn(reason);
        result.status = DiscoveryStatus::Unavailable;
        result
    }
    fn warn(&mut self, reason: &str) {
        self.status = DiscoveryStatus::Partial;
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(bounded(reason));
        }
    }
    fn push(&mut self, npu: Npu) {
        if self.devices.iter().any(|d| d.id == npu.id) {
            return;
        }
        if self.devices.len() == MAX_DEVICES {
            self.warn("Device discovery reached its 64-device limit; some NPUs may be omitted.");
        } else {
            self.devices.push(npu);
        }
    }
    fn finish(mut self) -> Self {
        self.devices.sort_by(|a, b| a.id.cmp(&b.id));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Intel,
    Amd,
    Apple,
    Qualcomm,
    Other,
}
impl Vendor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Intel => "Intel",
            Self::Amd => "AMD",
            Self::Apple => "Apple",
            Self::Qualcomm => "Qualcomm",
            Self::Other => "Other vendor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    LinuxPci,
    WindowsDxCore,
    CoreMl,
}
impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Self::LinuxPci => "Linux PCI",
            Self::WindowsDxCore => "Windows DXCore",
            Self::CoreMl => "Core ML",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Npu {
    /// Snapshot identity, never an assignment handle. Windows LUIDs and Core ML
    /// ordinals are not persistent hardware identifiers and must be rediscovered.
    pub id: String,
    pub name: String,
    pub vendor: Vendor,
    pub source: Source,
    pub driver: Option<String>,
    pub core_count: Option<u32>,
    pub pci: Option<Pci>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Pci {
    pub address: String,
    pub vendor_id: u16,
    pub device_id: u16,
    pub revision: Option<u8>,
    pub iommu_group: Option<u32>,
    pub group_members: Vec<String>,
    pub physical_function: Option<String>,
    /// None means discovery could not determine the capability, not zero VFs.
    pub total_vfs: Option<u32>,
    pub enabled_vfs: Option<u32>,
}

fn bounded(text: &str) -> String {
    text.chars().take(MAX_TEXT).collect()
}

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;

/// Runs an explicit, unprivileged snapshot. Failure remains distinct from no NPUs.
pub fn discover() -> Inventory {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        platform::discover().finish()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Inventory::unavailable(
            Platform::Unsupported,
            "NPU discovery is unavailable on this operating system.",
        )
    }
}

#[cfg(all(test, not(target_os = "linux"), unix))]
#[path = "linux.rs"]
mod linux_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_discovery_is_not_an_empty_success() {
        let failed = Inventory::unavailable(Platform::Linux, "permission denied");
        assert_eq!(failed.status, DiscoveryStatus::Unavailable);
        assert!(!failed.warnings.is_empty());
        assert_eq!(
            Inventory::empty(Platform::Linux).status,
            DiscoveryStatus::Complete
        );
    }
    #[test]
    fn snapshots_bound_and_deduplicate_external_values() {
        let mut result = Inventory::empty(Platform::Linux);
        for i in 0..100 {
            let npu = Npu {
                id: format!("npu:{i}"),
                name: "NPU".into(),
                vendor: Vendor::Other,
                source: Source::LinuxPci,
                driver: None,
                core_count: None,
                pci: None,
            };
            result.push(npu.clone());
            result.push(npu);
            result.warn(&"界".repeat(1024));
        }
        assert_eq!(result.devices.len(), MAX_DEVICES);
        assert_eq!(result.warnings.len(), MAX_WARNINGS);
        assert!(
            result
                .warnings
                .iter()
                .all(|w| w.chars().count() <= MAX_TEXT)
        );
        assert_eq!(result.status, DiscoveryStatus::Partial);
    }
}

/// Linux display-class PCI devices, including independently enumerated VFs.
#[derive(Clone, Debug, Default, Serialize)]
pub struct GpuInventory {
    pub devices: Vec<Gpu>,
    pub warnings: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Gpu {
    pub name: String,
    pub pci: Pci,
    pub driver: Option<String>,
    pub virtual_functions: Vec<VirtualFunction>,
}
#[derive(Clone, Debug, Serialize)]
pub struct VirtualFunction {
    pub index: u32,
    pub address: String,
}
pub fn discover_gpus() -> GpuInventory {
    #[cfg(target_os = "linux")]
    {
        platform::discover_gpus()
    }
    #[cfg(not(target_os = "linux"))]
    {
        GpuInventory::default()
    }
}
