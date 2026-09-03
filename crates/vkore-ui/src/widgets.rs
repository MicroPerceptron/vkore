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
//! Labels are uppercased here rather than by the caller, and carry the
//! system's tracking through the face they are set in.

use crate::{fonts, theme};
use iced_core::{Background, Border, Color, Theme};
use iced_widget::{button, text, text_editor};

/// A wizard step's position relative to the one being edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The step being edited.
    Current,
    /// A step already passed.
    Complete,
    /// A step not yet reached.
    Upcoming,
}

/// The controls the design defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// Black fill, magenta border. Send, Next, Save project, Allow once.
    Primary,
    /// Hairline border, secondary text. Browse, Back, New project, preflight.
    Secondary,
    /// Hairline border, error text. Cancel a running prompt, Deny.
    Danger,
    /// Bare until hover. Wizard cancel, sidebar footer navigation.
    Ghost,
    /// A selectable option: view tabs, workspace access, backends, add-ons.
    Choice(bool),
    /// A project row. Selected carries a magenta border; unselected has none.
    Row(bool),
    /// A wizard step marker.
    Step(Step),
}

/// Resolved colours for one control state, before the disabled dimming.
struct Face {
    fill: Option<u32>,
    text: u32,
    edge: Option<u32>,
}

impl Button {
    fn face(self, hovered: bool) -> Face {
        match self {
            // Magenta is never a fill: the primary button is the page black
            // with a magenta border, and lifts to off-white text on hover.
            Self::Primary => Face {
                fill: Some(theme::BACKGROUND),
                text: if hovered { theme::TEXT } else { theme::MAGENTA },
                edge: Some(theme::MAGENTA),
            },
            Self::Secondary => Face {
                fill: Some(theme::PANEL),
                text: if hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_SECONDARY
                },
                edge: Some(if hovered {
                    theme::MAGENTA
                } else {
                    theme::BORDER
                }),
            },
            Self::Danger => Face {
                fill: Some(theme::PANEL),
                text: theme::ERROR,
                edge: Some(if hovered { theme::ERROR } else { theme::BORDER }),
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
                fill: Some(theme::PANEL),
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
            Self::Row(selected) => Face {
                fill: hovered.then_some(theme::HOVER),
                text: if selected {
                    theme::TEXT
                } else {
                    theme::TEXT_SECONDARY
                },
                edge: (selected || hovered).then_some(theme::MAGENTA),
            },
            Self::Step(Step::Current) => Face {
                fill: None,
                text: theme::TEXT,
                edge: Some(theme::MAGENTA),
            },
            Self::Step(Step::Complete) => Face {
                fill: None,
                text: theme::TEXT_SECONDARY,
                edge: Some(theme::MAGENTA_DARKER),
            },
            Self::Step(Step::Upcoming) => Face {
                fill: None,
                text: if hovered {
                    theme::TEXT
                } else {
                    theme::TEXT_MUTED
                },
                edge: Some(if hovered {
                    theme::MAGENTA
                } else {
                    theme::BORDER
                }),
            },
        }
    }

    /// The radius this control carries.
    const fn radius(self) -> f32 {
        match self {
            Self::Step(_) => theme::RADIUS_SM,
            _ => theme::RADIUS_MD,
        }
    }
}

/// Styles a control, applying hover and the disabled dimming.
pub fn button_style(kind: Button) -> impl Fn(&Theme, button::Status) -> button::Style + Copy {
    move |_theme, status| {
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
                .map(|fill| Background::Color(dim(theme::rgb(fill)))),
            text_color: dim(theme::rgb(face.text)),
            border: Border {
                color: face
                    .edge
                    .map_or(Color::TRANSPARENT, |edge| dim(theme::rgb(edge))),
                width: face.edge.map_or(0.0, |_| 1.0),
                radius: kind.radius().into(),
            },
            shadow: theme::no_shadow(),
            ..Default::default()
        }
    }
}

/// The prompt composer and every wizard field share one editor treatment.
pub fn editor(_theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let focused = matches!(status, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: Background::Color(theme::rgb(theme::PANEL)),
        border: theme::border(
            if focused {
                theme::MAGENTA
            } else {
                theme::BORDER
            },
            theme::RADIUS_MD,
        ),
        placeholder: theme::rgb(theme::TEXT_MUTED),
        value: theme::rgb(theme::TEXT),
        selection: theme::rgba(theme::MAGENTA, 0.28),
    }
}

