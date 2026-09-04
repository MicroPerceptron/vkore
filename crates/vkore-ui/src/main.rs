mod fonts;
mod logo;
mod render_backend;
mod splash;
mod theme;
mod topology;
mod transcript;

use crate::topology::{filesystem, inspector, layout, model, view};
use crate::transcript::{
    AssistantBlock, InlineMarkdownHighlighter, assistant_blocks, inline_markdown_format,
    user_message_width,
};
mod widgets;

use axgfx::Extent2d;
use axui_iced::Renderer as AxRenderer;
use iced_core::Renderer as _;
use iced_core::{
    Background, Border, Color, Element, Length, Size, Theme, alignment, border, keyboard, mouse,
    renderer::{self, Scale},
    shell,
};
use iced_runtime::user_interface::{self, UserInterface};
use iced_widget::{
    button, column, container, row, scrollable, space, stack, text, text_editor, text_input,
};
use iced_winit::{conversion, winit};
use serde_json::{Value, json};
use softbuffer::{Context as SoftContext, Surface as SoftSurface};
use std::{
    alloc::System,
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    num::NonZeroU32,
    rc::Rc,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle},
    keyboard::ModifiersState,
    window::{Window, WindowId},
};

use crate::render_backend::NativeBackend;

/// The renderer every widget in this binary is generic over.
pub(crate) type AxUi = AxRenderer<System>;

type UiElement<'a> = Element<'a, Message, Theme, AxUi>;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(750);

fn native_renderer() -> AxRenderer<System> {
    AxRenderer::new_in(renderer::Settings::default(), System)
}

#[derive(Debug, Clone)]
enum Message {
    SelectProject(usize),
    NewProject,
    EditProject,
    ConfigAction(iced_core::text::editor::Action),
    WizardField(&'static str, String),
    WizardToggle(&'static str),
    WizardChoice(&'static str, &'static str),
    WizardAddon(String),
    WizardPage(u8),
    Preflight,
    SaveProject,
    Browse(RequestKind),
    CloseEditor,
    ToggleSession,
    #[allow(dead_code)]
    CleanSession,
    PromptAction(iced_core::text::editor::Action),
    SelectText(String, iced_core::text::editor::Action),
    SendPrompt,
    CancelPrompt,
    ToggleWorking(String),
    ToggleWorkOutput(String),
    ArchiveProject,
    DuplicateProject,
    ExportProject,
    View(View),
    DecideApproval(bool),
    TopologySelect(String),
    TopologySelectPath(usize),
    TopologyPan(f32, f32),
    TopologyZoom(iced_core::Point, f32),
    TopologyZoomStep(f32),
    TopologyFit,
    TopologyReset,
    TopologyInspector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// The transcript and composer.
    Session,
    /// The agent spawn tree and its inspector.
    Agents,
    /// The workspace permission graph and its inspector.
    Filesystem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Health,
    Catalog,
    Projects,
    Approvals,
    Preflight,
    SaveProject,
    StartSession,
    StopSession,
    SendPrompt,
    CancelPrompt,
    RequestApproval,
    DecideApproval,
    RevokeGrant,
    ArchiveProject,
    DuplicateProject,
    PickWorkspace,
    PickKore,
    PickLlama,
    PickModel,
}

fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkRow {
    speaker: String,
    context: Option<String>,
    body: String,
    kind: String,
    output: Option<WorkOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkOutput {
    summary: String,
    body: String,
    failed: bool,
}

#[derive(Debug, Clone)]
struct CatalogEntry {
    kind: String,
    label: String,
}

fn event_timestamp(event: &Value) -> Option<u64> {
    event["timestamp_unix_ms"].as_u64()
}

fn format_event_time(timestamp_ms: u64) -> Option<String> {
    let utc = chrono::DateTime::from_timestamp_millis(i64::try_from(timestamp_ms).ok()?)?;
    let rendered = utc
        .with_timezone(&chrono::Local)
        .format("%I:%M:%S %p")
        .to_string();
    Some(rendered.strip_prefix('0').unwrap_or(&rendered).to_owned())
}

fn format_elapsed(milliseconds: u64) -> String {
    if milliseconds < 1_000 {
        format!("{milliseconds} ms")
    } else if milliseconds < 60_000 {
        let seconds = milliseconds as f64 / 1_000.0;
        if milliseconds < 10_000 {
            format!("{seconds:.1} s")
        } else {
            format!("{seconds:.0} s")
        }
    } else {
        let minutes = milliseconds / 60_000;
        let seconds = ((milliseconds % 60_000) + 500) / 1_000;
        format!("{minutes}m {seconds}s")
    }
}

fn embedded_json(value: &Value) -> Option<Value> {
    match value {
        Value::String(value) => serde_json::from_str(value).ok(),
        Value::Object(_) | Value::Array(_) => Some(value.clone()),
        _ => None,
    }
}

fn broker_catalog(events: &[&Value]) -> BTreeMap<u64, CatalogEntry> {
    let mut entries = BTreeMap::new();
    for event in events {
        if event["kind"] != "inference"
            || event["payload"]["kind"] != "Input"
            || event["payload"]["role"] != "Broker"
        {
            continue;
        }
        let Some(input) = embedded_json(&event["payload"]["payload"]) else {
            continue;
        };
        for tuple in input["offer_catalog"].as_array().into_iter().flatten() {
            let Some(tuple) = tuple.as_array() else {
                continue;
            };
            if let (Some(index), Some(kind), Some(label)) = (
                tuple.first().and_then(Value::as_u64),
                tuple.get(1).and_then(Value::as_str),
                tuple.get(2).and_then(Value::as_str),
            ) {
                entries.insert(
                    index,
                    CatalogEntry {
                        kind: kind.into(),
                        label: label.into(),
                    },
                );
            }
        }
    }
    entries
}

fn display_resource(label: &str) -> String {
    label
        .strip_prefix("workspace/")
        .unwrap_or(label)
        .trim_start_matches('/')
        .to_owned()
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn format_broker_action(action: &Value, catalog: &BTreeMap<u64, CatalogEntry>) -> String {
    let action_name = action["action"].as_str().unwrap_or("decision");
    if action_name == "intent_claimed" {
        let objects = action["objects"].as_u64().unwrap_or_default();
        let turns = action["turns"].as_u64().unwrap_or_default();
        return format!("intent claimed · ceiling {objects} objects, {turns} turns");
    }
    if action_name == "install_scoped_object" {
        let path = action["path"].as_str().unwrap_or("workspace object");
        return format!("install scoped object · {path}");
    }
    if action_name == "create_agent" {
        let selected = action["selectors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .filter_map(|index| catalog.get(&index).map(|entry| (index, entry)))
            .collect::<Vec<_>>();
        let tools = selected
            .iter()
            .filter(|(_, entry)| entry.kind == "x")
            .map(|(_, entry)| display_resource(&entry.label))
            .collect::<Vec<_>>();
        let resources = selected
            .iter()
            .filter(|(_, entry)| matches!(entry.kind.as_str(), "o" | "v"))
            .map(|(_, entry)| display_resource(&entry.label))
            .collect::<Vec<_>>();
        let unresolved = action["selectors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .filter(|index| !catalog.contains_key(index))
            .map(|index| format!("#{index}"))
            .collect::<Vec<_>>();
        let mut visible_resources = resources;
        visible_resources.extend(unresolved);
        return format!(
            "Tool_use: create_agent(Agent 1, [{}], [{}])",
            tools.join(", "),
            visible_resources.join(", ")
        );
    }
    if action_name == "refused" {
        let reason = action["reason"]
            .as_str()
            .or_else(|| action["detail"].as_str())
            .unwrap_or("the requested authority was not available");
        return format!("Refused: {reason}");
    }

    let arguments = action
        .as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter(|(key, _)| key.as_str() != "action")
        .filter_map(|(key, value)| scalar(value).map(|value| format!("{key}: {value}")))
        .collect::<Vec<_>>();
    format!("Tool_use: {action_name}({})", arguments.join(", "))
}

fn clean_tool_argument(argument: &str) -> String {
    let argument = argument.trim();
    for prefix in ["Path(\"", "Object(\"", "Text(\""] {
        if let Some(value) = argument
            .strip_prefix(prefix)
            .and_then(|value| value.strip_suffix("\")"))
        {
            return value.strip_prefix("workspace/").unwrap_or(value).to_owned();
        }
    }
    let argument = argument.trim_matches('"');
    argument
        .strip_prefix("workspace/")
        .unwrap_or(argument)
        .to_owned()
}

fn tool_arguments(value: &Value) -> Vec<String> {
    if let Some(arguments) = value.as_array() {
        return arguments
            .iter()
            .filter_map(|argument| match argument {
                Value::String(argument) => Some(clean_tool_argument(argument)),
                Value::Object(argument) => argument
                    .get("value")
                    .or_else(|| argument.get("path"))
                    .and_then(Value::as_str)
                    .map(clean_tool_argument),
                _ => scalar(argument),
            })
            .collect();
    }
    let Some(arguments) = value.as_str() else {
        return Vec::new();
    };
    arguments
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(", ")
        .filter(|argument| !argument.is_empty())
        .map(clean_tool_argument)
        .collect()
}

fn decision_payload(event: &Value) -> Option<Value> {
    let payload = &event["payload"];
    if let Some(inner) = embedded_json(&payload["payload"]) {
        Some(inner)
    } else if payload.is_object() {
        Some(payload.clone())
    } else {
        None
    }
}

fn format_agent_tool_call(call: &Value) -> Option<String> {
    if call["action"]
        .as_str()
        .is_some_and(|action| action != "tool_call")
    {
        return None;
    }
    let command = call["command"].as_str().or_else(|| call["tool"].as_str())?;
    let arguments = tool_arguments(&call["arguments"]);
    Some(format!(
        "{command}{}",
        if arguments.is_empty() {
            String::new()
        } else {
            format!(" {}", arguments.join(" "))
        }
    ))
}

fn action_summary(
    role: &str,
    action: &Value,
    catalog: &BTreeMap<u64, CatalogEntry>,
) -> Option<String> {
    if role.eq_ignore_ascii_case("broker") {
        let rendered = format_broker_action(action, catalog);
        return Some(
            rendered
                .strip_prefix("Tool_use: ")
                .unwrap_or(&rendered)
                .to_owned(),
        );
    }
    if role.eq_ignore_ascii_case("agent") {
        return format_agent_tool_call(action);
    }
    None
}

/// Project only a schema-shaped model action into the public work timeline.
/// Raw completions are untrusted and may contain response text or tool output,
/// so never render them verbatim.
fn raw_model_proposal(event: &Value, catalog: &BTreeMap<u64, CatalogEntry>) -> Option<WorkRow> {
    let payload = &event["payload"];
    if event["kind"] != "inference" || payload["kind"] != "RawModelCompletion" {
        return None;
    }
    let role = payload["role"].as_str()?;
    let action = embedded_json(&payload["payload"])?;
    let action_name = action["action"].as_str()?;
    if !role.eq_ignore_ascii_case("broker") || action_name == "respond" {
        return None;
    }
    Some(WorkRow {
        speaker: "Broker".into(),
        context: None,
        body: action_summary(role, &action, catalog)?,
        kind: "decision".into(),
        output: None,
    })
}

fn model_validation_row(event: &Value, catalog: &BTreeMap<u64, CatalogEntry>) -> Option<WorkRow> {
    let payload = &event["payload"];
    if payload["kind"] != "Validation" {
        return None;
    }
    let accepted = payload["accepted"].as_bool()?;
    if accepted {
        return None;
    }
    let role = payload["role"].as_str()?;
    let action = embedded_json(&payload["payload"])?;
    let summary = action_summary(role, &action, catalog)?;
    let detail = payload["detail"].as_str().unwrap_or("policy validation");
    Some(WorkRow {
        speaker: "Denied".into(),
        context: None,
        body: format!("{summary} refused\n{detail}"),
        kind: "error".into(),
        output: None,
    })
}

fn agent_name(event: &Value) -> String {
    for candidate in [
        &event["agent_name"],
        &event["payload"]["agent_name"],
        &event["payload"]["agent_label"],
    ] {
        if let Some(name) = candidate.as_str().filter(|name| !name.is_empty()) {
            return name.to_owned();
        }
    }
    let generation = event["generation"]
        .as_u64()
        .or_else(|| event["payload"]["generation"].as_u64())
        .unwrap_or(1);
    format!("Agent {generation}")
}

fn work_output(result: &Value) -> WorkOutput {
    let status = result["result"]
        .as_str()
        .or_else(|| result["status"].as_str())
        .unwrap_or("output")
        .to_ascii_lowercase();
    let body = result["text"]
        .as_str()
        .or_else(|| result["output"].as_str())
        .unwrap_or_default()
        .to_owned();
    let lines = body.lines().count();
    let failed = matches!(
        status.as_str(),
        "refused" | "revoked" | "cancelled" | "failed" | "error"
    );
    WorkOutput {
        summary: format!(
            "{status} · {lines} line{}",
            if lines == 1 { "" } else { "s" }
        ),
        body,
        failed,
    }
}

fn work_output_height(text: &str) -> f32 {
    let lines = text.lines().count().max(1) as f32;
    (lines * 19.2 + 16.0).clamp(35.2, 180.0)
}

fn capability_row(event: &Value) -> WorkRow {
    let payload = &event["payload"];
    let state = payload["state"]
        .as_str()
        .unwrap_or("installed")
        .to_ascii_lowercase();
    let mut details = Vec::new();
    if let Some(kind) = payload["kind"]
        .as_str()
        .or_else(|| payload["capability"].as_str())
    {
        details.push(kind.to_ascii_lowercase());
    }
    let mut rights = payload["rights"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if let Some(right) = payload["rights"].as_str() {
        rights.push(right.to_ascii_lowercase());
    }
    details.extend(rights);
    let tools = payload["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if details.is_empty() && !tools.is_empty() {
        details.push("object".into());
    }
    details.extend(tools);
    WorkRow {
        speaker: "Authority".into(),
        context: None,
        body: if details.is_empty() {
            state
        } else {
            let first = details.remove(0);
            let remainder = if details.is_empty() {
                String::new()
            } else {
                format!(" · {}", details.join(" · "))
            };
            format!("{state} {first}{remainder}")
        },
        kind: "authority".into(),
        output: None,
    }
}

fn same_tool_exchange(call: &Value, result: &Value) -> bool {
    if let (Some(call_generation), Some(result_generation)) =
        (call["generation"].as_u64(), result["generation"].as_u64())
        && call_generation != result_generation
    {
        return false;
    }
    let call_payload = decision_payload(call).unwrap_or(Value::Null);
    let result_payload = decision_payload(result).unwrap_or(Value::Null);
    match (
        call_payload["call_id"].as_u64(),
        result_payload["call_id"].as_u64(),
    ) {
        (Some(call), Some(result)) => call == result,
        _ => true,
    }
}

fn meaningful_error(event: &Value) -> String {
    let payload = &event["payload"];
    payload["message"]
        .as_str()
        .or_else(|| payload["detail"].as_str())
        .or_else(|| payload["error"].as_str())
        .or_else(|| payload.as_str())
        .unwrap_or("The request could not be completed.")
        .to_owned()
}

fn work_rows(events: &[&Value]) -> Vec<WorkRow> {
    let catalog = broker_catalog(events);
    let mut rows = Vec::new();
    let has_raw_broker_proposal = events.iter().any(|event| {
        event["kind"] == "inference"
            && event["payload"]["kind"] == "RawModelCompletion"
            && event["payload"]["role"] == "Broker"
    });

    let mut consumed_results = BTreeSet::new();
    let mut seen_tool_calls: BTreeMap<(String, String), Option<u64>> = BTreeMap::new();
    for (event_index, event) in events.iter().enumerate() {
        let event_kind = event["kind"].as_str().unwrap_or("event");
        let row = raw_model_proposal(event, &catalog)
            .or_else(|| model_validation_row(event, &catalog))
            .or_else(|| match event_kind {
                "broker_proposal" if !has_raw_broker_proposal => {
                    decision_payload(event).map(|action| WorkRow {
                        speaker: "Broker".into(),
                        context: None,
                        body: format_broker_action(&action, &catalog),
                        kind: "decision".into(),
                        output: None,
                    })
                }
                "capability" => Some(capability_row(event)),
                "tool_call" => decision_payload(event).and_then(|call| {
                    let speaker = agent_name(event);
                    let body = format_agent_tool_call(&call).unwrap_or_else(|| "tool".into());
                    let call_id = call["call_id"].as_u64();
                    let signature = (speaker.clone(), body.clone());
                    if seen_tool_calls
                        .get(&signature)
                        .is_some_and(|seen_id| call_id.is_none() || *seen_id == call_id)
                    {
                        return None;
                    }
                    seen_tool_calls.insert(signature, call_id);
                    let result = events
                        .iter()
                        .enumerate()
                        .skip(event_index + 1)
                        .find(|(index, candidate)| {
                            !consumed_results.contains(index)
                                && candidate["kind"] == "tool_result"
                                && same_tool_exchange(event, candidate)
                        })
                        .and_then(|(index, result)| {
                            consumed_results.insert(index);
                            decision_payload(result)
                        });
                    let domain = call["execution_domain"]
                        .as_str()
                        .unwrap_or("guest")
                        .replace('_', " ");
                    WorkRow {
                        speaker,
                        context: Some(format!("tool · {domain}")),
                        body,
                        kind: "decision".into(),
                        output: result.as_ref().map(work_output),
                    }
                    .into()
                }),
                "tool_result" if !consumed_results.contains(&event_index) => {
                    decision_payload(event).map(|result| WorkRow {
                        speaker: agent_name(event),
                        context: Some("tool · guest".into()),
                        body: "Tool response".into(),
                        kind: "result".into(),
                        output: Some(work_output(&result)),
                    })
                }
                "error" => Some(WorkRow {
                    speaker: "Denied".into(),
                    context: None,
                    body: meaningful_error(event),
                    kind: "error".into(),
                    output: None,
                }),
                _ => None,
            });
        if let Some(row) = row {
            rows.push(row);
        }
    }
    rows
}

fn session_work_rows(events: &[Value]) -> Vec<WorkRow> {
    let mut rows = Vec::new();
    for user in events.iter().filter(|event| event["kind"] == "user_prompt") {
        let prompt_id = user["prompt_id"].as_str().unwrap_or_default();
        let prompt_events = events
            .iter()
            .filter(|event| event["prompt_id"].as_str() == Some(prompt_id))
            .collect::<Vec<_>>();
        rows.extend(work_rows(&prompt_events));
    }
    rows
}

/// The surface a dialog sits on.
fn glass_surface() -> iced_widget::container::Style {
    iced_widget::container::Style {
        background: Some(Background::Color(theme::glass())),
        border: Border {
            color: theme::glass_border(),
            width: 1.0,
            radius: theme::RADIUS_GLASS.into(),
        },
        shadow: theme::no_shadow(),
        ..Default::default()
    }
}

/// Records Axiom's painter-ordered backdrop filter before drawing `content`.
fn backdrop_filter<'a>(content: UiElement<'a>, radius: f32, corner_radius: f32) -> UiElement<'a> {
    use iced_core::layout::{self, Layout};
    use iced_core::mouse;
    use iced_core::widget::tree::{self, Tree};
    use iced_core::widget::{Operation, Widget};
    use iced_core::{Event, Rectangle, Shell, Vector};

    struct Backdrop<'a> {
        content: UiElement<'a>,
        radius: f32,
        corner_radius: f32,
    }

    impl Widget<Message, Theme, AxUi> for Backdrop<'_> {
        fn tag(&self) -> tree::Tag {
            self.content.as_widget().tag()
        }

        fn state(&self) -> tree::State {
            self.content.as_widget().state()
        }

        fn diff(&mut self, tree: &mut Tree) {
            self.content.as_widget_mut().diff(tree);
        }

        fn size(&self) -> Size<Length> {
            self.content.as_widget().size()
        }

        fn layout(
            &mut self,
            tree: &mut Tree,
            renderer: &AxUi,
            limits: &layout::Limits,
        ) -> layout::Node {
            self.content.as_widget_mut().layout(tree, renderer, limits)
        }

        fn draw(
            &self,
            tree: &Tree,
            renderer: &mut AxUi,
            theme: &Theme,
            style: &renderer::Style,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
        ) {
            let bounds = layout.bounds();
            renderer.recorder_mut().backdrop_blur(
                axui_core::Rect::new(bounds.x, bounds.y, bounds.width, bounds.height),
                axui_core::CornerRadii {
                    top_left: self.corner_radius,
                    top_right: self.corner_radius,
                    bottom_right: self.corner_radius,
                    bottom_left: self.corner_radius,
                },
                self.radius,
            );
            self.content
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }

        fn operate(
            &mut self,
            tree: &mut Tree,
            layout: Layout<'_>,
            renderer: &AxUi,
            operation: &mut dyn Operation,
        ) {
            self.content
                .as_widget_mut()
                .operate(tree, layout, renderer, operation);
        }

        fn update(
            &mut self,
            tree: &mut Tree,
            event: &Event,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &AxUi,
            shell: &mut Shell<'_, Message>,
            viewport: &Rectangle,
        ) {
            self.content
                .as_widget_mut()
                .update(tree, event, layout, cursor, renderer, shell, viewport);
        }

        fn mouse_interaction(
            &self,
            tree: &Tree,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
            renderer: &AxUi,
        ) -> mouse::Interaction {
            self.content
                .as_widget()
                .mouse_interaction(tree, layout, cursor, viewport, renderer)
        }

        fn overlay<'b>(
            &'b mut self,
            tree: &'b mut Tree,
            layout: Layout<'b>,
            renderer: &AxUi,
            viewport: &Rectangle,
            translation: Vector,
        ) -> Option<iced_core::overlay::Element<'b, Message, Theme, AxUi>> {
            self.content
                .as_widget_mut()
                .overlay(tree, layout, renderer, viewport, translation)
        }
    }

    Element::new(Backdrop {
        content,
        radius,
        corner_radius,
    })
}

/// One shared glass edge between a dialog and its header or footer.
fn glass_bottom_edge() -> iced_widget::container::Style {
    let mut style = glass_surface();
    style.background = None;
    style.border = Border::default();
    style.border_sides = border::Sides::default().bottom(border::side(theme::glass_border(), 1.0));
    style
}

fn glass_top_edge() -> iced_widget::container::Style {
    let mut style = glass_bottom_edge();
    style.border_sides = border::Sides::default().top(border::side(theme::glass_border(), 1.0));
    style
}

/// A dialog header: a display title over a mono subtitle, and a way out.
fn glass_header<'a>(
    title: &'static str,
    subtitle: &'static str,
    close: Option<Message>,
) -> UiElement<'a> {
    let mut heading = row![
        column![
            widgets::display(title),
            widgets::mono(subtitle, theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_1)
        .width(Length::Fill)
    ]
    .align_y(alignment::Vertical::Center);
    if let Some(close) = close {
        heading = heading.push(
            button(widgets::control("x"))
                .height(26)
                .padding([0, 9])
                .style(widgets::button_style(widgets::Button::Ghost))
                .on_press(close),
        );
    }
    container(heading)
        .padding([14, 20])
        .width(Length::Fill)
        .style(|_| glass_bottom_edge())
        .into()
}

