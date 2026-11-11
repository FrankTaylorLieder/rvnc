//! Server-to-client messages (RFC 6143 §7.6) and rectangle decoding.

use std::io::Read;

use crate::handshake::check_size;
use crate::wire::{read_bytes_u32, read_i32, read_u8, read_u16, skip};
use crate::{MAX_CUT_TEXT_LEN, ProtoError, Rect, encoding};

/// Receives decoded rectangles. Implemented by the client's framebuffer.
///
/// The decoder validates every rectangle against [`RectSink::size`] before
/// calling the sink, so implementations may assume rectangles are in bounds.
pub trait RectSink {
    /// Current framebuffer size.
    fn size(&self) -> (u16, u16);

    /// `pixels` holds `rect.w * rect.h` pixels, 4 bytes each (`B G R X`),
    /// row-major with no padding.
    fn raw(&mut self, rect: Rect, pixels: &[u8]);

    /// Copy the `dst.w` x `dst.h` region at `(src_x, src_y)` to `dst`.
    /// Source and destination may overlap.
    fn copy_rect(&mut self, dst: Rect, src_x: u16, src_y: u16);

    /// The server changed the framebuffer size. Contents are undefined until
    /// the next full update.
    fn desktop_size(&mut self, width: u16, height: u16);
}

/// A complete server message. Rectangle payloads are delivered to the
/// [`RectSink`] while decoding rather than being returned here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    FramebufferUpdate { rects: u16, resized: bool },
    SetColourMapEntries { first: u16, count: u16 },
    Bell,
    ServerCutText(Vec<u8>),
}

/// Decodes server messages, assuming the pixel format
/// [`crate::PixelFormat::rvnc_default`] (4 bytes per pixel).
#[derive(Debug, Default)]
pub struct Decoder {
    scratch: Vec<u8>,
}

const BYTES_PER_PIXEL: usize = 4;

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads exactly one server message from `r`.
    pub fn read_message(
        &mut self,
        r: &mut impl Read,
        sink: &mut impl RectSink,
    ) -> Result<ServerMessage, ProtoError> {
        match read_u8(r)? {
            0 => self.read_update(r, sink),
            1 => {
                read_u8(r)?;
                let first = read_u16(r)?;
                let count = read_u16(r)?;
                skip(r, u64::from(count) * 6)?;
                Ok(ServerMessage::SetColourMapEntries { first, count })
            }
            2 => Ok(ServerMessage::Bell),
            3 => {
                let mut pad = [0u8; 3];
                r.read_exact(&mut pad)?;
                let text = read_bytes_u32(r, "server cut text", MAX_CUT_TEXT_LEN)?;
                Ok(ServerMessage::ServerCutText(text))
            }
            t => Err(ProtoError::UnknownMessage(t)),
        }
    }

    fn read_update(
        &mut self,
        r: &mut impl Read,
        sink: &mut impl RectSink,
    ) -> Result<ServerMessage, ProtoError> {
        read_u8(r)?;
        let rects = read_u16(r)?;
        let mut resized = false;
        for _ in 0..rects {
            let rect = Rect {
                x: read_u16(r)?,
                y: read_u16(r)?,
                w: read_u16(r)?,
                h: read_u16(r)?,
            };
            let enc = read_i32(r)?;
            match enc {
                encoding::RAW => self.read_raw(r, sink, rect)?,
                encoding::COPY_RECT => {
                    let src_x = read_u16(r)?;
                    let src_y = read_u16(r)?;
                    check_bounds(sink, rect)?;
                    check_bounds(sink, Rect::new(src_x, src_y, rect.w, rect.h))?;
                    if !rect.is_empty() {
                        sink.copy_rect(rect, src_x, src_y);
                    }
                }
                encoding::DESKTOP_SIZE => {
                    check_size(rect.w, rect.h)?;
                    sink.desktop_size(rect.w, rect.h);
                    resized = true;
                }
                other => return Err(ProtoError::UnsupportedEncoding(other)),
            }
        }
        Ok(ServerMessage::FramebufferUpdate { rects, resized })
    }

    fn read_raw(
        &mut self,
        r: &mut impl Read,
        sink: &mut impl RectSink,
        rect: Rect,
    ) -> Result<(), ProtoError> {
        check_bounds(sink, rect)?;
        let len = usize::from(rect.w) * usize::from(rect.h) * BYTES_PER_PIXEL;
        self.scratch.resize(len, 0);
        r.read_exact(&mut self.scratch)?;
        if len > 0 {
            sink.raw(rect, &self.scratch);
        }
        // Don't hold on to a huge buffer after a one-off full-screen update
        // on a very large desktop.
        if self.scratch.capacity() > 64 * 1024 * 1024 {
            self.scratch = Vec::new();
        }
        Ok(())
    }
}

