//! Text from outside the binary, made safe to show.
//!
//! Machine names, ISO file names, project names and engine messages all come
//! from disk, a guest or a person. The front end renders them as text only, but
//! a control character, a bidirectional override or an invisible code point
//! still changes what a person reads, so each is shown as a visible `\u{..}`
//! escape, the same boundary `Typography::data` keeps in the iced desktop. The
//! result is also bounded, so no field is unbounded on the wire.

/// Text a guest drew on its console: hidden characters are escaped as in
/// [`shown`], line breaks are kept, and the length is the caller's to bound.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c != '\n' && is_hidden(c) {
            out.push_str(&format!("\\u{{{:04X}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// The longest any one shown field may be, in characters.
pub const MAX_CHARS: usize = 240;

/// Whether `c` changes how text reads without being a visible glyph.
fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            // Zero-width and joiner characters, word joiner, invisible math operators.
            '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}'
            // Bidirectional embeddings, overrides and isolates.
            | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            // Line and paragraph separators, byte-order mark, deprecated format characters.
            | '\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{206A}'..='\u{206F}'
            // Interlinear annotation and object replacement.
            | '\u{FFF9}'..='\u{FFFC}'
        )
}

/// `text` with hidden characters escaped and at most [`MAX_CHARS`] characters,
/// ending in `…` when it was cut.
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_CHARS));
    let mut count = 0;
    for c in text.chars() {
        let piece = if is_hidden(c) {
            format!("\\u{{{:04X}}}", c as u32)
        } else {
            c.to_string()
        };
        let width = piece.chars().count();
        if count + width > MAX_CHARS {
            out.push('…');
            return out;
        }
        count += width;
        out.push_str(&piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(shown("kore-dev · 4 GiB"), "kore-dev · 4 GiB");
    }

    #[test]
    fn controls_are_visible() {
        assert_eq!(shown("a\u{1b}[31mb\n"), "a\\u{001B}[31mb\\u{000A}");
        assert_eq!(shown("x\0y"), "x\\u{0000}y");
    }

    #[test]
    fn bidi_and_invisible_characters_are_visible() {
        for c in ['\u{202E}', '\u{2066}', '\u{200B}', '\u{FEFF}', '\u{2028}'] {
            let out = shown(&format!("a{c}b"));
            assert!(out.contains("\\u{"), "{c:?} was shown raw");
            assert!(!out.contains(c));
        }
    }

    #[test]
    fn console_text_keeps_its_lines_and_escapes_the_rest() {
        assert_eq!(
            escape_text("ls\n\u{1b}[0m\u{202E}x"),
            "ls\n\\u{001B}[0m\\u{202E}x"
        );
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let out = shown(&"é".repeat(1000));
        assert_eq!(out.chars().count(), MAX_CHARS + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn an_escape_is_never_split() {
        // 239 plain characters leave one slot, too few for a six-character escape.
        let out = shown(&format!("{}\u{202E}", "a".repeat(MAX_CHARS - 1)));
        assert!(out.ends_with("a…"), "{out}");
    }
}
