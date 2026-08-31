use crate::{ModelMetadata, PackageManifest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostPlatform {
    pub operating_system: String,
    pub architecture: String,
    pub cpu_features: BTreeSet<String>,
    pub acceleration: BTreeSet<String>,
    pub available_memory_bytes: Option<u64>,
    pub available_accelerator_memory_bytes: Option<u64>,
}

impl HostPlatform {
    pub fn detect() -> Self {
        let mut acceleration = BTreeSet::new();
        if cfg!(target_os = "windows") {
            acceleration.insert("whpx".into());
        }
        if cfg!(target_os = "macos") {
            acceleration.insert("hvf".into());
        }
        if cfg!(target_os = "linux") && std::path::Path::new("/dev/kvm").exists() {
            acceleration.insert("kvm".into());
        }
        Self {
            operating_system: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            cpu_features: BTreeSet::new(),
            acceleration,
            available_memory_bytes: None,
            available_accelerator_memory_bytes: None,
        }
    }

    pub fn supports(&self, manifest: &PackageManifest) -> bool {
        manifest.platforms.is_empty()
            || manifest.platforms.iter().any(|platform| {
                platform.operating_system == self.operating_system
                    && platform.architecture == self.architecture
                    && platform.required_cpu_features.is_subset(&self.cpu_features)
                    && platform
                        .required_acceleration
                        .as_ref()
                        .is_none_or(|required| self.acceleration.contains(required))
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeCompatibility {
    pub hostlink: u16,
    pub workspace: u16,
    pub addon_sdk: u16,
    pub inference_api: u16,
}

impl RuntimeCompatibility {
    pub fn supports(&self, manifest: &PackageManifest) -> bool {
        let compatibility = &manifest.compatibility;
        compatibility
            .hostlink
            .as_ref()
            .is_none_or(|range| range.contains(self.hostlink))
            && compatibility
                .workspace
                .as_ref()
                .is_none_or(|range| range.contains(self.workspace))
            && compatibility
                .addon_sdk
                .as_ref()
                .is_none_or(|range| range.contains(self.addon_sdk))
            && compatibility
                .inference_api
                .as_ref()
                .is_none_or(|range| range.contains(self.inference_api))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelFit {
    Fits,
    Tight,
    DoesNotFit,
    Unknown,
}

pub fn model_fit(model: &ModelMetadata, host: &HostPlatform) -> ModelFit {
    let Some(available) = host.available_memory_bytes else {
        return ModelFit::Unknown;
    };
    if model.estimated_memory_bytes > available {
        return ModelFit::DoesNotFit;
    }
    if model.estimated_memory_bytes.saturating_mul(5) > available.saturating_mul(4) {
        ModelFit::Tight
    } else {
        ModelFit::Fits
    }
}
