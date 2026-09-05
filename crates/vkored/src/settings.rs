//! Application-owned preferences and privacy-safe recent activity.

use crate::project::{InferenceConfig, ProjectConfig, ProjectStore, RuntimeBackend};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SETTINGS_SCHEMA_VERSION: u16 = 1;
const ACTIVITY_LIMIT: usize = 50;
const ACTIVITY_FILE_LIMIT: usize = 200;
const ACTIVITY_BYTES_PER_FILE: u64 = 1_048_576;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyFont {
    #[default]
    DesignDefault,
    Archivo,
    JetbrainsMono,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettings {
    pub schema_version: u16,
    pub default_inference: InferenceConfig,
    #[serde(default)]
    pub body_font: BodyFont,
    #[serde(default = "default_true")]
    pub show_working: bool,
    #[serde(default)]
    pub working_expanded_default: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            default_inference: unconfigured_live_inference(),
            body_font: BodyFont::DesignDefault,
            show_working: true,
            working_expanded_default: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelOption {
    pub id: String,
    pub display_name: String,
    pub backend: String,
    pub provider: String,
    pub path: Option<PathBuf>,
    pub status: &'static str,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActivitySummary {
    pub project_id: String,
    pub project_name: String,
    pub session_id: String,
    pub model: String,
    pub started_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub status: String,
    pub prompts: u32,
    pub completed: u32,
    pub cancelled: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SettingsError {
    InvalidFont,
    UnavailableModel,
    Io(String),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFont => formatter.write_str(
                "That font is not registered. Choose one of the fonts shown in Settings.",
            ),
            Self::UnavailableModel => formatter.write_str(
                "That model is unavailable. Restore its local files or choose an installed model.",
            ),
            Self::Io(reason) => write!(
                formatter,
                "Application settings could not be saved: {reason}"
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

pub struct SettingsStore {
    path: PathBuf,
    settings: Mutex<AppSettings>,
}

impl SettingsStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, SettingsError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        recover_interrupted_write(&path)?;
        let (settings, rewrite) = if path.exists() {
            match fs::read(&path).map_err(io_error).and_then(|bytes| {
                serde_json::from_slice::<Value>(&bytes)
                    .map_err(|error| SettingsError::Io(error.to_string()))
            }) {
                Ok(value) => migrate(value),
                Err(_) => {
                    preserve_corrupt(&path)?;
                    (AppSettings::default(), true)
                }
            }
        } else {
            (AppSettings::default(), true)
        };
        let store = Self {
            path,
            settings: Mutex::new(settings),
        };
        if rewrite {
            store.persist()?;
        }
        Ok(store)
    }

    pub fn get(&self) -> Result<AppSettings, SettingsError> {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .map_err(|_| SettingsError::Io("settings lock poisoned".into()))
    }

    pub fn view(&self, projects: &ProjectStore) -> Result<Value, SettingsError> {
        let settings = self.get()?;
        let options = model_candidates(&settings, projects);
        let selected = model_id(&settings.default_inference);
        Ok(json!({
            "schema_version": settings.schema_version,
            "default_model_id": selected,
            "default_inference": settings.default_inference,
            "body_font": settings.body_font,
            "show_working": settings.show_working,
            "working_expanded_default": settings.working_expanded_default,
            "models": options.into_iter().map(|(_, option)| option).collect::<Vec<_>>(),
            "fonts": [
                {"id":"design_default","name":"System / design default","family":"Space Grotesk","status":"installed"},
                {"id":"archivo","name":"Archivo","family":"Archivo","status":"installed"},
                {"id":"jetbrains_mono","name":"JetBrains Mono","family":"JetBrains Mono","status":"installed"}
            ]
        }))
    }

    pub fn update(&self, patch: &Value, projects: &ProjectStore) -> Result<Value, SettingsError> {
        let mut next = self.get()?;
        if let Some(id) = patch.get("default_model_id").and_then(Value::as_str) {
            let candidates = model_candidates(&next, projects);
            let Some((inference, option)) =
                candidates.into_iter().find(|(_, option)| option.id == id)
            else {
                return Err(SettingsError::UnavailableModel);
            };
            if option.status != "installed" {
                return Err(SettingsError::UnavailableModel);
            }
            next.default_inference = inference;
        }
        if let Some(font) = patch.get("body_font").and_then(Value::as_str) {
            next.body_font = match font {
                "design_default" => BodyFont::DesignDefault,
                "archivo" => BodyFont::Archivo,
                "jetbrains_mono" => BodyFont::JetbrainsMono,
                _ => return Err(SettingsError::InvalidFont),
            };
        }
        if let Some(show) = patch.get("show_working").and_then(Value::as_bool) {
            next.show_working = show;
        }
        if let Some(expanded) = patch
            .get("working_expanded_default")
            .and_then(Value::as_bool)
        {
            next.working_expanded_default = expanded;
        }
        next.schema_version = SETTINGS_SCHEMA_VERSION;
        self.persist_value(&next)?;
        *self
            .settings
            .lock()
            .map_err(|_| SettingsError::Io("settings lock poisoned".into()))? = next;
        self.view(projects)
    }

    pub fn default_inference(
        &self,
        projects: &ProjectStore,
    ) -> Result<InferenceConfig, SettingsError> {
        let settings = self.get()?;
        let id = model_id(&settings.default_inference);
        let Some((inference, option)) = model_candidates(&settings, projects)
            .into_iter()
            .find(|(_, option)| option.id == id)
        else {
            return Err(SettingsError::UnavailableModel);
        };
        if option.status != "installed" {
            return Err(SettingsError::UnavailableModel);
        }
        Ok(inference)
    }

    fn persist(&self) -> Result<(), SettingsError> {
        let settings = self.get()?;
        self.persist_value(&settings)
    }

    fn persist_value(&self, settings: &AppSettings) -> Result<(), SettingsError> {
        let bytes = serde_json::to_vec_pretty(&settings)
            .map_err(|error| SettingsError::Io(error.to_string()))?;
        atomic_write(&self.path, &bytes)
    }
}

pub fn recent_activity(
    transcript_root: &Path,
    projects: &ProjectStore,
) -> Result<Vec<ActivitySummary>, SettingsError> {
    if !transcript_root.exists() {
        return Ok(Vec::new());
    }
    let project_map = projects
        .list()
        .into_iter()
        .map(|project| (project.id.clone(), project))
        .collect::<BTreeMap<_, _>>();
    let mut entries = fs::read_dir(transcript_root)
        .map_err(io_error)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("jsonl"))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH),
        )
    });
    let mut activity = Vec::new();
    for entry in entries.into_iter().take(ACTIVITY_FILE_LIMIT) {
        if let Some(summary) = summarize_transcript(&entry.path(), &project_map)? {
            activity.push(summary);
        }
    }
    activity.sort_by_key(|summary| std::cmp::Reverse(summary.updated_at_unix_ms));
    activity.truncate(ACTIVITY_LIMIT);
    Ok(activity)
}

