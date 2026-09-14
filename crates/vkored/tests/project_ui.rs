use hostlinkd::{
    ApprovalProfile, BackendHarness, DeterministicBackend, DeterministicGuest, HarnessHttpServer,
    HarnessService, InferenceConfig, ProjectConfig, ProjectError, ProjectStore, ResourceLimits,
    RetentionConfig, RuntimeBackend, UiApprovalService, WorkspaceAccess, WorkspaceConfig,
};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn config(root: &Path, state: &Path) -> ProjectConfig {
    ProjectConfig {
        schema_version: 1,
        id: "secure-project".into(),
        name: "Secure project".into(),
        workspace: WorkspaceConfig {
            root: root.into(),
            state: state.into(),
            access: WorkspaceAccess::ReadOnly,
            exclusions: vec![".git/objects".into()],
        },
        inference: InferenceConfig {
            backend: RuntimeBackend::Deterministic,
            kore_image: None,
            llama_server: None,
            model: None,
            extra_args: Vec::new(),
            context_tokens: 32_768,
            output_tokens: 512,
            threads: 2,
            slots: 3,
        },
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
    }
}

#[test]
fn real_only_runtime_refuses_deterministic_projects_before_starting_a_backend() {
    use hostlinkd::{
        EventKind, KoreVmRuntime, KoreVmRuntimeConfig, PromptRequest, PromptStatus, SessionState,
        SessionStatus, TaskRuntime,
    };

    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let projects = temporary.path().join("projects");
    ProjectStore::open(&projects)
        .unwrap()
        .save(config(
            &workspace,
            &temporary.path().join("workspace-state"),
        ))
        .unwrap();
    let mut runtime = KoreVmRuntime::new(KoreVmRuntimeConfig {
        qemu_executable: "qemu-system-x86_64".into(),
        qemu_arguments: Vec::new(),
        project_root: projects,
        require_real: false, // Legacy flag cannot re-enable a test backend.
    })
    .unwrap();
    let session = SessionState {
        api_version: 1,
        project_id: "secure-project".into(),
        session_id: "real-only-session".into(),
        status: SessionStatus::Open,
        generation: 1,
        backend_incarnation: 1,
        last_event_sequence: 0,
        prompts: Default::default(),
    };
    let prompt = PromptRequest {
        api_version: 1,
        prompt_id: "real-only-prompt".into(),
        idempotency_key: "real-only-prompt".into(),
        text: "hello".into(),
    };
    let mut events = Vec::new();
    let result = runtime.run_prompt(
        &session,
        &prompt,
        &mut |kind, _, payload| events.push((kind, payload)),
        &|| false,
        &|| true,
    );
    assert_eq!(result, PromptStatus::Failed);
    let errors: Vec<_> = events.iter().filter(|(kind, _)| *kind == EventKind::Error).collect();
    assert_eq!(errors.len(), 1);
    assert!(events.iter().all(|(_, payload)| payload["stage"] != "model_server_starting"));
    assert!(
        errors[0].1["message"]
            .as_str()
            .unwrap()
            .contains("deterministic inference is forbidden")
    );
}

#[test]
fn project_persists_without_credentials_or_capabilities_and_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let state = temp.path().join("state");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("visible.txt"), b"bounded").unwrap();
    let store = ProjectStore::open(temp.path().join("projects")).unwrap();
    let saved = store.save(config(&workspace, &state)).unwrap();
    assert_eq!(
        saved.workspace.root.canonicalize().unwrap(),
        workspace.canonicalize().unwrap()
    );
    assert!(!saved.workspace.state.starts_with(&workspace));
    let json = std::fs::read_to_string(temp.path().join("projects/secure-project.json")).unwrap();
    assert!(!json.contains("credential"));
    assert!(!json.contains("oauth"));
    assert!(!json.contains("capability"));
    let reopened = ProjectStore::open(temp.path().join("projects")).unwrap();
    assert_eq!(reopened.get("secure-project").unwrap(), saved);
}

