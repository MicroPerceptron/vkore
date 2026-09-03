use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use vkore_addon_sdk::{
    ADDON_MANIFEST_VERSION, AddonEntrypoint, AddonManifest, ExecutionDomain,
    INVOCATION_PROTOCOL_VERSION, InvocationApproval, InvocationResult, MediationMode,
    ResourceEffect, ResultSensitivity, RiskClass, ToolManifest,
};

const MAX_GIT_OUTPUT: usize = 512 * 1024;

pub fn manifest(package_hash: impl Into<String>) -> AddonManifest {
    AddonManifest {
        manifest_version: ADDON_MANIFEST_VERSION,
        id: "git".into(),
        name: "Git".into(),
        version: "0.1.0".into(),
        package_hash: package_hash.into(),
        sdk_protocol: INVOCATION_PROTOCOL_VERSION,
        entrypoint: AddonEntrypoint::Process {
            executable: executable_name(),
            arguments: Vec::new(),
        },
        tools: vec![
            tool(
                "status",
                "Show branch and working-tree status for one exact repository.",
                empty_schema(),
                "inspect",
                RiskClass::ReadOnly,
                MediationMode::PerCall,
                &["status"],
            ),
            tool(
                "diff",
                "Show a bounded working-tree or staged patch, optionally for one path.",
                json!({"type":"object","properties":{"staged":{"type":"boolean"},"path":{"type":"string"},"context":{"type":"integer"}},"additionalProperties":false}),
                "inspect",
                RiskClass::ReadOnly,
                MediationMode::PerCall,
                &["patch"],
            ),
            tool(
                "log",
                "Return bounded structured commit history from one exact repository.",
                json!({"type":"object","properties":{"max_count":{"type":"integer"},"revision":{"type":"string"}},"additionalProperties":false}),
                "inspect",
                RiskClass::ReadOnly,
                MediationMode::PerCall,
                &["commits"],
            ),
            tool(
                "show",
                "Show one exact Git revision and optional repository-relative path.",
                json!({"type":"object","properties":{"revision":{"type":"string"},"path":{"type":"string"}},"required":["revision"],"additionalProperties":false}),
                "inspect",
                RiskClass::ReadOnly,
                MediationMode::PerCall,
                &["content"],
            ),
            tool(
                "stage",
                "Stage an explicit list of repository-relative paths.",
                paths_schema(),
                "stage",
                RiskClass::ExternalWrite,
                MediationMode::UserConfirm,
                &["staged"],
            ),
            tool(
                "unstage",
                "Remove an explicit list of paths from the index without changing working files.",
                paths_schema(),
                "unstage",
                RiskClass::ExternalWrite,
                MediationMode::UserConfirm,
                &["unstaged"],
            ),
            tool(
                "commit",
                "Commit the already-staged index with an explicit message and author identity; hooks and signing are disabled.",
                json!({"type":"object","properties":{"message":{"type":"string"},"author_name":{"type":"string"},"author_email":{"type":"string"}},"required":["message","author_name","author_email"],"additionalProperties":false}),
                "commit",
                RiskClass::ExternalWrite,
                MediationMode::UserConfirm,
                &["commit", "summary"],
            ),
        ],
    }
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    operation: &str,
    risk: RiskClass,
    minimum_mediation: MediationMode,
    result_fields: &[&str],
) -> ToolManifest {
    let properties: serde_json::Map<String, Value> = result_fields
        .iter()
        .map(|field| ((*field).into(), json!({})))
        .collect();
    ToolManifest {
        name: name.into(),
        description: description.into(),
        domain: ExecutionDomain::Host,
        input_schema,
        result_schema: json!({"type":"object","properties":properties}),
        effects: vec![ResourceEffect {
            resource_type: "git_repository".into(),
            operation: operation.into(),
            external: false,
            destructive: false,
        }],
        risk,
        minimum_mediation,
        required_connections: BTreeSet::new(),
        network_destinations: BTreeSet::new(),
        result_sensitivity: ResultSensitivity::Internal,
        timeout_ms: 10_000,
        maximum_input_bytes: 65_536,
        maximum_output_bytes: MAX_GIT_OUTPUT,
    }
}