fn summarize_transcript(
    path: &Path,
    projects: &BTreeMap<String, ProjectConfig>,
) -> Result<Option<ActivitySummary>, SettingsError> {
    let mut file = File::open(path).map_err(io_error)?;
    let length = file.metadata().map_err(io_error)?.len();
    let offset = length.saturating_sub(ACTIVITY_BYTES_PER_FILE);
    file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io_error)?;
    if offset > 0 {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        } else {
            return Ok(None);
        }
    }

    let mut summary: Option<ActivitySummary> = None;
    for line in BufReader::new(bytes.as_slice()).lines() {
        let Ok(line) = line else { continue };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let project_id = event["project_id"].as_str().unwrap_or_default();
        let session_id = event["session_id"].as_str().unwrap_or_default();
        let timestamp = event["timestamp_unix_ms"].as_u64().unwrap_or_default();
        if project_id.is_empty() || session_id.is_empty() {
            continue;
        }
        let project = projects.get(project_id);
        let current = summary.get_or_insert_with(|| ActivitySummary {
            project_id: project_id.into(),
            project_name: project
                .map_or(project_id, |project| project.name.as_str())
                .into(),
            session_id: session_id.into(),
            model: project.map_or_else(
                || "Unavailable".into(),
                |project| model_display_name(&project.inference),
            ),
            started_at_unix_ms: timestamp,
            updated_at_unix_ms: timestamp,
            status: "open".into(),
            prompts: 0,
            completed: 0,
            cancelled: 0,
            failed: 0,
        });
        current.started_at_unix_ms = current.started_at_unix_ms.min(timestamp);
        current.updated_at_unix_ms = current.updated_at_unix_ms.max(timestamp);
        match event["kind"].as_str().unwrap_or_default() {
            "user_prompt" => {
                current.prompts = current.prompts.saturating_add(1);
                current.status = "running".into();
            }
            "prompt_terminal" => match event["payload"]["status"].as_str().unwrap_or_default() {
                "completed" => {
                    current.completed = current.completed.saturating_add(1);
                    current.status = "completed".into();
                }
                "cancelled" => {
                    current.cancelled = current.cancelled.saturating_add(1);
                    current.status = "cancelled".into();
                }
                "failed" => {
                    current.failed = current.failed.saturating_add(1);
                    current.status = "failed".into();
                }
                _ => {}
            },
            "conversation_closed" => current.status = "closed".into(),
            _ => {}
        }
    }
    Ok(summary)
}

