//! The agent graph, derived from the session's event stream.
//!
//! The design's spawn tree is drawn from sample data. The real runtime keeps a
//! resident broker that grants prompt-scoped authority to a worker per
//! generation, so that is what this builds: a broker root holding the offered
//! catalog, and one worker beneath it for each generation the stream mentions.
//!
//! Nothing here invents structure. A generation appears only once the stream
//! names it, and a worker holds a capability only once a `capability` event
//! records it being installed.

use serde_json::Value;
use std::collections::BTreeMap;

/// What an agent is, in the runtime's own terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The resident broker. It holds the catalog and grants from it.
    Broker,
    /// A prompt-scoped worker, one per generation.
    Worker,
    /// A worker spawned by another prompt-scoped worker.
    SubAgent,
}

impl Role {
    /// The label the design puts in a node's title row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Broker => "resident",
            Self::Worker => "worker",
            Self::SubAgent => "sub-agent",
        }
    }
}

/// One capability, and whether this agent actually holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    /// Short identifier shown in the chip.
    pub label: String,
    /// False when the parent offers it and this agent was not granted it.
    pub held: bool,
}

/// One tool call this agent made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    /// Tool name.
    pub name: String,
    /// Arguments, already flattened for display.
    pub detail: String,
    /// Whether the call reported a failure.
    pub failed: bool,
}

/// A node in the spawn lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// Stable identity, used for selection.
    pub id: String,
    /// Display name.
    pub name: String,
    /// What this agent is.
    pub role: Role,
    /// Spawn generation. The broker is generation zero.
    pub generation: u64,
    /// Whether the agent is still live.
    pub live: bool,
    /// Held capabilities first, then withheld ones.
    pub capabilities: Vec<Capability>,
    /// Tools this agent executed, in order.
    pub tools: Vec<ToolCall>,
    /// Index of this agent's parent in the graph.
    pub parent: Option<usize>,
}

impl Agent {
    /// Capabilities this agent actually holds.
    pub fn held(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities.iter().filter(|entry| entry.held)
    }

    /// Capabilities the parent offers that this agent was not granted.
    pub fn withheld(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities.iter().filter(|entry| !entry.held)
    }

    /// The design's node footer: capabilities, executions, generation.
    pub fn footer(&self) -> String {
        format!(
            "{} capabilities · {} executed · gen {}",
            self.held().count(),
            self.tools.len(),
            self.generation
        )
    }
}

/// The derived spawn lineage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// Agents in placement order; the broker is always first when present.
    pub agents: Vec<Agent>,
}

