//! Drawing the topology: edges on a canvas, nodes as real widgets.
//!
//! The split is deliberate. Edges are curves and belong to the geometry lane.
//! Nodes are full of text -- a name, a role, capability chips, a footer -- and
//! canvas text would bypass shaping and the font registry entirely, so they
//! are ordinary widgets positioned absolutely over the canvas. That keeps node
//! text shaped, selectable, and hit-testable, and keeps the canvas doing the
//! one thing it is better at.
//!
//! Both layers read the same pan and zoom, so they move together.

use crate::topology::filesystem::{self as fs, Filesystem};
use crate::topology::layout::{self, Placement};
use crate::topology::model::{Agent, Graph};
use crate::{AxUi, Message, theme, widgets};
use iced_core::{Length, Point, Rectangle, Size, Theme, alignment, mouse};
use iced_widget::canvas::{self, Geometry, Path, Stroke};
use iced_widget::{button, column, container, row, space, stack};

/// Spacing of the canvas dot grid.
const GRID: f32 = 32.0;
/// Zoom limits, from the design.
const MIN_SCALE: f32 = 0.32;
const MAX_SCALE: f32 = 2.2;
/// Control-point offset for a parent-to-child edge.
const CONTROL: f32 = 44.0;

/// Pan, zoom, and what is selected.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// Horizontal pan.
    pub tx: f32,
    /// Vertical pan.
    pub ty: f32,
    /// Zoom, clamped to the design's range.
    pub scale: f32,
    /// Selected agent identity.
    pub selected: Option<String>,
    /// Whether the inspector is open.
    pub inspector: bool,
    /// Set once a viewport has been fitted, so a fit happens exactly once.
    pub fitted: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            tx: 0.0,
            ty: 0.0,
            scale: 1.0,
            selected: None,
            inspector: true,
            fitted: false,
        }
    }
}

impl State {
    /// Zooms about `pointer`, keeping the graph point under it fixed.
    pub fn zoom_about(&mut self, pointer: Point, factor: f32) {
        let next = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        if (next - self.scale).abs() < f32::EPSILON {
            return;
        }
        // Solve so the graph coordinate under the pointer does not move.
        let graph_x = (pointer.x - self.tx) / self.scale;
        let graph_y = (pointer.y - self.ty) / self.scale;
        self.tx = pointer.x - graph_x * next;
        self.ty = pointer.y - graph_y * next;
        self.scale = next;
    }

    /// The design's wheel response: `scale * exp(-deltaY * 0.0016)`.
    pub fn wheel_factor(delta_y: f32) -> f32 {
        (-delta_y * 0.0016).exp()
    }

    /// Fits `placements` into `viewport` and marks the fit as taken.
    pub fn fit(&mut self, placements: &[Placement], viewport: Size) {
        let (tx, ty, scale) = layout::fit(placements, viewport.width, viewport.height, 36.0);
        self.tx = tx;
        self.ty = ty;
        self.scale = scale;
        self.fitted = true;
    }

    /// Returns the zoom readout the design puts beside the controls.
    pub fn readout(&self) -> String {
        format!("{}%", (self.scale * 100.0).round() as i32)
    }
}

/// Pan and zoom, shared by both topology canvases.
fn pan_and_zoom(
    state: &mut EdgeState,
    event: &iced_core::Event,
    bounds: Rectangle,
    cursor: mouse::Cursor,
) -> Option<canvas::Action<Message>> {
    use iced_core::Event;
    use iced_core::mouse::{Button, Event as Mouse, ScrollDelta};

    let position = cursor.position_in(bounds);
    match event {
        Event::Mouse(Mouse::WheelScrolled { delta }) => {
            let pointer = position?;
            let delta_y = match delta {
                ScrollDelta::Lines { y, .. } => y * 16.0,
                ScrollDelta::Pixels { y, .. } => *y,
            };
            Some(
                canvas::Action::publish(Message::TopologyZoom(
                    pointer,
                    State::wheel_factor(delta_y),
                ))
                .and_capture(),
            )
        }
        Event::Mouse(Mouse::ButtonPressed(Button::Left)) => {
            state.drag = position;
            None
        }
        Event::Mouse(Mouse::CursorMoved { .. }) => {
            let from = state.drag?;
            let to = position?;
            // The design ignores movement under four pixels so a click still
            // selects rather than nudging the whole scene.
            if (to.x - from.x).abs() < 4.0 && (to.y - from.y).abs() < 4.0 {
                return None;
            }
            state.drag = Some(to);
            Some(
                canvas::Action::publish(Message::TopologyPan(to.x - from.x, to.y - from.y))
                    .and_capture(),
            )
        }
        Event::Mouse(Mouse::ButtonReleased(Button::Left)) => {
            state.drag = None;
            None
        }
        _ => None,
    }
}