fn model_candidates(
    settings: &AppSettings,
    projects: &ProjectStore,
) -> Vec<(InferenceConfig, ModelOption)> {
    let mut candidates = vec![settings.default_inference.clone()];
    candidates.extend(projects.list().into_iter().map(|project| project.inference));
    let mut unique = BTreeMap::new();
    for inference in candidates {
        if inference.backend != RuntimeBackend::LlamaCpp {
            continue;
        }
        unique.entry(model_id(&inference)).or_insert(inference);
    }
    unique
        .into_values()
        .map(|inference| {
            let option = model_option(&inference);
            (inference, option)
        })
        .collect()
}

fn model_option(inference: &InferenceConfig) -> ModelOption {
    let installed = inference_available(inference);
    match inference.backend {
        RuntimeBackend::Deterministic => ModelOption {
            id: model_id(inference),
            display_name: "Legacy test backend (disabled)".into(),
            backend: "deterministic".into(),
            provider: "vKore".into(),
            path: None,
            status: "unavailable",
            detail: "Live llama.cpp inference is required".into(),
        },
        RuntimeBackend::LlamaCpp => {
            let path = inference.model.clone();
            ModelOption {
                id: model_id(inference),
                display_name: path
                    .as_deref()
                    .and_then(Path::file_name)
                    .and_then(|value| value.to_str())
                    .unwrap_or("Unavailable local model")
                    .into(),
                backend: "llama_cpp".into(),
                provider: "llama.cpp".into(),
                path,
                status: if installed {
                    "installed"
                } else {
                    "unavailable"
                },
                detail: if installed {
                    "Local GGUF model and llama.cpp runtime are available".into()
                } else {
                    "The model, runtime, or Kore image is missing".into()
                },
            }
        }
    }
}

pub fn model_id(inference: &InferenceConfig) -> String {
    match inference.backend {
        RuntimeBackend::Deterministic => "deterministic".into(),
        RuntimeBackend::LlamaCpp => {
            let identity = serde_json::to_vec(&(
                &inference.llama_server,
                &inference.model,
                &inference.kore_image,
            ))
            .unwrap_or_default();
            let digest = blake3::hash(&identity).to_hex();
            format!("llama_cpp:{}", &digest.as_str()[..16])
        }
    }
}

fn model_display_name(inference: &InferenceConfig) -> String {
    match inference.backend {
        RuntimeBackend::Deterministic => "Legacy test backend (disabled)".into(),
        RuntimeBackend::LlamaCpp => inference
            .model
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .unwrap_or("Unavailable local model")
            .into(),
    }
}

fn inference_available(inference: &InferenceConfig) -> bool {
    match inference.backend {
        RuntimeBackend::Deterministic => false,
        RuntimeBackend::LlamaCpp => {
            inference
                .llama_server
                .as_deref()
                .is_some_and(|path| path.is_file())
                && inference
                    .model
                    .as_deref()
                    .is_some_and(|path| path.is_file())
                && inference
                    .kore_image
                    .as_deref()
                    .is_some_and(|path| path.is_file())
        }
    }
}

fn unconfigured_live_inference() -> InferenceConfig {
    InferenceConfig {
        backend: RuntimeBackend::LlamaCpp,
        kore_image: None,
        llama_server: None,
        model: None,
        extra_args: Vec::new(),
        context_tokens: 32_768,
        output_tokens: 512,
        threads: std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(u16::MAX as usize) as u16,
        slots: 3,
    }
}

