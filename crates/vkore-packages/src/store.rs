use crate::download::resumable_download;
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct StoreLimits {
    pub maximum_store_bytes: u64,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            maximum_store_bytes: 512 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PackageRef {
    pub id: String,
    pub version: String,
    pub digest: String,
}

impl From<&PackageManifest> for PackageRef {
    fn from(manifest: &PackageManifest) -> Self {
        Self {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            digest: manifest.digest.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct InstalledPackage {
    pub signed_manifest: SignedPackageManifest,
    pub artifact_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivationRecord {
    pub class: ArtifactClass,
    pub current: PackageRef,
    pub previous: Option<PackageRef>,
    pub activated_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskUsage {
    pub object_bytes: u64,
    pub download_bytes: u64,
    pub object_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackageError {
    Io(String),
    Json(String),
    Manifest(ManifestError),
    DigestMismatch,
    SizeMismatch { expected: u64, actual: u64 },
    PlatformIncompatible,
    ProtocolIncompatible,
    StoreLimitExceeded,
    Download(String),
    DownloadTooLarge,
    DownloadIncomplete { expected: u64, actual: u64 },
    UnknownPackage,
    ActiveWorkloads,
    PermissionExpansion(PermissionDiff),
    HealthCheckFailed(String),
    NoRollback,
    InvalidBundle,
    Conflict(String),
    VersionAlreadyRegistered,
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PackageError {}

impl From<std::io::Error> for PackageError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<serde_json::Error> for PackageError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value.to_string())
    }
}

impl From<ManifestError> for PackageError {
    fn from(value: ManifestError) -> Self {
        Self::Manifest(value)
    }
}

pub trait WorkloadGuard {
    fn active_workloads(&self, class: ArtifactClass) -> usize;
}

pub trait HealthCheck {
    fn check(&mut self, package: &InstalledPackage) -> Result<(), String>;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PackageAuditEvent {
    pub sequence: u64,
    pub action: String,
    pub package: Option<PackageRef>,
    pub detail: String,
}

pub struct PackageStore {
    root: PathBuf,
    trust: TrustPolicy,
    platform: HostPlatform,
    compatibility: RuntimeCompatibility,
    limits: StoreLimits,
}

impl PackageStore {
    pub fn open(
        root: impl Into<PathBuf>,
        trust: TrustPolicy,
        platform: HostPlatform,
        compatibility: RuntimeCompatibility,
        limits: StoreLimits,
    ) -> Result<Self, PackageError> {
        let root = root.into();
        for directory in ["objects", "manifests", "active", "downloads"] {
            std::fs::create_dir_all(root.join(directory))?;
        }
        Ok(Self {
            root,
            trust,
            platform,
            compatibility,
            limits,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn partial_path(&self, digest: &str) -> PathBuf {
        self.root.join("downloads").join(format!("{digest}.part"))
    }

    pub fn import_local(
        &self,
        artifact: impl AsRef<Path>,
        signed: SignedPackageManifest,
    ) -> Result<InstalledPackage, PackageError> {
        self.install_from_path(artifact.as_ref(), signed, true)
    }

    pub fn download_and_install(
        &self,
        url: &str,
        signed: SignedPackageManifest,
    ) -> Result<InstalledPackage, PackageError> {
        signed.verify_signature(&self.trust, false)?;
        self.preflight(&signed.manifest)?;
        let partial = self.partial_path(&signed.manifest.digest);
        resumable_download(url, &partial, signed.manifest.artifact_bytes)?;
        let installed = self.install_from_path(&partial, signed, false)?;
        let _ = std::fs::remove_file(partial);
        Ok(installed)
    }

    pub fn installed(&self, reference: &PackageRef) -> Result<InstalledPackage, PackageError> {
        let path = self.manifest_path(&reference.id, &reference.version);
        let signed: SignedPackageManifest = serde_json::from_slice(&std::fs::read(path)?)?;
        if signed.manifest.digest != reference.digest {
            return Err(PackageError::DigestMismatch);
        }
        let artifact_path = self.object_path(&reference.digest);
        if !artifact_path.is_file() {
            return Err(PackageError::UnknownPackage);
        }
        Ok(InstalledPackage {
            signed_manifest: signed,
            artifact_path,
        })
    }

    pub fn manifests(&self) -> Result<Vec<SignedPackageManifest>, PackageError> {
        let mut manifests = Vec::new();
        for entry in std::fs::read_dir(self.root.join("manifests"))? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                manifests.push(serde_json::from_slice(&std::fs::read(entry.path())?)?);
            }
        }
        Ok(manifests)
    }

    pub fn select_release(
        &self,
        package_id: &str,
        channel: ReleaseChannel,
        pinned: Option<&str>,
    ) -> Result<PackageRef, PackageError> {
        let mut candidates: Vec<_> = self
            .manifests()?
            .into_iter()
            .filter(|signed| signed.manifest.id == package_id && signed.manifest.channel == channel)
            .filter_map(|signed| {
                semver::Version::parse(&signed.manifest.version)
                    .ok()
                    .map(|version| (version, signed.manifest))
            })
            .collect();
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        let manifest = if let Some(pinned) = pinned {
            candidates
                .into_iter()
                .find(|(version, _)| version.to_string() == pinned)
                .map(|(_, manifest)| manifest)
        } else {
            candidates.pop().map(|(_, manifest)| manifest)
        }
        .ok_or(PackageError::UnknownPackage)?;
        Ok(PackageRef::from(&manifest))
    }

    pub fn active(&self, class: ArtifactClass) -> Result<Option<ActivationRecord>, PackageError> {
        let path = self.activation_path(class);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&std::fs::read(path)?)?))
    }

    pub fn activate(
        &self,
        reference: &PackageRef,
        permission_expansion_approved: bool,
        now_ms: u64,
        workloads: &impl WorkloadGuard,
        health: &mut impl HealthCheck,
    ) -> Result<ActivationRecord, PackageError> {
        let installed = self.installed(reference)?;
        let class = installed.signed_manifest.manifest.class;
        if workloads.active_workloads(class) > 0 {
            return Err(PackageError::ActiveWorkloads);
        }
        let old = self.active(class)?;
        self.check_conflicts(&installed.signed_manifest.manifest)?;
        if class == ArtifactClass::Addon
            && let Some(previous) = old.as_ref()
        {
            let previous = self.installed(&previous.current)?;
            let diff = PermissionDiff::between(
                &previous.signed_manifest.manifest.permissions,
                &installed.signed_manifest.manifest.permissions,
            );
            if diff.expands_permissions() && !permission_expansion_approved {
                return Err(PackageError::PermissionExpansion(diff));
            }
        }
        let record = ActivationRecord {
            class,
            current: reference.clone(),
            previous: old.as_ref().map(|record| record.current.clone()),
            activated_at_ms: now_ms,
        };
        self.write_activation(&record)?;
        self.audit(
            "activation_started",
            Some(reference.clone()),
            "awaiting health check",
        )?;
        if let Err(reason) = health.check(&installed) {
            if let Some(old) = old {
                self.write_activation(&old)?;
            } else {
                let _ = std::fs::remove_file(self.activation_path(class));
            }
            self.audit("rollback", Some(reference.clone()), &reason)?;
            return Err(PackageError::HealthCheckFailed(reason));
        }
        self.audit("activated", Some(reference.clone()), "health check passed")?;
        Ok(record)
    }

    pub fn rollback(
        &self,
        class: ArtifactClass,
        now_ms: u64,
        workloads: &impl WorkloadGuard,
        health: &mut impl HealthCheck,
    ) -> Result<ActivationRecord, PackageError> {
        if workloads.active_workloads(class) > 0 {
            return Err(PackageError::ActiveWorkloads);
        }
        let active = self.active(class)?.ok_or(PackageError::NoRollback)?;
        let previous = active.previous.clone().ok_or(PackageError::NoRollback)?;
        let installed = self.installed(&previous)?;
        let record = ActivationRecord {
            class,
            current: previous,
            previous: Some(active.current.clone()),
            activated_at_ms: now_ms,
        };
        self.write_activation(&record)?;
        if let Err(reason) = health.check(&installed) {
            self.write_activation(&active)?;
            return Err(PackageError::HealthCheckFailed(reason));
        }
        self.audit(
            "rollback",
            Some(record.current.clone()),
            "explicit rollback passed health check",
        )?;
        Ok(record)
    }

    pub fn disk_usage(&self) -> Result<DiskUsage, PackageError> {
        let (object_bytes, object_count) = directory_usage(&self.root.join("objects"))?;
        let (download_bytes, _) = directory_usage(&self.root.join("downloads"))?;
        Ok(DiskUsage {
            object_bytes,
            download_bytes,
            object_count,
        })
    }

    pub fn unregister(&self, reference: &PackageRef) -> Result<(), PackageError> {
        if self.active_references()?.contains(&reference.digest) {
            return Err(PackageError::ActiveWorkloads);
        }
        let path = self.manifest_path(&reference.id, &reference.version);
        if !path.exists() {
            return Err(PackageError::UnknownPackage);
        }
        std::fs::remove_file(path)?;
        self.audit(
            "unregistered",
            Some(reference.clone()),
            "artifact retained until explicit cleanup",
        )
    }

    pub fn cleanup_candidates(&self) -> Result<BTreeSet<String>, PackageError> {
        let referenced: BTreeSet<_> = self
            .manifests()?
            .into_iter()
            .map(|signed| signed.manifest.digest)
            .chain(self.active_references()?)
            .collect();
        let mut candidates = BTreeSet::new();
        for entry in std::fs::read_dir(self.root.join("objects"))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_dir() && !referenced.contains(&name) {
                candidates.insert(name);
            }
        }
        Ok(candidates)
    }

    pub fn remove_unreferenced(&self, requested: &BTreeSet<String>) -> Result<u64, PackageError> {
        let candidates = self.cleanup_candidates()?;
        if !requested.is_subset(&candidates) {
            return Err(PackageError::UnknownPackage);
        }
        let mut removed = 0u64;
        for digest in requested {
            let path = self.root.join("objects").join(digest);
            removed = removed.saturating_add(directory_usage(&path)?.0);
            std::fs::remove_dir_all(path)?;
        }
        self.audit(
            "cleanup",
            None,
            &format!("removed {removed} unreferenced bytes"),
        )?;
        Ok(removed)
    }

    pub fn export_bundle(
        &self,
        reference: &PackageRef,
        destination: impl AsRef<Path>,
    ) -> Result<(), PackageError> {
        let installed = self.installed(reference)?;
        let file = File::create(destination)?;
        let mut builder = tar::Builder::new(file);
        let manifest = serde_json::to_vec_pretty(&installed.signed_manifest)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder.append_data(&mut header, "manifest.json", manifest.as_slice())?;
        builder.append_path_with_name(&installed.artifact_path, "artifact")?;
        builder.finish()?;
        Ok(())
    }

    pub fn import_bundle(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<InstalledPackage, PackageError> {
        let file = File::open(source)?;
        let mut archive = tar::Archive::new(file);
        let mut manifest = None;
        let mut artifact = tempfile::NamedTempFile::new_in(self.root.join("downloads"))?;
        let mut found_artifact = false;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_string_lossy().into_owned();
            match path.as_str() {
                "manifest.json" => {
                    let mut bytes = Vec::new();
                    entry.take(1024 * 1024).read_to_end(&mut bytes)?;
                    manifest = Some(serde_json::from_slice::<SignedPackageManifest>(&bytes)?);
                }
                "artifact" => {
                    std::io::copy(&mut entry, artifact.as_file_mut())?;
                    artifact.as_file_mut().sync_all()?;
                    found_artifact = true;
                }
                _ => return Err(PackageError::InvalidBundle),
            }
        }
        let manifest = manifest.ok_or(PackageError::InvalidBundle)?;
        if !found_artifact {
            return Err(PackageError::InvalidBundle);
        }
        self.install_from_path(artifact.path(), manifest, false)
    }

    fn install_from_path(
        &self,
        artifact: &Path,
        signed: SignedPackageManifest,
        local: bool,
    ) -> Result<InstalledPackage, PackageError> {
        signed.verify_signature(&self.trust, local)?;
        self.preflight(&signed.manifest)?;
        let actual_size = artifact.metadata()?.len();
        if actual_size != signed.manifest.artifact_bytes {
            return Err(PackageError::SizeMismatch {
                expected: signed.manifest.artifact_bytes,
                actual: actual_size,
            });
        }
        let object_dir = self.root.join("objects").join(&signed.manifest.digest);
        let object_path = object_dir.join("artifact");
        let additional_bytes = if object_path.exists() {
            0
        } else {
            signed.manifest.installed_bytes
        };
        if self
            .disk_usage()?
            .object_bytes
            .saturating_add(additional_bytes)
            > self.limits.maximum_store_bytes
        {
            return Err(PackageError::StoreLimitExceeded);
        }
        if hash_file(artifact)? != signed.manifest.digest {
            return Err(PackageError::DigestMismatch);
        }
        if object_path.exists() {
            if hash_file(&object_path)? != signed.manifest.digest {
                return Err(PackageError::DigestMismatch);
            }
        } else {
            std::fs::create_dir_all(&object_dir)?;
            let mut temp = tempfile::NamedTempFile::new_in(&object_dir)?;
            let mut input = File::open(artifact)?;
            std::io::copy(&mut input, temp.as_file_mut())?;
            temp.as_file_mut().sync_all()?;
            temp.persist(&object_path)
                .map_err(|error| PackageError::Io(error.error.to_string()))?;
        }
        let manifest_path = self.manifest_path(&signed.manifest.id, &signed.manifest.version);
        if manifest_path.exists() {
            let existing: SignedPackageManifest =
                serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
            if existing != signed {
                return Err(PackageError::VersionAlreadyRegistered);
            }
        }
        atomic_write(&manifest_path, &serde_json::to_vec_pretty(&signed)?)?;
        let reference = PackageRef::from(&signed.manifest);
        self.audit(
            "installed",
            Some(reference),
            if local {
                "local import"
            } else {
                "verified artifact"
            },
        )?;
        Ok(InstalledPackage {
            signed_manifest: signed,
            artifact_path: object_path,
        })
    }

    fn preflight(&self, manifest: &PackageManifest) -> Result<(), PackageError> {
        manifest.validate()?;
        if !self.platform.supports(manifest) {
            return Err(PackageError::PlatformIncompatible);
        }
        if !self.compatibility.supports(manifest) {
            return Err(PackageError::ProtocolIncompatible);
        }
        Ok(())
    }

    fn check_conflicts(&self, manifest: &PackageManifest) -> Result<(), PackageError> {
        let mut active = BTreeSet::new();
        for class in all_classes() {
            if let Some(record) = self.active(class)? {
                active.insert(record.current.id);
            }
        }
        if let Some(conflict) = manifest
            .conflicts
            .iter()
            .find(|conflict| active.contains(*conflict))
        {
            return Err(PackageError::Conflict(conflict.clone()));
        }
        Ok(())
    }

    fn active_references(&self) -> Result<BTreeSet<String>, PackageError> {
        let mut references = BTreeSet::new();
        for class in all_classes() {
            if let Some(record) = self.active(class)? {
                references.insert(record.current.digest);
                if let Some(previous) = record.previous {
                    references.insert(previous.digest);
                }
            }
        }
        Ok(references)
    }

    fn write_activation(&self, record: &ActivationRecord) -> Result<(), PackageError> {
        atomic_write(
            &self.activation_path(record.class),
            &serde_json::to_vec_pretty(record)?,
        )
    }

    fn activation_path(&self, class: ArtifactClass) -> PathBuf {
        self.root
            .join("active")
            .join(format!("{}.json", class.key()))
    }

    fn manifest_path(&self, id: &str, version: &str) -> PathBuf {
        self.root
            .join("manifests")
            .join(format!("{}--{}.json", safe(id), safe(version)))
    }

    fn object_path(&self, digest: &str) -> PathBuf {
        self.root.join("objects").join(digest).join("artifact")
    }

    fn audit(
        &self,
        action: &str,
        package: Option<PackageRef>,
        detail: &str,
    ) -> Result<(), PackageError> {
        let path = self.root.join("audit.jsonl");
        let sequence = if path.exists() {
            std::fs::read_to_string(&path)?.lines().count() as u64 + 1
        } else {
            1
        };
        let event = PackageAuditEvent {
            sequence,
            action: action.into(),
            package,
            detail: detail.into(),
        };
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(&mut file, &event)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    }
}

fn hash_file(path: &Path) -> Result<String, PackageError> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), PackageError> {
    let parent = path
        .parent()
        .ok_or_else(|| PackageError::Io("path has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path)
        .map_err(|error| PackageError::Io(error.error.to_string()))?;
    Ok(())
}

fn safe(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn directory_usage(path: &Path) -> Result<(u64, usize), PackageError> {
    if !path.exists() {
        return Ok((0, 0));
    }
    if path.is_file() {
        return Ok((path.metadata()?.len(), 1));
    }
    let mut bytes = 0u64;
    let mut files = 0usize;
    for entry in std::fs::read_dir(path)? {
        let (entry_bytes, entry_files) = directory_usage(&entry?.path())?;
        bytes = bytes.saturating_add(entry_bytes);
        files += entry_files;
    }
    Ok((bytes, files))
}

fn all_classes() -> [ArtifactClass; 7] {
    [
        ArtifactClass::VkoreApplication,
        ArtifactClass::KoreGuest,
        ArtifactClass::VmRuntime,
        ArtifactClass::InferenceRuntime,
        ArtifactClass::Model,
        ArtifactClass::Addon,
        ArtifactClass::GuestTool,
    ]
}
