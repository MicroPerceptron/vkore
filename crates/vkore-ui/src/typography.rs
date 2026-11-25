//! Explicit, per-window typography. Only bundled faces can be selected.
use crate::{fonts, theme};
use iced_core::{Font, Length, Theme, alignment};
use iced_widget::{column, container, row, text};
pub use vkored::state::desktop::{FontPreset, TextSize};

pub fn text_size_label(size: TextSize) -> &'static str {
    match size {
        TextSize::Small => "Small",
        TextSize::Default => "Default",
        TextSize::Large => "Large",
        TextSize::ExtraLarge => "Extra large",
    }
}

pub fn font_preset_label(preset: FontPreset) -> &'static str {
    match preset {
        FontPreset::Grotesk => "Space Grotesk",
        FontPreset::Sans => "DM Sans",
        FontPreset::Mono => "JetBrains Mono",
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Typography {
    pub text_size: TextSize,
    pub preset: FontPreset,
}
impl Typography {
    pub fn font(self) -> Font {
        match self.preset {
            FontPreset::Grotesk => fonts::BODY,
            FontPreset::Sans => fonts::SANS,
            FontPreset::Mono => fonts::MONO,
        }
    }
    pub fn size(self, points: f32) -> f32 {
        points
            * match self.text_size {
                TextSize::Small => 0.9,
                TextSize::Default => 1.,
                TextSize::Large => 1.15,
                TextSize::ExtraLarge => 1.3,
            }
    }
    pub fn control_height(self) -> f32 {
        self.size(theme::CONTROL_SM)
    }
    pub fn label<'a>(
        self,
        content: impl AsRef<str>,
        color: theme::Tone,
    ) -> text::Text<'a, Theme, crate::AxUi> {
        text(content.as_ref().to_uppercase())
            .font(fonts::MONO_LABEL)
            .size(self.size(theme::SIZE_MONO))
            .line_height(iced_core::text::LineHeight::Relative(1.35))
            .style(theme::text(color))
    }

    /// Center text in the control's allocated inner height; the caller scales
    /// the control height too, so larger type retains its padding.
    pub fn control<'a>(
        self,
        content: impl text::IntoFragment<'a>,
    ) -> text::Text<'a, Theme, crate::AxUi> {
        text(content)
            .font(self.font())
            .size(self.size(theme::SIZE_SM))
            .line_height(iced_core::text::LineHeight::Relative(1.35))
            .height(Length::Fill)
            .align_y(alignment::Vertical::Center)
    }

    pub fn nav<'a>(
        self,
        content: impl text::IntoFragment<'a>,
    ) -> text::Text<'a, Theme, crate::AxUi> {
        text(content)
            .font(self.font())
            .size(self.size(theme::SIZE_SM))
            .line_height(iced_core::text::LineHeight::Relative(1.35))
            .height(Length::Fill)
            .align_y(alignment::Vertical::Center)
    }

    pub fn mono<'a>(
        self,
        content: impl text::IntoFragment<'a>,
        color: theme::Tone,
    ) -> text::Text<'a, Theme, crate::AxUi> {
        text(content)
            .font(fonts::MONO)
            .size(self.size(theme::SIZE_MONO))
            .line_height(iced_core::text::LineHeight::Relative(1.35))
            .style(theme::text(color))
    }

    /// Untrusted text stays in the fixed, registered mono face. Escape controls,
    /// invisible characters and missing glyphs before shaping; presets cannot
    /// change this boundary or introduce host-font fallback.
    pub fn data<'a>(
        self,
        content: impl Into<std::borrow::Cow<'a, str>>,
        color: theme::Tone,
    ) -> text::Text<'a, Theme, crate::AxUi> {
        self.mono(
            fonts::shown(fonts::MONO, content, fonts::Breaks::Escape),
            color,
        )
    }

    pub fn body<'a>(
        self,
        content: impl text::IntoFragment<'a>,
        size: f32,
        color: theme::Tone,
    ) -> text::Text<'a, Theme, crate::AxUi> {
        text(content)
            .font(self.font())
            .size(self.size(size))
            .line_height(iced_core::text::LineHeight::Relative(1.35))
            .style(theme::text(color))
    }

    pub fn display<'a>(self, content: impl AsRef<str>) -> text::Text<'a, Theme, crate::AxUi> {
        text(content.as_ref().to_owned())
            .font(fonts::DISPLAY)
            .size(self.size(theme::SIZE_DISPLAY))
            .line_height(iced_core::text::LineHeight::Relative(1.25))
            .style(theme::text(theme::TEXT))
    }

    /// A page's title, led by the glyph that is also its place in the
    /// navigation, so the page and its entry read as one thing.
    pub fn page_header<'a>(
        self,
        icon: crate::visuals::Icon,
        title: &'static str,
        subtitle: &'static str,
    ) -> crate::UiElement<'a> {
        let plate = self.size(48.);
        row![
            container(crate::visuals::icon(icon, theme::MAGENTA, plate * 0.5))
                .center(plate)
                .style(|appearance| theme::panel(
                    appearance,
                    theme::PANEL,
                    Some(theme::SEPARATOR),
                    theme::RADIUS_LG
                )),
            column![
                self.display(title),
                self.body(subtitle, theme::SIZE_SM, theme::TEXT_MUTED)
            ]
            .spacing(6),
        ]
        .spacing(theme::SPACE_4)
        .align_y(alignment::Vertical::Center)
        .into()
    }

    pub fn badge<'a>(
        self,
        label: impl text::IntoFragment<'a>,
        tone: theme::Tone,
    ) -> crate::UiElement<'a> {
        // Size to the content. Decoration and spacing follow the resolved
        // label size, so changing fonts or text scale needs no pixel offsets.
        let em = self.size(12.);
        container(
            row![self.body("·", 12., tone), self.body(label, 12., tone)]
                .spacing(em / 3.)
                .align_y(alignment::Vertical::Center),
        )
        .padding([em / 4., em * 0.75])
        .style(move |appearance| theme::panel(appearance, theme::PANEL_FILE, None, em * 2.))
        .into()
    }

    pub fn empty_state<'a>(
        self,
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
                self.body(title, 20., theme::TEXT),
                self.body(description, theme::SIZE_SM, theme::TEXT_MUTED)
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
}
