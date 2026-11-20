//! ZRLE encoding (RFC 6143 §7.7.6).
//!
//! A rectangle is a `u32` length followed by that many bytes of zlib data.
//! One zlib stream spans the whole connection, so the inflater lives as long
//! as the [`crate::server::Decoder`]. Servers end each rectangle's data with
//! a sync flush, so each rectangle inflates completely by itself.
//!
//! The inflated data is 64x64 tiles, left to right, top to bottom, each
//! starting with a subencoding byte:
//!
//! | Subencoding | Tile data |
//! |---|---|
//! | 0 | Raw: `w*h` CPIXELs |
//! | 1 | Solid: one CPIXEL |
//! | 2–16 | Packed palette: that many CPIXELs, then 1-, 2- or 4-bit indices, each row padded to a byte |
//! | 128 | Plain RLE: (CPIXEL, run length) pairs |
//! | 130–255 | Palette RLE: `sub - 128` CPIXELs, then index bytes; if the top bit is set, a run length follows |
//!
//! Run lengths are a series of bytes whose sum plus one is the length; every
//! byte but the last is 255.
//!
//! A CPIXEL is a pixel with its unused byte dropped. With rvnc's pixel
//! format (32 bpp, depth 24, little-endian, colours in the low 3 bytes) that
//! is 3 bytes: `B G R`.

use std::io::Read;

use flate2::{Decompress, FlushDecompress, Status};

use crate::wire::read_u32;
use crate::{ProtoError, Rect};

const TILE: usize = 64;
const BYTES_PER_PIXEL: usize = 4;
/// Inflate the compressed data in pieces this big.
const READ_CHUNK: usize = 64 * 1024;

fn bad(reason: &'static str) -> ProtoError {
    ProtoError::BadData {
        encoding: "ZRLE",
        reason,
    }
}

/// ZRLE state that persists across rectangles.
pub(crate) struct ZrleDecoder {
    inflater: Decompress,
    compressed: Vec<u8>,
    inflated: Vec<u8>,
}

impl std::fmt::Debug for ZrleDecoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZrleDecoder")
            .field("total_in", &self.inflater.total_in())
            .field("total_out", &self.inflater.total_out())
            .finish()
    }
}

impl Default for ZrleDecoder {
    fn default() -> Self {
        Self {
            inflater: Decompress::new(true),
            compressed: Vec::new(),
            inflated: Vec::new(),
        }
    }
}

/// The most inflated data a valid `w` x `h` rectangle can have. Plain RLE
/// with runs of one pixel is the worst case: 4 bytes per pixel, plus a
/// subencoding byte and a 127-colour palette per tile.
fn max_inflated(w: usize, h: usize) -> usize {
    let tiles = w.div_ceil(TILE) * h.div_ceil(TILE);
    w * h * 4 + tiles * (1 + 127 * 3)
}

impl ZrleDecoder {
    /// Reads one ZRLE rectangle and decodes it into `out` as `B G R X`
    /// pixels, `rect.w * rect.h * 4` bytes.
    pub(crate) fn read_rect(
        &mut self,
        r: &mut impl Read,
        rect: Rect,
        out: &mut Vec<u8>,
    ) -> Result<(), ProtoError> {
        let (w, h) = (usize::from(rect.w), usize::from(rect.h));
        let len = read_u32(r)? as usize;
        let limit = max_inflated(w, h);
        // zlib's worst-case expansion is about 0.03%, plus headers.
        if len > limit + limit / 64 + 1024 {
            return Err(ProtoError::TooLong {
                what: "ZRLE rectangle",
                len: len as u64,
                limit: (limit + limit / 64 + 1024) as u64,
            });
        }
        self.inflate(r, len, limit)?;

        out.clear();
        out.resize(w * h * BYTES_PER_PIXEL, 0);
        if w > 0 && h > 0 {
            decode_tiles(&self.inflated, w, h, out)?;
        }
        Ok(())
    }

