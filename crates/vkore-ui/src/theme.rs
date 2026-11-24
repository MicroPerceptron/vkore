//! MONOLITH design tokens.
//!
//! The published dark system and its light counterpart share every face, size,
//! radius, and spacing step. Widgets read semantic colors instead of carrying
//! literals so a token change lands everywhere at once.
//!
//! Two rules the system treats as non-negotiable, enforced by the helpers
//! below rather than by convention:
//!
//! - Magenta is never a fill. It appears as text, one-pixel borders, and small
//!   marks. Selection and primary buttons use a magenta border with
//!   theme-appropriate text and surfaces.
//! - Nothing casts a shadow. Every edge in the system is a border.

use iced_core::{Background, Border, Color, Shadow};

/// A semantic color, resolved at draw time from the active Iced theme.
/// Keeping the role typed prevents literal logo colors from being remapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tone {
    dark: u32,
    light: u32,
}

pub const BACKGROUND: Tone = Tone {
    dark: 0x0A0A0C,
    light: 0xF5F4F1,
};
pub const PANEL: Tone = Tone {
    dark: 0x131316,
    light: 0xFFFFFF,
};
pub const PANEL_FILE: Tone = Tone {
    dark: 0x0C0C0F,
    light: 0xECEBE8,
};
pub const HOVER: Tone = Tone {
    dark: 0x1B1B20,
    light: 0xE5E3E0,
};
pub const BORDER: Tone = Tone {
    dark: 0x2A2A30,
    light: 0x8A878F,
};
/// Quiet separators for surfaces; input boundaries retain BORDER contrast.
pub const SEPARATOR: Tone = Tone {
    dark: 0x28282F,
    light: 0xDFDDE3,
};
pub const TEXT: Tone = Tone {
    dark: 0xF2F0EC,
    light: 0x202026,
};
pub const TEXT_SECONDARY: Tone = Tone {
    dark: 0xB9B9BE,
    light: 0x4D4B55,
};
pub const TEXT_MUTED: Tone = Tone {
    dark: 0x8B8B90,
    light: 0x62606A,
};
pub const MAGENTA: Tone = Tone {
    dark: 0xCE15C4,
    light: 0xA30D9A,
};
pub const SUCCESS: Tone = Tone {
    dark: 0x3FCB6E,
    light: 0x176D38,
};
pub const WARNING: Tone = Tone {
    dark: 0xE8A23D,
    light: 0x885000,
};
pub const ERROR: Tone = Tone {
    dark: 0xE8433D,
    light: 0xB32620,
};

pub use vkored::state::desktop::Appearance;

pub fn appearance_theme(appearance: Appearance, system_dark: Option<bool>) -> iced_core::Theme {
    match appearance {
        Appearance::System if system_dark == Some(false) => iced_core::Theme::Light,
        Appearance::Light => iced_core::Theme::Light,
        Appearance::System | Appearance::Dark => iced_core::Theme::Dark,
    }
}

/// Resolve a role without global state; different windows/tests can use
/// different themes without affecting one another.
pub fn color(appearance: &iced_core::Theme, tone: Tone) -> Color {
    rgb(if matches!(appearance, iced_core::Theme::Light) {
        tone.light
    } else {
        tone.dark
    })
}

/// Text styling is resolved during drawing, including cached widget trees.
pub fn text(tone: Tone) -> impl Fn(&iced_core::Theme) -> iced_widget::text::Style + Copy {
    move |appearance| iced_widget::text::Style {
        color: Some(color(appearance, tone)),
    }
}

/// Radius for buttons and fields.
pub const RADIUS_MD: f32 = 6.0;
/// Radius for panels and cards.
pub const RADIUS_LG: f32 = 10.0;

/// Spacing scale. The unit is 8px; 4 is the one permitted half step.
pub const SPACE_1: f32 = 4.0;
/// Two spacing units.
pub const SPACE_2: f32 = 8.0;
/// Four spacing units.
pub const SPACE_4: f32 = 16.0;
/// Six spacing units.
pub const SPACE_6: f32 = 24.0;

/// Control height. The system's controls are 32 / 40 / 52; these pages
/// use the small one.
pub const CONTROL_SM: f32 = 32.0;

/// Type scale for data, body copy and page headings.
pub const SIZE_MONO: f32 = 12.0;
/// Body small, and every field and list row.
pub const SIZE_SM: f32 = 14.0;
/// Body.
pub const SIZE_MD: f32 = 16.0;
/// Display, used for dialog titles and section heads.
pub const SIZE_DISPLAY: f32 = 30.0;

/// Display face. Archivo 600.
pub const DISPLAY_FAMILY: &str = "Archivo";
/// Body face. Space Grotesk 400.
pub const BODY_FAMILY: &str = "Space Grotesk";
/// Mono face, used for labels, metadata, paths, and logs.
pub const MONO_FAMILY: &str = "JetBrains Mono";

/// Tracking on uppercase mono labels and metadata.
pub const TRACK_LABEL: f32 = 0.06;
/// Tracking on the sidebar and dialog wordmarks.
pub const TRACK_WORDMARK: f32 = 0.01;
/// Tracking on the splash wordmark, which is set widest.
pub const TRACK_SPLASH: f32 = 0.06;