/// Read-only conversation text, which carries no chrome of its own.
pub fn selectable(color: u32) -> impl Fn(&Theme, text_editor::Status) -> text_editor::Style + Copy {
    move |_theme, _status| text_editor::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        placeholder: theme::rgb(color),
        value: theme::rgb(color),
        selection: theme::rgba(theme::MAGENTA, 0.28),
    }
}

/// An uppercase mono label: metadata, navigation, and section markers.
pub fn label<'a>(content: impl AsRef<str>, color: u32) -> text::Text<'a, Theme, crate::AxUi> {
    text(content.as_ref().to_uppercase())
        .font(fonts::MONO_LABEL)
        .size(theme::SIZE_MONO)
        .color(theme::rgb(color))
}

/// A navigation item, which the system sets slightly tighter than a label.
pub fn nav<'a>(content: impl AsRef<str>, color: u32) -> text::Text<'a, Theme, crate::AxUi> {
    text(content.as_ref().to_uppercase())
        .font(fonts::MONO_NAV)
        .size(theme::SIZE_MONO)
        .color(theme::rgb(color))
}

/// Machine-shaped text: paths, agent and tool names, JSON, log bodies.
pub fn mono<'a>(content: impl Into<String>, color: u32) -> text::Text<'a, Theme, crate::AxUi> {
    text(content.into())
        .font(fonts::MONO)
        .size(theme::SIZE_MONO)
        .color(theme::rgb(color))
}

/// Prose: prompts, answers, field values, project names.
pub fn body<'a>(
    content: impl Into<String>,
    size: f32,
    color: u32,
) -> text::Text<'a, Theme, crate::AxUi> {
    text(content.into())
        .font(fonts::BODY)
        .size(size)
        .color(theme::rgb(color))
}

/// A display heading. The system sets these uppercase.
pub fn display<'a>(content: impl AsRef<str>) -> text::Text<'a, Theme, crate::AxUi> {
    text(content.as_ref().to_uppercase())
        .font(fonts::DISPLAY)
        .size(theme::SIZE_DISPLAY)
        .color(theme::rgb(theme::TEXT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(kind: Button, status: button::Status) -> button::Style {
        button_style(kind)(&Theme::Dark, status)
    }

    #[test]
    fn magenta_is_never_a_fill() {
        let magenta = theme::rgb(theme::MAGENTA);
        for kind in [
            Button::Primary,
            Button::Secondary,
            Button::Danger,
            Button::Ghost,
            Button::Choice(true),
            Button::Choice(false),
            Button::Row(true),
            Button::Row(false),
            Button::Step(Step::Current),
            Button::Step(Step::Complete),
            Button::Step(Step::Upcoming),
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
            Some(Background::Color(theme::rgb(theme::BACKGROUND)))
        );
        assert_eq!(style.border.color, theme::rgb(theme::MAGENTA));
        assert_eq!(style.border.width, 1.0);
    }

    #[test]
    fn hover_moves_a_border_to_magenta_rather_than_filling_it() {
        let rest = styled(Button::Secondary, button::Status::Active);
        let hovered = styled(Button::Secondary, button::Status::Hovered);

        assert_eq!(rest.border.color, theme::rgb(theme::BORDER));
        assert_eq!(hovered.border.color, theme::rgb(theme::MAGENTA));
        assert_eq!(rest.background, hovered.background);
        assert_eq!(hovered.text_color, theme::rgb(theme::TEXT));
    }

    #[test]
    fn selection_is_a_magenta_border_with_off_white_text() {
        let selected = styled(Button::Choice(true), button::Status::Active);
        assert_eq!(selected.border.color, theme::rgb(theme::MAGENTA));
        assert_eq!(selected.text_color, theme::rgb(theme::TEXT));
    }

    #[test]
    fn disabled_dims_to_forty_percent_and_does_not_hover() {
        let disabled = styled(Button::Primary, button::Status::Disabled);
        assert!((disabled.text_color.a - 0.4).abs() < f32::EPSILON);
        assert!((disabled.border.color.a - 0.4).abs() < f32::EPSILON);
        // A disabled control must resolve to its resting face, not its hover.
        assert_eq!(disabled.text_color.r, theme::rgb(theme::MAGENTA).r);
    }

    #[test]
    fn a_focused_editor_takes_the_accent_border() {
        let resting = editor(&Theme::Dark, text_editor::Status::Active);
        let focused = editor(
            &Theme::Dark,
            text_editor::Status::Focused { is_hovered: false },
        );
        assert_eq!(resting.border.color, theme::rgb(theme::BORDER));
        assert_eq!(focused.border.color, theme::rgb(theme::MAGENTA));
    }
}
