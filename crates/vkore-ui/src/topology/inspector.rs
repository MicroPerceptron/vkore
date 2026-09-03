//! What the selected node holds, ran, and can reach.
//!
//! Every row means the same thing: a label, and a right-aligned tail that
//! qualifies it. Colour carries meaning rather than decoration -- a struck
//! label is a capability the agent was not granted, a red one is a call that
//! failed, and magenta counts a path the agent reaches.

use crate::topology::model::{Agent, Graph};
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
    selected: Option<&str>,
) -> (&'static str, String, UiElement<'a>) {
    let Some(agent) = selected
        .and_then(|id| graph.index_of(id))
        .and_then(|index| graph.agents.get(index))
    else {
        return (
            "Nothing selected",
            "Click a node to inspect its authority.".into(),
            column![].into(),
        );
    };

    let mut body = column![
        meta(
            "state",
            if agent.live { "live" } else { "retired" }.into(),
            if agent.live {
                theme::SUCCESS
            } else {
                theme::TEXT_SECONDARY
            },
        ),
        meta(
            "generation",
            agent.generation.to_string(),
            theme::TEXT_SECONDARY
        ),
        meta(
            "capabilities",
            agent.held().count().to_string(),
            theme::MAGENTA,
        ),
    ]
    .spacing(theme::SPACE_2);

    let mut capabilities = Vec::new();
    for held in agent.held() {
        capabilities.push(entry(
            held.label.clone(),
            theme::TEXT,
            false,
            Some("held".into()),
            None,
            true,
        ));
    }
    for withheld in agent.withheld() {
        capabilities.push(entry(
            withheld.label.clone(),
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
                entry(
                    call.name.clone(),
                    if call.failed {
                        theme::ERROR
                    } else {
                        theme::TEXT
                    },
                    false,
                    Some(if call.failed { "failed" } else { "ok" }.into()),
                    (!call.detail.is_empty()).then(|| call.detail.clone()),
                    false,
                )
            })
            .collect()
    };
    body = body.push(section("Tools executed", tools));

    (
        title_for(agent),
        format!("{} · gen {}", agent.role.label(), agent.generation),
        body.into(),
    )
}

fn title_for(agent: &Agent) -> &'static str {
    match agent.role {
        crate::topology::model::Role::Broker => "Resident broker",
        crate::topology::model::Role::Worker => "Worker agent",
    }
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
        let (title, subtitle, _) = agents(&graph, None);
        assert_eq!(title, EMPTY_TITLE);
        assert!(subtitle.contains("Click a node"));
    }

    #[test]
    fn a_selection_names_the_role_and_generation() {
        let graph = graph();
        let (title, subtitle, _) = agents(&graph, Some("agent-1"));
        assert_eq!(title, "Worker agent");
        assert_eq!(subtitle, "worker · gen 1");
    }

    #[test]
    fn the_broker_is_named_as_the_resident() {
        let graph = graph();
        let (title, subtitle, _) = agents(&graph, Some("broker"));
        assert_eq!(title, "Resident broker");
        assert_eq!(subtitle, "resident · gen 0");
    }

    #[test]
    fn an_unknown_selection_falls_back_to_empty() {
        let graph = graph();
        let (title, _, _) = agents(&graph, Some("agent-does-not-exist"));
        assert_eq!(title, EMPTY_TITLE);
    }
}
