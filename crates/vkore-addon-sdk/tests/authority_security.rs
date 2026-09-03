use serde_json::{Value, json};
use std::collections::BTreeMap;
use vkore_addon_sdk::*;

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn agent(generation: u64) -> AgentRef {
    AgentRef {
        id: "worker-1".into(),
        generation,
    }
}

fn prompt() -> PromptRef {
    PromptRef {
        session_id: "session-1".into(),
        prompt_id: "prompt-1".into(),
    }
}

fn manifest(mediation: MediationMode) -> AddonManifest {
    AddonManifest {
        manifest_version: ADDON_MANIFEST_VERSION,
        id: "mail.test".into(),
        name: "Mail test".into(),
        version: "1.0.0".into(),
        package_hash: HASH.into(),
        sdk_protocol: INVOCATION_PROTOCOL_VERSION,
        entrypoint: AddonEntrypoint::Process {
            executable: "mail-test".into(),
            arguments: vec![],
        },
        tools: vec![ToolManifest {
            name: "send".into(),
            description: "Send one constrained message.".into(),
            domain: ExecutionDomain::Host,
            input_schema: json!({
                "type":"object",
                "properties":{"recipient":{"type":"string"},"body":{"type":"string"}},
                "required":["recipient","body"],
                "additionalProperties":false
            }),
            result_schema: json!({"type":"object"}),
            effects: vec![ResourceEffect {
                resource_type: "mailbox".into(),
                operation: "send".into(),
                external: true,
                destructive: false,
            }],
            risk: RiskClass::ExternalWrite,
            minimum_mediation: mediation,
            required_connections: ["account:alice".to_owned()].into_iter().collect(),
            network_destinations: ["api.mail.test:443".to_owned()].into_iter().collect(),
            result_sensitivity: ResultSensitivity::Sensitive,
            timeout_ms: 1_000,
            maximum_input_bytes: 4_096,
            maximum_output_bytes: 4_096,
        }],
    }
}

fn capability(mediation: MediationMode) -> ToolCapability {
    ToolCapability {
        schema_version: 1,
        id: "cap-1".into(),
        agent: agent(7),
        prompt: prompt(),
        addon_id: "mail.test".into(),
        addon_version: "1.0.0".into(),
        package_hash: HASH.into(),
        tool: "send".into(),
        domain: ExecutionDomain::Host,
        constraints: CallConstraints {
            arguments: [
                (
                    "recipient".into(),
                    ArgConstraint::Exact(json!("bob@example.com")),
                ),
                ("body".into(), ArgConstraint::StringPrefix("Meeting".into())),
                ("attachment".into(), ArgConstraint::Absent),
            ]
            .into_iter()
            .collect(),
            allowed_resources: [("account".into(), ["alice".into()].into_iter().collect())]
                .into_iter()
                .collect(),
            maximum_input_bytes: 1_024,
            maximum_output_bytes: 1_024,
            disclosure_fields: ["message_id".into()].into_iter().collect(),
        },
        mediation,
        expires_at_ms: 20_000,
        remaining_calls: 1,
        delegable: false,
    }
}

fn invocation(id: &str) -> Invocation {
    Invocation {
        protocol_version: INVOCATION_PROTOCOL_VERSION,
        invocation_id: id.into(),
        capability_id: "cap-1".into(),
        agent: agent(7),
        prompt: prompt(),
        addon_id: "mail.test".into(),
        package_hash: HASH.into(),
        tool: "send".into(),
        arguments: [
            ("recipient".into(), json!("bob@example.com")),
            ("body".into(), json!("Meeting is at 3 PM")),
        ]
        .into_iter()
        .collect(),
        resources: [("account".into(), "alice".into())].into_iter().collect(),
    }
}

fn approval(issuer: &TicketIssuer, invocation: Invocation) -> InvocationApproval {
    let ticket = issuer.issue(&invocation).unwrap();
    InvocationApproval { invocation, ticket }
}

