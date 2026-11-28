//! Every action passes here before it runs.
//!
//! [`crate::rule`] says whether a person must approve an action and what they may
//! allow to repeat. `vkore-policy` records each request and answer in a signed,
//! chained audit file, and decides whether an earlier "for this session" answer
//! covers a new request. A remembered answer is bound to this agent session and
//! machine, so nothing carries over once the agent is turned off.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use vkore_addon_sdk::{AgentRef, PromptRef};
use vkore_policy::{
    ApprovalDecision, ApprovalRequest, AuthorityScope, DecisionKind, DecisionLifetime, Evaluation,
    POLICY_SCHEMA_VERSION, PolicyEngine, PolicyError, PolicyStore, RequestProvenance,
    ResourceScope, UserActionAuthenticator,
};

use crate::rule::Approval;
use crate::tools::Action;

/// How long a person has to answer before a request lapses.
const ANSWER_WITHIN_MS: u64 = 10 * 60_000;

/// The rule a gate follows; [`crate::rule::rule`] outside tests.
pub type Rule = fn(&Action) -> Approval;

/// A person's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Once,
    /// Allow, and do not ask again for what the request's [`Remember`] covers.
    Session,
    Deny,
}

/// What "for this session" would cover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Remember {
    Exact,
    Kind,
}

/// A request awaiting an answer.
#[derive(Debug)]
pub struct Ask {
    pub id: String,
    /// `None` when this action may only be allowed once.
    pub remember: Option<Remember>,
    request: ApprovalRequest,
}

/// Whether an action may run.
#[derive(Debug)]
pub enum Verdict {
    Run,
    Refused(String),
    Ask(Box<Ask>),
}

pub struct Gate {
    engine: PolicyEngine,
    authenticator: UserActionAuthenticator,
    session: String,
    machine: String,
    rule: Rule,
    asked: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| {
            u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
        })
}

fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|error| format!("no randomness: {error}"))?;
    Ok(bytes)
}

/// The key the audit files in `directory` are signed with, made on first use and
/// readable by its owner only.
fn key(directory: &Path) -> Result<[u8; 32], String> {
    let path = directory.join("audit.key");
    let fail = |error: std::io::Error| format!("the agent's audit key: {error}");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options.open(&path) {
        Ok(mut file) => {
            let key = random::<32>()?;
            file.write_all(&key).map_err(fail)?;
            file.sync_all().map_err(fail)?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            <[u8; 32]>::try_from(fs::read(&path).map_err(fail)?.as_slice())
                .map_err(|_| "the agent's audit key is damaged".to_owned())
        }
        Err(error) => Err(fail(error)),
    }
}

fn policy_error(error: PolicyError) -> String {
    match error {
        PolicyError::ExpiredRequest => "that approval lapsed before it was answered".into(),
        other => format!("the approval engine refused: {other}"),
    }
}