#[test]
fn legacy_workspace_quotas_are_removed_without_disabling_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    let projects = temporary.path().join("projects");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::create_dir(&projects).unwrap();
    let mut legacy = serde_json::to_value(config(
        &workspace,
        &temporary.path().join("workspace-state"),
    ))
    .unwrap();
    legacy["workspace"]["access"] = Value::String("review_changes".into());
    legacy["workspace"]["max_write_bytes"] = Value::from(0);
    legacy["workspace"]["max_write_files"] = Value::from(0);
    std::fs::write(
        projects.join("secure-project.json"),
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let store = ProjectStore::open(&projects).unwrap();
    let migrated = store.get("secure-project").unwrap();
    assert_eq!(migrated.workspace.access, WorkspaceAccess::ReviewChanges);
    let serialized = std::fs::read_to_string(projects.join("secure-project.json")).unwrap();
    assert!(!serialized.contains("max_write_bytes"));
    assert!(!serialized.contains("max_write_files"));
}

#[test]
fn folder_first_project_creation_derives_identity_defaults_and_reuses_the_workspace() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("My Workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = ProjectStore::open(temporary.path().join("projects")).unwrap();
    let inference = config(&workspace, &temporary.path().join("unused")).inference;

    let created = store
        .create_from_workspace(&workspace, inference.clone())
        .unwrap();
    assert!(created.created);
    assert_eq!(created.project.name, "My Workspace");
    assert!(created.project.id.starts_with("my-workspace-"));
    assert_eq!(
        created.project.workspace.access,
        WorkspaceAccess::AutomaticWriteback
    );
    assert!(!created.preflight.writeback_confirmation_required);
    assert_eq!(created.project.inference, inference);
    assert!(!created.project.workspace.state.starts_with(&workspace));
    assert!(created.preflight.ready);

    let existing = store
        .create_from_workspace(&workspace, created.project.inference.clone())
        .unwrap();
    assert!(!existing.created);
    assert_eq!(existing.project.id, created.project.id);
    assert_eq!(store.list().len(), 1);

    store.archive(&created.project.id, true).unwrap();
    let reopened = store
        .create_from_workspace(&workspace, created.project.inference.clone())
        .unwrap();
    assert!(!reopened.created);
    assert!(!reopened.project.archived);

    let not_a_folder = temporary.path().join("file.txt");
    std::fs::write(&not_a_folder, b"not a workspace").unwrap();
    assert_eq!(
        store.create_from_workspace(&not_a_folder, created.project.inference.clone()),
        Err(ProjectError::RootNotDirectory)
    );

    let json = std::fs::read_to_string(
        temporary
            .path()
            .join("projects")
            .join(format!("{}.json", created.project.id)),
    )
    .unwrap();
    assert!(!json.contains("max_write"));
}

#[test]
fn project_preflight_rejects_state_inside_root_and_missing_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = ProjectStore::open(temp.path().join("projects")).unwrap();
    let mut empty_state = config(&workspace, Path::new(""));
    assert_eq!(
        store.save(empty_state.clone()),
        Err(ProjectError::InvalidWorkspaceState)
    );
    empty_state.workspace.state = temp.path().join("state");
    let mut value = config(&workspace, &workspace.join("state"));
    assert_eq!(
        store.save(value.clone()),
        Err(ProjectError::StateInsideWorkspace)
    );
    value.workspace.state = temp.path().join("state");
    value.workspace.access = WorkspaceAccess::AutomaticWriteback;
    value.inference.backend = RuntimeBackend::LlamaCpp;
    assert_eq!(store.save(value), Err(ProjectError::InvalidRuntime));

    let runtime = temp.path().join("llama-server");
    let model_directory = temp.path().join("model.gguf");
    std::fs::write(&runtime, b"runtime").unwrap();
    std::fs::create_dir(&model_directory).unwrap();
    let mut invalid_artifact = config(&workspace, &temp.path().join("artifact-state"));
    invalid_artifact.inference.backend = RuntimeBackend::LlamaCpp;
    invalid_artifact.inference.llama_server = Some(runtime);
    invalid_artifact.inference.model = Some(model_directory);
    assert_eq!(
        store.save(invalid_artifact),
        Err(ProjectError::MissingArtifact("model".into()))
    );

    let credential_root = temp.path().join(".ssh");
    std::fs::create_dir(&credential_root).unwrap();
    let risky = config(&credential_root, &temp.path().join("credential-state"));
    assert!(
        hostlinkd::project::preflight(&risky)
            .unwrap()
            .broad_root_warning
            .unwrap()
            .contains("credentials")
    );
    let error = store
        .create_from_workspace(&credential_root, risky.inference)
        .unwrap_err()
        .to_string();
    assert!(error.contains("too broad or sensitive"), "{error}");
    assert!(error.contains("credentials"), "{error}");
}

