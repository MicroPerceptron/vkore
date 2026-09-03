//! Authenticated adapter from the local harness UI to `vkore-policy`.

use serde_json::{Value, json};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use vkore_addon_sdk::{AgentRef, PromptRef};
use vkore_policy::{
    ActiveGrant, ApprovalDecision, ApprovalRequest, AuthorityScope, AuthorityTransport,
    BrokerCommitRequest, BrokerCommitResult, BudgetCeiling, DecisionKind, DecisionLifetime,
    DelegationCeiling, POLICY_SCHEMA_VERSION, PolicyEngine, PolicyError, PolicyStore,
    RequestProvenance, Revocation, ToolScope, UserActionAuthenticator,
};

struct Inner {
    engine: PolicyEngine,
    authenticator: UserActionAuthenticator,
}

pub struct UiApprovalService {
    inner: Mutex<Inner>,
}

impl UiApprovalService {
    pub fn open(state_root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(state_root).map_err(|error| error.to_string())?;
        let keys = load_or_create_keys(&state_root.join("policy.keys"))?;
        let store = PolicyStore::open(state_root.join("policy.audit.jsonl"), keys[0])
            .map_err(|error| error.to_string())?;
        let authenticator = UserActionAuthenticator::new("local-ui-v1", keys[1]);
        let mut engine = PolicyEngine::new(store, authenticator.clone(), keys[2]);
        let stale: Vec<_> = engine.state().grants.keys().cloned().collect();
        for (index, grant_id) in stale.into_iter().enumerate() {
            let now = now_ms().saturating_add(index as u64);
            let action = authenticator.issue(
                format!("startup-revoke-{now}-{index}"),
                "daemon-recovery",
                format!("revoke:{grant_id}"),
                now,
                now.saturating_add(30_000),
            );
            engine
                .revoke(
                    &grant_id,
                    &action,
                    "stale grant retired during daemon recovery",
                    now,
                    &mut ResidentBrokerAdapter,
                )
                .map_err(policy_error)?;
        }
        Ok(Self {
            inner: Mutex::new(Inner {
                engine,
                authenticator,
            }),
        })
    }

    pub fn request(
        &self,
        project_id: &str,
        session_id: &str,
        addon_id: &str,
        tool: &str,
    ) -> Result<Value, String> {
        let now = now_ms();
        let id = format!("approval-{}-{now}", safe_id(project_id));
        let requested = AuthorityScope {
            tools: vec![ToolScope {
                addon_id: addon_id.into(),
                tool: tool.into(),
                arguments: Default::default(),
            }],
            budgets: BudgetCeiling {
                wall_time_ms: 30_000,
                tool_calls: 1,
                result_bytes: 65_536,
                ..BudgetCeiling::default()
            },
            delegation: DelegationCeiling::default(),
            require_confirmation_for_external_write: true,
            require_confirmation_for_destructive: true,
            ..AuthorityScope::default()
        };
        let request = ApprovalRequest {
            schema_version: POLICY_SCHEMA_VERSION,
            id: id.clone(),
            agent: AgentRef {
                id: format!("worker-{project_id}"),
                generation: 1,
            },
            prompt: PromptRef {
                session_id: session_id.into(),
                prompt_id: "project-start".into(),
            },
            provenance: RequestProvenance::AuthenticatedUserIntent {
                intent_id: format!("enable-{addon_id}"),
            },
            current: AuthorityScope::default(),
            requested,
            effect_summary: format!("Allow {addon_id}.{tool} once for this agent"),
            resume_action: format!("{addon_id}.{tool}"),
            requested_at_ms: now,
            expires_at_ms: now.saturating_add(5 * 60_000),
        };
        let mut inner = self.inner.lock().map_err(|_| "policy lock poisoned")?;
        let evaluation = inner
            .engine
            .request(request.clone())
            .map_err(policy_error)?;
        Ok(json!({"request":request,"evaluation":format!("{evaluation:?}")}))
    }

    pub fn state(&self) -> Result<Value, String> {
        let inner = self.inner.lock().map_err(|_| "policy lock poisoned")?;
        let state = inner.engine.state();
        Ok(json!({
            "revision": state.revision,
            "pending": state.pending.values().collect::<Vec<_>>(),
            "grants": state.grants.values().collect::<Vec<_>>(),
            "policies": state.policies.values().collect::<Vec<_>>(),
            "audit_events": state.audit.len()
        }))
    }

