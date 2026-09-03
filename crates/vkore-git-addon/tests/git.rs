use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use vkore_addon_sdk::*;

fn init_repository(path: &Path) {
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(path)
        .status()
        .unwrap();
    assert!(status.success());
}

fn agent() -> AgentRef {
    AgentRef {
        id: "worker".into(),
        generation: 2,
    }
}

fn prompt() -> PromptRef {
    PromptRef {
        session_id: "session".into(),
        prompt_id: "prompt".into(),
    }
}

fn approval(
    hash: &str,
    repository: &Path,
    tool: &str,
    arguments: BTreeMap<String, Value>,
) -> InvocationApproval {
    InvocationApproval {
        invocation: Invocation {
            protocol_version: INVOCATION_PROTOCOL_VERSION,
            invocation_id: format!("call-{tool}"),
            capability_id: format!("cap-{tool}"),
            agent: agent(),
            prompt: prompt(),
            addon_id: "git".into(),
            package_hash: hash.into(),
            tool: tool.into(),
            arguments,
            resources: [(
                "repository".into(),
                repository.canonicalize().unwrap().display().to_string(),
            )]
            .into_iter()
            .collect(),
        },
        ticket: InvocationTicket {
            key_id: "direct-test".into(),
            authentication: "unused".into(),
        },
    }
}

#[test]
fn manifest_exposes_review_before_commit_workflow() {
    let manifest = vkore_git_addon::manifest("a".repeat(64));
    manifest.validate().unwrap();
    let names: Vec<_> = manifest
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "status", "diff", "log", "show", "stage", "unstage", "commit"
        ]
    );
    assert_eq!(manifest.tools[0].risk, RiskClass::ReadOnly);
    assert_eq!(
        manifest.tools[4].minimum_mediation,
        MediationMode::UserConfirm
    );
    assert_eq!(
        manifest.tools[6].minimum_mediation,
        MediationMode::UserConfirm
    );
}

#[test]
fn stage_review_commit_and_history_work_on_an_exact_repository() {
    let temp = tempfile::tempdir().unwrap();
    init_repository(temp.path());
    fs::write(temp.path().join("hello.txt"), "hello\n").unwrap();
    let hash = "a".repeat(64);

    let staged = vkore_git_addon::handle(&approval(
        &hash,
        temp.path(),
        "stage",
        [("paths".into(), json!(["hello.txt"]))]
            .into_iter()
            .collect(),
    ))
    .unwrap();
    assert_eq!(staged.disclosed["staged"], json!(["hello.txt"]));

    let patch = vkore_git_addon::handle(&approval(
        &hash,
        temp.path(),
        "diff",
        [("staged".into(), json!(true))].into_iter().collect(),
    ))
    .unwrap();
    assert!(
        patch.disclosed["patch"]
            .as_str()
            .unwrap()
            .contains("+hello")
    );

    let committed = vkore_git_addon::handle(&approval(
        &hash,
        temp.path(),
        "commit",
        [
            ("message".into(), json!("add greeting")),
            ("author_name".into(), json!("vKore Agent")),
            ("author_email".into(), json!("agent@example.invalid")),
        ]
        .into_iter()
        .collect(),
    ))
    .unwrap();
    assert_eq!(committed.disclosed["commit"].as_str().unwrap().len(), 40);

    let history = vkore_git_addon::handle(&approval(
        &hash,
        temp.path(),
        "log",
        [("max_count".into(), json!(5))].into_iter().collect(),
    ))
    .unwrap();
    assert_eq!(history.disclosed["commits"][0]["subject"], "add greeting");
}

#[test]
fn repository_escape_and_option_like_revisions_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    init_repository(temp.path());
    let hash = "a".repeat(64);
    let escaped = approval(
        &hash,
        temp.path(),
        "stage",
        [("paths".into(), json!(["../secret"]))]
            .into_iter()
            .collect(),
    );
    assert!(vkore_git_addon::handle(&escaped).is_err());
    let revision = approval(
        &hash,
        temp.path(),
        "show",
        [("revision".into(), json!("--help"))].into_iter().collect(),
    );
    assert!(vkore_git_addon::handle(&revision).is_err());
}

#[test]
fn git_addon_runs_through_the_capability_checked_process_runtime() {
    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_vkore-git-addon"));
    let temp = tempfile::tempdir().unwrap();
    let repository = temp.path().join("repository");
    fs::create_dir(&repository).unwrap();
    init_repository(&repository);
    fs::write(repository.join("untracked.txt"), "content\n").unwrap();

    let package_root = temp.path().join("package");
    fs::create_dir(&package_root).unwrap();
    let installed = package_root.join(binary.file_name().unwrap());
    fs::copy(&binary, &installed).unwrap();
    let hash = blake3::hash(&fs::read(&installed).unwrap())
        .to_hex()
        .to_string();
    let package = AddonPackage {
        manifest: vkore_git_addon::manifest(&hash),
        root: package_root,
    };
    let mut call = approval(&hash, &repository, "status", BTreeMap::new());
    let issuer = TicketIssuer::new("test", [5; 32]);
    call.ticket = issuer.issue(&call.invocation).unwrap();
    let repository_resource = repository.canonicalize().unwrap().display().to_string();
    let capability = ToolCapability {
        schema_version: TOOL_CAPABILITY_SCHEMA_VERSION,
        id: "cap-status".into(),
        agent: agent(),
        prompt: prompt(),
        addon_id: "git".into(),
        addon_version: "0.1.0".into(),
        package_hash: hash,
        tool: "status".into(),
        domain: ExecutionDomain::Host,
        constraints: CallConstraints {
            arguments: BTreeMap::new(),
            allowed_resources: [(
                "repository".into(),
                [repository_resource].into_iter().collect(),
            )]
            .into_iter()
            .collect(),
            maximum_input_bytes: 65_536,
            maximum_output_bytes: 512 * 1024,
            disclosure_fields: ["status".into()].into_iter().collect(),
        },
        mediation: MediationMode::PerCall,
        expires_at_ms: u64::MAX,
        remaining_calls: 1,
        delegable: false,
    };
    let mut authority = AuthoritySet::default();
    authority.grant(capability);
    let validated = authority
        .validate_and_consume(&package.manifest, &call, &issuer, unix_time_ms(), None)
        .unwrap();
    let runtime = HostRuntime::new(temp.path().join("scratch"), RuntimeLimits::default()).unwrap();
    let result = runtime.invoke(&package, validated).unwrap();
    assert!(
        result.disclosed["status"]
            .as_str()
            .unwrap()
            .contains("untracked.txt")
    );
}
