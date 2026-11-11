# Protocol handling (RFB)

Status: draft for MVP · Related: [architecture.md](architecture.md)

Reference: **RFC 6143** (RFB 3.8) plus the community spec at <https://github.com/rfbproto/rfbproto> for encodings and extensions not in the RFC.

All multi-byte integers on the wire are **big-endian** unless they are pixel data (which follows the negotiated pixel format).

## Scope

| Area | MVP | Later |
|---|---|---|
| Versions | 3.3, 3.7, 3.8 | — |
| Security | None (1) | VNC Auth (2), VeNCrypt/TLS (19), Apple ARD (30), RA2 |
| Encodings | Raw (0), CopyRect (1) | ZRLE (16), Tight (7), Hextile (5), TRLE (15) |
| Pseudo-encodings | DesktopSize (-223) | ExtendedDesktopSize (-308), Cursor (-239), LastRect (-224), ContinuousUpdates (-313), QEMU Extended Key (-258), Fence (-312) |
| Client → server | SetPixelFormat, SetEncodings, FramebufferUpdateRequest, KeyEvent, PointerEvent | ClientCutText, SetDesktopSize |
| Server → client | FramebufferUpdate, SetColourMapEntries (parse & ignore), Bell, ServerCutText (parse & ignore) | Clipboard integration |

**Rule: every server message type the MVP might receive must be fully parsed even if ignored**, otherwise the stream desynchronises. Unknown message types are a fatal protocol error (there is no length prefix to skip them).

## Handshake state machine

```
TCP connected
   │
   ▼
[ProtocolVersion]  S→C "RFB 003.00x\n" (12 bytes)
   │  choose min(server, 3.8); reply C→S with our chosen version string
   │  map: 3.3 → 3.3; 3.4–3.6 → 3.3; 3.7 → 3.7; ≥3.8 (incl. Apple's 3.889) → 3.8
   ▼
[Security]
   3.3:    S→C u32 security-type  (0 = failure + reason string, 1 = None, 2 = VNC auth)
   3.7+:   S→C u8 count, [u8; count] types   (count 0 → u32 reason-len + reason, fail)
           C→S u8 chosen type (must be 1 for MVP, else fail "no supported security type")
   ▼
[SecurityResult]
   3.8:    always sent, even for None: u32 (0 = OK, 1 = failed + u32 len + reason)
   3.7:    NOT sent for None
   3.3:    NOT sent for None
   ▼
[ClientInit]       C→S u8 shared-flag (1 = shared; don't kick other viewers)
   ▼
[ServerInit]       S→C u16 width, u16 height, PIXEL_FORMAT (16 bytes),
                       u32 name-length, name (treat as UTF-8, lossy)
   ▼
[Post-init]        C→S SetPixelFormat (our preferred, see below)
                   C→S SetEncodings [CopyRect, Raw, DesktopSize]   (order = preference)
                   C→S FramebufferUpdateRequest incremental=0, full screen
   ▼
Normal operation
```

The handshake runs synchronously on the reader thread before the writer thread exists, so it can freely read and write the stream. Each state is a function returning the next state or a `HandshakeError` with the server's reason string if one was given. A timeout (e.g. 10 s, via `set_read_timeout`) applies only during the handshake; it is cleared afterwards.

Security is implemented behind a trait so later types slot in:

```rust
trait SecurityHandler {
    fn type_id(&self) -> u8;
    fn authenticate(&mut self, stream: &mut dyn ReadWrite, version: Version) -> Result<(), HandshakeError>;
}
```

## Pixel format

`PIXEL_FORMAT` is 16 bytes: bits-per-pixel, depth, big-endian-flag, true-colour-flag, red-max (u16), green-max, blue-max, red-shift, green-shift, blue-shift, 3 padding.

We request:

| Field | Value |
|---|---|
| bits-per-pixel | 32 |
| depth | 24 |
| big-endian | 0 |
| true-colour | 1 |
| red/green/blue max | 255 / 255 / 255 |
| red/green/blue shift | 16 / 8 / 0 |

Result: each pixel arrives as bytes `B G R X`, i.e. `u32::from_le_bytes` gives `0x00RRGGBB` directly. All decoders in `rvnc-proto` assume this format in the MVP; a `PixelFormat` type is still parsed and stored so a converter can be added later.

Note on ordering: a `FramebufferUpdate` already in flight when we send `SetPixelFormat` may still use the old format. We send `SetPixelFormat` before our first `FramebufferUpdateRequest`, so no update can precede it. (Strictly, this is how every mainstream client behaves and servers honour it.)

## Client → server messages

| Type | Name | Layout (after u8 type) |
|---|---|---|
| 0 | SetPixelFormat | 3 pad, PIXEL_FORMAT |
| 2 | SetEncodings | 1 pad, u16 count, [i32; count] |
| 3 | FramebufferUpdateRequest | u8 incremental, u16 x, u16 y, u16 w, u16 h |
| 4 | KeyEvent | u8 down, 2 pad, u32 keysym |
| 5 | PointerEvent | u8 button-mask, u16 x, u16 y |
| 6 | ClientCutText | 3 pad, u32 len, Latin-1 text (post-MVP) |

