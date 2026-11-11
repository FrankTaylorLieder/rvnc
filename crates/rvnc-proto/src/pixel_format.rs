//! The 16-byte PIXEL_FORMAT structure (RFC 6143 §7.4).

use std::io::Read;

use crate::ProtoError;
use crate::wire::{put_u16, read_u8, read_u16};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub big_endian: bool,
    pub true_colour: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    pub const WIRE_LEN: usize = 16;

    /// The format rvnc always requests: 32 bpp little-endian, so each pixel
    /// arrives as bytes `B G R X`, which as a little-endian `u32` is
    /// `0x00RRGGBB`.
    pub const fn rvnc_default() -> Self {
        Self {
            bits_per_pixel: 32,
            depth: 24,
            big_endian: false,
            true_colour: true,
            red_max: 255,
            green_max: 255,
            blue_max: 255,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
        }
    }

    pub fn bytes_per_pixel(&self) -> usize {
        usize::from(self.bits_per_pixel / 8)
    }

    pub fn read(r: &mut impl Read) -> Result<Self, ProtoError> {
        let bits_per_pixel = read_u8(r)?;
        let depth = read_u8(r)?;
        let big_endian = read_u8(r)? != 0;
        let true_colour = read_u8(r)? != 0;
        let red_max = read_u16(r)?;
        let green_max = read_u16(r)?;
        let blue_max = read_u16(r)?;
        let red_shift = read_u8(r)?;
        let green_shift = read_u8(r)?;
        let blue_shift = read_u8(r)?;
        let mut pad = [0u8; 3];
        r.read_exact(&mut pad)?;
        Ok(Self {
            bits_per_pixel,
            depth,
            big_endian,
            true_colour,
            red_max,
            green_max,
            blue_max,
            red_shift,
            green_shift,
            blue_shift,
        })
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.bits_per_pixel);
        out.push(self.depth);
        out.push(u8::from(self.big_endian));
        out.push(u8::from(self.true_colour));
        put_u16(out, self.red_max);
        put_u16(out, self.green_max);
        put_u16(out, self.blue_max);
        out.push(self.red_shift);
        out.push(self.green_shift);
        out.push(self.blue_shift);
        out.extend_from_slice(&[0, 0, 0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let pf = PixelFormat::rvnc_default();
        let mut buf = Vec::new();
        pf.encode(&mut buf);
        assert_eq!(buf.len(), PixelFormat::WIRE_LEN);
        assert_eq!(
            buf,
            [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]
        );
        assert_eq!(PixelFormat::read(&mut buf.as_slice()).unwrap(), pf);
    }

    #[test]
    fn truncated_is_error() {
        let buf = [32u8, 24, 0];
        assert!(PixelFormat::read(&mut buf.as_slice()).unwrap_err().is_eof());
    }
}
