//! MONOLITH control styling.
//!
//! The kinds below are the complete set of controls the design uses; a screen
//! picks one rather than assembling colours itself.
//!
//! Behaviours shared by all of them, which is why this is one module rather
//! than per-screen styling:
//!
//! - The primary action is filled with the accent; nothing else is. A
//!   selection is a faint tint of the accent with an accent edge; a danger is
//!   a faint tint of the error colour. Everything else is a surface or bare.
//! - Hover lifts a fill one step and brightens an edge. It never moves layout
//!   and never scales.
//! - Disabled is 40% opacity, applied to whatever the kind already resolved,
//!   and never hovers.
//!
//! Typography and text sizing live in `typography`; its section labels are uppercase. Component-button and navigation labels
//! preserve the case supplied by the screen and inherit their colour from the
//! control style, so a disabled control dims its label too.

use crate::theme;
use iced_core::{Background, Border, Color, Length, Theme};
use iced_widget::{button, container, scrollable, text_input};
use std::fmt;

/// The controls the design defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Accent fill, white label. Open, Create, Start.
    Primary,
    /// Navigation with a tinted selected surface.
    Navigation(bool),
    /// A raised surface with a hairline edge. Refresh, Save, Import.
    Secondary,
    /// A faint error tint, error text. Remove, Stop.
    Danger,
    /// Bare until hover. Close, a breadcrumb.
    Ghost,
    /// A selectable option: a volume's places.
    Choice(bool),
    /// A list row: an entry, a machine. Selected is tinted with an accent edge.
    Row(bool),
}

/// What fills a control.
#[derive(Clone, Copy)]
enum Fill {
    None,
    /// A surface tone.
    Tone(theme::Tone),
    /// A tone at some opacity: a tint.
    Tint(theme::Tone, f32),
    /// The accent, lifted toward white (positive) or black (negative).
    Accent(f32),
}

/// Resolved colours for one control state, before the disabled dimming.
struct Face {
    fill: Fill,
    text: theme::Tone,
    edge: Option<(theme::Tone, f32)>,
}

impl Button {
    fn face(self, hovered: bool, pressed: bool) -> Face {
        match self {
            Self::Navigation(selected) => Face {
                fill: if selected {
                    Fill::Tint(theme::MAGENTA, if hovered { 0.20 } else { 0.14 })
                } else if hovered {
                    Fill::Tone(theme::HOVER)
                } else {
                    Fill::None
                },
                text: if selected || hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_SECONDARY
                },
                edge: None,
            },
            Self::Primary => Face {
                fill: Fill::Accent(if pressed {
                    -0.10
                } else if hovered {
                    0.10
                } else {
                    0.0
                }),
                text: theme::ON_ACCENT,
                edge: None,
            },
            Self::Secondary => Face {
                fill: Fill::Tone(if hovered { theme::HOVER } else { theme::PANEL }),
                text: theme::TEXT,
                edge: Some(if hovered {
                    (theme::TEXT_MUTED, 0.7)
                } else {
                    (theme::BORDER, 1.0)
                }),
            },
            Self::Danger => Face {
                fill: Fill::Tint(theme::ERROR, if hovered { 0.20 } else { 0.10 }),
                text: theme::ERROR,
                edge: Some((theme::ERROR, if hovered { 0.8 } else { 0.45 })),
            },
            Self::Ghost => Face {
                fill: if hovered {
                    Fill::Tone(theme::HOVER)
                } else {
                    Fill::None
                },
                text: if hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_MUTED
                },
                edge: None,
            },
            Self::Choice(selected) => Face {
                fill: if selected {
                    Fill::Tint(theme::MAGENTA, 0.12)
                } else if hovered {
                    Fill::Tone(theme::HOVER)
                } else {
                    Fill::None
                },
                text: if selected || hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_MUTED
                },
                edge: Some(if selected {
                    (theme::MAGENTA, 1.0)
                } else if hovered {
                    (theme::TEXT_MUTED, 0.7)
                } else {
                    (theme::BORDER, 1.0)
                }),
            },
            // In a list longer than the pane the selection can scroll out of
            // view, so the selected row carries both a tint and an edge, and a
            // hovered row only a neutral lift: the two never pass for each other.
            Self::Row(selected) => Face {
                fill: if selected {
                    Fill::Tint(theme::MAGENTA, 0.09)
                } else if hovered {
                    Fill::Tone(theme::HOVER)
                } else {
                    Fill::None
                },
                text: if selected {
                    theme::TEXT
                } else {
                    theme::TEXT_SECONDARY
                },
                edge: selected.then_some((theme::MAGENTA, 0.6)),
            },
        }
    }
}