fn empty_schema() -> Value {
    json!({"type":"object","properties":{},"additionalProperties":false})
}

fn paths_schema() -> Value {
    json!({"type":"object","properties":{"paths":{"type":"array"}},"required":["paths"],"additionalProperties":false})
}

pub fn handle(approval: &InvocationApproval) -> Result<InvocationResult, String> {
    let invocation = &approval.invocation;
    let repository = invocation
        .resources
        .get("repository")
        .ok_or_else(|| "a repository resource is required".to_owned())?;
    let repository = canonical_repository(Path::new(repository))?;
    match invocation.tool.as_str() {
        "status" => status(approval, &repository),
        "diff" => diff(approval, &repository),
        "log" => log(approval, &repository),
        "show" => show(approval, &repository),
        "stage" => stage(approval, &repository, false),
        "unstage" => stage(approval, &repository, true),
        "commit" => commit(approval, &repository),
        _ => Err("unknown Git tool".into()),
    }
}

fn status(approval: &InvocationApproval, repository: &Path) -> Result<InvocationResult, String> {
    ensure_no_arguments(approval)?;
    let output = run_git(
        repository,
        [
            "status",
            "--porcelain=v1",
            "--branch",
            "--untracked-files=all",
        ],
    )?;
    Ok(result(approval, json!({"status":output}), &["status"]))
}

fn diff(approval: &InvocationApproval, repository: &Path) -> Result<InvocationResult, String> {
    let arguments = &approval.invocation.arguments;
    let staged = optional_bool(arguments, "staged")?.unwrap_or(false);
    let context = optional_u64(arguments, "context")?.unwrap_or(3);
    if context > 20 {
        return Err("context must be at most 20".into());
    }
    let path = optional_path(arguments, "path")?;
    let mut command = vec![
        "diff".to_owned(),
        "--no-ext-diff".into(),
        "--no-textconv".into(),
        format!("--unified={context}"),
    ];
    if staged {
        command.push("--cached".into());
    }
    if let Some(path) = path {
        command.push("--".into());
        command.push(path);
    }
    let output = run_git_owned(repository, &command)?;
    Ok(result(approval, json!({"patch":output}), &["patch"]))
}

fn log(approval: &InvocationApproval, repository: &Path) -> Result<InvocationResult, String> {
    let arguments = &approval.invocation.arguments;
    let maximum = optional_u64(arguments, "max_count")?.unwrap_or(20);
    if !(1..=100).contains(&maximum) {
        return Err("max_count must be between 1 and 100".into());
    }
    let revision = optional_revision(arguments, "revision")?.unwrap_or_else(|| "HEAD".into());
    let command = vec![
        "log".into(),
        format!("--max-count={maximum}"),
        "--date=iso-strict".into(),
        "--pretty=format:%H%x1f%h%x1f%an%x1f%ae%x1f%aI%x1f%s%x1e".into(),
        revision,
    ];
    let output = run_git_owned(repository, &command)?;
    let commits: Vec<_> = output
        .split('\u{1e}')
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let fields: Vec<_> = record.trim_start_matches('\n').split('\u{1f}').collect();
            json!({
                "id": fields.first().copied().unwrap_or_default(),
                "short_id": fields.get(1).copied().unwrap_or_default(),
                "author_name": fields.get(2).copied().unwrap_or_default(),
                "author_email": fields.get(3).copied().unwrap_or_default(),
                "authored_at": fields.get(4).copied().unwrap_or_default(),
                "subject": fields.get(5).copied().unwrap_or_default(),
            })
        })
        .collect();
    Ok(result(approval, json!({"commits":commits}), &["commits"]))
}

