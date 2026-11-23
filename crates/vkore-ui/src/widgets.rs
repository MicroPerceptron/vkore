//! MONOLITH control styling.
//!
//! Every control in the system is a fill, a one-pixel border, and text. The
//! kinds below are the complete set the design uses; a screen picks one rather
//! than assembling colours itself.
//!
//! Two behaviours are shared by all of them and are the reason this is one
//! module rather than per-screen styling:
//!
//! - Hover moves a border to magenta and muted text to off-white, or fills a
//!   ghost row. It never fades opacity, never scales, and never floods a fill.
//! - Disabled is 40% opacity, applied to whatever the kind already resolved.
//!
//! Typography and text sizing live in `typography`; its section labels are uppercase. Component-button and navigation labels
//! preserve the case supplied by the screen, exactly like the published
//! MONOLITH components, and inherit their colour from the control style so a
//! disabled control dims its label too.

use crate::theme;
use iced_core::{Background, Border, Color, Length, Theme};
use iced_widget::{button, container, scrollable, text_input};
use std::fmt;

/// The controls the design defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Black fill, magenta border. Open, Create, Start in a window.
    Primary,
    /// Navigation with a neutral selected surface.
    Navigation(bool),
    /// Hairline border, secondary text. Refresh, Save, Import, Start headless.
    Secondary,
    /// Hairline border, error text. Remove, Stop.
    Danger,
    /// Bare until hover. Close, a breadcrumb.
    Ghost,
    /// A selectable option: the navigation rail, a volume's places.
    Choice(bool),
    /// A list row: an entry, a machine. Selected carries a magenta border;
    /// unselected has none.
    Row(bool),
}

/// Resolved colours for one control state, before the disabled dimming.
struct Face {
    fill: Option<theme::Tone>,
    text: theme::Tone,
    edge: Option<theme::Tone>,
}

impl Button {
    fn face(self, hovered: bool) -> Face {
        match self {
            Self::Navigation(selected) => Face {
                fill: (selected || hovered).then_some(theme::HOVER),
                text: if selected || hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_SECONDARY
                },
                edge: None,
            },
            // Magenta is never a fill: the primary button is the page black
            // with a magenta border. It reads off-white and gives the accent
            // to its label on hover, not the other way round.
            Self::Primary => Face {
                fill: Some(if hovered {
                    theme::PANEL
                } else {
                    theme::BACKGROUND
                }),
                text: if hovered { theme::MAGENTA } else { theme::TEXT },
                edge: Some(theme::MAGENTA),
            },
            // Secondary and danger carry no fill: a border and a label.
            Self::Secondary => Face {
                fill: None,
                text: theme::TEXT,
                edge: Some(if hovered {
                    theme::MAGENTA
                } else {
                    theme::BORDER
                }),
            },
            Self::Danger => Face {
                fill: None,
                text: theme::ERROR,
                edge: Some(theme::ERROR),
            },
            Self::Ghost => Face {
                fill: hovered.then_some(theme::HOVER),
                text: if hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_MUTED
                },
                edge: None,
            },
            Self::Choice(selected) => Face {
                fill: None,
                text: if selected || hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_MUTED
                },
                edge: Some(if selected || hovered {
                    theme::MAGENTA
                } else {
                    theme::BORDER
                }),
            },
            // Hover only tints a row: in a list longer than the pane, the
            // selection can scroll out of view, and a hovered row with its
            // border would pass for it.
            Self::Row(selected) => Face {
                fill: hovered.then_some(theme::HOVER),
                text: if selected {
                    theme::TEXT
                } else {
                    theme::TEXT_SECONDARY
                },
                edge: selected.then_some(theme::MAGENTA),
            },
        }
    }
}

/// Styles a control, applying hover and the disabled dimming.
pub fn button_style(kind: Button) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    move |appearance, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        let dim = |mut color: Color| {
            if disabled {
                color.a *= 0.4;
            }
            color
        };
        let face = kind.face(hovered && !disabled);
        button::Style {
            background: face
                .fill
                .map(|fill| Background::Color(dim(theme::color(appearance, fill)))),
            text_color: dim(theme::color(appearance, face.text)),
            border: Border {
                color: face.edge.map_or(Color::TRANSPARENT, |edge| {
                    dim(theme::color(appearance, edge))
                }),
                width: face.edge.map_or(0.0, |_| 1.0),
                radius: theme::RADIUS_MD.into(),
            },
            shadow: theme::no_shadow(),
            ..Default::default()
        }
    }
}

