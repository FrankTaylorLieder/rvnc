//! Mapping between window pixels and framebuffer pixels. Shared by
//! rendering and pointer input so the two can never disagree.

use rvnc_client::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    /// Scale to fit the window, preserving aspect ratio, letterboxed.
    Fit,
    /// One framebuffer pixel per window pixel, anchored top-left.
    OneToOne,
}

impl ScaleMode {
    pub fn toggled(self) -> Self {
        match self {
            ScaleMode::Fit => ScaleMode::OneToOne,
            ScaleMode::OneToOne => ScaleMode::Fit,
        }
    }
}

/// A rectangle in window pixels: `[x0, x1) x [y0, y1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl WinRect {
    pub fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    pub fn union(&self, o: &WinRect) -> WinRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        WinRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

/// The geometry of the remote image within the window.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    win_w: u32,
    win_h: u32,
    fb_w: u16,
    fb_h: u16,
    /// Window pixels per framebuffer pixel.
    scale: f64,
    /// Window-pixel position of the image's top-left corner.
    off_x: i64,
    off_y: i64,
    /// Destination size of the image in window pixels.
    img_w: u32,
    img_h: u32,
    /// For each window column/row inside the image, the source fb column/row.
    src_x: Vec<u16>,
    src_y: Vec<u16>,
}

impl View {
    pub fn new(win_w: u32, win_h: u32, fb_w: u16, fb_h: u16, mode: ScaleMode) -> Self {
        let (scale, img_w, img_h, off_x, off_y) = if fb_w == 0 || fb_h == 0 {
            (1.0, 0, 0, 0, 0)
        } else {
            match mode {
                ScaleMode::OneToOne => (1.0, u32::from(fb_w), u32::from(fb_h), 0, 0),
                ScaleMode::Fit => {
                    let s = (f64::from(win_w) / f64::from(fb_w))
                        .min(f64::from(win_h) / f64::from(fb_h));
                    let iw = ((f64::from(fb_w) * s).round() as u32).min(win_w);
                    let ih = ((f64::from(fb_h) * s).round() as u32).min(win_h);
                    let ox = i64::from((win_w - iw) / 2);
                    let oy = i64::from((win_h - ih) / 2);
                    (s, iw, ih, ox, oy)
                }
            }
        };
        let table = |n: u32, src_len: u16| -> Vec<u16> {
            (0..n)
                .map(|d| {
                    let s = ((f64::from(d) + 0.5) / scale) as u32;
                    s.min(u32::from(src_len.saturating_sub(1))) as u16
                })
                .collect()
        };
        Self {
            win_w,
            win_h,
            fb_w,
            fb_h,
            scale,
            off_x,
            off_y,
            img_w,
            img_h,
            src_x: table(img_w, fb_w),
            src_y: table(img_h, fb_h),
        }
    }

    pub fn window_size(&self) -> (u32, u32) {
        (self.win_w, self.win_h)
    }

    pub fn fb_size(&self) -> (u16, u16) {
        (self.fb_w, self.fb_h)
    }

    /// The part of the window covered by the image, clipped to the window.
    pub fn image_rect(&self) -> WinRect {
        let x0 = self.off_x.clamp(0, i64::from(self.win_w)) as u32;
        let y0 = self.off_y.clamp(0, i64::from(self.win_h)) as u32;
        WinRect {
            x0,
            y0,
            x1: (self.off_x + i64::from(self.img_w)).clamp(0, i64::from(self.win_w)) as u32,
            y1: (self.off_y + i64::from(self.img_h)).clamp(0, i64::from(self.win_h)) as u32,
        }
    }

    /// Maps a window position (physical pixels) to the framebuffer pixel
    /// under it, clamped to the framebuffer edges.
    pub fn window_to_fb(&self, x: f64, y: f64) -> (u16, u16) {
        let map = |v: f64, off: i64, len: u16| -> u16 {
            if len == 0 {
                return 0;
            }
            let f = ((v - off as f64) / self.scale).floor();
            f.clamp(0.0, f64::from(len - 1)) as u16
        };
        (map(x, self.off_x, self.fb_w), map(y, self.off_y, self.fb_h))
    }

