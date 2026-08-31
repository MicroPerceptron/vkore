use crate::{AddonManifest, ExecutionDomain};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AgentRef {
    pub id: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PromptRef {
    pub session_id: String,
    pub prompt_id: String,
}

pub type CapabilityId = String;
pub type InvocationId = String;
pub const TOOL_CAPABILITY_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediationMode {
    Pregranted,
    PerCall,
    BrokerReview,
    UserConfirm,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSensitivity {
    Public,
    Internal,
    Sensitive,
    Secret,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ArgConstraint {
    Exact(Value),
    OneOf(Vec<Value>),
    StringPrefix(String),
    MaximumU64(u64),
    Absent,
}

impl ArgConstraint {
    pub fn accepts(&self, value: Option<&Value>) -> bool {
        match self {
            Self::Exact(expected) => value == Some(expected),
            Self::OneOf(expected) => value.is_some_and(|value| expected.contains(value)),
            Self::StringPrefix(prefix) => value
                .and_then(Value::as_str)
                .is_some_and(|value| value.starts_with(prefix)),
            Self::MaximumU64(maximum) => value
                .and_then(Value::as_u64)
                .is_some_and(|value| value <= *maximum),
            Self::Absent => value.is_none(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CallConstraints {
    #[serde(default)]
    pub arguments: BTreeMap<String, ArgConstraint>,
    #[serde(default)]
    pub allowed_resources: BTreeMap<String, BTreeSet<String>>,
    pub maximum_input_bytes: usize,
    pub maximum_output_bytes: usize,
    #[serde(default)]
    pub disclosure_fields: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolCapability {
    pub schema_version: u16,
    pub id: CapabilityId,
    pub agent: AgentRef,
    pub prompt: PromptRef,
    pub addon_id: String,
    pub addon_version: String,
    pub package_hash: String,
    pub tool: String,
    pub domain: ExecutionDomain,
    pub constraints: CallConstraints,
    pub mediation: MediationMode,
    pub expires_at_ms: u64,
    pub remaining_calls: u32,
    #[serde(default)]
    pub delegable: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Invocation {
    pub protocol_version: u16,
    pub invocation_id: InvocationId,
    pub capability_id: CapabilityId,
    pub agent: AgentRef,
    pub prompt: PromptRef,
    pub addon_id: String,
    pub package_hash: String,
    pub tool: String,
    pub arguments: BTreeMap<String, Value>,
    #[serde(default)]
    pub resources: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InvocationTicket {
    pub key_id: String,
    pub authentication: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InvocationApproval {
    pub invocation: Invocation,
    pub ticket: InvocationTicket,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultHandle {
    pub id: String,
    pub invocation_id: InvocationId,
    pub agent: AgentRef,
    pub package_hash: String,
    pub sensitivity: ResultSensitivity,
    pub available_fields: BTreeSet<String>,
    pub byte_count: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct InvocationResult {
    pub protocol_version: u16,
    pub handle: ResultHandle,
    pub disclosed: Value,
    #[serde(default)]
    pub untrusted_annotations: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CancellationRecord {
    pub protocol_version: u16,
    pub invocation_id: InvocationId,
    pub agent: AgentRef,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolRevocation {
    pub schema_version: u16,
    pub capability_id: CapabilityId,
    pub agent: AgentRef,
    pub effective_at_ms: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Escalation {
    BrokerReview,
    UserConfirm,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthorityError {
    UnknownCapability,
    Revoked,
    Expired,
    Exhausted,
    Replayed,
    InvalidTicket,
    UnsupportedProtocol,
    StaleAgent,
    WrongPrompt,
    WrongAddon,
    WrongPackage,
    ForgedTool,
    WrongDomain,
    InputTooLarge,
    ArgumentDenied(String),
    ResourceDenied(String),
    MissingTool,
    MediationTooWeak,
    MediationRequired(Escalation),
}

impl fmt::Display for AuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for AuthorityError {}

#[derive(Clone)]
pub struct TicketIssuer {
    key_id: String,
    key: [u8; 32],
}

impl TicketIssuer {
    pub fn new(key_id: impl Into<String>, key: [u8; 32]) -> Self {
        Self {
            key_id: key_id.into(),
            key,
        }
    }

    pub fn issue(&self, invocation: &Invocation) -> Result<InvocationTicket, AuthorityError> {
        let payload = serde_json::to_vec(invocation).map_err(|_| AuthorityError::InvalidTicket)?;
        let authentication = blake3::keyed_hash(&self.key, &payload).to_hex().to_string();
        Ok(InvocationTicket {
            key_id: self.key_id.clone(),
            authentication,
        })
    }

    fn verify(&self, invocation: &Invocation, ticket: &InvocationTicket) -> bool {
        if ticket.key_id != self.key_id {
            return false;
        }
        let Ok(expected) = self.issue(invocation) else {
            return false;
        };
        constant_time_eq(
            expected.authentication.as_bytes(),
            ticket.authentication.as_bytes(),
        )
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

#[derive(Default)]
pub struct AuthoritySet {
    capabilities: BTreeMap<CapabilityId, ToolCapability>,
    revoked: BTreeSet<CapabilityId>,
    used_invocations: BTreeMap<InvocationId, AgentRef>,
    cancelled_invocations: BTreeSet<InvocationId>,
}

/// An invocation that passed the live authority, ticket, schema, and mediation checks.
///
/// Its fields are private so host execution cannot be reached with a deserialized
/// worker request alone.
#[derive(Debug, PartialEq)]
pub struct ValidatedInvocation {
    approval: InvocationApproval,
    maximum_output_bytes: usize,
    disclosure_fields: BTreeSet<String>,
}

impl ValidatedInvocation {
    pub fn approval(&self) -> &InvocationApproval {
        &self.approval
    }
    pub fn maximum_output_bytes(&self) -> usize {
        self.maximum_output_bytes
    }
    pub fn disclosure_fields(&self) -> &BTreeSet<String> {
        &self.disclosure_fields
    }
}

impl AuthoritySet {
    pub fn grant(&mut self, capability: ToolCapability) {
        self.revoked.remove(&capability.id);
        self.capabilities.insert(capability.id.clone(), capability);
    }

    pub fn revoke(&mut self, capability_id: &str) -> bool {
        let present = self.capabilities.contains_key(capability_id);
        if present {
            self.revoked.insert(capability_id.to_owned());
        }
        present
    }

    pub fn apply_revocation(&mut self, record: &ToolRevocation) -> Result<(), AuthorityError> {
        if record.schema_version != TOOL_CAPABILITY_SCHEMA_VERSION {
            return Err(AuthorityError::UnsupportedProtocol);
        }
        let capability = self
            .capabilities
            .get(&record.capability_id)
            .ok_or(AuthorityError::UnknownCapability)?;
        if capability.agent != record.agent {
            return Err(AuthorityError::StaleAgent);
        }
        self.revoked.insert(record.capability_id.clone());
        Ok(())
    }

    pub fn apply_cancellation(
        &mut self,
        record: &CancellationRecord,
    ) -> Result<(), AuthorityError> {
        if record.protocol_version != crate::INVOCATION_PROTOCOL_VERSION {
            return Err(AuthorityError::UnsupportedProtocol);
        }
        let agent = self
            .used_invocations
            .get(&record.invocation_id)
            .ok_or(AuthorityError::UnknownCapability)?;
        if *agent != record.agent {
            return Err(AuthorityError::StaleAgent);
        }
        self.cancelled_invocations
            .insert(record.invocation_id.clone());
        Ok(())
    }

    pub fn is_cancelled(&self, invocation_id: &str) -> bool {
        self.cancelled_invocations.contains(invocation_id)
    }

    pub fn active_capabilities(&self, agent: &AgentRef, now_ms: u64) -> Vec<&ToolCapability> {
        self.capabilities
            .values()
            .filter(|capability| {
                capability.agent == *agent
                    && capability.expires_at_ms >= now_ms
                    && capability.remaining_calls > 0
                    && !self.revoked.contains(&capability.id)
            })
            .collect()
    }

    pub fn validate_and_consume(
        &mut self,
        package: &AddonManifest,
        approval: &InvocationApproval,
        issuer: &TicketIssuer,
        now_ms: u64,
        completed_mediation: Option<MediationMode>,
    ) -> Result<ValidatedInvocation, AuthorityError> {
        let call = &approval.invocation;
        if call.protocol_version != crate::INVOCATION_PROTOCOL_VERSION {
            return Err(AuthorityError::UnsupportedProtocol);
        }
        if !issuer.verify(call, &approval.ticket) {
            return Err(AuthorityError::InvalidTicket);
        }
        if self.used_invocations.contains_key(&call.invocation_id) {
            return Err(AuthorityError::Replayed);
        }
        let capability = self
            .capabilities
            .get_mut(&call.capability_id)
            .ok_or(AuthorityError::UnknownCapability)?;
        if capability.schema_version != TOOL_CAPABILITY_SCHEMA_VERSION {
            return Err(AuthorityError::UnsupportedProtocol);
        }
        if self.revoked.contains(&capability.id) {
            return Err(AuthorityError::Revoked);
        }
        if capability.expires_at_ms < now_ms {
            return Err(AuthorityError::Expired);
        }
        if capability.remaining_calls == 0 {
            return Err(AuthorityError::Exhausted);
        }
        if call.agent != capability.agent {
            return Err(AuthorityError::StaleAgent);
        }
        if call.prompt != capability.prompt {
            return Err(AuthorityError::WrongPrompt);
        }
        if call.addon_id != capability.addon_id || package.id != capability.addon_id {
            return Err(AuthorityError::WrongAddon);
        }
        if call.package_hash != capability.package_hash
            || package.package_hash != capability.package_hash
            || package.version != capability.addon_version
        {
            return Err(AuthorityError::WrongPackage);
        }
        if call.tool != capability.tool {
            return Err(AuthorityError::ForgedTool);
        }
        let tool = package
            .tools
            .iter()
            .find(|tool| tool.name == call.tool)
            .ok_or(AuthorityError::MissingTool)?;
        if tool.domain != capability.domain {
            return Err(AuthorityError::WrongDomain);
        }
        let encoded =
            serde_json::to_vec(&call.arguments).map_err(|_| AuthorityError::InputTooLarge)?;
        if encoded.len() > capability.constraints.maximum_input_bytes
            || encoded.len() > tool.maximum_input_bytes
        {
            return Err(AuthorityError::InputTooLarge);
        }
        validate_object_schema(&tool.input_schema, &call.arguments)?;
        for (name, constraint) in &capability.constraints.arguments {
            if !constraint.accepts(call.arguments.get(name)) {
                return Err(AuthorityError::ArgumentDenied(name.clone()));
            }
        }
        for (kind, value) in &call.resources {
            if !capability
                .constraints
                .allowed_resources
                .get(kind)
                .is_some_and(|allowed| allowed.contains(value))
            {
                return Err(AuthorityError::ResourceDenied(kind.clone()));
            }
        }
        if capability.mediation < tool.minimum_mediation {
            return Err(AuthorityError::MediationTooWeak);
        }
        let escalation = match capability.mediation {
            MediationMode::BrokerReview => Some(Escalation::BrokerReview),
            MediationMode::UserConfirm => Some(Escalation::UserConfirm),
            MediationMode::Pregranted | MediationMode::PerCall => None,
        };
        if let Some(required) = escalation {
            let sufficient = matches!(
                (required.clone(), completed_mediation),
                (
                    Escalation::BrokerReview,
                    Some(MediationMode::BrokerReview | MediationMode::UserConfirm)
                ) | (Escalation::UserConfirm, Some(MediationMode::UserConfirm))
            );
            if !sufficient {
                return Err(AuthorityError::MediationRequired(required));
            }
        }
        capability.remaining_calls -= 1;
        self.used_invocations
            .insert(call.invocation_id.clone(), call.agent.clone());
        Ok(ValidatedInvocation {
            approval: approval.clone(),
            maximum_output_bytes: capability.constraints.maximum_output_bytes,
            disclosure_fields: capability.constraints.disclosure_fields.clone(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DisclosureGrant {
    pub handle_id: String,
    pub agent: AgentRef,
    pub fields: BTreeSet<String>,
}

impl InvocationResult {
    pub fn disclose(&self, grant: &DisclosureGrant) -> Result<Value, AuthorityError> {
        if self.handle.id != grant.handle_id || self.handle.agent != grant.agent {
            return Err(AuthorityError::StaleAgent);
        }
        if !grant.fields.is_subset(&self.handle.available_fields) {
            return Err(AuthorityError::ResourceDenied("result_field".into()));
        }
        let object = self
            .disclosed
            .as_object()
            .ok_or_else(|| AuthorityError::ResourceDenied("result".into()))?;
        let selected = grant
            .fields
            .iter()
            .filter_map(|field| {
                object
                    .get(field)
                    .cloned()
                    .map(|value| (field.clone(), value))
            })
            .collect();
        Ok(Value::Object(selected))
    }
}

pub fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis() as u64)
}

fn validate_object_schema(
    schema: &Value,
    arguments: &BTreeMap<String, Value>,
) -> Result<(), AuthorityError> {
    let Some(schema) = schema.as_object() else {
        return Ok(());
    };
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if !arguments.contains_key(name) {
                return Err(AuthorityError::ArgumentDenied(name.into()));
            }
        }
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        for name in arguments.keys() {
            if !properties.is_some_and(|properties| properties.contains_key(name)) {
                return Err(AuthorityError::ArgumentDenied(name.clone()));
            }
        }
    }
    if let Some(properties) = properties {
        for (name, value) in arguments {
            let Some(expected) = properties
                .get(name)
                .and_then(|property| property.get("type"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let matches = match expected {
                "string" => value.is_string(),
                "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                "null" => value.is_null(),
                _ => true,
            };
            if !matches {
                return Err(AuthorityError::ArgumentDenied(name.clone()));
            }
        }
    }
    Ok(())
}
