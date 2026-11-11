//! Keyboard and pointer state machines. Pure logic, no I/O: each method
//! returns the RFB events to send. See `docs/input.md`.

use std::collections::{HashMap, HashSet};

use rvnc_client::proto::client::buttons;
use winit::event::MouseButton;
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

use crate::keymap::{self, keysym};

/// Commands handled locally rather than sent to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    ToggleScale,
    ToggleFullscreen,
    SendCtrlAltDel,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyOutput {
    /// `(keysym, down)` pairs to send in order.
    pub events: Vec<(u32, bool)>,
    pub command: Option<LocalCommand>,
}

/// Tracks pressed keys so every press gets a matching release.
#[derive(Debug, Default)]
pub struct Keyboard {
    pressed: HashMap<PhysicalKey, u32>,
    /// Physical keys whose press triggered a local command; their releases
    /// are swallowed.
    consumed: HashSet<PhysicalKey>,
    mods: ModifiersState,
}

impl Keyboard {
    pub fn set_modifiers(&mut self, mods: ModifiersState) {
        self.mods = mods;
    }

    /// Handles one key event.
    ///
    /// `logical` is the key with all modifiers applied; `unmodified` is the
    /// same key ignoring modifiers (used when Ctrl or Cmd is held, since those
    /// combinations are shortcuts rather than text).
    pub fn key(
        &mut self,
        physical: PhysicalKey,
        logical: &Key,
        unmodified: &Key,
        pressed: bool,
    ) -> KeyOutput {
        let mut out = KeyOutput::default();

        if physical == PhysicalKey::Code(KeyCode::CapsLock) {
            // macOS reports Caps Lock as a toggle: press when it turns on,
            // release when it turns off. Each change is one tap on the remote.
            out.events = vec![(keysym::CAPS_LOCK, true), (keysym::CAPS_LOCK, false)];
            return out;
        }

        if !pressed {
            if self.consumed.remove(&physical) {
                return out;
            }
            if let Some(sym) = self.pressed.remove(&physical) {
                out.events.push((sym, false));
            }
            return out;
        }

        // Auto-repeat (or a duplicate press): repeat the original keysym so
        // the eventual release matches.
        if let Some(&sym) = self.pressed.get(&physical) {
            out.events.push((sym, true));
            return out;
        }
        if self.consumed.contains(&physical) {
            return out;
        }

        if let Some(cmd) = self.local_command(physical) {
            self.consumed.insert(physical);
            out.command = Some(cmd);
            return out;
        }

        let syms = match keymap::physical_keysym(physical) {
            Some(sym) => vec![sym],
            None => {
                let key = if self.mods.control_key() || self.mods.super_key() {
                    unmodified
                } else {
                    logical
                };
                keymap::logical_keysyms(key)
            }
        };

        match syms.as_slice() {
            [] => tracing::debug!(?physical, ?logical, "unmapped key"),
            [sym] => {
                self.pressed.insert(physical, *sym);
                out.events.push((*sym, true));
            }
            many => {
                // Multi-character text: type it all now; nothing to release.
                for sym in many {
                    out.events.push((*sym, true));
                    out.events.push((*sym, false));
                }
            }
        }
        out
    }

    fn local_command(&self, physical: PhysicalKey) -> Option<LocalCommand> {
        if !(self.mods.super_key() && self.mods.alt_key()) {
            return None;
        }
        match physical {
            PhysicalKey::Code(KeyCode::KeyS) => Some(LocalCommand::ToggleScale),
            PhysicalKey::Code(KeyCode::KeyF) => Some(LocalCommand::ToggleFullscreen),
            // The Mac "delete" key is physically Backspace.
            PhysicalKey::Code(KeyCode::Backspace | KeyCode::Delete) => {
                Some(LocalCommand::SendCtrlAltDel)
            }
            _ => None,
        }
    }

    /// Releases every key the remote thinks is down (e.g. on focus loss).
    pub fn release_all(&mut self) -> Vec<(u32, bool)> {
        self.consumed.clear();
        let mut syms: Vec<u32> = self.pressed.drain().map(|(_, s)| s).collect();
        // Release ordinary keys before modifiers.
        syms.sort_by_key(|s| is_modifier(*s));
        syms.into_iter().map(|s| (s, false)).collect()
    }
}

fn is_modifier(sym: u32) -> bool {
    (keysym::SHIFT_L..=keysym::SUPER_R).contains(&sym)
}

/// The Ctrl-Alt-Del sequence.
pub fn ctrl_alt_del() -> Vec<(u32, bool)> {
    vec![
        (keysym::CONTROL_L, true),
        (keysym::ALT_L, true),
        (keysym::DELETE, true),
        (keysym::DELETE, false),
        (keysym::ALT_L, false),
        (keysym::CONTROL_L, false),
    ]
}

