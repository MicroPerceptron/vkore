use hostlinkd::{
    ApprovalProfile, BackendHarness, DeterministicBackend, DeterministicGuest, HarnessHttpServer,
    HarnessService, InferenceConfig, ProjectConfig, ProjectStore, ResourceLimits, RetentionConfig,
    RuntimeBackend, UiApprovalService, WorkspaceAccess, WorkspaceConfig,
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
            max_write_bytes: 0,
            max_write_files: 0,
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
fn project_preflight_rejects_state_inside_root_missing_artifacts_and_unbounded_writeback() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = ProjectStore::open(temp.path().join("projects")).unwrap();
    let mut value = config(&workspace, &workspace.join("state"));
    assert!(
        store
            .save(value.clone())
            .unwrap_err()
            .to_string()
            .contains("StateInsideWorkspace")
    );
    value.workspace.state = temp.path().join("state");
    value.workspace.access = WorkspaceAccess::AutomaticWriteback;
    assert!(
        store
            .save(value.clone())
            .unwrap_err()
            .to_string()
            .contains("UnsafeWritebackBounds")
    );
    value.workspace.max_write_bytes = 1024;
    value.workspace.max_write_files = 1;
    value.inference.backend = RuntimeBackend::LlamaCpp;
    assert!(
        store
            .save(value)
            .unwrap_err()
            .to_string()
            .contains("InvalidRuntime")
    );

    let runtime = temp.path().join("llama-server");
    let model_directory = temp.path().join("model.gguf");
    std::fs::write(&runtime, b"runtime").unwrap();
    std::fs::create_dir(&model_directory).unwrap();
    let mut invalid_artifact = config(&workspace, &temp.path().join("artifact-state"));
    invalid_artifact.inference.backend = RuntimeBackend::LlamaCpp;
    invalid_artifact.inference.llama_server = Some(runtime);
    invalid_artifact.inference.model = Some(model_directory);
    assert!(
        store
            .save(invalid_artifact)
            .unwrap_err()
            .to_string()
            .contains("MissingArtifact")
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

    let origin = format!("http://{address}");
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
