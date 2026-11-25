//! Layout for machine tiles. The caller supplies tile identities and contents.

use iced_core::{Length, alignment};
use iced_widget::{PaneGrid, column, container, pane_grid, row};
use pane_grid::{Axis, Configuration, Node, Split};

use crate::{
    Message, UiElement, kit, theme,
    typography::Typography,
    visuals::{self, Icon},
    widgets,
};

pub(super) struct TileInfo {
    pub title: &'static str,
    /// What kind of thing the tile is, shown before its title.
    pub icon: Icon,
    pub closable: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Change<T> {
    Drag(pane_grid::DragEvent),
    Resize(pane_grid::ResizeEvent),
    Collapse(T),
    Show(T),
    Close(T),
    Expand(T),
    Restore,
}

pub(super) enum Layout<T> {
    Tile {
        id: T,
        hidden: bool,
    },
    Split {
        axis: Axis,
        ratio: f32,
        divider: Option<Split>,
        first: Box<Self>,
        second: Box<Self>,
    },
}

impl<T: Copy + Eq> Layout<T> {
    pub fn tile(id: T) -> Self {
        Self::Tile { id, hidden: false }
    }

    pub fn split(axis: Axis, ratio: f32, first: Self, second: Self) -> Self {
        Self::Split {
            axis,
            ratio,
            divider: None,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn configure<'a>(
        &'a mut self,
        dividers: &mut Vec<&'a mut Option<Split>>,
    ) -> Option<Configuration<T>> {
        match self {
            Self::Tile { id, hidden } => (!*hidden).then_some(Configuration::Pane(*id)),
            Self::Split {
                axis,
                ratio,
                divider,
                first,
                second,
            } => {
                *divider = None;
                let index = dividers.len();
                match (first.configure(dividers), second.configure(dividers)) {
                    (Some(a), Some(b)) => {
                        // PaneGrid's layout has the same preorder as its configuration.
                        dividers.insert(index, divider);
                        Some(Configuration::Split {
                            axis: *axis,
                            ratio: *ratio,
                            a: Box::new(a),
                            b: Box::new(b),
                        })
                    }
                    (a, b) => a.or(b),
                }
            }
        }
    }

    fn remove(self, tile: T) -> Option<Self> {
        match self {
            Self::Tile { id, .. } if id == tile => None,
            Self::Tile { .. } => Some(self),
            Self::Split {
                axis,
                ratio,
                first,
                second,
                ..
            } => match (first.remove(tile), second.remove(tile)) {
                (Some(first), Some(second)) => Some(Self::split(axis, ratio, first, second)),
                (first, second) => first.or(second),
            },
        }
    }

    fn visit(&mut self, f: &mut impl FnMut(&mut T, &mut bool)) {
        match self {
            Self::Tile { id, hidden } => f(id, hidden),
            Self::Split { first, second, .. } => {
                first.visit(f);
                second.visit(f);
            }
        }
    }

    fn hidden(&self, tiles: &mut Vec<T>) {
        match self {
            Self::Tile { id, hidden: true } => tiles.push(*id),
            Self::Tile { .. } => {}
            Self::Split { first, second, .. } => {
                first.hidden(tiles);
                second.hidden(tiles);
            }
        }
    }

    fn set_ratio(&mut self, split: Split, value: f32) -> bool {
        match self {
            Self::Tile { .. } => false,
            Self::Split {
                ratio,
                divider,
                first,
                second,
                ..
            } => {
                if *divider == Some(split) {
                    *ratio = value;
                    true
                } else {
                    first.set_ratio(split, value) || second.set_ratio(split, value)
                }
            }
        }
    }
}

fn bind_dividers<'a>(node: &Node, ids: &mut impl Iterator<Item = &'a mut Option<Split>>) {
    if let Node::Split { id, a, b, .. } = node {
        *ids.next()
            .expect("one divider binding per configured split") = Some(*id);
        bind_dividers(a, ids);
        bind_dividers(b, ids);
    }
}

pub(super) struct Workspace<T> {
    layout: Option<Layout<T>>,
    grid: Option<pane_grid::State<T>>,
}

impl<T: Copy + Eq> Workspace<T> {
    pub fn new(layout: Layout<T>) -> Self {
        let mut workspace = Self {
            layout: Some(layout),
            grid: None,
        };
        workspace.rebuild();
        workspace
    }