impl Gate {
    /// A gate for one agent session on `machine`, auditing into `directory`.
    pub fn open(directory: &Path, machine: &str, rule: Rule) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|error| format!("the agent's folder: {error}"))?;
        let master = key(directory)?;
        let derived = |context: &str| blake3::derive_key(context, &master);
        let session = u64::from_le_bytes(random::<8>()?);
        let session = format!("{session:016x}");
        let store = PolicyStore::open(
            directory.join(format!("audit-{session}.jsonl")),
            derived("vkore-agent 2026 audit chain"),
        )
        .map_err(|error| format!("the agent's audit file: {error}"))?;
        let authenticator =
            UserActionAuthenticator::new("vkore-agent", derived("vkore-agent 2026 user action"));
        Ok(Self {
            engine: PolicyEngine::new(
                store,
                authenticator.clone(),
                derived("vkore-agent 2026 receipt"),
            ),
            authenticator,
            session,
            machine: machine.to_owned(),
            rule,
            asked: 0,
        })
    }

    /// Decide whether `action`, proposed during prompt `prompt` as call `call`, may
    /// run, must be asked about, or is refused.
    pub fn check(&mut self, action: &Action, prompt: u64, call: &str) -> Result<Verdict, String> {
        let mut approval = (self.rule)(action);
        // The floor under every rule: a change is never made unasked.
        if approval == Approval::Free && action.changes_machine() {
            approval = Approval::Ask;
        }
        self.asked += 1;
        let (operation, remember) = match approval {
            Approval::Free => return Ok(Verdict::Run),
            // A key used once, so no remembered answer can ever cover it.
            Approval::Ask => (format!("{}#once-{}", action.operation(), self.asked), None),
            Approval::AskRememberExact => (
                format!(
                    "{}#exact-{}",
                    action.operation(),
                    blake3::hash(format!("{action:?}").as_bytes()).to_hex()
                ),
                Some(Remember::Exact),
            ),
            Approval::AskRememberKind => (action.operation().to_owned(), Some(Remember::Kind)),
        };
        let now = now_ms();
        let id = format!("{}-{}", self.session, self.asked);
        let request = ApprovalRequest {
            schema_version: POLICY_SCHEMA_VERSION,
            id: id.clone(),
            agent: AgentRef {
                id: "vkore.agent".into(),
                generation: 1,
            },
            prompt: PromptRef {
                session_id: self.session.clone(),
                prompt_id: prompt.to_string(),
            },
            provenance: RequestProvenance::WorkerProposal,
            current: AuthorityScope::default(),
            requested: AuthorityScope {
                resources: vec![ResourceScope {
                    // Naming the session here keeps a remembered answer inside it.
                    kind: format!("vkore.agent.session-{}", self.session),
                    selector: self.machine.clone(),
                    recursive: false,
                    operations: BTreeSet::from([operation]),
                }],
                ..AuthorityScope::default()
            },
            effect_summary: format!("{}: {}", action.title(), action.detail()),
            resume_action: call.to_owned(),
            requested_at_ms: now,
            expires_at_ms: now.saturating_add(ANSWER_WITHIN_MS),
        };
        match self.engine.request(request.clone()).map_err(policy_error)? {
            Evaluation::AllowedByPolicy { policy_id } => {
                self.engine
                    .authorize_by_policy(&request, &policy_id, now)
                    .map_err(policy_error)?;
                Ok(Verdict::Run)
            }
            Evaluation::DeniedByPolicy { reason, .. } => Ok(Verdict::Refused(reason)),
            Evaluation::Interactive { .. } => Ok(Verdict::Ask(Box::new(Ask {
                id,
                remember,
                request,
            }))),
        }
    }

    /// Record a person's answer. `Ok(true)` when the action may now run.
    /// Call only for an answer a person actually gave.
    pub fn answer(&mut self, ask: Ask, choice: Choice) -> Result<bool, String> {
        if choice == Choice::Session && ask.remember.is_none() {
            return Err("this action can only be allowed once".into());
        }
        let now = now_ms();
        let (kind, lifetime, maximum_uses, verb) = match choice {
            Choice::Once => (DecisionKind::Allow, DecisionLifetime::Once, 1, "approve"),
            Choice::Session => (
                DecisionKind::Allow,
                DecisionLifetime::Standing,
                u32::MAX,
                "approve",
            ),
            Choice::Deny => (DecisionKind::Deny, DecisionLifetime::Once, 0, "deny"),
        };
        let action = self.authenticator.issue(
            format!("{}-answer", ask.id),
            self.session.clone(),
            format!("{verb}:{}", ask.id),
            now,
            now.saturating_add(60_000),
        );
        self.engine
            .decide(
                ApprovalDecision {
                    schema_version: POLICY_SCHEMA_VERSION,
                    request_id: ask.request.id,
                    kind,
                    lifetime,
                    approved_scope: None,
                    maximum_uses,
                    reason: "answered in the vKore window".into(),
                },
                &action,
                now,
            )
            .map_err(policy_error)?;
        Ok(kind == DecisionKind::Allow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Action {
        Action::TypeConsole {
            text: text.into(),
            enter: true,
        }
    }

    fn ask(verdict: Verdict) -> Ask {
        match verdict {
            Verdict::Ask(ask) => *ask,
            other => panic!("expected to be asked, got {other:?}"),
        }
    }

    fn runs(gate: &mut Gate, action: &Action) -> bool {
        matches!(gate.check(action, 1, "c").unwrap(), Verdict::Run)
    }

    #[test]
    fn the_shipped_rule_reads_freely_and_asks_before_any_change() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate::open(directory.path(), "dev", crate::rule::rule).unwrap();
        assert!(runs(&mut gate, &Action::ReadConsole));
        for action in [Action::StartMachine, Action::StopMachine, typed("ls")] {
            assert!(!runs(&mut gate, &action), "{action:?} ran unasked");
        }
    }

    #[test]
    fn a_rule_cannot_let_a_change_run_unasked() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate::open(directory.path(), "dev", |_| Approval::Free).unwrap();
        let asked = ask(gate.check(&typed("ls"), 1, "c").unwrap());
        assert_eq!(asked.remember, None);
    }

    #[test]
    fn once_and_deny_are_not_remembered() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate::open(directory.path(), "dev", |_| Approval::AskRememberKind).unwrap();
        let first = ask(gate.check(&typed("ls"), 1, "a").unwrap());
        assert!(gate.answer(first, Choice::Once).unwrap());
        let second = ask(gate.check(&typed("ls"), 1, "b").unwrap());
        assert!(!gate.answer(second, Choice::Deny).unwrap());
        assert!(!runs(&mut gate, &typed("ls")));
    }

    #[test]
    fn an_exact_answer_covers_only_the_same_action() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate::open(directory.path(), "dev", |_| Approval::AskRememberExact).unwrap();
        let first = ask(gate.check(&typed("ls"), 1, "a").unwrap());
        assert_eq!(first.remember, Some(Remember::Exact));
        assert!(gate.answer(first, Choice::Session).unwrap());
        assert!(runs(&mut gate, &typed("ls")));
        assert!(!runs(&mut gate, &typed("ls -l")));
        // The same text without Enter is a different action.
        assert!(!runs(
            &mut gate,
            &Action::TypeConsole {
                text: "ls".into(),
                enter: false
            }
        ));
    }

    #[test]
    fn a_kind_answer_covers_its_kind_and_no_other() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate::open(directory.path(), "dev", |_| Approval::AskRememberKind).unwrap();
        let first = ask(gate.check(&typed("ls"), 1, "a").unwrap());
        assert!(gate.answer(first, Choice::Session).unwrap());
        assert!(runs(&mut gate, &typed("anything else")));
        assert!(!runs(&mut gate, &Action::StopMachine));
    }

    #[test]
    fn an_action_that_asks_every_time_is_never_covered() {
        // Typing "help" may be remembered; anything else must ask every time.
        fn rule(action: &Action) -> Approval {
            match action {
                Action::TypeConsole { text, .. } if text == "help" => Approval::AskRememberKind,
                _ => Approval::Ask,
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate::open(directory.path(), "dev", rule).unwrap();
        let safe = ask(gate.check(&typed("help"), 1, "a").unwrap());
        assert!(gate.answer(safe, Choice::Session).unwrap());
        let other = ask(gate.check(&typed("rm x"), 1, "b").unwrap());
        assert!(gate.answer(other, Choice::Session).is_err());
    }

    #[test]
    fn a_remembered_answer_ends_with_its_session_and_the_audit_verifies() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = Gate::open(directory.path(), "dev", |_| Approval::AskRememberKind).unwrap();
        let first = ask(gate.check(&typed("ls"), 1, "a").unwrap());
        assert!(gate.answer(first, Choice::Session).unwrap());
        assert!(runs(&mut gate, &typed("ls")));
        let session = gate.session.clone();
        drop(gate);

        let mut next = Gate::open(directory.path(), "dev", |_| Approval::AskRememberKind).unwrap();
        assert!(!runs(&mut next, &typed("ls")));

        // The first session's file reloads under the same key, so its chain holds.
        let master = key(directory.path()).unwrap();
        let path = directory.path().join(format!("audit-{session}.jsonl"));
        let chain = blake3::derive_key("vkore-agent 2026 audit chain", &master);
        let store = PolicyStore::open(&path, chain).unwrap();
        assert!(store.state().audit.len() >= 5);
        // And a changed line is noticed.
        let text = fs::read_to_string(&path).unwrap().replace("ls", "rm");
        fs::write(&path, text).unwrap();
        assert!(PolicyStore::open(&path, chain).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_audit_key_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        Gate::open(directory.path(), "dev", crate::rule::rule).unwrap();
        let mode = fs::metadata(directory.path().join("audit.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }
}