fn migrate(value: Value) -> (AppSettings, bool) {
    if value["schema_version"].as_u64() == Some(u64::from(SETTINGS_SCHEMA_VERSION))
        && let Ok(mut settings) = serde_json::from_value::<AppSettings>(value.clone())
    {
        if settings.default_inference.backend != RuntimeBackend::LlamaCpp {
            settings.default_inference = unconfigured_live_inference();
            return (settings, true);
        }
        return (settings, false);
    }
    let mut settings = AppSettings::default();
    if let Ok(inference) =
        serde_json::from_value::<InferenceConfig>(value["default_inference"].clone())
    {
        if inference.backend == RuntimeBackend::LlamaCpp {
            settings.default_inference = inference;
        }
    }
    if let Some(font) = value["body_font"]
        .as_str()
        .or_else(|| value["font_family"].as_str())
    {
        settings.body_font = match font {
            "archivo" => BodyFont::Archivo,
            "jetbrains_mono" => BodyFont::JetbrainsMono,
            _ => BodyFont::DesignDefault,
        };
    }
    if let Some(show) = value["show_working"].as_bool() {
        settings.show_working = show;
    }
    if let Some(expanded) = value["working_expanded_default"].as_bool() {
        settings.working_expanded_default = expanded;
    }
    (settings, true)
}

fn preserve_corrupt(path: &Path) -> Result<(), SettingsError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup = path.with_extension(format!("corrupt-{timestamp}.json"));
    fs::rename(path, backup).map_err(io_error)
}

fn recover_interrupted_write(path: &Path) -> Result<(), SettingsError> {
    if path.exists() {
        return Ok(());
    }
    let backup = path.with_extension("json.bak");
    if backup.exists() {
        return fs::rename(backup, path).map_err(io_error);
    }
    let temporary = path.with_extension("json.tmp");
    if temporary.exists() {
        fs::rename(temporary, path).map_err(io_error)?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SettingsError> {
    let temporary = path.with_extension("json.tmp");
    let backup = path.with_extension("json.bak");
    let mut file = File::create(&temporary).map_err(io_error)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(io_error)?;
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup).map_err(io_error)?;
        }
        fs::rename(path, &backup).map_err(io_error)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(io_error(error));
    }
    if backup.exists() {
        fs::remove_file(backup).map_err(io_error)?;
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}