    fn rebuild(&mut self) {
        let mut dividers = Vec::new();
        self.grid = self.layout.as_mut().and_then(|layout| {
            let grid = pane_grid::State::with_configuration(layout.configure(&mut dividers)?);
            bind_dividers(grid.layout(), &mut dividers.into_iter());
            Some(grid)
        });
    }

    #[cfg(any(test, feature = "browser-preview"))]
    pub fn append(&mut self, id: T) {
        let tile = Layout::tile(id);
        self.layout = Some(match self.layout.take() {
            Some(layout) => Layout::split(Axis::Vertical, 0.5, layout, tile),
            None => tile,
        });
        self.rebuild();
    }

    pub fn change(&mut self, change: Change<T>) {
        match change {
            Change::Drag(pane_grid::DragEvent::Dropped {
                pane,
                target: pane_grid::Target::Pane(target, _),
            }) => {
                if let Some(grid) = &mut self.grid
                    && grid.maximized().is_none()
                    && pane != target
                    && let (Some(&first), Some(&second)) = (grid.get(pane), grid.get(target))
                    && let Some(layout) = &mut self.layout
                {
                    grid.swap(pane, target);
                    layout.visit(&mut |tile, _| {
                        if *tile == first {
                            *tile = second;
                        } else if *tile == second {
                            *tile = first;
                        }
                    });
                }
            }
            Change::Drag(_) => {}
            Change::Collapse(tile) | Change::Show(tile) => {
                if let Some(layout) = &mut self.layout {
                    layout.visit(&mut |id, hidden| {
                        if *id == tile {
                            *hidden = matches!(change, Change::Collapse(_));
                        }
                    });
                    self.rebuild();
                }
            }
            Change::Close(tile) => {
                self.layout = self.layout.take().and_then(|layout| layout.remove(tile));
                self.rebuild();
            }
            Change::Resize(event) => {
                if let (Some(grid), Some(layout)) = (&mut self.grid, &mut self.layout)
                    && event.ratio.is_finite()
                {
                    let ratio = event.ratio.clamp(0.15, 0.85);
                    if layout.set_ratio(event.split, ratio) {
                        grid.resize(event.split, ratio);
                    }
                }
            }
            Change::Expand(tile) => {
                if let Some(grid) = &mut self.grid {
                    let pane = grid
                        .iter()
                        .find(|(_, id)| **id == tile)
                        .map(|(pane, _)| *pane);
                    if let Some(pane) = pane {
                        grid.maximize(pane);
                    }
                }
            }
            Change::Restore => {
                if let Some(grid) = &mut self.grid {
                    grid.restore();
                }
            }
        }
    }

