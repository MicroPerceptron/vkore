//! Explicit, per-window typography. Only bundled faces can be selected.
use crate::{fonts, theme};
use iced_core::{Font, Length, Theme, alignment};
use iced_widget::{column, container, row, text};
use std::{
    io::{Read, Write},
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextSize {
    Small,
    #[default]
    Default,
    Large,
    ExtraLarge,
}
impl TextSize {
    pub const ALL: [Self; 4] = [Self::Small, Self::Default, Self::Large, Self::ExtraLarge];
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
            Self::ExtraLarge => "Extra large",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Default => "default",
            Self::Large => "large",
            Self::ExtraLarge => "extra-large",
        }
    }
    fn factor(self) -> f32 {
        match self {
            Self::Small => 0.9,
            Self::Default => 1.,
            Self::Large => 1.15,
            Self::ExtraLarge => 1.3,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontPreset {
    #[default]
    Grotesk,
    Sans,
    Mono,
}
impl FontPreset {
    pub const ALL: [Self; 3] = [Self::Grotesk, Self::Sans, Self::Mono];
    pub fn label(self) -> &'static str {
        match self {
            Self::Grotesk => "Space Grotesk",
            Self::Sans => "DM Sans",
            Self::Mono => "JetBrains Mono",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Grotesk => "grotesk",
            Self::Sans => "sans",
            Self::Mono => "mono",
        }
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
        points * self.text_size.factor()
    }
    pub fn control_height(self) -> f32 {
        self.size(theme::CONTROL_SM)
    }
    pub fn load(root: &Path) -> std::io::Result<Self> {
        let file = match std::fs::File::open(root.join("typography")) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
        };
        let mut value = String::new();
        file.take(65).read_to_string(&mut value)?;
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid typography preference",
            )
        };
        if value.len() > 64 {
            return Err(invalid());
        }
        let keys: Vec<_> = value.split_whitespace().collect();
        if keys.len() != 2 {
            return Err(invalid());
        }
        let text_size = TextSize::ALL
            .into_iter()
            .find(|size| size.key() == keys[0])
            .ok_or_else(invalid)?;
        let preset = FontPreset::ALL
            .into_iter()
            .find(|preset| preset.key() == keys[1])
            .ok_or_else(invalid)?;
        Ok(Self { text_size, preset })
    }
    pub fn save(self, root: &Path) -> std::io::Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        writeln!(file, "{} {}", self.text_size.key(), self.preset.key())?;
        file.as_file().sync_all()?;
        file.persist(root.join("typography")).map_err(|e| e.error)?;
        Ok(())
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

    pub fn page_header<'a>(
        self,
        title: &'static str,
        subtitle: &'static str,
    ) -> crate::UiElement<'a> {
        column![
            self.display(title),
            self.body(subtitle, theme::SIZE_SM, theme::TEXT_MUTED)
        ]
        .spacing(6)
        .into()
    }

    pub fn badge<'a>(
        self,
        label: impl text::IntoFragment<'a>,
        tone: theme::Tone,
    ) -> crate::UiElement<'a> {
        container(
            row![self.body("·", 16., tone), self.body(label, 12., tone)]
                .spacing(5)
                .align_y(alignment::Vertical::Center),
        )
        .padding([3, 9])
        .style(|appearance| theme::panel(appearance, theme::PANEL_FILE, None, 20.))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_round_trip_and_reject_unbounded_or_unknown_values() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            Typography::load(root.path()).unwrap(),
            Typography::default()
        );
        for preset in FontPreset::ALL {
            for text_size in TextSize::ALL {
                let ty = Typography { preset, text_size };
                ty.save(root.path()).unwrap();
                assert_eq!(Typography::load(root.path()).unwrap(), ty);
            }
        }
        for invalid in [
            "large host-font".to_owned(),
            "infinite mono".into(),
            "default sans extra".into(),
            "default mono".to_owned() + &" ".repeat(100),
        ] {
            std::fs::write(root.path().join("typography"), invalid).unwrap();
            assert_eq!(
                Typography::load(root.path()).unwrap_err().kind(),
                std::io::ErrorKind::InvalidData
            );
        }
    }
}
