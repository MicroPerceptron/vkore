//! MONOLITH design tokens.
//!
//! The published system is the source of truth for every colour, face, size,
//! radius, and step here. Widgets read these constants instead of carrying
//! literals so a token change lands everywhere at once.
//!
//! Two rules the system treats as non-negotiable, enforced by the helpers
//! below rather than by convention:
//!
//! - Magenta is never a fill. It appears as text, one-pixel borders, and small
//!   marks. Selection is a magenta border with off-white text; the primary
//!   button is a black fill with a magenta border.
//! - Nothing casts a shadow. Every edge in the system is a border.

use iced_core::{Background, Border, Color, Shadow};

/// Page background.
pub const BACKGROUND: u32 = 0x0A0A0C;
/// Panel and card fill.
pub const PANEL: u32 = 0x131316;
/// Topology directory node fill.
pub const PANEL_DIR: u32 = 0x17171B;
/// Topology file node fill.
pub const PANEL_FILE: u32 = 0x0C0C0F;
/// Hovered ghost row fill.
pub const HOVER: u32 = 0x1B1B20;
/// Hairline border.
pub const BORDER: u32 = 0x2A2A30;
/// Dim border, and the colour of disabled or struck-through text.
pub const BORDER_DIM: u32 = 0x3A3A42;
/// Primary off-white text.
pub const TEXT: u32 = 0xF2F0EC;
/// Secondary text.
pub const TEXT_SECONDARY: u32 = 0xB9B9BE;
/// Muted text, used for mono metadata.
pub const TEXT_MUTED: u32 = 0x8B8B90;
/// The single accent.
pub const MAGENTA: u32 = 0xCE15C4;
/// First dark accent step, used for completed wizard steps.
pub const MAGENTA_DARK: u32 = 0xA410A0;
/// Second dark accent step.
pub const MAGENTA_DARKER: u32 = 0x7A0C77;
/// Success.
pub const SUCCESS: u32 = 0x3FCB6E;
/// Warning, and the colour of external paths.
pub const WARNING: u32 = 0xE8A23D;
/// Error.
pub const ERROR: u32 = 0xE8433D;

/// Radius for small controls and log rows.
pub const RADIUS_SM: f32 = 3.0;
/// Radius for buttons and fields.
pub const RADIUS_MD: f32 = 4.0;
/// Radius for panels and cards.
pub const RADIUS_LG: f32 = 6.0;
/// Radius for glass surfaces. Nothing in the system exceeds this.
pub const RADIUS_GLASS: f32 = 8.0;

/// Spacing scale. The unit is 8px; 4 is the one permitted half step.
pub const SPACE_1: f32 = 4.0;
/// Two spacing units.
pub const SPACE_2: f32 = 8.0;
/// Four spacing units.
pub const SPACE_4: f32 = 16.0;
/// Six spacing units.
pub const SPACE_6: f32 = 24.0;
/// Eight spacing units.
pub const SPACE_8: f32 = 32.0;

/// Small control height. The system's controls are 32 / 40 / 52.
pub const CONTROL_SM: f32 = 32.0;
/// Medium control height.
pub const CONTROL_MD: f32 = 40.0;

/// Type scale. Caps are used only above 22px.
pub const SIZE_MONO: f32 = 12.0;
/// Body small, and the topology node name size.
pub const SIZE_SM: f32 = 14.0;
/// Body.
pub const SIZE_MD: f32 = 16.0;
/// Large conversational prose.
pub const SIZE_LG: f32 = 18.0;
/// Display, used for dialog titles and section heads.
pub const SIZE_DISPLAY: f32 = 22.0;

/// Display face. Archivo 600, uppercase.
pub const DISPLAY_FAMILY: &str = "Archivo";
/// Display weight.
pub const DISPLAY_WEIGHT: u16 = 600;
/// Body face. Space Grotesk 400.
pub const BODY_FAMILY: &str = "Space Grotesk";
/// Body weight.
pub const BODY_WEIGHT: u16 = 400;
/// Mono face, used for labels, metadata, navigation, paths, and logs.
pub const MONO_FAMILY: &str = "JetBrains Mono";
/// Mono body weight, used for logs, JSON, and paths.
pub const MONO_WEIGHT: u16 = 400;
/// Mono label weight, used for uppercase labels and navigation.
pub const MONO_LABEL_WEIGHT: u16 = 500;

/// Tracking on uppercase mono labels and metadata.
pub const TRACK_LABEL: f32 = 0.06;
/// Tracking on navigation, which the system sets slightly tighter.
pub const TRACK_NAV: f32 = 0.04;
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
pub fn rgba(hex: u32, alpha: f32) -> Color {
    Color {
        a: alpha,
        ..rgb(hex)
    }
}

/// The scrim behind a modal.
pub fn scrim() -> Color {
    rgba(BACKGROUND, 0.66)
}

/// The glass surface a dialog sits on.
///
/// The system's translucent fill over its Axiom backdrop filter.
pub fn glass() -> Color {
    rgba(BACKGROUND, 0.86)
}

/// The border around a glass surface.
pub fn glass_border() -> Color {
    rgba(TEXT, 0.18)
}

/// The fill behind a selected topology node.
pub fn selected_fill() -> Color {
    rgba(MAGENTA, 0.07)
}

/// The border around a capability chip the agent holds.
pub fn chip_border() -> Color {
    rgba(MAGENTA, 0.42)
}

/// A one-pixel border in `hex`, at `radius`.
pub fn border(hex: u32, radius: f32) -> Border {
    Border {
        color: rgb(hex),
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
pub fn panel(fill: u32, edge: Option<u32>, radius: f32) -> iced_widget::container::Style {
    iced_widget::container::Style {
        background: Some(Background::Color(rgb(fill))),
        border: match edge {
            Some(edge) => border(edge, radius),
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
    fn tokens_expand_to_their_published_channels() {
        assert_eq!(rgb(MAGENTA), Color::from_rgb8(0xCE, 0x15, 0xC4));
        assert_eq!(rgb(TEXT), Color::from_rgb8(0xF2, 0xF0, 0xEC));
        assert_eq!(rgb(BACKGROUND), Color::from_rgb8(0x0A, 0x0A, 0x0C));
    }

    #[test]
    fn every_face_is_a_truetype_file() {
        for bytes in [DISPLAY_BYTES, BODY_BYTES, MONO_BYTES, MONO_LABEL_BYTES] {
            assert_eq!(&bytes[..4], &[0x00, 0x01, 0x00, 0x00]);
        }
    }

    #[test]
    fn panels_carry_a_border_and_never_a_shadow() {
        let styled = panel(PANEL, Some(BORDER), RADIUS_LG);
        assert_eq!(styled.border.width, 1.0);
        assert_eq!(styled.border.color, rgb(BORDER));
        assert_eq!(styled.shadow.color, Color::TRANSPARENT);
        assert_eq!(styled.shadow.blur_radius, 0.0);
    }
}
