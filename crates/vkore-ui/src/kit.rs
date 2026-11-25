//! Components built from marks, not words.
//!
//! A screen says what a thing is with a shape and says its name only where a
//! shape cannot: a state is a beacon and a row of segments, a command is a
//! glyph with a tooltip, a quantity is a ring or a tile. The words that remain
//! are the ones a person reads (a machine's name, a path); the words that were
//! doing a symbol's job ("Stop", "stopped", "-", "=", "+") are gone.
//!
//! These are the composites a screen reaches for. They take a `Typography`
//! because a larger text size grows the marks with it, and they take the
//! `widgets::Button` kind so a glyph button is still one of the controls the
//! design defines, not a new one.

use crate::{
    Message, UiElement,
    motion::{self, Beat, Steps},
    theme,
    typography::Typography,
    visuals::{self, Icon},
    widgets,
};
use iced_core::{Length, alignment};
use iced_widget::{button, column, container, row, stack, tooltip};

/// A state of a thing: how it moves, what colour it is, and what it is called.
///
/// The name is not decoration. It is the tooltip on a compact mark and the
/// label beside a large one, so a state is never colour alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub beat: Beat,
    pub tone: theme::Tone,
    pub label: &'static str,
    /// Progress through a sequence, when the state is one stop along it.
    pub steps: Option<Steps>,
}

/// A tooltip in the system's one style: a bounded card.
pub fn tip<'a>(
    content: impl Into<UiElement<'a>>,
    label: impl Into<UiElement<'a>>,
    position: tooltip::Position,
) -> UiElement<'a> {
    tooltip(
        content,
        // Iced hands tooltip contents an unbounded viewport. Text clips stay
        // finite, as Axiom's display-list contract requires.
        container(label)
            .padding(theme::SPACE_2)
            .style(widgets::floating)
            .clip(true),
        position,
    )
    .gap(theme::SPACE_2 + 2.0)
    .into()
}

/// A tooltip that is one line of text.
pub fn tip_text<'a>(
    ty: Typography,
    content: impl Into<UiElement<'a>>,
    label: &'static str,
    position: tooltip::Position,
) -> UiElement<'a> {
    tip(content, ty.body(label, 12., theme::TEXT), position)
}

/// A state as a pill: its beacon, then its name.
pub fn status<'a>(ty: Typography, status: Status) -> UiElement<'a> {
    status_as(ty, status, status.label)
}

/// A state as a pill, worded as `label` (a count, say) instead of its own name.
pub fn status_as<'a>(ty: Typography, status: Status, label: impl Into<String>) -> UiElement<'a> {
    let em = ty.size(12.);
    container(
        row![
            motion::beacon(status.beat, status.tone, ty.size(14.)),
            ty.body(label.into(), 12., status.tone),
        ]
        .spacing(em / 2.)
        .align_y(alignment::Vertical::Center),
    )
    .padding([em / 3., em * 0.8])
    .style(move |appearance| theme::tinted(appearance, status.tone, 0.13, 0.28, em * 2.))
    .into()
}

/// A state with no words: its beacon and, if it is a step along a sequence,
/// the sequence. The name is the tooltip. For a list, where a row of words per
/// line is noise and a column of marks is a scan.
pub fn status_mark<'a>(ty: Typography, status: Status) -> UiElement<'a> {
    let mut mark = row![motion::beacon(status.beat, status.tone, ty.size(14.))]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Center);
    if let Some(steps) = status.steps {
        mark = mark.push(motion::steps(steps, ty.size(56.), ty.size(5.)));
    }
    tip_text(ty, mark, status.label, tooltip::Position::Left)
}

