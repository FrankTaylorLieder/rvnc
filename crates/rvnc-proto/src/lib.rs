//! RFB (VNC) protocol types, encoders and decoders.
//!
//! This crate performs no network I/O of its own: decoders read from any
//! [`std::io::Read`] and encoders append to a `Vec<u8>`. See `docs/protocol.md`.

pub mod client;
pub mod encoding;
mod error;
pub mod handshake;
pub mod pixel_format;
pub mod server;
mod wire;

pub use error::ProtoError;
pub use pixel_format::PixelFormat;

/// Upper bound on any framebuffer dimension we accept from a server.
pub const MAX_DIMENSION: u16 = 16384;
/// Upper bound on server-supplied strings (desktop name, failure reasons).
pub const MAX_STRING_LEN: u32 = 64 * 1024;
/// Upper bound on server cut text we will read (the rest is a protocol error).
pub const MAX_CUT_TEXT_LEN: u32 = 16 * 1024 * 1024;

/// An axis-aligned rectangle in framebuffer coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Exclusive right edge, widened so it cannot overflow.
    pub fn right(&self) -> u32 {
        u32::from(self.x) + u32::from(self.w)
    }

    /// Exclusive bottom edge, widened so it cannot overflow.
    pub fn bottom(&self) -> u32 {
        u32::from(self.y) + u32::from(self.h)
    }

    /// True if the rectangle lies entirely within a `width` x `height` area.
    pub fn fits_in(&self, width: u16, height: u16) -> bool {
        self.right() <= u32::from(width) && self.bottom() <= u32::from(height)
    }

    /// Smallest rectangle containing both. Empty rectangles are ignored.
    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = self.right().max(other.right());
        let b = self.bottom().max(other.bottom());
        // Both inputs fit in u16 space, so r - x and b - y fit in u16.
        Rect::new(x, y, (r - u32::from(x)) as u16, (b - u32::from(y)) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_union() {
        let a = Rect::new(10, 10, 5, 5);
        let b = Rect::new(0, 20, 2, 2);
        assert_eq!(a.union(&b), Rect::new(0, 10, 15, 12));
        assert_eq!(Rect::default().union(&a), a);
        assert_eq!(a.union(&Rect::default()), a);
    }

    #[test]
    fn rect_fits_without_overflow() {
        let r = Rect::new(u16::MAX, u16::MAX, u16::MAX, u16::MAX);
        assert!(!r.fits_in(u16::MAX, u16::MAX));
        assert!(Rect::new(0, 0, 10, 10).fits_in(10, 10));
        assert!(!Rect::new(1, 0, 10, 10).fits_in(10, 10));
    }
}
