use crate::store::active_policy;
use crate::*;
use std::fmt;

#[derive(Clone)]
pub struct UserActionAuthenticator {
    key_id: String,
    key: [u8; 32],
}

impl UserActionAuthenticator {
    pub fn new(key_id: impl Into<String>, key: [u8; 32]) -> Self {
        Self {
            key_id: key_id.into(),
            key,
        }
    }

    /// Called only after the local UI has authenticated a real user action.
    pub fn issue(
        &self,
        action_id: impl Into<String>,
        ui_session: impl Into<String>,
        action: impl Into<String>,
        issued_at_ms: u64,
        expires_at_ms: u64,
    ) -> UserAction {
        let mut token = UserAction {
            action_id: action_id.into(),
            ui_session: ui_session.into(),
            action: action.into(),
            issued_at_ms,
            expires_at_ms,
            authentication: self.key_id.clone(),
        };
        token.authentication = format!(
            "{}:{}",
            self.key_id,
            self.authenticate(&token.action_payload())
        );
        token
    }

    fn verify(&self, action: &UserAction, expected: &str, now_ms: u64) -> bool {
        if action.action != expected
            || action.expires_at_ms < now_ms
            || action.issued_at_ms > now_ms
        {
            return false;
        }
        let expected = format!(
            "{}:{}",
            self.key_id,
            self.authenticate(&action.action_payload())
        );
        constant_time_eq(expected.as_bytes(), action.authentication.as_bytes())
    }

    fn authenticate(&self, payload: &[u8]) -> String {
        blake3::keyed_hash(&self.key, payload).to_hex().to_string()
    }
}

