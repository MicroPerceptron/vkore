use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AuthorityDiff {
    pub current_summary: Vec<String>,
    pub requested_additions: Vec<String>,
}

impl AuthorityDiff {
    pub fn between(current: &AuthorityScope, requested: &AuthorityScope) -> Self {
        let current_summary = summarize(current);
        let requested_additions = summarize(requested)
            .into_iter()
            .filter(|entry| !current_summary.contains(entry))
            .collect();
        Self {
            current_summary,
            requested_additions,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DashboardView {
    pub pending: Vec<ApprovalRequest>,
    pub active_grants: Vec<ActiveGrant>,
    pub standing_policies: Vec<StandingPolicy>,
    pub recent_audit: Vec<SignedAuditEvent>,
}

impl DashboardView {
    pub fn from_state(state: &PolicyState) -> Self {
        Self {
            pending: state.pending.values().cloned().collect(),
            active_grants: state.grants.values().cloned().collect(),
            standing_policies: state.policies.values().cloned().collect(),
            recent_audit: state.audit.iter().rev().take(50).cloned().collect(),
        }
    }
}

pub fn render_dashboard(view: &DashboardView) -> String {
    let mut output =
        String::from("<section class=\"policy-dashboard\"><h2>Authority and approvals</h2>");
    output.push_str("<h3>Pending approval</h3>");
    if view.pending.is_empty() {
        output.push_str("<p>No requests waiting.</p>");
    }
    for request in &view.pending {
        let diff = AuthorityDiff::between(&request.current, &request.requested);
        output.push_str(&format!(
            "<article data-request=\"{}\"><h4>{}</h4><p>Agent: {} generation {}</p><p>Source: {}</p><p>Resumes: {}</p><ul>",
            escape(&request.id), escape(&request.effect_summary), escape(&request.agent.id),
            request.agent.generation, escape(&format!("{:?}", request.provenance)),
            escape(&request.resume_action)
        ));
        for addition in diff.requested_additions {
            output.push_str(&format!("<li>{}</li>", escape(&addition)));
        }
        output.push_str("</ul><button data-decision=\"allow-once\">Allow once</button><button data-decision=\"constrain\">Constrain</button><button data-decision=\"deny\">Deny</button></article>");
    }
    output.push_str("<h3>Active grants</h3><ul>");
    for grant in &view.active_grants {
        output.push_str(&format!(
            "<li>{} - {} uses - committed: {} <button data-revoke=\"{}\">Revoke</button></li>",
            escape(&grant.id),
            grant.remaining_uses,
            grant.committed,
            escape(&grant.id)
        ));
    }
    output.push_str("</ul><h3>Standing policies</h3><ul>");
    for policy in &view.standing_policies {
        output.push_str(&format!(
            "<li>{} - {:?} <button data-revoke=\"{}\">Revoke</button></li>",
            escape(&policy.id),
            policy.effect,
            escape(&policy.id)
        ));
    }
    output.push_str("</ul></section>");
    output
}

fn summarize(scope: &AuthorityScope) -> Vec<String> {
    let mut entries = Vec::new();
    entries.extend(scope.resources.iter().map(|resource| {
        format!(
            "{} {}: {}",
            resource.kind,
            resource
                .operations
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
            resource.selector
        )
    }));
    entries.extend(
        scope
            .tools
            .iter()
            .map(|tool| format!("tool {}.{}", tool.addon_id, tool.tool)),
    );
    entries.extend(
        scope
            .accounts
            .iter()
            .map(|account| format!("account {account}")),
    );
    entries.extend(
        scope
            .recipients
            .iter()
            .map(|recipient| format!("recipient {recipient}")),
    );
    entries.extend(
        scope
            .network_destinations
            .iter()
            .map(|destination| format!("network {destination}")),
    );
    entries.push(format!(
        "budgets: {} tool calls, {} MiB memory, {} ms",
        scope.budgets.tool_calls,
        scope.budgets.memory_bytes / 1024 / 1024,
        scope.budgets.wall_time_ms
    ));
    entries
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
