use rvnc_proto::Rect;

/// The client's copy of the remote screen.
///
/// Pixels are `0x00RRGGBB`, row-major, with a stride equal to `width`.
#[derive(Debug, Clone)]
pub struct Framebuffer {
    width: u16,
    height: u16,
    pixels: Vec<u32>,
    dirty: Option<Rect>,
    generation: u64,
}

impl Framebuffer {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height)],
            dirty: None,
            generation: 0,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// Incremented every time the framebuffer is resized.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The union of regions changed since the last call, if any.
    pub fn take_dirty(&mut self) -> Option<Rect> {
        self.dirty.take()
    }

    pub fn dirty(&self) -> Option<Rect> {
        self.dirty
    }

    pub fn pixel(&self, x: u16, y: u16) -> u32 {
        self.pixels[usize::from(y) * usize::from(self.width) + usize::from(x)]
    }

    fn mark(&mut self, r: Rect) {
        self.dirty = Some(match self.dirty {
            Some(d) => d.union(&r),
            None => r,
        });
    }

    /// Writes `rect.w * rect.h` pixels in `B G R X` byte order.
    ///
    /// # Panics
    /// If `rect` is outside the framebuffer or `bgrx` is too short. The
    /// protocol decoder validates both before calling.
    pub fn put_raw(&mut self, rect: Rect, bgrx: &[u8]) {
        assert!(rect.fits_in(self.width, self.height));
        let w = usize::from(rect.w);
        let stride = usize::from(self.width);
        for (row, src) in bgrx
            .chunks_exact(w * 4)
            .take(usize::from(rect.h))
            .enumerate()
        {
            let start = (usize::from(rect.y) + row) * stride + usize::from(rect.x);
            let dst = &mut self.pixels[start..start + w];
            for (d, s) in dst.iter_mut().zip(src.chunks_exact(4)) {
                *d = u32::from_le_bytes([s[0], s[1], s[2], 0]);
            }
        }
        self.mark(rect);
    }

    /// Copies the region at `(src_x, src_y)` of size `dst.w` x `dst.h` to
    /// `dst`. Handles overlapping regions.
    pub fn copy_rect(&mut self, dst: Rect, src_x: u16, src_y: u16) {
        assert!(dst.fits_in(self.width, self.height));
        assert!(Rect::new(src_x, src_y, dst.w, dst.h).fits_in(self.width, self.height));
        let stride = usize::from(self.width);
        let w = usize::from(dst.w);
        let copy_row = |pixels: &mut [u32], row: usize| {
            let s = (usize::from(src_y) + row) * stride + usize::from(src_x);
            let d = (usize::from(dst.y) + row) * stride + usize::from(dst.x);
            pixels.copy_within(s..s + w, d);
        };
        // When moving down, copy bottom rows first so sources aren't
        // overwritten before they are read. copy_within handles overlap
        // within a single row.
        if dst.y > src_y {
            for row in (0..usize::from(dst.h)).rev() {
                copy_row(&mut self.pixels, row);
            }
        } else {
            for row in 0..usize::from(dst.h) {
                copy_row(&mut self.pixels, row);
            }
        }
        self.mark(dst);
    }

    /// Resizes to `width` x `height`, clearing to black.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels
            .resize(usize::from(width) * usize::from(height), 0);
        self.generation += 1;
        self.dirty = Some(Rect::new(0, 0, width, height));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fills each pixel with `y * 16 + x` for easy assertions.
    fn numbered(w: u16, h: u16) -> Framebuffer {
        let mut fb = Framebuffer::new(w, h);
        for y in 0..h {
            for x in 0..w {
                fb.pixels[usize::from(y) * usize::from(w) + usize::from(x)] =
                    u32::from(y) * 16 + u32::from(x);
            }
        }
        fb
    }

    #[test]
    fn put_raw_converts_bgrx() {
        let mut fb = Framebuffer::new(3, 2);
        fb.put_raw(Rect::new(1, 1, 2, 1), &[0x33, 0x22, 0x11, 0xff, 3, 2, 1, 0]);
        assert_eq!(fb.pixel(1, 1), 0x0011_2233);
        assert_eq!(fb.pixel(2, 1), 0x0001_0203);
        assert_eq!(fb.pixel(0, 0), 0);
        assert_eq!(fb.take_dirty(), Some(Rect::new(1, 1, 2, 1)));
        assert_eq!(fb.take_dirty(), None);
    }

    #[test]
    fn copy_rect_overlapping_down_right() {
        let mut fb = numbered(4, 4);
        fb.copy_rect(Rect::new(1, 1, 3, 3), 0, 0);
        let expect = numbered(4, 4);
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(fb.pixel(x + 1, y + 1), expect.pixel(x, y), "({x},{y})");
            }
        }
        assert_eq!(fb.pixel(0, 0), 0);
    }

    #[test]
    fn copy_rect_overlapping_up_left() {
        let mut fb = numbered(4, 4);
        fb.copy_rect(Rect::new(0, 0, 3, 3), 1, 1);
        let expect = numbered(4, 4);
        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(fb.pixel(x, y), expect.pixel(x + 1, y + 1), "({x},{y})");
            }
        }
    }

    #[test]
    fn resize_bumps_generation() {
        let mut fb = numbered(4, 4);
        fb.take_dirty();
        fb.resize(2, 3);
        assert_eq!((fb.width(), fb.height(), fb.generation()), (2, 3, 1));
        assert!(fb.pixels().iter().all(|p| *p == 0));
        assert_eq!(fb.take_dirty(), Some(Rect::new(0, 0, 2, 3)));
    }
}