/// The tone a glyph takes inside a control of this kind.
///
/// A canvas cannot follow its button's hover, so the glyph takes the colour
/// the control's label rests in: white on the accent fill, the error colour on
/// a danger. A disabled control's glyph drops to the muted tone.
fn glyph_tone(kind: widgets::Button, enabled: bool) -> theme::Tone {
    if !enabled {
        return theme::TEXT_MUTED;
    }
    match kind {
        widgets::Button::Primary => theme::ON_ACCENT,
        widgets::Button::Danger => theme::ERROR,
        widgets::Button::Ghost => theme::TEXT_MUTED,
        widgets::Button::Secondary
        | widgets::Button::Navigation(_)
        | widgets::Button::Choice(_)
        | widgets::Button::Row(_) => theme::TEXT_SECONDARY,
    }
}

/// A control that eases up under the pointer instead of snapping: wraps a
/// button styled with [`widgets::button_style_fading`] in its kind's wash.
pub fn lift<'a>(
    kind: widgets::Button,
    enabled: bool,
    content: impl Into<UiElement<'a>>,
) -> UiElement<'a> {
    let (tone, strength) = kind.wash();
    motion::fade(content, tone, strength, theme::RADIUS_MD, enabled)
}

/// A square control that is only a glyph, named by its tooltip.
pub fn icon_button<'a>(
    ty: Typography,
    icon: Icon,
    label: &'static str,
    kind: widgets::Button,
    message: Option<Message>,
) -> UiElement<'a> {
    let side = ty.control_height();
    let enabled = message.is_some();
    let control = button(
        container(visuals::icon(
            icon,
            glyph_tone(kind, message.is_some()),
            ty.size(16.),
        ))
        .center(Length::Fill),
    )
    .width(side)
    .height(side)
    .padding(0)
    .style(widgets::button_style_fading(kind))
    .on_press_maybe(message);
    tip_text(
        ty,
        lift(kind, enabled, control),
        label,
        tooltip::Position::Bottom,
    )
}

/// A control that is a glyph and a short word: for the commands a person
/// reaches for by name (Start, Create), where the word earns its place.
pub fn icon_label<'a>(
    ty: Typography,
    icon: Icon,
    label: &'static str,
    kind: widgets::Button,
    message: Option<Message>,
) -> UiElement<'a> {
    let enabled = message.is_some();
    lift(
        kind,
        enabled,
        button(
            row![
                visuals::icon(icon, glyph_tone(kind, enabled), ty.size(16.)),
                ty.control(label),
            ]
            .spacing(theme::SPACE_2)
            .align_y(alignment::Vertical::Center),
        )
        .height(ty.control_height())
        .padding([0, 12])
        .style(widgets::button_style_fading(kind))
        .on_press_maybe(message),
    )
}

/// A heading that leads with its glyph: a section of a page, a tile.
pub fn heading<'a>(ty: Typography, icon: Icon, title: &'static str) -> UiElement<'a> {
    row![
        visuals::icon(icon, theme::TEXT_MUTED, ty.size(14.)),
        ty.label(title, theme::TEXT_MUTED),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Center)
    .into()
}

/// A quantity that is small enough to sit in a line: a glyph and a figure.
pub fn figure<'a>(ty: Typography, icon: Icon, value: impl Into<String>) -> UiElement<'a> {
    row![
        visuals::icon(icon, theme::TEXT_MUTED, ty.size(13.)),
        ty.body(value.into(), 12., theme::TEXT_SECONDARY),
    ]
    .spacing(theme::SPACE_1 + 1.0)
    .align_y(alignment::Vertical::Center)
    .into()
}

/// A share of something, as a ring with its glyph at the centre and its
/// figure and name beneath.
pub fn gauge<'a>(
    ty: Typography,
    icon: Icon,
    fraction: f32,
    tone: theme::Tone,
    value: impl Into<String>,
    label: &'static str,
    of: impl Into<String>,
) -> UiElement<'a> {
    let size = ty.size(64.);
    column![
        stack![
            motion::ring(fraction, tone, size),
            container(visuals::icon(icon, theme::TEXT_SECONDARY, size * 0.34)).center(Length::Fill),
        ]
        .width(size)
        .height(size),
        ty.body(value.into(), 14., theme::TEXT),
        ty.label(label, theme::TEXT_MUTED),
        ty.body(of.into(), 11., theme::TEXT_MUTED),
    ]
    .spacing(theme::SPACE_1)
    .align_x(alignment::Horizontal::Center)
    .into()
}

