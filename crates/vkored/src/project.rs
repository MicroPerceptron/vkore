//! Portable project configuration and bounded host workspace preflight.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

pub const PROJECT_SCHEMA_VERSION: u16 = 2;
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
    /// Legacy roots migrate to optional focus folders. Virtual projects have
    /// no user folder; their empty backing directory is application-owned.
    #[serde(default)]
    pub virtual_only: bool,
    pub root: PathBuf,
    pub state: PathBuf,
    pub access: WorkspaceAccess,
    #[serde(default)]
    pub exclusions: Vec<String>,
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
    InvalidWorkspaceState,
    StateInsideWorkspace,
    WorkspaceInsideState,
    MissingArtifact(String),
    UnsafeWorkspace(String),
    UnsupportedWorkspace(String),
    InvalidRuntime,
    NotFound,
    Conflict,
    Io(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity => f.write_str("The generated project identity is invalid."),
            Self::InvalidName => f.write_str("Enter a project name between 1 and 128 characters."),
            Self::UnsupportedSchema => {
                f.write_str("This project was created by an unsupported vKore version.")
            }
            Self::RootMissing => f.write_str("The selected workspace no longer exists."),
            Self::RootNotDirectory => f.write_str("Select a workspace folder, not a file."),
            Self::InvalidWorkspaceState => {
                f.write_str("vKore could not derive a safe application state location.")
            }
            Self::StateInsideWorkspace | Self::WorkspaceInsideState => f.write_str(
                "The application state and workspace overlap. Choose a different workspace folder.",
            ),
            Self::MissingArtifact(artifact) => write!(
                f,
                "The configured {artifact} is unavailable. Choose an installed model in Settings."
            ),
            Self::UnsafeWorkspace(reason) => {
                write!(f, "This workspace is too broad or sensitive: {reason}")
            }
            Self::UnsupportedWorkspace(reason) => {
                write!(f, "This workspace contains an unsupported entry: {reason}")
            }
            Self::InvalidRuntime => f.write_str(
                "The configured model runtime is incomplete. Choose an available model in Settings.",
            ),
            Self::NotFound => f.write_str("That project is no longer available."),
            Self::Conflict => f.write_str("A project with that identity already exists."),
            Self::Io(reason) => write!(f, "The project could not be saved: {reason}"),
        }
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
            let path = entry.path();
            let bytes = fs::read(&path).map_err(io_error)?;
            let mut serialized: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|error| ProjectError::Io(error.to_string()))?;
            let legacy_focus = serialized["schema_version"].as_u64() == Some(1);
            if legacy_focus {
                serialized["schema_version"] = serde_json::json!(PROJECT_SCHEMA_VERSION);
                serialized["workspace"]["virtual_only"] = serde_json::json!(false);
            }
            let had_legacy_quotas = serialized
                .pointer_mut("/workspace")
                .and_then(serde_json::Value::as_object_mut)
                .is_some_and(|workspace| {
                    workspace.remove("max_write_bytes").is_some()
                        | workspace.remove("max_write_files").is_some()
                });
            let config: ProjectConfig = serde_json::from_value(serialized)
                .map_err(|error| ProjectError::Io(error.to_string()))?;
            if had_legacy_quotas || legacy_focus {
                write_project_atomically(&root, &config)?;
            }
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
        write_project_atomically(&self.root, &config)?;
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

    /// Creates the ordinary folder-first project shape used by the native UI.
    ///
    /// Project identity and state are derived from the canonical workspace;
    /// selecting the same folder again resolves to the existing project.
    pub fn create_virtual(
        &self,
        name: &str,
        inference: InferenceConfig,
    ) -> Result<NewProjectOutcome, ProjectError> {
        if name.trim().is_empty() || name.len() > 80 {
            return Err(ProjectError::InvalidName);
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| ProjectError::Io(e.to_string()))?
            .as_nanos();
        let empty = self
            .root
            .parent()
            .unwrap_or(&self.root)
            .join("virtual-projects")
            .join(format!("task-{nonce:x}"));
        fs::create_dir_all(&empty).map_err(io_error)?;
        let mut outcome = self.create_from_workspace(&empty, inference)?;
        outcome.project.name = name.trim().into();
        outcome.project.workspace.virtual_only = true;
        outcome.project.workspace.access = WorkspaceAccess::ReadOnly;
        outcome.project = self.save(outcome.project)?;
        outcome.preflight = preflight(&outcome.project)?;
        Ok(outcome)
    }

    pub fn create_from_workspace(
        &self,
        workspace: &Path,
        inference: InferenceConfig,
    ) -> Result<NewProjectOutcome, ProjectError> {
        let canonical = workspace
            .canonicalize()
            .map_err(|_| ProjectError::RootMissing)?;
        if !canonical.is_dir() {
            return Err(ProjectError::RootNotDirectory);
        }
        if let Some(mut project) = self
            .list()
            .into_iter()
            .find(|project| project.workspace.root.canonicalize().ok().as_ref() == Some(&canonical))
        {
            if project.archived {
                project.archived = false;
                project = self.save(project)?;
            }
            let report = preflight(&project)?;
            return Ok(NewProjectOutcome {
                project,
                preflight: report,
                created: false,
            });
        }
        if let Some(warning) = broad_root_warning(&canonical) {
            return Err(ProjectError::UnsafeWorkspace(warning));
        }

        let name = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Workspace")
            .to_owned();
        let id = self.unique_workspace_id(&name, &canonical)?;
        let state = self.derived_state(&id, &canonical);
        let config = ProjectConfig {
            schema_version: PROJECT_SCHEMA_VERSION,
            id: id.clone(),
            name,
            workspace: WorkspaceConfig {
                virtual_only: false,
                root: canonical,
                state,
                // A project can make useful edits, but every write remains in
                // the explicit review/approval path by default.
                access: WorkspaceAccess::AutomaticWriteback,
                exclusions: vec![".git/objects".into()],
            },
            inference,
            addons: Vec::new(),
            approvals: ApprovalProfile {
                default_scope: "once".into(),
                confirm_external_writes: true,
                confirm_destructive: true,
                standing_policy_enabled: false,
            },
            retention: RetentionConfig {
                transcript_days: 30,
                clean_kv_on_close: true,
                retain_raw_model_output: false,
            },
            resources: ResourceLimits {
                memory_mib: 2_048,
                virtual_cpus: 2,
                prompt_seconds: 300,
                agent_limit: 4,
            },
            archived: false,
        };
        let report = preflight(&config)?;
        if let Some(entry) = report.unsupported_entries.first() {
            return Err(ProjectError::UnsupportedWorkspace(entry.clone()));
        }
        let project = self.save(config)?;
        Ok(NewProjectOutcome {
            project,
            preflight: report,
            created: true,
        })
    }

    fn unique_workspace_id(&self, name: &str, workspace: &Path) -> Result<String, ProjectError> {
        let mut slug = name
            .chars()
            .flat_map(char::to_lowercase)
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '-'
                }
            })
            .collect::<String>();
        while slug.contains("--") {
            slug = slug.replace("--", "-");
        }
        let slug = slug.trim_matches('-');
        let slug = if slug.is_empty() { "project" } else { slug };
        let digest = blake3::hash(workspace.as_os_str().to_string_lossy().as_bytes()).to_hex();
        let prefix = slug.chars().take(47).collect::<String>();
        let base = format!("{prefix}-{}", &digest.as_str()[..12]);
        let projects = self
            .projects
            .lock()
            .map_err(|_| ProjectError::Io("project store lock poisoned".into()))?;
        if !projects.contains_key(&base) {
            return Ok(base);
        }
        for suffix in 2..=9_999 {
            let candidate = format!("{}-{suffix}", base.chars().take(59).collect::<String>());
            if !projects.contains_key(&candidate) {
                return Ok(candidate);
            }
        }
        Err(ProjectError::Conflict)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NewProjectOutcome {
    pub project: ProjectConfig,
    pub preflight: WorkspacePreflight,
    pub created: bool,
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
        writeback_confirmation_required: config.workspace.access == WorkspaceAccess::ReviewChanges,
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
    if config.workspace.state.as_os_str().is_empty() {
        return Err(ProjectError::InvalidWorkspaceState);
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

fn write_project_atomically(root: &Path, config: &ProjectConfig) -> Result<(), ProjectError> {
    let bytes =
        serde_json::to_vec_pretty(config).map_err(|error| ProjectError::Io(error.to_string()))?;
    let path = root.join(format!("{}.json", config.id));
    let temporary = root.join(format!(".{}.json.tmp", config.id));
    let backup = root.join(format!(".{}.json.bak", config.id));
    let mut file = fs::File::create(&temporary).map_err(io_error)?;
    use std::io::Write as _;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(io_error)?;
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup).map_err(io_error)?;
        }
        fs::rename(&path, &backup).map_err(io_error)?;
    }
    if let Err(error) = fs::rename(&temporary, &path) {
        if backup.exists() {
            let _ = fs::rename(&backup, &path);
        }
        return Err(io_error(error));
    }
    if backup.exists() {
        fs::remove_file(backup).map_err(io_error)?;
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