#[test]
fn granted_catalog_is_capability_filtered_and_revocable() {
    let package = AddonPackage {
        manifest: manifest(MediationMode::PerCall),
        root: ".".into(),
    };
    let mut registry = AddonRegistry::default();
    registry.register(package).unwrap();
    let mut authority = AuthoritySet::default();
    assert!(
        registry
            .worker_catalog(&authority, &agent(7), 10_000)
            .is_empty()
    );
    authority.grant(capability(MediationMode::PerCall));
    let tools = registry.worker_catalog(&authority, &agent(7), 10_000);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].qualified_name, "mail.test.send");
    assert!(authority.revoke("cap-1"));
    assert!(
        registry
            .worker_catalog(&authority, &agent(7), 10_000)
            .is_empty()
    );
}

#[test]
fn exact_arguments_generation_resources_and_replay_are_enforced() {
    let issuer = TicketIssuer::new("host-1", [9; 32]);
    let package = manifest(MediationMode::PerCall);

    let mut changed = invocation("changed");
    changed
        .arguments
        .insert("recipient".into(), json!("alice@example.com"));
    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    assert_eq!(
        authority.validate_and_consume(
            &package,
            &approval(&issuer, changed),
            &issuer,
            10_000,
            None
        ),
        Err(AuthorityError::ArgumentDenied("recipient".into()))
    );

    let mut stale = invocation("stale");
    stale.agent.generation = 8;
    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    assert_eq!(
        authority.validate_and_consume(&package, &approval(&issuer, stale), &issuer, 10_000, None),
        Err(AuthorityError::StaleAgent)
    );

    let mut wrong_resource = invocation("resource");
    wrong_resource
        .resources
        .insert("account".into(), "mallory".into());
    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    assert_eq!(
        authority.validate_and_consume(
            &package,
            &approval(&issuer, wrong_resource),
            &issuer,
            10_000,
            None
        ),
        Err(AuthorityError::ResourceDenied("account".into()))
    );

    let call = approval(&issuer, invocation("valid"));
    let mut authority = AuthoritySet::default();
    let mut cap = capability(MediationMode::PerCall);
    cap.remaining_calls = 2;
    authority.grant(cap);
    assert!(
        authority
            .validate_and_consume(&package, &call, &issuer, 10_000, None)
            .is_ok()
    );
    assert_eq!(
        authority.validate_and_consume(&package, &call, &issuer, 10_000, None),
        Err(AuthorityError::Replayed)
    );
}

#[test]
fn forged_ticket_tool_extra_argument_expiration_and_revocation_fail_closed() {
    let issuer = TicketIssuer::new("host-1", [3; 32]);
    let package = manifest(MediationMode::PerCall);

    let mut forged = approval(&issuer, invocation("forged-ticket"));
    forged.invocation.tool = "delete".into();
    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    assert_eq!(
        authority.validate_and_consume(&package, &forged, &issuer, 1, None),
        Err(AuthorityError::InvalidTicket)
    );

    let mut extra = invocation("extra");
    extra
        .arguments
        .insert("system_prompt_says_approved".into(), Value::Bool(true));
    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    assert_eq!(
        authority.validate_and_consume(&package, &approval(&issuer, extra), &issuer, 1, None),
        Err(AuthorityError::ArgumentDenied(
            "system_prompt_says_approved".into()
        ))
    );

    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    assert_eq!(
        authority.validate_and_consume(
            &package,
            &approval(&issuer, invocation("expired")),
            &issuer,
            20_001,
            None
        ),
        Err(AuthorityError::Expired)
    );

    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    authority.revoke("cap-1");
    assert_eq!(
        authority.validate_and_consume(
            &package,
            &approval(&issuer, invocation("revoked")),
            &issuer,
            1,
            None
        ),
        Err(AuthorityError::Revoked)
    );
}

#[test]
fn escalation_does_not_consume_until_trusted_mediation_completes() {
    let issuer = TicketIssuer::new("host-1", [4; 32]);
    let package = manifest(MediationMode::UserConfirm);
    let call = approval(&issuer, invocation("confirm"));
    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::UserConfirm));
    assert_eq!(
        authority.validate_and_consume(&package, &call, &issuer, 1, None),
        Err(AuthorityError::MediationRequired(Escalation::UserConfirm))
    );
    assert!(
        authority
            .validate_and_consume(
                &package,
                &call,
                &issuer,
                1,
                Some(MediationMode::UserConfirm)
            )
            .is_ok()
    );
}

