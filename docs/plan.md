# MVP plan and tasks

Status: draft · Related: [architecture.md](architecture.md), [protocol.md](protocol.md), [ui-framework.md](ui-framework.md), [input.md](input.md)

## MVP definition of done

`cargo run --release -p rvnc-app -- <host>[:display | ::port]` opens a window showing a remote desktop served by TigerVNC (`-SecurityTypes None`) on the LAN, and:

- Display updates with no visible corruption; window drags/scrolls on the remote use CopyRect.
- Typing, shortcuts, clicks, drags and scrolling work per the checklist in [input.md](input.md#test-checklist-manual-per-milestone).
- Remote resolution change (`xrandr -s ...`) is reflected without reconnecting.
- Server disconnect, wrong port, or server requiring auth all produce a clear message, not a panic.
- Window resizing scales correctly (Fit mode); 1:1 toggle works.
- `cargo test` and `cargo clippy -- -D warnings` pass on CI (GitHub Actions, `macos-latest`).

## Milestones

Each milestone ends in something runnable. Estimates are rough focused-hours for one developer.

### M0 — Project skeleton (≈ 2 h)

- [ ] `git init`, `.gitignore`, MIT/Apache-2.0 licence choice, README pointing at `docs/`.
- [ ] Cargo workspace with `rvnc-proto`, `rvnc-client`, `rvnc-app` crates; `edition = "2024"`, pinned `rust-toolchain.toml`.
- [ ] `tracing-subscriber` init in `rvnc-app` with `RUST_LOG`.
- [ ] CI workflow: fmt check, clippy, test on macOS.
- [ ] Dev server: `docker compose` file (or script) running TigerVNC `Xvnc` + a light WM (e.g. xfce4 or fluxbox) + xterm, with `-SecurityTypes None`, exposed on `localhost:5901`.

### M1 — Protocol core in `rvnc-proto` (≈ 6–8 h)

- [ ] Primitives: big-endian read helpers over `Read`, bounded string/byte reads with size caps.
- [ ] `Version` parsing/selection (3.3/3.7/3.8, Apple 3.889 → 3.8).
- [ ] `PixelFormat` struct, encode/decode, `PixelFormat::rvnc_default()` (32bpp BGRX).
- [ ] Client message encoders: SetPixelFormat, SetEncodings, FramebufferUpdateRequest, KeyEvent, PointerEvent.
- [ ] Server message decoder: FramebufferUpdate header + per-rect dispatch to `RectSink`; SetColourMapEntries, Bell, ServerCutText parsed and surfaced.
- [ ] Rect decoders: Raw, CopyRect, DesktopSize.
- [ ] Unit tests with hand-written byte fixtures for every message, including malformed/truncated inputs (must error, never panic).

### M2 — Headless session in `rvnc-client` (≈ 6–8 h)

- [ ] Handshake state machine with `SecurityHandler` trait; `NoneSecurity` implementation; clear errors for "server requires auth: types [2, 30]".
- [ ] `Framebuffer` with `RectSink` impl (bounds-checked raw copy, overlapping CopyRect, resize).
- [ ] Reader thread (decode loop, update-request pipelining) and writer thread (command channel).
- [ ] `EventSink` trait and `SessionEvent` enum; shutdown path that unblocks the reader.
- [ ] Fake RFB server test helper (in-process `TcpListener` on port 0, scripted responses) and integration tests: handshake for each version, raw update lands in framebuffer, CopyRect, resize, server disconnect.
- [ ] `examples/screenshot.rs`: connect, wait for first full update, write a PNG (dev-only `png` dependency). **Milestone check:** screenshot of the Docker desktop is correct.

### M3 — Viewer window in `rvnc-app` (≈ 6–8 h)

- [ ] CLI parsing (`clap`): `host`, `host:N` (port 5900+N), `host::port`, `--view-only`, `--scale fit|1:1`.
- [ ] winit `ApplicationHandler`; spawn session; `EventLoopProxy`-backed `EventSink`.
- [ ] `Presenter` trait + softbuffer implementation; window→fb mapping; Fit (letterboxed, nearest-neighbour) and 1:1 modes; dirty-rect-only copying.
- [ ] Initial window sizing (1:1 physical pixels, clamped to screen).
- [ ] Handle `Resized`, `Disconnected` (title + log, exit after showing), `Bell` (log).
- [ ] **Milestone check:** watch the remote desktop live; resize window; `xrandr` on the server.

### M4 — Input (≈ 6–8 h)

- [ ] Keysym tables: named keys, modifiers (by physical key), Latin-1 and Unicode keysyms.
- [ ] Press/release pairing map; release-all on focus loss and quit.
- [ ] Ctrl/Cmd → `key_without_modifiers()` rule.
- [ ] Reserved local shortcuts (Cmd-Q, Cmd-Option-S/F/Delete).
- [ ] Pointer: coordinate mapping, button mask, wheel and trackpad scroll accumulation.
- [ ] Unit tests for the mapping functions (pure functions from winit types → keysyms).
- [ ] **Milestone check:** full manual checklist in [input.md](input.md#test-checklist-manual-per-milestone) on UK and US layouts.

### M5 — Hardening and polish (≈ 4–6 h)

- [ ] Test against x11vnc and QEMU in addition to TigerVNC; fix divergences.
- [ ] Performance pass: measure decode + present time per frame at 1080p and 4K (log via `tracing` spans); fix obvious hot spots (avoid per-pixel loops, reuse buffers).
- [ ] Fullscreen toggle; window title `rvnc — <server name> (<w>×<h>)`.
- [ ] `cargo fuzz` target for the server-message decoder; run for a while, fix findings.
- [ ] README: build, run, dev server instructions, known limitations.

**MVP total: roughly 30–40 focused hours.**

## Post-MVP roadmap (ordered by value)

1. **VNC Authentication** (type 2, DES) + password prompt (stdin or `rpassword` first, GUI later). Unlocks most real servers.
2. **ZRLE encoding** (zlib + tiles) — big bandwidth win; widely supported. Then **Tight** (zlib + JPEG; best over WAN).
3. **Cursor pseudo-encoding** — local cursor rendering, no double cursor.
4. **ExtendedDesktopSize / SetDesktopSize** — resize remote to match window.
5. **Clipboard** sync (ServerCutText/ClientCutText, later Extended Clipboard for UTF-8).
6. **Apple ARD auth** (type 30, DH + AES) — connect to macOS Screen Sharing natively.
7. **VeNCrypt / TLS** (`rustls`).
8. **Connection UI**: egui overlay connect dialog, saved hosts, Keychain for passwords, Bonjour discovery.
9. **wgpu presenter** for smooth GPU scaling.
10. **ContinuousUpdates + Fence** for lower latency.
11. `.app` bundle (`cargo-bundle`), code signing, notarisation, Homebrew cask.
12. Windows / Linux builds.

## Testing strategy

| Layer | How |
|---|---|
| `rvnc-proto` | Byte-fixture unit tests per message/encoding; malformed-input tests; `cargo fuzz` on decoder. |
| `rvnc-client` | Integration tests against an in-process scripted fake server (handshake variants, updates, resize, disconnect, auth-required error). Framebuffer tests for CopyRect overlap cases. |
| `rvnc-app` | Pure-function unit tests for keysym mapping and coordinate mapping. Window/rendering verified manually. |
| End-to-end | Manual against TigerVNC (Docker), x11vnc, QEMU using the input checklist. The `screenshot` example gives a scriptable smoke test against a live server. |

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Keyboard mapping edge cases (Option chars, layouts, dead keys) | Feels broken | Dedicated design ([input.md](input.md)), physical-key pairing map, test on two layouts, QEMU raw-key mode later. |
| CPU scaling too slow for 4K remotes | Laggy UI | Dirty-rect-only scaling; measure in M5; wgpu presenter is already planned behind `Presenter` trait. |
| Raw encoding bandwidth (1080p full frame ≈ 8 MB) | Slow over Wi-Fi/WAN | Acceptable on LAN for MVP; ZRLE is roadmap item 2. |
| No-auth test servers aren't what users run | MVP has limited real-world use | VNC auth is first post-MVP item and isolated behind `SecurityHandler`. |
| winit/softbuffer API churn | Rework | Pin versions; thin wrappers (`Presenter`, input mapping module). |
| Server sends unsupported message/encoding | Disconnect | Clear error naming the type/encoding; we only advertise what we decode. |
