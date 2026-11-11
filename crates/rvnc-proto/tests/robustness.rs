//! Randomised robustness tests: the decoders must never panic, hang or
//! allocate unboundedly on arbitrary input. A cheap stand-in for the
//! cargo-fuzz target in `fuzz/`, run on every `cargo test`.

use rvnc_proto::handshake::{ServerInit, Version, read_security_offer};
use rvnc_proto::server::{Decoder, RectSink};
use rvnc_proto::{Rect, encoding};

/// xorshift64*: deterministic, dependency-free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A sink that checks the decoder's bounds guarantees.
struct Checker {
    w: u16,
    h: u16,
}

impl RectSink for Checker {
    fn size(&self) -> (u16, u16) {
        (self.w, self.h)
    }
    fn raw(&mut self, rect: Rect, pixels: &[u8]) {
        assert!(rect.fits_in(self.w, self.h));
        assert_eq!(pixels.len(), usize::from(rect.w) * usize::from(rect.h) * 4);
    }
    fn copy_rect(&mut self, dst: Rect, sx: u16, sy: u16) {
        assert!(dst.fits_in(self.w, self.h));
        assert!(Rect::new(sx, sy, dst.w, dst.h).fits_in(self.w, self.h));
    }
    fn desktop_size(&mut self, w: u16, h: u16) {
        self.w = w;
        self.h = h;
    }
}

/// Builds a plausible server stream so mutations reach deep code paths.
fn valid_stream(rng: &mut Rng) -> Vec<u8> {
    let mut b = Vec::new();
    for _ in 0..rng.below(4) + 1 {
        match rng.below(4) {
            0 => {
                let n = rng.below(4) as u16;
                b.extend_from_slice(&[0, 0]);
                b.extend_from_slice(&n.to_be_bytes());
                for _ in 0..n {
                    let (w, h) = (rng.below(5) as u16, rng.below(5) as u16);
                    let (x, y) = (rng.below(8) as u16, rng.below(8) as u16);
                    let enc = [encoding::RAW, encoding::COPY_RECT, encoding::DESKTOP_SIZE]
                        [rng.below(3) as usize];
                    for v in [x, y, w, h] {
                        b.extend_from_slice(&v.to_be_bytes());
                    }
                    b.extend_from_slice(&enc.to_be_bytes());
                    match enc {
                        encoding::RAW => b.extend(std::iter::repeat_n(7u8, usize::from(w * h) * 4)),
                        encoding::COPY_RECT => {
                            b.extend_from_slice(&(rng.below(8) as u16).to_be_bytes());
                            b.extend_from_slice(&(rng.below(8) as u16).to_be_bytes());
                        }
                        _ => {}
                    }
                }
            }
            1 => b.extend_from_slice(&[1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]),
            2 => b.push(2),
            _ => b.extend_from_slice(&[3, 0, 0, 0, 0, 0, 0, 2, b'h', b'i']),
        }
    }
    b
}

fn decode_all(data: &[u8]) {
    let mut r = data;
    let mut d = Decoder::new();
    let mut sink = Checker { w: 8, h: 8 };
    // Each successful message consumes at least one byte, so this ends.
    while !r.is_empty() && d.read_message(&mut r, &mut sink).is_ok() {}
}

#[test]
fn random_bytes() {
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for _ in 0..20_000 {
        let len = rng.below(64) as usize;
        let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        decode_all(&data);
        let _ = ServerInit::read(&mut data.as_slice());
        let _ = read_security_offer(&mut data.as_slice(), Version::V3_3);
        let _ = read_security_offer(&mut data.as_slice(), Version::V3_8);
        if data.len() >= 12 {
            let _ = Version::parse_server(data[..12].try_into().unwrap());
        }
    }
}

#[test]
fn mutated_valid_streams() {
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    for _ in 0..20_000 {
        let mut data = valid_stream(&mut rng);
        decode_all(&data);
        for _ in 0..rng.below(4) + 1 {
            if data.is_empty() {
                break;
            }
            let i = rng.below(data.len() as u64) as usize;
            match rng.below(3) {
                0 => data[i] = rng.next() as u8,
                1 => data.truncate(i),
                _ => data.insert(i, rng.next() as u8),
            }
        }
        decode_all(&data);
    }
}