impl UserAction {
    fn action_payload(&self) -> Vec<u8> {
        serde_json::to_vec(&(
            POLICY_SCHEMA_VERSION,
            &self.action_id,
            &self.ui_session,
            &self.action,
            self.issued_at_ms,
            self.expires_at_ms,
        ))
        .expect("user action fields serialize")
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyError {
    UnsupportedSchema,
    InvalidScope,
    UnknownRequest,
    ExpiredRequest,
    UnauthenticatedUserAction,
    ReplayedUserAction,
    ScopeExpansion,
    InvalidDecision,
    UnknownGrant,
    UnknownTarget,
    BrokerRefused(String),
    Store(String),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for PolicyError {}

impl From<StoreError> for PolicyError {
    fn from(value: StoreError) -> Self {
        Self::Store(value.to_string())
    }
}

pub trait AuthorityTransport {
    fn apply(&mut self, request: &BrokerCommitRequest) -> BrokerCommitResult;
    fn revoke(&mut self, revocation: &Revocation) -> BrokerCommitResult;
}

pub struct PolicyEngine {
    store: PolicyStore,
    authenticator: UserActionAuthenticator,
    receipt_key: [u8; 32],
}

impl PolicyEngine {
    pub fn new(
        store: PolicyStore,
        authenticator: UserActionAuthenticator,
        receipt_key: [u8; 32],
    ) -> Self {
        Self {
            store,
            authenticator,
            receipt_key,
        }
    }

    pub fn state(&self) -> &PolicyState {
        self.store.state()
    }

    pub fn export_policies(&self) -> Result<String, PolicyError> {
        self.store.export_policies().map_err(Into::into)
    }

    pub fn import_policies(
        &mut self,
        json: &str,
        action: &UserAction,
        now_ms: u64,
    ) -> Result<usize, PolicyError> {
        if self
            .store
            .state()
            .used_user_actions
            .contains(&action.action_id)
        {
            return Err(PolicyError::ReplayedUserAction);
        }
        let digest = blake3::hash(json.as_bytes()).to_hex().to_string();
        if !self
            .authenticator
            .verify(action, &format!("import-policies:{digest}"), now_ms)
        {
            return Err(PolicyError::UnauthenticatedUserAction);
        }
        let policies: std::collections::BTreeMap<String, StandingPolicy> =
            serde_json::from_str(json).map_err(|error| PolicyError::Store(error.to_string()))?;
        if policies
            .iter()
            .any(|(id, policy)| id != &policy.id || !policy.scope.validate())
        {
            return Err(PolicyError::InvalidScope);
        }
        let count = policies.len();
        for (_, mut policy) in policies {
            policy.revision = self.store.state().revision + 1;
            self.store
                .append(AuditEvent::PolicyCreated(Box::new(policy)))?;
        }
        self.store.append(AuditEvent::PolicyImported {
            action_id: action.action_id.clone(),
            count,
        })?;
        Ok(count)
    }

    pub fn request(&mut self, request: ApprovalRequest) -> Result<Evaluation, PolicyError> {
        if request.schema_version != POLICY_SCHEMA_VERSION {
            return Err(PolicyError::UnsupportedSchema);
        }
        if !request.requested.validate() || !request.current.validate() {
            return Err(PolicyError::InvalidScope);
        }
        let evaluation = self.evaluate(&request, request.requested_at_ms);
        self.store.append(AuditEvent::Request(Box::new(request)))?;
        Ok(evaluation)
    }

    pub fn evaluate(&self, request: &ApprovalRequest, now_ms: u64) -> Evaluation {
        if request.expires_at_ms < now_ms {
            return Evaluation::Interactive {
                reason: "request expired".into(),
            };
        }
        let matching: Vec<_> = self
            .store
            .state()
            .policies
            .values()
            .filter(|policy| {
                active_policy(policy, now_ms) && policy.scope.contains(&request.requested)
            })
            .collect();
        if let Some(policy) = matching
            .iter()
            .find(|policy| policy.effect == DecisionKind::Deny)
        {
            return Evaluation::DeniedByPolicy {
                policy_id: policy.id.clone(),
                reason: "matched an explicit never-allow rule".into(),
            };
        }
        if let Some(policy) = matching
            .iter()
            .find(|policy| policy.effect == DecisionKind::AlwaysAsk)
        {
            return Evaluation::Interactive {
                reason: format!("policy {} requires confirmation every time", policy.id),
            };
        }
        if let Some(policy) = matching
            .iter()
            .find(|policy| policy.effect == DecisionKind::Allow)
        {
            return Evaluation::AllowedByPolicy {
                policy_id: policy.id.clone(),
            };
        }
        Evaluation::Interactive {
            reason: "no standing policy contains the exact requested authority".into(),
        }
    }

    pub fn decide(
        &mut self,
        decision: ApprovalDecision,
        action: &UserAction,
        now_ms: u64,
    ) -> Result<DecisionOutcome, PolicyError> {
        if decision.schema_version != POLICY_SCHEMA_VERSION {
            return Err(PolicyError::UnsupportedSchema);
        }
        if self
            .store
            .state()
            .used_user_actions
            .contains(&action.action_id)
        {
            return Err(PolicyError::ReplayedUserAction);
        }
        let request = self
            .store
            .state()
            .pending
            .get(&decision.request_id)
            .cloned()
            .ok_or(PolicyError::UnknownRequest)?;
        if request.expires_at_ms < now_ms {
            return Err(PolicyError::ExpiredRequest);
        }
        let expected_action = match decision.kind {
            DecisionKind::Allow => format!("approve:{}", decision.request_id),
            DecisionKind::Deny => format!("deny:{}", decision.request_id),
            DecisionKind::AlwaysAsk => format!("always-ask:{}", decision.request_id),
        };
        if !self.authenticator.verify(action, &expected_action, now_ms) {
            return Err(PolicyError::UnauthenticatedUserAction);
        }
        let scope = decision
            .approved_scope
            .clone()
            .unwrap_or_else(|| request.requested.clone());
        if !request.requested.contains(&scope) {
            return Err(PolicyError::ScopeExpansion);
        }
        if decision.maximum_uses == 0 && decision.kind == DecisionKind::Allow {
            return Err(PolicyError::InvalidDecision);
        }
        self.store.append(AuditEvent::Decision {
            decision: Box::new(decision.clone()),
            action_id: action.action_id.clone(),
        })?;
        let expires_at_ms = lifetime_expiry(&decision.lifetime, &request, now_ms);
        let policy = if matches!(decision.lifetime, DecisionLifetime::Standing)
            || decision.kind == DecisionKind::AlwaysAsk
        {
            let policy = StandingPolicy {
                id: format!("policy:{}:{}", self.store.state().revision + 1, request.id),
                revision: self.store.state().revision + 1,
                effect: decision.kind,
                scope: scope.clone(),
                created_from_request: request.id.clone(),
                created_at_ms: now_ms,
                expires_at_ms: None,
                maximum_uses: (decision.maximum_uses > 0).then_some(decision.maximum_uses),
                uses: 0,
            };
            self.store
                .append(AuditEvent::PolicyCreated(Box::new(policy.clone())))?;
            Some(policy)
        } else {
            None
        };
        if decision.kind == DecisionKind::Deny {
            self.store.append(AuditEvent::Refused {
                request_id: request.id,
                reason: decision.reason,
            })?;
            return Ok(DecisionOutcome {
                receipt: None,
                grant: None,
                policy,
            });
        }
        if decision.kind == DecisionKind::AlwaysAsk {
            return Ok(DecisionOutcome {
                receipt: None,
                grant: None,
                policy,
            });
        }
        let grant = ActiveGrant {
            id: format!("grant:{}:{}", self.store.state().revision + 1, request.id),
            request_id: request.id.clone(),
            agent: request.agent.clone(),
            prompt: request.prompt.clone(),
            scope: scope.clone(),
            lifetime: decision.lifetime,
            expires_at_ms,
            remaining_uses: decision.maximum_uses,
            committed: false,
        };
        self.store
            .append(AuditEvent::GrantCreated(Box::new(grant.clone())))?;
        let receipt = self.receipt(&request, &grant, scope)?;
        Ok(DecisionOutcome {
            receipt: Some(receipt),
            grant: Some(grant),
            policy,
        })
    }

    pub fn authorize_by_policy(
        &mut self,
        request: &ApprovalRequest,
        policy_id: &str,
        now_ms: u64,
    ) -> Result<ActiveGrant, PolicyError> {
        let policy = self
            .store
            .state()
            .policies
            .get(policy_id)
            .cloned()
            .ok_or(PolicyError::UnknownTarget)?;
        if !active_policy(&policy, now_ms)
            || policy.effect != DecisionKind::Allow
            || !policy.scope.contains(&request.requested)
            || request.expires_at_ms < now_ms
        {
            return Err(PolicyError::InvalidDecision);
        }
        self.store.append(AuditEvent::PolicyUsed {
            policy_id: policy.id,
            request_id: request.id.clone(),
        })?;
        let grant = ActiveGrant {
            id: format!("grant:{}:{}", self.store.state().revision + 1, request.id),
            request_id: request.id.clone(),
            agent: request.agent.clone(),
            prompt: request.prompt.clone(),
            scope: request.requested.clone(),
            lifetime: DecisionLifetime::Once,
            expires_at_ms: request.expires_at_ms,
            remaining_uses: 1,
            committed: false,
        };
        self.store
            .append(AuditEvent::GrantCreated(Box::new(grant.clone())))?;
        Ok(grant)
    }

    pub fn commit(
        &mut self,
        grant_id: &str,
        transport: &mut impl AuthorityTransport,
    ) -> Result<BrokerCommitResult, PolicyError> {
        let grant = self
            .store
            .state()
            .grants
            .get(grant_id)
            .cloned()
            .ok_or(PolicyError::UnknownGrant)?;
        let scope = serde_json::to_vec(&grant.scope)
            .map_err(|error| PolicyError::Store(error.to_string()))?;
        let request = BrokerCommitRequest {
            schema_version: POLICY_SCHEMA_VERSION,
            request_id: grant.request_id.clone(),
            grant_id: grant.id.clone(),
            agent: grant.agent,
            policy_revision: self.store.state().revision,
            expires_at_ms: grant.expires_at_ms,
            maximum_uses: grant.remaining_uses,
            authority_ceiling_digest: blake3::hash(&scope).to_hex().to_string(),
        };
        let result = transport.apply(&request);
        self.store.append(AuditEvent::Applied(result.clone()))?;
        if !result.committed {
            return Err(PolicyError::BrokerRefused(result.detail));
        }
        Ok(result)
    }

    pub fn verify_receipt(
        &self,
        receipt: &ApprovalReceipt,
        request_id: &str,
        agent: &vkore_addon_sdk::AgentRef,
        now_ms: u64,
    ) -> bool {
        if receipt.schema_version != POLICY_SCHEMA_VERSION
            || receipt.request_id != request_id
            || &receipt.agent != agent
            || receipt.expires_at_ms < now_ms
        {
            return false;
        }
        let mut unsigned = receipt.clone();
        unsigned.authentication.clear();
        let Ok(payload) = serde_json::to_vec(&unsigned) else {
            return false;
        };
        let expected = blake3::keyed_hash(&self.receipt_key, &payload)
            .to_hex()
            .to_string();
        constant_time_eq(expected.as_bytes(), receipt.authentication.as_bytes())
    }

    pub fn revoke(
        &mut self,
        target_id: &str,
        action: &UserAction,
        reason: impl Into<String>,
        now_ms: u64,
        transport: &mut impl AuthorityTransport,
    ) -> Result<BrokerCommitResult, PolicyError> {
        if self
            .store
            .state()
            .used_user_actions
            .contains(&action.action_id)
        {
            return Err(PolicyError::ReplayedUserAction);
        }
        if !self
            .authenticator
            .verify(action, &format!("revoke:{target_id}"), now_ms)
        {
            return Err(PolicyError::UnauthenticatedUserAction);
        }
        let agent = self
            .store
            .state()
            .grants
            .get(target_id)
            .map(|grant| grant.agent.clone());
        if agent.is_none() && !self.store.state().policies.contains_key(target_id) {
            return Err(PolicyError::UnknownTarget);
        }
        let revocation = Revocation {
            id: format!("revoke:{}:{}", self.store.state().revision + 1, target_id),
            target_id: target_id.into(),
            agent,
            reason: reason.into(),
            effective_at_ms: now_ms,
        };
        let result = transport.revoke(&revocation);
        self.store.append(AuditEvent::Revoked(revocation))?;
        self.store.append(AuditEvent::Applied(result.clone()))?;
        if !result.committed {
            return Err(PolicyError::BrokerRefused(result.detail));
        }
        Ok(result)
    }

    pub fn expire(&mut self, now_ms: u64) -> Result<(), PolicyError> {
        let expired: Vec<String> = self
            .store
            .state()
            .grants
            .values()
            .filter(|grant| grant.expires_at_ms < now_ms)
            .map(|grant| grant.id.clone())
            .collect();
        for target_id in expired {
            self.store.append(AuditEvent::Expired {
                target_id,
                at_ms: now_ms,
            })?;
        }
        Ok(())
    }

    fn receipt(
        &self,
        request: &ApprovalRequest,
        grant: &ActiveGrant,
        scope: AuthorityScope,
    ) -> Result<ApprovalReceipt, PolicyError> {
        let mut receipt = ApprovalReceipt {
            schema_version: POLICY_SCHEMA_VERSION,
            receipt_id: format!("receipt:{}", grant.id),
            request_id: request.id.clone(),
            agent: request.agent.clone(),
            policy_revision: self.store.state().revision,
            scope,
            expires_at_ms: grant.expires_at_ms,
            maximum_uses: grant.remaining_uses,
            authentication: String::new(),
        };
        let payload =
            serde_json::to_vec(&receipt).map_err(|error| PolicyError::Store(error.to_string()))?;
        receipt.authentication = blake3::keyed_hash(&self.receipt_key, &payload)
            .to_hex()
            .to_string();
        Ok(receipt)
    }
}

fn lifetime_expiry(lifetime: &DecisionLifetime, request: &ApprovalRequest, now_ms: u64) -> u64 {
    match lifetime {
        DecisionLifetime::Once | DecisionLifetime::ToolCall(_) => now_ms.saturating_add(5 * 60_000),
        DecisionLifetime::Prompt(prompt) if prompt == &request.prompt.prompt_id => {
            now_ms.saturating_add(24 * 60 * 60_000)
        }
        DecisionLifetime::Session(session) if session == &request.prompt.session_id => {
            now_ms.saturating_add(24 * 60 * 60_000)
        }
        DecisionLifetime::Until(expiry) => *expiry,
        DecisionLifetime::Standing => u64::MAX,
        _ => now_ms,
    }
}
