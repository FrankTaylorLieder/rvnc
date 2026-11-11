//! Mapping from winit keys to X11 keysyms (see `docs/input.md`).

use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

pub mod keysym {
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const CLEAR: u32 = 0xff0b;
    pub const RETURN: u32 = 0xff0d;
    pub const PAUSE: u32 = 0xff13;
    pub const SCROLL_LOCK: u32 = 0xff14;
    pub const ESCAPE: u32 = 0xff1b;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const PRINT: u32 = 0xff61;
    pub const INSERT: u32 = 0xff63;
    pub const UNDO: u32 = 0xff65;
    pub const REDO: u32 = 0xff66;
    pub const MENU: u32 = 0xff67;
    pub const HELP: u32 = 0xff6a;
    pub const NUM_LOCK: u32 = 0xff7f;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const F1: u32 = 0xffbe;
    pub const SHIFT_L: u32 = 0xffe1;
    pub const SHIFT_R: u32 = 0xffe2;
    pub const CONTROL_L: u32 = 0xffe3;
    pub const CONTROL_R: u32 = 0xffe4;
    pub const CAPS_LOCK: u32 = 0xffe5;
    pub const ALT_L: u32 = 0xffe9;
    pub const ALT_R: u32 = 0xffea;
    pub const SUPER_L: u32 = 0xffeb;
    pub const SUPER_R: u32 = 0xffec;
    pub const DELETE: u32 = 0xffff;
    pub const SPACE: u32 = 0x0020;
}

use keysym::*;

/// Keys identified by physical position: modifiers (to keep left/right) and
/// keys whose logical value loses information (keypad Enter).
pub fn physical_keysym(key: PhysicalKey) -> Option<u32> {
    let PhysicalKey::Code(code) = key else {
        return None;
    };
    Some(match code {
        KeyCode::ShiftLeft => SHIFT_L,
        KeyCode::ShiftRight => SHIFT_R,
        KeyCode::ControlLeft => CONTROL_L,
        KeyCode::ControlRight => CONTROL_R,
        // Option.
        KeyCode::AltLeft => ALT_L,
        KeyCode::AltRight => ALT_R,
        // Command.
        KeyCode::SuperLeft => SUPER_L,
        KeyCode::SuperRight => SUPER_R,
        KeyCode::CapsLock => CAPS_LOCK,
        KeyCode::NumpadEnter => KP_ENTER,
        _ => return None,
    })
}

/// Named (non-character) keys.
pub fn named_keysym(key: NamedKey) -> Option<u32> {
    use NamedKey as N;
    Some(match key {
        N::Enter => RETURN,
        N::Tab => TAB,
        N::Space => SPACE,
        N::Backspace => BACKSPACE,
        N::Delete => DELETE,
        N::Escape => ESCAPE,
        N::ArrowLeft => LEFT,
        N::ArrowUp => UP,
        N::ArrowRight => RIGHT,
        N::ArrowDown => DOWN,
        N::Home => HOME,
        N::End => END,
        N::PageUp => PAGE_UP,
        N::PageDown => PAGE_DOWN,
        N::Insert => INSERT,
        N::Clear => CLEAR,
        N::Pause => PAUSE,
        N::ScrollLock => SCROLL_LOCK,
        N::PrintScreen => PRINT,
        N::ContextMenu => MENU,
        N::Help => HELP,
        N::NumLock => NUM_LOCK,
        N::Undo => UNDO,
        N::Redo => REDO,
        N::Shift => SHIFT_L,
        N::Control => CONTROL_L,
        N::Alt => ALT_L,
        N::Super | N::Meta => SUPER_L,
        N::CapsLock => CAPS_LOCK,
        N::F1 => F1,
        N::F2 => F1 + 1,
        N::F3 => F1 + 2,
        N::F4 => F1 + 3,
        N::F5 => F1 + 4,
        N::F6 => F1 + 5,
        N::F7 => F1 + 6,
        N::F8 => F1 + 7,
        N::F9 => F1 + 8,
        N::F10 => F1 + 9,
        N::F11 => F1 + 10,
        N::F12 => F1 + 11,
        N::F13 => F1 + 12,
        N::F14 => F1 + 13,
        N::F15 => F1 + 14,
        N::F16 => F1 + 15,
        N::F17 => F1 + 16,
        N::F18 => F1 + 17,
        N::F19 => F1 + 18,
        N::F20 => F1 + 19,
        _ => return None,
    })
}

/// The keysym for a character: Latin-1 maps directly, everything else uses
/// the X11 Unicode keysym range. Control characters have no keysym.
pub fn char_keysym(c: char) -> Option<u32> {
    let cp = u32::from(c);
    match cp {
        0x20..=0x7e | 0xa0..=0xff => Some(cp),
        0..=0x1f | 0x7f..=0x9f => None,
        _ => Some(0x0100_0000 | cp),
    }
}

/// The keysyms for a logical key. Usually one; several for multi-character
/// text (e.g. a composed sequence delivered at once).
pub fn logical_keysyms(key: &Key) -> Vec<u32> {
    match key {
        Key::Named(n) => named_keysym(*n).into_iter().collect(),
        Key::Character(s) => s.chars().filter_map(char_keysym).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    #[test]
    fn chars() {
        assert_eq!(char_keysym('a'), Some(0x61));
        assert_eq!(char_keysym('@'), Some(0x40));
        assert_eq!(char_keysym('£'), Some(0xa3));
        assert_eq!(char_keysym('é'), Some(0xe9));
        assert_eq!(char_keysym('€'), Some(0x0100_20ac));
        assert_eq!(char_keysym('\u{3}'), None);
        assert_eq!(char_keysym('\u{7f}'), None);
    }

    #[test]
    fn named() {
        assert_eq!(named_keysym(NamedKey::Enter), Some(0xff0d));
        assert_eq!(named_keysym(NamedKey::F12), Some(0xffc9));
        assert_eq!(named_keysym(NamedKey::Space), Some(0x20));
        assert_eq!(named_keysym(NamedKey::Delete), Some(0xffff));
    }

    #[test]
    fn modifiers_by_position() {
        let p = |c| physical_keysym(PhysicalKey::Code(c));
        assert_eq!(p(KeyCode::ShiftRight), Some(SHIFT_R));
        assert_eq!(p(KeyCode::AltLeft), Some(ALT_L));
        assert_eq!(p(KeyCode::SuperLeft), Some(SUPER_L));
        assert_eq!(p(KeyCode::KeyA), None);
    }

    #[test]
    fn logical() {
        assert_eq!(
            logical_keysyms(&Key::Character(SmolStr::new("ab"))),
            [0x61, 0x62]
        );
        assert_eq!(logical_keysyms(&Key::Named(NamedKey::Tab)), [TAB]);
        assert!(logical_keysyms(&Key::Dead(Some('´'))).is_empty());
    }
}