#[test]
fn daemon_recovery_retires_persisted_grants_before_serving_ui_state() {
    let temp = tempfile::tempdir().unwrap();
    let service = UiApprovalService::open(temp.path()).unwrap();
    let request = service
        .request("project", "session", "reference.host", "echo")
        .unwrap();
    let id = request["request"]["id"].as_str().unwrap();
    service.decide(id, true, "once", "ui").unwrap();
    assert_eq!(service.state().unwrap()["grants"][0]["committed"], true);
    drop(service);
    let recovered = UiApprovalService::open(temp.path()).unwrap();
    assert!(
        recovered.state().unwrap()["grants"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn ui_api_discovers_native_shell_persists_project_and_blocks_cross_origin_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    // Catalog fixtures only: this API test does not run inference or a guest.
    let mut settings = hostlinkd::AppSettings::default();
    for (name, destination) in [
        ("llama-server", &mut settings.default_inference.llama_server),
        ("model.gguf", &mut settings.default_inference.model),
        ("kore.iso", &mut settings.default_inference.kore_image),
    ] {
        let path = temp.path().join(name);
        std::fs::write(&path, b"catalog test fixture").unwrap();
        *destination = Some(path);
    }
    std::fs::write(
        temp.path().join("settings.json"),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    let service = HarnessService::start(
        BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
        temp.path().join("transcripts"),
    );
    let server = HarnessHttpServer::bind_with_state(
        "127.0.0.1:0".parse().unwrap(),
        service.handle(),
        temp.path().join("projects"),
    )
    .unwrap();
    let address = server.local_addr();

    let discovery = transact(&server, address, "GET", "/", "", &[]);
    assert!(discovery.starts_with("HTTP/1.1 200"));
    assert!(discovery.contains("application/json"));
    assert!(discovery.contains("vkore-ui"));
    assert!(discovery.contains("axiom"));
    assert!(!discovery.contains("text/html"));
    assert!(!discovery.contains("launchToken"));
    assert!(!discovery.contains("Authorization"));

    let discovery_with_query = transact(&server, address, "GET", "/?demo=1", "", &[]);
    assert!(discovery_with_query.starts_with("HTTP/1.1 200"));
    assert!(discovery_with_query.contains("vkore-ui"));
    assert!(!discovery_with_query.contains("text/html"));

    let health = transact(&server, address, "GET", "/api/v1/health?check=1", "", &[]);
    assert!(health.starts_with("HTTP/1.1 200"));
    let health: Value = serde_json::from_str(health.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(health["ok"], true);

    let catalog = transact(&server, address, "GET", "/api/v1/catalog", "", &[]);
    let catalog: Value = serde_json::from_str(catalog.split_once("\r\n\r\n").unwrap().1).unwrap();
    let addons = catalog["addons"].as_array().unwrap();
    assert!(addons.iter().any(|addon| addon["id"] == "git"));
    assert!(addons.iter().any(|addon| addon["id"] == "gmail"));
    assert_eq!(
        addons.iter().find(|addon| addon["id"] == "gmail").unwrap()["network_destinations"][0],
        "gmail.googleapis.com:443"
    );

    let settings = transact(&server, address, "GET", "/api/v1/settings", "", &[]);
    let settings: Value = serde_json::from_str(settings.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(settings["default_inference"]["backend"], "llama_cpp");
    assert_eq!(settings["show_working"], true);
    assert!(
        settings["models"]
            .as_array()
            .unwrap()
            .iter()
            .all(|model| model["backend"] == "llama_cpp")
    );
    assert!(
        catalog["backends"]
            .as_array()
            .unwrap()
            .iter()
            .all(|backend| backend["real_inference"] == true)
    );

    let origin = format!("http://{address}");
    let changed = transact(
        &server,
        address,
        "POST",
        "/api/v1/settings",
        r#"{"body_font":"archivo","show_working":false}"#,
        &[("Origin", &origin), ("X-vKore-Client", "harness-ui-v1")],
    );
    let changed: Value = serde_json::from_str(changed.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(changed["body_font"], "archivo");
    assert_eq!(changed["show_working"], false);
    assert!(temp.path().join("settings.json").is_file());

    let folder_workspace = temp.path().join("folder-first");
    std::fs::create_dir(&folder_workspace).unwrap();
    let created = transact(
        &server,
        address,
        "POST",
        "/api/v1/projects/from-workspace",
        &serde_json::to_string(&serde_json::json!({"root":folder_workspace})).unwrap(),
        &[("Origin", &origin), ("X-vKore-Client", "harness-ui-v1")],
    );
    assert!(created.starts_with("HTTP/1.1 200"), "{created}");
    let created: Value = serde_json::from_str(created.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(created["created"], true);
    assert_eq!(created["project"]["name"], "folder-first");
    assert_eq!(
        created["project"]["workspace"]["access"],
        "automatic_writeback"
    );
    assert!(
        created["project"]["workspace"]
            .get("max_write_bytes")
            .is_none()
    );

    let body = serde_json::to_string(&config(&workspace, &temp.path().join("state"))).unwrap();
    let rejected = transact(
        &server,
        address,
        "POST",
        "/api/v1/projects",
        &body,
        &[
            ("Origin", "https://attacker.invalid"),
            ("X-vKore-Client", "harness-ui-v1"),
        ],
    );
    assert!(rejected.starts_with("HTTP/1.1 403"));

    let saved = transact(
        &server,
        address,
        "POST",
        "/api/v1/projects",
        &body,
        &[("Origin", &origin), ("X-vKore-Client", "harness-ui-v1")],
    );
    assert!(saved.starts_with("HTTP/1.1 200"), "{saved}");
    let payload = saved.split_once("\r\n\r\n").unwrap().1;
    let value: Value = serde_json::from_str(payload).unwrap();
    assert_eq!(value["id"], "secure-project");
    assert!(temp.path().join("projects/secure-project.json").exists());

    let requested = transact(
        &server,
        address,
        "POST",
        "/api/v1/projects/secure-project/approvals",
        r#"{"session_id":"ui","addon_id":"reference.host","tool":"echo"}"#,
        &[("Origin", &origin), ("X-vKore-Client", "harness-ui-v1")],
    );
    let payload: Value = serde_json::from_str(requested.split_once("\r\n\r\n").unwrap().1).unwrap();
    let request_id = payload["request"]["id"].as_str().unwrap();
    let decision_path = format!("/api/v1/approvals/{request_id}/decision");
    let approved = transact(
        &server,
        address,
        "POST",
        &decision_path,
        r#"{"allow":true,"lifetime":"once","ui_session":"ui"}"#,
        &[("Origin", &origin), ("X-vKore-Client", "harness-ui-v1")],
    );
    let payload: Value = serde_json::from_str(approved.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(payload["commit"]["committed"], true);
    assert_eq!(payload["grant"]["committed"], false);
    let policy = transact(&server, address, "GET", "/api/v1/approvals", "", &[]);
    let payload: Value = serde_json::from_str(policy.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(payload["grants"][0]["committed"], true);
    let grant_id = payload["grants"][0]["id"].as_str().unwrap();
    let revoke_path = format!("/api/v1/grants/{grant_id}/revoke");
    let revoked = transact(
        &server,
        address,
        "POST",
        &revoke_path,
        r#"{"ui_session":"ui"}"#,
        &[("Origin", &origin), ("X-vKore-Client", "harness-ui-v1")],
    );
    let payload: Value = serde_json::from_str(revoked.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(payload["commit"]["committed"], true);
    let policy = transact(&server, address, "GET", "/api/v1/approvals", "", &[]);
    let payload: Value = serde_json::from_str(policy.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert!(payload["grants"].as_array().unwrap().is_empty());
}

fn transact(
    server: &HarnessHttpServer,
    address: SocketAddr,
    method: &str,
    path: &str,
    body: &str,
    headers: &[(&'static str, &str)],
) -> String {
    let body = body.to_owned();
    let method = method.to_owned();
    let path = path.to_owned();
    let headers: Vec<_> = headers
        .iter()
        .map(|(name, value)| (*name, (*value).to_owned()))
        .collect();
    let (tx, rx) = mpsc::channel();
    let client = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n"
        )
        .unwrap();
        for (name, value) in headers {
            write!(stream, "{name}: {value}\r\n").unwrap();
        }
        write!(
            stream,
            "Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        tx.send(response).unwrap();
    });
    server.poll(Duration::from_secs(2)).unwrap();
    let response = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    client.join().unwrap();
    response
}