fn show(approval: &InvocationApproval, repository: &Path) -> Result<InvocationResult, String> {
    let arguments = &approval.invocation.arguments;
    let revision = required_revision(arguments, "revision")?;
    let path = optional_path(arguments, "path")?;
    let mut command = vec![
        "show".into(),
        "--no-ext-diff".into(),
        "--no-textconv".into(),
        "--format=fuller".into(),
        "--stat".into(),
        "--patch".into(),
        revision,
    ];
    if let Some(path) = path {
        command.push("--".into());
        command.push(path);
    }
    let output = run_git_owned(repository, &command)?;
    Ok(result(approval, json!({"content":output}), &["content"]))
}

fn stage(
    approval: &InvocationApproval,
    repository: &Path,
    unstage: bool,
) -> Result<InvocationResult, String> {
    let paths = required_paths(&approval.invocation.arguments)?;
    let mut command = if unstage {
        vec!["restore".into(), "--staged".into(), "--".into()]
    } else {
        vec!["add".into(), "--".into()]
    };
    command.extend(paths.iter().cloned());
    run_git_owned(repository, &command)?;
    let field = if unstage { "unstaged" } else { "staged" };
    Ok(result(approval, json!({field:paths}), &[field]))
}

fn commit(approval: &InvocationApproval, repository: &Path) -> Result<InvocationResult, String> {
    let arguments = &approval.invocation.arguments;
    let message = required_string(arguments, "message", 16 * 1024)?;
    let author_name = required_header(arguments, "author_name", 256)?;
    let author_email = required_header(arguments, "author_email", 320)?;
    if !author_email.contains('@') {
        return Err("author_email is invalid".into());
    }
    let command = vec![
        "-c".into(),
        format!("user.name={author_name}"),
        "-c".into(),
        format!("user.email={author_email}"),
        "-c".into(),
        "commit.gpgSign=false".into(),
        "commit".into(),
        "--no-verify".into(),
        "--no-gpg-sign".into(),
        "-m".into(),
        message.into(),
    ];
    let summary = run_git_owned(repository, &command)?;
    let id = run_git(repository, ["rev-parse", "HEAD"])?
        .trim()
        .to_owned();
    Ok(result(
        approval,
        json!({"commit":id,"summary":summary}),
        &["commit", "summary"],
    ))
}

fn result(approval: &InvocationApproval, value: Value, fields: &[&str]) -> InvocationResult {
    InvocationResult::from_addon(
        approval,
        ResultSensitivity::Internal,
        value,
        fields.iter().map(|field| (*field).to_owned()),
    )
}

fn canonical_repository(path: &Path) -> Result<PathBuf, String> {
    let repository = path
        .canonicalize()
        .map_err(|error| format!("repository is unavailable: {error}"))?;
    if !repository.is_dir() {
        return Err("repository is not a directory".into());
    }
    let top = run_git(&repository, ["rev-parse", "--show-toplevel"])?;
    let top = Path::new(top.trim())
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if top != repository {
        return Err("repository resource must name the exact Git worktree root".into());
    }
    Ok(repository)
}

fn run_git<'a>(
    repository: &Path,
    arguments: impl IntoIterator<Item = &'a str>,
) -> Result<String, String> {
    let arguments: Vec<String> = arguments.into_iter().map(str::to_owned).collect();
    run_git_owned(repository, &arguments)
}

fn run_git_owned(repository: &Path, arguments: &[String]) -> Result<String, String> {
    let mut command = Command::new(git_executable());
    command
        .env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        // An empty override disables the global config on every platform.
        // Git for Windows does not accept the Win32 `NUL` device here as a
        // config-file path, even though it works as a shell redirection.
        .env("GIT_CONFIG_GLOBAL", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .arg("--no-pager")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg(format!("safe.directory={}", repository.display()))
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let mut stdout = child.stdout.take().ok_or("Git stdout unavailable")?;
    let mut stderr = child.stderr.take().ok_or("Git stderr unavailable")?;
    let stdout_reader = thread::spawn(move || read_bounded(&mut stdout));
    let stderr_reader = thread::spawn(move || read_bounded(&mut stderr));
    let status = child.wait().map_err(|error| error.to_string())?;
    let stdout = stdout_reader
        .join()
        .map_err(|_| "Git stdout reader failed")??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "Git stderr reader failed")??;
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(format!("Git refused the operation: {}", detail.trim()));
    }
    String::from_utf8(stdout).map_err(|_| "Git returned non-UTF-8 output".into())
}

