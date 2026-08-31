use crate::{AuditEvent, POLICY_SCHEMA_VERSION, PolicyState, SignedAuditEvent};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    Json(serde_json::Error),
    CorruptAudit { sequence: u64 },
    UnsupportedSchema(u16),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

pub struct PolicyStore {
    path: PathBuf,
    key: [u8; 32],
    state: PolicyState,
    last_authentication: String,
}

impl PolicyStore {
    pub fn open(path: impl Into<PathBuf>, key: [u8; 32]) -> Result<Self, StoreError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut store = Self {
            path,
            key,
            state: PolicyState::default(),
            last_authentication: "genesis".into(),
        };
        store.reload()?;
        Ok(store)
    }

    pub fn state(&self) -> &PolicyState {
        &self.state
    }

    pub fn export_policies(&self) -> Result<String, StoreError> {
        serde_json::to_string_pretty(&self.state.policies).map_err(StoreError::Json)
    }

    pub fn append(&mut self, event: AuditEvent) -> Result<SignedAuditEvent, StoreError> {
        let sequence = self.state.revision + 1;
        let authentication =
            authenticate_event(&self.key, sequence, &self.last_authentication, &event)?;
        let signed = SignedAuditEvent {
            schema_version: POLICY_SCHEMA_VERSION,
            sequence,
            previous_authentication: self.last_authentication.clone(),
            event,
            authentication,
        };
        let mut encoded = serde_json::to_vec(&signed)?;
        encoded.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(&encoded)?;
        file.sync_all()?;
        self.last_authentication = signed.authentication.clone();
        apply_event(&mut self.state, signed.clone());
        Ok(signed)
    }

    fn reload(&mut self) -> Result<(), StoreError> {
        if !self.path.exists() {
            return Ok(());
        }
        let file = File::open(&self.path)?;
        let mut expected_previous = "genesis".to_owned();
        let mut expected_sequence = 1u64;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let signed: SignedAuditEvent = serde_json::from_str(&line)?;
            if signed.schema_version != POLICY_SCHEMA_VERSION {
                return Err(StoreError::UnsupportedSchema(signed.schema_version));
            }
            let expected = authenticate_event(
                &self.key,
                signed.sequence,
                &signed.previous_authentication,
                &signed.event,
            )?;
            if signed.sequence != expected_sequence
                || signed.previous_authentication != expected_previous
                || !constant_time_eq(expected.as_bytes(), signed.authentication.as_bytes())
            {
                return Err(StoreError::CorruptAudit {
                    sequence: signed.sequence,
                });
            }
            expected_sequence += 1;
            expected_previous = signed.authentication.clone();
            apply_event(&mut self.state, signed);
        }
        self.last_authentication = expected_previous;
        Ok(())
    }
}

fn authenticate_event(
    key: &[u8; 32],
    sequence: u64,
    previous: &str,
    event: &AuditEvent,
) -> Result<String, StoreError> {
    let payload = serde_json::to_vec(&(POLICY_SCHEMA_VERSION, sequence, previous, event))?;
    Ok(blake3::keyed_hash(key, &payload).to_hex().to_string())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn apply_event(state: &mut PolicyState, signed: SignedAuditEvent) {
    state.revision = signed.sequence;
    match &signed.event {
        AuditEvent::Request(request) => {
            state
                .pending
                .insert(request.id.clone(), (**request).clone());
        }
        AuditEvent::Decision {
            decision,
            action_id,
        } => {
            state.used_user_actions.insert(action_id.clone());
            state.pending.remove(&decision.request_id);
        }
        AuditEvent::PolicyCreated(policy) => {
            state.policies.insert(policy.id.clone(), (**policy).clone());
        }
        AuditEvent::PolicyUsed { policy_id, .. } => {
            if let Some(policy) = state.policies.get_mut(policy_id) {
                policy.uses = policy.uses.saturating_add(1);
            }
        }
        AuditEvent::PolicyImported { action_id, .. } => {
            state.used_user_actions.insert(action_id.clone());
        }
        AuditEvent::GrantCreated(grant) => {
            state.grants.insert(grant.id.clone(), (**grant).clone());
        }
        AuditEvent::Applied(result) => {
            if let Some(grant) = state.grants.get_mut(&result.grant_id) {
                grant.committed = result.committed;
            }
        }
        AuditEvent::Refused { request_id, .. } => {
            state.pending.remove(request_id);
        }
        AuditEvent::Expired { target_id, .. } => {
            state.grants.remove(target_id);
            state.policies.remove(target_id);
            state.pending.remove(target_id);
        }
        AuditEvent::Revoked(revocation) => {
            state.grants.remove(&revocation.target_id);
            state.policies.remove(&revocation.target_id);
        }
    }
    state.audit.push(signed);
}

pub(crate) fn active_policy(policy: &crate::StandingPolicy, now_ms: u64) -> bool {
    policy.expires_at_ms.is_none_or(|expiry| expiry >= now_ms)
        && policy
            .maximum_uses
            .is_none_or(|maximum| policy.uses < maximum)
}
