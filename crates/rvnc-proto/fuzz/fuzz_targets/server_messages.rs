//! `cargo +nightly fuzz run server_messages` (from crates/rvnc-proto).
#![no_main]

use libfuzzer_sys::fuzz_target;
use rvnc_proto::Rect;
use rvnc_proto::handshake::{ServerInit, Version, read_security_offer};
use rvnc_proto::server::{Decoder, RectSink};

struct Sink {
    w: u16,
    h: u16,
}

impl RectSink for Sink {
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

fuzz_target!(|data: &[u8]| {
    let _ = ServerInit::read(&mut &data[..]);
    let _ = read_security_offer(&mut &data[..], Version::V3_8);
    let mut r = data;
    let mut d = Decoder::new();
    let mut sink = Sink { w: 64, h: 64 };
    while !r.is_empty() && d.read_message(&mut r, &mut sink).is_ok() {}
});
