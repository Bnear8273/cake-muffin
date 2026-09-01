//! Key parsing: turn a raw byte stream from the terminal into [`Key`]
//! events. The parser is incremental and stateful so that multi-byte UTF-8
//! characters and ESC sequences split across separate `Platform::read` calls
//! are reassembled correctly.

use alloc::vec::Vec;
use core::mem;

/// Control characters the editor acts on (mapped from raw bytes 0x01-0x1a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlKey {
    A,
    B,
    C,
    D,
    E,
    F,
    K,
    L,
    U,
    W,
}

/// A single decoded key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Ctrl(CtrlKey),
    Esc,
}

enum State {
    Idle,
    /// Accumulating UTF-8 continuation bytes for a multi-byte character.
    Utf8 {
        buf: [u8; 4],
        len: u8,
        need: u8,
    },
    /// Saw a bare ESC; waiting for the byte that follows it.
    Esc,
    /// Saw ESC `[`; collecting CSI parameters until the final byte.
    Csi {
        params: Vec<u8>,
    },
    /// Saw ESC `O` (SS3); exactly one final byte follows.
    Ss3,
}

/// Incremental byte-stream -> key parser.
pub struct KeyParser {
    state: State,
}

impl Default for KeyParser {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyParser {
    pub fn new() -> Self {
        KeyParser { state: State::Idle }
    }

    /// Push raw bytes; every complete key decodes into `out`. Partial UTF-8
    /// characters and ESC sequences are buffered internally.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Key>) {
        for &b in bytes {
            self.step(b, out);
        }
    }