fn divider_right(background: u32) -> iced_widget::container::Style {
    theme::panel(background, None, 0.0)
        .border_sides(border::Sides::default().right(border::side(theme::rgb(theme::BORDER), 1.0)))
}

fn divider_bottom(background: u32) -> iced_widget::container::Style {
    theme::panel(background, None, 0.0)
        .border_sides(border::Sides::default().bottom(border::side(theme::rgb(theme::BORDER), 1.0)))
}

fn divider_left(background: u32) -> iced_widget::container::Style {
    theme::panel(background, None, 0.0)
        .border_sides(border::Sides::default().left(border::side(theme::rgb(theme::BORDER), 1.0)))
}

fn input_style(
    _theme: &Theme,
    status: iced_widget::text_input::Status,
) -> iced_widget::text_input::Style {
    let focused = matches!(status, iced_widget::text_input::Status::Focused { .. });
    iced_widget::text_input::Style {
        background: Background::Color(theme::rgb(theme::BACKGROUND)),
        border: Border {
            color: rgb(if focused { 0x596573 } else { 0x343b45 }),
            width: 1.0,
            radius: 8.0.into(),
        },
        placeholder: theme::rgb(theme::BORDER_DIM),
        value: theme::rgb(theme::TEXT),
        selection: theme::rgb(theme::MAGENTA),
    }
}

struct Request {
    kind: RequestKind,
    method: &'static str,
    path: String,
    body: Option<Value>,
}

#[derive(Debug)]
enum UserEvent {
    Response(RequestKind, Result<ApiResponse, String>),
    Poll(Result<Vec<Value>, String>),
}

#[derive(Debug)]
struct ApiResponse {
    content_type: String,
    body: Vec<u8>,
}

impl ApiResponse {
    fn json(self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|error| error.to_string())
    }
}

struct Network {
    sender: mpsc::Sender<Request>,
    session: mpsc::Sender<PollTarget>,
}

struct PollTarget {
    session: Option<String>,
    active: bool,
}

impl Network {
    fn start(server: SocketAddr, proxy: EventLoopProxy<UserEvent>) -> Self {
        let (sender, receiver) = mpsc::channel::<Request>();
        let request_proxy = proxy.clone();
        thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                let result = http(server, request.method, &request.path, request.body.as_ref());
                let _ = request_proxy.send_event(UserEvent::Response(request.kind, result));
            }
        });

        let (session_sender, session_receiver) = mpsc::channel::<PollTarget>();
        thread::spawn(move || {
            let mut session = None;
            let mut active = false;
            let mut cursor = 0u64;
            loop {
                while let Ok(next) = session_receiver.try_recv() {
                    if session != next.session {
                        session = next.session;
                        cursor = 0;
                    }
                    active = next.active;
                }
                let Some(id) = session.as_deref() else {
                    match session_receiver.recv() {
                        Ok(next) => {
                            if session != next.session {
                                session = next.session;
                                cursor = 0;
                            }
                            active = next.active;
                            continue;
                        }
                        Err(_) => break,
                    }
                };
                match http(
                    server,
                    "GET",
                    &format!("/api/v1/sessions/{id}/events?after={cursor}"),
                    None,
                )
                .and_then(parse_sse)
                {
                    Ok(events) => {
                        if let Some(sequence) = events
                            .iter()
                            .filter_map(|event| event["sequence"].as_u64())
                            .max()
                        {
                            cursor = cursor.max(sequence);
                        }
                        if !events.is_empty() {
                            let _ = proxy.send_event(UserEvent::Poll(Ok(events)));
                        }
                    }
                    Err(error) => {
                        let _ = proxy.send_event(UserEvent::Poll(Err(error)));
                    }
                }
                thread::sleep(if active {
                    ACTIVE_POLL_INTERVAL
                } else {
                    IDLE_POLL_INTERVAL
                });
            }
        });
        Self {
            sender,
            session: session_sender,
        }
    }

    fn request(&self, request: Request) {
        let _ = self.sender.send(request);
    }

    fn poll(&self, session: Option<String>, active: bool) {
        let _ = self.session.send(PollTarget { session, active });
    }
}

struct Model {
    network: Network,
    connected: bool,
    projects: Vec<Value>,
    catalog: Value,
    approvals: Value,
    selected: Option<usize>,
    launch_project: Option<String>,
    auto_open_pending: bool,
    session: Option<String>,
    prompt_id: Option<String>,
    prompt: text_editor::Content<AxRenderer<System>>,
    events: Vec<Value>,
    selectable_text: BTreeMap<String, text_editor::Content<AxRenderer<System>>>,
    expanded_work: BTreeSet<String>,
    expanded_outputs: BTreeSet<String>,
    view: View,
    topology: view::State,
    /// When the process started, so the splash knows how far along it is.
    booted_at: Option<Instant>,
    topology_graph: model::Graph,
    topology_placements: Vec<layout::Placement>,
    topology_filesystem: filesystem::Filesystem,
    topology_agent_bands: Vec<layout::Placement>,
    topology_path_bands: Vec<layout::Placement>,
    viewport: Size,
    editor: Option<text_editor::Content<AxRenderer<System>>>,
    wizard_page: u8,
    preflight: Option<String>,
    approval: Option<Value>,
    status: String,
    busy: bool,
    restart_after_stop: bool,
}

impl Model {
    #[cfg(test)]
    fn new(network: Network) -> Self {
        Self::with_launch_project(network, None)
    }

    fn with_launch_project(network: Network, launch_project: Option<String>) -> Self {
        let model = Self {
            network,
            connected: false,
            projects: Vec::new(),
            catalog: Value::Null,
            approvals: Value::Null,
            selected: None,
            launch_project,
            auto_open_pending: true,
            session: None,
            prompt_id: None,
            prompt: text_editor::Content::new(),
            events: Vec::new(),
            selectable_text: BTreeMap::new(),
            expanded_work: BTreeSet::new(),
            expanded_outputs: BTreeSet::new(),
            view: View::Session,
            topology: view::State::default(),
            booted_at: Some(Instant::now()),
            topology_graph: model::Graph::default(),
            topology_placements: Vec::new(),
            topology_filesystem: filesystem::Filesystem::default(),
            topology_agent_bands: Vec::new(),
            topology_path_bands: Vec::new(),
            viewport: Size::new(1280.0, 800.0),
            editor: None,
            wizard_page: 0,
            preflight: None,
            approval: None,
            status: "Connecting to the vKore daemon…".into(),
            busy: false,
            restart_after_stop: false,
        };
        model.reload();
        model
    }

    fn reload(&self) {
        for (kind, path) in [
            (RequestKind::Health, "/api/v1/health"),
            (RequestKind::Catalog, "/api/v1/catalog"),
            (RequestKind::Projects, "/api/v1/projects"),
            (RequestKind::Approvals, "/api/v1/approvals"),
        ] {
            self.network.request(Request {
                kind,
                method: "GET",
                path: path.into(),
                body: None,
            });
        }
    }

    /// Refreshes everything derived from the event stream.
    ///
    /// The selectable transcript and the topology graph are both projections
    /// of the same events. Refreshing them together is what keeps a view from
    /// showing one and not the other.
    fn events_changed(&mut self) {
        self.sync_selectable_text();
        self.refresh_topology();
    }

    /// Rebuilds the derived graph and its placement.
    ///
    /// Derivation walks the whole stream, so it happens when the stream
    /// changes rather than once per frame.
    fn refresh_topology(&mut self) {
        self.topology_graph = model::Graph::from_events(&self.events, self.session.is_some());
        self.topology_placements =
            layout::tidy(&self.topology_graph.parents(), layout::Metrics::AGENTS);
        self.topology_filesystem =
            filesystem::Filesystem::from_events(&self.events, &self.topology_graph);
        let (agents, paths) = self.topology_filesystem.place(&self.topology_graph);
        self.topology_agent_bands = agents;
        self.topology_path_bands = paths;
        // A changed lineage invalidates a fit taken against the old one.
        self.topology.fitted = false;
        if let Some(selected) = self.topology.selected.clone()
            && self.topology_graph.index_of(&selected).is_none()
        {
            self.topology.selected = None;
        }
        if self.topology.selected.is_none()
            && let Some(root) = self.topology_graph.agents.first()
        {
            self.topology.selected = Some(root.id.clone());
        }
    }

    /// The area a topology canvas gets, once the shell has taken its share.
    fn canvas_area(&self, viewport: Size) -> Size {
        let sidebar = if viewport.width <= 720.0 { 0.0 } else { 248.0 };
        let inspector = if self.topology.inspector { 300.0 } else { 0.0 };
        Size::new(
            (viewport.width - sidebar - inspector).max(1.0),
            // The graph sits below both the 64px harness header and the 52px
            // embedded topology toolbar.
            (viewport.height - 116.0).max(1.0),
        )
    }

    /// Fits a topology view the first time it is shown at a given size.
    ///
    /// Fitting needs the viewport, which only the shell knows, so it happens
    /// here rather than while building the view.
    fn fit_topology(&mut self, viewport: Size) {
        self.viewport = viewport;
        if self.view == View::Session || self.topology.fitted {
            return;
        }
        let area = self.canvas_area(viewport);
        let placements: Vec<layout::Placement> = if self.view == View::Agents {
            // The handoff's fit routine reserves 132px per chip-bearing node
            // even though the visible card metric is 124px.
            self.topology_placements
                .iter()
                .map(|placement| layout::Placement {
                    height: 132.0,
                    ..*placement
                })
                .collect()
        } else {
            self.topology_agent_bands
                .iter()
                .chain(self.topology_path_bands.iter())
                .copied()
                .collect()
        };
        self.topology.fit(&placements, area);
    }

    fn project(&self) -> Option<&Value> {
        self.selected.and_then(|index| self.projects.get(index))
    }

