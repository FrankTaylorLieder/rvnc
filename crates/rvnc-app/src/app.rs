//! The winit application: one window showing one session.

use std::rc::Rc;
use std::time::Instant;

use rvnc_client::{InputSender, ServerInfo, Session, SessionEvent};
use tracing::{debug, error, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Fullscreen, Window, WindowId};

use crate::input::{Keyboard, LocalCommand, Pointer, PointerEvent};
use crate::present::{Presenter, SoftbufferPresenter};
use crate::view::{ScaleMode, View, WinRect};

/// Trackpad scroll distance (logical pixels) per wheel click.
const PIXELS_PER_SCROLL_CLICK: f64 = 24.0;
/// Largest share of the screen the window may initially occupy.
const MAX_INITIAL_SCREEN_FRACTION: f64 = 0.9;

pub struct Options {
    pub view_only: bool,
    pub scale: ScaleMode,
}

pub struct App {
    session: Option<Session>,
    input: InputSender,
    info: ServerInfo,
    opts: Options,
    window: Option<Rc<Window>>,
    presenter: Option<Box<dyn Presenter>>,
    /// Current geometry plus the framebuffer generation it was built for.
    view: Option<(View, u64)>,
    full_redraw: bool,
    keyboard: Keyboard,
    pointer: Pointer,
    /// Set when the session ended with an error; reported after exit.
    pub error: Option<String>,
    frames: FrameStats,
}

impl App {
    pub fn new(session: Session, opts: Options) -> Self {
        Self {
            input: session.input().clone(),
            info: session.info().clone(),
            session: Some(session),
            opts,
            window: None,
            presenter: None,
            view: None,
            full_redraw: true,
            keyboard: Keyboard::default(),
            pointer: Pointer::default(),
            error: None,
            frames: FrameStats::new(),
        }
    }

    fn create_window(&mut self, el: &ActiveEventLoop) -> anyhow::Result<()> {
        let (fb_w, fb_h) = (f64::from(self.info.width), f64::from(self.info.height));
        // One framebuffer pixel per logical point (crisp 2x on Retina),
        // shrunk to fit the screen if necessary.
        let mut size = LogicalSize::new(fb_w.max(320.0), fb_h.max(200.0));
        if let Some(m) = el.primary_monitor() {
            let screen: LogicalSize<f64> = m.size().to_logical(m.scale_factor());
            let s = (screen.width * MAX_INITIAL_SCREEN_FRACTION / size.width)
                .min(screen.height * MAX_INITIAL_SCREEN_FRACTION / size.height)
                .min(1.0);
            size = LogicalSize::new(size.width * s, size.height * s);
        }
        let attrs = Window::default_attributes()
            .with_title(self.title())
            .with_inner_size(size);
        let window = Rc::new(el.create_window(attrs)?);
        self.presenter = Some(Box::new(SoftbufferPresenter::new(window.clone())?));
        window.request_redraw();
        self.window = Some(window);
        Ok(())
    }

    fn title(&self) -> String {
        let (w, h) = self.session.as_ref().map_or((0, 0), |s| {
            let fb = lock(s);
            (fb.width(), fb.height())
        });
        let mut t = format!("{} ({w}×{h})", self.info.name);
        if self.opts.view_only {
            t.push_str(" — view only");
        }
        if self.opts.scale == ScaleMode::OneToOne {
            t.push_str(" — 1:1");
        }
        t
    }

    fn update_title(&self) {
        if let Some(w) = &self.window {
            w.set_title(&self.title());
        }
    }

    fn redraw(&mut self) {
        let (Some(window), Some(presenter), Some(session)) =
            (&self.window, &mut self.presenter, &self.session)
        else {
            return;
        };
        let start = Instant::now();
        let PhysicalSize { width, height } = window.inner_size();
        let mut fb = lock(session);

        let stale = match &self.view {
            Some((v, generation)) => {
                v.window_size() != (width, height)
                    || v.fb_size() != (fb.width(), fb.height())
                    || *generation != fb.generation()
            }
            None => true,
        };
        if stale {
            let v = View::new(width, height, fb.width(), fb.height(), self.opts.scale);
            self.view = Some((v, fb.generation()));
            self.full_redraw = true;
        }
        let view = &self.view.as_ref().expect("set above").0;

        let dirty = fb.take_dirty();
        let damage = if self.full_redraw {
            None
        } else {
            match dirty {
                Some(r) => Some(view.fb_to_window(r)),
                None => Some(WinRect {
                    x0: 0,
                    y0: 0,
                    x1: 0,
                    y1: 0,
                }),
            }
        };
        presenter.draw(fb.pixels(), view, damage);
        drop(fb);
        let drawn = start.elapsed();
        if let Err(e) = presenter.present() {
            error!("presenting frame: {e:#}");
        }
        self.full_redraw = false;
        self.frames.record(drawn, start.elapsed());
    }