    /// True when a multi-byte sequence is incomplete and needs more input.
    pub fn needs_more(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    fn step(&mut self, b: u8, out: &mut Vec<Key>) {
        match mem::replace(&mut self.state, State::Idle) {
            State::Idle => self.idle(b, out),
            State::Utf8 { buf, len, need } => self.utf8(b, buf, len, need, out),
            State::Esc => self.esc(b, out),
            State::Csi { params } => self.csi(b, params, out),
            State::Ss3 => self.ss3(b, out),
        }
    }

    fn idle(&mut self, b: u8, out: &mut Vec<Key>) {
        match b {
            0x0d | 0x0a => out.push(Key::Enter),
            0x09 => out.push(Key::Tab),
            0x08 | 0x7f => out.push(Key::Backspace),
            0x1b => self.state = State::Esc,
            0x01..=0x1f => {
                if let Some(k) = ctrl_from_byte(b) {
                    out.push(Key::Ctrl(k));
                }
            }
            0x80..=0xff => {
                if b < 0xc0 {
                    // Stray continuation byte: drop.
                } else {
                    let need = utf8_need(b);
                    if need == 0 {
                        // Invalid leading byte: drop.
                    } else {
                        self.state = State::Utf8 {
                            buf: [b, 0, 0, 0],
                            len: 1,
                            need,
                        };
                    }
                }
            }
            _ => out.push(Key::Char(b as char)),
        }
    }

    fn utf8(&mut self, b: u8, mut buf: [u8; 4], len: u8, need: u8, out: &mut Vec<Key>) {
        if (0x80..=0xbf).contains(&b) {
            buf[len as usize] = b;
            let len = len + 1;
            if len > need {
                if let Ok(s) = core::str::from_utf8(&buf[..=need as usize])
                    && let Some(c) = s.chars().next()
                {
                    out.push(Key::Char(c));
                }
                self.state = State::Idle;
            } else {
                self.state = State::Utf8 { buf, len, need };
            }
        } else {
            // Not a continuation byte: abandon the partial character and
            // re-process this byte from scratch.
            self.state = State::Idle;
            self.step(b, out);
        }
    }

    fn esc(&mut self, b: u8, out: &mut Vec<Key>) {
        match b {
            b'[' => self.state = State::Csi { params: Vec::new() },
            b'O' => self.state = State::Ss3,
            _ => {
                // A lone ESC, followed by a plain byte.
                out.push(Key::Esc);
                self.step(b, out);
            }
        }
    }

    fn csi(&mut self, b: u8, mut params: Vec<u8>, out: &mut Vec<Key>) {
        if (0x40..=0x7e).contains(&b) {
            if let Some(k) = csi_key(&params, b) {
                out.push(k);
            }
            self.state = State::Idle;
        } else if b == 0x1b {
            // ESC inside a CSI sequence: restart as a fresh ESC.
            self.state = State::Esc;
        } else if (0x20..=0x3f).contains(&b) {
            if params.len() < 32 {
                params.push(b);
            }
            self.state = State::Csi { params };
        } else {
            // Unexpected control byte: abandon the sequence.
            self.state = State::Idle;
        }
    }

    fn ss3(&mut self, b: u8, out: &mut Vec<Key>) {
        self.state = State::Idle;
        let key = match b {
            b'A' => Some(Key::Up),
            b'B' => Some(Key::Down),
            b'C' => Some(Key::Right),
            b'D' => Some(Key::Left),
            b'H' => Some(Key::Home),
            b'F' => Some(Key::End),
            _ => None,
        };
        if let Some(k) = key {
            out.push(k);
        }
    }
}

fn utf8_need(b: u8) -> u8 {
    match b {
        0xc2..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf4 => 3,
        _ => 0,
    }
}

fn ctrl_from_byte(b: u8) -> Option<CtrlKey> {
    Some(match b {
        0x01 => CtrlKey::A,
        0x02 => CtrlKey::B,
        0x03 => CtrlKey::C,
        0x04 => CtrlKey::D,
        0x05 => CtrlKey::E,
        0x06 => CtrlKey::F,
        0x0b => CtrlKey::K,
        0x0c => CtrlKey::L,
        0x15 => CtrlKey::U,
        0x17 => CtrlKey::W,
        _ => return None,
    })
}

fn csi_key(params: &[u8], final_byte: u8) -> Option<Key> {
    match final_byte {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        b'H' => Some(Key::Home),
        b'F' => Some(Key::End),
        b'Z' => Some(Key::BackTab),
        b'~' => match param_number(params) {
            1 | 7 => Some(Key::Home),
            3 => Some(Key::Delete),
            4 | 8 => Some(Key::End),
            _ => None,
        },
        _ => None,
    }
}

/// The first numeric parameter of a CSI sequence (the value before any `;`).
fn param_number(params: &[u8]) -> u32 {
    let mut n: u32 = 0;
    for &b in params {
        if b.is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add((b - b'0') as u32);
        } else if b == b';' {
            return n;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn parse(bytes: &[u8]) -> Vec<Key> {
        let mut p = KeyParser::new();
        let mut out = Vec::new();
        p.feed(bytes, &mut out);
        out
    }

    fn parse_fragments(frags: &[&[u8]]) -> Vec<Key> {
        let mut p = KeyParser::new();
        let mut out = Vec::new();
        for f in frags {
            p.feed(f, &mut out);
        }
        out
    }

    #[test]
    fn ascii_characters() {
        assert_eq!(
            parse(b"hello"),
            vec![
                Key::Char('h'),
                Key::Char('e'),
                Key::Char('l'),
                Key::Char('l'),
                Key::Char('o'),
            ]
        );
    }

    #[test]
    fn enter_tab_backspace() {
        assert_eq!(parse(b"\r"), vec![Key::Enter]);
        assert_eq!(parse(b"\n"), vec![Key::Enter]);
        assert_eq!(parse(b"\t"), vec![Key::Tab]);
        assert_eq!(parse(b"\x7f"), vec![Key::Backspace]);
        assert_eq!(parse(b"\x08"), vec![Key::Backspace]);
    }

    #[test]
    fn control_keys() {
        assert_eq!(parse(b"\x03"), vec![Key::Ctrl(CtrlKey::C)]);
        assert_eq!(parse(b"\x04"), vec![Key::Ctrl(CtrlKey::D)]);
        assert_eq!(parse(b"\x01"), vec![Key::Ctrl(CtrlKey::A)]);
        assert_eq!(parse(b"\x05"), vec![Key::Ctrl(CtrlKey::E)]);
        assert_eq!(parse(b"\x0b"), vec![Key::Ctrl(CtrlKey::K)]);
        assert_eq!(parse(b"\x15"), vec![Key::Ctrl(CtrlKey::U)]);
        assert_eq!(parse(b"\x17"), vec![Key::Ctrl(CtrlKey::W)]);
        assert_eq!(parse(b"\x0c"), vec![Key::Ctrl(CtrlKey::L)]);
        // Unknown control byte is dropped.
        assert_eq!(parse(b"\x07"), vec![]);
    }

    #[test]
    fn utf8_single_call() {
        assert_eq!(
            parse("a你好é".as_bytes()),
            vec![
                Key::Char('a'),
                Key::Char('你'),
                Key::Char('好'),
                Key::Char('é'),
            ]
        );
    }

    #[test]
    fn utf8_split_across_feeds() {
        let bytes = "你好".as_bytes();
        assert_eq!(
            parse_fragments(&[&bytes[..1], &bytes[1..]]),
            vec![Key::Char('你'), Key::Char('好')]
        );
        // Every single byte split point must reassemble identically.
        for split in 0..bytes.len() {
            assert_eq!(
                parse_fragments(&[&bytes[..split], &bytes[split..]]),
                vec![Key::Char('你'), Key::Char('好')],
                "split at {split}"
            );
        }
    }

    #[test]
    fn invalid_utf8_dropped() {
        // Stray continuation byte.
        assert_eq!(parse(&[0x80]), vec![]);
        // Overlong/invalid leading byte.
        assert_eq!(parse(&[0xc0]), vec![]);
        // Leading byte with an invalid following byte abandons the sequence
        // and re-processes the stray byte.
        assert_eq!(parse(&[0xc3, b'x']), vec![Key::Char('x')]);
    }

    #[test]
    fn needs_more_mid_sequence() {
        let mut p = KeyParser::new();
        let mut out = Vec::new();
        p.feed(&[0x1b], &mut out);
        assert!(p.needs_more());
        p.feed(b"[", &mut out);
        assert!(p.needs_more());
        p.feed(b"C", &mut out);
        assert!(!p.needs_more());
        assert_eq!(out, vec![Key::Right]);
    }

    #[test]
    fn arrow_keys_csi() {
        assert_eq!(parse(b"\x1b[A"), vec![Key::Up]);
        assert_eq!(parse(b"\x1b[B"), vec![Key::Down]);
        assert_eq!(parse(b"\x1b[C"), vec![Key::Right]);
        assert_eq!(parse(b"\x1b[D"), vec![Key::Left]);
        assert_eq!(parse(b"\x1b[H"), vec![Key::Home]);
        assert_eq!(parse(b"\x1b[F"), vec![Key::End]);
        assert_eq!(parse(b"\x1b[Z"), vec![Key::BackTab]);
    }

    #[test]
    fn arrows_split_across_feeds() {
        for split in 0..3 {
            assert_eq!(
                parse_fragments(&[&b"\x1b[A"[..split], &b"\x1b[A"[split..]]),
                vec![Key::Up],
                "split at {split}"
            );
        }
    }

    #[test]
    fn ss3_sequences() {
        assert_eq!(parse(b"\x1bOH"), vec![Key::Home]);
        assert_eq!(parse(b"\x1bOF"), vec![Key::End]);
        assert_eq!(parse(b"\x1bOD"), vec![Key::Left]);
        assert_eq!(parse(b"\x1bOC"), vec![Key::Right]);
    }

    #[test]
    fn tildes() {
        assert_eq!(parse(b"\x1b[3~"), vec![Key::Delete]);
        assert_eq!(parse(b"\x1b[1~"), vec![Key::Home]);
        assert_eq!(parse(b"\x1b[4~"), vec![Key::End]);
        assert_eq!(parse(b"\x1b[7~"), vec![Key::Home]);
        assert_eq!(parse(b"\x1b[8~"), vec![Key::End]);
    }

    #[test]
    fn esc_followed_by_char() {
        // Lone ESC then a printable char: ESC is emitted, char is not lost.
        assert_eq!(parse(b"\x1bx"), vec![Key::Esc, Key::Char('x')]);
        // Double ESC: the first ESC starts a sequence, the second terminates
        // it (emitting Esc) and starts a new sequence. The trailing ESC
        // leaves the parser in a pending state (needs_more), so only one
        // Esc key is emitted in this batch.
        assert_eq!(parse(b"\x1b\x1b"), vec![Key::Esc]);
    }

    #[test]
    fn unknown_csi_ignored() {
        assert_eq!(parse(b"\x1b[25~"), vec![]);
        // Modifier-prefixed arrows: treat as plain arrows.
        assert_eq!(parse(b"\x1b[1;5C"), vec![Key::Right]);
    }
}