fn check_bounds(sink: &impl RectSink, rect: Rect) -> Result<(), ProtoError> {
    let (width, height) = sink.size();
    if rect.fits_in(width, height) {
        Ok(())
    } else {
        Err(ProtoError::RectOutOfBounds {
            rect,
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records calls for assertions.
    #[derive(Default)]
    struct Recorder {
        width: u16,
        height: u16,
        calls: Vec<String>,
    }

    impl RectSink for Recorder {
        fn size(&self) -> (u16, u16) {
            (self.width, self.height)
        }
        fn raw(&mut self, rect: Rect, pixels: &[u8]) {
            self.calls.push(format!("raw {rect:?} {pixels:?}"));
        }
        fn copy_rect(&mut self, dst: Rect, sx: u16, sy: u16) {
            self.calls.push(format!("copy {dst:?} {sx} {sy}"));
        }
        fn desktop_size(&mut self, w: u16, h: u16) {
            self.width = w;
            self.height = h;
            self.calls.push(format!("size {w} {h}"));
        }
    }

    fn sink(w: u16, h: u16) -> Recorder {
        Recorder {
            width: w,
            height: h,
            ..Default::default()
        }
    }

    fn rect_header(out: &mut Vec<u8>, x: u16, y: u16, w: u16, h: u16, enc: i32) {
        for v in [x, y, w, h] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&enc.to_be_bytes());
    }

    fn update(n: u16) -> Vec<u8> {
        let mut v = vec![0, 0];
        v.extend_from_slice(&n.to_be_bytes());
        v
    }

    #[test]
    fn raw_rect() {
        let mut buf = update(1);
        rect_header(&mut buf, 1, 1, 2, 1, encoding::RAW);
        buf.extend_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]);
        let mut s = sink(4, 4);
        let msg = Decoder::new()
            .read_message(&mut buf.as_slice(), &mut s)
            .unwrap();
        assert_eq!(
            msg,
            ServerMessage::FramebufferUpdate {
                rects: 1,
                resized: false
            }
        );
        assert_eq!(
            s.calls,
            ["raw Rect { x: 1, y: 1, w: 2, h: 1 } [1, 2, 3, 0, 4, 5, 6, 0]"]
        );
    }

    #[test]
    fn copy_rect_and_resize() {
        let mut buf = update(2);
        rect_header(&mut buf, 0, 0, 2, 2, encoding::COPY_RECT);
        buf.extend_from_slice(&[0, 1, 0, 1]);
        rect_header(&mut buf, 0, 0, 800, 600, encoding::DESKTOP_SIZE);
        let mut s = sink(4, 4);
        let msg = Decoder::new()
            .read_message(&mut buf.as_slice(), &mut s)
            .unwrap();
        assert_eq!(
            msg,
            ServerMessage::FramebufferUpdate {
                rects: 2,
                resized: true
            }
        );
        assert_eq!(
            s.calls,
            ["copy Rect { x: 0, y: 0, w: 2, h: 2 } 1 1", "size 800 600"]
        );
    }

    #[test]
    fn raw_after_resize_uses_new_bounds() {
        let mut buf = update(2);
        rect_header(&mut buf, 0, 0, 8, 8, encoding::DESKTOP_SIZE);
        rect_header(&mut buf, 7, 7, 1, 1, encoding::RAW);
        buf.extend_from_slice(&[0; 4]);
        let mut s = sink(4, 4);
        Decoder::new()
            .read_message(&mut buf.as_slice(), &mut s)
            .unwrap();
        assert_eq!(s.calls.len(), 2);
    }

    #[test]
    fn out_of_bounds_raw_is_error() {
        let mut buf = update(1);
        rect_header(&mut buf, 3, 3, 2, 2, encoding::RAW);
        buf.extend_from_slice(&[0; 16]);
        let mut s = sink(4, 4);
        let err = Decoder::new()
            .read_message(&mut buf.as_slice(), &mut s)
            .unwrap_err();
        assert!(matches!(err, ProtoError::RectOutOfBounds { .. }));
        assert!(s.calls.is_empty());
    }

    #[test]
    fn out_of_bounds_copy_source_is_error() {
        let mut buf = update(1);
        rect_header(&mut buf, 0, 0, 2, 2, encoding::COPY_RECT);
        buf.extend_from_slice(&[0, 3, 0, 0]);
        let err = Decoder::new()
            .read_message(&mut buf.as_slice(), &mut sink(4, 4))
            .unwrap_err();
        assert!(matches!(err, ProtoError::RectOutOfBounds { .. }));
    }

    #[test]
    fn unrequested_encoding_is_error() {
        let mut buf = update(1);
        rect_header(&mut buf, 0, 0, 1, 1, encoding::ZRLE);
        let err = Decoder::new()
            .read_message(&mut buf.as_slice(), &mut sink(4, 4))
            .unwrap_err();
        assert!(matches!(err, ProtoError::UnsupportedEncoding(16)));
    }

    #[test]
    fn truncated_raw_is_eof() {
        let mut buf = update(1);
        rect_header(&mut buf, 0, 0, 2, 2, encoding::RAW);
        buf.extend_from_slice(&[0; 7]);
        let err = Decoder::new()
            .read_message(&mut buf.as_slice(), &mut sink(4, 4))
            .unwrap_err();
        assert!(err.is_eof());
    }

    #[test]
    fn other_messages() {
        let mut buf = vec![1, 0, 0, 5, 0, 2];
        buf.extend_from_slice(&[0; 12]);
        buf.push(2);
        buf.extend_from_slice(&[3, 0, 0, 0, 0, 0, 0, 3, b'a', b'b', b'c']);
        let mut r = buf.as_slice();
        let mut d = Decoder::new();
        let mut s = sink(1, 1);
        assert_eq!(
            d.read_message(&mut r, &mut s).unwrap(),
            ServerMessage::SetColourMapEntries { first: 5, count: 2 }
        );
        assert_eq!(d.read_message(&mut r, &mut s).unwrap(), ServerMessage::Bell);
        assert_eq!(
            d.read_message(&mut r, &mut s).unwrap(),
            ServerMessage::ServerCutText(b"abc".to_vec())
        );
        assert!(r.is_empty());
    }

    #[test]
    fn unknown_message_type() {
        let buf = [200u8];
        assert!(matches!(
            Decoder::new().read_message(&mut buf.as_slice(), &mut sink(1, 1)),
            Err(ProtoError::UnknownMessage(200))
        ));
    }

    #[test]
    fn huge_cut_text_rejected() {
        let buf = [3u8, 0, 0, 0, 0xff, 0xff, 0xff, 0xff];
        assert!(matches!(
            Decoder::new().read_message(&mut buf.as_slice(), &mut sink(1, 1)),
            Err(ProtoError::TooLong { .. })
        ));
    }

    /// Every prefix of a valid stream must fail cleanly, never panic.
    #[test]
    fn all_truncations_error_cleanly() {
        let mut buf = update(3);
        rect_header(&mut buf, 0, 0, 2, 2, encoding::RAW);
        buf.extend_from_slice(&[7; 16]);
        rect_header(&mut buf, 0, 0, 1, 1, encoding::COPY_RECT);
        buf.extend_from_slice(&[0, 1, 0, 1]);
        rect_header(&mut buf, 0, 0, 3, 3, encoding::DESKTOP_SIZE);
        for n in 0..buf.len() {
            let res = Decoder::new().read_message(&mut &buf[..n], &mut sink(2, 2));
            assert!(res.unwrap_err().is_eof(), "prefix {n}");
        }
        assert!(
            Decoder::new()
                .read_message(&mut buf.as_slice(), &mut sink(2, 2))
                .is_ok()
        );
    }
}