    /// Reads `len` compressed bytes and inflates them into `self.inflated`,
    /// failing if that would exceed `limit` bytes.
    fn inflate(&mut self, r: &mut impl Read, len: usize, limit: usize) -> Result<(), ProtoError> {
        self.inflated.clear();
        let mut remaining = len;
        while remaining > 0 {
            let n = remaining.min(READ_CHUNK);
            self.compressed.resize(n, 0);
            r.read_exact(&mut self.compressed)?;
            remaining -= n;

            let mut input = self.compressed.as_slice();
            loop {
                if self.inflated.len() == self.inflated.capacity() {
                    if self.inflated.len() > limit {
                        return Err(bad("rectangle inflates to more data than it can hold"));
                    }
                    let want = (limit + 1 - self.inflated.len()).min(READ_CHUNK * 4);
                    self.inflated.reserve(want);
                }
                let (in0, out0) = (self.inflater.total_in(), self.inflater.total_out());
                let status = self
                    .inflater
                    .decompress_vec(input, &mut self.inflated, FlushDecompress::Sync)
                    .map_err(|_| bad("corrupt zlib data"))?;
                let consumed = (self.inflater.total_in() - in0) as usize;
                let produced = self.inflater.total_out() - out0;
                input = &input[consumed..];
                // Done with this chunk once all input is used and the output
                // wasn't cut short by a full buffer.
                if input.is_empty() && self.inflated.len() < self.inflated.capacity() {
                    break;
                }
                if consumed == 0 && produced == 0 {
                    if status == Status::StreamEnd {
                        return Err(bad("zlib stream ended"));
                    }
                    if self.inflated.len() < self.inflated.capacity() {
                        return Err(bad("zlib stream stalled"));
                    }
                }
            }
        }
        if self.inflated.len() > limit {
            return Err(bad("rectangle inflates to more data than it can hold"));
        }
        // Don't keep a huge buffer after one large update.
        if self.inflated.capacity() > 64 * 1024 * 1024 {
            self.inflated.shrink_to(READ_CHUNK);
        }
        Ok(())
    }
}

