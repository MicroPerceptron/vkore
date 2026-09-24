//! What the selected node holds, ran, and can reach.
//!
//! Every row means the same thing: a label, and a right-aligned tail that
//! qualifies it. Colour carries meaning rather than decoration -- a struck
//! label is a capability the agent was not granted, a red one is a call that
//! failed, and magenta counts a path the agent reaches.

use crate::topology::filesystem::{self as fs, Filesystem};
use crate::topology::model::Graph;
use crate::{Message, UiElement, theme, widgets};
use iced_core::{Length, alignment};
use iced_widget::{column, container, row, space};

/// A label with a right-aligned tail, and an optional detail line.
pub fn entry<'a>(
    label: String,
    label_color: u32,
    struck: bool,
    tail: Option<String>,
    detail: Option<String>,
    accented: bool,
) -> UiElement<'a> {
    // Strikethrough is not expressible in the text layer, so a withheld
    // capability is carried by the dim colour and the "withheld" tail.
    let _ = struck;
    let mut head = row![widgets::mono(label, label_color)]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center);
    if let Some(tail) = tail {
        head = head.push(space::horizontal());
        head = head.push(widgets::mono(tail, theme::BORDER_DIM));
    }
    let mut body = column![head].spacing(theme::SPACE_1);
    if let Some(detail) = detail {
        body = body.push(widgets::mono(detail, theme::TEXT_MUTED));
    }
    container(body)
        .padding([7, 9])
        .width(Length::Fill)
        .style(move |_| {
            let mut style = theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_SM);
            if accented {
                style.border.color = theme::chip_border();
            }
            style
        })
        .into()
}

/// A section: an uppercase title over a hairline, then its rows.
pub fn section<'a>(title: &'static str, rows: Vec<UiElement<'a>>) -> UiElement<'a> {
    let mut body = column![
        container(widgets::label(title, theme::TEXT_MUTED))
            .padding(iced_core::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 8.0,
                left: 0.0,
            })
            .width(Length::Fill)
            .style(|_| {
                let mut style = theme::panel(theme::BACKGROUND, None, 0.0);
                style.border = theme::border(theme::BORDER, 0.0);
                style
            })
    ]
    .spacing(theme::SPACE_2);
    for entry in rows {
        body = body.push(entry);
    }
    body.into()
}

/// A label/value pair, mono, value right-aligned and coloured by meaning.
pub fn meta<'a>(label: &'static str, value: String, color: u32) -> UiElement<'a> {
    row![
        widgets::mono(label, theme::TEXT_MUTED),
        space::horizontal(),
        widgets::mono(value, color),
    ]
    .align_y(alignment::Vertical::Center)
    .into()
}