impl Button {
    /// The wash a `motion::Fade` lays over this control on hover: its tone,
    /// and how opaque it gets at most.
    pub fn wash(self) -> (theme::Tone, f32) {
        match self {
            Self::Primary => (theme::ON_ACCENT, 0.16),
            Self::Danger => (theme::ERROR, 0.12),
            Self::Secondary | Self::Navigation(_) | Self::Choice(_) => (theme::TEXT, 0.07),
            Self::Ghost => (theme::TEXT, 0.08),
            Self::Row(_) => (theme::TEXT, 0.05),
        }
    }
}

/// Styles a control, applying hover and the disabled dimming. Hover changes
/// the fill at once; use [`button_style_fading`] under a `motion::Fade`.
pub fn button_style(kind: Button) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    styled(kind, false)
}

/// Styles a control that a `motion::Fade` wraps: the fill stays at rest,
/// because the wash is what lifts it; the edge and the label still answer the
/// pointer, which is cheap to do at once.
pub fn button_style_fading(
    kind: Button,
) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    styled(kind, true)
}

fn styled(kind: Button, fading: bool) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    move |appearance, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let pressed = matches!(status, button::Status::Pressed);
        let disabled = matches!(status, button::Status::Disabled);
        let dim = |mut color: Color| {
            if disabled {
                color.a *= 0.4;
            }
            color
        };
        let face = kind.face(hovered && !disabled, pressed && !disabled);
        let fill = if fading {
            kind.face(false, pressed && !disabled).fill
        } else {
            face.fill
        };
        let background = match fill {
            Fill::None => None,
            Fill::Tone(tone) => Some(Background::Color(dim(theme::color(appearance, tone)))),
            Fill::Tint(tone, alpha) => {
                Some(Background::Color(dim(theme::rgba(appearance, tone, alpha))))
            }
            // A gradient cannot be dimmed channel by channel; a disabled
            // accent is the plain accent at forty percent.
            Fill::Accent(_) if disabled => Some(Background::Color(dim(theme::color(
                appearance,
                theme::MAGENTA,
            )))),
            Fill::Accent(lift) => Some(theme::accent_fill(appearance, lift)),
        };
        button::Style {
            background,
            text_color: dim(theme::color(appearance, face.text)),
            border: Border {
                color: face.edge.map_or(Color::TRANSPARENT, |(edge, alpha)| {
                    dim(theme::rgba(appearance, edge, alpha))
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

/// A card resting on the page: lit from above, edged, softly shadowed.
pub fn card(appearance: &Theme) -> container::Style {
    theme::surface(
        appearance,
        theme::PANEL,
        Some(theme::SEPARATOR),
        theme::RADIUS_LG,
        theme::Elevation::Raised,
    )
}

/// A recess in a card: a metric, a field's worth of detail. Darker than the
/// card around it, flat, edged.
pub fn well(appearance: &Theme) -> container::Style {
    theme::panel(
        appearance,
        theme::PANEL_FILE,
        Some(theme::SEPARATOR),
        theme::RADIUS_MD,
    )
}

/// A dialog over a scrim: the floating surface, with the softer large radius.
pub fn dialog(appearance: &Theme) -> container::Style {
    theme::surface(
        appearance,
        theme::PANEL,
        Some(theme::BORDER),
        theme::RADIUS_LG,
        theme::Elevation::Floating,
    )
}

/// A surface over the page: a tooltip, a menu.
pub fn floating(appearance: &Theme) -> container::Style {
    theme::surface(
        appearance,
        theme::PANEL,
        Some(theme::BORDER),
        theme::RADIUS_MD,
        theme::Elevation::Floating,
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
    // On is the accent; off is a recessed track.
    style.background = if selected {
        color(theme::MAGENTA).into()
    } else {
        color(theme::PANEL_FILE).into()
    };
    style.foreground = color(if selected {
        theme::ON_ACCENT
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

    const ALL: [Button; 10] = [
        Button::Primary,
        Button::Secondary,
        Button::Danger,
        Button::Ghost,
        Button::Navigation(true),
        Button::Navigation(false),
        Button::Choice(true),
        Button::Choice(false),
        Button::Row(true),
        Button::Row(false),
    ];

    fn styled(kind: Button, status: button::Status) -> button::Style {
        button_style(kind)(&Theme::Dark, status)
    }

    fn dark(tone: theme::Tone) -> Color {
        theme::color(&Theme::Dark, tone)
    }

    #[test]
    fn only_the_primary_button_is_filled_with_the_accent() {
        for kind in ALL {
            for status in [
                button::Status::Active,
                button::Status::Hovered,
                button::Status::Pressed,
            ] {
                let style = styled(kind, status);
                let accent_fill = matches!(style.background, Some(Background::Gradient(_)));
                assert_eq!(
                    accent_fill,
                    kind == Button::Primary,
                    "{kind:?} {status:?}: {:?}",
                    style.background
                );
                // A tint is faint: nothing but the primary reaches half opacity.
                if let Some(Background::Color(fill)) = style.background
                    && fill.r > fill.g
                    && fill.b > fill.g
                {
                    assert!(fill.a < 0.5, "{kind:?} fills with a loud accent");
                }
            }
        }
    }

    #[test]
    fn controls_cast_no_shadow_of_their_own() {
        for kind in ALL {
            let style = styled(kind, button::Status::Active);
            assert_eq!(style.shadow.color, Color::TRANSPARENT, "{kind:?}");
            assert_eq!(style.shadow.blur_radius, 0.0, "{kind:?}");
        }
    }

    #[test]
    fn the_primary_button_is_accent_filled_with_white_ink_and_lifts_on_hover() {
        let rest = styled(Button::Primary, button::Status::Active);
        let hover = styled(Button::Primary, button::Status::Hovered);
        let pressed = styled(Button::Primary, button::Status::Pressed);
        assert_eq!(rest.text_color, dark(theme::ON_ACCENT));
        assert_eq!(rest.border.width, 0.0);
        assert!(matches!(rest.background, Some(Background::Gradient(_))));
        // Hover lifts, press sinks: the three fills are three different ones.
        assert_ne!(rest.background, hover.background);
        assert_ne!(rest.background, pressed.background);
        assert_ne!(hover.background, pressed.background);
    }

    #[test]
    fn white_ink_reads_on_the_accent_in_both_appearances() {
        fn luminance(c: Color) -> f32 {
            let lin = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
        }
        for appearance in [Theme::Dark, Theme::Light] {
            let ink = luminance(theme::color(&appearance, theme::ON_ACCENT));
            let accent = luminance(theme::color(&appearance, theme::MAGENTA));
            let contrast = (ink + 0.05) / (accent + 0.05);
            assert!(contrast >= 4.0, "{appearance:?}: {contrast}");
        }
    }

    #[test]
    fn hover_lifts_a_secondary_fill_and_brightens_its_edge() {
        let rest = styled(Button::Secondary, button::Status::Active);
        let hovered = styled(Button::Secondary, button::Status::Hovered);
        assert_eq!(rest.background, Some(Background::Color(dark(theme::PANEL))));
        assert_eq!(
            hovered.background,
            Some(Background::Color(dark(theme::HOVER)))
        );
        assert_eq!(rest.border.color, dark(theme::BORDER));
        assert_ne!(hovered.border.color, rest.border.color);
        assert_eq!(hovered.text_color, dark(theme::TEXT));
    }

    #[test]
    fn selection_is_an_accent_edge_with_a_faint_tint_and_off_white_text() {
        for selected in [
            styled(Button::Choice(true), button::Status::Active),
            styled(Button::Row(true), button::Status::Active),
        ] {
            assert_eq!(selected.border.width, 1.0);
            assert_eq!(selected.text_color, dark(theme::TEXT));
            assert!(matches!(selected.background, Some(Background::Color(c)) if c.a < 0.2));
        }
        let choice = styled(Button::Choice(true), button::Status::Active);
        assert_eq!(choice.border.color, dark(theme::MAGENTA));
    }

    #[test]
    fn a_hovered_row_never_passes_for_the_selected_one() {
        let selected = styled(Button::Row(true), button::Status::Active);
        let hovered = styled(Button::Row(false), button::Status::Hovered);
        assert!(selected.border.width > 0.0 && selected.border.color.a > 0.0);
        assert_eq!(hovered.border.width, 0.0);
        assert_eq!(
            hovered.background,
            Some(Background::Color(dark(theme::HOVER)))
        );
        assert_ne!(hovered.background, selected.background);
        assert_ne!(hovered.text_color, selected.text_color);
    }

    #[test]
    fn under_a_fade_the_fill_stays_at_rest_and_the_label_still_answers() {
        for kind in ALL {
            let rest = button_style_fading(kind)(&Theme::Dark, button::Status::Active);
            let hot = button_style_fading(kind)(&Theme::Dark, button::Status::Hovered);
            assert_eq!(rest.background, hot.background, "{kind:?} fills at once");
        }
        // The wash is what lifts it, so every kind has one, and it is faint.
        for kind in ALL {
            let (_, strength) = kind.wash();
            assert!(
                (0.0..=0.2).contains(&strength) && strength > 0.0,
                "{kind:?}"
            );
        }
        let rest = button_style_fading(Button::Secondary)(&Theme::Dark, button::Status::Active);
        let hot = button_style_fading(Button::Secondary)(&Theme::Dark, button::Status::Hovered);
        assert_ne!(rest.border.color, hot.border.color);
    }

    #[test]
    fn a_count_agrees_with_its_noun() {
        assert_eq!(count(0usize, "file", "files"), "0 files");
        assert_eq!(count(1usize, "file", "files"), "1 file");
        assert_eq!(count(2u64, "byte", "bytes"), "2 bytes");
    }

    #[test]
    fn disabled_dims_to_forty_percent_and_does_not_hover() {
        let disabled = styled(Button::Secondary, button::Status::Disabled);
        assert!((disabled.text_color.a - 0.4).abs() < f32::EPSILON);
        assert!((disabled.border.color.a - 0.4).abs() < 0.001);
        // A disabled control resolves to its resting face, not its hover.
        let rest = styled(Button::Secondary, button::Status::Active);
        assert_eq!(disabled.text_color.r, rest.text_color.r);
        assert!(matches!(
            disabled.background,
            Some(Background::Color(c)) if (c.a - 0.4).abs() < 0.001 && c.r == dark(theme::PANEL).r
        ));
        // A disabled accent is the flat accent at forty percent, not a gradient.
        let primary = styled(Button::Primary, button::Status::Disabled);
        assert!(matches!(
            primary.background,
            Some(Background::Color(c)) if (c.a - 0.4).abs() < 0.001
        ));
    }

    #[test]
    fn a_danger_control_is_a_faint_error_tint_with_error_text() {
        let rest = styled(Button::Danger, button::Status::Active);
        let hover = styled(Button::Danger, button::Status::Hovered);
        assert_eq!(rest.text_color, dark(theme::ERROR));
        assert!(matches!(rest.background, Some(Background::Color(c)) if c.a < 0.2));
        assert!(matches!(
            (rest.background, hover.background),
            (Some(Background::Color(a)), Some(Background::Color(b))) if b.a > a.a
        ));
    }

    #[test]
    fn a_ghost_is_bare_until_hover() {
        let rest = styled(Button::Ghost, button::Status::Active);
        let hover = styled(Button::Ghost, button::Status::Hovered);
        assert_eq!(rest.background, None);
        assert_eq!(
            hover.background,
            Some(Background::Color(dark(theme::HOVER)))
        );
        assert_eq!(rest.text_color, dark(theme::TEXT_MUTED));
        assert_eq!(hover.text_color, dark(theme::TEXT));
    }

    #[test]
    fn a_card_is_raised_lit_and_edged_and_a_tooltip_floats_higher() {
        for appearance in [Theme::Dark, Theme::Light] {
            let card = card(&appearance);
            let floating = floating(&appearance);
            assert_eq!(card.border.width, 1.0);
            assert!(matches!(card.background, Some(Background::Gradient(_))));
            assert!(card.shadow.blur_radius > 0.0 && card.shadow.color.a > 0.0);
            assert!(floating.shadow.blur_radius > card.shadow.blur_radius);
            assert!(floating.shadow.offset.y > card.shadow.offset.y);
        }
    }

    #[test]
    fn a_focused_field_takes_the_accent_border_and_selects_with_a_tint() {
        let resting = field(&Theme::Dark, text_input::Status::Active);
        let focused = field(
            &Theme::Dark,
            text_input::Status::Focused { is_hovered: false },
        );
        assert_eq!(resting.border.color, dark(theme::BORDER));
        assert_eq!(focused.border.color, dark(theme::MAGENTA));
        assert_eq!(
            resting.background,
            Background::Color(dark(theme::BACKGROUND))
        );
        assert!(focused.selection.a < 0.5);
    }
}
