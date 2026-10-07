use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Raw winit input is the terminal's sole keyboard owner. egui still sees the
/// events for focus/platform integration, but its Text/Copy/Paste events are
/// never also sent to the PTY.
pub fn encode_key(
    key: &Key,
    text: Option<&str>,
    modifiers: ModifiersState,
    application_cursor: bool,
) -> Vec<u8> {
    // AltGr can be reported as Ctrl+Alt. A printable text result takes
    // precedence over control-key encoding for that layout combination.
    if modifiers.control_key()
        && modifiers.alt_key()
        && let Some(text) =
            text.filter(|text| !text.is_empty() && text.chars().all(|ch| !ch.is_control()))
    {
        return text.as_bytes().to_vec();
    }
    let modifier = 1
        + u8::from(modifiers.shift_key())
        + 2 * u8::from(modifiers.alt_key())
        + 4 * u8::from(modifiers.control_key());
    let sequence = match key {
        Key::Named(named) => {
            let cursor = match named {
                NamedKey::ArrowUp => Some('A'),
                NamedKey::ArrowDown => Some('B'),
                NamedKey::ArrowRight => Some('C'),
                NamedKey::ArrowLeft => Some('D'),
                NamedKey::Home => Some('H'),
                NamedKey::End => Some('F'),
                _ => None,
            };
            if let Some(code) = cursor {
                return if modifier != 1 {
                    format!("\x1b[1;{modifier}{code}").into_bytes()
                } else if application_cursor {
                    format!("\x1bO{code}").into_bytes()
                } else {
                    format!("\x1b[{code}").into_bytes()
                };
            }
            let tilde = match named {
                NamedKey::Insert => Some(2),
                NamedKey::Delete => Some(3),
                NamedKey::PageUp => Some(5),
                NamedKey::PageDown => Some(6),
                NamedKey::F5 => Some(15),
                NamedKey::F6 => Some(17),
                NamedKey::F7 => Some(18),
                NamedKey::F8 => Some(19),
                NamedKey::F9 => Some(20),
                NamedKey::F10 => Some(21),
                NamedKey::F11 => Some(23),
                NamedKey::F12 => Some(24),
                _ => None,
            };
            if let Some(code) = tilde {
                return if modifier == 1 {
                    format!("\x1b[{code}~").into_bytes()
                } else {
                    format!("\x1b[{code};{modifier}~").into_bytes()
                };
            }
            match named {
                NamedKey::Enter => "\r".to_owned(),
                NamedKey::Backspace => "\x7f".to_owned(),
                NamedKey::Escape => "\x1b".to_owned(),
                NamedKey::Tab if modifiers.shift_key() => return b"\x1b[Z".to_vec(),
                NamedKey::Tab => "\t".to_owned(),
                NamedKey::Space => if modifiers.control_key() { "\0" } else { " " }.to_owned(),
                NamedKey::F1 | NamedKey::F2 | NamedKey::F3 | NamedKey::F4 => {
                    let code = match named {
                        NamedKey::F1 => 'P',
                        NamedKey::F2 => 'Q',
                        NamedKey::F3 => 'R',
                        _ => 'S',
                    };
                    return if modifier == 1 {
                        format!("\x1bO{code}").into_bytes()
                    } else {
                        format!("\x1b[1;{modifier}{code}").into_bytes()
                    };
                }
                _ => return Vec::new(),
            }
        }
        Key::Character(value) if modifiers.control_key() => {
            let ch = value.chars().next().unwrap_or('\0');
            let control = match ch {
                'a'..='z' => ch as u8 - b'a' + 1,
                '@'..='_' => ch as u8 & 0x1f,
                ' ' | '2' => 0,
                '3' => 27,
                '4' => 28,
                '5' => 29,
                '6' => 30,
                '7' | '/' => 31,
                '8' | '?' => 127,
                _ => return Vec::new(),
            };
            String::from(char::from(control))
        }
        Key::Character(value) => text.unwrap_or(value).to_owned(),
        _ => return Vec::new(),
    };
    let mut bytes = Vec::new();
    if modifiers.alt_key() {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(sequence.as_bytes());
    bytes
}

pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    if bracketed {
        // A pasted escape must not terminate the bracketed paste envelope.
        format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
    } else {
        text.replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_keys_and_cursor_modes() {
        let none = ModifiersState::empty();
        assert_eq!(
            encode_key(
                &Key::Character("c".into()),
                Some("c"),
                ModifiersState::CONTROL,
                false
            ),
            b"\x03"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::ArrowUp), None, none, true),
            b"\x1bOA"
        );
        assert_eq!(
            encode_key(
                &Key::Named(NamedKey::ArrowLeft),
                None,
                ModifiersState::CONTROL,
                false
            ),
            b"\x1b[1;5D"
        );
        assert_eq!(
            encode_key(&Key::Character("é".into()), Some("é"), none, false),
            "é".as_bytes()
        );
        assert_eq!(
            encode_key(
                &Key::Character("x".into()),
                Some("x"),
                ModifiersState::ALT,
                false
            ),
            b"\x1bx"
        );
        assert_eq!(
            encode_key(&Key::Named(NamedKey::Enter), Some("\r"), none, false),
            b"\r"
        );
    }

    #[test]
    fn paste_normalizes_lines_and_preserves_bracket_boundaries() {
        assert_eq!(paste("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(
            paste("a\r\nb\x1b[201~", true),
            b"\x1b[200~a\nb[201~\x1b[201~"
        );
    }
}
