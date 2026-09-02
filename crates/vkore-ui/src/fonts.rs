//! MONOLITH face registration.
//!
//! The design uses four faces: Archivo 600 for display, Space Grotesk 400 for
//! body, and JetBrains Mono at 400 and 500 for machine-shaped strings and
//! uppercase labels. They are registered once at startup so shaping can select
//! them and the backend can install their coverage resources.

use crate::theme;
use axui_core::ResourceId;
use iced_core::{Font, font};

/// The registered resource identities, in registration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Faces {
    /// Archivo 600.
    pub display: ResourceId,
    /// Space Grotesk 400.
    pub body: ResourceId,
    /// JetBrains Mono 400.
    pub mono: ResourceId,
    /// JetBrains Mono 500.
    pub mono_label: ResourceId,
}

/// Registers every MONOLITH face and returns their display-list identities.
///
/// Registration is idempotent, so calling this more than once in a process is
/// harmless and returns the same identities.
pub fn register() -> Result<Faces, axui_iced::TextError> {
    Ok(Faces {
        display: axui_iced::register_font(theme::DISPLAY_BYTES)?,
        body: axui_iced::register_font(theme::BODY_BYTES)?,
        mono: axui_iced::register_font(theme::MONO_BYTES)?,
        mono_label: axui_iced::register_font(theme::MONO_LABEL_BYTES)?,
    })
}

const fn face(family: &'static str, weight: font::Weight) -> Font {
    Font {
        family: font::Family::Name(family),
        weight,
        ..Font::DEFAULT
    }
}

/// Archivo 600. Dialog titles and section heads, uppercase.
pub const DISPLAY: Font = face(theme::DISPLAY_FAMILY, font::Weight::Semibold);
/// Space Grotesk 400. Prose, prompts, answers, field values.
pub const BODY: Font = face(theme::BODY_FAMILY, font::Weight::Normal);
/// JetBrains Mono 400. Logs, JSON, paths, agent and tool names.
pub const MONO: Font = face(theme::MONO_FAMILY, font::Weight::Normal);
/// JetBrains Mono 500. Uppercase labels, metadata, navigation.
pub const MONO_LABEL: Font = face(theme::MONO_FAMILY, font::Weight::Medium);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_idempotent_and_gives_each_weight_its_own_identity() {
        let first = register().expect("faces register");
        let second = register().expect("faces register twice");

        assert_eq!(first, second);
        let identities = [first.display, first.body, first.mono, first.mono_label];
        for (index, identity) in identities.iter().enumerate() {
            assert!(
                !identities[..index].contains(identity),
                "faces must not share a resource identity"
            );
            assert!(axui_iced::font_face(*identity).is_some());
        }
    }

    #[test]
    fn handles_name_the_registered_families() {
        assert_eq!(DISPLAY.family, font::Family::Name(theme::DISPLAY_FAMILY));
        assert_eq!(BODY.family, font::Family::Name(theme::BODY_FAMILY));
        assert_eq!(MONO.family, font::Family::Name(theme::MONO_FAMILY));
        assert_eq!(MONO.weight, font::Weight::Normal);
        assert_eq!(MONO_LABEL.weight, font::Weight::Medium);
    }
}