    pub fn decide(
        &self,
        request_id: &str,
        allow: bool,
        lifetime: &str,
        ui_session: &str,
    ) -> Result<Value, String> {
        let now = now_ms();
        let action_name = if allow {
            format!("approve:{request_id}")
        } else {
            format!("deny:{request_id}")
        };
        let mut inner = self.inner.lock().map_err(|_| "policy lock poisoned")?;
        let action = inner.authenticator.issue(
            format!("click-{request_id}-{now}"),
            ui_session,
            action_name,
            now,
            now.saturating_add(30_000),
        );
        let lifetime = match lifetime {
            "standing" => DecisionLifetime::Standing,
            "session" => DecisionLifetime::Session(ui_session.into()),
            "prompt" => DecisionLifetime::Prompt("project-start".into()),
            _ => DecisionLifetime::Once,
        };
        let outcome = inner
            .engine
            .decide(
                ApprovalDecision {
                    schema_version: POLICY_SCHEMA_VERSION,
                    request_id: request_id.into(),
                    kind: if allow {
                        DecisionKind::Allow
                    } else {
                        DecisionKind::Deny
                    },
                    lifetime,
                    approved_scope: None,
                    maximum_uses: 1,
                    reason: "authenticated local UI decision".into(),
                },
                &action,
                now,
            )
            .map_err(policy_error)?;
        let commit = if let Some(ActiveGrant { id, .. }) = &outcome.grant {
            Some(
                inner
                    .engine
                    .commit(id, &mut ResidentBrokerAdapter)
                    .map_err(policy_error)?,
            )
        } else {
            None
        };
        Ok(
            json!({"allowed":allow,"receipt":outcome.receipt,"grant":outcome.grant,"policy":outcome.policy,"commit":commit}),
        )
    }

    pub fn revoke(&self, grant_id: &str, ui_session: &str) -> Result<Value, String> {
        let now = now_ms();
        let mut inner = self.inner.lock().map_err(|_| "policy lock poisoned")?;
        let action = inner.authenticator.issue(
            format!("revoke-click-{now}"),
            ui_session,
            format!("revoke:{grant_id}"),
            now,
            now.saturating_add(30_000),
        );
        let result = inner
            .engine
            .revoke(
                grant_id,
                &action,
                "conversation closed by authenticated local UI",
                now,
                &mut ResidentBrokerAdapter,
            )
            .map_err(policy_error)?;
        Ok(json!({"revoked":true,"commit":result}))
    }
}

struct ResidentBrokerAdapter;
impl AuthorityTransport for ResidentBrokerAdapter {
    fn apply(&mut self, request: &BrokerCommitRequest) -> BrokerCommitResult {
        BrokerCommitResult {
            request_id: request.request_id.clone(),
            grant_id: request.grant_id.clone(),
            committed: true,
            kore_transaction: format!("deterministic-ui: {}", request.authority_ceiling_digest),
            detail: "committed by deterministic resident-broker adapter".into(),
        }
    }
    fn revoke(&mut self, revocation: &Revocation) -> BrokerCommitResult {
        BrokerCommitResult {
            request_id: revocation.id.clone(),
            grant_id: revocation.target_id.clone(),
            committed: true,
            kore_transaction: format!("deterministic-revoke:{}", revocation.id),
            detail: "revoked by deterministic resident-broker adapter".into(),
        }
    }
}

fn load_or_create_keys(path: &Path) -> Result<[[u8; 32]; 3], String> {
    let bytes = if path.exists() {
        std::fs::read(path).map_err(|error| error.to_string())?
    } else {
        let seed = format!(
            "{}:{}:{:?}",
            std::process::id(),
            now_ms(),
            std::thread::current().id()
        );
        let first = blake3::hash(seed.as_bytes());
        let second = blake3::keyed_hash(first.as_bytes(), b"vkore-policy-ui-action");
        let third = blake3::keyed_hash(first.as_bytes(), b"vkore-policy-receipt");
        let bytes = [
            first.as_bytes().as_slice(),
            second.as_bytes().as_slice(),
            third.as_bytes().as_slice(),
        ]
        .concat();
        std::fs::write(path, &bytes).map_err(|error| error.to_string())?;
        bytes
    };
    if bytes.len() != 96 {
        return Err("invalid policy key store".into());
    }
    let mut keys = [[0u8; 32]; 3];
    for (index, key) in keys.iter_mut().enumerate() {
        key.copy_from_slice(&bytes[index * 32..(index + 1) * 32]);
    }
    Ok(keys)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn safe_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
fn policy_error(error: PolicyError) -> String {
    error.to_string()
}
