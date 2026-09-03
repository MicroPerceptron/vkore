use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PACKAGE_MANIFEST_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactClass {
    VkoreApplication,
    KoreGuest,
    VmRuntime,
    InferenceRuntime,
    Model,
    Addon,
    GuestTool,
}

impl ArtifactClass {
    pub fn key(self) -> &'static str {
        match self {
            Self::VkoreApplication => "vkore",
            Self::KoreGuest => "kore-guest",
            Self::VmRuntime => "vm-runtime",
            Self::InferenceRuntime => "inference-runtime",
            Self::Model => "model",
            Self::Addon => "addon",
            Self::GuestTool => "guest-tool",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Stable,
    Beta,
    Development,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlatformConstraint {
    pub operating_system: String,
    pub architecture: String,
    #[serde(default)]
    pub required_cpu_features: BTreeSet<String>,
    pub required_acceleration: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VersionRange {
    pub minimum: u16,
    pub maximum: u16,
}

impl VersionRange {
    pub fn contains(&self, version: u16) -> bool {
        self.minimum <= version && version <= self.maximum
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProtocolCompatibility {
    pub hostlink: Option<VersionRange>,
    pub workspace: Option<VersionRange>,
    pub addon_sdk: Option<VersionRange>,
    pub inference_api: Option<VersionRange>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionManifest {
    #[serde(default)]
    pub network_destinations: BTreeSet<String>,
    #[serde(default)]
    pub credential_kinds: BTreeSet<String>,
    #[serde(default)]
    pub filesystem_scopes: BTreeSet<String>,
    #[serde(default)]
    pub host_applications: BTreeSet<String>,
    #[serde(default)]
    pub tool_effects: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelMetadata {
    pub family: String,
    pub quantization: String,
    pub context_length: u32,
    pub chat_template: Option<String>,
    pub estimated_memory_bytes: u64,
    pub accelerator_memory_bytes: Option<u64>,
    #[serde(default)]
    pub redistribution_restricted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthCheckSpec {
    pub kind: String,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PackageManifest {
    pub schema_version: u16,
    pub id: String,
    pub version: String,
    pub class: ArtifactClass,
    pub channel: ReleaseChannel,
    pub source_url: String,
    pub project_url: String,
    pub publisher: String,
    pub license: String,
    pub digest: String,
    pub artifact_bytes: u64,
    pub installed_bytes: u64,
    #[serde(default)]
    pub platforms: Vec<PlatformConstraint>,
    pub compatibility: ProtocolCompatibility,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub conflicts: BTreeSet<String>,
    #[serde(default)]
    pub entrypoints: BTreeMap<String, String>,
    pub permissions: PermissionManifest,
    pub model: Option<ModelMetadata>,
    pub kore_boot_manifest_digest: Option<String>,
    pub health_check: Option<HealthCheckSpec>,
}

impl PackageManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version != PACKAGE_MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedSchema(self.schema_version));
        }
        if !valid_id(&self.id) || semver::Version::parse(&self.version).is_err() {
            return Err(ManifestError::InvalidIdentity);
        }
        if self.source_url.trim().is_empty()
            || self.project_url.trim().is_empty()
            || self.publisher.trim().is_empty()
            || self.license.trim().is_empty()
        {
            return Err(ManifestError::MissingProvenance);
        }
        if self.digest.len() != 64 || !self.digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ManifestError::InvalidDigest);
        }
        if self.artifact_bytes == 0 || self.installed_bytes < self.artifact_bytes {
            return Err(ManifestError::InvalidSize);
        }
        if self.class == ArtifactClass::Model && self.model.is_none() {
            return Err(ManifestError::MissingModelMetadata);
        }
        if self.class == ArtifactClass::KoreGuest && self.kore_boot_manifest_digest.is_none() {
            return Err(ManifestError::MissingBootManifest);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignedPackageManifest {
    pub manifest: PackageManifest,
    pub signature_hex: Option<String>,
}

impl SignedPackageManifest {
    pub fn verify_signature(&self, trust: &TrustPolicy, local: bool) -> Result<(), ManifestError> {
        self.manifest.validate()?;
        let Some(signature_hex) = &self.signature_hex else {
            return if local && trust.allow_unsigned_local {
                Ok(())
            } else {
                Err(ManifestError::SignatureRequired)
            };
        };
        let key_hex = trust
            .trusted_publishers
            .get(&self.manifest.publisher)
            .ok_or(ManifestError::UntrustedPublisher)?;
        let key_bytes: [u8; 32] = hex::decode(key_hex)
            .map_err(|_| ManifestError::InvalidSignature)?
            .try_into()
            .map_err(|_| ManifestError::InvalidSignature)?;
        let key =
            VerifyingKey::from_bytes(&key_bytes).map_err(|_| ManifestError::InvalidSignature)?;
        let signature_bytes: [u8; 64] = hex::decode(signature_hex)
            .map_err(|_| ManifestError::InvalidSignature)?
            .try_into()
            .map_err(|_| ManifestError::InvalidSignature)?;
        let signature = Signature::from_bytes(&signature_bytes);
        let payload =
            serde_json::to_vec(&self.manifest).map_err(|_| ManifestError::InvalidSignature)?;
        key.verify(&payload, &signature)
            .map_err(|_| ManifestError::InvalidSignature)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrustPolicy {
    pub trusted_publishers: BTreeMap<String, String>,
    pub allow_unsigned_local: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestError {
    UnsupportedSchema(u16),
    InvalidIdentity,
    MissingProvenance,
    InvalidDigest,
    InvalidSize,
    MissingModelMetadata,
    MissingBootManifest,
    SignatureRequired,
    UntrustedPublisher,
    InvalidSignature,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PermissionDiff {
    pub added_network_destinations: BTreeSet<String>,
    pub added_credential_kinds: BTreeSet<String>,
    pub added_filesystem_scopes: BTreeSet<String>,
    pub added_host_applications: BTreeSet<String>,
    pub added_tool_effects: BTreeSet<String>,
}

impl PermissionDiff {
    pub fn between(old: &PermissionManifest, new: &PermissionManifest) -> Self {
        Self {
            added_network_destinations: difference(
                &new.network_destinations,
                &old.network_destinations,
            ),
            added_credential_kinds: difference(&new.credential_kinds, &old.credential_kinds),
            added_filesystem_scopes: difference(&new.filesystem_scopes, &old.filesystem_scopes),
            added_host_applications: difference(&new.host_applications, &old.host_applications),
            added_tool_effects: difference(&new.tool_effects, &old.tool_effects),
        }
    }

    pub fn expands_permissions(&self) -> bool {
        !self.added_network_destinations.is_empty()
            || !self.added_credential_kinds.is_empty()
            || !self.added_filesystem_scopes.is_empty()
            || !self.added_host_applications.is_empty()
            || !self.added_tool_effects.is_empty()
    }
}

fn difference(left: &BTreeSet<String>, right: &BTreeSet<String>) -> BTreeSet<String> {
    left.difference(right).cloned().collect()
}