    fn send_keys(&self, events: &[(u32, bool)]) {
        for &(sym, down) in events {
            trace!(sym = format_args!("{sym:#x}"), down, "key");
            self.input.key(sym, down);
        }
    }

    fn send_pointer(&self, ev: Option<PointerEvent>) {
        if let Some((x, y, mask)) = ev {
            self.input.pointer(x, y, mask);
        }
    }

    fn release_all(&mut self) {
        let keys = self.keyboard.release_all();
        self.send_keys(&keys);
        let p = self.pointer.release_all();
        self.send_pointer(p);
    }

    fn local_command(&mut self, cmd: LocalCommand) {
        info!(?cmd, "local command");
        match cmd {
            LocalCommand::ToggleScale => {
                self.opts.scale = self.opts.scale.toggled();
                self.view = None;
                self.update_title();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            LocalCommand::ToggleFullscreen => {
                if let Some(w) = &self.window {
                    let fs = match w.fullscreen() {
                        Some(_) => None,
                        None => Some(Fullscreen::Borderless(None)),
                    };
                    w.set_fullscreen(fs);
                }
            }
            LocalCommand::SendCtrlAltDel => {
                if !self.opts.view_only {
                    let keys = self.keyboard.ctrl_alt_del();
                    self.send_keys(&keys);
                }
            }
        }
    }

    fn shutdown(&mut self) {
        if let Some(session) = self.session.take() {
            self.release_all();
            session.close();
        }
    }
}

fn lock(session: &Session) -> std::sync::MutexGuard<'_, rvnc_client::Framebuffer> {
    session
        .framebuffer()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

impl ApplicationHandler<SessionEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(e) = self.create_window(el) {
                self.error = Some(format!("could not open window: {e:#}"));
                el.exit();
            }
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: SessionEvent) {
        match event {
            SessionEvent::FramebufferUpdated => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            SessionEvent::Resized { width, height } => {
                info!(width, height, "remote desktop resized");
                self.view = None;
                self.update_title();
            }
            SessionEvent::Bell => info!("bell"),
            SessionEvent::Disconnected { error } => {
                if error.is_some() {
                    self.error = error;
                }
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(p) = &mut self.presenter {
                    if let Err(e) = p.resize(size.width, size.height) {
                        error!("{e:#}");
                    }
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Focused(false) => {
                debug!("focus lost; releasing keys and buttons");
                self.release_all();
            }
            WindowEvent::ModifiersChanged(m) => self.keyboard.set_modifiers(m.state()),
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                let out = self.keyboard.key(
                    event.physical_key,
                    &event.logical_key,
                    &event.key_without_modifiers(),
                    event.state == ElementState::Pressed,
                );
                if !self.opts.view_only {
                    self.send_keys(&out.events);
                }
                if let Some(cmd) = out.command {
                    self.local_command(cmd);
                }
            }
            _ if self.opts.view_only => {}
            WindowEvent::CursorMoved { position, .. } => {
                if let Some((view, _)) = &self.view {
                    let (x, y) = view.window_to_fb(position.x, position.y);
                    let ev = self.pointer.moved(x, y);
                    self.send_pointer(ev);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let ev = self.pointer.button(button, state == ElementState::Pressed);
                self.send_pointer(ev);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
                    MouseScrollDelta::PixelDelta(p) => {
                        let sf = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
                        let per_click = PIXELS_PER_SCROLL_CLICK * sf;
                        (p.x / per_click, p.y / per_click)
                    }
                };
                for (x, y, mask) in self.pointer.scroll(dx, dy) {
                    self.input.pointer(x, y, mask);
                }
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.shutdown();
        self.frames.log_summary();
    }
}

/// Lightweight frame-time statistics for the performance pass.
struct FrameStats {
    count: u64,
    /// Time spent holding the framebuffer lock.
    locked: std::time::Duration,
    total: std::time::Duration,
    max: std::time::Duration,
}

impl FrameStats {
    fn new() -> Self {
        Self {
            count: 0,
            locked: Default::default(),
            total: Default::default(),
            max: Default::default(),
        }
    }

    fn record(&mut self, locked: std::time::Duration, d: std::time::Duration) {
        self.count += 1;
        self.locked += locked;
        self.total += d;
        self.max = self.max.max(d);
        trace!(?locked, ?d, "frame");
    }

    fn log_summary(&self) {
        if self.count > 0 {
            info!(
                frames = self.count,
                avg = ?(self.total / self.count as u32),
                avg_locked = ?(self.locked / self.count as u32),
                max = ?self.max,
                "render stats"
            );
        }
    }
}
