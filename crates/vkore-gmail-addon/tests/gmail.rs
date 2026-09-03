use base64::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;
use vkore_addon_sdk::*;

fn agent() -> AgentRef {
    AgentRef {
        id: "worker".into(),
        generation: 4,
    }
}

fn prompt() -> PromptRef {
    PromptRef {
        session_id: "session".into(),
        prompt_id: "prompt".into(),
    }
}

fn approval(hash: &str, tool: &str, arguments: BTreeMap<String, Value>) -> InvocationApproval {
    let invocation = Invocation {
        protocol_version: INVOCATION_PROTOCOL_VERSION,
        invocation_id: format!("call-{tool}"),
        capability_id: format!("cap-{tool}"),
        agent: agent(),
        prompt: prompt(),
        addon_id: "gmail".into(),
        package_hash: hash.into(),
        tool: tool.into(),
        arguments,
        resources: [("connection".into(), "gmail:alice".into())]
            .into_iter()
            .collect(),
    };
    let issuer = TicketIssuer::new("test", [8; 32]);
    InvocationApproval {
        ticket: issuer.issue(&invocation).unwrap(),
        invocation,
    }
}

#[test]
fn manifest_exposes_bounded_read_write_and_destructive_tools() {
    let manifest = vkore_gmail_addon::manifest("a".repeat(64));
    manifest.validate().unwrap();
    assert!(matches!(
        manifest.entrypoint,
        AddonEntrypoint::BrokeredProcess { .. }
    ));
    assert_eq!(manifest.tools.len(), 5);
    assert_eq!(manifest.tools[2].name, "send");
    assert_eq!(
        manifest.tools[2].minimum_mediation,
        MediationMode::UserConfirm
    );
    assert_eq!(manifest.tools[4].risk, RiskClass::Destructive);
}

#[test]
fn request_builder_encodes_mail_and_rejects_header_injection() {
    let valid = approval(
        &"a".repeat(64),
        "send",
        [
            ("to".into(), json!(["bob@example.com"])),
            ("subject".into(), json!("Status")),
            ("body".into(), json!("Build is green.")),
        ]
        .into_iter()
        .collect(),
    );
    let request = vkore_gmail_addon::request(&valid).unwrap();
    assert_eq!(request.method, ServiceMethod::Post);
    assert_eq!(request.path, "/gmail/v1/users/me/messages/send");
    let raw = request.body.unwrap()["raw"].as_str().unwrap().to_owned();
    let message = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .unwrap();
    let message = String::from_utf8(message).unwrap();
    assert!(message.contains("To: bob@example.com\r\n"));
    assert!(message.ends_with("\r\n\r\nBuild is green."));

    let injected = approval(
        &"a".repeat(64),
        "send",
        [
            (
                "to".into(),
                json!(["bob@example.com\r\nBcc: eve@example.com"]),
            ),
            ("subject".into(), json!("Status")),
            ("body".into(), json!("No")),
        ]
        .into_iter()
        .collect(),
    );
    assert!(vkore_gmail_addon::request(&injected).is_err());
}

#[derive(Default)]
struct MockBroker {
    request: Mutex<Option<ServiceRequest>>,
}

impl ServiceBroker for MockBroker {
    fn execute(
        &self,
        request: &ServiceRequest,
        _maximum_response_bytes: usize,
    ) -> Result<ServiceResponse, ServiceBrokerError> {
        self.request.lock().unwrap().replace(request.clone());
        Ok(ServiceResponse {
            status: 200,
            body: json!({"messages":[{"id":"m1","threadId":"t1"}]}),
        })
    }
}

#[test]
fn brokered_process_executes_without_receiving_an_oauth_token() {
    let binary = std::path::PathBuf::from(env!("CARGO_BIN_EXE_vkore-gmail-addon"));
    let temp = tempfile::tempdir().unwrap();
    let package_root = temp.path().join("package");
    fs::create_dir(&package_root).unwrap();
    let installed = package_root.join(binary.file_name().unwrap());
    fs::copy(&binary, &installed).unwrap();
    let hash = blake3::hash(&fs::read(&installed).unwrap())
        .to_hex()
        .to_string();
    let package = AddonPackage {
        manifest: vkore_gmail_addon::manifest(&hash),
        root: package_root,
    };
    let call = approval(
        &hash,
        "search",
        [
            ("query".into(), json!("is:unread from:bob@example.com")),
            ("max_results".into(), json!(5)),
        ]
        .into_iter()
        .collect(),
    );
    let issuer = TicketIssuer::new("test", [8; 32]);
    let capability = ToolCapability {
        schema_version: TOOL_CAPABILITY_SCHEMA_VERSION,
        id: "cap-search".into(),
        agent: agent(),
        prompt: prompt(),
        addon_id: "gmail".into(),
        addon_version: "0.1.0".into(),
        package_hash: hash,
        tool: "search".into(),
        domain: ExecutionDomain::Host,
        constraints: CallConstraints {
            arguments: BTreeMap::new(),
            allowed_resources: [(
                "connection".into(),
                ["gmail:alice".into()].into_iter().collect(),
            )]
            .into_iter()
            .collect(),
            maximum_input_bytes: 65_536,
            maximum_output_bytes: 512 * 1024,
            disclosure_fields: ["status".into(), "response".into()].into_iter().collect(),
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
    let broker = MockBroker::default();
    let runtime = HostRuntime::new(
        temp.path().join("scratch"),
        RuntimeLimits {
            allowed_network_destinations: [vkore_gmail_addon::DESTINATION.into()]
                .into_iter()
                .collect(),
            ..RuntimeLimits::default()
        },
    )
    .unwrap();
    let result = runtime
        .invoke_brokered(&package, validated, &broker)
        .unwrap();
    assert_eq!(result.disclosed["status"], 200);
    assert_eq!(result.disclosed["response"]["messages"][0]["id"], "m1");
    let request = broker.request.lock().unwrap().clone().unwrap();
    assert_eq!(request.connection, "gmail:alice");
    assert_eq!(request.query["maxResults"], "5");
    let serialized = serde_json::to_string(&request).unwrap();
    assert!(!serialized.contains("oauth-token"));
    assert!(!serialized.contains("Authorization"));
}

#[test]
fn concrete_broker_requires_a_live_connection_before_network_access() {
    let call = approval(
        &"a".repeat(64),
        "search",
        [("query".into(), json!("is:unread"))].into_iter().collect(),
    );
    let request = vkore_gmail_addon::request(&call).unwrap();
    let broker = vkore_gmail_addon::GmailCredentialBroker::default();
    assert_eq!(
        broker.execute(&request, 4096),
        Err(ServiceBrokerError::UnknownConnection)
    );

    let mut broker = vkore_gmail_addon::GmailCredentialBroker::default();
    broker
        .connect(
            "gmail:alice",
            "oauth-token-never-serialized",
            [vkore_gmail_addon::SCOPE_SEND.into()],
        )
        .unwrap();
    assert!(matches!(
        broker.execute(&request, 4096),
        Err(ServiceBrokerError::InvalidRequest(_))
    ));

    let mut confused_deputy = request.clone();
    confused_deputy.method = ServiceMethod::Post;
    confused_deputy.path = "/gmail/v1/users/me/messages/send".into();
    confused_deputy.query.clear();
    confused_deputy.body = Some(json!({"raw":"forged"}));
    assert!(matches!(
        broker.execute(&confused_deputy, 4096),
        Err(ServiceBrokerError::InvalidRequest(_))
    ));
    assert!(broker.disconnect("gmail:alice"));
}