    fn update(&mut self, message: Message) {
        match message {
            Message::SelectProject(index) => {
                if self.session.is_none() {
                    self.selected = Some(index);
                    self.events.clear();
                    self.expanded_work.clear();
                    self.expanded_outputs.clear();
                    self.selectable_text.clear();
                    self.status = "Opening a fresh isolated Kore sandbox…".into();
                    self.update(Message::ToggleSession);
                }
            }
            Message::NewProject => {
                // The published control stays visually available while a
                // session is active, but its handler is intentionally a no-op.
                if self.session.is_some() {
                    return;
                }
                let root = std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.editor = Some(
                    text_editor::Content::with_text(&serde_json::to_string_pretty(&json!({
                        "schema_version":1,
                        "id":"my-project",
                        "name":"My project",
                        "workspace":{"root":root,"state":"","access":"read_only","exclusions":[".git/objects"],"max_write_bytes":0,"max_write_files":0},
                        "inference":{"backend":"deterministic","kore_image":null,"llama_server":null,"model":null,"extra_args":[],"context_tokens":32768,"output_tokens":512,"threads":4,"slots":3},
                        "addons":[],
                        "approvals":{"default_scope":"once","confirm_external_writes":true,"confirm_destructive":true,"standing_policy_enabled":false},
                        "retention":{"transcript_days":30,"clean_kv_on_close":true,"retain_raw_model_output":false},
                        "resources":{"memory_mib":2048,"virtual_cpus":2,"prompt_seconds":300,"agent_limit":4},
                        "archived":false
                    }))
                    .unwrap()),
                );
                self.wizard_page = 0;
                self.preflight = None;
            }
            Message::EditProject => {
                if let Some(project) = self.project() {
                    self.editor = Some(text_editor::Content::with_text(
                        &serde_json::to_string_pretty(project).unwrap_or_default(),
                    ));
                    self.wizard_page = 0;
                    self.preflight = None;
                }
            }
            Message::ConfigAction(action) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.perform(action);
                    self.preflight = None;
                }
            }
            Message::WizardField(pointer, value) => self.update_config(pointer, &value),
            Message::WizardToggle(pointer) => self.toggle_config(pointer),
            Message::WizardChoice(pointer, value) => self.update_config(pointer, value),
            Message::WizardAddon(id) => self.toggle_addon(&id),
            Message::WizardPage(page) => {
                self.wizard_page = page.min(4);
                if self.wizard_page == 4 {
                    self.preflight = None;
                }
            }
            Message::Preflight => self.submit_editor(RequestKind::Preflight),
            Message::SaveProject => self.submit_editor(RequestKind::SaveProject),
            Message::Browse(kind) => {
                let directory = kind == RequestKind::PickWorkspace;
                self.network.request(Request {
                    kind,
                    method: "POST",
                    path: "/api/v1/picker".into(),
                    body: Some(json!({"kind":if directory {"directory"} else {"file"}})),
                });
            }
            Message::CloseEditor => {
                self.editor = None;
                self.preflight = None;
            }
            Message::ToggleSession => {
                if let Some(session) = self.session.clone() {
                    self.busy = true;
                    self.network.request(Request {
                        kind: RequestKind::StopSession,
                        method: "DELETE",
                        path: format!("/api/v1/sessions/{session}"),
                        body: None,
                    });
                } else if let Some(project) = self.project() {
                    if let Some(addon) = project["addons"]
                        .as_array()
                        .and_then(|addons| addons.iter().find(|addon| addon["enabled"] == true))
                    {
                        let addon_id = addon["id"].as_str().unwrap_or_default().to_owned();
                        let tool = addon["tool_ceiling"]
                            .as_array()
                            .and_then(|tools| tools.first())
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let project_id = project["id"].as_str().unwrap_or_default().to_owned();
                        self.busy = true;
                        self.network.request(Request {
                            kind: RequestKind::RequestApproval,
                            method: "POST",
                            path: format!("/api/v1/projects/{project_id}/approvals"),
                            body: Some(json!({"session_id":"project-start","addon_id":addon_id,"tool":tool})),
                        });
                    } else {
                        self.start_session();
                    }
                }
            }
            Message::CleanSession => {
                if self.session.is_some() {
                    self.restart_after_stop = true;
                    self.update(Message::ToggleSession);
                } else {
                    self.update(Message::ToggleSession);
                }
            }
            Message::PromptAction(action) => self.prompt.perform(action),
            Message::SelectText(key, action) => {
                if !action.is_edit()
                    && let Some(content) = self.selectable_text.get_mut(&key)
                {
                    content.perform(action);
                }
            }
            Message::SendPrompt => self.send_prompt(),
            Message::CancelPrompt => {
                if let (Some(session), Some(prompt_id)) = (&self.session, &self.prompt_id) {
                    self.network.request(Request {
                        kind: RequestKind::CancelPrompt,
                        method: "POST",
                        path: format!("/api/v1/sessions/{session}/prompts/{prompt_id}/cancel"),
                        body: Some(json!({"api_version":1,"prompt_id":prompt_id})),
                    });
                }
            }
            Message::ToggleWorking(prompt_id) => {
                if !self.expanded_work.remove(&prompt_id) {
                    self.expanded_work.insert(prompt_id);
                }
            }
            Message::ToggleWorkOutput(key) => {
                if !self.expanded_outputs.remove(&key) {
                    self.expanded_outputs.insert(key);
                }
            }
            Message::ArchiveProject => {
                if let Some(project) = self.project() {
                    let id = project["id"].as_str().unwrap_or_default();
                    self.network.request(Request {
                        kind: RequestKind::ArchiveProject,
                        method: "POST",
                        path: format!("/api/v1/projects/{id}/archive"),
                        body: Some(json!({"archived":true})),
                    });
                }
            }
            Message::DuplicateProject => {
                if let Some(project) = self.project() {
                    let source = project["id"].as_str().unwrap_or_default();
                    let suffix = now_id();
                    self.network.request(Request {
                        kind: RequestKind::DuplicateProject,
                        method: "POST",
                        path: format!("/api/v1/projects/{source}/duplicate"),
                        body: Some(json!({
                            "id":format!("{source}-copy-{suffix}"),
                            "name":format!("{} copy", project["name"].as_str().unwrap_or("Project"))
                        })),
                    });
                }
            }
            Message::ExportProject => {
                if let Some(project) = self.project() {
                    let id = project["id"].as_str().unwrap_or("project");
                    let path = format!("{id}.vkore.json");
                    match std::fs::write(
                        &path,
                        serde_json::to_vec_pretty(project).unwrap_or_default(),
                    ) {
                        Ok(()) => self.status = format!("Exported {path}"),
                        Err(error) => self.status = format!("Export failed: {error}"),
                    }
                }
            }
            Message::View(value) => {
                // Entering a topology view fits it once; returning keeps the
                // reader's own pan and zoom.
                if value != View::Session && self.view == View::Session {
                    self.topology.fitted = false;
                }
                self.view = value;
            }
            Message::TopologySelectPath(index) => {
                self.topology.selected = None;
                self.topology.path = (self.topology.path != Some(index)).then_some(index);
            }
            Message::TopologySelect(id) => {
                self.topology.path = None;
                self.topology.selected =
                    (self.topology.selected.as_deref() != Some(id.as_str())).then_some(id);
            }
            Message::TopologyPan(dx, dy) => {
                self.topology.tx += dx;
                self.topology.ty += dy;
            }
            Message::TopologyZoom(pointer, factor) => {
                self.topology.zoom_about(pointer, factor);
            }
            Message::TopologyZoomStep(factor) => {
                let area = self.canvas_area(self.viewport);
                self.topology.zoom_about(
                    iced_core::Point::new(area.width / 2.0, area.height / 2.0),
                    factor,
                );
                self.topology.fitted = true;
            }
            Message::TopologyFit => self.topology.fitted = false,
            Message::TopologyReset => {
                let selected = self.topology.selected.take();
                self.topology = view::State::default();
                self.topology.selected = selected;
            }
            Message::TopologyInspector => {
                self.topology.inspector = !self.topology.inspector;
            }
            Message::DecideApproval(allow) => {
                if let Some(request) = self.approval.as_ref() {
                    let id = request["id"].as_str().unwrap_or_default();
                    self.busy = true;
                    self.network.request(Request {
                        kind: RequestKind::DecideApproval,
                        method: "POST",
                        path: format!("/api/v1/approvals/{id}/decision"),
                        body: Some(
                            json!({"allow":allow,"lifetime":"once","ui_session":"axiom-ui"}),
                        ),
                    });
                }
            }
        }
    }

    fn submit_editor(&mut self, kind: RequestKind) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        match serde_json::from_str(&source.text()) {
            Ok(body) => {
                self.busy = true;
                self.network.request(Request {
                    kind,
                    method: "POST",
                    path: if kind == RequestKind::Preflight {
                        "/api/v1/projects/preflight".into()
                    } else {
                        "/api/v1/projects".into()
                    },
                    body: Some(body),
                });
            }
            Err(error) => self.preflight = Some(format!("Invalid JSON: {error}")),
        }
    }

    fn update_config(&mut self, pointer: &str, value: &str) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&source.text()) else {
            return;
        };
        let Some(slot) = config.pointer_mut(pointer) else {
            return;
        };
        *slot = match slot {
            Value::Number(_) => value
                .parse::<u64>()
                .map(Value::from)
                .unwrap_or_else(|_| Value::from(0)),
            Value::Array(_) if pointer == "/inference/extra_args" => Value::Array(
                value
                    .split_whitespace()
                    .map(|part| Value::String(part.to_owned()))
                    .collect(),
            ),
            Value::Null if value.is_empty() => Value::Null,
            _ => Value::String(value.to_owned()),
        };
        self.editor = Some(text_editor::Content::with_text(
            &serde_json::to_string_pretty(&config).unwrap_or_default(),
        ));
        self.preflight = None;
    }

    fn toggle_config(&mut self, pointer: &str) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&source.text()) else {
            return;
        };
        let Some(slot) = config.pointer_mut(pointer) else {
            return;
        };
        *slot = Value::Bool(!slot.as_bool().unwrap_or(false));
        self.editor = Some(text_editor::Content::with_text(
            &serde_json::to_string_pretty(&config).unwrap_or_default(),
        ));
        self.preflight = None;
    }

    fn toggle_addon(&mut self, id: &str) {
        let Some(source) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&source.text()) else {
            return;
        };
        let Some(addons) = config["addons"].as_array_mut() else {
            return;
        };
        if let Some(addon) = addons.iter_mut().find(|addon| addon["id"] == id) {
            addon["enabled"] = Value::Bool(!addon["enabled"].as_bool().unwrap_or(false));
        } else if let Some(catalog) = self.catalog["addons"]
            .as_array()
            .and_then(|entries| entries.iter().find(|addon| addon["id"] == id))
        {
            let tools = catalog["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tool| tool["name"].as_str())
                .collect::<Vec<_>>();
            addons.push(json!({
                "id": id,
                "enabled": true,
                "mediation": catalog["domain"].as_str().unwrap_or("host"),
                "tool_ceiling": tools,
            }));
        }
        self.editor = Some(text_editor::Content::with_text(
            &serde_json::to_string_pretty(&config).unwrap_or_default(),
        ));
        self.preflight = None;
    }

    fn start_session(&mut self) {
        let Some(project) = self.project() else {
            return;
        };
        let project_id = project["id"].as_str().unwrap_or_default().to_owned();
        let session_id = format!("ui-{project_id}-{}", now_id());
        self.busy = true;
        self.network.request(Request {
            kind: RequestKind::StartSession,
            method: "POST",
            path: "/api/v1/sessions".into(),
            body: Some(json!({"api_version":1,"project_id":project_id,"session_id":session_id})),
        });
    }

    fn send_prompt(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let text = self.prompt.text();
        let text = text.trim();
        if text.is_empty() || self.prompt_id.is_some() {
            return;
        }
        let prompt_id = format!("p-{}", now_id());
        self.expanded_work.insert(prompt_id.clone());
        self.prompt_id = Some(prompt_id.clone());
        self.busy = true;
        self.network.poll(Some(session.clone()), true);
        self.network.request(Request {
            kind: RequestKind::SendPrompt,
            method: "POST",
            path: format!("/api/v1/sessions/{session}/prompts"),
            body: Some(json!({
                "api_version":1,
                "prompt_id":prompt_id,
                "idempotency_key":format!("native-{}", now_id()),
                "text":text
            })),
        });
        self.prompt = text_editor::Content::new();
    }

    fn response(&mut self, kind: RequestKind, result: Result<ApiResponse, String>) {
        let result = result.and_then(ApiResponse::json);
        match (kind, result) {
            (RequestKind::Health, Ok(_)) => {
                self.connected = true;
                self.status = "Connected to the resident vKore broker.".into();
            }
            (RequestKind::Catalog, Ok(value)) => self.catalog = value,
            (RequestKind::Projects, Ok(Value::Array(projects))) => {
                self.projects = projects;
                if self
                    .selected
                    .is_some_and(|index| index >= self.projects.len())
                {
                    self.selected = None;
                }
                if self.auto_open_pending {
                    self.auto_open_pending = false;
                    let requested = self.launch_project.take();
                    let selected = requested.as_ref().and_then(|project_id| {
                        self.projects
                            .iter()
                            .position(|project| project["id"].as_str() == Some(project_id))
                    });
                    let only_active = (requested.is_none())
                        .then(|| {
                            let mut active =
                                self.projects.iter().enumerate().filter(|(_, project)| {
                                    !project["archived"].as_bool().unwrap_or(false)
                                });
                            let first = active.next().map(|(index, _)| index);
                            first.filter(|_| active.next().is_none())
                        })
                        .flatten();
                    if let Some(index) = selected.or(only_active) {
                        self.update(Message::SelectProject(index));
                    } else if let Some(project_id) = requested {
                        self.status = format!(
                            "Launch project {project_id:?} is not available from this daemon."
                        );
                    }
                }
            }
            (RequestKind::Approvals, Ok(value)) => self.approvals = value,
            (RequestKind::Preflight, Ok(value)) => {
                self.busy = false;
                self.preflight = Some(serde_json::to_string_pretty(&value).unwrap_or_default());
                self.status = "Preflight passed.".into();
            }
            (RequestKind::SaveProject, Ok(value)) => {
                self.busy = false;
                let id = value["id"].as_str().unwrap_or_default();
                if let Some(index) = self.projects.iter().position(|item| item["id"] == id) {
                    self.projects[index] = value;
                    self.selected = Some(index);
                } else {
                    self.projects.push(value);
                    self.selected = Some(self.projects.len() - 1);
                }
                self.editor = None;
                self.preflight = None;
                self.status = "Project saved after daemon preflight. Starting sandbox…".into();
                if self.session.is_some() {
                    self.restart_after_stop = true;
                }
                self.update(Message::ToggleSession);
            }
            (RequestKind::StartSession, Ok(value)) => {
                self.busy = false;
                self.session = value["session_id"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value["id"].as_str().map(str::to_owned));
                self.events.clear();
                self.selectable_text.clear();
                self.expanded_work.clear();
                self.expanded_outputs.clear();
                self.prompt_id = None;
                self.network.poll(self.session.clone(), false);
                self.status = "Chat session ready. Kore starts with the first message.".into();
            }
            (RequestKind::StopSession, Ok(_)) => {
                self.busy = false;
                self.network.poll(None, false);
                self.session = None;
                self.prompt_id = None;
                self.expanded_work.clear();
                self.expanded_outputs.clear();
                self.selectable_text.clear();
                self.status = "Routes retired and session KV erased.".into();
                for grant in self.approvals["grants"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|grant| grant["committed"].as_bool().unwrap_or(false))
                {
                    if let Some(id) = grant["id"].as_str() {
                        self.network.request(Request {
                            kind: RequestKind::RevokeGrant,
                            method: "POST",
                            path: format!("/api/v1/grants/{id}/revoke"),
                            body: Some(json!({"ui_session":"axiom-ui"})),
                        });
                    }
                }
                if self.restart_after_stop {
                    self.restart_after_stop = false;
                    self.update(Message::ToggleSession);
                } else {
                    self.reload();
                }
            }
            (RequestKind::SendPrompt, Ok(_)) => {
                self.busy = false;
                self.status = "Prompt accepted by the broker.".into();
            }
            (RequestKind::CancelPrompt, Ok(_)) => {
                self.status = "Cancellation requested.".into();
            }
            (RequestKind::RequestApproval, Ok(value)) => {
                self.busy = false;
                self.approval = value.get("request").cloned();
            }
            (RequestKind::DecideApproval, Ok(_)) => {
                self.busy = false;
                self.approval = None;
                self.reload();
                self.start_session();
            }
            (RequestKind::RevokeGrant, Ok(_)) => self.reload(),
            (RequestKind::ArchiveProject, Ok(_)) => {
                self.selected = None;
                self.reload();
                self.status = "Project archived.".into();
            }
            (RequestKind::DuplicateProject, Ok(value)) => {
                self.projects.push(value);
                self.selected = Some(self.projects.len() - 1);
                self.status = "Project duplicated. Starting sandbox…".into();
                self.update(Message::ToggleSession);
            }
            (
                kind @ (RequestKind::PickWorkspace
                | RequestKind::PickKore
                | RequestKind::PickLlama
                | RequestKind::PickModel),
                Ok(value),
            ) => {
                if let Some(path) = value["path"].as_str() {
                    self.apply_picker(kind, path);
                }
            }
            (_, Err(error)) => {
                self.busy = false;
                self.status = format!("Request refused: {error}");
                if kind == RequestKind::Preflight {
                    self.preflight = Some(self.status.clone());
                }
                if kind == RequestKind::SendPrompt {
                    self.prompt_id = None;
                    self.network.poll(self.session.clone(), false);
                }
            }
            _ => {}
        }
    }

    fn poll_result(&mut self, result: Result<Vec<Value>, String>) {
        match result {
            Ok(events) => {
                let mut prompt_finished = false;
                for event in events {
                    if event["kind"] == "prompt_terminal"
                        && event["prompt_id"].as_str() == self.prompt_id.as_deref()
                    {
                        self.prompt_id = None;
                        self.busy = false;
                        prompt_finished = true;
                    }
                    self.events.push(event);
                }
                if prompt_finished {
                    self.network.poll(self.session.clone(), false);
                }
                self.events_changed();
            }
            Err(error) => self.status = format!("Event stream interrupted: {error}"),
        }
    }

    fn sync_selectable_text(&mut self) {
        let mut desired = BTreeMap::new();
        for user in self
            .events
            .iter()
            .filter(|event| event["kind"] == "user_prompt")
        {
            let prompt_id = user["prompt_id"].as_str().unwrap_or_default();
            // Timestamp and message text are drawn separately so the body
            // stays selectable without pulling metadata into copied text.
            desired.insert(
                format!("prompt:{prompt_id}:user"),
                user["payload"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
            let prompt_events = self
                .events
                .iter()
                .filter(|event| event["prompt_id"].as_str() == Some(prompt_id))
                .collect::<Vec<_>>();
            for (index, row) in work_rows(&prompt_events).into_iter().enumerate() {
                // The speaker is drawn as its own label above the body so it
                // can carry the row's colour; only the body is selectable.
                desired.insert(format!("prompt:{prompt_id}:work:{index}"), row.body);
                if let Some(output) = row.output {
                    desired.insert(
                        format!("prompt:{prompt_id}:work:{index}:output"),
                        output.body,
                    );
                }
            }
            if let Some(assistant) = prompt_events.iter().find(|event| {
                event["kind"] == "assistant" && event["public"].as_bool().unwrap_or(false)
            }) {
                let response = assistant["payload"]["text"].as_str().unwrap_or_default();
                for (index, block) in assistant_blocks(response).into_iter().enumerate() {
                    desired.insert(
                        format!("prompt:{prompt_id}:assistant:{index}"),
                        block.text().to_owned(),
                    );
                }
            }
        }

        let activity = session_work_rows(&self.events);
        for (index, row) in activity.iter().enumerate() {
            desired.insert(
                format!("activity:{index}"),
                format!("{}:\n{}", row.speaker, row.body),
            );
        }
        if let Some(row) = activity.iter().rev().find(|row| row.speaker == "Broker") {
            desired.insert(
                "authority:latest".into(),
                format!("{}:\n{}", row.speaker, row.body),
            );
        }

        self.selectable_text
            .retain(|key, _| desired.contains_key(key));
        for (key, value) in desired {
            let unchanged = self
                .selectable_text
                .get(&key)
                .is_some_and(|content| content.text() == value);
            if !unchanged {
                self.selectable_text
                    .insert(key, text_editor::Content::with_text(&value));
            }
        }
    }

    /// Read-only conversation text the reader can select and copy.
    ///
    /// The face is explicit rather than derived: prose carries the body face,
    /// while machine-shaped content -- logs, tool arguments, JSON -- carries
    /// mono so its columns line up.
    fn selectable<'a>(
        &'a self,
        key: &str,
        size: f32,
        color: u32,
        face: iced_core::Font,
        line_height: f32,
    ) -> UiElement<'a> {
        let key_owned = key.to_owned();
        let content = self
            .selectable_text
            .get(key)
            .expect("selectable conversation text must be synchronized");
        text_editor(content)
            .on_action(move |action| Message::SelectText(key_owned.clone(), action))
            .padding(0)
            .size(size)
            .line_height(iced_core::text::LineHeight::Relative(line_height))
            .font(face)
            .style(widgets::selectable(color))
            .into()
    }

    fn selectable_width<'a>(
        &'a self,
        key: &str,
        width: f32,
        size: f32,
        color: u32,
        face: iced_core::Font,
    ) -> UiElement<'a> {
        let key_owned = key.to_owned();
        let content = self
            .selectable_text
            .get(key)
            .expect("selectable conversation text must be synchronized");
        text_editor(content)
            .on_action(move |action| Message::SelectText(key_owned.clone(), action))
            .padding(0)
            .width(width)
            .size(size)
            .line_height(iced_core::text::LineHeight::Relative(1.55))
            .font(face)
            .style(widgets::selectable(color))
            .into()
    }

    fn selectable_markdown<'a>(
        &'a self,
        key: &str,
        size: f32,
        color: u32,
        face: iced_core::Font,
    ) -> UiElement<'a> {
        let key_owned = key.to_owned();
        let content = self
            .selectable_text
            .get(key)
            .expect("selectable conversation text must be synchronized");
        text_editor(content)
            .on_action(move |action| Message::SelectText(key_owned.clone(), action))
            .padding(0)
            .size(size)
            .line_height(iced_core::text::LineHeight::Relative(1.7))
            .font(face)
            .highlight_with::<InlineMarkdownHighlighter>((), inline_markdown_format)
            .style(widgets::selectable(color))
            .into()
    }

    fn apply_picker(&mut self, kind: RequestKind, path: &str) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let Ok(mut config) = serde_json::from_str::<Value>(&editor.text()) else {
            self.preflight = Some("Fix the project JSON before applying a selected path.".into());
            return;
        };
        let pointer = match kind {
            RequestKind::PickWorkspace => "/workspace/root",
            RequestKind::PickKore => "/inference/kore_image",
            RequestKind::PickLlama => "/inference/llama_server",
            RequestKind::PickModel => "/inference/model",
            _ => return,
        };
        if let Some(slot) = config.pointer_mut(pointer) {
            *slot = Value::String(path.into());
            self.editor = Some(text_editor::Content::with_text(
                &serde_json::to_string_pretty(&config).unwrap_or_default(),
            ));
            self.preflight = None;
        }
    }

    fn view(&self, viewport: Size) -> UiElement<'_> {
        let base: UiElement<'_> = if viewport.width <= 720.0 {
            self.main_panel(viewport)
        } else if viewport.width <= 1050.0 {
            row![self.sidebar(210.0), self.main_panel(viewport)]
                .height(Length::Fill)
                .into()
        } else {
            row![self.sidebar(248.0), self.main_panel(viewport)]
                .height(Length::Fill)
                .into()
        };
        if let Some(phase) = self.booted_at.and_then(|at| splash::progress(at.elapsed())) {
            return stack![base, splash::overlay(phase)]
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }
        if self.editor.is_some() {
            stack![base, self.modal_layer(self.editor_panel())]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else if self.approval.is_some() {
            stack![base, self.modal_layer(self.approval_panel())]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            base
        }
    }

    fn modal_layer<'a>(&self, content: UiElement<'a>) -> UiElement<'a> {
        backdrop_filter(
            container(content)
                .center(Length::Fill)
                .padding(32)
                .style(|_| {
                    let mut overlay = theme::panel(theme::BACKGROUND, None, 0.0);
                    overlay.background = Some(Background::Color(theme::scrim()));
                    overlay
                })
                .into(),
            2.0,
            0.0,
        )
    }

    fn sidebar(&self, width: f32) -> UiElement<'_> {
        let brand = container(
            row![
                splash::mark(20.0),
                text("KORE AGENTS")
                    .font(fonts::DISPLAY)
                    .size(15.0)
                    .color(theme::rgb(theme::TEXT)),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
        )
        .padding(iced_core::Padding {
            top: 14.0,
            right: 14.0,
            bottom: 10.0,
            left: 14.0,
        });
        let mut projects = column![].spacing(theme::SPACE_1);
        for (index, project) in self
            .projects
            .iter()
            .enumerate()
            .filter(|(_, p)| !p["archived"].as_bool().unwrap_or(false))
        {
            let active = self.selected == Some(index);
            let backend = project["inference"]["backend"]
                .as_str()
                .unwrap_or("deterministic")
                .replace('_', " ");
            let access = project["workspace"]["access"]
                .as_str()
                .unwrap_or("read only")
                .replace('_', " ");
            // A 5x5 square, not a circle: the system marks state with a mark,
            // never with a fill behind the row.
            let dot = container(space::Space::new().width(5).height(5)).style(move |_| {
                theme::panel(
                    if active {
                        theme::MAGENTA
                    } else {
                        theme::BORDER_DIM
                    },
                    None,
                    1.0,
                )
            });
            let label = column![
                widgets::body(
                    project["name"].as_str().unwrap_or("Project").to_owned(),
                    theme::SIZE_SM,
                    if active {
                        theme::TEXT
                    } else {
                        theme::TEXT_SECONDARY
                    },
                ),
                widgets::mono(format!("{backend} · {access}"), theme::TEXT_MUTED),
            ]
            .spacing(2);
            projects = projects.push(
                button(
                    row![dot, label]
                        .spacing(theme::SPACE_2)
                        .align_y(alignment::Vertical::Center),
                )
                .width(Length::Fill)
                .padding([9, 10])
                .style(widgets::button_style(widgets::Button::Row(active)))
                .on_press(Message::SelectProject(index)),
            );
        }
        let project_idle = self.project().is_some() && self.session.is_none();
        let project_editable = self.project().is_some() && self.prompt_id.is_none() && !self.busy;
        let nav = |label, message| {
            button(widgets::nav(label))
                .height(34)
                .padding([0, 10])
                .width(Length::Fill)
                .style(widgets::button_style(widgets::Button::Ghost))
                .on_press_maybe(message)
        };
        let body = column![
            brand,
            container(widgets::label("Projects", theme::TEXT_MUTED)).padding(iced_core::Padding {
                top: 10.0,
                right: 14.0,
                bottom: 6.0,
                left: 14.0
            }),
            container(
                scrollable(projects)
                    .direction(scrollable::Direction::Vertical(
                        scrollable::Scrollbar::hidden()
                    ))
                    .style(widgets::hidden_scrollbar)
                    .height(Length::Fill),
            )
            .height(Length::Fill)
            .padding([0, 10]),
            container(
                button(
                    container(widgets::control("New project"))
                        .width(Length::Fill)
                        .align_x(alignment::Horizontal::Center),
                )
                .width(Length::Fill)
                .height(theme::CONTROL_SM)
                .padding([0, 10])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press(Message::NewProject)
            )
            .padding(10),
            container(space::Space::new().height(1).width(Length::Fill)).style(|_| theme::panel(
                theme::BORDER,
                None,
                0.0
            )),
            container(
                column![
                    nav(
                        "Project settings",
                        project_editable.then_some(Message::EditProject)
                    ),
                    nav(
                        "Duplicate project",
                        project_idle.then_some(Message::DuplicateProject)
                    ),
                    nav(
                        "Export configuration",
                        self.project().map(|_| Message::ExportProject)
                    ),
                    nav(
                        "Archive project",
                        project_idle.then_some(Message::ArchiveProject)
                    ),
                    nav("Transcript & audit", Some(Message::View(View::Session))),
                ]
                .spacing(3)
            )
            .padding(iced_core::Padding {
                top: 6.0,
                right: 10.0,
                bottom: 10.0,
                left: 10.0
            }),
        ];
        container(body)
            .width(width)
            .height(Length::Fill)
            .style(|_| divider_right(theme::BACKGROUND))
            .into()
    }

    fn main_panel(&self, viewport: Size) -> UiElement<'_> {
        let project_name = self
            .project()
            .and_then(|p| p["name"].as_str())
            .unwrap_or("No project open");
        // The subtitle carries what status chips used to: backend, workspace
        // root, and either the live session id or that the sandbox is stopped.
        let subtitle = match self.project() {
            None => "create a project to begin".to_owned(),
            Some(project) => {
                let backend = project["inference"]["backend"]
                    .as_str()
                    .unwrap_or("deterministic")
                    .replace('_', " ");
                let root = project["workspace"]["root"]
                    .as_str()
                    .unwrap_or("no workspace");
                match self.session.as_deref() {
                    Some(session) => format!("{backend} · {root} · {session}"),
                    None => format!("{backend} · {root} · sandbox stopped"),
                }
            }
        };
        let view_button = |label: &'static str, target: View| {
            button(widgets::control_upper(label))
                .height(30)
                .padding([0, 12])
                .style(widgets::button_style(widgets::Button::Choice(
                    self.view == target,
                )))
                .on_press(Message::View(target))
        };
        let top = container(
            row![
                column![
                    widgets::body(project_name.to_owned(), theme::SIZE_SM, theme::TEXT)
                        .wrapping(iced_core::text::Wrapping::None)
                        .ellipsis(iced_core::text::Ellipsis::End),
                    widgets::mono(subtitle, theme::TEXT_MUTED)
                        .wrapping(iced_core::text::Wrapping::None)
                        .ellipsis(iced_core::text::Ellipsis::End),
                ]
                .spacing(2)
                .width(Length::Fill)
                .clip(true),
                view_button("Session", View::Session),
                view_button("Agents", View::Agents),
                view_button("Filesystem", View::Filesystem),
            ]
            .spacing(6)
            .align_y(alignment::Vertical::Center),
        )
        .padding([0, 22])
        .height(64)
        .width(Length::Fill)
        .align_y(alignment::Vertical::Center)
        .style(|_| divider_bottom(theme::BACKGROUND));

        if self.view != View::Session {
            let graph = &self.topology_graph;
            let placements = &self.topology_placements;
            let (title, subtitle, body) = match (self.view, self.topology.path) {
                (View::Filesystem, Some(path)) => {
                    inspector::path(graph, &self.topology_filesystem, path)
                }
                _ => inspector::agents(
                    graph,
                    &self.topology_filesystem,
                    self.topology.selected.as_deref(),
                ),
            };
            let canvas: UiElement<'_> = if self.view == View::Agents {
                view::canvas_layer(graph, placements, &self.topology)
            } else {
                view::filesystem_layer(
                    graph,
                    &self.topology_filesystem,
                    &self.topology_agent_bands,
                    &self.topology_path_bands,
                    &self.topology,
                )
            };
            let mut layers = row![
                container(canvas)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .clip(true)
            ];
            if self.topology.inspector {
                layers = layers.push(self.inspector(title, subtitle, body));
            }
            return container(column![
                top,
                self.topology_controls(),
                layers.height(Length::Fill)
            ])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::panel(theme::BACKGROUND, None, 0.0))
            .into();
        }

        // An empty harness stays empty. The composer sits at the bottom with
        // nothing above it: no headline, no helper copy, no status chip.
        let conversation: UiElement<'_> = if self.events.is_empty() {
            container(space::vertical()).height(Length::Fill).into()
        } else {
            let mut items = column![].spacing(44.0);
            let prompts = self
                .events
                .iter()
                .filter(|event| event["kind"] == "user_prompt");
            for user in prompts {
                let prompt_id = user["prompt_id"].as_str().unwrap_or_default();
                let prompt_events = self
                    .events
                    .iter()
                    .filter(|event| event["prompt_id"].as_str() == Some(prompt_id))
                    .collect::<Vec<_>>();
                let started_at = event_timestamp(user);
                let user_key = format!("prompt:{prompt_id}:user");
                let sidebar_width = if viewport.width <= 720.0 {
                    0.0
                } else if viewport.width <= 1050.0 {
                    210.0
                } else {
                    248.0
                };
                let transcript_width = (viewport.width - sidebar_width - 80.0).max(160.0);
                let message_width = user_message_width(
                    user["payload"]["text"].as_str().unwrap_or_default(),
                    transcript_width,
                );
                let user_time = started_at.and_then(format_event_time).unwrap_or_default();
                let user_block = container(
                    column![
                        self.selectable_width(
                            &user_key,
                            message_width,
                            theme::SIZE_SM,
                            theme::TEXT_MUTED,
                            fonts::BODY
                        ),
                        text(user_time)
                            .font(fonts::MONO_MEDIUM)
                            .size(theme::SIZE_MONO)
                            .line_height(iced_core::text::LineHeight::Relative(1.0))
                            .color(theme::rgb(theme::TEXT_MUTED)),
                    ]
                    .spacing(theme::SPACE_2)
                    .align_x(alignment::Horizontal::Right),
                )
                .align_right(Length::Fill);
                let mut turn = column![user_block].spacing(18.0);

                let rows = work_rows(&prompt_events);
                let terminal = prompt_events
                    .iter()
                    .find(|event| event["kind"] == "prompt_terminal");
                let active = terminal.is_none() && self.prompt_id.as_deref() == Some(prompt_id);
                let failed = terminal.is_some_and(|event| event["payload"]["status"] == "failed");
                let ended_at = terminal
                    .and_then(|event| event_timestamp(event))
                    .or_else(|| {
                        prompt_events
                            .last()
                            .and_then(|event| event_timestamp(event))
                    })
                    .or(started_at);
                let work_title = if active {
                    "Working…".to_owned()
                } else if failed {
                    format!(
                        "Stopped after {}",
                        format_elapsed(
                            ended_at
                                .unwrap_or_default()
                                .saturating_sub(started_at.unwrap_or_default())
                        )
                    )
                } else {
                    format!(
                        "Worked for {}",
                        format_elapsed(
                            ended_at
                                .unwrap_or_default()
                                .saturating_sub(started_at.unwrap_or_default())
                        )
                    )
                };
                let work_color = if active {
                    theme::MAGENTA
                } else if failed {
                    theme::ERROR
                } else {
                    theme::TEXT_MUTED
                };
                let expanded = self.expanded_work.contains(prompt_id);
                let work_header = button(
                    row![
                        text(work_title)
                            .font(fonts::BODY)
                            .size(theme::SIZE_SM)
                            .line_height(iced_core::text::LineHeight::Relative(1.5)),
                        text(if expanded { "\u{25be}" } else { "\u{25b8}" })
                            .font(fonts::MONO)
                            .size(9)
                            .color(theme::rgb(theme::TEXT_MUTED)),
                    ]
                    .spacing(7)
                    .align_y(alignment::Vertical::Center),
                )
                .padding(0)
                .style(widgets::disclosure_button_style(work_color))
                .on_press(Message::ToggleWorking(prompt_id.to_owned()));
                let mut timeline = column![work_header].spacing(12);
                if expanded {
                    let mut details = column![].spacing(10);
                    if rows.is_empty() {
                        details = details
                            .push(widgets::mono("waiting for the sandbox", theme::TEXT_MUTED));
                    }
                    for (index, work) in rows.into_iter().enumerate() {
                        let label_color = match work.kind.as_str() {
                            "error" => theme::ERROR,
                            "authority" | "decision" if work.speaker == "Broker" => theme::MAGENTA,
                            "authority" => theme::MAGENTA,
                            _ => theme::TEXT_MUTED,
                        };
                        let body_color = if work.kind == "error" {
                            theme::ERROR
                        } else {
                            theme::CHAT_TEXT
                        };
                        let work_key = format!("prompt:{prompt_id}:work:{index}");
                        let mut label = row![
                            text(work.speaker.clone())
                                .font(fonts::BODY)
                                .size(13)
                                .line_height(iced_core::text::LineHeight::Relative(1.45))
                                .color(theme::rgb(label_color))
                        ]
                        .spacing(theme::SPACE_1);
                        if let Some(context) = &work.context {
                            label = label.push(
                                text(format!("· {context}"))
                                    .font(fonts::BODY)
                                    .size(13)
                                    .line_height(iced_core::text::LineHeight::Relative(1.45))
                                    .color(theme::rgb(theme::TEXT_MUTED)),
                            );
                        }
                        let mut card = column![self.selectable(
                            &work_key,
                            theme::SIZE_MONO,
                            body_color,
                            fonts::MONO,
                            1.6,
                        )]
                        .spacing(theme::SPACE_1);
                        if let Some(output) = &work.output {
                            let output_key = format!("{work_key}:output");
                            let output_expanded = self.expanded_outputs.contains(&output_key);
                            let output_button = button(
                                text(format!(
                                    "{} {}",
                                    if output_expanded {
                                        "\u{25be}"
                                    } else {
                                        "\u{25b8}"
                                    },
                                    output.summary
                                ))
                                .font(fonts::MONO_NAV)
                                .size(11)
                                .line_height(iced_core::text::LineHeight::Relative(1.6)),
                            )
                            .padding(0)
                            .style(widgets::disclosure_button_style(theme::TEXT_MUTED))
                            .on_press(Message::ToggleWorkOutput(output_key.clone()));
                            let mut output_group = column![output_button].spacing(6);
                            if output_expanded {
                                output_group = output_group.push(
                                    container(
                                        scrollable(self.selectable(
                                            &output_key,
                                            theme::SIZE_MONO,
                                            if output.failed {
                                                theme::ERROR
                                            } else {
                                                theme::TEXT_SECONDARY
                                            },
                                            fonts::MONO,
                                            1.6,
                                        ))
                                        .height(Length::Fill)
                                        .direction(scrollable::Direction::Vertical(
                                            scrollable::Scrollbar::hidden(),
                                        ))
                                        .style(widgets::hidden_scrollbar),
                                    )
                                    .padding([8, 10])
                                    .height(work_output_height(&output.body))
                                    .width(Length::Fill)
                                    .style(|_| {
                                        theme::panel(
                                            theme::BACKGROUND,
                                            Some(theme::BORDER),
                                            theme::RADIUS_SM,
                                        )
                                    }),
                                );
                            }
                            card = card.push(output_group);
                        }
                        details = details.push(
                            column![
                                label,
                                container(card)
                                    .padding([9, 11])
                                    .width(Length::Fill)
                                    .style(|_| {
                                        theme::panel(theme::CHAT_ROW, None, theme::RADIUS_SM)
                                    }),
                            ]
                            .spacing(6),
                        );
                    }
                    timeline = timeline.push(
                        container(details)
                            .padding(iced_core::Padding {
                                top: 0.0,
                                right: 0.0,
                                bottom: 0.0,
                                left: 22.0,
                            })
                            .width(Length::Fill),
                    );
                }
                turn = turn.push(timeline);

                if let Some(assistant) = prompt_events.iter().find(|event| {
                    event["kind"] == "assistant" && event["public"].as_bool().unwrap_or(false)
                }) {
                    let response = assistant["payload"]["text"].as_str().unwrap_or_default();
                    let mut response_blocks = column![].spacing(theme::SPACE_4);
                    for (index, block) in assistant_blocks(response).into_iter().enumerate() {
                        let key = format!("prompt:{prompt_id}:assistant:{index}");
                        let element: UiElement<'_> = match block {
                            AssistantBlock::Heading { level, .. } => self.selectable(
                                &key,
                                if level <= 2 {
                                    theme::SIZE_DISPLAY
                                } else {
                                    theme::SIZE_MD
                                },
                                theme::TEXT,
                                fonts::BODY,
                                1.7,
                            ),
                            AssistantBlock::Paragraph(_) => self.selectable_markdown(
                                &key,
                                theme::SIZE_MD,
                                theme::TEXT,
                                fonts::BODY,
                            ),
                            AssistantBlock::List(_) => container(self.selectable_markdown(
                                &key,
                                theme::SIZE_MD,
                                theme::TEXT,
                                fonts::BODY,
                            ))
                            .padding(iced_core::Padding {
                                top: 0.0,
                                right: 0.0,
                                bottom: 0.0,
                                left: 12.0,
                            })
                            .into(),
                            AssistantBlock::Quote(_) => container(self.selectable(
                                &key,
                                theme::SIZE_MD,
                                theme::TEXT_SECONDARY,
                                fonts::BODY,
                                1.7,
                            ))
                            .padding(iced_core::Padding {
                                top: 4.0,
                                right: 0.0,
                                bottom: 4.0,
                                left: 12.0,
                            })
                            .style(|_| divider_left(theme::BACKGROUND))
                            .into(),
                            AssistantBlock::Code { language, .. } => container(
                                column![
                                    widgets::label(
                                        if language.is_empty() {
                                            "Code"
                                        } else {
                                            &language
                                        },
                                        theme::TEXT_MUTED
                                    ),
                                    self.selectable(
                                        &key,
                                        theme::SIZE_MONO,
                                        theme::TEXT_SECONDARY,
                                        fonts::MONO,
                                        1.6,
                                    ),
                                ]
                                .spacing(theme::SPACE_2),
                            )
                            .padding([12, 14])
                            .width(Length::Fill)
                            .style(|_| {
                                theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_MD)
                            })
                            .into(),
                        };
                        response_blocks = response_blocks.push(element);
                    }
                    turn = turn.push(container(response_blocks).padding(iced_core::Padding {
                        top: 0.0,
                        right: 64.0,
                        bottom: 0.0,
                        left: 0.0,
                    }));
                }
                items = items.push(
                    container(container(turn).width(transcript_width.min(820.0)))
                        .center_x(Length::Fill),
                );
            }
            container(
                scrollable(items)
                    .direction(scrollable::Direction::Vertical(
                        scrollable::Scrollbar::hidden(),
                    ))
                    .style(widgets::hidden_scrollbar)
                    .height(Length::Fill),
            )
            .padding([28, 40])
            .height(Length::Fill)
            .into()
        };

        let send = if self.prompt_id.is_some() {
            button(widgets::control("Cancel"))
                .height(theme::CONTROL_SM)
                .padding([0, 12])
                .style(widgets::button_style(widgets::Button::Danger))
                .on_press(Message::CancelPrompt)
        } else {
            button(widgets::control("Send"))
                .height(theme::CONTROL_SM)
                .padding([0, 14])
                .style(widgets::button_style(widgets::Button::Primary))
                .on_press_maybe(
                    (self.session.is_some() && !self.prompt.text().trim().is_empty())
                        .then_some(Message::SendPrompt),
                )
        };
        let access = self
            .project()
            .and_then(|project| project["workspace"]["access"].as_str())
            .unwrap_or("read only")
            .replace('_', " ");
        let prompt = text_editor(&self.prompt)
            .placeholder("Describe the change")
            .on_action(Message::PromptAction)
            .key_binding(|press| {
                if matches!(press.key, keyboard::Key::Named(keyboard::key::Named::Enter))
                    && !press.modifiers.shift()
                {
                    Some(iced_core::text::editor::Binding::Custom(
                        Message::SendPrompt,
                    ))
                } else {
                    iced_core::text::editor::Binding::from_key_press(press)
                }
            })
            .height(96)
            .padding([14, 16])
            .size(theme::SIZE_SM)
            .font(fonts::BODY)
            .line_height(iced_core::text::LineHeight::Relative(1.55))
            .style(widgets::composer_editor);
        let composer = container(column![
            prompt,
            container(
                row![
                    space::horizontal(),
                    container(widgets::mono(access, theme::TEXT_MUTED))
                        .padding([3, 8])
                        .style(|_| theme::panel(
                            theme::BACKGROUND,
                            Some(theme::BORDER),
                            theme::RADIUS_SM
                        )),
                    send,
                ]
                .spacing(theme::SPACE_2)
                .align_y(alignment::Vertical::Center)
            )
            .padding(iced_core::Padding {
                top: 8.0,
                right: 10.0,
                bottom: 8.0,
                left: 16.0,
            })
            .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), 0.0)),
        ])
        .style(|_| theme::panel(theme::PANEL, Some(theme::BORDER), theme::RADIUS_LG));
        let sidebar_width = if viewport.width <= 720.0 {
            0.0
        } else if viewport.width <= 1050.0 {
            210.0
        } else {
            248.0
        };
        let composer_width = (viewport.width - sidebar_width - 80.0).clamp(160.0, 820.0);
        let composer_wrap = container(container(composer).width(composer_width))
            .center_x(Length::Fill)
            .padding(iced_core::Padding {
                top: 10.0,
                right: 40.0,
                bottom: 18.0,
                left: 40.0,
            })
            .width(Length::Fill)
            .style(|_| theme::panel(theme::BACKGROUND, None, 0.0));
        container(column![top, conversation, composer_wrap])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::panel(theme::BACKGROUND, None, 0.0))
            .into()
    }

    /// The legend and view controls that sit above a topology canvas.
    fn topology_controls(&self) -> UiElement<'_> {
        let rule = |color: u32| {
            container(space::Space::new().width(16).height(2))
                .style(move |_: &Theme| theme::panel(color, None, 0.0))
        };
        let dashed_rule = || {
            let dash = || {
                container(space::Space::new().width(4).height(2))
                    .style(|_: &Theme| theme::panel(theme::MAGENTA, None, 0.0))
            };
            row![dash(), dash(), dash()].spacing(2).width(16)
        };
        let legend: UiElement<'_> = if self.view == View::Agents {
            row![
                row![
                    rule(theme::MAGENTA),
                    widgets::mono("spawn lineage", theme::TEXT_MUTED)
                ]
                .spacing(7)
                .align_y(alignment::Vertical::Center),
                row![
                    rule(theme::BORDER_DIM),
                    widgets::mono("withheld capability", theme::TEXT_MUTED)
                ]
                .spacing(7)
                .align_y(alignment::Vertical::Center),
            ]
            .spacing(14)
            .align_y(alignment::Vertical::Center)
            .into()
        } else {
            row![
                row![
                    rule(theme::TEXT_SECONDARY),
                    widgets::mono("containment", theme::TEXT_MUTED)
                ]
                .spacing(7)
                .align_y(alignment::Vertical::Center),
                row![
                    dashed_rule(),
                    widgets::mono("permission", theme::TEXT_MUTED)
                ]
                .spacing(7)
                .align_y(alignment::Vertical::Center),
            ]
            .spacing(14)
            .align_y(alignment::Vertical::Center)
            .into()
        };

        let control = |label: &'static str, message: Message| {
            button(widgets::control(label))
                .height(28)
                .padding([0, 9])
                .style(widgets::button_style(widgets::Button::Secondary))
                .on_press(message)
        };
        let controls = row![
            button(widgets::control_upper("Inspector"))
                .height(28)
                .padding([0, 9])
                .style(widgets::button_style(widgets::Button::Choice(
                    self.topology.inspector
                )))
                .on_press(Message::TopologyInspector),
            control("−", Message::TopologyZoomStep(1.0 / 1.15)),
            control("+", Message::TopologyZoomStep(1.15)),
            control("fit", Message::TopologyFit),
            control("reset", Message::TopologyReset),
            container(widgets::mono(self.topology.readout(), theme::TEXT_MUTED))
                .width(44)
                .align_x(alignment::Horizontal::Right),
        ]
        .spacing(6)
        .align_y(alignment::Vertical::Center);
        container(
            row![space::horizontal(), legend, controls,]
                .spacing(16)
                .align_y(alignment::Vertical::Center),
        )
        .padding([12, 18])
        .width(Length::Fill)
        .style(|_| divider_bottom(theme::BACKGROUND))
        .into()
    }

    /// The topology inspector: what the selected node holds and can reach.
    ///
    /// It belongs to the topology views rather than standing beside the
    /// session, which is why it is built here and not in the shell row.
    fn inspector<'a>(
        &'a self,
        title: String,
        subtitle: String,
        body: UiElement<'a>,
    ) -> UiElement<'a> {
        container(column![
            container(
                column![
                    widgets::mono(title, theme::TEXT),
                    widgets::mono(subtitle, theme::TEXT_MUTED),
                ]
                .spacing(theme::SPACE_1)
            )
            .padding(iced_core::Padding {
                top: 14.0,
                right: 14.0,
                bottom: 12.0,
                left: 14.0,
            })
            .width(Length::Fill)
            .style(|_| divider_bottom(theme::BACKGROUND)),
            container(
                scrollable(body)
                    .direction(scrollable::Direction::Vertical(
                        scrollable::Scrollbar::hidden()
                    ))
                    .style(widgets::hidden_scrollbar)
                    .height(Length::Fill)
            )
            .padding(14),
        ])
        .width(300)
        .height(Length::Fill)
        .style(|_| divider_left(theme::BACKGROUND))
        .into()
    }

    fn editor_panel(&self) -> UiElement<'_> {
        let source = self.editor.as_ref().expect("editor panel requires content");
        let config = serde_json::from_str::<Value>(&source.text()).unwrap_or(Value::Null);
        let value = |pointer: &str| {
            config
                .pointer(pointer)
                .map(|value| match value {
                    Value::String(value) => value.clone(),
                    Value::Null => String::new(),
                    Value::Array(values) => values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "),
                    other => other.to_string(),
                })
                .unwrap_or_default()
        };
        let field = |label: &'static str, pointer: &'static str, placeholder: &'static str| {
            column![
                text(label)
                    .size(11)
                    .color(theme::rgb(theme::TEXT_SECONDARY)),
                text_input(placeholder, value(pointer))
                    .on_input(move |value| Message::WizardField(pointer, value))
                    .padding([9, 10])
                    .size(12)
                    .style(input_style),
            ]
            .spacing(6)
        };
        let picker = |label: &'static str,
                      pointer: &'static str,
                      placeholder: &'static str,
                      kind: RequestKind| {
            column![
                text(label)
                    .size(11)
                    .color(theme::rgb(theme::TEXT_SECONDARY)),
                row![
                    text_input(placeholder, value(pointer))
                        .on_input(move |value| Message::WizardField(pointer, value))
                        .padding([9, 10])
                        .size(12)
                        .style(input_style),
                    button(widgets::control("Browse"))
                        .height(34)
                        .padding([0, 10])
                        .style(widgets::button_style(widgets::Button::Secondary))
                        .on_press(Message::Browse(kind)),
                ]
                .spacing(6),
            ]
            .spacing(6)
        };
        let toggle = |label: &'static str, pointer: &'static str| {
            let checked = config
                .pointer(pointer)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            button(
                row![
                    container(widgets::mono(
                        if checked { "\u{2713}" } else { " " },
                        theme::MAGENTA,
                    ))
                    .width(16)
                    .height(16)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(alignment::Vertical::Center)
                    .style(move |_| theme::panel(
                        theme::BACKGROUND,
                        Some(if checked {
                            theme::MAGENTA
                        } else {
                            theme::BORDER
                        }),
                        theme::RADIUS_SM,
                    )),
                    widgets::body(
                        label.to_owned(),
                        theme::SIZE_SM,
                        if checked {
                            theme::TEXT
                        } else {
                            theme::TEXT_SECONDARY
                        }
                    ),
                ]
                .spacing(theme::SPACE_2)
                .align_y(alignment::Vertical::Center),
            )
            .padding([7, 8])
            .width(Length::Fill)
            .style(widgets::button_style(widgets::Button::Ghost))
            .on_press(Message::WizardToggle(pointer))
        };
        let choice = |label: &'static str, pointer: &'static str, option: &'static str| {
            let selected = config.pointer(pointer).and_then(Value::as_str) == Some(option);
            button(widgets::control(label))
                .height(theme::CONTROL_SM)
                .padding([0, 12])
                .style(widgets::button_style(widgets::Button::Choice(selected)))
                .on_press(Message::WizardChoice(pointer, option))
        };

        let page: UiElement<'_> = match self.wizard_page {
            0 => column![
                row![
                    field("Project name", "/name", "My project"),
                    field("Portable project ID", "/id", "my-project"),
                ]
                .spacing(13),
                picker(
                    "Workspace root",
                    "/workspace/root",
                    "Choose a project folder",
                    RequestKind::PickWorkspace,
                ),
                field(
                    "vKore state directory",
                    "/workspace/state",
                    "Outside the workspace"
                ),
                column![
                    text("Workspace access")
                        .size(11)
                        .color(theme::rgb(theme::TEXT_SECONDARY)),
                    row![
                        choice("Read only", "/workspace/access", "read_only"),
                        choice("Review changes", "/workspace/access", "review_changes"),
                        choice(
                            "Automatic writeback",
                            "/workspace/access",
                            "automatic_writeback"
                        ),
                    ]
                    .spacing(6),
                ]
                .spacing(6),
                row![
                    field("Maximum write bytes", "/workspace/max_write_bytes", "0"),
                    field("Maximum write files", "/workspace/max_write_files", "0"),
                ]
                .spacing(13),
            ]
            .spacing(13)
            .into(),
            1 => {
                let real = config.pointer("/inference/backend").and_then(Value::as_str)
                    == Some("llama_cpp");
                let mut form = column![
                    column![
                        text("Inference backend")
                            .size(11)
                            .color(theme::rgb(theme::TEXT_SECONDARY)),
                        row![
                            choice(
                                "Deterministic security oracle",
                                "/inference/backend",
                                "deterministic"
                            ),
                            choice("llama.cpp", "/inference/backend", "llama_cpp"),
                        ]
                        .spacing(6),
                    ]
                    .spacing(6),
                    picker(
                        "Kore guest image",
                        "/inference/kore_image",
                        "Optional for deterministic testing",
                        RequestKind::PickKore
                    ),
                ]
                .spacing(13);
                if real {
                    form = form
                        .push(picker(
                            "llama-server executable",
                            "/inference/llama_server",
                            "llama-server",
                            RequestKind::PickLlama,
                        ))
                        .push(picker(
                            "GGUF model",
                            "/inference/model",
                            "Model file",
                            RequestKind::PickModel,
                        ))
                        .push(field(
                            "Advanced llama.cpp arguments",
                            "/inference/extra_args",
                            "--device none",
                        ));
                }
                form.push(
                    row![
                        field("Context tokens", "/inference/context_tokens", "32768"),
                        field("Output tokens", "/inference/output_tokens", "512"),
                        field("Threads", "/inference/threads", "4"),
                        field("Inference slots", "/inference/slots", "3"),
                    ]
                    .spacing(10),
                )
                .push(
                    container(text(if real {
                        "llama.cpp runs behind the authenticated Kore bridge; paths remain host-side configuration."
                    } else {
                        "Deterministic mode tests the complete authenticated broker, agent, tool, and UI flow without downloading a model."
                    }).size(11).color(theme::rgb(theme::MAGENTA)))
                    .padding(10)
                    .style(|_| theme::panel(theme::PANEL, Some(theme::SUCCESS), theme::RADIUS_MD)),
                )
                .into()
            }
            2 => {
                let mut addons = column![
                    widgets::display("Host and guest add-ons"),
                    text("Enable only the bounded capabilities this project needs. Authority is still proposed per prompt.")
                        .size(11)
                        .color(theme::rgb(theme::TEXT_MUTED)),
                ]
                .spacing(8);
                if let Some(entries) = self.catalog["addons"].as_array() {
                    for addon in entries {
                        let id = addon["id"].as_str().unwrap_or_default().to_owned();
                        let name = addon["name"].as_str().unwrap_or("Add-on");
                        let domain = addon["domain"].as_str().unwrap_or("bounded");
                        let enabled = config["addons"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|selected| selected["id"] == id && selected["enabled"] == true);
                        let mut detail = addon["tools"]
                            .as_array()
                            .map(|tools| {
                                tools
                                    .iter()
                                    .map(|tool| {
                                        format!(
                                            "{} ({})",
                                            tool["name"].as_str().unwrap_or("tool"),
                                            tool["risk"]
                                                .as_str()
                                                .unwrap_or("bounded")
                                                .replace('_', " ")
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .filter(|tools| !tools.is_empty())
                            .unwrap_or_else(|| "No declared tools".into());
                        let destinations = addon["network_destinations"]
                            .as_array()
                            .map(|entries| {
                                entries
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .filter(|entries| !entries.is_empty())
                            .unwrap_or_else(|| "no network destinations".into());
                        detail.push_str(" · ");
                        detail.push_str(&destinations);
                        addons = addons.push(
                            button(
                                row![
                                    container(widgets::mono(
                                        if enabled { "✓" } else { "" },
                                        theme::MAGENTA,
                                    ))
                                    .width(16)
                                    .height(16)
                                    .align_x(alignment::Horizontal::Center)
                                    .align_y(alignment::Vertical::Center)
                                    .style(move |_| {
                                        theme::panel(
                                            theme::BACKGROUND,
                                            Some(if enabled {
                                                theme::MAGENTA
                                            } else {
                                                theme::BORDER
                                            }),
                                            3.0,
                                        )
                                    }),
                                    column![
                                        text(domain.to_uppercase())
                                            .size(9)
                                            .color(theme::rgb(theme::TEXT_MUTED)),
                                        text(name.to_owned())
                                            .size(12)
                                            .color(theme::rgb(theme::TEXT)),
                                        text(detail).size(11).color(theme::rgb(theme::TEXT_MUTED)),
                                    ]
                                    .spacing(4),
                                ]
                                .spacing(12)
                                .align_y(alignment::Vertical::Top),
                            )
                            .padding(12)
                            .width(Length::Fill)
                            .style(widgets::button_style(widgets::Button::Choice(enabled)))
                            .on_press(Message::WizardAddon(id)),
                        );
                    }
                }
                addons.into()
            }
            3 => column![
                text("Authority defaults")
                    .size(13)
                    .color(theme::rgb(theme::TEXT)),
                row![
                    choice("Allow once", "/approvals/default_scope", "once"),
                    choice("For this session", "/approvals/default_scope", "session"),
                ]
                .spacing(6),
                toggle(
                    "Confirm external writes",
                    "/approvals/confirm_external_writes"
                ),
                toggle(
                    "Confirm destructive operations",
                    "/approvals/confirm_destructive"
                ),
                toggle(
                    "Enable standing policy",
                    "/approvals/standing_policy_enabled"
                ),
                container(space::Space::new().height(1).width(Length::Fill))
                    .style(|_| theme::panel(theme::BORDER, None, 0.0)),
                text("Retention and project limits")
                    .size(13)
                    .color(theme::rgb(theme::TEXT)),
                row![
                    field("Transcript days", "/retention/transcript_days", "30"),
                    field("Memory (MiB)", "/resources/memory_mib", "2048"),
                    field("Virtual CPUs", "/resources/virtual_cpus", "2"),
                ]
                .spacing(10),
                row![
                    field("Prompt timeout (s)", "/resources/prompt_seconds", "300"),
                    field("Agent limit", "/resources/agent_limit", "4"),
                ]
                .spacing(10),
                toggle(
                    "Clean Kore KV state on close",
                    "/retention/clean_kv_on_close"
                ),
                toggle(
                    "Retain raw model output",
                    "/retention/retain_raw_model_output"
                ),
            ]
            .spacing(10)
            .into(),
            _ => {
                let report = self.preflight.as_deref().unwrap_or(
                    "Run preflight to resolve paths, scan workspace bounds, and verify the selected runtime before saving.",
                );
                column![
                    text("Review and preflight")
                        .size(13)
                        .color(theme::rgb(theme::TEXT)),
                    container(
                        column![
                            text(value("/name")).size(16).color(theme::rgb(theme::TEXT)),
                            text(format!(
                                "{} · {}",
                                value("/workspace/access"),
                                value("/inference/backend")
                            ))
                            .size(11)
                            .color(theme::rgb(theme::TEXT_MUTED)),
                            text(value("/workspace/root"))
                                .size(11)
                                .color(theme::rgb(theme::TEXT_SECONDARY)),
                        ]
                        .spacing(6),
                    )
                    .padding(13)
                    .width(Length::Fill)
                    .style(|_| theme::panel(
                        theme::BACKGROUND,
                        Some(theme::BORDER),
                        theme::RADIUS_SM
                    )),
                    container(text(report.to_owned()).size(11).color(rgb(
                        if self.preflight.is_some() {
                            0xb8ef6a
                        } else {
                            0xf5c66a
                        }
                    )))
                    .padding(11)
                    .width(Length::Fill)
                    // Warning before a preflight has run, success after it.
                    .style(|_| theme::panel(
                        theme::PANEL,
                        Some(if self.preflight.is_some() {
                            theme::SUCCESS
                        } else {
                            theme::WARNING
                        }),
                        theme::RADIUS_MD
                    )),
                    text("Advanced portable configuration")
                        .size(11)
                        .color(theme::rgb(theme::TEXT_MUTED)),
                    text_editor(source)
                        .on_action(Message::ConfigAction)
                        .height(150)
                        .size(11)
                        .style(widgets::editor),
                ]
                .spacing(11)
                .into()
            }
        };

        // The strip names each step and says where the reader is, rather
        // than showing five anonymous rules.
        const STEPS: [&str; 5] = [
            "01 workspace",
            "02 runtime",
            "03 add-ons",
            "04 policy",
            "05 review",
        ];
        let mut steps = row![].spacing(6);
        for (index, label) in STEPS.iter().enumerate() {
            let position = match index.cmp(&usize::from(self.wizard_page)) {
                core::cmp::Ordering::Equal => widgets::Step::Current,
                core::cmp::Ordering::Less => widgets::Step::Complete,
                core::cmp::Ordering::Greater => widgets::Step::Upcoming,
            };
            steps = steps.push(
                button(widgets::control_upper(label))
                    .height(28)
                    .padding([0, 8])
                    .width(Length::Fill)
                    .style(widgets::button_style(widgets::Button::Step(position)))
                    .on_press(Message::WizardPage(index as u8)),
            );
        }
        let back = button(widgets::control("Back"))
            .height(theme::CONTROL_SM)
            .padding([0, 12])
            .style(widgets::button_style(widgets::Button::Secondary))
            .on_press_maybe(self.wizard_page.checked_sub(1).map(Message::WizardPage));
        let next: UiElement<'_> = if self.wizard_page < 4 {
            button(widgets::control("Next"))
                .height(theme::CONTROL_SM)
                .padding([0, 14])
                .style(widgets::button_style(widgets::Button::Primary))
                .on_press(Message::WizardPage(self.wizard_page + 1))
                .into()
        } else {
            row![
                button(widgets::control("Run preflight"))
                    .height(theme::CONTROL_SM)
                    .padding([0, 12])
                    .style(widgets::button_style(widgets::Button::Secondary))
                    .on_press_maybe((!self.busy).then_some(Message::Preflight)),
                // Saving stays closed until a preflight has actually run.
                button(widgets::control("Save project"))
                    .height(theme::CONTROL_SM)
                    .padding([0, 14])
                    .style(widgets::button_style(widgets::Button::Primary))
                    .on_press_maybe(
                        (!self.busy && self.preflight.is_some()).then_some(Message::SaveProject)
                    ),
            ]
            .spacing(theme::SPACE_2)
            .into()
        };
        let dialog = column![
            glass_header(
                "Project configuration",
                "portable vkore schema · no credentials or capabilities are stored here",
                Some(Message::CloseEditor),
            ),
            container(steps).padding(iced_core::Padding {
                top: 14.0,
                right: 20.0,
                bottom: 0.0,
                left: 20.0,
            }),
            container(
                scrollable(page)
                    .direction(scrollable::Direction::Vertical(
                        scrollable::Scrollbar::hidden()
                    ))
                    .style(widgets::hidden_scrollbar)
                    .height(Length::Fill)
            )
            .padding(20),
            container(
                row![
                    button(widgets::control("Cancel"))
                        .height(theme::CONTROL_SM)
                        .padding([0, 12])
                        .style(widgets::button_style(widgets::Button::Ghost))
                        .on_press(Message::CloseEditor),
                    space::horizontal(),
                    back,
                    next,
                ]
                .spacing(theme::SPACE_2)
                .align_y(alignment::Vertical::Center),
            )
            .padding([14, 20])
            .width(Length::Fill)
            .style(|_| glass_top_edge()),
        ];
        backdrop_filter(
            container(dialog)
                .width(760)
                .height(Length::FillPortion(9))
                .style(|_| glass_surface())
                .into(),
            14.0,
            theme::RADIUS_GLASS,
        )
    }

    fn approval_panel(&self) -> UiElement<'_> {
        let request = self.approval.as_ref().unwrap_or(&Value::Null);
        backdrop_filter(container(column![
            glass_header("Review exact authority", "", None),
            container(
                column![
                    widgets::body(
                        request["effect_summary"]
                            .as_str()
                            .unwrap_or("The add-on requests a bounded tool ceiling.")
                            .to_owned(),
                        theme::SIZE_SM,
                        theme::WARNING,
                    ),
                    container(widgets::mono(
                        serde_json::to_string_pretty(request).unwrap_or_default(),
                        theme::TEXT_SECONDARY,
                    ))
                    .padding(11)
                    .width(Length::Fill)
                    .style(|_| theme::panel(
                        theme::BACKGROUND,
                        Some(theme::BORDER),
                        theme::RADIUS_MD
                    )),
                    // The dialog says plainly where it came from: a model or a
                    // transcript cannot put this in front of the reader.
                    widgets::mono(
                        "Authenticated user action. Model or transcript text cannot create this dialog.",
                        theme::TEXT_MUTED,
                    ),
                ]
                .spacing(theme::SPACE_2)
            )
            .padding(20),
            container(
                row![
                    button(widgets::control("Deny"))
                        .height(theme::CONTROL_SM)
                        .padding([0, 12])
                        .style(widgets::button_style(widgets::Button::Danger))
                        .on_press(Message::DecideApproval(false)),
                    space::horizontal(),
                    button(widgets::control("Allow once"))
                        .height(theme::CONTROL_SM)
                        .padding([0, 14])
                        .style(widgets::button_style(widgets::Button::Primary))
                        .on_press(Message::DecideApproval(true)),
                ]
                .spacing(theme::SPACE_2)
                .align_y(alignment::Vertical::Center),
            )
            .padding([14, 20])
            .width(Length::Fill)
            .style(|_| glass_top_edge()),
        ])
        .width(620)
        .style(|_| glass_surface())
        .into(), 14.0, theme::RADIUS_GLASS)
    }
}

struct App {
    model: Model,
    context: SoftContext<OwnedDisplayHandle>,
    surface: Option<SoftSurface<OwnedDisplayHandle, Rc<Window>>>,
    renderer: AxRenderer<System>,
    backend: NativeBackend,
    cache: user_interface::Cache,
    events: Vec<iced_core::Event>,
    cursor: mouse::Cursor,
    modifiers: ModifiersState,
    redraw: bool,
    redraw_requested: bool,
    last_frame: Option<Instant>,
    profile: bool,
}

impl App {
    fn process_events(&mut self, window: &Rc<Window>) {
        if self.events.is_empty() {
            return;
        }
        let size = logical_size(window);
        self.model.fit_topology(size);
        let mut interface = UserInterface::build(
            self.model.view(size),
            size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let waker = shell::Waker::noop();
        let mut messages = shell::Bus::new();
        let (state, _) = interface.update(
            window,
            &waker,
            &self.events,
            self.cursor,
            &mut self.renderer,
            &mut messages,
        );
        if let user_interface::State::Updated {
            mouse_interaction, ..
        } = state
        {
            if let Some(icon) = conversion::mouse_interaction(mouse_interaction) {
                window.set_cursor(icon);
                window.set_cursor_visible(true);
            } else {
                window.set_cursor_visible(false);
            }
        }
        self.events.clear();
        self.cache = interface.into_cache();
        for message in messages {
            self.model.update(message);
        }
        self.redraw = true;
    }

    fn draw(&mut self, window: &Rc<Window>) -> Result<(), String> {
        let physical = window.inner_size();
        if physical.width == 0 || physical.height == 0 {
            return Ok(());
        }
        let record_started = Instant::now();
        let scale = window.scale_factor() as f32;
        self.renderer.hint(Scale {
            window: scale,
            application: 1.0,
        });
        self.model.fit_topology(logical_size(window));
        if self.profile {
            let size = logical_size(window);
            let physical = window.inner_size();
            eprintln!(
                "vkore-ui state: view={:?} logical={}x{} physical={}x{} scale={} projects={} connected={}",
                self.model.view,
                size.width,
                size.height,
                physical.width,
                physical.height,
                window.scale_factor(),
                self.model.projects.len(),
                self.model.connected,
            );
        }
        let mut interface = UserInterface::build(
            self.model.view(logical_size(window)),
            logical_size(window),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let now = Instant::now();
        let waker = shell::Waker::noop();
        let mut messages = shell::Bus::new();
        let (state, _) = interface.update(
            window,
            &waker,
            &[iced_core::Event::Window(
                iced_core::window::Event::RedrawRequested(now),
            )],
            self.cursor,
            &mut self.renderer,
            &mut messages,
        );
        let animate = matches!(
            state,
            user_interface::State::Updated {
                redraw_request: iced_core::window::RedrawRequest::NextFrame,
                ..
            }
        );
        interface.draw(
            &mut self.renderer,
            &Theme::Dark,
            &renderer::Style {
                text_color: theme::rgb(theme::TEXT),
            },
            self.cursor,
        );
        self.cache = interface.into_cache();
        let mut model_changed = false;
        for message in messages {
            self.model.update(message);
            model_changed = true;
        }
        let frame = self
            .renderer
            .take_frame()
            .map_err(|error| format!("{error:?}"))?;
        let text_errors = self.renderer.take_text_errors();
        if !text_errors.is_empty() {
            return Err(format!("unsupported text request: {text_errors:?}"));
        }
        let geometry_errors = self.renderer.take_geometry_errors();
        if !geometry_errors.is_empty() {
            return Err(format!("unsupported geometry request: {geometry_errors:?}"));
        }
        // The frame names its coverage masks by identity; hand the bytes over
        // before it is rendered.
        self.backend.set_masks(self.renderer.take_images());
        let text_stats = self.renderer.take_text_stats();
        let record_ns = record_started.elapsed().as_nanos();
        axgfx::frame_requirements(frame.view())
            .map_err(|error| format!("axgfx rejected the display list: {error:?}"))?;
        if !self.backend.begin_frame(&frame) {
            if self.profile {
                eprintln!(
                    "vkore-ui frame: record {:.3} ms ({} runs, {} glyphs); exact retained hit, 0 raster/present bytes",
                    record_ns as f64 / 1_000_000.0,
                    text_stats.glyph_runs,
                    text_stats.glyphs,
                );
            }
            self.redraw = animate || model_changed;
            self.last_frame = Some(Instant::now());
            return Ok(());
        }
        let surface = self.surface.as_mut().ok_or("surface unavailable")?;
        surface
            .resize(
                NonZeroU32::new(physical.width).unwrap(),
                NonZeroU32::new(physical.height).unwrap(),
            )
            .map_err(|error| error.to_string())?;
        let mut buffer = surface.buffer_mut().map_err(|error| error.to_string())?;
        let extent =
            Extent2d::new(physical.width, physical.height).map_err(|error| format!("{error:?}"))?;
        let render = self
            .backend
            .render_changed(&frame, &mut buffer, extent, physical.width as usize)
            .map_err(|error| format!("{error:?}"))?;
        window.pre_present_notify();
        buffer.present().map_err(|error| error.to_string())?;
        self.backend.mark_visible(frame);
        if self.profile {
            eprintln!(
                "vkore-ui frame: record {:.3} ms ({} runs, {} glyphs); structure {:.3} ms ({} commands, {} kernel/{} reference items, {} glyphs, {} atlas hits/{} misses, {} pixel candidates, {} coverage samples); direct present {} bytes; scratch high-water {} pixels",
                record_ns as f64 / 1_000_000.0,
                text_stats.glyph_runs,
                text_stats.glyphs,
                render.structure_ns as f64 / 1_000_000.0,
                render.commands,
                render.kernel_items,
                render.scalar_items,
                render.glyphs,
                render.atlas_hits,
                render.atlas_misses,
                render.pixels_written,
                render.coverage_samples,
                u64::from(physical.width) * u64::from(physical.height) * 4,
                render.scratch_high_water_pixels,
            );
        }
        self.redraw = animate || model_changed;
        self.last_frame = Some(Instant::now());
        Ok(())
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
            return;
        }
        let window = Rc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("vKore · Axiom")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1280, 800))
                        .with_min_inner_size(winit::dpi::PhysicalSize::new(800, 560)),
                )
                .expect("create vKore window"),
        );
        self.surface = Some(
            SoftSurface::new(&self.context, Rc::clone(&window))
                .expect("create presentation surface"),
        );
        self.redraw = true;
        self.redraw_requested = true;
        window.request_redraw();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Response(kind, result) => self.model.response(kind, result),
            UserEvent::Poll(result) => self.model.poll_result(result),
        }
        self.redraw = true;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self
            .surface
            .as_ref()
            .map(SoftSurface::window)
            .map(Rc::clone)
        else {
            return;
        };
        if window.id() != window_id {
            return;
        }
        match &event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = mouse::Cursor::Available(conversion::cursor_position(
                    *position,
                    window.scale_factor() as f32,
                ));
            }
            WindowEvent::CursorLeft { .. } => self.cursor = mouse::Cursor::Unavailable,
            WindowEvent::ModifiersChanged(value) => self.modifiers = value.state(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.redraw = true,
            WindowEvent::RedrawRequested => {
                self.redraw_requested = false;
                if self.redraw
                    && let Err(error) = self.draw(&window)
                {
                    self.model.status = format!("Axiom render failed: {error}");
                }
                return;
            }
            _ => {}
        }
        if let Some(event) =
            conversion::window_event(event, window.scale_factor() as f32, self.modifiers)
        {
            self.events.push(event);
            self.process_events(&window);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
        if !self.redraw || self.redraw_requested {
            return;
        }
        let now = Instant::now();
        let due = self
            .last_frame
            .and_then(|last| last.checked_add(FRAME_INTERVAL))
            .unwrap_or(now);
        if due <= now {
            if let Some(window) = self.surface.as_ref().map(SoftSurface::window) {
                self.redraw_requested = true;
                window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(due));
        }
    }
}

