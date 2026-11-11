# rvnc

A VNC (RFB) viewer for macOS, written in Rust.

**Status:** MVP. Connects to servers that allow security type *None* (no
password), using Raw and CopyRect encodings. See [docs/rnvc.md](docs/rnvc.md)
for the design and roadmap.

## Install

Requires Rust 1.88 or later. Installs the `rvnc` binary into `~/.cargo/bin`
(which should be on your `PATH`):

```sh
cargo install --git https://github.com/FrankTaylorLieder/rvnc rvnc-app
rvnc localhost:5901
```

To uninstall (use the package name `rvnc-app`, not the binary name):

```sh
cargo uninstall rvnc-app
```

## Build and run from source

```sh
cargo build --release
./target/release/rvnc <host>[:port] [--port PORT | --display N] [--view-only] [--scale fit|1:1] [--exclusive]
```

To install from a local checkout:

```sh
cargo install --path crates/rvnc-app
rvnc localhost:5901
```

Address forms: `host` (port 5900), `host:5901` (port 5901, as in a URL),
`[fe80::1]:5901` for IPv6. To choose the port with an option instead:

- `--port P` (`-p`) connects to port P.
- `--display N` (`-d`) connects to VNC display N, which is port 5900 + N.
  For example, `rvnc myhost -d 1` connects to port 5901.

Either option overrides a port given in the address. They can't be combined.

Unlike TigerVNC and RealVNC, rvnc reads the number after a single colon as a
port, not a display number, and doesn't accept `host::port`.

Logging: `RUST_LOG=debug ./target/release/rvnc ...` (`trace` on
`rvnc_client` logs every update; on `rvnc::app` every frame).

## Keyboard shortcuts

All keys go to the remote except:

| Shortcut | Action |
|---|---|
| Cmd-Option-S | Toggle Fit / 1:1 scaling |
| Cmd-Option-F | Toggle fullscreen |
| Cmd-Option-Delete | Send Ctrl-Alt-Del |
| Cmd-Q, Cmd-H, Cmd-Option-H | Quit / hide / hide others (macOS app menu) |

Mac keys map as Option → Alt, Cmd → Super. With Ctrl or Cmd held, the
unmodified key is sent, so shortcuts work. Otherwise the character macOS
produces is sent, so Option-3 gives `#` on a UK layout.

## Development servers

`dev/` has a Docker image running two password-less servers, bound to
localhost only:

```sh
docker compose -f dev/compose.yaml up -d --build
cargo run --release -p rvnc-app -- localhost:5901   # TigerVNC Xvnc (display 1)
cargo run --release -p rvnc-app -- localhost:5902   # x11vnc on Xvfb (display 2)
docker exec rvnc-dev-vnc-1 sh -c 'DISPLAY=:1 xrandr -s 1024x768'  # remote resize
```

A headless smoke test that saves the remote screen to a PNG:

```sh
cargo run --release -p rvnc-client --example screenshot -- localhost:5901 out.png
```

## Tests

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --release -p rvnc-app -- --ignored --nocapture bench   # render micro-benchmark
cd crates/rvnc-proto && cargo +nightly fuzz run server_messages     # needs cargo-fuzz
```

## Layout

| Crate | Role |
|---|---|
| `crates/rvnc-proto` | RFB wire format: handshake, messages, rectangle decoding. No I/O. |
| `crates/rvnc-client` | Session: TCP, handshake, reader/writer threads, framebuffer. No UI. |
| `crates/rvnc-app` | The `rvnc` binary: winit window, softbuffer rendering, input mapping. |

## Known limitations

- Servers that require a password don't work yet, including macOS Screen
  Sharing. VNC password auth is next on the roadmap.
- Only Raw and CopyRect encodings are supported, so it's bandwidth-hungry and
  meant for LAN use.
- Scaling runs on the CPU via softbuffer, which costs about 11 ms per frame
  for a full Retina window. That caps rendering at roughly 60–80 fps.
- The remote cursor is drawn by the server, so you see two cursors.
- No clipboard sync. Dead keys and IME composition are not forwarded.
- macOS reserves some shortcuts (Cmd-Tab, Cmd-Space, Mission Control), and
  they never reach the remote.

## Licence

MIT OR Apache-2.0