fn grab_cursor(state: &EdgeState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
    if state.drag.is_some() {
        mouse::Interaction::Grabbing
    } else if cursor.is_over(bounds) {
        mouse::Interaction::Grab
    } else {
        mouse::Interaction::default()
    }
}

/// The background grid, as one path so it costs one mask rather than many.
fn dot_grid(bounds: Rectangle, state: &State) -> Path {
    Path::new(|builder| {
        let first_x = (state.tx % GRID) - GRID;
        let first_y = (state.ty % GRID) - GRID;
        let mut y = first_y;
        while y < bounds.height + GRID {
            let mut x = first_x;
            while x < bounds.width + GRID {
                builder.rectangle(Point::new(x, y), Size::new(1.0, 1.0));
                x += GRID;
            }
            y += GRID;
        }
    })
}

/// The edge layer: the dot grid and every parent-to-child curve.
pub struct Edges<'a> {
    graph: &'a Graph,
    placements: &'a [Placement],
    state: &'a State,
}

impl<'a> Edges<'a> {
    pub fn new(graph: &'a Graph, placements: &'a [Placement], state: &'a State) -> Self {
        Self {
            graph,
            placements,
            state,
        }
    }
}

/// What the edge layer keeps between draws.
#[derive(Default)]
pub struct EdgeState {
    /// Where a drag began, in viewport coordinates.
    drag: Option<Point>,
    /// The rasterized layer, kept until something it depends on moves.
    cache: canvas::Cache<AxUi>,
    /// Signature of the scene the cache holds.
    signature: std::cell::Cell<u64>,
}

impl Edges<'_> {
    /// Everything the drawn layer depends on, folded into one value.
    ///
    /// The cache exists so a graph that has not moved is not rasterized
    /// again; this is what tells it that something did move.
    fn signature(&self, bounds: Rectangle) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut mix = |value: u64| {
            hash = (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3);
        };
        mix(self.state.tx.to_bits().into());
        mix(self.state.ty.to_bits().into());
        mix(self.state.scale.to_bits().into());
        mix(bounds.width.to_bits().into());
        mix(bounds.height.to_bits().into());
        mix(self.graph.agents.len() as u64);
        for byte in self.state.selected.as_deref().unwrap_or("").bytes() {
            mix(u64::from(byte));
        }
        hash
    }
}

