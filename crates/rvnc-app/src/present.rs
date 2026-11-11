//! Getting pixels onto the screen. Behind a trait so a GPU presenter can
//! replace softbuffer later (see `docs/ui-framework.md`).

use std::num::NonZeroU32;
use std::rc::Rc;

use anyhow::Context;
use softbuffer::{Context as SbContext, Surface};
use winit::window::Window;

use crate::view::{View, WinRect};

/// Colour of the letterbox area around the remote image.
pub const BACKGROUND: u32 = 0x0020_2020;

/// Rendering is split in two so the framebuffer lock is held only for the
/// cheap part: [`Presenter::draw`] runs under the lock, [`Presenter::present`]
/// (which on macOS costs ~10 ms for a Retina-sized window) runs after it.
pub trait Presenter {
    /// Called when the window's physical size changes.
    fn resize(&mut self, width: u32, height: u32) -> anyhow::Result<()>;

    /// Draws the changed window region (`None` = everything) of `fb` through
    /// `view` into the back buffer.
    fn draw(&mut self, fb: &[u32], view: &View, damage: Option<WinRect>);

    /// Shows the back buffer if anything was drawn since the last present.
    fn present(&mut self) -> anyhow::Result<()>;
}

pub struct SoftbufferPresenter {
    surface: Surface<Rc<Window>, Rc<Window>>,
    size: (u32, u32),
    /// Persistent back buffer. softbuffer's macOS backend hands out a fresh,
    /// zeroed buffer every frame (age 0), so we keep our own and copy it.
    canvas: Vec<u32>,
    /// Region drawn since the last present.
    pending: Option<WinRect>,
}

impl SoftbufferPresenter {
    pub fn new(window: Rc<Window>) -> anyhow::Result<Self> {
        let context = SbContext::new(window.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("creating softbuffer context")?;
        let surface = Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("creating softbuffer surface")?;
        let size = window.inner_size();
        let mut p = Self {
            surface,
            size: (0, 0),
            canvas: Vec::new(),
            pending: None,
        };
        p.resize(size.width, size.height)?;
        Ok(p)
    }

    fn full(&self) -> WinRect {
        WinRect {
            x0: 0,
            y0: 0,
            x1: self.size.0,
            y1: self.size.1,
        }
    }
}

impl Presenter for SoftbufferPresenter {
    fn resize(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) else {
            return Ok(());
        };
        if (width, height) != self.size {
            self.surface
                .resize(w, h)
                .map_err(|e| anyhow::anyhow!("resizing surface: {e}"))?;
            self.size = (width, height);
            self.canvas.clear();
            self.canvas
                .resize(width as usize * height as usize, BACKGROUND);
            self.pending = None;
        }
        Ok(())
    }

    fn draw(&mut self, fb: &[u32], view: &View, damage: Option<WinRect>) {
        if view.window_size() != self.size || self.canvas.is_empty() {
            return;
        }
        let area = damage.unwrap_or_else(|| self.full());
        if area.is_empty() {
            return;
        }
        view.render(fb, &mut self.canvas, area, BACKGROUND);
        self.pending = Some(match self.pending {
            Some(p) => p.union(&area),
            None => area,
        });
    }

    fn present(&mut self) -> anyhow::Result<()> {
        let Some(area) = self.pending.take() else {
            return Ok(());
        };
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("getting buffer: {e}"))?;
        if buffer.len() != self.canvas.len() {
            return Ok(());
        }
        // Copying the whole canvas (~0.4 ms at 2560x1600) is simpler than
        // tracking buffer age, which this backend doesn't support anyway.
        buffer.copy_from_slice(&self.canvas);
        let rect = softbuffer::Rect {
            x: area.x0,
            y: area.y0,
            width: NonZeroU32::new(area.x1 - area.x0).expect("non-empty"),
            height: NonZeroU32::new(area.y1 - area.y0).expect("non-empty"),
        };
        buffer
            .present_with_damage(&[rect])
            .map_err(|e| anyhow::anyhow!("presenting: {e}"))?;
        Ok(())
    }
}