    pub fn view<'a>(
        &'a self,
        ty: Typography,
        describe: impl Fn(T) -> TileInfo,
        contents: impl Fn(T) -> UiElement<'a>,
        message: impl Fn(Change<T>) -> Message + Copy + 'a,
    ) -> UiElement<'a> {
        let mut hidden = Vec::new();
        if let Some(layout) = &self.layout {
            layout.hidden(&mut hidden);
        }
        let mut reveal = row![].spacing(theme::SPACE_2);
        for tile in &hidden {
            let info = describe(*tile);
            reveal = reveal.push(kit::icon_label(
                ty,
                info.icon,
                info.title,
                widgets::Button::Secondary,
                Some(message(Change::Show(*tile))),
            ));
        }
        let board: UiElement<'_> = match &self.grid {
            Some(grid) => PaneGrid::new(grid, |_, tile, maximized| {
                let info = describe(*tile);
                let mut controls = row![
                    tile_button(
                        ty,
                        Icon::Minus,
                        "Collapse tile",
                        message(Change::Collapse(*tile))
                    ),
                    if maximized {
                        tile_button(
                            ty,
                            Icon::Restore,
                            "Restore tile layout",
                            message(Change::Restore),
                        )
                    } else {
                        tile_button(
                            ty,
                            Icon::Maximize,
                            "Expand tile",
                            message(Change::Expand(*tile)),
                        )
                    },
                ];
                if info.closable {
                    controls = controls.push(tile_button(
                        ty,
                        Icon::Close,
                        "Close tile",
                        message(Change::Close(*tile)),
                    ));
                }
                let controls = controls
                    .align_y(alignment::Vertical::Center)
                    .spacing(theme::SPACE_1);
                // The drag preview gives the title bar an infinite viewport.
                // Bound its text just as we do the body before recording it.
                let title = pane_grid::TitleBar::new(
                    container(
                        row![
                            // The grip says the bar can be dragged to swap tiles.
                            visuals::icon(Icon::Grip, theme::TEXT_MUTED, ty.size(14.)),
                            visuals::icon(info.icon, theme::TEXT_SECONDARY, ty.size(16.)),
                            ty.body(info.title, 14., theme::TEXT),
                        ]
                        .spacing(theme::SPACE_2)
                        .align_y(alignment::Vertical::Center),
                    )
                    .clip(true),
                )
                .padding([theme::SPACE_1, theme::SPACE_2])
                .controls(UiElement::from(container(controls).clip(true)))
                .always_show_controls();
                pane_grid::Content::new(
                    container(contents(*tile))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(theme::SPACE_2)
                        .clip(true),
                )
                .title_bar(title)
                .style(widgets::card)
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .spacing(theme::SPACE_2)
            .min_size(140)
            .on_drag(move |event| message(Change::Drag(event)))
            .on_resize(8, move |event| message(Change::Resize(event)))
            .style(|appearance| {
                let mut style = pane_grid::default(appearance);
                let accent = theme::color(appearance, theme::MAGENTA);
                style.hovered_region.background = iced_core::Color { a: 0.12, ..accent }.into();
                style.hovered_region.border.color = accent;
                style.hovered_split.color = theme::color(appearance, theme::TEXT_MUTED);
                style.picked_split.color = accent;
                style
            })
            .into(),
            None => container(ty.body("Choose a tile to show.", 14., theme::TEXT_MUTED))
                .center(Length::Fill)
                .style(widgets::card)
                .into(),
        };
        let mut layout = column![].spacing(theme::SPACE_2).height(Length::Fill);
        if !hidden.is_empty() {
            layout = layout.push(reveal.wrap());
        }
        layout.push(board).into()
    }
}

/// A control in a tile's title bar: a glyph, named by its tooltip.
fn tile_button<'a>(
    ty: Typography,
    icon: Icon,
    label: &'static str,
    message: Message,
) -> UiElement<'a> {
    kit::icon_button(ty, icon, label, widgets::Button::Ghost, Some(message))
}

#[cfg(test)]
mod tests {
    use super::super::{MachineMessage, Tile, workspace, wrap};
    use super::*;
    use iced_core::{Event, Point, Rectangle, Renderer as _, Size, Theme, mouse, shell};
    use iced_runtime::user_interface;