impl canvas::Program<Message, Theme, AxUi> for Edges<'_> {
    type State = EdgeState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced_core::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        pan_and_zoom(state, event, bounds, cursor)
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &AxUi,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        use iced_graphics::geometry::frame::Backend as _;

        let signature = self.signature(bounds);
        if state.signature.replace(signature) != signature {
            state.cache.clear();
        }
        vec![state.cache.draw_with_bounds(renderer, bounds, |frame| {
            frame.fill(&dot_grid(bounds, self.state), theme::rgb(theme::HOVER));

            let selected = self
                .state
                .selected
                .as_deref()
                .and_then(|id| self.graph.index_of(id));
            for (index, agent) in self.graph.agents.iter().enumerate() {
                let Some(parent) = agent.parent else { continue };
                let (Some(from), Some(to)) =
                    (self.placements.get(parent), self.placements.get(index))
                else {
                    continue;
                };
                let touched = selected.is_some_and(|chosen| chosen == index || chosen == parent);
                let start = self.point(from.center_x(), from.y + from.height);
                let end = self.point(to.center_x(), to.y);
                let curve = Path::new(|builder| {
                    builder.move_to(start);
                    builder.bezier_curve_to(
                        Point::new(start.x, start.y + CONTROL * self.state.scale),
                        Point::new(end.x, end.y - CONTROL * self.state.scale),
                        end,
                    );
                });
                frame.stroke(
                    &curve,
                    Stroke::default()
                        .with_width(if touched { 1.6 } else { 1.1 })
                        .with_color(theme::rgba(
                            theme::MAGENTA,
                            if touched { 1.0 } else { 0.45 },
                        ))
                        .with_line_cap(iced_graphics::geometry::LineCap::Round),
                );
            }
        })]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        grab_cursor(state, bounds, cursor)
    }
}

impl Edges<'_> {
    /// Maps a graph coordinate into the viewport.
    fn point(&self, x: f32, y: f32) -> Point {
        Point::new(
            x * self.state.scale + self.state.tx,
            y * self.state.scale + self.state.ty,
        )
    }
}

/// Builds one agent node.
///
/// Chips cap at the design's counts so a broker holding a large catalog stays
/// the same height as its children rather than growing without bound.
fn node<'a>(agent: &'a Agent, selected: bool) -> crate::UiElement<'a> {
    let dot_color = if agent.live {
        theme::SUCCESS
    } else {
        theme::BORDER_DIM
    };
    let title = row![
        container(space::Space::new().width(5).height(5))
            .style(move |_| theme::panel(dot_color, None, 1.0)),
        widgets::mono(agent.name.clone(), theme::TEXT)
            .wrapping(iced_core::text::Wrapping::None)
            .ellipsis(iced_core::text::Ellipsis::End),
        space::horizontal(),
        widgets::label(agent.role.label(), theme::TEXT_MUTED),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Center);

    let chip = |label: String, held: bool| {
        container(widgets::mono(
            label,
            if held {
                theme::MAGENTA
            } else {
                theme::BORDER_DIM
            },
        ))
        .padding([2, 6])
        .style(move |_| {
            let mut style = theme::panel(
                theme::PANEL,
                Some(if held { theme::MAGENTA } else { theme::BORDER }),
                2.0,
            );
            style.background = None;
            if held {
                style.border.color = theme::chip_border();
            }
            style
        })
    };

    let mut chips = row![].spacing(theme::SPACE_1);
    let held: Vec<&str> = agent.held().map(|c| c.label.as_str()).collect();
    for label in held.iter().take(7) {
        chips = chips.push(chip((*label).to_owned(), true));
    }
    if held.len() > 7 {
        chips = chips.push(chip(format!("+{}", held.len() - 7), true));
    }
    let withheld: Vec<&str> = agent.withheld().map(|c| c.label.as_str()).collect();
    for label in withheld.iter().take(3) {
        chips = chips.push(chip((*label).to_owned(), false));
    }
    if withheld.len() > 3 {
        chips = chips.push(chip(format!("+{}", withheld.len() - 3), false));
    }

    button(
        column![
            title,
            chips.wrap(),
            // The footer wraps rather than truncating: which capabilities an
            // agent holds and how many it ran is the point of the node.
            widgets::mono(agent.footer(), theme::TEXT_MUTED),
        ]
        .spacing(9),
    )
    .padding([12, 13])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_theme, status| {
        let hovered = matches!(
            status,
            iced_widget::button::Status::Hovered | iced_widget::button::Status::Pressed
        );
        iced_widget::button::Style {
            background: Some(iced_core::Background::Color(if selected {
                theme::selected_fill()
            } else {
                theme::rgb(theme::PANEL)
            })),
            text_color: theme::rgb(theme::TEXT),
            border: theme::border(
                if selected || hovered {
                    theme::MAGENTA
                } else {
                    theme::BORDER
                },
                theme::RADIUS_LG,
            ),
            shadow: theme::no_shadow(),
            ..Default::default()
        }
    })
    .on_press(Message::TopologySelect(agent.id.clone()))
    .into()
}

