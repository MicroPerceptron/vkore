use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use vkore_addon_sdk::*;

#[test]
fn reference_addon_runs_out_of_process_with_bounded_disclosure() {
    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_vkore-reference-addon"));
    let temp = tempfile::tempdir().unwrap();
    let package_root = temp.path().join("package");
    let scratch = temp.path().join("scratch");
    fs::create_dir(&package_root).unwrap();
    let installed = package_root.join(binary.file_name().unwrap());
    fs::copy(&binary, &installed).unwrap();
    let hash = blake3::hash(&fs::read(&installed).unwrap())
        .to_hex()
        .to_string();
    let package = AddonPackage {
        manifest: vkore_reference_addon::manifest(&hash),
        root: package_root,
    };
    package.manifest.validate().unwrap();

    let agent = AgentRef {
        id: "worker".into(),
        generation: 3,
    };
    let prompt = PromptRef {
        session_id: "session".into(),
        prompt_id: "prompt".into(),
    };
    let invocation = Invocation {
        protocol_version: INVOCATION_PROTOCOL_VERSION,
        invocation_id: "call-1".into(),
        capability_id: "cap-1".into(),
        agent: agent.clone(),
        prompt: prompt.clone(),
        addon_id: "reference.host".into(),
        package_hash: hash.clone(),
        tool: "echo".into(),
        arguments: [("message".into(), json!("hello from vKore"))]
            .into_iter()
            .collect(),
        resources: BTreeMap::new(),
    };
    let issuer = TicketIssuer::new("test", [7; 32]);
    let approval = InvocationApproval {
        ticket: issuer.issue(&invocation).unwrap(),
        invocation,
    };
    let capability = ToolCapability {
        schema_version: 1,
        id: "cap-1".into(),
        agent,
        prompt,
        addon_id: "reference.host".into(),
        addon_version: "0.1.0".into(),
        package_hash: hash,
        tool: "echo".into(),
        domain: ExecutionDomain::Host,
        constraints: CallConstraints {
            arguments: BTreeMap::new(),
            allowed_resources: BTreeMap::new(),
            maximum_input_bytes: 32_768,
            maximum_output_bytes: 32_768,
            disclosure_fields: ["message".into()].into_iter().collect(),
        },
        mediation: MediationMode::PerCall,
        expires_at_ms: u64::MAX,
        remaining_calls: 1,
        delegable: false,
    };
    let mut authority = AuthoritySet::default();
    authority.grant(capability);
    let validated = authority
        .validate_and_consume(&package.manifest, &approval, &issuer, unix_time_ms(), None)
        .unwrap();
    let runtime = HostRuntime::new(scratch, RuntimeLimits::default()).unwrap();
    let result = runtime.invoke(&package, validated).unwrap();
    assert_eq!(result.disclosed, json!({"message":"hello from vKore"}));
}

#[test]
fn guest_reference_tool_is_never_host_executable() {
    let package = vkore_reference_addon::guest_manifest("b".repeat(64));
    package.validate().unwrap();
    assert_eq!(package.tools[0].domain, ExecutionDomain::Guest);
}