/// Reads inflated tile data.
struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn u8(&mut self) -> Result<u8, ProtoError> {
        let (&b, rest) = self.0.split_first().ok_or(bad("tile data ended early"))?;
        self.0 = rest;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&[u8], ProtoError> {
        if self.0.len() < n {
            return Err(bad("tile data ended early"));
        }
        let (a, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(a)
    }

    /// A CPIXEL as an output pixel.
    fn cpixel(&mut self) -> Result<[u8; 4], ProtoError> {
        let p = self.take(3)?;
        Ok([p[0], p[1], p[2], 0])
    }

    fn palette(&mut self, n: usize, palette: &mut [[u8; 4]; 128]) -> Result<(), ProtoError> {
        for p in palette.iter_mut().take(n) {
            *p = self.cpixel()?;
        }
        Ok(())
    }

    /// A run length, which must not exceed `max`.
    fn run_len(&mut self, max: usize) -> Result<usize, ProtoError> {
        let mut len = 1usize;
        loop {
            let b = self.u8()?;
            len += usize::from(b);
            if len > max {
                return Err(bad("run extends past the end of the tile"));
            }
            if b != 255 {
                return Ok(len);
            }
        }
    }
}

/// One decoded tile, row-major with no padding.
struct Tile {
    px: Box<[[u8; 4]; TILE * TILE]>,
    w: usize,
    h: usize,
}

impl Tile {
    /// Copies the tile into the output rectangle at `(x, y)`.
    fn blit(&self, out: &mut [u8], stride: usize, x: usize, y: usize) {
        for (row, src) in self.px[..self.w * self.h].chunks_exact(self.w).enumerate() {
            let i = (y + row) * stride + x * BYTES_PER_PIXEL;
            out[i..i + self.w * BYTES_PER_PIXEL].copy_from_slice(src.as_flattened());
        }
    }
}

fn decode_tiles(data: &[u8], w: usize, h: usize, out: &mut [u8]) -> Result<(), ProtoError> {
    let mut c = Cursor(data);
    let mut palette = [[0u8; 4]; 128];
    let mut tile = Tile {
        px: Box::new([[0u8; 4]; TILE * TILE]),
        w: 0,
        h: 0,
    };
    let stride = w * BYTES_PER_PIXEL;
    for y in (0..h).step_by(TILE) {
        for x in (0..w).step_by(TILE) {
            tile.w = TILE.min(w - x);
            tile.h = TILE.min(h - y);
            decode_tile(&mut c, &mut tile, &mut palette)?;
            tile.blit(out, stride, x, y);
        }
    }
    Ok(())
}

fn decode_tile(
    c: &mut Cursor<'_>,
    t: &mut Tile,
    palette: &mut [[u8; 4]; 128],
) -> Result<(), ProtoError> {
    let total = t.w * t.h;
    let px = &mut t.px[..total];
    match c.u8()? {
        0 => {
            let src = c.take(total * 3)?;
            for (p, s) in px.iter_mut().zip(src.chunks_exact(3)) {
                *p = [s[0], s[1], s[2], 0];
            }
        }
        1 => px.fill(c.cpixel()?),
        n @ 2..=16 => {
            let n = usize::from(n);
            c.palette(n, palette)?;
            let bits = match n {
                2 => 1,
                3..=4 => 2,
                _ => 4,
            };
            let row_bytes = (t.w * bits).div_ceil(8);
            let mask = (1u8 << bits) - 1;
            for dst in px.chunks_exact_mut(t.w) {
                let row = c.take(row_bytes)?;
                for (tx, p) in dst.iter_mut().enumerate() {
                    let bit = tx * bits;
                    let shift = 8 - bits - bit % 8;
                    let i = usize::from((row[bit / 8] >> shift) & mask);
                    if i >= n {
                        return Err(bad("palette index out of range"));
                    }
                    *p = palette[i];
                }
            }
        }
        128 => {
            let mut i = 0;
            while i < total {
                let colour = c.cpixel()?;
                let len = c.run_len(total - i)?;
                px[i..i + len].fill(colour);
                i += len;
            }
        }
        sub @ 130..=255 => {
            let n = usize::from(sub - 128);
            c.palette(n, palette)?;
            let mut i = 0;
            while i < total {
                let b = c.u8()?;
                let idx = usize::from(b & 0x7f);
                if idx >= n {
                    return Err(bad("palette index out of range"));
                }
                let len = if b & 0x80 != 0 {
                    c.run_len(total - i)?
                } else {
                    1
                };
                px[i..i + len].fill(palette[idx]);
                i += len;
            }
        }
        _ => return Err(bad("invalid tile subencoding")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    use super::*;

    /// Builds tile data and compresses it like a server: one zlib stream,
    /// sync-flushed at the end of each rectangle.
    struct Server {
        z: ZlibEncoder<Vec<u8>>,
    }

    impl Server {
        fn new() -> Self {
            Self {
                z: ZlibEncoder::new(Vec::new(), Compression::default()),
            }
        }

        /// Returns the rectangle payload: length then compressed bytes.
        fn rect(&mut self, tiles: &[u8]) -> Vec<u8> {
            self.z.write_all(tiles).unwrap();
            self.z.flush().unwrap();
            let data = std::mem::take(self.z.get_mut());
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(&data);
            out
        }
    }

    const R: [u8; 3] = [0, 0, 255];
    const G: [u8; 3] = [0, 255, 0];
    const B: [u8; 3] = [255, 0, 0];

    fn px(c: [u8; 3]) -> [u8; 4] {
        [c[0], c[1], c[2], 0]
    }

    fn decode(d: &mut ZrleDecoder, payload: &[u8], w: u16, h: u16) -> Vec<[u8; 4]> {
        let mut out = Vec::new();
        let mut r = payload;
        d.read_rect(&mut r, Rect::new(0, 0, w, h), &mut out)
            .unwrap();
        assert!(r.is_empty(), "payload not fully consumed");
        out.chunks_exact(4).map(|p| p.try_into().unwrap()).collect()
    }

    fn try_decode(tiles: &[u8], w: u16, h: u16) -> Result<(), ProtoError> {
        let payload = Server::new().rect(tiles);
        let mut out = Vec::new();
        ZrleDecoder::default().read_rect(&mut payload.as_slice(), Rect::new(0, 0, w, h), &mut out)
    }

    #[test]
    fn raw_and_solid() {
        let mut s = Server::new();
        let mut d = ZrleDecoder::default();
        let mut t = vec![0];
        for c in [R, G, B, R] {
            t.extend_from_slice(&c);
        }
        assert_eq!(
            decode(&mut d, &s.rect(&t), 2, 2),
            [px(R), px(G), px(B), px(R)]
        );

        // The zlib stream continues into the next rectangle.
        let mut t = vec![1];
        t.extend_from_slice(&G);
        assert_eq!(decode(&mut d, &s.rect(&t), 3, 1), [px(G); 3]);
    }

    #[test]
    fn packed_palettes() {
        // 2 colours, 1 bit: 3x2 tile, rows padded to a byte.
        let mut t = vec![2];
        t.extend_from_slice(&R);
        t.extend_from_slice(&G);
        t.extend_from_slice(&[0b0100_0000, 0b1010_0000]);
        let got = decode(&mut ZrleDecoder::default(), &Server::new().rect(&t), 3, 2);
        assert_eq!(got, [px(R), px(G), px(R), px(G), px(R), px(G)]);

        // 3 colours, 2 bits.
        let mut t = vec![3];
        for c in [R, G, B] {
            t.extend_from_slice(&c);
        }
        t.push(0b10_01_00_10);
        let got = decode(&mut ZrleDecoder::default(), &Server::new().rect(&t), 4, 1);
        assert_eq!(got, [px(B), px(G), px(R), px(B)]);

        // 5 colours, 4 bits, odd width.
        let mut t = vec![5];
        for c in [R, G, B, R, [1, 2, 3]] {
            t.extend_from_slice(&c);
        }
        t.extend_from_slice(&[0x41, 0x20]);
        let got = decode(&mut ZrleDecoder::default(), &Server::new().rect(&t), 3, 1);
        assert_eq!(got, [px([1, 2, 3]), px(G), px(B)]);
    }

    #[test]
    fn plain_rle() {
        // 300 pixels of red (255 + 44 = 299, +1), then 4 of green, over a
        // 64x... single-tile 76x4 rectangle.
        let mut t = vec![128];
        t.extend_from_slice(&R);
        t.extend_from_slice(&[255, 44]);
        t.extend_from_slice(&G);
        t.push(3);
        let got = decode(&mut ZrleDecoder::default(), &Server::new().rect(&t), 38, 8);
        assert_eq!(got.len(), 304);
        assert!(got[..300].iter().all(|&p| p == px(R)));
        assert!(got[300..].iter().all(|&p| p == px(G)));
    }

    #[test]
    fn palette_rle() {
        let mut t = vec![130];
        t.extend_from_slice(&R);
        t.extend_from_slice(&G);
        // G once, R run of 3, G once.
        t.extend_from_slice(&[1, 0x80, 2, 1]);
        let got = decode(&mut ZrleDecoder::default(), &Server::new().rect(&t), 5, 1);
        assert_eq!(got, [px(G), px(R), px(R), px(R), px(G)]);
    }

    #[test]
    fn multiple_tiles_in_order() {
        // 65x65: tiles of 64x64, 1x64, 64x1, 1x1, each solid.
        let mut t = Vec::new();
        for c in [R, G, B, [9, 9, 9]] {
            t.push(1);
            t.extend_from_slice(&c);
        }
        let got = decode(&mut ZrleDecoder::default(), &Server::new().rect(&t), 65, 65);
        assert_eq!(got[0], px(R));
        assert_eq!(got[64], px(G));
        assert_eq!(got[64 * 65], px(B));
        assert_eq!(got[65 * 65 - 1], px([9, 9, 9]));
        assert_eq!(got[63 * 65 + 63], px(R));
    }

    #[test]
    fn corrupt_tiles_are_errors() {
        let cases: &[&[u8]] = &[
            &[17],                                          // unused subencoding
            &[129],                                         // unused subencoding
            &[0, 1, 2],                                     // raw tile too short
            &[2, 1, 1, 1, 2, 2, 2],                         // packed palette missing indices
            &[3, 1, 1, 1, 2, 2, 2, 3, 3, 3, 0b11_00_00_00], // index 3 of 3 colours
            &[128, 1, 2, 3, 5],                             // run of 6 in a 4-pixel tile
            &[128, 1, 2, 3, 255, 255],                      // run that never ends
            &[130, 1, 1, 1, 2, 2, 2, 5],                    // palette index out of range
        ];
        for (i, t) in cases.iter().enumerate() {
            assert!(
                matches!(try_decode(t, 4, 1), Err(ProtoError::BadData { .. })),
                "case {i}"
            );
        }
    }

    #[test]
    fn corrupt_zlib_is_an_error() {
        let payload = [0, 0, 0, 4, 1, 2, 3, 4];
        let mut out = Vec::new();
        let err = ZrleDecoder::default()
            .read_rect(&mut payload.as_slice(), Rect::new(0, 0, 1, 1), &mut out)
            .unwrap_err();
        assert!(matches!(err, ProtoError::BadData { .. }), "{err}");
    }

    #[test]
    fn oversized_inflation_is_rejected() {
        // A 1x1 rectangle whose data inflates to 100 KB of zeros.
        let err = try_decode(&vec![0u8; 100_000], 1, 1).unwrap_err();
        assert!(matches!(err, ProtoError::BadData { .. }), "{err}");
    }

    #[test]
    fn huge_length_is_rejected_before_reading() {
        let payload = [0xff, 0xff, 0xff, 0xff];
        let mut out = Vec::new();
        let err = ZrleDecoder::default()
            .read_rect(&mut payload.as_slice(), Rect::new(0, 0, 4, 4), &mut out)
            .unwrap_err();
        assert!(matches!(err, ProtoError::TooLong { .. }), "{err}");
    }

    /// `cargo test --release -p rvnc-proto -- --ignored --nocapture zrle_bench`
    #[test]
    #[ignore]
    fn zrle_bench() {
        let (w, h) = (1920usize, 1080usize);
        for (name, sub) in [("raw tiles (noise)", 0u8), ("plain RLE (text-like)", 128)] {
            let mut t = Vec::new();
            let mut seed = 7u32;
            for ty in (0..h).step_by(TILE) {
                for tx in (0..w).step_by(TILE) {
                    let n = TILE.min(w - tx) * TILE.min(h - ty);
                    t.push(sub);
                    for _ in 0..n {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        t.extend_from_slice(&seed.to_le_bytes()[..3]);
                        if sub == 128 {
                            t.push(0);
                        }
                    }
                }
            }
            let payload = Server::new().rect(&t);
            let mut d = ZrleDecoder::default();
            let mut out = Vec::new();
            let start = std::time::Instant::now();
            let mut r = &payload[4..];
            d.inflate(&mut r, payload.len() - 4, max_inflated(w, h))
                .unwrap();
            let inflated = start.elapsed();
            out.resize(w * h * BYTES_PER_PIXEL, 0);
            decode_tiles(&d.inflated, w, h, &mut out).unwrap();
            println!(
                "{name}: 1920x1080 in {:?} (inflate {inflated:?}, {} KiB compressed)",
                start.elapsed(),
                payload.len() / 1024
            );
        }
    }

    #[test]
    fn large_rect_spans_read_chunks() {
        // Raw tiles of noise for 300x300 don't compress well, so the
        // compressed data is bigger than one read chunk.
        let (w, h) = (300usize, 300usize);
        let mut seed = 1u32;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        };
        let mut t = Vec::new();
        let mut expect = vec![[0u8; 4]; w * h];
        for ty in (0..h).step_by(TILE) {
            for tx in (0..w).step_by(TILE) {
                t.push(0);
                for y in ty..(ty + TILE).min(h) {
                    for x in tx..(tx + TILE).min(w) {
                        let c = [noise(), noise(), noise()];
                        t.extend_from_slice(&c);
                        expect[y * w + x] = px(c);
                    }
                }
            }
        }
        let payload = Server::new().rect(&t);
        assert!(payload.len() > READ_CHUNK);
        let got = decode(&mut ZrleDecoder::default(), &payload, w as u16, h as u16);
        assert!(got == expect);
    }
}