Encoding is a `fn encode(&self, out: &mut Vec<u8>)` per message type in `rvnc-proto`.

## Server → client messages

| Type | Name | MVP handling |
|---|---|---|
| 0 | FramebufferUpdate | 1 pad, u16 n-rects, then n × (u16 x, y, w, h, i32 encoding, payload) |
| 1 | SetColourMapEntries | 1 pad, u16 first, u16 n, n × (u16 r, g, b) — parse & discard (we're true-colour) |
| 2 | Bell | no payload — forward as `SessionEvent::Bell` (app calls `NSBeep` later; log for now) |
| 3 | ServerCutText | 3 pad, u32 len, bytes — parse & discard |

### Decoder design

`rvnc-proto` exposes a pull-style decoder over `std::io::Read` (simple, matches blocking threads) built on small primitives (`read_u8`, `read_u16_be`, `read_exact`). A sans-IO incremental variant (`fn decode(buf: &[u8]) -> Result<Option<(Msg, usize)>>`) is preferable for fuzzing and async; we start with `Read` because it is much simpler for rect payloads that can be tens of MB, and keep the parsing functions pure so the switch is cheap.

For `FramebufferUpdate`, the decoder does not materialise the whole message. It yields rectangles one at a time to a `RectSink`:

```rust
trait RectSink {
    fn raw(&mut self, rect: Rect, pixels: &[u8]);         // w*h*4 bytes, BGRX
    fn copy_rect(&mut self, dst: Rect, src_x: u16, src_y: u16);
    fn desktop_size(&mut self, width: u16, height: u16);
}
```

`rvnc-client` implements `RectSink` on the framebuffer. This keeps memory bounded (one rect's scratch buffer, reused) and keeps decoding logic testable without a framebuffer.

### Encodings

**Raw (0):** `w * h * 4` bytes of pixels, row-major. Copy row by row into the framebuffer at `(x, y)`. Validate the rect is within bounds; out-of-bounds is a protocol error (don't panic, don't write).

**CopyRect (1):** `u16 src_x, u16 src_y`. Copy a `w × h` region within the framebuffer from source to destination. Source and destination may overlap — use `copy_within` per row, iterating rows bottom-to-top when `dst_y > src_y`. Critical for scrolling/window-drag performance even without compression.

**DesktopSize (-223) pseudo-rect:** `w`, `h` of the rect header are the new framebuffer size; no payload. Reallocate the framebuffer (fill black), bump `generation`, emit `SessionEvent::Resized`, and request a full non-incremental update. Per spec it is the last rect in an update, but don't depend on that. *Implementation note:* TigerVNC sometimes sends DesktopSize at the current size around a real resize. A same-size resize keeps the existing contents, to avoid a flash of black.

Encodings we don't advertise must never be received; if one is, fail with a clear error naming the encoding number.

## Update loop

```
after ServerInit:  request(incremental = false, full)
on each complete FramebufferUpdate:
    notify UI
    request(incremental = true, full)
on DesktopSize:    request(incremental = false, full new size)
```

One outstanding request at a time. This is the classic VNC flow-control model and is adequate for LAN use. Latency improvement (ContinuousUpdates + Fence) is a post-MVP item.

## Input messages

- **KeyEvent**: X11 keysyms. Mapping from macOS keys is in [input.md](input.md).
- **PointerEvent**: button-mask bits: 0 = left, 1 = middle, 2 = right, 3/4 = wheel up/down, 5/6 = wheel left/right. Wheel "clicks" are sent as a press event followed by a release event with the same position. Coordinates are clamped to framebuffer bounds.

## Robustness and limits

- Cap allocation sizes derived from the wire: name length (e.g. ≤ 64 KiB), cut-text length (≤ 16 MiB, read & discard in chunks), framebuffer dimensions (≤ 16384 × 16384), rect bounds checked against current framebuffer.
- Every length-driven read uses `read_exact` into bounded buffers; no `Vec::with_capacity(untrusted)` unbounded.
- `rvnc-proto` must never panic on malformed input — enforced by a `cargo fuzz` target on the server-message decoder (post-MVP but cheap to add early).

## Test targets

macOS's built-in Screen Sharing does **not** offer security type None (it requires VNC auth or Apple ARD auth), so it is not an MVP test target. Use instead:

- **TigerVNC `Xvnc` in Docker** with `-SecurityTypes None` (primary target; supports DesktopSize via `xrandr`).
- **x11vnc** with `-nopw`.
- **QEMU** VM with `-vnc :0` (no auth by default; good for raw/CopyRect and resize on guest mode changes).
- **In-process fake server** in tests (see [plan.md](plan.md#testing-strategy)).

VNC Auth (type 2: DES challenge-response, ~50 lines) is the first post-MVP item because it unlocks macOS Screen Sharing (when "VNC viewers may control screen with password" is enabled) and most real-world servers.