/// The inspector body for the agents view.
pub fn agents<'a>(
    graph: &'a Graph,
    filesystem: &'a Filesystem,
    selected: Option<&str>,
) -> (String, String, UiElement<'a>) {
    let Some(agent) = selected
        .and_then(|id| graph.index_of(id))
        .and_then(|index| graph.agents.get(index))
    else {
        return (
            "Nothing selected".into(),
            "Click a node to inspect its authority.".into(),
            column![].into(),
        );
    };

    let index = graph
        .index_of(&agent.id)
        .expect("selected agent is in graph");
    let parent = agent.parent.and_then(|parent| graph.agents.get(parent));
    let parent_capabilities: Vec<&str> = parent
        .map(|parent| parent.held().map(|entry| entry.label.as_str()).collect())
        .unwrap_or_else(|| agent.held().map(|entry| entry.label.as_str()).collect());
    let held_capabilities: Vec<&str> = agent.held().map(|entry| entry.label.as_str()).collect();
    let withheld: Vec<&str> = parent_capabilities
        .iter()
        .copied()
        .filter(|label| !held_capabilities.contains(label))
        .collect();
    let grants: Vec<_> = filesystem
        .grants
        .iter()
        .filter(|grant| grant.agent == index)
        .collect();
    let child_count = graph
        .agents
        .iter()
        .filter(|candidate| candidate.parent == Some(index))
        .count();

    let mut body = column![
        meta(
            "status",
            if agent.live { "live" } else { "retired" }.into(),
            if agent.live {
                theme::SUCCESS
            } else {
                theme::TEXT_SECONDARY
            },
        ),
        meta("children", child_count.to_string(), theme::TEXT),
        meta(
            "capabilities",
            format!(
                "{} of {}",
                held_capabilities.len(),
                parent_capabilities.len()
            ),
            theme::TEXT,
        ),
        meta(
            "granted paths",
            if grants.is_empty() {
                "—".into()
            } else {
                grants.len().to_string()
            },
            if grants.is_empty() {
                theme::TEXT_MUTED
            } else {
                theme::MAGENTA
            },
        ),
    ]
    .spacing(13);

    let mut capabilities = Vec::new();
    for held in agent.held() {
        capabilities.push(entry(
            held.label.clone(),
            theme::MAGENTA,
            false,
            Some("held".into()),
            None,
            true,
        ));
    }
    for withheld in withheld {
        capabilities.push(entry(
            withheld.into(),
            theme::BORDER_DIM,
            true,
            Some("withheld".into()),
            None,
            false,
        ));
    }
    if capabilities.is_empty() {
        capabilities.push(entry(
            "no capabilities offered".into(),
            theme::TEXT_MUTED,
            false,
            None,
            None,
            false,
        ));
    }
    body = body.push(section("Capabilities", capabilities));

    let tools: Vec<UiElement<'a>> = if agent.tools.is_empty() {
        vec![entry(
            "no tool calls yet".into(),
            theme::TEXT_MUTED,
            false,
            None,
            None,
            false,
        )]
    } else {
        agent
            .tools
            .iter()
            .map(|call| {
                let (detail, tail) = call_detail(&call.detail, call.failed);
                entry(
                    call.name.clone(),
                    if call.failed {
                        theme::ERROR
                    } else {
                        theme::TEXT
                    },
                    false,
                    Some(tail),
                    detail,
                    false,
                )
            })
            .collect()
    };
    body = body.push(section("Tools executed", tools));

    let paths = if grants.is_empty() {
        vec![entry(
            "No workspace authority.".into(),
            theme::TEXT_MUTED,
            false,
            None,
            None,
            false,
        )]
    } else {
        grants
            .into_iter()
            .filter_map(|grant| {
                let node = filesystem.paths.get(grant.path)?;
                Some(entry(
                    node.path.replacen("Codebase_directory", ".", 1),
                    if node.kind == fs::Kind::External {
                        theme::MAGENTA
                    } else {
                        theme::TEXT
                    },
                    false,
                    Some(grant.rights.clone()),
                    None,
                    false,
                ))
            })
            .collect()
    };
    body = body.push(section("Granted paths", paths));

    (
        agent.name.clone(),
        format!(
            "{} · generation {}",
            parent
                .map(|parent| format!("child of {}", parent.name))
                .unwrap_or_else(|| "root of the authority tree".into()),
            agent.generation
        ),
        body.into(),
    )
}

fn call_detail(detail: &str, failed: bool) -> (Option<String>, String) {
    if let Some((detail, duration)) = detail.rsplit_once(" · ")
        && duration.ends_with("ms")
        && duration[..duration.len() - 2]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        return (Some(detail.into()), duration.into());
    }
    (
        (!detail.is_empty()).then(|| detail.to_owned()),
        if failed { "failed" } else { "—" }.into(),
    )
}

