//! MONOLITH face registration.
//!
//! The design uses four faces: Archivo 600 for display, Space Grotesk 400 for
//! body, and JetBrains Mono at 400 and 500 for machine-shaped strings and
//! uppercase labels. They are registered once at startup so shaping can select
//! them and the backend can install their coverage resources, and they are the
//! only faces shaping may select: see [`register`].

use crate::theme;
use axui_core::ResourceId;
use iced_core::{Font, font};
use iced_graphics::text::cosmic_text::fontdb;
use std::{borrow::Cow, sync::OnceLock};

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

/// Registers every MONOLITH face, confines shaping to the faces Axiom can
/// draw, and returns their display-list identities.
///
/// Registration is idempotent, so calling this more than once in a process is
/// harmless and returns the same identities. Call it before the first renderer
/// is built: a renderer, or a paragraph, made earlier may already hold a host
/// face that confinement removes.
pub fn register() -> Result<Faces, axui_iced::TextError> {
    let faces = Faces {
        display: axui_iced::register_font(theme::DISPLAY_BYTES)?,
        body: axui_iced::register_font(theme::BODY_BYTES)?,
        mono: axui_iced::register_font(theme::MONO_BYTES)?,
        mono_label: axui_iced::register_font(theme::MONO_LABEL_BYTES)?,
    };
    confine();
    Ok(faces)
}

/// The bytes of every face a frame may name: Axiom's default and ours.
const DRAWABLE: [&[u8]; 5] = [
    axui_fonts::DM_SANS,
    theme::DISPLAY_BYTES,
    theme::BODY_BYTES,
    theme::MONO_BYTES,
    theme::MONO_LABEL_BYTES,
];

/// Removes every face Axiom cannot draw from the shared font system.
///
/// The font system starts out holding every face installed on the host, and
/// shaping selects by family name, preferring the face loaded first. A host
/// with its own JetBrains Mono -- one package away on Ubuntu -- answers
/// "JetBrains Mono" with that copy, which was never registered, and Axiom
/// refuses the whole frame. Glyph fallback is no safer: a character our faces
/// lack, in a file name a guest wrote or a log it kept, falls back to whichever
/// host face covers it, and one name blanks the window. With only drawable
/// faces left, a family resolves to our bytes, fallback moves between our faces
/// (JetBrains Mono covers what Space Grotesk lacks), and a character none of
/// them covers shapes as its face's missing-glyph box. The window then draws
/// the same on every host.
fn confine() {
    let mut system = iced_graphics::text::font_system()
        .write()
        .expect("write font system");
    let database = system.raw().db_mut();
    let foreign: Vec<fontdb::ID> = database
        .faces()
        .filter(|face| !drawable(&face.source))
        .map(|face| face.id)
        .collect();
    for id in foreign {
        database.remove_face(id);
    }
}

/// Whether `source` holds one of the [`DRAWABLE`] faces. Bytes are the only
/// exact key: a host file may carry the same family, and even the same name.
fn drawable(source: &fontdb::Source) -> bool {
    match source {
        fontdb::Source::Binary(loaded) => {
            let loaded: &[u8] = loaded.as_ref().as_ref();
            DRAWABLE.contains(&loaded)
        }
        _ => false,
    }
}

/// What a string set from data keeps of its line structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breaks {
    /// One line: a line break or a tab is escaped like any other control.
    Escape,
    /// A preview: line breaks and tabs stay layout, and a carriage return
    /// ends its line.
    Keep,
}

