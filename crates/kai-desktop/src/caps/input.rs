//! The Input capability (K23): `type_text` and `key_press` (WP16c). A provider receives what the
//! tools have already checked and parsed: the text as X keysyms, the keys as evdev codes. The
//! `plasma` provider sends them through `org_kde_kwin_fake_input`, the `fdo` provider through the
//! RemoteDesktop portal.
//!
//! **Text** goes as keysyms, one press and one release each, so the active layout's keymap does not
//! matter: [`keysyms`] turns a text into them (Latin-1 is its own code point, the rest is
//! `0x0100_0000 + code point`, a line break is Return and a tab is Tab).
//!
//! **Keys** go as evdev codes: [`parse_keys`] reads the `key_press` text. Names are the keys of a US
//! keyboard (`a` is the key that is `a` there); on another layout the key at that place produces
//! its own letter.
//!
//! **The guard:** a provider asks the call's [`Guard`] before every character and every chord (and
//! so before the first), and stops where it refuses; the tools' guard checks that the window the
//! owner confirmed still has the keyboard focus. **Pacing** is the same for every provider: each
//! event is acknowledged by the compositor's side (a round trip) before the next, and every
//! release is followed by [`RELEASE_PAUSE`].

use std::sync::Arc;
use std::time::Duration;

use super::Fut;

/// The pause after each key release (a character, a key of a chord), after the release has been
/// acknowledged: it leaves the application a moment to take a character before the next arrives.
/// 2 ms kept a 1000-character text exact in GTK and Qt through KWin's fake input and through the
/// RemoteDesktop portal (the live check). It is pacing, not a limit: the whole text is typed.
pub const RELEASE_PAUSE: Duration = Duration::from_millis(2);

/// One chord: the evdev codes in the order they are pressed. They are released in the reverse
/// order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord(pub Vec<u32>);

/// Asked before each unit a provider sends (a character, a chord): `done` units are sent. An error
/// stops the call there, with every key released; it is the call's answer.
pub trait Guard: Send + Sync {
    fn check(&self, done: usize) -> Fut<'_, ()>;
}

pub trait Input: Send + Sync {
    /// Types the keysyms, in order, into whatever has the keyboard focus, asking `guard` before
    /// each.
    fn type_keysyms<'a>(&'a self, keysyms: &'a [u32], guard: Arc<dyn Guard>) -> Fut<'a, ()>;

    /// Presses the chords one after the other, asking `guard` before each.
    fn press_chords<'a>(&'a self, chords: &'a [Chord], guard: Arc<dyn Guard>) -> Fut<'a, ()>;
}

/// The keysym a character is typed as, or `None` for one that is no key: a control character other
/// than a line break and a tab, and the characters of the C1 block.
pub fn keysym(c: char) -> Option<u32> {
    let cp = c as u32;
    match c {
        '\n' => Some(0xff0d),
        '\t' => Some(0xff09),
        _ if (0x20..=0x7e).contains(&cp) || (0xa0..=0xff).contains(&cp) => Some(cp),
        _ if cp < 0x20 || (0x7f..0xa0).contains(&cp) => None,
        _ => Some(0x0100_0000 + cp),
    }
}

/// A text as keysyms. A carriage return before a line feed is dropped (one Return), a lone one is
/// a Return. The error names the first character that is no key (by its code point, never the
/// text).
pub fn keysyms(text: &str) -> Result<Vec<u32>, String> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let c = if c == '\r' {
            if chars.peek() == Some(&'\n') {
                continue;
            }
            '\n'
        } else {
            c
        };
        match keysym(c) {
            Some(k) => out.push(k),
            None => {
                return Err(format!(
                    "text holds a control character (U+{:04X}) that is no key; only line breaks \
                     and tabs are typed",
                    c as u32
                ));
            }
        }
    }
    Ok(out)
}

/// The evdev code of a modifier's name.
fn modifier(name: &str) -> Option<u32> {
    Some(match name {
        "ctrl" | "control" => 29,
        "shift" => 42,
        "alt" => 56,
        "altgr" => 100,
        "super" | "meta" | "win" | "windows" => 125,
        _ => return None,
    })
}