/// The inspector body for a selected path.
pub fn path<'a>(
    graph: &'a Graph,
    filesystem: &'a Filesystem,
    selected: usize,
) -> (String, String, UiElement<'a>) {
    let Some(node) = filesystem.paths.get(selected) else {
        return (
            EMPTY_TITLE.into(),
            "Click a node to inspect its authority.".into(),
            column![].into(),
        );
    };
    let external = node.kind == fs::Kind::External;
    let direct = filesystem
        .grants
        .iter()
        .filter(|grant| grant.path == selected)
        .count();

    let mut body = column![
        meta(
            "location",
            if external {
                "outside workspace"
            } else {
                "workspace"
            }
            .into(),
            if external {
                theme::WARNING
            } else {
                theme::TEXT_SECONDARY
            },
        ),
        meta(
            "type",
            node.object_kind.label().into(),
            theme::TEXT_SECONDARY
        ),
        meta("direct grants", direct.to_string(), theme::MAGENTA),
    ]
    .spacing(theme::SPACE_2);

    let holders = filesystem.holders(selected);
    let rows: Vec<UiElement<'a>> = if holders.is_empty() {
        vec![entry(
            "no agent holds authority over this path".into(),
            theme::TEXT_MUTED,
            false,
            None,
            None,
            false,
        )]
    } else {
        holders
            .into_iter()
            .filter_map(|(index, direct)| {
                let agent = graph.agents.get(index)?;
                let rights = filesystem
                    .grants
                    .iter()
                    .find(|grant| grant.agent == index && grant.path == selected)
                    .map(|grant| grant.rights.clone());
                Some(entry(
                    agent.name.clone(),
                    theme::TEXT,
                    false,
                    Some(if direct { "direct" } else { "inherited" }.into()),
                    rights,
                    true,
                ))
            })
            .collect()
    };
    body = body.push(section("Agents with access", rows));

    (node.name.clone(), node.path.clone(), body.into())
}

/// The inspector's empty message, so the caller need not repeat it.
pub const EMPTY_TITLE: &str = "Nothing selected";

#[allow(dead_code)]
fn _unused(_: Message) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::model::Graph;
    use serde_json::json;

    fn graph() -> Graph {
        Graph::from_events(
            &[
                json!({
                    "kind": "inference",
                    "payload": {
                        "kind": "Input",
                        "role": "Broker",
                        "payload": json!({"offer_catalog": [[0, "tool", "read_workspace"]]}).to_string()
                    }
                }),
                json!({"kind": "tool_call", "generation": 1, "payload": {"tool": "read_workspace"}}),
            ],
            true,
        )
    }

    #[test]
    fn nothing_selected_says_so_and_asks_for_a_click() {
        let graph = graph();
        let filesystem = Filesystem::default();
        let (title, subtitle, _) = agents(&graph, &filesystem, None);
        assert_eq!(title, EMPTY_TITLE);
        assert!(subtitle.contains("Click a node"));
    }

    #[test]
    fn a_selection_names_the_role_and_generation() {
        let graph = graph();
        let filesystem = Filesystem::default();
        let (title, subtitle, _) = agents(&graph, &filesystem, Some("agent-1"));
        assert_eq!(title, "agent-0001");
        assert_eq!(subtitle, "child of ai-broker · generation 1");
    }

    #[test]
    fn the_broker_is_named_as_the_resident() {
        let graph = graph();
        let filesystem = Filesystem::default();
        let (title, subtitle, _) = agents(&graph, &filesystem, Some("broker"));
        assert_eq!(title, "ai-broker");
        assert_eq!(subtitle, "root of the authority tree · generation 0");
    }

    #[test]
    fn a_path_reports_where_it_lives_and_who_reaches_it() {
        let events = [
            json!({"kind":"capability","generation":1,"payload":{"state":"installed","kind":"view","label":"src","rights":"read"}}),
            json!({"kind":"capability","generation":2,"payload":{"state":"installed","kind":"object","label":"src/main.rs","rights":"read"}}),
        ];
        let graph = Graph::from_events(&events, false);
        let filesystem = Filesystem::from_events(&events, &graph);
        let main = filesystem
            .paths
            .iter()
            .position(|p| p.path == "src/main.rs")
            .unwrap();

        let (title, subtitle, _) = path(&graph, &filesystem, main);
        assert_eq!(title, "main.rs");
        assert_eq!(subtitle, "src/main.rs");
        // One agent was handed it, another reaches it through the directory.
        assert_eq!(filesystem.holders(main).len(), 2);
    }

    #[test]
    fn a_path_index_that_does_not_exist_falls_back_to_empty() {
        let graph = graph();
        let filesystem = Filesystem::default();
        let (title, _, _) = path(&graph, &filesystem, 42);
        assert_eq!(title, EMPTY_TITLE);
    }

    #[test]
    fn an_unknown_selection_falls_back_to_empty() {
        let graph = graph();
        let filesystem = Filesystem::default();
        let (title, _, _) = agents(&graph, &filesystem, Some("agent-does-not-exist"));
        assert_eq!(title, EMPTY_TITLE);
    }
}
