# Architecture

Status: draft for MVP · Related: [protocol.md](protocol.md), [ui-framework.md](ui-framework.md), [input.md](input.md), [plan.md](plan.md)

## Goals and non-goals

**MVP goals**

- Connect to an RFB (VNC) server over plain TCP, security type *None*.
- Display the remote framebuffer in a native macOS window, kept up to date.
- Forward keyboard and mouse (buttons, motion, scroll) to the server.
- Survive server-side resolution changes and disconnects without crashing.

**Non-goals for MVP** (designed for, not built)

- TLS / VeNCrypt, VNC password auth, Apple ARD auth.
- Compressed encodings (ZRLE, Tight, Hextile), cursor pseudo-encoding.
- Clipboard sync, file transfer, audio, multiple simultaneous sessions.
- Connection manager / saved hosts UI, `.app` bundling and notarisation.
- Windows / Linux (but nothing in the design should block them).

## Shape of the system

Cargo workspace with three crates. Dependencies point downward only.

```
rvnc/
├── Cargo.toml            (workspace)
├── crates/
│   ├── rvnc-proto/       pure RFB protocol: types, encode/decode, no I/O
│   ├── rvnc-client/      session: TCP, handshake, threads, framebuffer
│   └── rvnc-app/         binary: winit window, rendering, input mapping
└── docs/
```

| Crate | Knows about | Does not know about |
|---|---|---|
| `rvnc-proto` | Bytes, RFB message layouts, pixel formats, encodings | Sockets, threads, windows |
| `rvnc-client` | `std::net::TcpStream`, threads, channels, the framebuffer | winit, macOS, rendering |
| `rvnc-app` | winit, softbuffer, keysym mapping, CLI args | RFB wire format |

Why split this way:

- **`rvnc-proto` is "sans-IO".** Decoders take `&[u8]` / a buffered reader and return messages or "need more bytes". That makes it trivially unit-testable with byte fixtures and fuzzable, and lets us swap blocking I/O for async later without touching protocol code.
- **`rvnc-client` is UI-agnostic.** A headless test harness (or a future CLI screenshot tool) can drive a full session.
- **`rvnc-app` is the only platform-specific crate.** Porting to Windows/Linux is mostly a matter of keyboard-layout quirks in this crate.

## Runtime model

Plain OS threads and channels — no async runtime in the MVP. RFB is a single long-lived TCP stream with a simple request/response rhythm; blocking I/O on two threads is the simplest correct design.

```
            ┌──────────────── rvnc-app (main thread) ────────────────┐
            │  winit event loop                                       │
            │   ├─ WindowEvent::KeyboardInput ──► map to keysym ─┐    │
            │   ├─ WindowEvent::CursorMoved/MouseInput/Wheel ─────┤    │
            │   ├─ UserEvent::FramebufferDirty ◄──────────┐       │    │
            │   └─ RedrawRequested ─► lock fb, blit ──┐   │       │    │
            └─────────────────────────────────────────┼───┼───────┼────┘
                                                      │   │       │ ClientCommand
                                    Arc<Mutex<Framebuffer>> │       ▼ (mpsc)
            ┌──────────────── rvnc-client ────────────┼───┼───────────────┐
            │  reader thread                          │   │  writer thread │
            │   loop: read ServerMessage ─► decode ───┘   │   loop: recv cmd│
            │         into framebuffer, mark dirty rect   │   encode, write │
            │         ─► EventLoopProxy::send_event ──────┘   to socket     │
            │         ─► ask writer for next incremental update request     │
            └───────────────────────── TcpStream (try_clone) ───────────────┘
```

- **Reader thread** owns the read half. It runs the handshake (synchronously, before spawning the writer), then loops decoding server messages. Rectangles are decoded straight into the shared framebuffer. After a complete `FramebufferUpdate` it notifies the UI and enqueues the next incremental `FramebufferUpdateRequest`.
- **Writer thread** owns the write half (`TcpStream::try_clone`). It drains a `std::sync::mpsc::Receiver<ClientCommand>` and writes encoded client messages. Keeping writes on one thread means input events and update requests never interleave mid-message.
- **Main thread** runs the winit event loop (macOS requires UI on the main thread). It never touches the socket.
- **Notifications** from client to UI go through a small `SessionEvent` enum delivered via winit's `EventLoopProxy` (wrapped behind a trait in `rvnc-client` so the client crate doesn't depend on winit).