/// A quantity with no share to show, in the same footprint as a [`gauge`]: a
/// plate with its glyph, then the figure and its name, so the two sit side by
/// side as equals.
pub fn tile_stat<'a>(
    ty: Typography,
    icon: Icon,
    value: impl Into<String>,
    label: &'static str,
    of: impl Into<String>,
) -> UiElement<'a> {
    let size = ty.size(64.);
    column![
        container(visuals::icon(icon, theme::TEXT_SECONDARY, size * 0.42))
            .center(size)
            .style(move |appearance| theme::surface(
                appearance,
                theme::PANEL,
                Some(theme::SEPARATOR),
                size / 2.,
                theme::Elevation::Raised,
            )),
        ty.body(value.into(), 14., theme::TEXT),
        ty.label(label, theme::TEXT_MUTED),
        ty.body(of.into(), 11., theme::TEXT_MUTED),
    ]
    .spacing(theme::SPACE_1)
    .align_x(alignment::Horizontal::Center)
    .into()
}

/// The lead of a form or dialog: an accent plate with its glyph, a title, and
/// an optional line under it. Smaller than a page header. The title is an
/// element so that a name from outside can be set through the data face.
pub fn lead<'a>(
    ty: Typography,
    icon: Icon,
    title: impl Into<UiElement<'a>>,
    subtitle: Option<&'static str>,
) -> UiElement<'a> {
    let plate = ty.size(40.);
    let mut words = column![title.into()].spacing(theme::SPACE_1);
    if let Some(subtitle) = subtitle {
        words = words.push(ty.body(subtitle, 13., theme::TEXT_MUTED));
    }
    row![
        container(visuals::icon(icon, theme::MAGENTA, plate * 0.5))
            .center(plate)
            .style(|appearance| {
                theme::tinted(appearance, theme::MAGENTA, 0.13, 0.30, theme::RADIUS_MD)
            }),
        words.width(Length::Fill),
    ]
    .spacing(theme::SPACE_4 - 2.)
    .align_y(alignment::Vertical::Center)
    .into()
}

/// A block that calls something out: a glyph, then its text, on a faint wash
/// of the tone it speaks in.
pub fn callout<'a>(
    ty: Typography,
    icon: Icon,
    tone: theme::Tone,
    content: impl Into<UiElement<'a>>,
) -> UiElement<'a> {
    container(
        row![
            container(visuals::icon(icon, tone, ty.size(14.))).padding([2, 0]),
            container(content).width(Length::Fill),
        ]
        .spacing(theme::SPACE_2)
        .align_y(alignment::Vertical::Top),
    )
    .padding([theme::SPACE_2, theme::SPACE_2 + 2.])
    .width(Length::Fill)
    .style(move |appearance| theme::tinted(appearance, tone, 0.09, 0.22, theme::RADIUS_MD))
    .into()
}

/// An explanation: small, muted, led by an info glyph.
pub fn note(ty: Typography, text: impl Into<String>) -> UiElement<'static> {
    row![
        container(visuals::icon(Icon::Info, theme::TEXT_MUTED, ty.size(13.))).padding([2, 0]),
        ty.body(text.into(), 12., theme::TEXT_MUTED)
            .width(Length::Fill),
    ]
    .spacing(theme::SPACE_2)
    .align_y(alignment::Vertical::Top)
    .into()
}

/// Something to be careful about.
pub fn caution(ty: Typography, text: impl Into<String>) -> UiElement<'static> {
    callout(
        ty,
        Icon::Alert,
        theme::WARNING,
        ty.body(text.into(), 12., theme::WARNING),
    )
}

/// Something that has to be fixed.
pub fn problem(ty: Typography, text: impl Into<String>) -> UiElement<'static> {
    callout(
        ty,
        Icon::Alert,
        theme::ERROR,
        ty.body(text.into(), 12., theme::ERROR),
    )
}