/// The evdev code of a key's name (a US keyboard's, lower case).
fn key(name: &str) -> Option<u32> {
    if let Some(m) = modifier(name) {
        return Some(m);
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        for (row, first) in [("qwertyuiop", 16), ("asdfghjkl", 30), ("zxcvbnm", 44)] {
            if let Some(i) = row.find(c) {
                return Some(first + i as u32);
            }
        }
        return Some(match c {
            '1'..='9' => 2 + (c as u32 - '1' as u32),
            '0' => 11,
            '-' => 12,
            '=' => 13,
            '[' => 26,
            ']' => 27,
            ';' => 39,
            '\'' => 40,
            '`' => 41,
            '\\' => 43,
            ',' => 51,
            '.' => 52,
            '/' => 53,
            _ => return None,
        });
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return match n {
            1..=10 => Some(58 + n),
            11 | 12 => Some(76 + n),
            13..=24 => Some(170 + n),
            _ => None,
        };
    }
    Some(match name {
        "esc" | "escape" => 1,
        "minus" => 12,
        "equal" | "equals" => 13,
        "backspace" => 14,
        "tab" => 15,
        "enter" | "return" => 28,
        "capslock" => 58,
        "space" => 57,
        "plus" => 78,
        "print" | "printscreen" => 99,
        "menu" => 127,
        "pause" => 119,
        "home" => 102,
        "up" => 103,
        "pageup" | "pgup" => 104,
        "left" => 105,
        "right" => 106,
        "end" => 107,
        "down" => 108,
        "pagedown" | "pgdn" => 109,
        "insert" | "ins" => 110,
        "delete" | "del" => 111,
        _ => return None,
    })
}

/// What `key_press` accepts, for the error that says it.
pub const KEY_NAMES: &str = "chords of key names joined by +, separated by spaces (ctrl+a, \
     alt+tab, enter): letters, digits, f1 to f24, ctrl, shift, alt, altgr, super, enter, esc, tab, \
     space, backspace, delete, insert, home, end, pageup, pagedown, up, down, left, right, \
     capslock, print, menu, pause, plus, minus, equal and the US keyboard's punctuation keys";

/// The chords `keys` names, in order (module doc). The error says which chord and what is wrong;
/// it never repeats the text.
pub fn parse_keys(keys: &str) -> Result<Vec<Chord>, String> {
    let mut chords = Vec::new();
    for (i, word) in keys.split_whitespace().enumerate() {
        let n = i + 1;
        let mut codes = Vec::new();
        for name in word.split('+') {
            let code = if name.is_empty() {
                None
            } else {
                key(&name.to_lowercase())
            };
            match code {
                Some(c) => codes.push(c),
                None => {
                    return Err(format!(
                        "chord {n} has a key name Kai does not know; keys is {KEY_NAMES}"
                    ));
                }
            }
        }
        chords.push(Chord(codes));
    }
    if chords.is_empty() {
        return Err(format!("keys is empty; it is {KEY_NAMES}"));
    }
    Ok(chords)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_typed_as_the_spike_verified_keysyms() {
        assert_eq!(
            keysyms("aä€\n\t😀").unwrap(),
            [0x61, 0xe4, 0x0100_20ac, 0xff0d, 0xff09, 0x0101_f600]
        );
        assert_eq!(
            keysyms("a\r\nb\rc").unwrap(),
            [0x61, 0xff0d, 0x62, 0xff0d, 0x63]
        );
        assert_eq!(keysyms("").unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn a_control_character_is_refused_without_echoing_the_text() {
        let e = keysyms("secret\u{7}word").unwrap_err();
        assert!(e.contains("U+0007"), "{e}");
        assert!(!e.contains("secret"), "{e}");
        assert!(keysyms("a\u{85}").is_err());
        assert!(keysyms("\u{7f}").is_err());
    }

    #[test]
    fn keys_are_chords_of_evdev_codes() {
        let codes = |s: &str| -> Vec<Vec<u32>> {
            parse_keys(s).unwrap().into_iter().map(|c| c.0).collect()
        };
        assert_eq!(codes("ctrl+a"), [vec![29, 30]]);
        assert_eq!(codes("Ctrl+Shift+T enter"), [vec![29, 42, 20], vec![28]]);
        assert_eq!(codes("alt+F4"), [vec![56, 62]]);
        assert_eq!(
            codes("f11 f12 f13 f24"),
            [vec![87], vec![88], vec![183], vec![194]]
        );
        assert_eq!(codes("1 0 - ="), [vec![2], vec![11], vec![12], vec![13]]);
        assert_eq!(
            codes("q p a l z m"),
            [vec![16], vec![25], vec![30], vec![38], vec![44], vec![50]]
        );
        assert_eq!(codes("super+d"), [vec![125, 32]]);
    }

    #[test]
    fn a_bad_chord_names_its_place_and_not_its_text() {
        for bad in [
            "",
            "   ",
            "ctrl+",
            "ctrl+nonsense",
            "+a",
            "ctrl++a",
            "f25",
            "f0",
        ] {
            let e = parse_keys(bad).unwrap_err();
            assert!(e.contains("key"), "{bad}: {e}");
            assert!(!e.contains("nonsense"), "{bad}: {e}");
        }
        assert!(
            parse_keys("a ctrl+zzz")
                .unwrap_err()
                .starts_with("chord 2 ")
        );
    }
}