fn read_bounded(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_GIT_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_GIT_OUTPUT {
        return Err("Git output exceeded the add-on bound".into());
    }
    Ok(bytes)
}

fn ensure_no_arguments(approval: &InvocationApproval) -> Result<(), String> {
    if approval.invocation.arguments.is_empty() {
        Ok(())
    } else {
        Err("status takes no arguments".into())
    }
}

fn required_string<'a>(
    arguments: &'a BTreeMap<String, Value>,
    name: &str,
    maximum: usize,
) -> Result<&'a str, String> {
    let value = arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))?;
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        return Err(format!("{name} is invalid"));
    }
    Ok(value)
}

fn required_header<'a>(
    arguments: &'a BTreeMap<String, Value>,
    name: &str,
    maximum: usize,
) -> Result<&'a str, String> {
    let value = required_string(arguments, name, maximum)?;
    if value.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err(format!("{name} contains a line break"));
    }
    Ok(value)
}

fn optional_bool(arguments: &BTreeMap<String, Value>, name: &str) -> Result<Option<bool>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("{name} must be boolean"))
        })
        .transpose()
}

fn optional_u64(arguments: &BTreeMap<String, Value>, name: &str) -> Result<Option<u64>, String> {
    arguments
        .get(name)
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| format!("{name} must be an integer"))
        })
        .transpose()
}

fn optional_path(
    arguments: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<String>, String> {
    arguments
        .get(name)
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| format!("{name} must be a string"))?;
            validate_relative_path(value)
        })
        .transpose()
}

fn required_paths(arguments: &BTreeMap<String, Value>) -> Result<Vec<String>, String> {
    let values = arguments
        .get("paths")
        .and_then(Value::as_array)
        .ok_or("paths must be an array")?;
    if values.is_empty() || values.len() > 128 {
        return Err("paths must contain between 1 and 128 entries".into());
    }
    values
        .iter()
        .map(|value| {
            let value = value.as_str().ok_or("path must be a string")?;
            validate_relative_path(value)
        })
        .collect()
}

fn validate_relative_path(value: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 4_096 || value.contains('\0') {
        return Err("path is invalid".into());
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("path must remain within the repository".into());
    }
    Ok(value.into())
}

fn optional_revision(
    arguments: &BTreeMap<String, Value>,
    name: &str,
) -> Result<Option<String>, String> {
    arguments
        .get(name)
        .map(|_| required_revision(arguments, name))
        .transpose()
}

fn required_revision(arguments: &BTreeMap<String, Value>, name: &str) -> Result<String, String> {
    let value = required_string(arguments, name, 512)?;
    if value.starts_with('-') || value.bytes().any(|byte| matches!(byte, b'\r' | b'\n')) {
        return Err("revision is invalid".into());
    }
    Ok(value.into())
}

fn git_executable() -> PathBuf {
    let candidates = if cfg!(windows) {
        vec![
            PathBuf::from(r"C:\Program Files\Git\cmd\git.exe"),
            PathBuf::from(r"C:\Program Files\Git\bin\git.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Git\cmd\git.exe"),
            PathBuf::from("git.exe"),
        ]
    } else {
        vec![
            PathBuf::from("/usr/bin/git"),
            PathBuf::from("/usr/local/bin/git"),
            PathBuf::from("/opt/homebrew/bin/git"),
            PathBuf::from("git"),
        ]
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.components().count() == 1 || candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("git"))
}

fn executable_name() -> String {
    if cfg!(windows) {
        "vkore-git-addon.exe".into()
    } else {
        "vkore-git-addon".into()
    }
}