/// Every text field's treatment.
pub fn field(appearance: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        // A field sits on the page black, not on a panel: it is a well cut
        // into the surface rather than a card resting on it.
        background: Background::Color(theme::color(appearance, theme::BACKGROUND)),
        border: theme::border(
            appearance,
            if focused {
                theme::MAGENTA
            } else {
                theme::BORDER
            },
            theme::RADIUS_MD,
        ),
        placeholder: theme::color(appearance, theme::TEXT_MUTED),
        value: theme::color(appearance, theme::TEXT),
        selection: theme::rgba(appearance, theme::MAGENTA, 0.28),
    }
}

/// Scroll container with the same slim, neutral thumb on every page.
pub fn scroll<'a>(
    content: impl Into<crate::UiElement<'a>>,
) -> scrollable::Scrollable<'a, crate::Message, Theme, crate::AxUi> {
    iced_widget::scrollable(content)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::default()
                .width(6)
                .scroller_width(4)
                .margin(2)
                // Reserve space instead of painting over card borders and fields.
                .spacing(6),
        ))
        .style(scrollbar)
}

/// Quiet, visible scroll thumbs make content below the fold discoverable.
pub fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let mut style = scrollable::default(theme, status);
    for rail in [&mut style.vertical_rail, &mut style.horizontal_rail] {
        rail.background = None;
        rail.border = Border::default();
        rail.scroller.background =
            Background::Color(crate::theme::rgba(theme, crate::theme::TEXT_MUTED, 0.35));
        rail.scroller.border = Border {
            radius: 4.0.into(),
            ..Border::default()
        };
    }
    style.gap = None;
    style
}

/// `n` and its noun in agreement: "1 file", "2 files".
pub fn count<N: fmt::Display + PartialEq + From<u8>>(n: N, one: &str, many: &str) -> String {
    let noun = if n == N::from(1) { one } else { many };
    format!("{n} {noun}")
}

/// A shared logical-pixel content measure. Extra desktop width becomes equal
/// gutters instead of stretching forms or leaving a column pinned to one edge.
pub fn page_frame<'a>(content: impl Into<crate::UiElement<'a>>) -> crate::UiElement<'a> {
    container(
        container(content)
            .id("page-content")
            .width(Length::Fill.max(1440))
            .height(Length::Fill),
    )
    .id("page-frame")
    .center_x(Length::Fill)
    .height(Length::Fill)
    .padding(theme::SPACE_6)
    .into()
}

pub fn card(appearance: &Theme) -> container::Style {
    theme::panel(
        appearance,
        theme::PANEL,
        Some(theme::SEPARATOR),
        theme::RADIUS_LG,
    )
}

/// Resource sliders share the workspace accent and neutral track.
pub fn resource_slider(
    appearance: &Theme,
    status: iced_widget::slider::Status,
) -> iced_widget::slider::Style {
    let mut style = iced_widget::slider::default(appearance, status);
    style.rail.backgrounds = (
        theme::color(appearance, theme::MAGENTA).into(),
        theme::color(appearance, theme::BORDER).into(),
    );
    style.handle.background = theme::color(appearance, theme::PANEL).into();
    style.handle.border_color = theme::color(appearance, theme::MAGENTA);
    style.handle.border_width = 2.;
    style
}

/// A native switch with neutral fills and the workspace selection outline.
pub fn resource_toggle(
    appearance: &Theme,
    status: iced_widget::toggler::Status,
) -> iced_widget::toggler::Style {
    use iced_widget::toggler::Status;
    let (selected, disabled) = match status {
        Status::Active { is_toggled } | Status::Hovered { is_toggled } => (is_toggled, false),
        Status::Disabled { is_toggled } => (is_toggled, true),
    };
    let mut style = iced_widget::toggler::default(appearance, status);
    let color = |tone| {
        let mut color = theme::color(appearance, tone);
        if disabled {
            color.a *= 0.4;
        }
        color
    };
    style.background = color(theme::PANEL).into();
    style.foreground = color(if selected {
        theme::TEXT
    } else {
        theme::TEXT_MUTED
    })
    .into();
    style.background_border_width = 1.;
    style.background_border_color = color(if selected {
        theme::MAGENTA
    } else {
        theme::BORDER
    });
    style.text_color = Some(color(theme::TEXT));
    style
}

