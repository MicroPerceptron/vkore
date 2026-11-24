//! Layout only. A tile can be hidden or maximized without changing its machine.

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
    Resize(pane_grid::ResizeEvent),
    Collapse(Tile),
    Show(Tile),
    Expand(pane_grid::Pane),
    Restore,
}

pub(super) struct Workspace {
    grid: Option<pane_grid::State<Tile>>,
    collapsed: [bool; 4],
    console_split: Option<pane_grid::Split>,
    details_split: Option<pane_grid::Split>,
    plugins_split: Option<pane_grid::Split>,
    console_ratio: f32,
    details_ratio: f32,
    plugins_ratio: f32,
}

impl Default for Workspace {
    fn default() -> Self {
        let mut workspace = Self {
            grid: None,
            collapsed: [false; 4],
            console_split: None,
            details_split: None,
            plugins_split: None,
            console_ratio: 0.57,
            details_ratio: 0.54,
            plugins_ratio: 0.66,
        };
        workspace.rebuild();
        workspace
    }
}

impl Workspace {
    fn rebuild(&mut self) {
        use pane_grid::Axis;

        self.console_split = None;
        self.details_split = None;
        self.plugins_split = None;
        let visible = |tile: &Tile| !self.collapsed[*tile as usize];
        let mut left = [Tile::Console, Tile::Plugins].into_iter().filter(visible);
        let mut right = [Tile::Overview, Tile::BootActivity]
            .into_iter()
            .filter(visible);
        let left_tile = left.next();
        let right_tile = right.next();
        let Some(first) = left_tile.or(right_tile) else {
            self.grid = None;
            return;
        };
        let (mut grid, first_pane) = pane_grid::State::new(first);
        let mut details_pane = first_pane;
        if left_tile.is_some()
            && let Some(tile) = right_tile
            && let Some((pane, split)) = grid.split(Axis::Vertical, first_pane, tile)
        {
            grid.resize(split, self.console_ratio);
            self.console_split = Some(split);
            details_pane = pane;
        }
        if let Some(tile) = right.next()
            && let Some((_, split)) = grid.split(Axis::Horizontal, details_pane, tile)
        {
            grid.resize(split, self.details_ratio);
            self.details_split = Some(split);
        }
        if let Some(tile) = left.next()
            && let Some((_, split)) = grid.split(Axis::Horizontal, first_pane, tile)
        {
            grid.resize(split, self.plugins_ratio);
            self.plugins_split = Some(split);
        }
        self.grid = Some(grid);
    }

    pub(super) fn change(&mut self, change: Change) {
        match change {
            Change::Collapse(tile) | Change::Show(tile) => {
                self.collapsed[tile as usize] = matches!(change, Change::Collapse(_));
                self.rebuild();
            }
            Change::Resize(event) => {
                if let Some(grid) = &mut self.grid
                    && event.ratio.is_finite()
                {
                    let ratio = event.ratio.clamp(0.15, 0.85);
                    if self.console_split == Some(event.split) {
                        self.console_ratio = ratio;
                    } else if self.details_split == Some(event.split) {
                        self.details_ratio = ratio;
                    } else if self.plugins_split == Some(event.split) {
                        self.plugins_ratio = ratio;
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
                let title = pane_grid::TitleBar::new(ty.body(tile.title(), 14., theme::TEXT))
                    .padding([theme::SPACE_1, theme::SPACE_2])
                    .controls(controls)
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
            .on_resize(8, move |event| message(Change::Resize(event)))
            .style(|appearance| {
                let mut style = pane_grid::default(appearance);
                style.hovered_split.color = theme::color(appearance, theme::TEXT_MUTED);
                style.picked_split.color = theme::color(appearance, theme::MAGENTA);
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

    #[test]
    fn collapsing_and_expanding_preserve_the_local_divider() {
        let mut workspace = Workspace::default();
        workspace.change(Change::Resize(pane_grid::ResizeEvent {
            split: workspace.console_split.unwrap(),
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