impl Graph {
    /// Builds the lineage the events describe.
    pub fn from_events(events: &[Value], session_live: bool) -> Self {
        let references: Vec<&Value> = events.iter().collect();
        let catalog = crate::broker_catalog(&references);
        let offered: Vec<String> = catalog.values().map(|entry| entry.label.clone()).collect();
        if catalog.is_empty() && !events.iter().any(worker_evidence) {
            return Self::default();
        }

        let mut agents = vec![Agent {
            id: "broker".into(),
            name: "ai-broker".into(),
            role: Role::Broker,
            generation: 0,
            live: session_live,
            capabilities: offered
                .iter()
                .map(|label| Capability {
                    label: label.clone(),
                    held: true,
                })
                .collect(),
            tools: Vec::new(),
            parent: None,
        }];

        // One worker per generation the stream mentions, in first-seen order.
        let mut workers: BTreeMap<String, usize> = BTreeMap::new();
        let mut calls: BTreeMap<(usize, u64), (usize, usize)> = BTreeMap::new();
        for event in events {
            if !worker_evidence(event) && event["kind"] != "error" {
                continue;
            }
            let Some(generation) = event["generation"]
                .as_u64()
                .or_else(|| event["payload"]["generation"].as_u64())
            else {
                continue;
            };
            let identity = worker_identity(event);
            if generation == 0 || (event["kind"] == "error" && !workers.contains_key(&identity)) {
                continue;
            }
            let index = *workers.entry(identity.clone()).or_insert_with(|| {
                agents.push(Agent {
                    id: identity,
                    name: format!("agent-{generation:04x}"),
                    role: Role::Worker,
                    generation,
                    live: false,
                    capabilities: offered
                        .iter()
                        .map(|label| Capability {
                            label: label.clone(),
                            // Withheld until an install says otherwise.
                            held: false,
                        })
                        .collect(),
                    tools: Vec::new(),
                    parent: Some(0),
                });
                agents.len() - 1
            });

            match event["kind"].as_str() {
                Some("broker_proposal") => {
                    if let Some(label) = crate::decision_payload(event)
                        .and_then(|proposal| proposal["label"].as_str().map(str::to_owned))
                        .filter(|label| !label.is_empty())
                    {
                        agents[index].name = label;
                    }
                }
                Some("capability") => {
                    for label in capability_labels(event) {
                        grant(&mut agents[index], &label);
                    }
                }
                Some("tool_call") => {
                    if let Some(call) = tool_call(event) {
                        let call_id = crate::decision_payload(event)
                            .and_then(|payload| payload["call_id"].as_u64());
                        if call_id.is_some_and(|call_id| calls.contains_key(&(index, call_id))) {
                            continue;
                        }
                        agents[index].tools.push(call);
                        if let Some(call_id) = call_id {
                            calls.insert((index, call_id), (index, agents[index].tools.len() - 1));
                        }
                    }
                }
                Some("tool_result") => {
                    if tool_result_failed(event) {
                        let call_id = crate::decision_payload(event)
                            .and_then(|payload| payload["call_id"].as_u64());
                        let tool = call_id
                            .and_then(|call_id| calls.get(&(index, call_id)).copied())
                            .or_else(|| {
                                agents[index]
                                    .tools
                                    .len()
                                    .checked_sub(1)
                                    .map(|tool| (index, tool))
                            });
                        if let Some((agent, tool)) = tool
                            && let Some(call) = agents
                                .get_mut(agent)
                                .and_then(|agent| agent.tools.get_mut(tool))
                        {
                            call.failed = true;
                        }
                    }
                }
                Some("error") => {
                    if let Some(last) = agents[index].tools.last_mut() {
                        last.failed = true;
                    }
                }
                _ => {}
            }
        }

        // The newest generation is the live one while the session runs.
        let latest_lifecycle = events.iter().rev().find(|event| {
            worker_evidence(event)
                || matches!(
                    event["kind"].as_str(),
                    Some("prompt_terminal" | "conversation_closed")
                )
        });
        let retired = latest_lifecycle.is_some_and(|event| {
            matches!(
                event["kind"].as_str(),
                Some("prompt_terminal" | "conversation_closed")
            ) || event["payload"]["state"]
                .as_str()
                .is_some_and(|state| state.eq_ignore_ascii_case("teardown"))
        });
        if session_live
            && !retired
            && let Some(last) = agents.iter_mut().skip(1).next_back()
        {
            last.live = true;
        }
        Self { agents }
    }

    /// Parent links in placement order, for [`super::layout::tidy`].
    pub fn parents(&self) -> Vec<Option<usize>> {
        self.agents.iter().map(|agent| agent.parent).collect()
    }

    /// Finds an agent by its selection identity.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.agents.iter().position(|agent| agent.id == id)
    }
}

/// Prompts in one chat have independent workers despite sharing a chat generation.
pub(super) fn worker_identity(event: &Value) -> String {
    event["prompt_id"]
        .as_str()
        .map(|id| format!("task-{id}"))
        .unwrap_or_else(|| {
            format!(
                "agent-{}",
                event["generation"]
                    .as_u64()
                    .or_else(|| event["payload"]["generation"].as_u64())
                    .unwrap_or_default()
            )
        })
}

// Session generation and readiness metadata do not prove that a worker exists.
fn worker_evidence(event: &Value) -> bool {
    match event["kind"].as_str() {
        Some("broker_proposal") => crate::decision_payload(event).is_some_and(|proposal| {
            proposal["action"].as_str().is_some_and(|action| {
                action.eq_ignore_ascii_case("CreateAgent") || action == "create_agent"
            })
        }),
        Some("capability" | "tool_call" | "tool_result") => true,
        _ => false,
    }
}