fn logical_size(window: &Window) -> Size {
    let physical = window.inner_size();
    let scale = window.scale_factor() as f32;
    Size::new(
        physical.width as f32 / scale,
        physical.height as f32 / scale,
    )
}

fn http(
    server: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<ApiResponse, String> {
    let payload = body
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut stream = TcpStream::connect_timeout(&server, Duration::from_secs(3))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {server}\r\nAccept: application/json, text/event-stream\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    )
    .map_err(|error| error.to_string())?;
    stream
        .write_all(&payload)
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    let headers = String::from_utf8_lossy(&bytes[..split]);
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or("malformed HTTP status")?;
    let content_type = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        })
        .map(|(_, value)| value.trim().to_owned())
        .unwrap_or_default();
    let response = ApiResponse {
        content_type,
        body: bytes[split + 4..].to_vec(),
    };
    if (200..300).contains(&status) {
        Ok(response)
    } else {
        let detail = serde_json::from_slice::<Value>(&response.body)
            .ok()
            .and_then(|value| value["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| String::from_utf8_lossy(&response.body).into_owned());
        Err(format!("HTTP {status}: {detail}"))
    }
}

fn parse_sse(response: ApiResponse) -> Result<Vec<Value>, String> {
    if !response.content_type.starts_with("text/event-stream") {
        return Err("daemon did not return an event stream".into());
    }
    String::from_utf8(response.body)
        .map_err(|error| error.to_string())?
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).map_err(|error| error.to_string()))
        .collect()
}

