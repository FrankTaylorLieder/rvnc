# Keyboard and mouse handling

Status: draft for MVP · Related: [protocol.md](protocol.md), [ui-framework.md](ui-framework.md)

Input is where VNC clients most often feel broken, so it gets its own doc. RFB sends **X11 keysyms**, which describe the *symbol* a key produces, not the physical key. macOS gives us both a physical key code and the characters produced. We need to bridge the two without stuck keys, doubled shifts or wrong characters.

## Keyboard

### Source events (winit 0.30)

- `WindowEvent::KeyboardInput { event: KeyEvent, .. }`
  - `physical_key: PhysicalKey` — layout-independent key position (`KeyCode::KeyA`, `KeyCode::ShiftLeft`…)
  - `logical_key: Key` — `Key::Named(NamedKey::Enter)` or `Key::Character("a")`, layout and modifier aware
  - `state: Pressed | Released`, `repeat: bool`
- `WindowEvent::ModifiersChanged(Modifiers)` — current modifier state, including left/right distinction.
- `WindowEvent::Focused(bool)`.

### Mapping strategy

1. **Named keys** (`Key::Named`) → fixed table to keysyms, e.g. `Enter → XK_Return (0xff0d)`, `Backspace → XK_BackSpace (0xff08)`, `Tab → 0xff09`, `Escape → 0xff1b`, arrows `0xff51–0xff54`, `Home/End/PageUp/PageDown`, `Delete → 0xffff`, `F1–F12 → 0xffbe–0xffc9`.
2. **Modifiers** are mapped by *physical* key so left/right are preserved:

   | Mac key | Keysym sent | Rationale |
   |---|---|---|
   | Shift L/R | `Shift_L 0xffe1` / `Shift_R 0xffe2` | |
   | Control L/R | `Control_L 0xffe3` / `Control_R 0xffe4` | |
   | Option L/R | `Alt_L 0xffe9` / `Alt_R 0xffea` | Matches PC Alt position |
   | Command L/R | `Super_L 0xffeb` / `Super_R 0xffec` | Windows key on PC; Cmd on Mac servers |
   | Caps Lock | `Caps_Lock 0xffe5` | Sent as press+release on each toggle (macOS reports it as a toggle) |

   A "swap Cmd and Ctrl" option (common request when controlling Linux/Windows from a Mac) is post-MVP but the table is data, so it's trivial.
3. **Printable characters** (`Key::Character(s)`) → keysym of the produced character:
   - U+0020–U+007E and U+00A0–U+00FF: keysym = code point (Latin-1 is identity-mapped in X11).
   - Anything else: keysym = `0x0100_0000 | codepoint` (X11 Unicode keysym convention; supported by TigerVNC, x11vnc, QEMU, etc.).
   - Strings with more than one character (rare — dead-key compositions) are sent as a press/release sequence per char.
4. **Unmapped keys** are logged at `debug` and dropped.

### Press/release pairing (no stuck keys)

The logical key reported on release can differ from the one on press (e.g. press `Shift`, press `2` → `@`, release `Shift`, release `2` → logical key now `2`). If we derived the release keysym independently we'd send "press `@`, release `2`" and leave `@` stuck on the server.

So: keep `pressed: HashMap<PhysicalKey, u32 /*keysym*/>`.

- On press: compute keysym, store under the physical key, send `KeyEvent(down=1)`. If the physical key is already in the map (auto-repeat), resend the *stored* keysym as down (servers handle repeated downs as repeat).
- On release: look up the stored keysym by physical key, send `KeyEvent(down=0)`, remove. If not found, ignore.
- On `Focused(false)` (and on window close): send release for every entry in the map and clear it. This fixes the classic "Cmd-Tab away and Cmd is stuck on the remote" bug.

### Modifiers and produced characters

We send modifier keys as real key events *and* send the shifted character's keysym (e.g. `Shift_L` down, `at` down). This is what TigerVNC and RealVNC viewers do; X servers handle it correctly because the server-side keymap resolves `at` to whatever key+modifier combination it needs.

The awkward case is **Option**: on a Mac, Option+key produces characters (`Option-3` → `#` on UK layouts, `Option-e` → dead acute). Two sub-cases:

