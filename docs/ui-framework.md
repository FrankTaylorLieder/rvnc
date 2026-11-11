# UI framework choice

Status: decided for MVP (revisit after MVP) · Related: [architecture.md](architecture.md), [input.md](input.md)

## What the UI actually needs

A VNC viewer's UI is unusual: almost the whole window is one bitmap, and the hard requirements are about **input fidelity** and **pixel throughput**, not widgets.

| Need | Weight | Notes |
|---|---|---|
| Fast blit of a large, frequently-changing bitmap (up to 5K, partial updates) | High | Core of the app |
| Low-level keyboard events: press *and* release, physical key, logical key, modifiers | High | Needed for correct keysym mapping |
| Mouse: buttons, motion, precise trackpad scroll deltas | High | |
| HiDPI / Retina awareness | High | Mac default |
| Native Mac feel: menu bar, Cmd-Q, fullscreen | Medium | Nice for MVP, required later |
| Simple forms (connect dialog, password prompt, settings) | Low for MVP | MVP connects from CLI args |
| Cross-platform later | Medium | Stated requirement |
| Pure Rust, no Swift/ObjC build step | Medium | Keeps tooling to `cargo` |

## Options considered

| Option | Bitmap display | Keyboard fidelity | Forms/widgets | Cross-platform | Verdict |
|---|---|---|---|---|---|
| **winit + softbuffer** | CPU copy to window, simple | Excellent (`KeyEvent` has physical + logical key, press/release, repeat flag) | None | Yes | **MVP choice** |
| winit + wgpu | GPU texture, free scaling | Excellent | None (add egui on top) | Yes | Upgrade path (M4+) |
| eframe / egui | Texture via egui, fine | Lossy: egui's `Key` enum is limited, events are pre-processed for text editing; hard to get raw press/release for every key | Great, immediate-mode | Yes | Good for dialogs, poor for the remote view |
| iced | Image widget / custom shader | Better than egui but still widget-oriented | Good | Yes | Heavier, more churn than needed |
| Slint | Image element | Widget-oriented key handling | Good, declarative | Yes | Overkill; licensing considerations |
| Native AppKit via `objc2` | `CALayer`/`NSView` direct | Best possible (raw `NSEvent`, keycodes) | Native but verbose | Mac only | Great fidelity, lots of unsafe-ish boilerplate, kills cross-platform |
| Tauri / webview | Canvas from Rust via IPC | Browser key events (poor for e.g. Cmd combos) | Great | Yes | IPC cost for pixels; wrong tool |
| SwiftUI shell + Rust core (FFI) | Native | Native | Native | Mac only UI | Best Mac UX long-term; two languages, two build systems |

## Decision

**MVP: `winit` 0.30 for windowing/input + `softbuffer` 0.4 for presentation.**

Reasons:

1. **Input fidelity is the hardest part of a VNC client** and winit exposes exactly what we need: `KeyEvent { physical_key, logical_key, text, state, repeat }` plus `ModifiersChanged`. The winit macOS backend is built on `NSEvent`, so nothing important is lost.
2. **softbuffer is the smallest possible path to pixels** — give it a `&mut [u32]` of `0x00RRGGBB` and it presents. That matches our negotiated pixel format exactly, so a raw rect goes wire → framebuffer → surface with two straight copies.
3. **Pure cargo, cross-platform**, consistent with the stated future direction.
4. **Low lock-in:** rendering is behind a `Presenter` trait, and the winit event loop is the same foundation wgpu and egui sit on, so both upgrades are additive.

### Known limitations we accept for MVP

- **Scaling is done on the CPU** (nearest-neighbour) when the window is not 1:1 with the framebuffer. Fine for 1080p–1440p remotes; may be costly for 4K/5K remotes scaled into a smaller window. Mitigation: only re-scale the dirty rect; move to wgpu (bilinear, GPU) in M4 if profiling shows it matters.
- **No widgets.** Host/port come from the command line (`rvnc myhost:1`). Errors go to the window title and the log.
- **Minimal menu bar.** winit gives a default app menu with Quit on macOS. A proper menu (Send Ctrl-Alt-Del, toggle scaling, fullscreen) uses the `muda` crate in a later milestone.
- **System shortcuts** (Cmd-Tab, Cmd-Space, Mission Control) are captured by macOS before the app sees them. Intercepting them requires a `CGEventTap` and Accessibility permission — out of scope.

## Rendering details

- Window is created at the framebuffer's logical size divided by the display scale factor, so a 1920×1080 remote opens as a 1920×1080 *physical-pixel* window on a Retina display (1:1, crisp). If that exceeds the screen, fit to the screen's visible frame.
- Two display modes, toggled by a key (MVP: `Cmd-Option-S`; see [input.md](input.md)):
  - **Fit** (default): scale to fit the window preserving aspect ratio, letterbox in dark grey.
  - **1:1**: unscaled, anchored top-left; pixels beyond the window are clipped (scrolling is post-MVP).
- Keep a cached mapping `window_px → fb_px` (scale + offset) that is used both for rendering and for translating mouse coordinates, so they can never disagree.
- On `Resized` from the server: recompute the mapping, resize the softbuffer surface if the window size changes, and request a redraw. Window is not auto-resized after the initial open (it's jarring); the user can resize.
- On `RedrawRequested`: lock the framebuffer, copy (and scale if needed) into the surface buffer, `present()` (or `present_with_damage` for the dirty region where supported), clear `dirty`.

## Upgrade path

| Milestone | Change |
|---|---|
| M5 (polish) | `muda` native menu bar; window title shows server name + state |
| Post-MVP | `wgpu` presenter: upload dirty rects to a texture, draw a fullscreen quad with linear filtering; frees CPU, smooth scaling |
| Post-MVP | `egui` overlay (via `egui-winit` + `egui-wgpu`) for connect dialog, password prompt, settings — rendered on top of the remote view, keyboard routed to egui only while a dialog is open |
| Long-term option | If Mac polish becomes the priority (Keychain, Bonjour browser, Handoff, sandboxed App Store build), wrap `rvnc-client` in a C ABI / `uniffi` and build a SwiftUI shell. The crate split in [architecture.md](architecture.md) keeps this open. |