/// A pointer event: `(x, y, button_mask)`.
pub type PointerEvent = (u16, u16, u8);

/// Pointer position, button mask and scroll accumulation.
#[derive(Debug, Default)]
pub struct Pointer {
    x: u16,
    y: u16,
    mask: u8,
    /// Fractional scroll, in wheel clicks, per axis.
    acc_x: f64,
    acc_y: f64,
}

/// Most wheel clicks generated by a single scroll event.
const MAX_CLICKS_PER_EVENT: i32 = 10;

impl Pointer {
    pub fn moved(&mut self, x: u16, y: u16) -> Option<PointerEvent> {
        if (x, y) == (self.x, self.y) {
            return None;
        }
        self.x = x;
        self.y = y;
        Some((x, y, self.mask))
    }

    pub fn button(&mut self, button: MouseButton, pressed: bool) -> Option<PointerEvent> {
        let bit = match button {
            MouseButton::Left => buttons::LEFT,
            MouseButton::Middle => buttons::MIDDLE,
            MouseButton::Right => buttons::RIGHT,
            _ => return None,
        };
        let mask = if pressed {
            self.mask | bit
        } else {
            self.mask & !bit
        };
        if mask == self.mask {
            return None;
        }
        self.mask = mask;
        Some((self.x, self.y, mask))
    }

    /// Scrolls by a (possibly fractional) number of wheel clicks. Positive
    /// `dy` scrolls up and positive `dx` scrolls left, matching winit's
    /// "content moves right/down" convention.
    pub fn scroll(&mut self, dx: f64, dy: f64) -> Vec<PointerEvent> {
        let mut out = Vec::new();
        for (delta, acc, pos_bit, neg_bit) in [
            (dy, &mut self.acc_y, buttons::WHEEL_UP, buttons::WHEEL_DOWN),
            (
                dx,
                &mut self.acc_x,
                buttons::WHEEL_LEFT,
                buttons::WHEEL_RIGHT,
            ),
        ] {
            if delta == 0.0 {
                continue;
            }
            // Reverse immediately on a direction change.
            if acc.signum() != delta.signum() {
                *acc = 0.0;
            }
            *acc += delta;
            let clicks = (acc.trunc() as i32).clamp(-MAX_CLICKS_PER_EVENT, MAX_CLICKS_PER_EVENT);
            *acc -= f64::from(clicks);
            let bit = if clicks > 0 { pos_bit } else { neg_bit };
            for _ in 0..clicks.abs() {
                out.push((self.x, self.y, self.mask | bit));
                out.push((self.x, self.y, self.mask));
            }
        }
        out
    }