/// `text`, which came from outside the binary -- a name a guest wrote, a log
/// it kept, a host path, an error quoting one -- as `font` can set it.
///
/// Every character `font` has no glyph for, every control, every
/// default-ignorable character (a direction override, a zero-width joiner),
/// and every space but the ASCII one is replaced by its escape, `\u{...}`; a
/// backslash is doubled so an escape cannot be forged. Escapes are ASCII,
/// which every bundled face covers. So:
///
/// - The paragraph shapes in `font` alone, with no fallback to another face.
///   Axiom's recorder panics when a run of only invisible glyphs is followed
///   by a change of face within a line, and the panic poisons the shared font
///   system for the rest of the process; no data can lead there.
/// - A name shows what it is: a right-to-left override cannot show `gnp.exe`
///   as `exe.png`, and a zero-width or look-alike space cannot make two names
///   look the same.
///
/// Text that needs nothing escaped comes back as it went in, without a copy.
pub fn shown<'a>(font: Font, text: impl Into<Cow<'a, str>>, breaks: Breaks) -> Cow<'a, str> {
    let text = text.into();
    let face = coverage(font);
    if text.chars().all(|character| kept(face, character, breaks)) {
        return text;
    }
    let mut shown = String::with_capacity(text.len() + text.len() / 4);
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => shown.push_str("\\\\"),
            '\r' if breaks == Breaks::Keep => {
                if characters.peek() != Some(&'\n') {
                    shown.push('\n');
                }
            }
            character if kept(face, character, breaks) => shown.push(character),
            character => shown.extend(character.escape_unicode()),
        }
    }
    Cow::Owned(shown)
}

/// Whether `character` is set as itself in `face`.
fn kept(face: &ttf_parser::Face<'_>, character: char, breaks: Breaks) -> bool {
    match character {
        '\\' => false,
        ' ' => true,
        // Every bundled face covers printable ASCII: no lookup.
        character if character.is_ascii_graphic() => true,
        '\n' | '\t' => breaks == Breaks::Keep,
        character
            if character.is_control() || character.is_whitespace() || ignorable(character) =>
        {
            false
        }
        character => face.glyph_index(character).is_some(),
    }
}

/// Unicode's default-ignorable code points (DerivedCoreProperties.txt) and
/// the interlinear annotation controls: characters a renderer draws as
/// nothing, so a name can carry them unseen.
fn ignorable(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00AD
            | 0x034F
            | 0x061C
            | 0x115F..=0x1160
            | 0x17B4..=0x17B5
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF0..=0xFFFB
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}

