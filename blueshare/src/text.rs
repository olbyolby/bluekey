use std::{fmt, time::Duration};

use keycode::{KeyMap, KeyMappingCode};

use crate::bluetooth::{Target, keyboard::Keyboard};

// USB HID usage for the left shift key. The keyboard server treats 0xE0..=0xE7 as modifiers.
const LEFT_SHIFT: u8 = 0xE1;

const LETTERS: [KeyMappingCode; 26] = [
    KeyMappingCode::KeyA, KeyMappingCode::KeyB, KeyMappingCode::KeyC, KeyMappingCode::KeyD,
    KeyMappingCode::KeyE, KeyMappingCode::KeyF, KeyMappingCode::KeyG, KeyMappingCode::KeyH,
    KeyMappingCode::KeyI, KeyMappingCode::KeyJ, KeyMappingCode::KeyK, KeyMappingCode::KeyL,
    KeyMappingCode::KeyM, KeyMappingCode::KeyN, KeyMappingCode::KeyO, KeyMappingCode::KeyP,
    KeyMappingCode::KeyQ, KeyMappingCode::KeyR, KeyMappingCode::KeyS, KeyMappingCode::KeyT,
    KeyMappingCode::KeyU, KeyMappingCode::KeyV, KeyMappingCode::KeyW, KeyMappingCode::KeyX,
    KeyMappingCode::KeyY, KeyMappingCode::KeyZ,
];

const DIGITS: [KeyMappingCode; 9] = [
    KeyMappingCode::Digit1, KeyMappingCode::Digit2, KeyMappingCode::Digit3,
    KeyMappingCode::Digit4, KeyMappingCode::Digit5, KeyMappingCode::Digit6,
    KeyMappingCode::Digit7, KeyMappingCode::Digit8, KeyMappingCode::Digit9,
];

/// Map a character to a USB HID keyboard usage and whether shift must be held.
///
/// The mapping assumes a US keyboard layout, as HID usages are interpreted by the host's layout.
pub fn char_to_hid(character: char) -> Option<(u8, bool)> {
    let (code, shift) = match character {
        'a'..='z' => (LETTERS[(character as u8 - b'a') as usize], false),
        'A'..='Z' => (LETTERS[(character as u8 - b'A') as usize], true),
        '1'..='9' => (DIGITS[(character as u8 - b'1') as usize], false),
        '0' => (KeyMappingCode::Digit0, false),

        '!' => (KeyMappingCode::Digit1, true),
        '@' => (KeyMappingCode::Digit2, true),
        '#' => (KeyMappingCode::Digit3, true),
        '$' => (KeyMappingCode::Digit4, true),
        '%' => (KeyMappingCode::Digit5, true),
        '^' => (KeyMappingCode::Digit6, true),
        '&' => (KeyMappingCode::Digit7, true),
        '*' => (KeyMappingCode::Digit8, true),
        '(' => (KeyMappingCode::Digit9, true),
        ')' => (KeyMappingCode::Digit0, true),

        '\n' | '\r' => (KeyMappingCode::Enter, false),
        '\t' => (KeyMappingCode::Tab, false),
        ' ' => (KeyMappingCode::Space, false),
        '\x08' => (KeyMappingCode::Backspace, false),
        '\x1b' => (KeyMappingCode::Escape, false),
        '\x7f' => (KeyMappingCode::Delete, false),

        '-' => (KeyMappingCode::Minus, false),
        '_' => (KeyMappingCode::Minus, true),
        '=' => (KeyMappingCode::Equal, false),
        '+' => (KeyMappingCode::Equal, true),
        '[' => (KeyMappingCode::BracketLeft, false),
        '{' => (KeyMappingCode::BracketLeft, true),
        ']' => (KeyMappingCode::BracketRight, false),
        '}' => (KeyMappingCode::BracketRight, true),
        '\\' => (KeyMappingCode::Backslash, false),
        '|' => (KeyMappingCode::Backslash, true),
        ';' => (KeyMappingCode::Semicolon, false),
        ':' => (KeyMappingCode::Semicolon, true),
        '\'' => (KeyMappingCode::Quote, false),
        '"' => (KeyMappingCode::Quote, true),
        '`' => (KeyMappingCode::Backquote, false),
        '~' => (KeyMappingCode::Backquote, true),
        ',' => (KeyMappingCode::Comma, false),
        '<' => (KeyMappingCode::Comma, true),
        '.' => (KeyMappingCode::Period, false),
        '>' => (KeyMappingCode::Period, true),
        '/' => (KeyMappingCode::Slash, false),
        '?' => (KeyMappingCode::Slash, true),

        _ => return None,
    };

    Some((KeyMap::from(code).usb as u8, shift))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    UnsupportedCharacter { character: char, position: usize },
    ServerDied,
}
impl fmt::Display for TextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCharacter { character, position } => write!(
                f,
                "cannot type character {:?} at position {}; only US keyboard characters are supported",
                character, position
            ),
            Self::ServerDied => write!(f, "keyboard server died"),
        }
    }
}
impl std::error::Error for TextError {}