fn io_error(error: std::io::Error) -> SettingsError {
    SettingsError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_config(
        id: &str,
        workspace: &Path,
        state: &Path,
        inference: InferenceConfig,
    ) -> ProjectConfig {
        ProjectConfig {
            schema_version: crate::project::PROJECT_SCHEMA_VERSION,
            id: id.into(),
            name: id.into(),
            workspace: crate::project::WorkspaceConfig {
                root: workspace.into(),
                state: state.into(),
                access: crate::project::WorkspaceAccess::ReviewChanges,
                exclusions: Vec::new(),
            },
            inference,
            addons: Vec::new(),
            approvals: crate::project::ApprovalProfile {
                default_scope: "once".into(),
                confirm_external_writes: true,
                confirm_destructive: true,
                standing_policy_enabled: false,
            },
            retention: crate::project::RetentionConfig {
                transcript_days: 30,
                clean_kv_on_close: true,
                retain_raw_model_output: false,
            },
            resources: crate::project::ResourceLimits {
                memory_mib: 2_048,
                virtual_cpus: 2,
                prompt_seconds: 300,
                agent_limit: 4,
            },
            archived: false,
        }
    }

    #[test]
    fn settings_persist_migrate_and_preserve_corrupt_input() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        fs::write(
            &path,
            br#"{"schema_version":0,"font_family":"archivo","show_working":false}"#,
        )
        .unwrap();
        let store = SettingsStore::open(&path).unwrap();
        let settings = store.get().unwrap();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.body_font, BodyFont::Archivo);
        assert!(!settings.show_working);
        let projects = ProjectStore::open(temporary.path().join("projects")).unwrap();
        store
            .update(
                &json!({
                    "body_font":"jetbrains_mono",
                    "show_working":true,
                    "working_expanded_default":true
                }),
                &projects,
            )
            .unwrap();
        let settings = store.get().unwrap();
        assert_eq!(settings.body_font, BodyFont::JetbrainsMono);
        assert!(settings.show_working);
        assert!(settings.working_expanded_default);
        drop(store);
        assert_eq!(SettingsStore::open(&path).unwrap().get().unwrap(), settings);

        let mut with_secret = serde_json::to_value(&settings).unwrap();
        with_secret["credential"] = Value::String("must-not-survive".into());
        fs::write(&path, serde_json::to_vec_pretty(&with_secret).unwrap()).unwrap();
        SettingsStore::open(&path).unwrap();
        assert!(
            !fs::read_to_string(&path)
                .unwrap()
                .contains("must-not-survive")
        );

        fs::write(&path, b"not json and never a secret").unwrap();
        let recovered = SettingsStore::open(&path).unwrap();
        assert_eq!(recovered.get().unwrap(), AppSettings::default());
        assert!(
            temporary
                .path()
                .read_dir()
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("settings.corrupt-"))
        );
    }

    #[test]
    fn interrupted_atomic_write_recovers_the_last_complete_settings_file() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        let backup = path.with_extension("json.bak");
        let settings = AppSettings {
            show_working: false,
            ..AppSettings::default()
        };
        fs::write(&backup, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
        fs::write(path.with_extension("json.tmp"), b"partial").unwrap();

        let recovered = SettingsStore::open(&path).unwrap();

        assert_eq!(recovered.get().unwrap(), settings);
        assert!(path.is_file());
        assert!(!backup.exists());
    }

    #[test]
    fn activity_is_bounded_to_privacy_safe_summary_fields() {
        let temporary = tempfile::tempdir().unwrap();
        let transcripts = temporary.path().join("transcripts");
        fs::create_dir(&transcripts).unwrap();
        let records = [
            json!({
                "timestamp_unix_ms":10,"project_id":"project","session_id":"session",
                "kind":"session_opened","payload":{"credential":"SECRET"}
            }),
            json!({
                "timestamp_unix_ms":20,"project_id":"project","session_id":"session",
                "kind":"user_prompt","payload":{"text":"PRIVATE PROMPT"}
            }),
            json!({
                "timestamp_unix_ms":30,"project_id":"project","session_id":"session",
                "kind":"prompt_terminal","payload":{"status":"completed","raw":"MODEL OUTPUT"}
            }),
        ];
        let mut transcript = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        transcript.push('\n');
        fs::write(transcripts.join("session.jsonl"), transcript).unwrap();
        let projects = ProjectStore::open(temporary.path().join("projects")).unwrap();

        let activity = recent_activity(&transcripts, &projects).unwrap();

        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].prompts, 1);
        assert_eq!(activity[0].completed, 1);
        assert_eq!(activity[0].status, "completed");
        let visible = serde_json::to_string(&activity).unwrap();
        assert!(!visible.contains("PRIVATE PROMPT"));
        assert!(!visible.contains("MODEL OUTPUT"));
        assert!(!visible.contains("SECRET"));
    }

    #[test]
    fn activity_history_has_a_hard_summary_limit() {
        let temporary = tempfile::tempdir().unwrap();
        let transcripts = temporary.path().join("transcripts");
        fs::create_dir(&transcripts).unwrap();
        for index in 0..(ACTIVITY_LIMIT + 17) {
            let record = json!({
                "timestamp_unix_ms": index as u64 + 1,
                "project_id":"project",
                "session_id":format!("session-{index}"),
                "kind":"session_opened",
                "payload":{}
            });
            fs::write(
                transcripts.join(format!("session-{index}.jsonl")),
                format!("{record}\n"),
            )
            .unwrap();
        }
        let projects = ProjectStore::open(temporary.path().join("projects")).unwrap();

        let activity = recent_activity(&transcripts, &projects).unwrap();

        assert_eq!(activity.len(), ACTIVITY_LIMIT);
    }

    #[test]
    fn legacy_test_default_is_disabled_without_losing_preferences() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        let mut old = AppSettings::default();
        old.default_inference.backend = RuntimeBackend::Deterministic;
        old.body_font = BodyFont::Archivo;
        old.show_working = false;
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let store = SettingsStore::open(&path).unwrap();
        let projects = ProjectStore::open(temporary.path().join("projects")).unwrap();
        let migrated = store.get().unwrap();
        assert_eq!(migrated.default_inference.backend, RuntimeBackend::LlamaCpp);
        assert_eq!(migrated.body_font, BodyFont::Archivo);
        assert!(!migrated.show_working);
        assert_eq!(
            store.default_inference(&projects),
            Err(SettingsError::UnavailableModel)
        );
        let view = store.view(&projects).unwrap();
        assert!(
            view["models"]
                .as_array()
                .unwrap()
                .iter()
                .all(|model| model["backend"] == "llama_cpp" && model["status"] == "unavailable")
        );
        assert_eq!(
            store.update(&json!({"default_model_id":"deterministic"}), &projects),
            Err(SettingsError::UnavailableModel)
        );
        assert_eq!(SettingsStore::open(&path).unwrap().get().unwrap(), migrated);
    }

    #[test]
    fn application_default_applies_only_to_projects_created_after_the_change() {
        let temporary = tempfile::tempdir().unwrap();
        let projects = ProjectStore::open(temporary.path().join("projects")).unwrap();
        let existing_workspace = temporary.path().join("existing-workspace");
        let catalog_workspace = temporary.path().join("catalog-workspace");
        let new_workspace = temporary.path().join("new-workspace");
        for workspace in [&existing_workspace, &catalog_workspace, &new_workspace] {
            fs::create_dir(workspace).unwrap();
        }
        let mut original = unconfigured_live_inference();
        original.backend = RuntimeBackend::Deterministic;
        projects
            .save(project_config(
                "existing",
                &existing_workspace,
                &temporary.path().join("existing-state"),
                original.clone(),
            ))
            .unwrap();

        let runtime = temporary.path().join("llama-server");
        let model = temporary.path().join("model.gguf");
        fs::write(&runtime, b"runtime").unwrap();
        fs::write(&model, b"model").unwrap();
        let image = temporary.path().join("kore.iso");
        fs::write(&image, b"test image").unwrap();
        let mut local = unconfigured_live_inference();
        local.kore_image = Some(image);
        local.llama_server = Some(runtime);
        local.model = Some(model);
        projects
            .save(project_config(
                "local-model-catalog",
                &catalog_workspace,
                &temporary.path().join("catalog-state"),
                local.clone(),
            ))
            .unwrap();

        let settings_path = temporary.path().join("settings.json");
        let settings = SettingsStore::open(&settings_path).unwrap();
        settings
            .update(&json!({"default_model_id":model_id(&local)}), &projects)
            .unwrap();
        assert_eq!(
            projects.get("existing").unwrap().inference,
            original,
            "changing the application default must not rewrite an existing override"
        );

        let created = projects
            .create_from_workspace(
                &new_workspace,
                settings.default_inference(&projects).unwrap(),
            )
            .unwrap();
        assert_eq!(created.project.inference, local);
        drop(settings);
        assert_eq!(
            SettingsStore::open(&settings_path)
                .unwrap()
                .get()
                .unwrap()
                .default_inference,
            created.project.inference
        );
    }

    #[test]
    fn unavailable_model_is_visible_but_cannot_become_a_project_default() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("settings.json");
        let mut settings = AppSettings::default();
        settings.default_inference.backend = RuntimeBackend::LlamaCpp;
        settings.default_inference.llama_server = Some(temporary.path().join("missing-server"));
        settings.default_inference.model = Some(temporary.path().join("missing-model.gguf"));
        fs::write(&path, serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
        let store = SettingsStore::open(&path).unwrap();
        let projects = ProjectStore::open(temporary.path().join("projects")).unwrap();

        let view = store.view(&projects).unwrap();
        let selected = view["default_model_id"].as_str().unwrap();
        let option = view["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == selected)
            .unwrap();
        assert_eq!(option["status"], "unavailable");
        assert_eq!(
            store.default_inference(&projects),
            Err(SettingsError::UnavailableModel)
        );
        assert_eq!(
            store.update(&json!({"default_model_id":selected}), &projects),
            Err(SettingsError::UnavailableModel)
        );
        assert_eq!(
            store.update(&json!({"body_font":"comic_sans"}), &projects),
            Err(SettingsError::InvalidFont)
        );
    }
}
