//! Portable project configuration and bounded host workspace preflight.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

pub const PROJECT_SCHEMA_VERSION: u16 = 1;
const SCAN_ENTRY_LIMIT: usize = 2_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceAccess {
    ReadOnly,
    ReviewChanges,
    AutomaticWriteback,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    pub state: PathBuf,
    pub access: WorkspaceAccess,
    #[serde(default)]
    pub exclusions: Vec<String>,
    pub max_write_bytes: u64,
    pub max_write_files: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackend {
    Deterministic,
    LlamaCpp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InferenceConfig {
    pub backend: RuntimeBackend,
    pub kore_image: Option<PathBuf>,
    pub llama_server: Option<PathBuf>,
    pub model: Option<PathBuf>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    pub context_tokens: u32,
    pub output_tokens: u32,
    pub threads: u16,
    pub slots: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AddonSelection {
    pub id: String,
    pub enabled: bool,
    pub mediation: String,
    #[serde(default)]
    pub tool_ceiling: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalProfile {
    pub default_scope: String,
    pub confirm_external_writes: bool,
    pub confirm_destructive: bool,
    pub standing_policy_enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RetentionConfig {
    pub transcript_days: u16,
    pub clean_kv_on_close: bool,
    pub retain_raw_model_output: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceLimits {
    pub memory_mib: u32,
    pub virtual_cpus: u16,
    pub prompt_seconds: u32,
    pub agent_limit: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectConfig {
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub workspace: WorkspaceConfig,
    pub inference: InferenceConfig,
    #[serde(default)]
    pub addons: Vec<AddonSelection>,
    pub approvals: ApprovalProfile,
    pub retention: RetentionConfig,
    pub resources: ResourceLimits,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkspacePreflight {
    pub resolved_root: PathBuf,
    pub resolved_state: PathBuf,
    pub broad_root_warning: Option<String>,
    pub entries_scanned: usize,
    pub symlinks: usize,
    pub unsupported_entries: Vec<String>,
    pub estimated_workspace_bytes: u64,
    pub writeback_confirmation_required: bool,
    pub ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectError {
    InvalidIdentity,
    InvalidName,
    UnsupportedSchema,
    RootMissing,
    RootNotDirectory,
    StateInsideWorkspace,
    WorkspaceInsideState,
    MissingArtifact(String),
    UnsafeWritebackBounds,
    InvalidRuntime,
    NotFound,
    Conflict,
    Io(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProjectError {}

pub struct ProjectStore {
    root: PathBuf,
    projects: Mutex<BTreeMap<String, ProjectConfig>>,
}

impl ProjectStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ProjectError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(io_error)?;
        let mut projects = BTreeMap::new();
        for entry in fs::read_dir(&root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(io_error)?;
            let config: ProjectConfig = serde_json::from_slice(&bytes)
                .map_err(|error| ProjectError::Io(error.to_string()))?;
            projects.insert(config.id.clone(), config);
        }
        Ok(Self {
            root,
            projects: Mutex::new(projects),
        })
    }

    pub fn list(&self) -> Vec<ProjectConfig> {
        self.projects
            .lock()
            .map(|projects| projects.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Result<ProjectConfig, ProjectError> {
        self.projects
            .lock()
            .map_err(|_| ProjectError::Io("project store lock poisoned".into()))?
            .get(id)
            .cloned()
            .ok_or(ProjectError::NotFound)
    }

    pub fn save(&self, config: ProjectConfig) -> Result<ProjectConfig, ProjectError> {
        validate_config(&config)?;
        let report = preflight(&config)?;
        if !report.ready {
            return Err(ProjectError::RootMissing);
        }
        fs::create_dir_all(&report.resolved_state).map_err(io_error)?;
        let bytes = serde_json::to_vec_pretty(&config)
            .map_err(|error| ProjectError::Io(error.to_string()))?;
        let path = self.root.join(format!("{}.json", config.id));
        let temporary = self.root.join(format!(".{}.json.tmp", config.id));
        fs::write(&temporary, &bytes).map_err(io_error)?;
        fs::rename(&temporary, &path).map_err(io_error)?;
        self.projects
            .lock()
            .map_err(|_| ProjectError::Io("project store lock poisoned".into()))?
            .insert(config.id.clone(), config.clone());
        Ok(config)
    }

    pub fn archive(&self, id: &str, archived: bool) -> Result<ProjectConfig, ProjectError> {
        let mut config = self.get(id)?;
        config.archived = archived;
        self.save(config)
    }

    pub fn duplicate(
        &self,
        id: &str,
        new_id: &str,
        new_name: &str,
    ) -> Result<ProjectConfig, ProjectError> {
        if self
            .projects
            .lock()
            .map_err(|_| ProjectError::Conflict)?
            .contains_key(new_id)
        {
            return Err(ProjectError::Conflict);
        }
        let mut config = self.get(id)?;
        config.id = new_id.into();
        config.name = new_name.into();
        config.workspace.state =
            derived_workspace_state(&self.root, new_id, &config.workspace.root);
        self.save(config)
    }

    pub fn derived_state(&self, id: &str, workspace: &Path) -> PathBuf {
        derived_workspace_state(&self.root, id, workspace)
    }
}

pub fn derived_workspace_state(project_store: &Path, id: &str, workspace: &Path) -> PathBuf {
    let digest = blake3::hash(workspace.as_os_str().to_string_lossy().as_bytes()).to_hex();
    project_store
        .parent()
        .unwrap_or(project_store)
        .join("workspaces")
        .join(id)
        .join(&digest.as_str()[..16])
}

pub fn preflight(config: &ProjectConfig) -> Result<WorkspacePreflight, ProjectError> {
    validate_config(config)?;
    let resolved_root = config
        .workspace
        .root
        .canonicalize()
        .map_err(|_| ProjectError::RootMissing)?;
    if !resolved_root.is_dir() {
        return Err(ProjectError::RootNotDirectory);
    }
    let resolved_state = absolute_clean(&config.workspace.state)?;
    if resolved_state.starts_with(&resolved_root) {
        return Err(ProjectError::StateInsideWorkspace);
    }
    if resolved_root.starts_with(&resolved_state) {
        return Err(ProjectError::WorkspaceInsideState);
    }
    let warning = broad_root_warning(&resolved_root);
    let mut pending = vec![resolved_root.clone()];
    let mut entries_scanned = 0usize;
    let mut symlinks = 0usize;
    let mut unsupported_entries = Vec::new();
    let mut estimated_workspace_bytes = 0u64;
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                unsupported_entries.push(format!("{}: {error}", directory.display()));
                continue;
            }
        };
        for entry in entries {
            if entries_scanned == SCAN_ENTRY_LIMIT {
                unsupported_entries.push(format!("scan bounded at {SCAN_ENTRY_LIMIT} entries"));
                pending.clear();
                break;
            }
            entries_scanned += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    unsupported_entries.push(error.to_string());
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(value) => value,
                Err(error) => {
                    unsupported_entries.push(format!("{}: {error}", entry.path().display()));
                    continue;
                }
            };
            if file_type.is_symlink() {
                symlinks += 1;
            } else if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                estimated_workspace_bytes = estimated_workspace_bytes
                    .saturating_add(entry.metadata().map(|value| value.len()).unwrap_or(0));
            } else {
                unsupported_entries.push(entry.path().display().to_string());
            }
        }
    }
    Ok(WorkspacePreflight {
        resolved_root,
        resolved_state,
        broad_root_warning: warning,
        entries_scanned,
        symlinks,
        unsupported_entries,
        estimated_workspace_bytes,
        writeback_confirmation_required: config.workspace.access != WorkspaceAccess::ReadOnly,
        ready: true,
    })
}

fn validate_config(config: &ProjectConfig) -> Result<(), ProjectError> {
    if config.schema_version != PROJECT_SCHEMA_VERSION {
        return Err(ProjectError::UnsupportedSchema);
    }
    if !valid_id(&config.id) {
        return Err(ProjectError::InvalidIdentity);
    }
    if config.name.trim().is_empty() || config.name.len() > 128 {
        return Err(ProjectError::InvalidName);
    }
    if config.workspace.access != WorkspaceAccess::ReadOnly
        && (config.workspace.max_write_bytes == 0 || config.workspace.max_write_files == 0)
    {
        return Err(ProjectError::UnsafeWritebackBounds);
    }
    for (label, path) in [
        ("Kore image", config.inference.kore_image.as_ref()),
        ("llama.cpp runtime", config.inference.llama_server.as_ref()),
        ("model", config.inference.model.as_ref()),
    ] {
        if let Some(path) = path {
            let Ok(metadata) = path.metadata() else {
                return Err(ProjectError::MissingArtifact(label.into()));
            };
            if !metadata.is_file() {
                return Err(ProjectError::MissingArtifact(label.into()));
            }
        }
    }
    if config.inference.backend == RuntimeBackend::LlamaCpp
        && (config.inference.llama_server.is_none() || config.inference.model.is_none())
    {
        return Err(ProjectError::InvalidRuntime);
    }
    Ok(())
}

fn broad_root_warning(root: &Path) -> Option<String> {
    if root.parent().is_none() {
        return Some("A drive or filesystem root exposes an unusually broad scope.".into());
    }
    for variable in ["USERPROFILE", "HOME"] {
        if std::env::var_os(variable).is_some_and(|home| home == root.as_os_str()) {
            return Some(
                "Your entire home directory may contain credentials and private data.".into(),
            );
        }
    }
    let sensitive = [".ssh", ".aws", ".azure", ".gnupg", ".kube"];
    if root.components().any(|component| {
        let Component::Normal(value) = component else {
            return false;
        };
        sensitive
            .iter()
            .any(|name| value.eq_ignore_ascii_case(name))
    }) {
        return Some("This directory commonly contains authentication credentials.".into());
    }
    None
}

fn absolute_clean(path: &Path) -> Result<PathBuf, ProjectError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(io_error)?
    };
    if absolute.exists() {
        return absolute.canonicalize().map_err(io_error);
    }
    let ancestor = absolute
        .ancestors()
        .find(|candidate| candidate.exists())
        .ok_or(ProjectError::RootMissing)?;
    let suffix = absolute
        .strip_prefix(ancestor)
        .map_err(|_| ProjectError::RootMissing)?;
    Ok(ancestor.canonicalize().map_err(io_error)?.join(suffix))
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
fn io_error(error: std::io::Error) -> ProjectError {
    ProjectError::Io(error.to_string())
}