    #[test]
    fn title_bar_drag_swaps_tiles_and_survives_a_collapse() {
        let mut workspace = workspace();
        let size = Size::new(1000., 640.);
        let tile_bounds = |workspace: &Workspace<Tile>, wanted: Tile| {
            let grid = workspace.grid.as_ref().unwrap();
            let pane = grid.iter().find(|(_, tile)| **tile == wanted).unwrap().0;
            grid.layout().pane_regions(theme::SPACE_2, 140., size)[pane]
        };
        let console = tile_bounds(&workspace, Tile::Console);
        let overview = tile_bounds(&workspace, Tile::Overview);
        let grab = Point::new(console.x + console.width * 0.6, console.y + 16.);
        let drop_at = overview.center();
        let mut renderer = crate::native_renderer();
        crate::fonts::register().unwrap();
        renderer.reset(Rectangle::with_size(size));
        let mut cache = user_interface::Cache::new();
        for (position, event) in [
            (grab, mouse::Event::ButtonPressed(mouse::Button::Left)),
            (drop_at, mouse::Event::CursorMoved { position: drop_at }),
            (drop_at, mouse::Event::ButtonReleased(mouse::Button::Left)),
        ] {
            let cursor = mouse::Cursor::Available(position);
            let element = workspace.view(
                Typography::default(),
                Tile::info,
                |_| iced_widget::space::vertical().into(),
                |change| wrap(MachineMessage::TileLayout("demo".into(), change)),
            );
            let mut interface =
                user_interface::UserInterface::build(element, size, cache, &mut renderer);
            let mut messages = shell::Bus::new();
            interface.update(
                &iced_core::window::Headless,
                &shell::Waker::noop(),
                &[Event::Mouse(event)],
                cursor,
                &mut renderer,
                &mut messages,
            );
            if matches!(event, mouse::Event::CursorMoved { .. }) {
                interface.draw(&mut renderer, &Theme::Dark, &Default::default(), cursor);
                let frame = renderer.take_frame().unwrap();
                axgfx::frame_requirements(frame.view()).unwrap();
                assert!(renderer.take_text_errors().is_empty());
            }
            cache = interface.into_cache();
            for message in messages {
                if let Message::Machine(MachineMessage::TileLayout(_, change)) = message {
                    workspace.change(change);
                }
            }
        }

        assert_eq!(tile_bounds(&workspace, Tile::Console), overview);
        assert_eq!(tile_bounds(&workspace, Tile::Overview), console);
        workspace.change(Change::Collapse(Tile::Console));
        workspace.change(Change::Show(Tile::Console));
        assert_eq!(tile_bounds(&workspace, Tile::Console), overview);
        assert_eq!(tile_bounds(&workspace, Tile::Overview), console);
    }

    #[test]
    fn collapsing_and_expanding_preserve_the_local_divider() {
        let mut workspace = workspace();
        let Node::Split { id, .. } = *workspace.grid.as_ref().unwrap().layout() else {
            panic!("missing divider");
        };
        workspace.change(Change::Resize(pane_grid::ResizeEvent {
            split: id,
            ratio: 0.68,
        }));
        for tile in Tile::BUILTIN {
            workspace.change(Change::Collapse(tile));
        }
        assert!(workspace.grid.is_none());
        for tile in Tile::BUILTIN.into_iter().rev() {
            workspace.change(Change::Show(tile));
        }
        let grid = workspace.grid.as_ref().unwrap();
        let pane_grid::Node::Split { ratio, .. } = grid.layout() else {
            panic!("missing divider");
        };
        assert_eq!(*ratio, 0.68);
        let console = *grid
            .iter()
            .find(|(_, tile)| **tile == Tile::Console)
            .unwrap()
            .0;
        workspace.change(Change::Expand(Tile::Console));
        assert_eq!(workspace.grid.as_ref().unwrap().maximized(), Some(console));
        workspace.change(Change::Restore);
        assert_eq!(workspace.grid.as_ref().unwrap().maximized(), None);
        assert_eq!(workspace.grid.as_ref().unwrap().len(), 4);
    }

    #[test]
    fn a_dynamic_tile_can_collapse_expand_and_close() {
        let mut workspace = Workspace::new(Layout::split(
            Axis::Horizontal,
            0.64,
            Layout::tile(1),
            Layout::tile(2),
        ));
        workspace.append(3);
        assert_eq!(workspace.grid.as_ref().unwrap().len(), 3);
        workspace.change(Change::Collapse(3));
        assert_eq!(workspace.grid.as_ref().unwrap().len(), 2);
        workspace.change(Change::Show(3));
        workspace.change(Change::Expand(3));
        let grid = workspace.grid.as_ref().unwrap();
        assert_eq!(grid.get(grid.maximized().unwrap()), Some(&3));
        workspace.change(Change::Close(3));
        let grid = workspace.grid.as_ref().unwrap();
        assert_eq!(grid.len(), 2);
        let Node::Split { ratio, .. } = grid.layout() else {
            panic!("missing original divider");
        };
        assert_eq!(*ratio, 0.64);
    }
}
