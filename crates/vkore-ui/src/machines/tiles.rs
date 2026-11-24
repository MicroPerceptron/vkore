//! Layout only. Tiles can swap slots, hide, or maximize independently.

use iced_core::{Length, alignment};
use iced_widget::{PaneGrid, button, column, container, pane_grid, row, tooltip};

use crate::{Message, UiElement, theme, typography::Typography, widgets};

use super::{MachineMessage, wrap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Console,
    Overview,
    BootActivity,
    Plugins,
}

impl Tile {
    const ALL: [Self; 4] = [
        Self::Console,
        Self::Overview,
        Self::BootActivity,
        Self::Plugins,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Console => "Console",
            Self::Overview => "Overview",
            Self::BootActivity => "Boot activity",
            Self::Plugins => "Plugins",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Change {
    Drag(pane_grid::DragEvent),
    Resize(pane_grid::ResizeEvent),
    Collapse(Tile),
    Show(Tile),
    Expand(pane_grid::Pane),
    Restore,
}

pub(super) struct Workspace {
    grid: Option<pane_grid::State<Tile>>,
    /// The two left slots followed by the two right slots, including hidden tiles.
    order: [Tile; 4],
    collapsed: [bool; 4],
    columns_split: Option<pane_grid::Split>,
    left_split: Option<pane_grid::Split>,
    right_split: Option<pane_grid::Split>,
    columns_ratio: f32,
    left_ratio: f32,
    right_ratio: f32,
}

impl Default for Workspace {
    fn default() -> Self {
        let mut workspace = Self {
            grid: None,
            order: [
                Tile::Console,
                Tile::Plugins,
                Tile::Overview,
                Tile::BootActivity,
            ],
            collapsed: [false; 4],
            columns_split: None,
            left_split: None,
            right_split: None,
            columns_ratio: 0.57,
            left_ratio: 0.66,
            right_ratio: 0.54,
        };
        workspace.rebuild();
        workspace
    }
}

impl Workspace {
    fn rebuild(&mut self) {
        use pane_grid::Axis;

        self.columns_split = None;
        self.left_split = None;
        self.right_split = None;
        let visible = |tile: &Tile| !self.collapsed[*tile as usize];
        let (left_slots, right_slots) = self.order.split_at(2);
        let mut left = left_slots.iter().copied().filter(visible);
        let mut right = right_slots.iter().copied().filter(visible);
        let left_tile = left.next();
        let right_tile = right.next();
        let Some(first) = left_tile.or(right_tile) else {
            self.grid = None;
            return;
        };
        let (mut grid, first_pane) = pane_grid::State::new(first);
        let mut right_pane = first_pane;
        if left_tile.is_some()
            && let Some(tile) = right_tile
            && let Some((pane, split)) = grid.split(Axis::Vertical, first_pane, tile)
        {
            grid.resize(split, self.columns_ratio);
            self.columns_split = Some(split);
            right_pane = pane;
        }
        if let Some(tile) = right.next()
            && let Some((_, split)) = grid.split(Axis::Horizontal, right_pane, tile)
        {
            grid.resize(split, self.right_ratio);
            self.right_split = Some(split);
        }
        if let Some(tile) = left.next()
            && let Some((_, split)) = grid.split(Axis::Horizontal, first_pane, tile)
        {
            grid.resize(split, self.left_ratio);
            self.left_split = Some(split);
        }
        self.grid = Some(grid);
    }

    pub(super) fn change(&mut self, change: Change) {
        match change {
            Change::Drag(pane_grid::DragEvent::Dropped {
                pane,
                target: pane_grid::Target::Pane(target, _),
            }) => {
                if let Some(grid) = &mut self.grid
                    && grid.maximized().is_none()
                    && pane != target
                    && let (Some(&first), Some(&second)) = (grid.get(pane), grid.get(target))
                {
                    grid.swap(pane, target);
                    // Rebuilding after a hide or show restores the swapped slots.
                    for tile in &mut self.order {
                        if *tile == first {
                            *tile = second;
                        } else if *tile == second {
                            *tile = first;
                        }
                    }
                }
            }
            Change::Drag(_) => {}
            Change::Collapse(tile) | Change::Show(tile) => {
                self.collapsed[tile as usize] = matches!(change, Change::Collapse(_));
                self.rebuild();
            }
            Change::Resize(event) => {
                if let Some(grid) = &mut self.grid
                    && event.ratio.is_finite()
                {
                    let ratio = event.ratio.clamp(0.15, 0.85);
                    if self.columns_split == Some(event.split) {
                        self.columns_ratio = ratio;
                    } else if self.right_split == Some(event.split) {
                        self.right_ratio = ratio;
                    } else if self.left_split == Some(event.split) {
                        self.left_ratio = ratio;
                    } else {
                        return;
                    }
                    grid.resize(event.split, ratio);
                }
            }
            Change::Expand(pane) => {
                if let Some(grid) = &mut self.grid
                    && grid.get(pane).is_some()
                {
                    grid.maximize(pane);
                }
            }
            Change::Restore => {
                if let Some(grid) = &mut self.grid {
                    grid.restore();
                }
            }
        }
    }

    pub(super) fn view<'a>(
        &'a self,
        name: &'a str,
        ty: Typography,
        contents: impl Fn(Tile) -> UiElement<'a>,
    ) -> UiElement<'a> {
        let message = move |change| wrap(MachineMessage::TileLayout(name.to_owned(), change));
        let hidden = Tile::ALL
            .into_iter()
            .filter(|tile| self.collapsed[*tile as usize]);
        let mut reveal = row![].spacing(theme::SPACE_2);
        for tile in hidden {
            reveal = reveal.push(
                button(ty.control(format!("Show {}", tile.title())))
                    .height(ty.control_height())
                    .padding([0, 10])
                    .style(widgets::button_style(widgets::Button::Secondary))
                    .on_press(message(Change::Show(tile))),
            );
        }
        let board: UiElement<'_> = match &self.grid {
            Some(grid) => PaneGrid::new(grid, |pane, tile, maximized| {
                let controls: UiElement<'_> = row![
                    tile_button(ty, "-", "Collapse tile", message(Change::Collapse(*tile))),
                    if maximized {
                        tile_button(ty, "=", "Restore tile layout", message(Change::Restore))
                    } else {
                        tile_button(ty, "+", "Expand tile", message(Change::Expand(pane)))
                    },
                ]
                .align_y(alignment::Vertical::Center)
                .spacing(theme::SPACE_1)
                .into();
                // The drag preview gives the title bar an infinite viewport.
                // Bound its text just as we do the body before recording it.
                let title = pane_grid::TitleBar::new(
                    container(ty.body(tile.title(), 14., theme::TEXT)).clip(true),
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
        if self.collapsed.contains(&true) {
            layout = layout.push(reveal.wrap());
        }
        layout.push(board).into()
    }
}

fn tile_button<'a>(
    ty: Typography,
    glyph: &'static str,
    label: &'static str,
    message: Message,
) -> UiElement<'a> {
    tooltip(
        button(ty.control(glyph))
            .height(ty.control_height())
            .padding([0, 7])
            .style(widgets::button_style(widgets::Button::Ghost))
            .on_press(message),
        container(ty.body(label, 12., theme::TEXT))
            .padding(8)
            .style(widgets::card)
            .clip(true),
        tooltip::Position::Bottom,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_core::{Event, Point, Rectangle, Renderer as _, Size, Theme, mouse, shell};
    use iced_runtime::user_interface;

    #[test]
    fn title_bar_drag_swaps_tiles_and_survives_a_collapse() {
        let mut workspace = Workspace::default();
        let size = Size::new(1000., 640.);
        let tile_bounds = |workspace: &Workspace, wanted: Tile| {
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
            let element = workspace.view("demo", Typography::default(), |_| {
                iced_widget::space::vertical().into()
            });
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
        let mut workspace = Workspace::default();
        workspace.change(Change::Resize(pane_grid::ResizeEvent {
            split: workspace.columns_split.unwrap(),
            ratio: 0.68,
        }));
        for tile in Tile::ALL {
            workspace.change(Change::Collapse(tile));
        }
        assert!(workspace.grid.is_none());
        for tile in Tile::ALL.into_iter().rev() {
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
        workspace.change(Change::Expand(console));
        assert_eq!(workspace.grid.as_ref().unwrap().maximized(), Some(console));
        workspace.change(Change::Restore);
        assert_eq!(workspace.grid.as_ref().unwrap().maximized(), None);
        assert_eq!(workspace.grid.as_ref().unwrap().len(), 4);
    }
}