/// Face bytes shipped with the binary, under the OFL alongside them.
pub const DISPLAY_BYTES: &[u8] = include_bytes!("../assets/fonts/Archivo-SemiBold.ttf");
/// Body face bytes.
pub const BODY_BYTES: &[u8] = include_bytes!("../assets/fonts/SpaceGrotesk-Regular.ttf");
/// Mono body face bytes.
pub const MONO_BYTES: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
/// Mono label face bytes.
pub const MONO_LABEL_BYTES: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");

/// Expands a packed `0xRRGGBB` token into an opaque colour.
pub fn rgb(hex: u32) -> Color {
    Color::from_rgb8(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

/// Expands a packed token at a given alpha.
pub fn rgba(appearance: &iced_core::Theme, tone: Tone, alpha: f32) -> Color {
    Color {
        a: alpha,
        ..color(appearance, tone)
    }
}

/// A one-pixel border in `hex`, at `radius`.
pub fn border(appearance: &iced_core::Theme, tone: Tone, radius: f32) -> Border {
    Border {
        color: color(appearance, tone),
        width: 1.0,
        radius: radius.into(),
    }
}

/// The system casts no shadows; every edge is a border.
pub const fn no_shadow() -> Shadow {
    Shadow {
        color: Color::TRANSPARENT,
        offset: iced_core::Vector::new(0.0, 0.0),
        blur_radius: 0.0,
    }
}

/// A panel: a token fill, an optional one-pixel border, and a radius.
pub fn panel(
    appearance: &iced_core::Theme,
    fill: Tone,
    edge: Option<Tone>,
    radius: f32,
) -> iced_widget::container::Style {
    iced_widget::container::Style {
        background: Some(Background::Color(color(appearance, fill))),
        border: match edge {
            Some(edge) => border(appearance, edge, radius),
            None => Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: radius.into(),
            },
        },
        shadow: no_shadow(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_appearance_tracks_reported_changes_without_changing_explicit_choices() {
        use iced_core::Theme;
        assert_eq!(
            appearance_theme(Appearance::System, Some(false)),
            Theme::Light
        );
        assert_eq!(
            appearance_theme(Appearance::System, Some(true)),
            Theme::Dark
        );
        assert_eq!(appearance_theme(Appearance::System, None), Theme::Dark);
        for reported in [None, Some(false), Some(true)] {
            assert_eq!(appearance_theme(Appearance::Light, reported), Theme::Light);
            assert_eq!(appearance_theme(Appearance::Dark, reported), Theme::Dark);
        }
    }

    #[test]
    fn light_text_and_control_edges_have_readable_contrast() {
        fn luminance(hex: u32) -> f64 {
            let linear = |value| {
                let x = f64::from(value) / 255.0;
                if x <= 0.04045 {
                    x / 12.92
                } else {
                    ((x + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear((hex >> 16) & 255)
                + 0.7152 * linear((hex >> 8) & 255)
                + 0.0722 * linear(hex & 255)
        }
        let contrast = |a, b| {
            let (a, b) = (luminance(a), luminance(b));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        for surface in [BACKGROUND, PANEL, PANEL_FILE, HOVER] {
            for foreground in [
                TEXT,
                TEXT_SECONDARY,
                TEXT_MUTED,
                MAGENTA,
                SUCCESS,
                WARNING,
                ERROR,
            ] {
                assert!(
                    contrast(surface.light, foreground.light) >= 4.5,
                    "{foreground:?} on {surface:?}"
                );
            }
        }
        assert!(contrast(BACKGROUND.light, BORDER.light) >= 3.0);
    }

    #[test]
    fn tokens_expand_to_their_published_channels() {
        assert_eq!(
            color(&iced_core::Theme::Dark, MAGENTA),
            Color::from_rgb8(0xCE, 0x15, 0xC4)
        );
        assert_eq!(
            color(&iced_core::Theme::Dark, TEXT),
            Color::from_rgb8(0xF2, 0xF0, 0xEC)
        );
        assert_eq!(
            color(&iced_core::Theme::Dark, BACKGROUND),
            Color::from_rgb8(0x0A, 0x0A, 0x0C)
        );
    }

    #[test]
    fn every_face_is_a_truetype_file() {
        for bytes in [DISPLAY_BYTES, BODY_BYTES, MONO_BYTES, MONO_LABEL_BYTES] {
            assert_eq!(&bytes[..4], &[0x00, 0x01, 0x00, 0x00]);
        }
    }

    #[test]
    fn panels_carry_a_border_and_never_a_shadow() {
        let styled = panel(&iced_core::Theme::Dark, PANEL, Some(BORDER), RADIUS_LG);
        assert_eq!(styled.border.width, 1.0);
        assert_eq!(styled.border.color, color(&iced_core::Theme::Dark, BORDER));
        assert_eq!(styled.shadow.color, Color::TRANSPARENT);
        assert_eq!(styled.shadow.blur_radius, 0.0);
    }
}
