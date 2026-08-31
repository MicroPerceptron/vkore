use serde_json::{Value, json};
use std::collections::BTreeSet;
use vkore_addon_sdk::{
    ADDON_MANIFEST_VERSION, AddonEntrypoint, AddonManifest, ExecutionDomain,
    INVOCATION_PROTOCOL_VERSION, InvocationApproval, InvocationResult, MediationMode,
    ResourceEffect, ResultSensitivity, RiskClass, ToolManifest,
};

pub fn manifest(package_hash: impl Into<String>) -> AddonManifest {
    AddonManifest {
        manifest_version: ADDON_MANIFEST_VERSION,
        id: "reference.host".into(),
        name: "vKore reference host add-on".into(),
        version: "0.1.0".into(),
        package_hash: package_hash.into(),
        sdk_protocol: INVOCATION_PROTOCOL_VERSION,
        entrypoint: AddonEntrypoint::Process {
            executable: executable_name(),
            arguments: Vec::new(),
        },
        tools: vec![ToolManifest {
            name: "echo".into(),
            description: "Return a bounded message from an isolated host add-on process.".into(),
            domain: ExecutionDomain::Host,
            input_schema: json!({"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false}),
            result_schema: json!({"type":"object","properties":{"message":{"type":"string"}}}),
            effects: vec![ResourceEffect {
                resource_type: "message".into(),
                operation: "read".into(),
                external: false,
                destructive: false,
            }],
            risk: RiskClass::ReadOnly,
            minimum_mediation: MediationMode::PerCall,
            required_connections: BTreeSet::new(),
            network_destinations: BTreeSet::new(),
            result_sensitivity: ResultSensitivity::Internal,
            timeout_ms: 2_000,
            maximum_input_bytes: 32_768,
            maximum_output_bytes: 32_768,
        }],
    }
}

pub fn guest_manifest(artifact_hash: impl Into<String>) -> AddonManifest {
    let artifact_hash = artifact_hash.into();
    AddonManifest {
        manifest_version: ADDON_MANIFEST_VERSION,
        id: "reference.guest".into(),
        name: "vKore reference guest add-on".into(),
        version: "0.1.0".into(),
        package_hash: artifact_hash.clone(),
        sdk_protocol: INVOCATION_PROTOCOL_VERSION,
        entrypoint: AddonEntrypoint::GuestExecutable {
            artifact: artifact_hash,
        },
        tools: vec![ToolManifest {
            name: "list_workspace".into(),
            description: "List names in the agent's capability-scoped Kore workspace.".into(),
            domain: ExecutionDomain::Guest,
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
            result_schema: json!({"type":"object","properties":{"entries":{"type":"array"}}}),
            effects: vec![ResourceEffect {
                resource_type: "workspace_path".into(),
                operation: "list".into(),
                external: false,
                destructive: false,
            }],
            risk: RiskClass::ReadOnly,
            minimum_mediation: MediationMode::PerCall,
            required_connections: BTreeSet::new(),
            network_destinations: BTreeSet::new(),
            result_sensitivity: ResultSensitivity::Internal,
            timeout_ms: 2_000,
            maximum_input_bytes: 8_192,
            maximum_output_bytes: 65_536,
        }],
    }
}

pub fn handle(approval: &InvocationApproval) -> Result<InvocationResult, &'static str> {
    if approval.invocation.tool != "echo" {
        return Err("unknown tool");
    }
    let message = approval
        .invocation
        .arguments
        .get("message")
        .and_then(Value::as_str)
        .ok_or("message must be a string")?;
    if message.len() > 4_096 {
        return Err("message too large");
    }
    Ok(InvocationResult::from_addon(
        approval,
        ResultSensitivity::Internal,
        json!({"message": message}),
        ["message".to_owned()],
    ))
}

fn executable_name() -> String {
    if cfg!(windows) {
        "vkore-reference-addon.exe".into()
    } else {
        "vkore-reference-addon".into()
    }
}