### Key types (sketch)

```rust
// rvnc-client
pub struct Framebuffer {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u32>,        // 0x00RRGGBB, row-major, stride == width
    pub dirty: Option<Rect>,     // union of rects since last present
    pub generation: u64,         // bumps on resize
}

pub enum ClientCommand {
    Key { keysym: u32, down: bool },
    Pointer { x: u16, y: u16, buttons: u8 },
    RequestUpdate { incremental: bool, rect: Rect },
    Shutdown,
}

pub enum SessionEvent {
    Connected { name: String, width: u16, height: u16 },
    FramebufferUpdated,
    Resized { width: u16, height: u16 },
    Bell,
    Disconnected { reason: Option<String> },
}

pub trait EventSink: Send + 'static {
    fn send(&self, ev: SessionEvent);
}

pub fn connect(addr: &str, opts: ConnectOptions, sink: impl EventSink)
    -> Result<Session, ConnectError>;

impl Session {
    pub fn framebuffer(&self) -> Arc<Mutex<Framebuffer>>;
    pub fn commands(&self) -> mpsc::Sender<ClientCommand>;
}
```

### Pixel format decision

We send `SetPixelFormat` immediately after `ServerInit` asking for **32 bpp, depth 24, true-colour, little-endian, red-shift 16, green-shift 8, blue-shift 0**. On the wire each pixel is then `[B, G, R, X]`, which read as a little-endian `u32` is `0x00RRGGBB` — exactly what softbuffer wants. Raw rects become a `memcpy`-like row copy with no per-pixel conversion. Servers are required to honour `SetPixelFormat`, so we do not need a general pixel converter in the MVP (a converter for the server's native format is a stretch item, useful only for servers that misbehave).

### Locking and frame pacing

- The reader holds the framebuffer mutex only while writing a rectangle, not across socket reads (decode a rect's bytes into a scratch buffer, then lock and copy). This keeps the UI's lock wait bounded.
- The UI coalesces: many `FramebufferUpdated` events between frames produce a single `request_redraw`. Redraw copies the framebuffer into the softbuffer surface (with scaling, see [ui-framework.md](ui-framework.md)).
- Update requests are pipelined one at a time: request → wait for the update → request again. This naturally throttles to what the network and server can deliver. (ContinuousUpdates extension is a post-MVP optimisation.)

### Errors and lifecycle

- `thiserror` error enums per crate; `anyhow` only in `rvnc-app`'s `main`.
- Any protocol error or EOF on the reader → `SessionEvent::Disconnected` → the app shows the reason in the window title / a log line and exits (MVP). Reconnect is post-MVP.
- Closing the window sends `ClientCommand::Shutdown`, then `shutdown(Both)` on the socket so the reader unblocks; threads are joined.
- Logging via `tracing` + `tracing-subscriber`, controlled by `RUST_LOG`. Protocol-level trace logging (message types, rect counts, timings) is invaluable when debugging against odd servers.

## Dependencies (MVP)

| Crate | Use |
|---|---|
| `winit` 0.30 | Window, event loop, keyboard/mouse events |
| `softbuffer` 0.4 | CPU framebuffer presentation |
| `thiserror`, `anyhow` | Errors |
| `tracing`, `tracing-subscriber` | Logging |
| `clap` | CLI parsing (`rvnc host[:display|::port]`) |

Deliberately *not* in MVP: `tokio`, `wgpu`, `flate2`/`zlib` (needed for ZRLE/Tight later), `des` (needed for VNC auth later).

## Future-proofing notes

- **Async later:** because `rvnc-proto` is sans-IO, swapping threads for tokio means rewriting only `rvnc-client`'s two loops.
- **GPU later:** rendering sits behind a `Presenter` trait in `rvnc-app` (`resize`, `present(&Framebuffer, dirty)`), so a wgpu backend can replace softbuffer.
- **Auth/TLS later:** the handshake is a state machine with a pluggable `SecurityHandler` trait; *None* is the only MVP implementation. TLS wraps the stream before the handshake, so the reader/writer must be generic over `Read`/`Write` rather than hard-coded to `TcpStream` (with a `try_clone`-like split abstraction).