/// Marks `label` held, adding it when the catalog never offered it.
fn grant(agent: &mut Agent, label: &str) {
    if let Some(entry) = agent
        .capabilities
        .iter_mut()
        .find(|entry| entry.label == label)
    {
        entry.held = true;
        return;
    }
    agent.capabilities.push(Capability {
        label: label.to_owned(),
        held: true,
    });
}

fn capability_labels(event: &Value) -> Vec<String> {
    let payload = &event["payload"];
    let state = payload["state"].as_str().unwrap_or("installed");
    if !matches!(
        state.to_ascii_lowercase().as_str(),
        "installed" | "granted" | "active"
    ) {
        return Vec::new();
    }
    let mut labels = payload["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|capability| {
            capability
                .as_str()
                .or_else(|| capability["label"].as_str())
                .or_else(|| capability["name"].as_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    labels.extend(
        payload["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned),
    );
    if let Some(label) = payload["capability"]
        .as_str()
        .or_else(|| payload["label"].as_str())
        .or_else(|| payload["name"].as_str())
        .or_else(|| payload["tool"].as_str())
    {
        labels.push(label.to_owned());
    }
    labels.sort();
    labels.dedup();
    labels
}

fn tool_result_failed(event: &Value) -> bool {
    let payload = crate::decision_payload(event).unwrap_or(Value::Null);
    payload["result"]
        .as_str()
        .or_else(|| payload["status"].as_str())
        .is_some_and(|status| {
            matches!(
                status.to_ascii_lowercase().as_str(),
                "refused" | "revoked" | "cancelled" | "failed" | "error"
            )
        })
}

fn tool_call(event: &Value) -> Option<ToolCall> {
    let call = crate::decision_payload(event)?;
    let name = call["command"]
        .as_str()
        .or_else(|| call["tool"].as_str())?
        .to_owned();
    let detail = crate::call_arguments(&call).join(" ");
    Some(ToolCall {
        name,
        detail,
        failed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn catalog_event() -> Value {
        json!({
            "kind": "inference",
            "payload": {
                "kind": "Input",
                "role": "Broker",
                "payload": json!({
                    "offer_catalog": [[0, "tool", "read_workspace"], [1, "tool", "write_workspace"]]
                }).to_string()
            }
        })
    }

    #[test]
    fn an_empty_stream_has_no_graph() {
        assert_eq!(Graph::from_events(&[], false), Graph::default());
    }

    #[test]
    fn session_metadata_does_not_invent_agents_and_terminal_retires_the_worker() {
        let metadata = [
            json!({"kind":"session_opened","generation":1,"payload":{}}),
            json!({"kind":"user_prompt","generation":1,"payload":{"text":"hello"}}),
            json!({"kind":"validation","generation":1,"payload":{"stage":"model_server_starting"}}),
        ];
        assert_eq!(Graph::from_events(&metadata, true), Graph::default());
        let events = [
            json!({"kind":"tool_call","generation":1,"payload":{"tool":"cat"}}),
            json!({"kind":"prompt_terminal","generation":1,"payload":{"status":"completed"}}),
        ];
        let graph = Graph::from_events(&events, true);
        assert_eq!(graph.agents.len(), 2);
        assert!(!graph.agents[1].live);
    }

    #[test]
    fn a_broker_refusal_does_not_create_a_worker() {
        let graph = Graph::from_events(
            &[
                catalog_event(),
                json!({"kind":"broker_proposal","generation":1,"payload":{"action":"refused","reason":"unavailable"}}),
                json!({"kind":"error","generation":1,"payload":{"stage":"agent_failure"}}),
            ],
            true,
        );
        assert_eq!(graph.agents.len(), 1);
        assert_eq!(graph.agents[0].role, Role::Broker);
    }

    #[test]
    fn the_broker_holds_everything_it_offers() {
        let graph = Graph::from_events(&[catalog_event()], true);
        assert_eq!(graph.agents.len(), 1);
        let broker = &graph.agents[0];
        assert_eq!(broker.role, Role::Broker);
        assert_eq!(broker.generation, 0);
        assert!(broker.live);
        assert_eq!(broker.held().count(), 2);
        assert_eq!(broker.withheld().count(), 0);
        assert_eq!(broker.parent, None);
    }

    #[test]
    fn a_worker_starts_with_every_offer_withheld() {
        let graph = Graph::from_events(
            &[
                catalog_event(),
                json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read_workspace"}}),
            ],
            false,
        );
        assert_eq!(graph.agents.len(), 2);
        let worker = &graph.agents[1];
        assert_eq!(worker.role, Role::Worker);
        assert_eq!(worker.generation, 1);
        assert_eq!(worker.parent, Some(0));
        // Nothing was installed, so the offers show as withheld.
        assert_eq!(worker.held().count(), 0);
        assert_eq!(worker.withheld().count(), 2);
    }

    #[test]
    fn installing_a_capability_moves_it_from_withheld_to_held() {
        let graph = Graph::from_events(
            &[
                catalog_event(),
                json!({"kind": "capability", "generation": 1, "payload": {"capability": "read_workspace"}}),
            ],
            false,
        );
        let worker = &graph.agents[1];
        assert_eq!(
            worker.held().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            vec!["read_workspace"]
        );
        assert_eq!(
            worker
                .withheld()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            vec!["write_workspace"]
        );
    }

    #[test]
    fn dynamic_lease_installs_every_capability_and_refused_result_marks_its_call() {
        let graph = Graph::from_events(
            &[
                catalog_event(),
                json!({"kind":"broker_proposal","generation":7,"payload":{"action":"CreateAgent","label":"release-worker"}}),
                json!({"kind":"capability","generation":7,"payload":{"state":"installed","capabilities":[
                    {"kind":"executable","label":"publish_release"},
                    {"kind":"view","label":"workspace/releases"}
                ],"tools":["publish_release","checksum"]}}),
                json!({"kind":"tool_call","generation":7,"payload":{"command":"publish_release","call_id":41,"arguments":[{"kind":"path","value":"/workspace/releases/v1"}]}}),
                json!({"kind":"tool_result","generation":7,"payload":{"call_id":41,"result":"Refused","text":"policy denied"}}),
            ],
            false,
        );
        let worker = &graph.agents[1];
        assert_eq!(worker.name, "release-worker");
        assert_eq!(
            worker
                .held()
                .map(|cap| cap.label.as_str())
                .collect::<Vec<_>>(),
            vec!["checksum", "publish_release", "workspace/releases"]
        );
        assert_eq!(worker.tools.len(), 1);
        assert!(worker.tools[0].failed);
        assert_eq!(worker.tools[0].detail, "/releases/v1");
    }

    #[test]
    fn each_generation_becomes_exactly_one_worker() {
        let graph = Graph::from_events(
            &[
                catalog_event(),
                json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "a"}}),
                json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "b"}}),
                json!({"kind": "tool_call", "generation": 2, "payload": {"tool": "c"}}),
            ],
            false,
        );
        assert_eq!(graph.agents.len(), 3);
        assert_eq!(graph.agents[1].tools.len(), 2);
        assert_eq!(graph.agents[2].tools.len(), 1);
        assert_eq!(graph.parents(), vec![None, Some(0), Some(0)]);
    }

    #[test]
    fn only_the_newest_generation_is_live_and_only_while_running() {
        let events = [
            catalog_event(),
            json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "a"}}),
            json!({"kind": "tool_call", "generation": 2, "payload": {"tool": "b"}}),
        ];
        let running = Graph::from_events(&events, true);
        assert!(!running.agents[1].live);
        assert!(running.agents[2].live);

        let stopped = Graph::from_events(&events, false);
        assert!(stopped.agents.iter().all(|agent| !agent.live));
    }

    #[test]
    fn an_error_marks_the_call_it_followed() {
        let graph = Graph::from_events(
            &[
                catalog_event(),
                json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read_workspace"}}),
                json!({"kind": "error", "generation": 1, "payload": {"message": "denied"}}),
            ],
            false,
        );
        assert!(graph.agents[1].tools[0].failed);
    }

    #[test]
    fn the_footer_counts_what_the_agent_actually_holds() {
        let graph = Graph::from_events(
            &[
                catalog_event(),
                json!({"kind": "capability", "generation": 1, "payload": {"capability": "read_workspace"}}),
                json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read_workspace"}}),
            ],
            false,
        );
        assert_eq!(
            graph.agents[1].footer(),
            "1 capabilities · 1 executed · gen 1"
        );
    }
}