/// The whole canvas: edges beneath, nodes pinned over them.
pub fn canvas_layer<'a>(
    graph: &'a Graph,
    placements: &'a [Placement],
    state: &'a State,
) -> crate::UiElement<'a> {
    let mut layers = stack![
        iced_widget::canvas(Edges::new(graph, placements, state))
            .width(Length::Fill)
            .height(Length::Fill)
    ];
    for (index, agent) in graph.agents.iter().enumerate() {
        let Some(placement) = placements.get(index) else {
            continue;
        };
        let selected = state.selected.as_deref() == Some(agent.id.as_str());
        layers = layers.push(
            iced_widget::pin(
                container(node(agent, selected))
                    .width(placement.width * state.scale)
                    .height(placement.height * state.scale),
            )
            .x(placement.x * state.scale + state.tx)
            .y(placement.y * state.scale + state.ty),
        );
    }
    layers.width(Length::Fill).height(Length::Fill).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_gesture_zooms_by_the_published_response() {
        // Scrolling up magnifies, scrolling down shrinks, both about one.
        assert!(State::wheel_factor(-100.0) > 1.0);
        assert!(State::wheel_factor(100.0) < 1.0);
        assert!((State::wheel_factor(0.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let mut state = State::default();
        let pointer = Point::new(320.0, 180.0);
        let before = (
            (pointer.x - state.tx) / state.scale,
            (pointer.y - state.ty) / state.scale,
        );

        state.zoom_about(pointer, 1.6);
        let after = (
            (pointer.x - state.tx) / state.scale,
            (pointer.y - state.ty) / state.scale,
        );

        assert!((before.0 - after.0).abs() < 0.001);
        assert!((before.1 - after.1).abs() < 0.001);
        assert!(state.scale > 1.0);
    }

    #[test]
    fn zoom_is_clamped_to_the_published_range() {
        let mut state = State::default();
        for _ in 0..40 {
            state.zoom_about(Point::ORIGIN, 2.0);
        }
        assert_eq!(state.scale, MAX_SCALE);
        for _ in 0..80 {
            state.zoom_about(Point::ORIGIN, 0.5);
        }
        assert_eq!(state.scale, MIN_SCALE);
    }

    #[test]
    fn the_readout_reports_whole_percent() {
        let mut state = State::default();
        assert_eq!(state.readout(), "100%");
        state.scale = 0.923;
        assert_eq!(state.readout(), "92%");
    }

    #[test]
    fn fitting_records_that_it_happened() {
        let placements = layout::tidy(&[None, Some(0)], layout::Metrics::AGENTS);
        let mut state = State::default();
        assert!(!state.fitted);
        state.fit(&placements, Size::new(900.0, 600.0));
        assert!(state.fitted);
        assert!(state.scale <= 1.0 && state.scale >= 0.7);
    }
}

/// The filesystem edge layer: containment beneath, grants over it.
pub struct Permissions<'a> {
    graph: &'a Graph,
    filesystem: &'a Filesystem,
    agents: &'a [Placement],
    paths: &'a [Placement],
    state: &'a State,
}

impl<'a> Permissions<'a> {
    pub fn new(
        graph: &'a Graph,
        filesystem: &'a Filesystem,
        agents: &'a [Placement],
        paths: &'a [Placement],
        state: &'a State,
    ) -> Self {
        Self {
            graph,
            filesystem,
            agents,
            paths,
            state,
        }
    }

    fn point(&self, x: f32, y: f32) -> Point {
        Point::new(
            x * self.state.scale + self.state.tx,
            y * self.state.scale + self.state.ty,
        )
    }

    fn signature(&self, bounds: Rectangle) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut mix = |value: u64| {
            hash = (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3);
        };
        mix(self.state.tx.to_bits().into());
        mix(self.state.ty.to_bits().into());
        mix(self.state.scale.to_bits().into());
        mix(bounds.width.to_bits().into());
        mix(bounds.height.to_bits().into());
        mix(self.filesystem.paths.len() as u64);
        mix(self.filesystem.grants.len() as u64);
        for byte in self.state.selected.as_deref().unwrap_or("").bytes() {
            mix(u64::from(byte));
        }
        hash
    }
}