#[test]
fn result_disclosure_is_independent_and_field_scoped() {
    let approval = InvocationApproval {
        invocation: invocation("result"),
        ticket: InvocationTicket {
            key_id: "unused".into(),
            authentication: "unused".into(),
        },
    };
    let result = InvocationResult::from_addon(
        &approval,
        ResultSensitivity::Sensitive,
        json!({"message_id":"42","body":"secret"}),
        ["message_id".to_owned(), "body".to_owned()],
    );
    let grant = DisclosureGrant {
        handle_id: result.handle.id.clone(),
        agent: agent(7),
        fields: ["message_id".into()].into_iter().collect(),
    };
    assert_eq!(result.disclose(&grant).unwrap(), json!({"message_id":"42"}));
    let denied = DisclosureGrant {
        fields: ["credentials".into()].into_iter().collect(),
        ..grant
    };
    assert!(matches!(
        result.disclose(&denied),
        Err(AuthorityError::ResourceDenied(_))
    ));
}

#[test]
fn destructive_tools_cannot_recommend_weak_mediation() {
    let mut package = manifest(MediationMode::PerCall);
    package.tools[0].risk = RiskClass::Destructive;
    assert!(matches!(
        package.validate(),
        Err(ManifestError::UnsafeMediation(_))
    ));
}

#[test]
fn brokered_tools_require_declared_connections_and_reject_url_smuggling() {
    let mut package = manifest(MediationMode::PerCall);
    package.entrypoint = AddonEntrypoint::BrokeredProcess {
        executable: "mail-test".into(),
        arguments: Vec::new(),
    };
    package.tools[0].required_connections.clear();
    assert!(matches!(
        package.validate(),
        Err(ManifestError::UnsafeBrokeredTool(_))
    ));

    let request = ServiceRequest {
        protocol_version: INVOCATION_PROTOCOL_VERSION,
        invocation_id: "request".into(),
        agent: agent(7),
        package_hash: HASH.into(),
        tool: "send".into(),
        connection: "account:alice".into(),
        connection_kind: "mail.oauth".into(),
        destination: "api.mail.test:443".into(),
        method: ServiceMethod::Get,
        path: "/messages?redirect=https://attacker.invalid".into(),
        query: BTreeMap::new(),
        body: None,
    };
    assert!(matches!(
        request.validate_shape(),
        Err(ServiceBrokerError::InvalidRequest(_))
    ));
}

#[test]
fn cancellation_and_revocation_are_generation_bound() {
    let issuer = TicketIssuer::new("host-1", [5; 32]);
    let package = manifest(MediationMode::PerCall);
    let mut authority = AuthoritySet::default();
    authority.grant(capability(MediationMode::PerCall));
    authority
        .validate_and_consume(
            &package,
            &approval(&issuer, invocation("cancel-me")),
            &issuer,
            1,
            None,
        )
        .unwrap();
    let stale_cancel = CancellationRecord {
        protocol_version: INVOCATION_PROTOCOL_VERSION,
        invocation_id: "cancel-me".into(),
        agent: agent(8),
        reason: "test".into(),
    };
    assert_eq!(
        authority.apply_cancellation(&stale_cancel),
        Err(AuthorityError::StaleAgent)
    );
    let cancel = CancellationRecord {
        agent: agent(7),
        ..stale_cancel
    };
    authority.apply_cancellation(&cancel).unwrap();
    assert!(authority.is_cancelled("cancel-me"));

    let stale_revoke = ToolRevocation {
        schema_version: TOOL_CAPABILITY_SCHEMA_VERSION,
        capability_id: "cap-1".into(),
        agent: agent(8),
        effective_at_ms: 2,
        reason: "test".into(),
    };
    assert_eq!(
        authority.apply_revocation(&stale_revoke),
        Err(AuthorityError::StaleAgent)
    );
    authority
        .apply_revocation(&ToolRevocation {
            agent: agent(7),
            ..stale_revoke
        })
        .unwrap();
    assert!(authority.active_capabilities(&agent(7), 2).is_empty());
}
