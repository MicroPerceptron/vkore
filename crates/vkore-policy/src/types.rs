use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use vkore_addon_sdk::{AgentRef, ArgConstraint, PromptRef};

pub const POLICY_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestProvenance {
    AuthenticatedUserIntent { intent_id: String },
    ParentAgent { parent: AgentRef },
    WorkerProposal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceScope {
    pub kind: String,
    pub selector: String,
    #[serde(default)]
    pub recursive: bool,
    pub operations: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolScope {
    pub addon_id: String,
    pub tool: String,
    #[serde(default)]
    pub arguments: BTreeMap<String, ArgConstraint>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BudgetCeiling {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub storage_bytes: u64,
    pub wall_time_ms: u64,
    pub tool_calls: u32,
    pub result_bytes: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationCeiling {
    pub child_count: u32,
    pub maximum_depth: u16,
    pub lifetime_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AuthorityScope {
    #[serde(default)]
    pub resources: Vec<ResourceScope>,
    #[serde(default)]
    pub tools: Vec<ToolScope>,
    #[serde(default)]
    pub accounts: BTreeSet<String>,
    #[serde(default)]
    pub recipients: BTreeSet<String>,
    #[serde(default)]
    pub network_destinations: BTreeSet<String>,
    pub budgets: BudgetCeiling,
    pub delegation: DelegationCeiling,
    #[serde(default)]
    pub require_confirmation_for_external_write: bool,
    #[serde(default)]
    pub require_confirmation_for_destructive: bool,
}

impl AuthorityScope {
    pub fn validate(&self) -> bool {
        self.resources.iter().all(|resource| {
            !resource.kind.is_empty()
                && valid_selector(&resource.selector)
                && !resource.operations.is_empty()
        }) && self
            .tools
            .iter()
            .all(|tool| !tool.addon_id.is_empty() && !tool.tool.is_empty())
    }

    /// Returns true when this ceiling contains every right in `requested`.
    pub fn contains(&self, requested: &Self) -> bool {
        self.validate()
            && requested.validate()
            && requested.resources.iter().all(|request| {
                self.resources
                    .iter()
                    .any(|allowed| resource_contains(allowed, request))
            })
            && requested.tools.iter().all(|request| {
                self.tools
                    .iter()
                    .any(|allowed| tool_contains(allowed, request))
            })
            && requested.accounts.is_subset(&self.accounts)
            && requested.recipients.iter().all(|recipient| {
                self.recipients
                    .iter()
                    .any(|allowed| recipient_matches(allowed, recipient))
            })
            && requested
                .network_destinations
                .is_subset(&self.network_destinations)
            && budget_contains(&self.budgets, &requested.budgets)
            && self.delegation.child_count >= requested.delegation.child_count
            && self.delegation.maximum_depth >= requested.delegation.maximum_depth
            && self.delegation.lifetime_ms >= requested.delegation.lifetime_ms
            && (!self.require_confirmation_for_external_write
                || requested.require_confirmation_for_external_write)
            && (!self.require_confirmation_for_destructive
                || requested.require_confirmation_for_destructive)
    }
}

fn resource_contains(allowed: &ResourceScope, requested: &ResourceScope) -> bool {
    if allowed.kind != requested.kind || !requested.operations.is_subset(&allowed.operations) {
        return false;
    }
    let allowed_selector = normalize_selector(&allowed.selector);
    let requested_selector = normalize_selector(&requested.selector);
    requested_selector == allowed_selector
        || (allowed.recursive
            && requested_selector
                .strip_prefix(&allowed_selector)
                .is_some_and(|suffix| suffix.starts_with('/')))
}

fn tool_contains(allowed: &ToolScope, requested: &ToolScope) -> bool {
    allowed.addon_id == requested.addon_id
        && allowed.tool == requested.tool
        && requested.arguments.iter().all(|(name, constraint)| {
            allowed
                .arguments
                .get(name)
                .is_some_and(|ceiling| constraint_within(ceiling, constraint))
        })
}

fn constraint_within(allowed: &ArgConstraint, requested: &ArgConstraint) -> bool {
    match requested {
        ArgConstraint::Exact(value) => allowed.accepts(Some(value)),
        _ => allowed == requested,
    }
}

fn recipient_matches(allowed: &str, requested: &str) -> bool {
    if let Some(domain) = allowed.strip_prefix("*@") {
        requested
            .rsplit_once('@')
            .is_some_and(|(_, candidate)| candidate.eq_ignore_ascii_case(domain))
    } else {
        allowed.eq_ignore_ascii_case(requested)
    }
}

fn budget_contains(allowed: &BudgetCeiling, requested: &BudgetCeiling) -> bool {
    allowed.cpu_millis >= requested.cpu_millis
        && allowed.memory_bytes >= requested.memory_bytes
        && allowed.storage_bytes >= requested.storage_bytes
        && allowed.wall_time_ms >= requested.wall_time_ms
        && allowed.tool_calls >= requested.tool_calls
        && allowed.result_bytes >= requested.result_bytes
}

fn valid_selector(selector: &str) -> bool {
    !selector.is_empty()
        && !selector
            .split(['/', '\\'])
            .any(|component| component == "..")
        && !selector.contains('\0')
}

fn normalize_selector(selector: &str) -> String {
    selector.replace('\\', "/").trim_end_matches('/').to_owned()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalRequest {
    pub schema_version: u16,
    pub id: String,
    pub agent: AgentRef,
    pub prompt: PromptRef,
    pub provenance: RequestProvenance,
    pub current: AuthorityScope,
    pub requested: AuthorityScope,
    pub effect_summary: String,
    pub resume_action: String,
    pub requested_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Allow,
    Deny,
    AlwaysAsk,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum DecisionLifetime {
    Once,
    ToolCall(String),
    Prompt(String),
    Session(String),
    Until(u64),
    Standing,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalDecision {
    pub schema_version: u16,
    pub request_id: String,
    pub kind: DecisionKind,
    pub lifetime: DecisionLifetime,
    pub approved_scope: Option<AuthorityScope>,
    pub maximum_uses: u32,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UserAction {
    pub action_id: String,
    pub ui_session: String,
    pub action: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub authentication: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ApprovalReceipt {
    pub schema_version: u16,
    pub receipt_id: String,
    pub request_id: String,
    pub agent: AgentRef,
    pub policy_revision: u64,
    pub scope: AuthorityScope,
    pub expires_at_ms: u64,
    pub maximum_uses: u32,
    pub authentication: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StandingPolicy {
    pub id: String,
    pub revision: u64,
    pub effect: DecisionKind,
    pub scope: AuthorityScope,
    pub created_from_request: String,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub maximum_uses: Option<u32>,
    pub uses: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActiveGrant {
    pub id: String,
    pub request_id: String,
    pub agent: AgentRef,
    pub prompt: PromptRef,
    pub scope: AuthorityScope,
    pub lifetime: DecisionLifetime,
    pub expires_at_ms: u64,
    pub remaining_uses: u32,
    pub committed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BrokerCommitRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub grant_id: String,
    pub agent: AgentRef,
    pub policy_revision: u64,
    pub expires_at_ms: u64,
    pub maximum_uses: u32,
    pub authority_ceiling_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BrokerCommitResult {
    pub request_id: String,
    pub grant_id: String,
    pub committed: bool,
    pub kore_transaction: String,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Revocation {
    pub id: String,
    pub target_id: String,
    pub agent: Option<AgentRef>,
    pub reason: String,
    pub effective_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum AuditEvent {
    Request(Box<ApprovalRequest>),
    Decision {
        decision: Box<ApprovalDecision>,
        action_id: String,
    },
    PolicyCreated(Box<StandingPolicy>),
    PolicyUsed {
        policy_id: String,
        request_id: String,
    },
    PolicyImported {
        action_id: String,
        count: usize,
    },
    GrantCreated(Box<ActiveGrant>),
    Applied(BrokerCommitResult),
    Refused {
        request_id: String,
        reason: String,
    },
    Expired {
        target_id: String,
        at_ms: u64,
    },
    Revoked(Revocation),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SignedAuditEvent {
    pub schema_version: u16,
    pub sequence: u64,
    pub previous_authentication: String,
    pub event: AuditEvent,
    pub authentication: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Evaluation {
    AllowedByPolicy { policy_id: String },
    DeniedByPolicy { policy_id: String, reason: String },
    Interactive { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionOutcome {
    pub receipt: Option<ApprovalReceipt>,
    pub grant: Option<ActiveGrant>,
    pub policy: Option<StandingPolicy>,
}

#[derive(Clone, Debug, Default)]
pub struct PolicyState {
    pub revision: u64,
    pub pending: BTreeMap<String, ApprovalRequest>,
    pub policies: BTreeMap<String, StandingPolicy>,
    pub grants: BTreeMap<String, ActiveGrant>,
    pub audit: Vec<SignedAuditEvent>,
    pub used_user_actions: BTreeSet<String>,
}