    /// The window region affected by a change to framebuffer `r`, clipped.
    pub fn fb_to_window(&self, r: Rect) -> WinRect {
        let img = self.image_rect();
        let x0 = self.off_x + (f64::from(r.x) * self.scale).floor() as i64 - 1;
        let y0 = self.off_y + (f64::from(r.y) * self.scale).floor() as i64 - 1;
        let x1 = self.off_x + (r.right() as f64 * self.scale).ceil() as i64 + 1;
        let y1 = self.off_y + (r.bottom() as f64 * self.scale).ceil() as i64 + 1;
        let cx = |v: i64| v.clamp(i64::from(img.x0), i64::from(img.x1)) as u32;
        let cy = |v: i64| v.clamp(i64::from(img.y0), i64::from(img.y1)) as u32;
        WinRect {
            x0: cx(x0),
            y0: cy(y0),
            x1: cx(x1),
            y1: cy(y1),
        }
    }

    /// Renders the window region `area` from `fb` (stride `fb_w`) into `dst`
    /// (stride `win_w`). Pixels in `area` outside the image get `background`.
    pub fn render(&self, fb: &[u32], dst: &mut [u32], area: WinRect, background: u32) {
        let img = self.image_rect();
        let win_w = self.win_w as usize;
        let fb_w = usize::from(self.fb_w);
        let x1 = area.x1.min(self.win_w);
        let y1 = area.y1.min(self.win_h);
        let area = WinRect {
            x0: area.x0.min(x1),
            y0: area.y0.min(y1),
            x1,
            y1,
        };
        for y in area.y0..area.y1 {
            let row = &mut dst[y as usize * win_w..(y as usize + 1) * win_w];
            let in_img_y = y >= img.y0 && y < img.y1;
            // [ix0, ix1) is the image span within the area; empty spans
            // collapse so the background fills cover exactly the area.
            let (ix0, ix1) = if in_img_y {
                let a = img.x0.clamp(area.x0, area.x1);
                (a, img.x1.clamp(a, area.x1))
            } else {
                (area.x1, area.x1)
            };
            row[area.x0 as usize..ix0 as usize].fill(background);
            if ix0 < ix1 {
                let sy = usize::from(self.src_y[(i64::from(y) - self.off_y) as usize]);
                let src_row = &fb[sy * fb_w..(sy + 1) * fb_w];
                let base = (i64::from(ix0) - self.off_x) as usize;
                let n = (ix1 - ix0) as usize;
                let out = &mut row[ix0 as usize..ix1 as usize];
                if self.scale == 1.0 {
                    out.copy_from_slice(&src_row[base..base + n]);
                } else {
                    for (o, sx) in out.iter_mut().zip(&self.src_x[base..base + n]) {
                        *o = src_row[usize::from(*sx)];
                    }
                }
            }
            row[ix1 as usize..area.x1 as usize].fill(background);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_letterboxes_horizontally() {
        let v = View::new(400, 100, 100, 100, ScaleMode::Fit);
        assert_eq!(
            v.image_rect(),
            WinRect {
                x0: 150,
                y0: 0,
                x1: 250,
                y1: 100
            }
        );
        assert_eq!(v.window_to_fb(150.0, 0.0), (0, 0));
        assert_eq!(v.window_to_fb(249.9, 99.9), (99, 99));
        // Letterbox clicks clamp to the nearest edge.
        assert_eq!(v.window_to_fb(10.0, 50.0), (0, 50));
        assert_eq!(v.window_to_fb(390.0, 50.0), (99, 50));
    }

    #[test]
    fn fit_scales_up_and_down() {
        let v = View::new(200, 100, 100, 50, ScaleMode::Fit);
        assert_eq!(v.window_to_fb(199.0, 99.0), (99, 49));
        assert_eq!(v.window_to_fb(2.0, 2.0), (1, 1));
        let v = View::new(50, 25, 100, 50, ScaleMode::Fit);
        assert_eq!(v.window_to_fb(49.0, 24.0), (98, 48));
    }

    #[test]
    fn one_to_one_is_top_left_and_clips() {
        let v = View::new(50, 50, 100, 100, ScaleMode::OneToOne);
        assert_eq!(
            v.image_rect(),
            WinRect {
                x0: 0,
                y0: 0,
                x1: 50,
                y1: 50
            }
        );
        assert_eq!(v.window_to_fb(10.0, 20.0), (10, 20));
    }

    #[test]
    fn render_letterbox_and_scale() {
        // 2x1 fb into a 4x4 window: scale 2, image 4x2 centred vertically.
        let fb = [1u32, 2];
        let v = View::new(4, 4, 2, 1, ScaleMode::Fit);
        let mut dst = vec![9u32; 16];
        v.render(
            &fb,
            &mut dst,
            WinRect {
                x0: 0,
                y0: 0,
                x1: 4,
                y1: 4,
            },
            0,
        );
        #[rustfmt::skip]
        assert_eq!(dst, [
            0, 0, 0, 0,
            1, 1, 2, 2,
            1, 1, 2, 2,
            0, 0, 0, 0,
        ]);
    }

    #[test]
    fn render_partial_area() {
        let fb: Vec<u32> = (0..16).collect();
        let v = View::new(4, 4, 4, 4, ScaleMode::OneToOne);
        let mut dst = vec![99u32; 16];
        v.render(
            &fb,
            &mut dst,
            WinRect {
                x0: 1,
                y0: 1,
                x1: 3,
                y1: 2,
            },
            0,
        );
        assert_eq!(&dst[4..8], &[99, 5, 6, 99]);
        assert_eq!(dst[0], 99);
    }

    #[test]
    fn fb_to_window_covers_scaled_region() {
        let v = View::new(200, 200, 100, 100, ScaleMode::Fit);
        let r = v.fb_to_window(Rect::new(10, 10, 5, 5));
        assert!(r.x0 <= 20 && r.x1 >= 30 && r.y0 <= 20 && r.y1 >= 30);
        let r = v.fb_to_window(Rect::new(99, 99, 1, 1));
        assert_eq!((r.x1, r.y1), (200, 200));
    }

    #[test]
    fn zero_sized_framebuffer() {
        let v = View::new(100, 100, 0, 0, ScaleMode::Fit);
        assert!(v.image_rect().is_empty());
        assert_eq!(v.window_to_fb(50.0, 50.0), (0, 0));
        let mut dst = vec![1u32; 100 * 100];
        v.render(
            &[],
            &mut dst,
            WinRect {
                x0: 0,
                y0: 0,
                x1: 100,
                y1: 100,
            },
            0,
        );
        assert!(dst.iter().all(|p| *p == 0));
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// `cargo test --release -p rvnc-app -- --ignored --nocapture bench`
    #[test]
    #[ignore]
    fn bench_render() {
        for (fw, fh, ww, wh) in [
            (1280u16, 800u16, 2560u32, 1600u32),
            (1920, 1080, 2560, 1440),
            (3840, 2160, 2880, 1620),
            (2560, 1600, 2560, 1600),
        ] {
            let fb: Vec<u32> = (0..u32::from(fw) * u32::from(fh)).collect();
            let v = View::new(ww, wh, fw, fh, ScaleMode::Fit);
            let mut dst = vec![0u32; (ww * wh) as usize];
            let full = WinRect {
                x0: 0,
                y0: 0,
                x1: ww,
                y1: wh,
            };
            let n = 30;
            let t = Instant::now();
            for _ in 0..n {
                v.render(&fb, &mut dst, full, 0);
            }
            let t2 = Instant::now();
            let mut out = vec![0u32; dst.len()];
            for _ in 0..n {
                out.copy_from_slice(&dst);
            }
            println!(
                "{fw}x{fh} -> {ww}x{wh}: render {:?}/frame, memcpy {:?}/frame",
                (t2 - t) / n,
                t2.elapsed() / n
            );
            std::hint::black_box(&out);
        }
    }
}
