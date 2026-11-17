# rvnc

A VNC (RFB) viewer for macOS, written in Rust.

**Status:** MVP plus password auth. Connects to servers with no password, a
VNC password, or a macOS account (Screen Sharing), using Raw and CopyRect
encodings. See [docs/rnvc.md](docs/rnvc.md) for the design and roadmap.

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
                      [--user NAME] [--password-file PATH] [--save-password | --no-keychain]
                      [--forget-password] [--security none|vnc|ard]
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

## Passwords

rvnc supports VNC passwords (TigerVNC, x11vnc, QEMU and others) and macOS
account logins (Apple Remote Desktop auth, used by macOS Screen Sharing). It
only asks for a password if the server requires one:

```sh
rvnc myhost                          # prompts: VNC password for myhost:5900
rvnc mymac --user alice              # macOS Screen Sharing as alice
rvnc myhost --save-password          # save in the keychain after logging in
rvnc myhost                          # next time, uses the saved password
rvnc myhost --forget-password        # remove saved passwords for myhost:5900
```

Passwords come from `--password-file PATH` (or `-` for stdin), or
`RVNC_PASSWORD`, then the keychain, then a terminal prompt. If a password is
typed wrong, rvnc asks again, up to 3 times. If a saved password stops working,
rvnc prompts and replaces it with the new one once that works. Saved passwords
appear in Keychain Access under names starting `rvnc:`.

If a Mac offers both kinds of login, rvnc uses the VNC password, unless you give
`--user`. `--security vnc|ard|none` forces a choice.

Neither kind of login encrypts the session itself. VNC passwords in
particular are weak: only the first 8 characters count. Use them on a trusted
network or over an SSH tunnel. Details: [docs/auth.md](docs/auth.md).

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

`dev/` has a Docker image running four servers, bound to localhost only. Ports
5901 and 5902 have no password; 5903 and 5904 use the VNC password `rvnc-dev`:

```sh
docker compose -f dev/compose.yaml up -d --build
cargo run --release -p rvnc-app -- localhost:5901   # TigerVNC Xvnc (display 1)
cargo run --release -p rvnc-app -- localhost:5902   # x11vnc on Xvfb (display 2)
cargo run --release -p rvnc-app -- localhost:5903   # TigerVNC with VNC password
cargo run --release -p rvnc-app -- localhost:5904   # x11vnc with VNC password
docker exec rvnc-dev-vnc-1 sh -c 'DISPLAY=:1 xrandr -s 1024x768'  # remote resize
```

A headless smoke test that saves the remote screen to a PNG:

```sh
cargo run --release -p rvnc-client --example screenshot -- localhost:5901 out.png
RVNC_PASSWORD=rvnc-dev cargo run --release -p rvnc-client --example screenshot -- localhost:5903 out.png
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

- No TLS or VeNCrypt, so TigerVNC servers set up for TLS-only or
  username/password (`Plain`) logins don't work yet.
- With a locally built binary, macOS may ask to allow keychain access again
  after each rebuild.
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