fn now_id() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchOptions {
    server: SocketAddr,
    project: Option<String>,
}

fn parse_launch_options(args: impl IntoIterator<Item = String>) -> Result<LaunchOptions, String> {
    let mut server = "127.0.0.1:8787".parse().unwrap();
    let mut project = None;
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--server" => {
                server = args
                    .next()
                    .ok_or("missing --server address")?
                    .parse()
                    .map_err(|_| "invalid --server address")?;
            }
            "--project" => {
                let value = args.next().ok_or("missing --project id")?;
                if value.trim().is_empty() {
                    return Err("empty --project id".into());
                }
                project = Some(value);
            }
            _ => {
                return Err(
                    "usage: vkore-ui [--server 127.0.0.1:8787] [--project PROJECT_ID]".into(),
                );
            }
        }
    }
    Ok(LaunchOptions { server, project })
}

fn main() {
    let options = parse_launch_options(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("vkore-ui: {error}");
        std::process::exit(2);
    });
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    let network = Network::start(options.server, event_loop.create_proxy());
    let context = SoftContext::new(event_loop.owned_display_handle())
        .expect("create software presentation context");
    let mut app = App {
        model: Model::with_launch_project(network, options.project),
        context,
        surface: None,
        renderer: native_renderer(),
        backend: NativeBackend::new().unwrap_or_else(|error| {
            eprintln!("vkore-ui: failed to initialize the Axiom backend: {error:?}");
            std::process::exit(2);
        }),
        cache: user_interface::Cache::new(),
        events: Vec::new(),
        cursor: mouse::Cursor::Unavailable,
        modifiers: ModifiersState::default(),
        redraw: true,
        redraw_requested: false,
        last_frame: None,
        profile: std::env::var_os("VKORE_UI_PROFILE").is_some(),
    };
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app).expect("run vKore UI");
}

