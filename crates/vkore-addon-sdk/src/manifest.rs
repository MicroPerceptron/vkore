use crate::{MediationMode, ResultSensitivity};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;

pub const ADDON_MANIFEST_VERSION: u16 = 1;
pub const INVOCATION_PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionDomain {
    Host,
    Guest,
    Split,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskClass {
    ReadOnly,
    ExternalWrite,
    Destructive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceEffect {
    pub resource_type: String,
    pub operation: String,
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub destructive: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AddonEntrypoint {
    Process {
        executable: String,
        arguments: Vec<String>,
    },
    GuestExecutable {
        artifact: String,
    },
    SplitProcess {
        executable: String,
        arguments: Vec<String>,
        guest_artifact: String,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolManifest {
    pub name: String,
    pub description: String,
    pub domain: ExecutionDomain,
    pub input_schema: Value,
    pub result_schema: Value,
    #[serde(default)]
    pub effects: Vec<ResourceEffect>,
    pub risk: RiskClass,
    pub minimum_mediation: MediationMode,
    #[serde(default)]
    pub required_connections: BTreeSet<String>,
    #[serde(default)]
    pub network_destinations: BTreeSet<String>,
    pub result_sensitivity: ResultSensitivity,
    pub timeout_ms: u64,
    pub maximum_input_bytes: usize,
    pub maximum_output_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AddonManifest {
    pub manifest_version: u16,
    pub id: String,
    pub name: String,
    pub version: String,
    pub package_hash: String,
    pub sdk_protocol: u16,
    pub entrypoint: AddonEntrypoint,
    #[serde(default)]
    pub tools: Vec<ToolManifest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestError {
    UnsupportedManifest(u16),
    UnsupportedProtocol(u16),
    InvalidId,
    InvalidVersion,
    InvalidPackageHash,
    DuplicateTool(String),
    InvalidTool(String),
    DomainMismatch(String),
    UnsafeLimits(String),
    UnsafeMediation(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ManifestError {}

impl AddonManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.manifest_version != ADDON_MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedManifest(self.manifest_version));
        }
        if self.sdk_protocol != INVOCATION_PROTOCOL_VERSION {
            return Err(ManifestError::UnsupportedProtocol(self.sdk_protocol));
        }
        if !valid_identifier(&self.id) {
            return Err(ManifestError::InvalidId);
        }
        if self.version.trim().is_empty() {
            return Err(ManifestError::InvalidVersion);
        }
        if self.package_hash.len() != 64
            || !self
                .package_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ManifestError::InvalidPackageHash);
        }
        let mut names = BTreeSet::new();
        for tool in &self.tools {
            if !valid_identifier(&tool.name) || tool.description.trim().is_empty() {
                return Err(ManifestError::InvalidTool(tool.name.clone()));
            }
            if !names.insert(tool.name.clone()) {
                return Err(ManifestError::DuplicateTool(tool.name.clone()));
            }
            if tool.timeout_ms == 0
                || tool.maximum_input_bytes == 0
                || tool.maximum_output_bytes == 0
                || tool.maximum_input_bytes > 16 * 1024 * 1024
                || tool.maximum_output_bytes > 16 * 1024 * 1024
            {
                return Err(ManifestError::UnsafeLimits(tool.name.clone()));
            }
            if tool.risk == RiskClass::Destructive
                && tool.minimum_mediation < MediationMode::UserConfirm
            {
                return Err(ManifestError::UnsafeMediation(tool.name.clone()));
            }
            let matches = matches!(
                (&self.entrypoint, tool.domain),
                (AddonEntrypoint::Process { .. }, ExecutionDomain::Host)
                    | (
                        AddonEntrypoint::GuestExecutable { .. },
                        ExecutionDomain::Guest
                    )
                    | (AddonEntrypoint::SplitProcess { .. }, ExecutionDomain::Split)
            );
            if !matches {
                return Err(ManifestError::DomainMismatch(tool.name.clone()));
            }
        }
        Ok(())
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}
