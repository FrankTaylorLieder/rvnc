//! Client-to-server messages (RFC 6143 §7.5).

use crate::wire::{put_i32, put_u16, put_u32};
use crate::{PixelFormat, Rect};

/// Button-mask bits for [`ClientMessage::PointerEvent`].
pub mod buttons {
    pub const LEFT: u8 = 1 << 0;
    pub const MIDDLE: u8 = 1 << 1;
    pub const RIGHT: u8 = 1 << 2;
    pub const WHEEL_UP: u8 = 1 << 3;
    pub const WHEEL_DOWN: u8 = 1 << 4;
    pub const WHEEL_LEFT: u8 = 1 << 5;
    pub const WHEEL_RIGHT: u8 = 1 << 6;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    SetPixelFormat(PixelFormat),
    SetEncodings(Vec<i32>),
    FramebufferUpdateRequest {
        incremental: bool,
        rect: Rect,
    },
    KeyEvent {
        down: bool,
        keysym: u32,
    },
    PointerEvent {
        buttons: u8,
        x: u16,
        y: u16,
    },
    /// Latin-1 text. Longer than `u32::MAX` is truncated.
    ClientCutText(Vec<u8>),
}

impl ClientMessage {
    /// Appends the wire encoding of this message to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            ClientMessage::SetPixelFormat(pf) => {
                out.extend_from_slice(&[0, 0, 0, 0]);
                pf.encode(out);
            }
            ClientMessage::SetEncodings(encs) => {
                let n = encs.len().min(usize::from(u16::MAX));
                out.extend_from_slice(&[2, 0]);
                put_u16(out, n as u16);
                for e in &encs[..n] {
                    put_i32(out, *e);
                }
            }
            ClientMessage::FramebufferUpdateRequest { incremental, rect } => {
                out.extend_from_slice(&[3, u8::from(*incremental)]);
                put_u16(out, rect.x);
                put_u16(out, rect.y);
                put_u16(out, rect.w);
                put_u16(out, rect.h);
            }
            ClientMessage::KeyEvent { down, keysym } => {
                out.extend_from_slice(&[4, u8::from(*down), 0, 0]);
                put_u32(out, *keysym);
            }
            ClientMessage::PointerEvent { buttons, x, y } => {
                out.extend_from_slice(&[5, *buttons]);
                put_u16(out, *x);
                put_u16(out, *y);
            }
            ClientMessage::ClientCutText(text) => {
                let n = text.len().min(u32::MAX as usize);
                out.extend_from_slice(&[6, 0, 0, 0]);
                put_u32(out, n as u32);
                out.extend_from_slice(&text[..n]);
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_pixel_format() {
        let b = ClientMessage::SetPixelFormat(PixelFormat::rvnc_default()).to_bytes();
        assert_eq!(b.len(), 20);
        assert_eq!(&b[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn set_encodings() {
        let b = ClientMessage::SetEncodings(vec![1, 0, -223]).to_bytes();
        assert_eq!(
            b,
            [2, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0x21]
        );
    }

    #[test]
    fn update_request() {
        let b = ClientMessage::FramebufferUpdateRequest {
            incremental: true,
            rect: Rect::new(1, 2, 0x0300, 0x0400),
        }
        .to_bytes();
        assert_eq!(b, [3, 1, 0, 1, 0, 2, 3, 0, 4, 0]);
    }

    #[test]
    fn key_event() {
        let b = ClientMessage::KeyEvent {
            down: true,
            keysym: 0xff0d,
        }
        .to_bytes();
        assert_eq!(b, [4, 1, 0, 0, 0, 0, 0xff, 0x0d]);
    }

    #[test]
    fn pointer_event() {
        let b = ClientMessage::PointerEvent {
            buttons: buttons::LEFT | buttons::RIGHT,
            x: 0x0102,
            y: 0x0304,
        }
        .to_bytes();
        assert_eq!(b, [5, 5, 1, 2, 3, 4]);
    }

    #[test]
    fn cut_text() {
        let b = ClientMessage::ClientCutText(b"hi".to_vec()).to_bytes();
        assert_eq!(b, [6, 0, 0, 0, 0, 0, 0, 2, b'h', b'i']);
    }
}