/// A small accent mark for the bounded official-image transfer.
pub fn download_progress(appearance: &Theme) -> iced_widget::progress_bar::Style {
    iced_widget::progress_bar::Style {
        background: theme::color(appearance, theme::SEPARATOR).into(),
        bar: theme::color(appearance, theme::MAGENTA).into(),
        border: iced_core::border::rounded(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(kind: Button, status: button::Status) -> button::Style {
        button_style(kind)(&Theme::Dark, status)
    }

    #[test]
    fn magenta_is_never_a_fill() {
        let magenta = theme::color(&Theme::Dark, theme::MAGENTA);
        for kind in [
            Button::Primary,
            Button::Secondary,
            Button::Danger,
            Button::Ghost,
            Button::Choice(true),
            Button::Choice(false),
            Button::Row(true),
            Button::Row(false),
        ] {
            for status in [
                button::Status::Active,
                button::Status::Hovered,
                button::Status::Pressed,
                button::Status::Disabled,
            ] {
                let style = styled(kind, status);
                assert_ne!(
                    style.background,
                    Some(Background::Color(magenta)),
                    "{kind:?} filled with the accent"
                );
            }
        }
    }

    #[test]
    fn nothing_casts_a_shadow() {
        for kind in [Button::Primary, Button::Secondary, Button::Ghost] {
            let style = styled(kind, button::Status::Active);
            assert_eq!(style.shadow.color, Color::TRANSPARENT);
            assert_eq!(style.shadow.blur_radius, 0.0);
        }
    }

    #[test]
    fn the_primary_button_is_black_with_a_magenta_border() {
        let style = styled(Button::Primary, button::Status::Active);
        assert_eq!(
            style.background,
            Some(Background::Color(theme::color(
                &Theme::Dark,
                theme::BACKGROUND
            )))
        );
        assert_eq!(
            style.border.color,
            theme::color(&Theme::Dark, theme::MAGENTA)
        );
        assert_eq!(style.border.width, 1.0);
    }

    #[test]
    fn hover_moves_a_border_to_magenta_rather_than_filling_it() {
        let rest = styled(Button::Secondary, button::Status::Active);
        let hovered = styled(Button::Secondary, button::Status::Hovered);

        assert_eq!(rest.border.color, theme::color(&Theme::Dark, theme::BORDER));
        assert_eq!(
            hovered.border.color,
            theme::color(&Theme::Dark, theme::MAGENTA)
        );
        assert_eq!(rest.background, hovered.background);
        assert_eq!(hovered.text_color, theme::color(&Theme::Dark, theme::TEXT));
    }

    #[test]
    fn selection_is_a_magenta_border_with_off_white_text() {
        let selected = styled(Button::Choice(true), button::Status::Active);
        assert_eq!(
            selected.border.color,
            theme::color(&Theme::Dark, theme::MAGENTA)
        );
        assert_eq!(selected.text_color, theme::color(&Theme::Dark, theme::TEXT));
    }

    #[test]
    fn a_hovered_row_never_passes_for_the_selected_one() {
        let selected = styled(Button::Row(true), button::Status::Active);
        let hovered = styled(Button::Row(false), button::Status::Hovered);
        assert_eq!(
            selected.border.color,
            theme::color(&Theme::Dark, theme::MAGENTA)
        );
        assert_eq!(hovered.border.width, 0.0);
        assert_eq!(
            hovered.background,
            Some(Background::Color(theme::color(&Theme::Dark, theme::HOVER)))
        );
        assert_ne!(hovered.text_color, selected.text_color);
    }

    #[test]
    fn a_count_agrees_with_its_noun() {
        assert_eq!(count(0usize, "file", "files"), "0 files");
        assert_eq!(count(1usize, "file", "files"), "1 file");
        assert_eq!(count(2u64, "byte", "bytes"), "2 bytes");
    }

    #[test]
    fn disabled_dims_to_forty_percent_and_does_not_hover() {
        let disabled = styled(Button::Primary, button::Status::Disabled);
        assert!((disabled.text_color.a - 0.4).abs() < f32::EPSILON);
        assert!((disabled.border.color.a - 0.4).abs() < f32::EPSILON);
        // A disabled control must resolve to its resting face, not its hover.
        // Primary rests off-white and takes the accent only on hover.
        assert_eq!(
            disabled.text_color.r,
            theme::color(&Theme::Dark, theme::TEXT).r
        );
    }

    /// The published component library's own variant table.
    ///
    /// Transcribed from `Button.jsx` in the design system bundle, which is
    /// what the reference renders. Each row is `(fill, text, border)` at rest
    /// and on hover. Nothing here is a judgement call.
    #[test]
    fn variants_match_the_published_component() {
        let rest = |kind| styled(kind, button::Status::Active);
        let hover = |kind| styled(kind, button::Status::Hovered);

        // primary: black fill, off-white label, magenta border; the label
        // takes the accent on hover, the fill lifts to the panel.
        let p = rest(Button::Primary);
        assert_eq!(
            p.background,
            Some(Background::Color(theme::color(
                &Theme::Dark,
                theme::BACKGROUND
            )))
        );
        assert_eq!(p.text_color, theme::color(&Theme::Dark, theme::TEXT));
        assert_eq!(p.border.color, theme::color(&Theme::Dark, theme::MAGENTA));
        let ph = hover(Button::Primary);
        assert_eq!(ph.text_color, theme::color(&Theme::Dark, theme::MAGENTA));
        assert_eq!(ph.border.color, theme::color(&Theme::Dark, theme::MAGENTA));

        // secondary: no fill at rest or on hover, only the border moves.
        let s = rest(Button::Secondary);
        let sh = hover(Button::Secondary);
        assert_eq!(s.background, None);
        assert_eq!(sh.background, None);
        assert_eq!(s.text_color, theme::color(&Theme::Dark, theme::TEXT));
        assert_eq!(sh.text_color, theme::color(&Theme::Dark, theme::TEXT));
        assert_eq!(s.border.color, theme::color(&Theme::Dark, theme::BORDER));
        assert_eq!(sh.border.color, theme::color(&Theme::Dark, theme::MAGENTA));

        // danger: the error border is present in both states.
        let d = rest(Button::Danger);
        let dh = hover(Button::Danger);
        assert_eq!(d.background, None);
        assert_eq!(dh.background, None);
        assert_eq!(d.text_color, theme::color(&Theme::Dark, theme::ERROR));
        assert_eq!(d.border.color, theme::color(&Theme::Dark, theme::ERROR));
        assert_eq!(dh.border.color, theme::color(&Theme::Dark, theme::ERROR));

        // ghost: transparent until hover, then the surface-hover fill.
        let g = rest(Button::Ghost);
        let gh = hover(Button::Ghost);
        assert_eq!(g.background, None);
        assert_eq!(
            gh.background,
            Some(Background::Color(theme::color(&Theme::Dark, theme::HOVER)))
        );
        assert_eq!(g.text_color, theme::color(&Theme::Dark, theme::TEXT_MUTED));
        assert_eq!(gh.text_color, theme::color(&Theme::Dark, theme::TEXT));
    }

    #[test]
    fn a_focused_field_takes_the_accent_border_and_selects_without_a_fill() {
        let resting = field(&Theme::Dark, text_input::Status::Active);
        let focused = field(
            &Theme::Dark,
            text_input::Status::Focused { is_hovered: false },
        );
        assert_eq!(
            resting.border.color,
            theme::color(&Theme::Dark, theme::BORDER)
        );
        assert_eq!(
            focused.border.color,
            theme::color(&Theme::Dark, theme::MAGENTA)
        );
        assert_eq!(
            resting.background,
            Background::Color(theme::color(&Theme::Dark, theme::BACKGROUND))
        );
        // Magenta is never a fill: a selection is a tint, not a block.
        assert!(focused.selection.a < 0.5);
    }
}
