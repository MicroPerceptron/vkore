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
//! Section labels are uppercased here. Component-button and navigation labels
//! preserve the case supplied by the screen, exactly like the published
//! MONOLITH components, and inherit their colour from the control style so a
//! disabled control dims its label too.

use crate::{fonts, theme};
use iced_core::{Background, Border, Color, Length, Theme, alignment};
use iced_widget::{button, column, container, row, scrollable, text, text_input};
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
                .margin(2),
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

/// An uppercase mono label: metadata, navigation, and section markers.
pub fn label<'a>(
    content: impl AsRef<str>,
    color: theme::Tone,
) -> text::Text<'a, Theme, crate::AxUi> {
    text(content.as_ref().to_uppercase())
        .font(fonts::MONO_LABEL)
        .size(theme::SIZE_MONO)
        .line_height(iced_core::text::LineHeight::Relative(1.35))
        .style(theme::text(color))
}

/// A component-button label. Buttons preserve case and own the text colour.
///
/// Filling the button's computed inner height gives the paragraph a shape to
/// center against. This keeps every control size on the same rule and avoids
/// font- or button-specific padding offsets.
pub fn control<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a, Theme, crate::AxUi> {
    text(content)
        .font(fonts::BODY)
        .size(theme::SIZE_SM)
        .line_height(iced_core::text::LineHeight::Relative(1.35))
        .height(Length::Fill)
        .align_y(alignment::Vertical::Center)
}

/// A navigation item, centered inside its control.
pub fn nav<'a>(content: impl text::IntoFragment<'a>) -> text::Text<'a, Theme, crate::AxUi> {
    text(content)
        .font(fonts::BODY)
        .size(theme::SIZE_SM)
        .line_height(iced_core::text::LineHeight::Relative(1.35))
        .height(Length::Fill)
        .align_y(alignment::Vertical::Center)
}

/// Machine-shaped text the binary wrote: labels, sizes, hashes. Text from
/// outside it goes through [`data`].
pub fn mono<'a>(
    content: impl text::IntoFragment<'a>,
    color: theme::Tone,
) -> text::Text<'a, Theme, crate::AxUi> {
    text(content)
        .font(fonts::MONO)
        .size(theme::SIZE_MONO)
        .line_height(iced_core::text::LineHeight::Relative(1.35))
        .style(theme::text(color))
}

/// Text from outside the binary -- a name a guest wrote, its logs, a host
/// path, an error quoting one -- set as [`mono`] through [`fonts::shown`], so
/// it can neither break the frame nor pass for something it is not.
pub fn data<'a>(
    content: impl Into<std::borrow::Cow<'a, str>>,
    color: theme::Tone,
) -> text::Text<'a, Theme, crate::AxUi> {
    mono(
        fonts::shown(fonts::MONO, content, fonts::Breaks::Escape),
        color,
    )
}

/// Prose the binary wrote.
pub fn body<'a>(
    content: impl text::IntoFragment<'a>,
    size: f32,
    color: theme::Tone,
) -> text::Text<'a, Theme, crate::AxUi> {
    text(content)
        .font(fonts::BODY)
        .size(size)
        .line_height(iced_core::text::LineHeight::Relative(1.35))
        .style(theme::text(color))
}

/// `n` and its noun in agreement: "1 file", "2 files".
pub fn count<N: fmt::Display + PartialEq + From<u8>>(n: N, one: &str, many: &str) -> String {
    let noun = if n == N::from(1) { one } else { many };
    format!("{n} {noun}")
}

/// A page heading, with case supplied by the screen.
pub fn display<'a>(content: impl AsRef<str>) -> text::Text<'a, Theme, crate::AxUi> {
    text(content.as_ref().to_owned())
        .font(fonts::DISPLAY)
        .size(theme::SIZE_DISPLAY)
        .line_height(iced_core::text::LineHeight::Relative(1.25))
        .style(theme::text(theme::TEXT))
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

/// A page title and its one-line purpose.
pub fn page_header<'a>(title: &'static str, subtitle: &'static str) -> crate::UiElement<'a> {
    column![
        display(title),
        body(subtitle, theme::SIZE_SM, theme::TEXT_MUTED)
    ]
    .spacing(6)
    .into()
}

pub fn badge<'a>(label: impl text::IntoFragment<'a>, tone: theme::Tone) -> crate::UiElement<'a> {
    container(
        row![body("·", 16., tone), body(label, 12., tone)]
            .spacing(5)
            .align_y(alignment::Vertical::Center),
    )
    .padding([3, 9])
    .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 20.))
    .into()
}

pub fn empty_state<'a>(
    icon: crate::visuals::Icon,
    title: &'static str,
    description: &'static str,
) -> crate::UiElement<'a> {
    container(
        column![
            container(crate::visuals::icon(icon, theme::TEXT_MUTED, 30.))
                .padding(18)
                .style(|appearance| theme::panel(
                    appearance,
                    theme::PANEL_FILE,
                    Some(theme::SEPARATOR),
                    16.
                )),
            body(title, 20., theme::TEXT),
            body(description, theme::SIZE_SM, theme::TEXT_MUTED)
                .align_x(iced_core::text::Alignment::Center),
        ]
        .spacing(14)
        .align_x(alignment::Horizontal::Center)
        .width(Length::Fill.max(360)),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .padding(24)
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
