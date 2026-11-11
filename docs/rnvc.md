# Rust VNC

A VNC client built in Rust. Targetted at Mac.

## Requirements

- VNC client
- Keyboard/mouse support
- Mac client (other platforms in future)

## Status

The MVP (milestones M0–M5) is implemented. See the [README](../README.md) for
build and run instructions, and [plan.md](plan.md#status-2026-11-11) for what has
been verified and what's left.

## Design summary

Full detail: [architecture.md](architecture.md) · [protocol.md](protocol.md) · [ui-framework.md](ui-framework.md) · [input.md](input.md)

- **Protocol:** RFB per RFC 6143. Negotiates versions 3.3 / 3.7 / 3.8. MVP supports security type *None*, Raw and CopyRect encodings, and the DesktopSize pseudo-encoding. The client asks the server for 32-bit BGRX pixels so data can be copied straight into the display buffer with no conversion.
- **Crate layout:** Cargo workspace with three crates:
  - `rvnc-proto`: pure protocol encode/decode, with no I/O, so it is easy to test and fuzz.
  - `rvnc-client`: TCP session, handshake, framebuffer and threads. It has no UI dependency.
  - `rvnc-app`: the macOS window, rendering and input mapping.
- **Runtime:** no async runtime. A reader thread decodes server messages into a shared `Arc<Mutex<Framebuffer>>` and notifies the UI through winit's `EventLoopProxy`. A writer thread drains a channel of input events and update requests. The UI runs on the main thread and never touches the socket. Update requests are pipelined: after each update arrives, one incremental request is sent.
- **UI:** `winit` for the window and raw keyboard/mouse events, `softbuffer` for CPU blitting. "Fit" (letterboxed) and 1:1 scaling modes. Rendering sits behind a `Presenter` trait, so a `wgpu` backend and an `egui` overlay can be added later. egui, iced, Tauri and native AppKit were considered and rejected for the MVP, mainly because winit gives the most precise keyboard input (see [ui-framework.md](ui-framework.md)).
- **Input:** Mac keys are mapped to X11 keysyms. Modifiers are mapped by physical key (Option→Alt, Cmd→Super). Each key release sends the same keysym as its press, tracked by physical key. All held keys are released when the window loses focus. Trackpad scrolling is accumulated into discrete wheel clicks. Local shortcuts use the Cmd-Option prefix.

## Plan

Full detail and task lists: [plan.md](plan.md)

### MVP

- Basic client
- No TLS
- No authentication

Done when: `rvnc <host>[:port]` shows and controls a TigerVNC desktop (`-SecurityTypes None`) on the LAN. That includes correct updates, keyboard, mouse and scrolling, handling a remote resolution change, and a clear error for a disconnect or a server that requires auth.

| Milestone | Outcome | Est. |
|---|---|---|
| M0 Skeleton | Workspace, CI, Docker TigerVNC dev server | ~2 h |
| M1 Protocol core | `rvnc-proto` messages, Raw/CopyRect/DesktopSize, fixture tests | 6–8 h |
| M2 Headless session | Handshake, threads, framebuffer, fake-server tests, `screenshot` example | 6–8 h |
| M3 Viewer window | winit + softbuffer live view, scaling, resize/disconnect handling | 6–8 h |
| M4 Input | Keysym mapping, key pairing, pointer and scroll | 6–8 h |
| M5 Hardening | Test against x11vnc/QEMU, perf pass, fuzzing, fullscreen, README | 4–6 h |

### After MVP (in priority order)

VNC password auth → ZRLE/Tight → cursor pseudo-encoding → resize remote to window → clipboard → Apple ARD auth (macOS Screen Sharing) → TLS → connect dialog/Keychain/Bonjour → wgpu renderer → `.app` bundle and notarisation → Windows/Linux.

Note: macOS's built-in Screen Sharing requires authentication, so the MVP can't connect to it. MVP testing uses TigerVNC/x11vnc/QEMU, and VNC auth is the first item after MVP.