#[cfg(test)]
mod tests {
    use super::*;
    use hostlinkd::{
        BackendHarness, DeterministicBackend, DeterministicGuest, HarnessHttpServer, HarnessService,
    };

    fn populated_fixture_model() -> Model {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.connected = true;
        model.booted_at = None;
        model.status = "Kore sandbox ready".into();
        model.catalog = json!({
            "addons": [
                {
                    "id":"reference.host",
                    "name":"Reference host add-on · 0.1.0",
                    "domain":"host domain · MicroPerceptron",
                    "tools":[{"name":"echo","risk":"read_only"}],
                    "network_destinations":[]
                },
                {
                    "id":"reference.guest",
                    "name":"Reference guest add-on · 0.1.0",
                    "domain":"guest domain · MicroPerceptron",
                    "tools":[{"name":"list_workspace","risk":"read_only"}],
                    "network_destinations":[]
                }
            ]
        });
        model.projects = vec![
            json!({
                "id":"vkore-bridge",
                "name":"vkore bridge",
                "archived":false,
                "workspace":{"root":"/home/dev/src/vkore","access":"read_only"},
                "inference":{"backend":"deterministic"},
                "resources":{"memory_mib":4096,"virtual_cpus":4,"prompt_seconds":300,"agent_limit":6},
                "addons":[]
            }),
            json!({
                "id":"kore-frontend-ingress",
                "name":"kore frontend ingress",
                "archived":false,
                "workspace":{"root":"/home/dev/src/kore","access":"review_changes"},
                "inference":{"backend":"llama_cpp"},
                "resources":{"memory_mib":2048,"virtual_cpus":2,"prompt_seconds":180,"agent_limit":4},
                "addons":[]
            }),
        ];
        model.selected = Some(0);
        model.session = Some("session-5f37cccc".into());
        model.expanded_work.insert("native-prompt".into());
        model.events = vec![
            json!({"kind":"user_prompt","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_000_000_u64,"payload":{"text":"The broker keeps refusing the second tool call on the ingress project. Walk the transcript: widen the workspace to review_changes, the agent asks for cat on config/ingress.toml, that lands, then the follow-up write is denied with policy:write-tracked. I want to know whether the denial comes from workspace access mode, the add-on tool ceiling, or the standing policy being off, and what the smallest change is that lets the write through without granting blanket write access to the whole tree."}}),
            json!({"kind":"broker_proposal","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_002_000_u64,"payload":{"action":"intent_claimed","objects":3,"turns":8}}),
            json!({"kind":"broker_proposal","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_004_000_u64,"payload":{"action":"install_scoped_object","path":"workspace/config/ingress.toml"}}),
            json!({"kind":"capability","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_006_000_u64,"payload":{"state":"installed","kind":"object","tools":["read","cat"]}}),
            json!({"kind":"tool_call","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_008_000_u64,"payload":{"command":"cat","arguments":["Path(\"/config/ingress.toml\")"],"call_id":1,"execution_domain":"guest"}}),
            json!({"kind":"tool_result","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_010_000_u64,"payload":{"call_id":1,"result":"output","text":"[gate]\nmode = \"review_changes\"\ntracked = true"}}),
            json!({"kind":"error","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_012_000_u64,"payload":{"message":"write workspace file /config/ingress.generated.toml refused\npolicy:write-tracked · tool not present in add-on ceiling"}}),
            json!({"kind":"assistant","prompt_id":"native-prompt","public":true,"timestamp_unix_ms":1_750_000_030_000_u64,"payload":{"text":"The denial is not the workspace access mode. `review_changes` already permits a staged write; it only requires the diff be held for review instead of applied in place.\n\nThe refusal comes from the add-on tool ceiling. The broker can install a read object for `config/ingress.toml` through the built-in cat path, but there is no ceiling entry that covers a write tool.\n\nStanding policy being off is not the cause either. The smallest change is to add the tracked write tool for the generated file only."}}),
            json!({"kind":"prompt_terminal","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_031_000_u64,"payload":{"status":"completed"}}),
        ];
        // Events set directly bypass the update path that keeps the selectable
        // conversation editors in step, and the view expects them present.
        model.events_changed();
        install_design_topology_fixture(&mut model);
        model
    }

    fn capabilities(held: &[&str]) -> Vec<model::Capability> {
        const ALL: [&str; 12] = [
            "fs.read",
            "fs.list",
            "fs.write",
            "proc.exec",
            "net.fetch",
            "kv.read",
            "kv.write",
            "spawn.agent",
            "tool.cat",
            "tool.grep",
            "tool.patch",
            "tool.test",
        ];
        ALL.into_iter()
            .map(|label| model::Capability {
                label: label.into(),
                held: held.contains(&label),
            })
            .collect()
    }

    fn tool_calls(calls: &[(&str, &str, bool)]) -> Vec<model::ToolCall> {
        calls
            .iter()
            .map(|(name, detail, failed)| model::ToolCall {
                name: (*name).into(),
                detail: (*detail).into(),
                failed: *failed,
            })
            .collect()
    }

    /// Exact sample content from the design handoff. This is intentionally
    /// capture-only: the live application continues to derive topology from
    /// the daemon's event stream and never invents agents or authority.
    fn install_design_topology_fixture(model: &mut Model) {
        let all = [
            "fs.read",
            "fs.list",
            "fs.write",
            "proc.exec",
            "net.fetch",
            "kv.read",
            "kv.write",
            "spawn.agent",
            "tool.cat",
            "tool.grep",
            "tool.patch",
            "tool.test",
        ];
        model.topology_graph = model::Graph {
            agents: vec![
                model::Agent {
                    id: "broker".into(),
                    name: "ai-broker".into(),
                    role: model::Role::Broker,
                    generation: 0,
                    live: true,
                    capabilities: capabilities(&all),
                    tools: tool_calls(&[
                        (
                            "spawn.agent",
                            "reader-4e91 · rights read,list · 14ms",
                            false,
                        ),
                        (
                            "spawn.agent",
                            "indexer-bc07 · rights read,list,kv · 11ms",
                            false,
                        ),
                        (
                            "spawn.agent",
                            "patcher-2f6a · rights read,write,exec · 12ms",
                            false,
                        ),
                    ]),
                    parent: None,
                },
                model::Agent {
                    id: "reader".into(),
                    name: "reader-4e91".into(),
                    role: model::Role::Worker,
                    generation: 1,
                    live: true,
                    capabilities: capabilities(&["fs.read", "fs.list", "tool.cat", "tool.grep"]),
                    tools: tool_calls(&[
                        ("cat", "workspace/src/main.rs · 38ms", false),
                        ("grep", "\"verification code\" · 2 hits · 121ms", false),
                        ("cat", "workspace/Cargo.toml · 22ms", false),
                    ]),
                    parent: Some(0),
                },
                model::Agent {
                    id: "indexer".into(),
                    name: "indexer-bc07".into(),
                    role: model::Role::Worker,
                    generation: 1,
                    live: true,
                    capabilities: capabilities(&[
                        "fs.read",
                        "fs.list",
                        "kv.read",
                        "kv.write",
                        "spawn.agent",
                        "tool.grep",
                    ]),
                    tools: tool_calls(&[
                        ("spawn.agent", "symbols-9f2a · rights read · 9ms", false),
                        ("spawn.agent", "embed-31d5 · rights read,kv · 10ms", false),
                        ("kv.write", "index/manifest · 1 412 keys · 64ms", false),
                    ]),
                    parent: Some(0),
                },
                model::Agent {
                    id: "symbols".into(),
                    name: "symbols-9f2a".into(),
                    role: model::Role::SubAgent,
                    generation: 2,
                    live: true,
                    capabilities: capabilities(&["fs.read", "tool.grep"]),
                    tools: tool_calls(&[
                        ("grep", "\"pub fn\" · crates/vkore-core · 208ms", false),
                        ("grep", "\"impl Policy\" · 6 hits · 96ms", false),
                    ]),
                    parent: Some(2),
                },
                model::Agent {
                    id: "embed".into(),
                    name: "embed-31d5".into(),
                    role: model::Role::SubAgent,
                    generation: 2,
                    live: true,
                    capabilities: capabilities(&["fs.read", "kv.read", "kv.write"]),
                    tools: tool_calls(&[
                        ("kv.write", "embed/shard-00 · 512 vectors · 143ms", false),
                        ("kv.write", "embed/shard-01 · 512 vectors · 138ms", false),
                        ("fs.read", "tests/integration.rs · 27ms", false),
                    ]),
                    parent: Some(2),
                },
                model::Agent {
                    id: "patcher".into(),
                    name: "patcher-2f6a".into(),
                    role: model::Role::Worker,
                    generation: 1,
                    live: true,
                    capabilities: capabilities(&[
                        "fs.read",
                        "fs.write",
                        "proc.exec",
                        "tool.patch",
                        "tool.test",
                    ]),
                    tools: tool_calls(&[
                        ("tool.patch", "src/ui/theme.rs · +18 −4 · 71ms", false),
                        ("proc.exec", "cargo test -p vkore-ui · 8420ms", false),
                        (
                            "tool.patch",
                            "src/ui/view.rs · denied by ceiling · 3ms",
                            true,
                        ),
                    ]),
                    parent: Some(0),
                },
            ],
        };
        model.topology_placements =
            layout::tidy(&model.topology_graph.parents(), layout::Metrics::AGENTS);

        use filesystem::{Grant, Kind, PathNode};
        let path = |path: &str, name: &str, kind, parent| PathNode {
            path: path.into(),
            name: name.into(),
            kind,
            parent,
        };
        model.topology_filesystem = filesystem::Filesystem {
            paths: vec![
                path(
                    "Codebase_directory",
                    "Codebase_directory",
                    Kind::Directory,
                    None,
                ),
                path("Codebase_directory/src", "src", Kind::Directory, Some(0)),
                path(
                    "Codebase_directory/src/main.rs",
                    "main.rs",
                    Kind::File,
                    Some(1),
                ),
                path(
                    "Codebase_directory/src/policy.rs",
                    "policy.rs",
                    Kind::File,
                    Some(1),
                ),
                path("Codebase_directory/src/ui", "ui", Kind::Directory, Some(1)),
                path(
                    "Codebase_directory/src/ui/view.rs",
                    "view.rs",
                    Kind::File,
                    Some(4),
                ),
                path(
                    "Codebase_directory/src/ui/theme.rs",
                    "theme.rs",
                    Kind::File,
                    Some(4),
                ),
                path(
                    "Codebase_directory/crates",
                    "crates",
                    Kind::Directory,
                    Some(0),
                ),
                path(
                    "Codebase_directory/crates/vkore-core",
                    "vkore-core",
                    Kind::Directory,
                    Some(7),
                ),
                path(
                    "Codebase_directory/crates/vkore-core/lib.rs",
                    "lib.rs",
                    Kind::File,
                    Some(8),
                ),
                path(
                    "Codebase_directory/crates/vkore-policy",
                    "vkore-policy",
                    Kind::Directory,
                    Some(7),
                ),
                path(
                    "Codebase_directory/tests",
                    "tests",
                    Kind::Directory,
                    Some(0),
                ),
                path(
                    "Codebase_directory/tests/integration.rs",
                    "integration.rs",
                    Kind::File,
                    Some(11),
                ),
                path(
                    "Codebase_directory/Cargo.toml",
                    "Cargo.toml",
                    Kind::File,
                    Some(0),
                ),
                path(
                    "Codebase_directory/README.md",
                    "README.md",
                    Kind::File,
                    Some(0),
                ),
                path(
                    "~/.cache/vkore/embed.idx",
                    "embed.idx",
                    Kind::External,
                    None,
                ),
                path(
                    "/etc/vkore/policy.toml",
                    "policy.toml",
                    Kind::External,
                    None,
                ),
                path("~/.cargo/registry", "registry", Kind::External, None),
            ],
            grants: [
                (0, 0, "read,list,write*"),
                (0, 15, "read"),
                (0, 16, "read"),
                (0, 17, "read"),
                (1, 2, "read"),
                (1, 3, "read"),
                (1, 13, "read"),
                (2, 9, "read,list"),
                (2, 10, "read,list"),
                (2, 17, "read"),
                (3, 9, "read"),
                (3, 5, "read"),
                (4, 12, "read"),
                (4, 15, "read,write"),
                (5, 6, "read,write"),
                (5, 5, "read"),
                (5, 16, "read"),
            ]
            .into_iter()
            .map(|(agent, path, rights)| Grant {
                agent,
                path,
                rights: rights.into(),
            })
            .collect(),
        };
        (model.topology_agent_bands, model.topology_path_bands) =
            model.topology_filesystem.place(&model.topology_graph);
        model.topology.selected = Some("broker".into());
        model.topology.path = None;
        model.topology.fitted = false;
    }

    fn percentile(samples: &mut [u128], numerator: usize, denominator: usize) -> u128 {
        samples.sort_unstable();
        let index = (samples.len().saturating_sub(1) * numerator) / denominator;
        samples[index]
    }

    fn write_ppm(path: &str, width: usize, height: usize, pixels: &[u32]) {
        let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
        bytes.reserve(width * height * 3);
        for pixel in pixels {
            bytes.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]);
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// Renders one frame the way the shell does, masks included.
    fn render_test_frame(
        frame: &axui_core::Frame<System>,
        masks: Vec<axui_iced::PublishedImage>,
        target: &mut [u32],
        width: u32,
        height: u32,
    ) -> render_backend::RenderProfile {
        let mut backend = NativeBackend::new().unwrap();
        backend.set_masks(masks);
        assert!(backend.begin_frame(frame));
        backend
            .render_changed(
                frame,
                target,
                Extent2d::new(width, height).unwrap(),
                width as usize,
            )
            .unwrap()
    }

    #[test]
    fn control_labels_follow_the_vertical_center_of_their_button_shape() {
        fonts::register().expect("MONOLITH faces register");
        for height in [26.0, 28.0, 30.0, 32.0, 34.0] {
            let size = Size::new(140.0, height);
            let mut renderer = native_renderer();
            renderer.reset(iced_core::Rectangle::with_size(size));
            let element: UiElement<'_> = button(widgets::control_upper("Session"))
                .width(140)
                .height(height)
                .padding([0, 12])
                .style(widgets::button_style(widgets::Button::Choice(true)))
                .on_press(Message::View(View::Session))
                .into();
            let mut interface =
                UserInterface::build(element, size, user_interface::Cache::new(), &mut renderer);
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style {
                    text_color: theme::rgb(theme::TEXT),
                },
                mouse::Cursor::Unavailable,
            );
            let frame = renderer.take_frame().expect("a control frame");
            let quad = frame.quads().first().expect("the button shape");
            let label = frame.glyph_runs().first().expect("the button label");
            let shape_center = quad.bounds.y + quad.bounds.height / 2.0;
            let label_center = label.bounds.y + label.bounds.height / 2.0;
            assert!(
                (shape_center - label_center).abs() <= 2.0,
                "{height}px control placed its label at {label_center}, outside the shape center {shape_center}"
            );
        }
    }

    #[test]
    fn shared_dividers_are_single_edges_instead_of_nested_boxes() {
        let right = divider_right(theme::BACKGROUND);
        assert_eq!(right.border.width, 0.0);
        assert_eq!(right.border_sides.top.width, 0.0);
        assert_eq!(right.border_sides.right.width, 1.0);
        assert_eq!(right.border_sides.bottom.width, 0.0);
        assert_eq!(right.border_sides.left.width, 0.0);

        let glass = glass_bottom_edge();
        assert_eq!(glass.border.width, 0.0);
        assert_eq!(glass.border_sides.bottom.width, 1.0);
    }

    #[test]
    #[ignore = "manual reproducible UI capture and timing probe"]
    fn capture_and_benchmark_ui() {
        let mut model = populated_fixture_model();
        if std::env::var_os("VKORE_UI_EMPTY").is_some() {
            model.events.clear();
            model.selectable_text.clear();
        }
        if std::env::var_os("VKORE_UI_EXPAND_OUTPUTS").is_some() {
            model.expanded_outputs.extend(
                model
                    .selectable_text
                    .keys()
                    .filter(|key| key.ends_with(":output"))
                    .cloned(),
            );
        }
        if let Ok(view) = std::env::var("VKORE_UI_VIEW") {
            model.view = match view.as_str() {
                "agents" => View::Agents,
                "filesystem" => View::Filesystem,
                _ => View::Session,
            };
        }
        if let Ok(ms) = std::env::var("VKORE_UI_SPLASH") {
            let elapsed = std::time::Duration::from_millis(ms.parse().unwrap_or(0));
            model.booted_at = Instant::now().checked_sub(elapsed);
        }
        if std::env::var("VKORE_UI_WIZARD").is_ok() {
            model.update(Message::EditProject);
            if let Ok(page) = std::env::var("VKORE_UI_WIZARD_PAGE") {
                model.wizard_page = page.parse().unwrap_or(0);
            }
        }
        if std::env::var_os("VKORE_UI_APPROVAL").is_some() {
            model.approval = Some(json!({
                "id":"apr-5f37cccc",
                "addon_id":"reference.guest",
                "tool":"list_workspace",
                "session_id":"session-5f37cccc",
                "risk":"read_only",
                "execution_domain":"guest",
                "effect_summary":"Allow reference.guest.list_workspace once for this agent",
                "requested_by":"harness-ui-v1"
            }));
        }
        if let Ok(selected) = std::env::var("VKORE_UI_SELECT") {
            model.topology.selected = Some(selected);
        }
        let capture_fast = std::env::var_os("VKORE_UI_CAPTURE_FAST").is_some();
        let capture_width = capture_fast
            .then(|| std::env::var("VKORE_UI_WIDTH").ok()?.parse().ok())
            .flatten()
            .unwrap_or(1280_u32);
        let capture_height = capture_fast
            .then(|| std::env::var("VKORE_UI_HEIGHT").ok()?.parse().ok())
            .flatten()
            .unwrap_or(800_u32);
        let size = Size::new(capture_width as f32, capture_height as f32);
        model.fit_topology(size);
        let extent = Extent2d::new(capture_width, capture_height).unwrap();
        let mut renderer = native_renderer();
        renderer.hint(Scale {
            window: 1.0,
            application: 1.0,
        });
        let mut cache = user_interface::Cache::new();
        let mut target = vec![0u32; capture_width as usize * capture_height as usize];
        let mut record_ns = Vec::new();
        let mut kernel_full_ns = Vec::new();
        let mut reference_full_ns = Vec::new();
        let mut kernel_damage_ns = Vec::new();
        let mut reference_damage_ns = Vec::new();
        let mut stable_submit_ns = Vec::new();
        let mut kernel_stats = render_backend::RenderProfile::default();
        let mut damage_stats = render_backend::RenderProfile::default();
        let mut reference_damage_stats = render_backend::RenderProfile::default();
        let mut backend = NativeBackend::new().unwrap();
        let mut benchmark_frame = None;
        let mut benchmark_masks = Vec::new();
        let capture_cursor = std::env::var("VKORE_UI_CURSOR")
            .ok()
            .and_then(|value| {
                value
                    .split_once(',')
                    .map(|(x, y)| (x.to_owned(), y.to_owned()))
            })
            .and_then(|(x, y)| Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?)))
            .map(|(x, y)| mouse::Cursor::Available(iced_core::Point::new(x, y)))
            .unwrap_or(mouse::Cursor::Unavailable);
        let capture_press = std::env::var_os("VKORE_UI_PRESS").is_some();

        for iteration in 0..if capture_fast { 1 } else { 70 } {
            let started = Instant::now();
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                std::mem::take(&mut cache),
                &mut renderer,
            );
            // Iced resolves a button's Active/Disabled status during event
            // traversal. Prime the headless capture exactly as the window
            // loop does; drawing a freshly built tree without an event makes
            // every enabled control look disabled on its first frame.
            let waker = shell::Waker::noop();
            let mut messages = shell::Bus::new();
            let mut events = vec![iced_core::Event::Window(
                iced_core::window::Event::RedrawRequested(Instant::now()),
            )];
            if capture_press && iteration == 0 {
                events.push(iced_core::Event::Mouse(mouse::Event::ButtonPressed(
                    mouse::Button::Left,
                )));
            }
            let _ = interface.update(
                &iced_core::window::Headless,
                &waker,
                &events,
                capture_cursor,
                &mut renderer,
                &mut messages,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style {
                    text_color: theme::rgb(theme::TEXT),
                },
                capture_cursor,
            );
            cache = interface.into_cache();
            let frame = renderer.take_frame().unwrap();
            let masks = renderer.take_images();
            axgfx::frame_requirements(frame.view()).unwrap();
            let recorded = Instant::now();
            backend.set_masks(masks.clone());
            backend.invalidate_for_measurement();
            assert!(backend.begin_frame(&frame));
            let stats = backend
                .render_changed(&frame, &mut target, extent, capture_width as usize)
                .unwrap();
            benchmark_frame = Some(frame.clone());
            benchmark_masks = masks.clone();
            backend.mark_visible(frame);
            let finished = Instant::now();
            std::hint::black_box(&target);
            if iteration >= 10 {
                record_ns.push(recorded.duration_since(started).as_nanos());
                kernel_full_ns.push(finished.duration_since(recorded).as_nanos());
            }
            kernel_stats = stats;
        }

        if capture_fast {
            write_ppm(
                &std::env::var("VKORE_UI_CAPTURE")
                    .expect("VKORE_UI_CAPTURE_FAST requires VKORE_UI_CAPTURE"),
                capture_width as usize,
                capture_height as usize,
                &target,
            );
            return;
        }

        let benchmark_frame = benchmark_frame.unwrap();
        let mut reference_target = vec![0u32; 1280 * 800];
        let mut reference = NativeBackend::reference_for_measurement().unwrap();
        reference.set_masks(benchmark_masks.clone());
        let mut reference_stats = render_backend::RenderProfile::default();
        for iteration in 0..70 {
            let started = Instant::now();
            reference_stats = reference
                .render_changed(&benchmark_frame, &mut reference_target, extent, 1280)
                .unwrap();
            let finished = Instant::now();
            std::hint::black_box(&reference_target);
            if iteration >= 10 {
                reference_full_ns.push(finished.duration_since(started).as_nanos());
            }
        }

        let damage_rect = axgfx::PixelRect::new(416, 608, 448, 128).unwrap();
        for iteration in 0..70 {
            let started = Instant::now();
            damage_stats = backend
                .render_damage_for_measurement(
                    &benchmark_frame,
                    &mut target,
                    extent,
                    1280,
                    axgfx::Damage::Rects(&[damage_rect]),
                )
                .unwrap();
            let finished = Instant::now();
            std::hint::black_box(&target);
            if iteration >= 10 {
                kernel_damage_ns.push(finished.duration_since(started).as_nanos());
            }
        }
        for iteration in 0..70 {
            let started = Instant::now();
            reference_damage_stats = reference
                .render_damage_for_measurement(
                    &benchmark_frame,
                    &mut reference_target,
                    extent,
                    1280,
                    axgfx::Damage::Rects(&[damage_rect]),
                )
                .unwrap();
            let finished = Instant::now();
            std::hint::black_box(&reference_target);
            if iteration >= 10 {
                reference_damage_ns.push(finished.duration_since(started).as_nanos());
            }
        }

        let mut differing_pixels = 0u64;
        let mut max_channel_delta = 0u32;
        for (kernel, reference) in target.iter().zip(&reference_target) {
            if kernel != reference {
                differing_pixels += 1;
            }
            for shift in [24, 16, 8, 0] {
                max_channel_delta = max_channel_delta
                    .max(((kernel >> shift) & 0xff).abs_diff((reference >> shift) & 0xff));
            }
        }
        assert_eq!(differing_pixels, 0, "kernel and reference pixels drifted");
        assert_eq!(max_channel_delta, 0, "kernel and reference colors drifted");

        for iteration in 0..70 {
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                std::mem::take(&mut cache),
                &mut renderer,
            );
            let waker = shell::Waker::noop();
            let mut messages = shell::Bus::new();
            let events = [iced_core::Event::Window(
                iced_core::window::Event::RedrawRequested(Instant::now()),
            )];
            let _ = interface.update(
                &iced_core::window::Headless,
                &waker,
                &events,
                capture_cursor,
                &mut renderer,
                &mut messages,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style {
                    text_color: theme::rgb(theme::TEXT),
                },
                capture_cursor,
            );
            cache = interface.into_cache();
            let frame = renderer.take_frame().unwrap();
            let _masks = renderer.take_images();
            let started = Instant::now();
            // A running splash animates, so its frames differ by design and
            // retention has nothing to recognize.
            if model.booted_at.is_none() {
                assert!(!backend.begin_frame(&frame));
            }
            let finished = Instant::now();
            if iteration >= 10 {
                stable_submit_ns.push(finished.duration_since(started).as_nanos());
            }
        }

        if let Ok(path) = std::env::var("VKORE_UI_CAPTURE") {
            write_ppm(&path, 1280, 800, &target);
        }
        let samples = record_ns.len();
        let record_median = percentile(&mut record_ns, 1, 2);
        let record_p95 = percentile(&mut record_ns, 95, 100);
        let kernel_full_median = percentile(&mut kernel_full_ns, 1, 2);
        let kernel_full_p95 = percentile(&mut kernel_full_ns, 95, 100);
        let reference_full_median = percentile(&mut reference_full_ns, 1, 2);
        let reference_full_p95 = percentile(&mut reference_full_ns, 95, 100);
        let kernel_damage_median = percentile(&mut kernel_damage_ns, 1, 2);
        let kernel_damage_p95 = percentile(&mut kernel_damage_ns, 95, 100);
        let reference_damage_median = percentile(&mut reference_damage_ns, 1, 2);
        let reference_damage_p95 = percentile(&mut reference_damage_ns, 95, 100);
        let stable_submit_median = percentile(&mut stable_submit_ns, 1, 2);
        let stable_submit_p95 = percentile(&mut stable_submit_ns, 95, 100);
        println!(
            "UI_BENCH {{\"implementation\":\"shared_axiom_kernels\",\"width\":1280,\"height\":800,\"samples\":{samples},\"record_median_ns\":{record_median},\"record_p95_ns\":{record_p95},\"kernel_full_median_ns\":{kernel_full_median},\"kernel_full_p95_ns\":{kernel_full_p95},\"reference_full_median_ns\":{reference_full_median},\"reference_full_p95_ns\":{reference_full_p95},\"kernel_damage_median_ns\":{kernel_damage_median},\"kernel_damage_p95_ns\":{kernel_damage_p95},\"reference_damage_median_ns\":{reference_damage_median},\"reference_damage_p95_ns\":{reference_damage_p95},\"stable_submit_median_ns\":{stable_submit_median},\"stable_submit_p95_ns\":{stable_submit_p95},\"kernel_items\":{},\"kernel_scalar_fallback_items\":{},\"reference_scalar_items\":{},\"damage_kernel_items\":{},\"damage_scalar_fallback_items\":{},\"reference_damage_scalar_items\":{},\"full_pixel_candidates\":{},\"damage_pixel_candidates\":{},\"reference_damage_pixel_candidates\":{},\"damage_width\":448,\"damage_height\":128,\"differing_pixels\":{differing_pixels},\"max_channel_delta\":{max_channel_delta},\"full_present_bytes\":4096000,\"damage_present_bytes\":229376,\"stable_present_bytes\":0}}",
            kernel_stats.kernel_items,
            kernel_stats.scalar_items,
            reference_stats.scalar_items,
            damage_stats.kernel_items,
            damage_stats.scalar_items,
            reference_damage_stats.scalar_items,
            kernel_stats.pixels_written,
            damage_stats.pixels_written,
            reference_damage_stats.pixels_written,
        );
    }

    #[test]
    fn launch_options_accept_a_preloaded_project() {
        let options = parse_launch_options([
            "--server".into(),
            "127.0.0.1:9876".into(),
            "--project".into(),
            "qwen9b-live-kore".into(),
        ])
        .unwrap();

        assert_eq!(options.server, "127.0.0.1:9876".parse().unwrap());
        assert_eq!(options.project.as_deref(), Some("qwen9b-live-kore"));
        assert!(parse_launch_options(["--project".into(), "".into()]).is_err());
    }

    #[test]
    fn launch_project_opens_directly_and_single_project_is_the_default() {
        for requested in [Some("qwen9b-live-kore".into()), None] {
            let (requests, request_receiver) = mpsc::channel();
            let (sessions, _session_receiver) = mpsc::channel();
            let mut model = Model::with_launch_project(
                Network {
                    sender: requests,
                    session: sessions,
                },
                requested,
            );
            while request_receiver.try_recv().is_ok() {}
            let projects = json!([{
                "id":"qwen9b-live-kore",
                "name":"Qwen 9B live Kore",
                "archived":false,
                "addons":[]
            }]);

            model.response(
                RequestKind::Projects,
                Ok(ApiResponse {
                    content_type: "application/json".into(),
                    body: serde_json::to_vec(&projects).unwrap(),
                }),
            );

            assert_eq!(model.selected, Some(0));
            assert!(model.busy);
            let request = request_receiver
                .recv_timeout(Duration::from_millis(100))
                .unwrap();
            assert_eq!(request.kind, RequestKind::StartSession);
            assert_eq!(request.path, "/api/v1/sessions");
        }
    }

    #[test]
    fn working_rows_show_sanitized_proposal_execution_and_result() {
        let events = [
            json!({"kind":"user_prompt","prompt_id":"p","payload":{"text":"read X"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"Input","role":"Broker","payload":"{\"authenticated_task_intent\":\"read X\",\"offer_catalog\":[[18,\"o\",\"workspace/X\",[],1,null],[31,\"x\",\"cat\",[],2,null]]}"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"ModelPrompt","role":"Broker","payload":"SECRET SYSTEM PROMPT"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"ModelOutputChunk","role":"Broker","payload":"{\"action\":"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"RawModelCompletion","role":"Broker","payload":"{\"action\":\"create_agent\",\"selectors\":[31,18]}"}}),
            json!({"kind":"broker_proposal","prompt_id":"p","payload":{"kind":"BrokerProposal","role":"Broker","payload":"{\"action\":\"create_agent\",\"budget\":{\"tool_calls\":2},\"label\":\"agent/prompt\",\"selectors\":[31,18]}"}}),
            json!({"kind":"tool_call","prompt_id":"p","payload":{"kind":"ToolCall","role":"Agent","payload":"{\"arguments\":[\"Path(\\\"/X\\\")\"],\"call_id\":1,\"command\":\"cat\"}"}}),
            json!({"kind":"tool_call","prompt_id":"p","payload":{"command":"cat","arguments":"[Path(\"/X\")]"}}),
            json!({"kind":"tool_result","generation":1,"payload":{"text":"MANIFOLD-ORANGE-742"}}),
            json!({"kind":"assistant","public":true,"payload":{"text":"The code is MANIFOLD-ORANGE-742"}}),
        ];
        let refs = events.iter().collect::<Vec<_>>();

        let rows = work_rows(&refs);

        assert_eq!(
            rows.len(),
            2,
            "duplicate calls and hidden records stay hidden"
        );
        assert_eq!(rows[0].speaker, "Broker");
        assert_eq!(rows[0].body, "create_agent(Agent 1, [cat], [X])");
        assert_eq!(rows[1].speaker, "Agent 1");
        assert_eq!(rows[1].context.as_deref(), Some("tool · guest"));
        assert_eq!(rows[1].body, "cat /X");
        assert_eq!(rows[1].output.as_ref().unwrap().body, "MANIFOLD-ORANGE-742");
        let visible = rows
            .iter()
            .map(|row| {
                format!(
                    "{}: {}\n{}",
                    row.speaker,
                    row.body,
                    row.output
                        .as_ref()
                        .map(|output| output.body.as_str())
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!visible.contains("SECRET SYSTEM PROMPT"));
        assert!(visible.contains("MANIFOLD-ORANGE-742"));
        assert!(!visible.contains('{'));
    }

    #[test]
    fn rejected_raw_broker_proposal_is_visible_before_validation_failure() {
        let events = [
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"Input","role":"Broker","payload":"{\"authenticated_task_intent\":\"read archetecture.md\",\"offer_catalog\":[[18,\"o\",\"workspace/X\",1,0,null],[19,\"o\",\"workspace/Y\",1,0,null],[20,\"o\",\"workspace/V\",1,0,null],[21,\"x\",\"hello\",1,4096,null],[22,\"x\",\"time\",1,4096,null],[23,\"x\",\"sleep\",1,4096,null],[24,\"x\",\"status\",1,4096,null],[25,\"x\",\"help\",1,4096,null]]}"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"kind":"RawModelCompletion","role":"Broker","payload":"{\"action\":\"create_agent\",\"budget\":{\"tool_calls\":0},\"label\":\"agent/prompt\",\"selectors\":[18,19,20,21,22,23,24,25]}"}}),
            json!({"kind":"inference","prompt_id":"p","payload":{"accepted":false,"detail":"broker action fields do not match the offered authority kinds or ceilings","kind":"Validation","role":"Broker","payload":"{\"action\":\"create_agent\",\"budget\":{\"tool_calls\":0},\"label\":\"agent/prompt\",\"selectors\":[18,19,20,21,22,23,24,25]}"}}),
        ];
        let refs = events.iter().collect::<Vec<_>>();

        let rows = work_rows(&refs);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].speaker, "Broker");
        assert_eq!(
            rows[0].body,
            "create_agent(Agent 1, [hello, time, sleep, status, help], [X, Y, V])"
        );
        assert_eq!(rows[1].speaker, "Denied");
        assert!(rows[1].body.starts_with(
            "create_agent(Agent 1, [hello, time, sleep, status, help], [X, Y, V]) refused"
        ));
        assert!(rows[1].body.contains("do not match"));
    }

    #[test]
    fn every_assistant_block_remains_selectable_and_read_only() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.events = vec![
            json!({"kind":"user_prompt","prompt_id":"p","payload":{"text":"Show code"}}),
            json!({"kind":"assistant","prompt_id":"p","public":true,"payload":{"text":"Answer.\n\n```rs\nlet x = 1;\n```"}}),
        ];
        model.events_changed();

        for key in ["prompt:p:assistant:0", "prompt:p:assistant:1"] {
            let original = model.selectable_text[key].text();
            model.update(Message::SelectText(
                key.into(),
                iced_core::text::editor::Action::SelectAll,
            ));
            assert_eq!(
                model.selectable_text[key].selection(),
                Some(original.clone())
            );
            model.update(Message::SelectText(
                key.into(),
                iced_core::text::editor::Action::Edit(iced_core::text::editor::Edit::Insert('!')),
            ));
            assert_eq!(model.selectable_text[key].text(), original);
        }
    }

    #[test]
    fn conversation_layout_records_at_narrow_wide_and_scaled_sizes() {
        let mut model = populated_fixture_model();
        model.events[0]["payload"]["text"] = Value::String(
            "Explain how this deliberately long user request wraps without occupying the full transcript width on every display size."
                .into(),
        );
        model.events_changed();

        for (size, scale) in [
            (Size::new(800.0, 600.0), 1.0),
            (Size::new(1280.0, 800.0), 1.0),
            (Size::new(800.0, 600.0), 1.5),
        ] {
            let mut renderer = native_renderer();
            renderer.hint(Scale {
                window: scale,
                application: 1.0,
            });
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                user_interface::Cache::new(),
                &mut renderer,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style::default(),
                mouse::Cursor::Unavailable,
            );
            let frame = renderer.take_frame().expect("conversation frame");
            axgfx::frame_requirements(frame.view()).expect("supported conversation display list");
            assert!(renderer.take_text_errors().is_empty());
            assert!(frame.glyphs().len() > 30);
        }
    }

