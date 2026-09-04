//! MONOLITH face registration.
//!
//! The design uses four faces: Archivo 600 for display, Space Grotesk 400 for
//! body, and JetBrains Mono at 400 and 500 for machine-shaped strings and
//! uppercase labels. They are registered once at startup so shaping can select
//! them and the backend can install their coverage resources.

use crate::theme;
use axui_core::ResourceId;
use iced_core::{Font, font};
use std::sync::atomic::{AtomicU8, Ordering};

const BODY_DESIGN_DEFAULT: u8 = 0;
const BODY_ARCHIVO: u8 = 1;
const BODY_JETBRAINS_MONO: u8 = 2;
static BODY_PREFERENCE: AtomicU8 = AtomicU8::new(BODY_DESIGN_DEFAULT);

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

const fn face(family: &'static str, weight: font::Weight, tracking: f32) -> Font {
    Font {
        family: font::Family::Name(family),
        weight,
        tracking: font::Tracking(tracking),
        ..Font::DEFAULT
    }
}

/// Archivo 600. Dialog titles and section heads, uppercase.
pub const DISPLAY: Font = face(
    theme::DISPLAY_FAMILY,
    font::Weight::Semibold,
    theme::TRACK_WORDMARK,
);
/// Space Grotesk 400. Prose, prompts, answers, field values.
pub const BODY: Font = face(theme::BODY_FAMILY, font::Weight::Normal, 0.0);
/// JetBrains Mono 400. Logs, JSON, paths, agent and tool names.
pub const MONO: Font = face(theme::MONO_FAMILY, font::Weight::Normal, 0.0);
/// JetBrains Mono 500 without tracking. Timestamps use the heavier face but
/// keep the compact spacing of ordinary metadata.
pub const MONO_MEDIUM: Font = face(theme::MONO_FAMILY, font::Weight::Medium, 0.0);
/// JetBrains Mono 500. Uppercase labels, metadata, navigation.
pub const MONO_LABEL: Font = face(theme::MONO_FAMILY, font::Weight::Medium, theme::TRACK_LABEL);

/// JetBrains Mono 500, tracked for navigation.
pub const MONO_NAV: Font = face(theme::MONO_FAMILY, font::Weight::Medium, theme::TRACK_NAV);

/// Archivo 600, tracked for the splash wordmark.
pub const SPLASH: Font = face(
    theme::DISPLAY_FAMILY,
    font::Weight::Semibold,
    theme::TRACK_SPLASH,
);

/// Applies one persisted, registered body-family choice.
pub fn set_body_preference(preference: &str) -> bool {
    let value = match preference {
        "design_default" => BODY_DESIGN_DEFAULT,
        "archivo" => BODY_ARCHIVO,
        "jetbrains_mono" => BODY_JETBRAINS_MONO,
        _ => return false,
    };
    BODY_PREFERENCE.store(value, Ordering::Release);
    true
}

/// The body face selected in application Settings.
pub fn body() -> Font {
    match BODY_PREFERENCE.load(Ordering::Acquire) {
        BODY_ARCHIVO => face(theme::DISPLAY_FAMILY, font::Weight::Semibold, 0.0),
        BODY_JETBRAINS_MONO => face(theme::MONO_FAMILY, font::Weight::Normal, 0.0),
        _ => BODY,
    }
}

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
    fn the_published_tracking_reaches_the_faces_that_carry_it() {
        // Prose is set solid; labels and wordmarks are tracked out.
        assert_eq!(BODY.tracking, font::Tracking::NONE);
        assert_eq!(MONO.tracking, font::Tracking::NONE);
        assert_eq!(MONO_MEDIUM.tracking, font::Tracking::NONE);
        assert_eq!(MONO_LABEL.tracking, font::Tracking(theme::TRACK_LABEL));
        assert_eq!(MONO_NAV.tracking, font::Tracking(theme::TRACK_NAV));
        assert_eq!(DISPLAY.tracking, font::Tracking(theme::TRACK_WORDMARK));
        assert_eq!(SPLASH.tracking, font::Tracking(theme::TRACK_SPLASH));
        // Navigation is set tighter than a label, as the system has it.
        assert!(MONO_NAV.tracking.0 < MONO_LABEL.tracking.0);
    }

    #[test]
    fn a_tracked_face_shares_the_registered_family() {
        // Tracking is a request about setting, not a different typeface.
        assert_eq!(MONO_LABEL.family, MONO_NAV.family);
        assert_eq!(SPLASH.family, DISPLAY.family);
    }

    #[test]
    fn handles_name_the_registered_families() {
        assert_eq!(DISPLAY.family, font::Family::Name(theme::DISPLAY_FAMILY));
        assert_eq!(BODY.family, font::Family::Name(theme::BODY_FAMILY));
        assert_eq!(MONO.family, font::Family::Name(theme::MONO_FAMILY));
        assert_eq!(MONO.weight, font::Weight::Normal);
        assert_eq!(MONO_MEDIUM.weight, font::Weight::Medium);
        assert_eq!(MONO_LABEL.weight, font::Weight::Medium);
    }
}