impl canvas::Program<Message, Theme, AxUi> for Permissions<'_> {
    type State = EdgeState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced_core::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        pan_and_zoom(state, event, bounds, cursor)
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &AxUi,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<AxUi>> {
        use iced_graphics::geometry::frame::Backend as _;

        let signature = self.signature(bounds);
        if state.signature.replace(signature) != signature {
            state.cache.clear();
        }
        vec![state.cache.draw_with_bounds(renderer, bounds, |frame| {
            frame.fill(&dot_grid(bounds, self.state), theme::rgb(theme::HOVER));

            // Containment first, so a grant is never hidden behind it.
            for (index, node) in self.filesystem.paths.iter().enumerate() {
                let Some(parent) = node.parent else { continue };
                let (Some(from), Some(to)) = (self.paths.get(parent), self.paths.get(index)) else {
                    continue;
                };
                let start = self.point(from.center_x(), from.y + from.height);
                let end = self.point(to.center_x(), to.y);
                let offset = 26.0 * self.state.scale;
                let curve = Path::new(|builder| {
                    builder.move_to(start);
                    builder.bezier_curve_to(
                        Point::new(start.x, start.y + offset),
                        Point::new(end.x, end.y - offset),
                        end,
                    );
                });
                frame.stroke(
                    &curve,
                    Stroke::default()
                        .with_width(1.0)
                        .with_color(theme::rgba(theme::TEXT, 0.30)),
                );
            }

            let chosen = self
                .state
                .selected
                .as_deref()
                .and_then(|id| self.graph.index_of(id));
            for grant in &self.filesystem.grants {
                let (Some(from), Some(to)) =
                    (self.agents.get(grant.agent), self.paths.get(grant.path))
                else {
                    continue;
                };
                // A selection makes its own grants the only legible ones.
                let opacity = match chosen {
                    Some(agent) if agent == grant.agent => 1.0,
                    Some(_) => 0.14,
                    None => 0.4,
                };
                let start = self.point(from.center_x(), from.y + from.height);
                let end = self.point(to.center_x(), to.y);
                let midpoint = (start.y + end.y) / 2.0;
                let curve = Path::new(|builder| {
                    builder.move_to(start);
                    builder.bezier_curve_to(
                        Point::new(start.x, midpoint),
                        Point::new(end.x, midpoint),
                        end,
                    );
                });
                frame.stroke(
                    &curve,
                    Stroke {
                        width: 1.0,
                        line_dash: iced_graphics::geometry::LineDash {
                            segments: &[6.0, 5.0],
                            offset: 0,
                        },
                        ..Stroke::default().with_color(theme::rgba(theme::MAGENTA, opacity))
                    },
                );
            }
        })]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        grab_cursor(state, bounds, cursor)
    }
}

/// One path node, drawn by what it is.
fn path_node<'a>(node: &'a fs::PathNode, lit: bool) -> crate::UiElement<'a> {
    let (fill, edge, name_color, radius) = match node.kind {
        fs::Kind::Directory => (
            theme::PANEL_DIR,
            theme::rgba(theme::TEXT, 0.42),
            theme::TEXT,
            5.0,
        ),
        fs::Kind::File => (
            theme::PANEL_FILE,
            theme::rgba(theme::TEXT, 0.20),
            theme::TEXT_SECONDARY,
            theme::RADIUS_SM,
        ),
        fs::Kind::External => (
            theme::PANEL_FILE,
            theme::rgba(theme::WARNING, 0.55),
            theme::WARNING,
            theme::RADIUS_SM,
        ),
    };
    container(
        row![
            widgets::mono(node.name.clone(), name_color)
                .wrapping(iced_core::text::Wrapping::None)
                .ellipsis(iced_core::text::Ellipsis::End),
            space::horizontal(),
            widgets::label(node.kind.label(), theme::TEXT_MUTED),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center),
    )
    .padding([9, 11])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| {
        let mut style = theme::panel(fill, None, radius);
        style.border.width = 1.0;
        style.border.color = if lit {
            theme::rgb(theme::MAGENTA)
        } else {
            edge
        };
        if lit {
            style.background = Some(iced_core::Background::Color(theme::rgba(
                theme::MAGENTA,
                0.05,
            )));
        }
        style
    })
    .into()
}

