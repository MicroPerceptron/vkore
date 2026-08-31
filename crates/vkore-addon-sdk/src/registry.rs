use crate::{
    AddonManifest, AgentRef, AuthoritySet, ExecutionDomain, ManifestError, ToolCapability,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct AddonPackage {
    pub manifest: AddonManifest,
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkerTool {
    pub qualified_name: String,
    pub description: String,
    pub domain: ExecutionDomain,
    pub input_schema: Value,
}

#[derive(Debug)]
pub enum CatalogError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Manifest(ManifestError),
    DuplicateAddon(String),
    UnknownAddon(String),
    PackageHashMismatch,
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for CatalogError {}

impl From<std::io::Error> for CatalogError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for CatalogError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<ManifestError> for CatalogError {
    fn from(value: ManifestError) -> Self {
        Self::Manifest(value)
    }
}

#[derive(Default)]
pub struct AddonRegistry {
    packages: BTreeMap<String, AddonPackage>,
}

impl AddonRegistry {
    pub fn register(&mut self, package: AddonPackage) -> Result<(), CatalogError> {
        package.manifest.validate()?;
        if self.packages.contains_key(&package.manifest.id) {
            return Err(CatalogError::DuplicateAddon(package.manifest.id));
        }
        self.packages.insert(package.manifest.id.clone(), package);
        Ok(())
    }

    pub fn load_manifest(path: impl AsRef<Path>) -> Result<AddonPackage, CatalogError> {
        let path = path.as_ref();
        let manifest: AddonManifest = serde_json::from_slice(&std::fs::read(path)?)?;
        manifest.validate()?;
        let root = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Ok(AddonPackage { manifest, root })
    }

    pub fn package(&self, id: &str) -> Result<&AddonPackage, CatalogError> {
        self.packages
            .get(id)
            .ok_or_else(|| CatalogError::UnknownAddon(id.into()))
    }

    pub fn installed(&self) -> impl Iterator<Item = &AddonPackage> {
        self.packages.values()
    }

    pub fn candidate_catalog(&self, allowed_addons: &BTreeSet<String>) -> Vec<WorkerTool> {
        self.packages
            .values()
            .filter(|package| allowed_addons.contains(&package.manifest.id))
            .flat_map(|package| {
                package.manifest.tools.iter().map(move |tool| WorkerTool {
                    qualified_name: format!("{}.{}", package.manifest.id, tool.name),
                    description: tool.description.clone(),
                    domain: tool.domain,
                    input_schema: tool.input_schema.clone(),
                })
            })
            .collect()
    }

    pub fn worker_catalog(
        &self,
        authority: &AuthoritySet,
        agent: &AgentRef,
        now_ms: u64,
    ) -> Vec<WorkerTool> {
        authority
            .active_capabilities(agent, now_ms)
            .into_iter()
            .filter_map(|capability| self.tool_for_capability(capability))
            .collect()
    }

    fn tool_for_capability(&self, capability: &ToolCapability) -> Option<WorkerTool> {
        let package = self.packages.get(&capability.addon_id)?;
        if package.manifest.package_hash != capability.package_hash
            || package.manifest.version != capability.addon_version
        {
            return None;
        }
        let tool = package
            .manifest
            .tools
            .iter()
            .find(|tool| tool.name == capability.tool)?;
        Some(WorkerTool {
            qualified_name: format!("{}.{}", package.manifest.id, tool.name),
            description: tool.description.clone(),
            domain: tool.domain,
            input_schema: tool.input_schema.clone(),
        })
    }
}