    /// Releases all buttons (e.g. on focus loss).
    pub fn release_all(&mut self) -> Option<PointerEvent> {
        self.acc_x = 0.0;
        self.acc_y = 0.0;
        if self.mask == 0 {
            return None;
        }
        self.mask = 0;
        Some((self.x, self.y, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{NamedKey, SmolStr};

    fn ch(s: &str) -> Key {
        Key::Character(SmolStr::new(s))
    }
    fn code(c: KeyCode) -> PhysicalKey {
        PhysicalKey::Code(c)
    }

    #[test]
    fn shifted_character_release_matches_press() {
        let mut k = Keyboard::default();
        let shift = Key::Named(NamedKey::Shift);
        assert_eq!(
            k.key(code(KeyCode::ShiftLeft), &shift, &shift, true).events,
            [(keysym::SHIFT_L, true)]
        );
        k.set_modifiers(ModifiersState::SHIFT);
        assert_eq!(
            k.key(code(KeyCode::Digit2), &ch("@"), &ch("2"), true)
                .events,
            [(0x40, true)]
        );
        k.key(code(KeyCode::ShiftLeft), &shift, &shift, false);
        k.set_modifiers(ModifiersState::empty());
        // Release now reports "2", but we must release "@".
        assert_eq!(
            k.key(code(KeyCode::Digit2), &ch("2"), &ch("2"), false)
                .events,
            [(0x40, false)]
        );
    }

    #[test]
    fn repeat_resends_same_keysym() {
        let mut k = Keyboard::default();
        k.key(code(KeyCode::KeyA), &ch("a"), &ch("a"), true);
        assert_eq!(
            k.key(code(KeyCode::KeyA), &ch("a"), &ch("a"), true).events,
            [(0x61, true)]
        );
    }

    #[test]
    fn ctrl_uses_unmodified_key() {
        let mut k = Keyboard::default();
        k.set_modifiers(ModifiersState::CONTROL);
        assert_eq!(
            k.key(code(KeyCode::KeyC), &ch("\u{3}"), &ch("c"), true)
                .events,
            [(0x63, true)]
        );
        k.set_modifiers(ModifiersState::SUPER);
        assert_eq!(
            k.key(code(KeyCode::KeyV), &ch("v"), &ch("v"), true).events,
            [(0x76, true)]
        );
    }

    #[test]
    fn option_uses_produced_character() {
        let mut k = Keyboard::default();
        k.set_modifiers(ModifiersState::ALT);
        assert_eq!(
            k.key(code(KeyCode::Digit3), &ch("#"), &ch("3"), true)
                .events,
            [(0x23, true)]
        );
    }

    #[test]
    fn release_all_on_focus_loss() {
        let mut k = Keyboard::default();
        let sup = Key::Named(NamedKey::Super);
        k.key(code(KeyCode::SuperLeft), &sup, &sup, true);
        k.key(code(KeyCode::KeyA), &ch("a"), &ch("a"), true);
        assert_eq!(k.release_all(), [(0x61, false), (keysym::SUPER_L, false)]);
        assert!(k.release_all().is_empty());
        // A stray release afterwards sends nothing.
        assert!(
            k.key(code(KeyCode::KeyA), &ch("a"), &ch("a"), false)
                .events
                .is_empty()
        );
    }

    #[test]
    fn local_shortcuts_are_consumed() {
        let mut k = Keyboard::default();
        k.set_modifiers(ModifiersState::SUPER | ModifiersState::ALT);
        let out = k.key(code(KeyCode::KeyS), &ch("ß"), &ch("s"), true);
        assert_eq!(out.command, Some(LocalCommand::ToggleScale));
        assert!(out.events.is_empty());
        // Repeat and release are swallowed too.
        assert_eq!(
            k.key(code(KeyCode::KeyS), &ch("ß"), &ch("s"), true),
            KeyOutput::default()
        );
        assert_eq!(
            k.key(code(KeyCode::KeyS), &ch("s"), &ch("s"), false),
            KeyOutput::default()
        );
        let out = k.key(
            code(KeyCode::Backspace),
            &Key::Named(NamedKey::Backspace),
            &Key::Named(NamedKey::Backspace),
            true,
        );
        assert_eq!(out.command, Some(LocalCommand::SendCtrlAltDel));
    }

    #[test]
    fn caps_lock_toggles() {
        let mut k = Keyboard::default();
        let caps = Key::Named(NamedKey::CapsLock);
        let tap = [(keysym::CAPS_LOCK, true), (keysym::CAPS_LOCK, false)];
        assert_eq!(
            k.key(code(KeyCode::CapsLock), &caps, &caps, true).events,
            tap
        );
        assert_eq!(
            k.key(code(KeyCode::CapsLock), &caps, &caps, false).events,
            tap
        );
        assert!(k.release_all().is_empty());
    }

    #[test]
    fn dead_keys_are_ignored() {
        let mut k = Keyboard::default();
        let dead = Key::Dead(Some('´'));
        assert!(
            k.key(code(KeyCode::KeyE), &dead, &ch("e"), true)
                .events
                .is_empty()
        );
        assert!(
            k.key(code(KeyCode::KeyE), &dead, &ch("e"), false)
                .events
                .is_empty()
        );
    }

    #[test]
    fn pointer_buttons_and_motion() {
        let mut p = Pointer::default();
        assert_eq!(p.moved(10, 20), Some((10, 20, 0)));
        assert_eq!(p.moved(10, 20), None);
        assert_eq!(p.button(MouseButton::Left, true), Some((10, 20, 1)));
        assert_eq!(p.button(MouseButton::Right, true), Some((10, 20, 5)));
        assert_eq!(p.moved(11, 20), Some((11, 20, 5)));
        assert_eq!(p.button(MouseButton::Left, false), Some((11, 20, 4)));
        assert_eq!(p.button(MouseButton::Back, true), None);
        assert_eq!(p.release_all(), Some((11, 20, 0)));
        assert_eq!(p.release_all(), None);
    }

    #[test]
    fn scroll_accumulates() {
        let mut p = Pointer::default();
        assert!(p.scroll(0.0, 0.4).is_empty());
        assert!(p.scroll(0.0, 0.4).is_empty());
        let up = buttons::WHEEL_UP;
        assert_eq!(p.scroll(0.0, 0.4), [(0, 0, up), (0, 0, 0)]);
        // Direction change resets the remainder.
        let down = buttons::WHEEL_DOWN;
        assert_eq!(p.scroll(0.0, -1.0), [(0, 0, down), (0, 0, 0)]);
        assert_eq!(p.scroll(-2.0, 0.0).len(), 4);
        assert_eq!(
            p.scroll(0.0, 1000.0).len(),
            2 * MAX_CLICKS_PER_EVENT as usize
        );
    }

    #[test]
    fn scroll_keeps_held_buttons() {
        let mut p = Pointer::default();
        p.button(MouseButton::Left, true);
        assert_eq!(
            p.scroll(0.0, 1.0),
            [(0, 0, 1 | buttons::WHEEL_UP), (0, 0, 1)]
        );
    }
}