/// One agent node in the permission graph.
fn permission_agent<'a>(agent: &'a Agent, grants: usize, selected: bool) -> crate::UiElement<'a> {
    button(
        column![
            row![
                container(space::Space::new().width(5).height(5)).style(|_| theme::panel(
                    theme::MAGENTA,
                    None,
                    1.0
                )),
                widgets::mono(agent.name.clone(), theme::MAGENTA)
                    .wrapping(iced_core::text::Wrapping::None)
                    .ellipsis(iced_core::text::Ellipsis::End),
                space::horizontal(),
                widgets::label(agent.role.label(), theme::TEXT_MUTED),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
            widgets::mono(format!("{grants} grants"), theme::TEXT_MUTED),
        ]
        .spacing(2),
    )
    .padding([6, 10])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_theme, status| {
        let hovered = matches!(
            status,
            iced_widget::button::Status::Hovered | iced_widget::button::Status::Pressed
        );
        iced_widget::button::Style {
            background: Some(iced_core::Background::Color(theme::rgba(
                theme::MAGENTA,
                if selected { 0.12 } else { 0.06 },
            ))),
            text_color: theme::rgb(theme::TEXT),
            border: iced_core::Border {
                color: if selected || hovered {
                    theme::rgb(theme::MAGENTA)
                } else {
                    theme::rgba(theme::MAGENTA, 0.5)
                },
                width: 1.0,
                radius: theme::RADIUS_LG.into(),
            },
            shadow: theme::no_shadow(),
            ..Default::default()
        }
    })
    .on_press(Message::TopologySelect(agent.id.clone()))
    .into()
}

/// The filesystem view: permissions beneath, agents and paths pinned over.
pub fn filesystem_layer<'a>(
    graph: &'a Graph,
    filesystem: &'a Filesystem,
    agents: &'a [Placement],
    paths: &'a [Placement],
    state: &'a State,
) -> crate::UiElement<'a> {
    let mut layers = stack![
        iced_widget::canvas(Permissions::new(graph, filesystem, agents, paths, state))
            .width(Length::Fill)
            .height(Length::Fill)
    ];
    let chosen = state.selected.as_deref().and_then(|id| graph.index_of(id));
    for (index, agent) in graph.agents.iter().enumerate() {
        let Some(placement) = agents.get(index) else {
            continue;
        };
        let grants = filesystem
            .grants
            .iter()
            .filter(|grant| grant.agent == index)
            .count();
        layers = layers.push(
            iced_widget::pin(
                container(permission_agent(agent, grants, chosen == Some(index)))
                    .width(placement.width * state.scale)
                    .height(placement.height * state.scale),
            )
            .x(placement.x * state.scale + state.tx)
            .y(placement.y * state.scale + state.ty),
        );
    }
    for (index, node) in filesystem.paths.iter().enumerate() {
        let Some(placement) = paths.get(index) else {
            continue;
        };
        let lit = chosen.is_some_and(|agent| filesystem.covers(agent, index));
        layers = layers.push(
            iced_widget::pin(
                container(path_node(node, lit))
                    .width(placement.width * state.scale)
                    .height(placement.height * state.scale),
            )
            .x(placement.x * state.scale + state.tx)
            .y(placement.y * state.scale + state.ty),
        );
    }
    layers.width(Length::Fill).height(Length::Fill).into()
}