/// The character map of the face shaping selects for `font`, parsed once.
fn coverage(font: Font) -> &'static ttf_parser::Face<'static> {
    static FACES: OnceLock<[ttf_parser::Face<'static>; 5]> = OnceLock::new();
    let faces = FACES.get_or_init(|| {
        DRAWABLE.map(|bytes| ttf_parser::Face::parse(bytes, 0).expect("a bundled face parses"))
    });
    // In the order of DRAWABLE. JetBrains Mono's weights cover the same set.
    let index = match font.family {
        font::Family::Name(name) if name == theme::DISPLAY_FAMILY => 1,
        font::Family::Name(name) if name == theme::BODY_FAMILY => 2,
        font::Family::Name(name) if name == theme::MONO_FAMILY => {
            if font.weight == font::Weight::Medium {
                4
            } else {
                3
            }
        }
        _ => 0,
    };
    &faces[index]
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
    fn only_drawable_faces_remain_for_shaping() {
        register().expect("faces register");
        let mut system = iced_graphics::text::font_system()
            .write()
            .expect("write font system");
        let database = system.raw().db();
        assert!(database.faces().all(|face| drawable(&face.source)));
        // Every MONOLITH family still resolves, to our bytes.
        for family in [
            theme::DISPLAY_FAMILY,
            theme::BODY_FAMILY,
            theme::MONO_FAMILY,
        ] {
            let face = database
                .query(&fontdb::Query {
                    families: &[fontdb::Family::Name(family)],
                    ..fontdb::Query::default()
                })
                .unwrap_or_else(|| panic!("{family} resolves"));
            let (source, _) = database.face_source(face).expect("a source");
            assert!(drawable(&source), "{family} resolved to a host face");
        }
    }

    #[test]
    fn every_face_covers_printable_ascii_so_escapes_need_no_lookup() {
        for bytes in DRAWABLE {
            let face = ttf_parser::Face::parse(bytes, 0).expect("a bundled face parses");
            for character in (0x20u8..0x7F).map(char::from) {
                assert!(face.glyph_index(character).is_some(), "{character:?}");
            }
        }
    }

    /// Every code point `bytes` maps to a glyph.
    fn mapped(bytes: &[u8]) -> std::collections::BTreeSet<u32> {
        let face = ttf_parser::Face::parse(bytes, 0).expect("a bundled face parses");
        let mut mapped = std::collections::BTreeSet::new();
        for subtable in face.tables().cmap.expect("a character map").subtables {
            if subtable.is_unicode() {
                subtable.codepoints(|code| {
                    if subtable.glyph_index(code).is_some_and(|glyph| glyph.0 != 0) {
                        mapped.insert(code);
                    }
                });
            }
        }
        mapped
    }

    #[test]
    fn both_mono_weights_cover_the_same_characters() {
        // `coverage` answers for either weight from its own map; a release
        // of the face that split their sets should be seen.
        let regular = mapped(theme::MONO_BYTES);
        assert!(regular.len() > 1000);
        assert_eq!(regular, mapped(theme::MONO_LABEL_BYTES));
    }

    #[test]
    fn plain_text_comes_back_as_it_went_in() {
        let name = "2026-11-02_11-32-55.png";
        assert!(matches!(shown(MONO, name, Breaks::Escape), Cow::Borrowed(text) if text == name));
        let owned = String::from("Ωμέγα Кириллица");
        assert!(
            matches!(shown(MONO, owned, Breaks::Escape), Cow::Owned(text) if text == "Ωμέγα Кириллица")
        );
        assert_eq!(shown(MONO, "a\tb\nc", Breaks::Keep), "a\tb\nc");
    }

    #[test]
    fn what_a_face_cannot_set_or_would_hide_is_escaped() {
        // Space Grotesk has no Cyrillic; JetBrains Mono has.
        assert_eq!(shown(BODY, "Ж", Breaks::Escape), "\\u{416}");
        assert_eq!(shown(MONO, "Ж", Breaks::Escape), "Ж");
        assert_eq!(
            shown(MONO, "日本.png", Breaks::Escape),
            "\\u{65e5}\\u{672c}.png"
        );
        assert_eq!(shown(MONO, "😀", Breaks::Escape), "\\u{1f600}");
        // The classic: an override that shows gnp.exe as exe.png.
        assert_eq!(
            shown(MONO, "\u{202e}gnp.exe", Breaks::Escape),
            "\\u{202e}gnp.exe"
        );
        // Invisible and look-alike characters show.
        assert_eq!(
            shown(MONO, "a\u{200b}b\u{a0}c", Breaks::Escape),
            "a\\u{200b}b\\u{a0}c"
        );
        assert_eq!(shown(MONO, "\u{1b}[31m", Breaks::Escape), "\\u{1b}[31m");
        // One line keeps nothing of line structure.
        assert_eq!(shown(MONO, "a\nb\tc", Breaks::Escape), "a\\u{a}b\\u{9}c");
    }

    #[test]
    fn an_escape_cannot_be_forged() {
        assert_eq!(shown(MONO, "\\u{41}", Breaks::Escape), "\\\\u{41}");
    }

    #[test]
    fn a_preview_keeps_its_lines_and_ends_one_at_a_carriage_return() {
        assert_eq!(shown(MONO, "a\r\nb\rc\n", Breaks::Keep), "a\nb\nc\n");
    }

    #[test]
    fn the_published_tracking_reaches_the_faces_that_carry_it() {
        // Prose is set solid; labels and wordmarks are tracked out.
        assert_eq!(BODY.tracking, font::Tracking::NONE);
        assert_eq!(MONO.tracking, font::Tracking::NONE);
        assert_eq!(MONO_LABEL.tracking, font::Tracking(theme::TRACK_LABEL));
        assert_eq!(MONO_NAV.tracking, font::Tracking(theme::TRACK_NAV));
        assert_eq!(DISPLAY.tracking, font::Tracking(theme::TRACK_WORDMARK));
        assert_eq!(SPLASH.tracking, font::Tracking(theme::TRACK_SPLASH));
        // Navigation is set tighter than a label, as the system has it.
        const { assert!(MONO_NAV.tracking.0 < MONO_LABEL.tracking.0) };
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
        assert_eq!(MONO_LABEL.weight, font::Weight::Medium);
    }
}
