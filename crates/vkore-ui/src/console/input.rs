//! Keyboard encoding for the existing VT terminal. No local line editor or echo.
use iced_core::{
    Event,
    keyboard::{self, Key, key::Named},
};

pub(super) enum Action {
    Send(Vec<u8>),
    Release,
    Paste,
}

pub(super) fn key(event: &Event, application_cursor: bool) -> Option<Action> {
    let Event::Keyboard(keyboard::Event::KeyPressed {
        key,
        modifiers,
        text,
        ..
    }) = event
    else {
        return None;
    };
    if *key == Key::Named(Named::Escape) && modifiers.control() && modifiers.shift() {
        return Some(Action::Release);
    }
    if matches!(key.as_ref(), Key::Character("v" | "V"))
        && ((modifiers.control() && modifiers.shift()) || modifiers.logo())
    {
        return Some(Action::Paste);
    }
    // Leave platform shortcuts such as Command-Q to the host.
    if modifiers.logo() {
        return None;
    }
    let modifier = 1
        + u8::from(modifiers.shift())
        + 2 * u8::from(modifiers.alt())
        + 4 * u8::from(modifiers.control());
    let navigation = |final_byte| {
        if modifier != 1 {
            format!("\x1b[1;{modifier}{final_byte}")
        } else if application_cursor {
            format!("\x1bO{final_byte}")
        } else {
            format!("\x1b[{final_byte}")
        }
    };
    let tilde = |number| {
        if modifier == 1 {
            format!("\x1b[{number}~")
        } else {
            format!("\x1b[{number};{modifier}~")
        }
    };
    let bytes = match key.as_ref() {
        Key::Named(Named::Enter) => vec![b'\r'],
        Key::Named(Named::Backspace) if modifiers.alt() => vec![0x1b, 0x7f],
        Key::Named(Named::Backspace) => vec![0x7f],
        Key::Named(Named::Space) if modifiers.control() => vec![0],
        Key::Named(Named::Tab) if modifiers.shift() => b"\x1b[Z".to_vec(),
        Key::Named(Named::Tab) => vec![b'\t'],
        Key::Named(Named::Escape) => vec![0x1b],
        Key::Named(Named::ArrowUp) => navigation('A').into_bytes(),
        Key::Named(Named::ArrowDown) => navigation('B').into_bytes(),
        Key::Named(Named::ArrowRight) => navigation('C').into_bytes(),
        Key::Named(Named::ArrowLeft) => navigation('D').into_bytes(),
        Key::Named(Named::Home) => navigation('H').into_bytes(),
        Key::Named(Named::End) => navigation('F').into_bytes(),
        Key::Named(Named::Insert) => tilde(2).into_bytes(),
        Key::Named(Named::Delete) => tilde(3).into_bytes(),
        Key::Named(Named::PageUp) => tilde(5).into_bytes(),
        Key::Named(Named::PageDown) => tilde(6).into_bytes(),
        Key::Named(named @ (Named::F1 | Named::F2 | Named::F3 | Named::F4)) => {
            let letter = match named {
                Named::F1 => 'P',
                Named::F2 => 'Q',
                Named::F3 => 'R',
                _ => 'S',
            };
            if modifier == 1 {
                format!("\x1bO{letter}")
            } else {
                format!("\x1b[1;{modifier}{letter}")
            }
            .into_bytes()
        }
        Key::Named(
            named @ (Named::F5
            | Named::F6
            | Named::F7
            | Named::F8
            | Named::F9
            | Named::F10
            | Named::F11
            | Named::F12),
        ) => tilde(match named {
            Named::F5 => 15,
            Named::F6 => 17,
            Named::F7 => 18,
            Named::F8 => 19,
            Named::F9 => 20,
            Named::F10 => 21,
            Named::F11 => 23,
            _ => 24,
        })
        .into_bytes(),
        Key::Character(value) if modifiers.control() => {
            let byte = match value.as_bytes() {
                [b'a'..=b'z' | b'A'..=b'Z' | b'@' | b'[' | b'\\' | b']' | b'^' | b'_'] => {
                    value.as_bytes()[0] & 0x1f
                }
                [b' ' | b'2'] => 0,
                [b'3'] => 0x1b,
                [b'4'] => 0x1c,
                [b'5'] => 0x1d,
                [b'6'] => 0x1e,
                [b'7' | b'/'] => 0x1f,
                [b'8' | b'?'] => 0x7f,
                _ => return None,
            };
            if modifiers.alt() {
                vec![0x1b, byte]
            } else {
                vec![byte]
            }
        }
        _ => {
            let text = text.as_deref().or_else(|| match key.as_ref() {
                Key::Character(s) => Some(s),
                _ => None,
            })?;
            if text.is_empty() || text.chars().any(char::is_control) {
                return None;
            }
            let mut bytes = Vec::new();
            if modifiers.alt() {
                bytes.push(0x1b);
            }
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
    };
    Some(Action::Send(bytes))
}

#[cfg(test)]
pub(super) fn press(key: Key, modifiers: keyboard::Modifiers, text: Option<&str>) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        modified_key: key.clone(),
        key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text: text.map(Into::into),
        repeat: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard::Modifiers;
    fn bytes(
        key_value: Key,
        modifiers: Modifiers,
        text: Option<&str>,
        application: bool,
    ) -> Vec<u8> {
        match key(&press(key_value, modifiers, text), application).unwrap() {
            Action::Send(bytes) => bytes,
            _ => panic!("expected guest bytes"),
        }
    }
    #[test]
    fn text_editing_interrupts_and_cursor_modes_are_encoded_without_local_echo() {
        assert_eq!(
            bytes(
                Key::Character("é".into()),
                Modifiers::empty(),
                Some("é"),
                false
            ),
            "é".as_bytes()
        );
        for (key, expected) in [
            (Named::Enter, b"\r".as_slice()),
            (Named::Backspace, b"\x7f"),
            (Named::Tab, b"\t"),
            (Named::Delete, b"\x1b[3~"),
            (Named::Home, b"\x1b[H"),
            (Named::ArrowLeft, b"\x1b[D"),
        ] {
            assert_eq!(
                bytes(Key::Named(key), Modifiers::empty(), None, false),
                expected
            );
        }
        assert_eq!(
            bytes(Key::Character("c".into()), Modifiers::CTRL, None, false),
            b"\x03"
        );
        assert_eq!(
            bytes(Key::Character("d".into()), Modifiers::CTRL, None, false),
            b"\x04"
        );
        assert_eq!(
            bytes(Key::Named(Named::ArrowUp), Modifiers::empty(), None, true),
            b"\x1bOA"
        );
        assert_eq!(
            bytes(Key::Named(Named::ArrowRight), Modifiers::CTRL, None, true),
            b"\x1b[1;5C"
        );
        assert!(matches!(
            key(
                &press(
                    Key::Named(Named::Escape),
                    Modifiers::CTRL | Modifiers::SHIFT,
                    None
                ),
                false
            ),
            Some(Action::Release)
        ));
        assert!(matches!(
            key(
                &press(
                    Key::Character("v".into()),
                    Modifiers::CTRL | Modifiers::SHIFT,
                    None
                ),
                false
            ),
            Some(Action::Paste)
        ));
        assert!(
            key(
                &press(Key::Character("q".into()), Modifiers::LOGO, None),
                false
            )
            .is_none()
        );
    }
}