/// Check that every character in `text` can be typed before sending anything.
pub fn validate_text(text: &str) -> Result<(), TextError> {
    for (position, character) in text.chars().enumerate() {
        if char_to_hid(character).is_none() {
            return Err(TextError::UnsupportedCharacter { character, position });
        }
    }
    Ok(())
}

/// Type `text` on `target` by pressing and releasing keys with `delay` between events.
pub async fn type_text(
    keyboard: &Keyboard,
    target: Target,
    text: &str,
    delay: Duration,
) -> Result<(), TextError> {
    // Normalize Windows line endings so they produce a single Enter.
    let text = text.replace("\r\n", "\n");
    validate_text(&text)?;

    for character in text.chars() {
        let (keycode, shift) = char_to_hid(character).expect("text was validated");

        if shift {
            keyboard.press(target, LEFT_SHIFT).await.map_err(|_| TextError::ServerDied)?;
            tokio::time::sleep(delay).await;
        }

        keyboard.press(target, keycode).await.map_err(|_| TextError::ServerDied)?;
        tokio::time::sleep(delay).await;
        keyboard.release(target, keycode).await.map_err(|_| TextError::ServerDied)?;
        tokio::time::sleep(delay).await;

        if shift {
            keyboard.release(target, LEFT_SHIFT).await.map_err(|_| TextError::ServerDied)?;
            tokio::time::sleep(delay).await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_lowercase_letters() {
        assert_eq!(char_to_hid('a'), Some((0x04, false)));
        assert_eq!(char_to_hid('z'), Some((0x1d, false)));
    }

    #[test]
    fn maps_uppercase_letters_with_shift() {
        assert_eq!(char_to_hid('A'), Some((0x04, true)));
        assert_eq!(char_to_hid('Z'), Some((0x1d, true)));
    }

    #[test]
    fn maps_digits_and_shifted_digits() {
        assert_eq!(char_to_hid('1'), Some((0x1e, false)));
        assert_eq!(char_to_hid('0'), Some((0x27, false)));
        assert_eq!(char_to_hid('!'), Some((0x1e, true)));
        assert_eq!(char_to_hid(')'), Some((0x27, true)));
    }

    #[test]
    fn maps_control_characters() {
        assert_eq!(char_to_hid('\n'), Some((0x28, false)));
        assert_eq!(char_to_hid('\t'), Some((0x2b, false)));
        assert_eq!(char_to_hid(' '), Some((0x2c, false)));
        assert_eq!(char_to_hid('\x08'), Some((0x2a, false)));
    }

    #[test]
    fn rejects_non_ascii() {
        assert_eq!(char_to_hid('é'), None);
        assert_eq!(char_to_hid('😀'), None);
    }

    #[test]
    fn validates_whole_text() {
        assert!(validate_text("hello\n").is_ok());
        assert_eq!(
            validate_text("héllo"),
            Err(TextError::UnsupportedCharacter { character: 'é', position: 1 })
        );
    }
}