/// A caution whose words come from outside (the host, a driver): they go
/// through the data face.
pub fn caution_data<'a>(
    ty: Typography,
    text: impl Into<std::borrow::Cow<'a, str>>,
) -> UiElement<'a> {
    callout(
        ty,
        Icon::Alert,
        theme::WARNING,
        ty.data(text, theme::WARNING)
            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
    )
}

/// An error whose words come from outside: through the data face.
pub fn error_data<'a>(ty: Typography, text: impl Into<std::borrow::Cow<'a, str>>) -> UiElement<'a> {
    callout(
        ty,
        Icon::Alert,
        theme::ERROR,
        ty.data(text, theme::ERROR)
            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
    )
}

/// A fact about a thing, as a small tinted pill: its glyph and a short word.
pub fn chip<'a>(
    ty: Typography,
    icon: Icon,
    label: impl Into<String>,
    tone: theme::Tone,
) -> UiElement<'a> {
    let em = ty.size(12.);
    container(
        row![
            visuals::icon(icon, tone, ty.size(13.)),
            ty.body(label.into(), 12., tone),
        ]
        .spacing(em / 2.)
        .align_y(alignment::Vertical::Center),
    )
    .padding([em / 3., em * 0.75])
    .style(move |appearance| theme::tinted(appearance, tone, 0.12, 0.26, em * 2.))
    .into()
}

/// A name and its value on one line: the name muted and fixed-width so a
/// column of them lines up, the value free to wrap. Both are outside text and
/// go through the data face.
pub fn pair<'a>(
    ty: Typography,
    key: impl Into<std::borrow::Cow<'a, str>>,
    value: impl Into<std::borrow::Cow<'a, str>>,
) -> UiElement<'a> {
    row![
        ty.data(key, theme::TEXT_MUTED)
            .width(ty.size(104.))
            .wrapping(iced_core::text::Wrapping::None)
            .ellipsis(iced_core::text::Ellipsis::End),
        ty.data(value, theme::TEXT)
            .width(Length::Fill)
            .wrapping(iced_core::text::Wrapping::WordOrGlyph),
    ]
    .spacing(theme::SPACE_2)
    .into()
}

/// A capability that is on or off: its glyph lit or dimmed, with its name.
/// The lit glyph is a tick, so state never rests on colour alone.
pub fn feature<'a>(ty: Typography, icon: Icon, label: &'static str, on: bool) -> UiElement<'a> {
    let tone = if on { theme::TEXT } else { theme::TEXT_MUTED };
    let em = ty.size(12.);
    container(
        row![
            visuals::icon(icon, tone, ty.size(14.)),
            ty.body(label, 12., tone),
            visuals::icon(
                if on { Icon::Check } else { Icon::Minus },
                if on {
                    theme::SUCCESS
                } else {
                    theme::TEXT_MUTED
                },
                ty.size(12.),
            ),
        ]
        .spacing(em / 2.)
        .align_y(alignment::Vertical::Center),
    )
    .padding([em / 3., em * 0.75])
    .style(move |appearance| {
        if on {
            theme::tinted(appearance, theme::SUCCESS, 0.10, 0.30, em * 2.)
        } else {
            theme::panel(
                appearance,
                theme::PANEL_FILE,
                Some(theme::SEPARATOR),
                em * 2.,
            )
        }
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glyph_takes_the_tone_its_control_rests_in() {
        assert_eq!(glyph_tone(widgets::Button::Primary, true), theme::ON_ACCENT);
        assert_eq!(glyph_tone(widgets::Button::Danger, true), theme::ERROR);
        assert_eq!(
            glyph_tone(widgets::Button::Secondary, true),
            theme::TEXT_SECONDARY
        );
    }

    #[test]
    fn a_disabled_controls_glyph_is_muted_whatever_its_kind() {
        for kind in [
            widgets::Button::Primary,
            widgets::Button::Danger,
            widgets::Button::Secondary,
            widgets::Button::Ghost,
        ] {
            assert_eq!(glyph_tone(kind, false), theme::TEXT_MUTED, "{kind:?}");
        }
    }
}