    #[test]
    fn working_panel_toggle_is_reversible() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.expanded_work.insert("p".into());

        model.update(Message::ToggleWorking("p".into()));
        assert!(!model.expanded_work.contains("p"));

        model.update(Message::ToggleWorking("p".into()));
        assert!(model.expanded_work.contains("p"));
    }

    #[test]
    fn tool_calls_pair_with_full_outputs_under_the_specific_agent() {
        let long_output = (0..80)
            .map(|line| format!("line {line}: complete tool response"))
            .collect::<Vec<_>>()
            .join("\n");
        let events = [
            json!({"kind":"tool_call","generation":1,"agent_name":"Reader","payload":{"command":"cat","arguments":["Path(\"/one\")"],"call_id":11,"execution_domain":"guest"}}),
            json!({"kind":"tool_result","generation":1,"payload":{"call_id":11,"result":"output","text":long_output}}),
            json!({"kind":"tool_call","generation":2,"payload":{"command":"grep","arguments":["Text(\"needle\")","Path(\"/two\")"],"call_id":12,"execution_domain":"host"}}),
            json!({"kind":"tool_result","generation":2,"payload":{"call_id":12,"result":"output","text":"match"}}),
        ];
        let refs = events.iter().collect::<Vec<_>>();

        let rows = work_rows(&refs);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].speaker, "Reader");
        assert_eq!(rows[0].context.as_deref(), Some("tool · guest"));
        assert_eq!(rows[0].output.as_ref().unwrap().body, long_output);
        assert_eq!(
            rows[0].output.as_ref().unwrap().summary,
            "output · 80 lines"
        );
        assert_eq!(rows[1].speaker, "Agent 2");
        assert_eq!(rows[1].context.as_deref(), Some("tool · host"));
        assert_eq!(rows[1].output.as_ref().unwrap().body, "match");
        assert_eq!(work_output_height(&long_output), 180.0);
    }

    #[test]
    fn nested_tool_output_toggle_is_reversible() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        let key = "prompt:p:work:0:output".to_owned();

        model.update(Message::ToggleWorkOutput(key.clone()));
        assert!(model.expanded_outputs.contains(&key));

        model.update(Message::ToggleWorkOutput(key.clone()));
        assert!(!model.expanded_outputs.contains(&key));
    }

    #[test]
    fn conversation_text_is_selectable_but_read_only() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.events = vec![json!({
            "kind":"user_prompt",
            "prompt_id":"p",
            "timestamp_unix_ms":1_750_000_000_000_u64,
            "payload":{"text":"select this request"}
        })];
        model.events_changed();
        let key = "prompt:p:user";
        let original = model.selectable_text[key].text();

        model.update(Message::SelectText(
            key.into(),
            iced_core::text::editor::Action::SelectAll,
        ));
        assert_eq!(
            model.selectable_text[key].selection(),
            Some(original.clone())
        );

        model.update(Message::SelectText(
            key.into(),
            iced_core::text::editor::Action::Edit(iced_core::text::editor::Edit::Insert('!')),
        ));
        assert_eq!(model.selectable_text[key].text(), original);
    }

    #[test]
    fn saving_an_open_project_restarts_its_chat_session() {
        let (requests, request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        request_receiver.try_iter().for_each(drop);
        model.projects = vec![json!({
            "id":"saved-project",
            "name":"Saved project",
            "archived":false,
            "addons":[],
            "inference":{"backend":"deterministic"},
        })];
        model.selected = Some(0);
        model.session = Some("old-session".into());

        let saved = model.projects[0].clone();
        model.response(
            RequestKind::SaveProject,
            Ok(ApiResponse {
                content_type: "application/json".into(),
                body: serde_json::to_vec(&saved).unwrap(),
            }),
        );

        assert!(model.restart_after_stop);
        let stop = request_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(stop.kind, RequestKind::StopSession);
        assert_eq!(stop.path, "/api/v1/sessions/old-session");

        model.response(
            RequestKind::StopSession,
            Ok(ApiResponse {
                content_type: "application/json".into(),
                body: br#"{"closed":true}"#.to_vec(),
            }),
        );

        assert!(!model.restart_after_stop);
        let start = request_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(start.kind, RequestKind::StartSession);
        assert_eq!(start.path, "/api/v1/sessions");
        assert_eq!(start.body.unwrap()["project_id"], "saved-project");
    }

    #[test]
    fn complete_shell_records_text_and_renders_directly() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.connected = true;
        model.status = "Native Axiom renderer test".into();
        model.projects = vec![json!({
            "id":"test-project",
            "name":"Test project",
            "archived":false,
            "resources":{"memory_mib":2048,"virtual_cpus":2,"prompt_seconds":300,"agent_limit":4},
            "addons":[]
        })];
        model.selected = Some(0);

        let mut renderer = native_renderer();
        renderer.hint(Scale {
            window: 1.0,
            application: 1.0,
        });
        let mut interface = UserInterface::build(
            model.view(Size::new(900.0, 600.0)),
            Size::new(900.0, 600.0),
            user_interface::Cache::new(),
            &mut renderer,
        );
        interface.draw(
            &mut renderer,
            &Theme::Dark,
            &renderer::Style::default(),
            mouse::Cursor::Unavailable,
        );
        let frame = renderer.take_frame().unwrap();
        let masks = renderer.take_images();
        assert!(frame.commands().len() > 10);
        assert!(
            !frame.glyphs().is_empty(),
            "the native shell must retain text"
        );
        let requirements = axgfx::frame_requirements(frame.view()).unwrap();
        assert!(requirements.features().contains(axgfx::Features::GLYPH_RUN));

        let mut target = vec![0u32; 900 * 600];
        let stats = render_test_frame(&frame, masks, &mut target, 900, 600);
        assert_eq!(stats.scratch_high_water_pixels, 0);
        assert!(stats.pixels_written > 900 * 600 / 8);
        assert!(target.iter().any(|pixel| *pixel != 0x0008_0a0d));
    }

    #[test]
    fn expanded_and_collapsed_working_panel_render_directly() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.session = Some("native-session".into());
        model.expanded_work.insert("native-prompt".into());
        model.events = vec![
            json!({"kind":"user_prompt","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_000_000_u64,"payload":{"text":"Read the verification file"}}),
            json!({"kind":"inference","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_001_000_u64,"payload":{"kind":"ModelOutputChunk","role":"broker","turn":1,"payload":"Granting the exact read capability."}}),
            json!({"kind":"tool_call","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_002_000_u64,"payload":{"tool":"read_workspace","path":"tool-test.txt"}}),
            json!({"kind":"tool_result","prompt_id":"native-prompt","generation":1,"timestamp_unix_ms":1_750_000_003_000_u64,"payload":{"text":"MANIFOLD-ORANGE-742"}}),
            json!({"kind":"assistant","prompt_id":"native-prompt","public":true,"timestamp_unix_ms":1_750_000_004_000_u64,"payload":{"text":"The verification code is MANIFOLD-ORANGE-742."}}),
            json!({"kind":"prompt_terminal","prompt_id":"native-prompt","timestamp_unix_ms":1_750_000_004_000_u64,"payload":{"status":"completed"}}),
        ];
        model.events_changed();

        for expanded in [true, false] {
            if expanded != model.expanded_work.contains("native-prompt") {
                model.update(Message::ToggleWorking("native-prompt".into()));
            }
            let mut renderer = native_renderer();
            renderer.hint(Scale {
                window: 1.0,
                application: 1.0,
            });
            let size = Size::new(1100.0, 760.0);
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                user_interface::Cache::new(),
                &mut renderer,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style::default(),
                mouse::Cursor::Unavailable,
            );
            let frame = renderer.take_frame().unwrap();
            let masks = renderer.take_images();
            axgfx::frame_requirements(frame.view()).unwrap();
            assert!(frame.glyphs().len() > 30);

            let mut target = vec![0u32; 1100 * 760];
            let stats = render_test_frame(&frame, masks, &mut target, 1100, 760);
            assert!(stats.pixels_written > 1100 * 760 / 8);
        }
    }

    #[test]
    fn every_native_wizard_page_records_and_renders() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.catalog = json!({
            "addons":[
                {"id":"reference.host","name":"Reference host add-on","domain":"host","tools":[{"name":"echo"}]},
                {"id":"reference.guest","name":"Reference guest add-on","domain":"guest","tools":[{"name":"list_workspace"}]}
            ]
        });
        model.update(Message::NewProject);

        for page in 0..5 {
            model.wizard_page = page;
            let mut renderer = native_renderer();
            renderer.hint(Scale {
                window: 1.25,
                application: 1.0,
            });
            let size = Size::new(1024.0, 640.0);
            let mut interface = UserInterface::build(
                model.view(size),
                size,
                user_interface::Cache::new(),
                &mut renderer,
            );
            interface.draw(
                &mut renderer,
                &Theme::Dark,
                &renderer::Style {
                    text_color: theme::rgb(theme::TEXT),
                },
                mouse::Cursor::Unavailable,
            );
            let frame = renderer.take_frame().unwrap();
            let masks = renderer.take_images();
            axgfx::frame_requirements(frame.view()).unwrap();
            assert!(frame.glyphs().len() > 20, "wizard page {page} lost text");

            let mut target = vec![0u32; 1280 * 800];
            let stats = render_test_frame(&frame, masks, &mut target, 1280, 800);
            assert!(stats.pixels_written > 1280 * 800 / 5);
        }
    }

    #[test]
    fn new_project_control_is_a_no_op_during_a_live_session() {
        let (requests, _request_receiver) = mpsc::channel();
        let (sessions, _session_receiver) = mpsc::channel();
        let mut model = Model::new(Network {
            sender: requests,
            session: sessions,
        });
        model.session = Some("session-live".into());

        model.update(Message::NewProject);

        assert!(model.editor.is_none());
    }

    #[test]
    fn native_transport_drives_a_complete_prompt_lifecycle() {
        let temporary = tempfile::tempdir().unwrap();
        let service = HarnessService::start(
            BackendHarness::new(DeterministicBackend::new(1), DeterministicGuest::new()),
            temporary.path().join("transcripts"),
        );
        let server = HarnessHttpServer::bind_with_state(
            "127.0.0.1:0".parse().unwrap(),
            service.handle(),
            temporary.path().join("projects"),
        )
        .unwrap();
        let address = server.local_addr();

        let health = roundtrip(&server, address, "GET", "/api/v1/health", None)
            .json()
            .unwrap();
        assert_eq!(health["ok"], true);
        let session = roundtrip(
            &server,
            address,
            "POST",
            "/api/v1/sessions",
            Some(&json!({
                "api_version":1,
                "project_id":"native-project",
                "session_id":"native-session"
            })),
        )
        .json()
        .unwrap();
        assert_eq!(session["session_id"], "native-session");
        roundtrip(
            &server,
            address,
            "POST",
            "/api/v1/sessions/native-session/prompts",
            Some(&json!({
                "api_version":1,
                "prompt_id":"native-prompt",
                "idempotency_key":"native-key",
                "text":"hello from axiom"
            })),
        );

        let mut events = Vec::new();
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(10));
            events = parse_sse(roundtrip(
                &server,
                address,
                "GET",
                "/api/v1/sessions/native-session/events?after=0",
                None,
            ))
            .unwrap();
            if events
                .iter()
                .any(|event| event["kind"] == "prompt_terminal")
            {
                break;
            }
        }
        assert!(events.iter().any(|event| event["kind"] == "user_prompt"));
        assert!(events.iter().any(|event| event["kind"] == "assistant"));
        assert!(
            events
                .iter()
                .any(|event| event["kind"] == "prompt_terminal")
        );
        roundtrip(
            &server,
            address,
            "DELETE",
            "/api/v1/sessions/native-session",
            None,
        );
    }

    fn roundtrip(
        server: &HarnessHttpServer,
        address: SocketAddr,
        method: &'static str,
        path: &'static str,
        body: Option<&Value>,
    ) -> ApiResponse {
        let body = body.cloned();
        let client = thread::spawn(move || http(address, method, path, body.as_ref()).unwrap());
        server.poll(Duration::from_secs(1)).unwrap();
        client.join().unwrap()
    }
}