- If `logical_key` is a character that differs from the unmodified key, we send the produced character. Remote sees `Alt_L` down + `numbersign` — X servers usually do the right thing; some apps may see Alt+#.
- For shortcuts where the user means Alt+key (e.g. Alt-F in a Linux app), the produced character is wrong. winit exposes `KeyEventExtModifierSupplement::key_without_modifiers()` on macOS — when Ctrl or Cmd is held, use the *unmodified* key's keysym, since those combinations are shortcuts, not text.

MVP rule: **if Ctrl or Cmd is held, use `key_without_modifiers()`; otherwise use `logical_key`.** Revisit with real usage. A "raw scancode" mode using the QEMU Extended Key Event pseudo-encoding (sends physical keycodes; ideal for VMs) is post-MVP.

### Dead keys / IME

MVP ignores IME composition (`WindowEvent::Ime`) and dead-key sequences beyond what `KeyEvent` delivers as completed characters. Composed characters that arrive as text get sent as Unicode keysyms.

### Local shortcuts

All keys go to the remote except a small reserved set with **Cmd-Option** prefix (unlikely to be needed on the remote):

| Shortcut | Action |
|---|---|
| `Cmd-Q` | Quit (macOS convention; also via menu) |
| `Cmd-Option-S` | Toggle Fit / 1:1 scaling |
| `Cmd-Option-F` | Toggle fullscreen |
| `Cmd-Option-Delete` | Send Ctrl-Alt-Del sequence |

Reserved shortcuts are consumed locally (never sent, and never added to `pressed`). Cmd-Q leaking a `Super_L` press is avoided by releasing all pressed keys before quitting.

## Mouse

### Coordinates

- winit `CursorMoved` gives physical pixel positions in the window. Using the shared window→framebuffer mapping from [ui-framework.md](ui-framework.md): `fb_x = (win_x - offset_x) / scale`, clamp to `[0, fb_w-1]`.
- Positions in the letterbox area are clamped to the nearest edge (so you can still reach screen edges).
- Coalesce motion: if several `CursorMoved` arrive before the writer drains the channel, only the latest matters. Implement by keeping the last pointer state in an `AtomicU64`/mutex and sending a "pointer dirty" command, or simply rate-limit to one PointerEvent per ~8 ms. MVP: send every event; optimise if the channel backs up.

### Buttons

Maintain the current `button_mask: u8`. `MouseInput` press/release sets/clears bit 0 (left), 1 (middle), 2 (right). Every change sends a PointerEvent with the current position and mask. Extra buttons (back/forward) are bits 7+ in some extensions — ignored in MVP.

Ctrl-click: macOS users expect Ctrl-click = right-click locally, but on a remote the Ctrl key is already forwarded, and Ctrl-click is meaningful in remote apps. MVP: no translation (right-click via trackpad two-finger click works natively and arrives as `MouseButton::Right`).

### Scrolling

RFB scroll is discrete: a press+release of button 4 (up), 5 (down), 6 (left), 7 (right) — bits 3–6 of the mask.

- `MouseScrollDelta::LineDelta(x, y)` (mouse wheel): send `round(|y|)` clicks in the corresponding direction.
- `MouseScrollDelta::PixelDelta(pos)` (trackpad, momentum): accumulate per axis; each time the accumulator passes a threshold (start with ~ 30 logical px; tune) emit one click and subtract. This prevents trackpads producing hundreds of clicks per swipe.
- Respect macOS "natural scrolling": winit already reports deltas with the user's direction preference applied, so sending them as-is makes the remote scroll the way the user's Mac does.

### Cursor display

MVP: the remote's cursor is drawn into the framebuffer by the server (most servers do this when the Cursor pseudo-encoding is not negotiated), and we keep the local cursor visible as a normal arrow. This doubles cursors with a little lag on the remote one — acceptable for MVP. Post-MVP: Cursor pseudo-encoding (-239) to get the remote cursor shape, hide local cursor while over the view, and set it as the local cursor image for zero-latency feel.

## Test checklist (manual, per milestone)

- Type `Hello, World! @#£€` into a remote terminal/editor on a UK and US layout.
- Shift-arrow selection, Ctrl-C/Ctrl-V in a Linux terminal, Alt-Tab on remote.
- Hold a key: auto-repeat on remote, no stuck key after release.
- Cmd-Tab away mid-keypress and back: no stuck modifiers on remote (`xev` on the server).
- Left/middle/right click, drag-select text, double-click.
- Mouse-wheel and trackpad scroll in a browser: reasonable speed both axes.
- Fit mode with letterboxing: clicking corners of the remote screen hits the corners.
