# Authentication

rvnc supports three RFB security types:

| Type | Id | Servers | Credentials |
|---|---|---|---|
| None | 1 | Servers with no password | — |
| VNC Authentication | 2 | TigerVNC, x11vnc, QEMU (`password=on`), RealVNC in legacy mode, NAS boxes and routers. macOS Screen Sharing when "VNC viewers may control screen with password" is on. | Password; only the first 8 bytes count |
| Apple Remote Desktop | 30 | macOS Screen Sharing | macOS account username and password |

Not supported yet: VeNCrypt and TLS (19), which come with the TLS work; RealVNC RA2 (5/6); UltraVNC MS-Logon; and Apple's newer undocumented types 33, 35 and 36. macOS offers type 30 alongside those.

**Security warning.** Neither type encrypts the session.
- VNC Authentication sends a DES-encrypted challenge response. A captured exchange can be brute-forced offline, because the key is at most 8 bytes.
- ARD encrypts the credentials, but not the screen or keystrokes that follow.

Use them on a trusted LAN or over an SSH tunnel.

## Choosing a type

The server lists the types it offers (or, with RFB 3.3, picks one itself). rvnc uses the first type in its preference order that the server also offers:

- By default the order is None, VNC Authentication, then ARD.
- With `--user`, the order is None, ARD, then VNC Authentication. This matters for Macs that offer both.
- `--security none|vnc|ard` allows only that one type.

## Wire formats

**VNC Authentication** (RFC 6143 §7.2.2):

1. The server sends a 16-byte random challenge.
2. The client replies with the challenge encrypted using DES-ECB, as two 8-byte blocks. The key is the password, truncated or zero-padded to 8 bytes, with **the bits of each byte reversed**. This quirk comes from the original implementation, and every server copies it.

**Apple Remote Desktop** (type 30):

1. The server sends `generator: u16`, `key_len: u16`, `prime[key_len]` and `server_public[key_len]`. macOS uses a 1024-bit group, with key_len = 128. rvnc rejects key_len > 1024.
2. The client builds its Diffie-Hellman key pair:
   - The private key is random, with `key_len` bytes.
   - `public = g^x mod p`.
   - `shared = server_public^x mod p`, left-padded to `key_len` bytes.
3. The AES-128 key is `MD5(shared)`.
4. It fills a 128-byte block with random bytes. The username goes at offset 0 and the password at offset 64. Each is NUL-terminated, so each can be at most 63 bytes.
5. It encrypts the block with AES-128-ECB.
6. The client sends the 128-byte ciphertext, followed by `public` (padded to `key_len`).

Then `SecurityResult` follows as usual. With 3.8 a failure carries a reason; with 3.3 and 3.7 it doesn't, and rvnc reports "the server gave no reason".

## Code layout

| Where | What |
|---|---|
| `rvnc-proto/src/auth.rs` | `vnc_auth_response`, `ArdParams::read`, `ard_response`. Pure functions: the caller supplies the random bytes, so they are deterministic under test. The DES test vector was cross-checked with `openssl enc -des-ecb`. |
| `rvnc-client/src/credentials.rs` | `CredentialProvider` trait, `CredentialRequest { security_type, username }`, `Credentials`, `NoCredentials` (the default) |
| `rvnc-client/src/handshake.rs` | `VncAuth` and `AppleAuth` handlers. They ask the provider only once the server has asked for authentication. Randomness comes from `getrandom`. |
| `rvnc-app/src/auth.rs` | The app's provider: password sources, prompting, retry and save policy |
| `rvnc-app/src/keychain.rs` | macOS keychain access (`security-framework`), with stubs on other platforms |

Passwords are held in `Zeroizing` buffers, which are wiped on drop. `Credentials`' `Debug` output redacts the password.

## Where passwords come from

The app checks these in order, stopping at the first one that gives a password:

1. **`--password-file PATH`**, or `-` for stdin, with one trailing newline dropped. Otherwise the **`RVNC_PASSWORD`** environment variable. There is deliberately no `--password` option, because command lines are visible in `ps` and shell history.
2. **The keychain**, unless `--no-keychain` is given.
3. **A terminal prompt** with echo turned off (`rpassword`). This only happens when stdin is a terminal and no explicit password was given.

If none of these gives a password, the connection fails with a message naming the options.

**ARD username.** For ARD the username comes from:
1. `--user`.
2. Otherwise the first saved account in the keychain for that server.
3. Otherwise a prompt that defaults to `$USER`.
4. Without a terminal, `$USER` is used.

## Keychain

Passwords are stored as generic passwords in the login keychain, so they show up in Keychain Access:

| Kind | Service (shown as "Name") | Account |
|---|---|---|
| VNC password | `rvnc: host:port (VNC password)` | `vnc` |
| macOS account | `rvnc: host:port (macOS account)` | the username |

- **`--save-password`** saves the password after a successful login.
- **A rejected saved password** isn't saved over automatically. Instead rvnc prompts, and if the new password works it replaces the saved one, even without `--save-password`.
- **`--forget-password`** removes every saved password for the server, then exits.
- **`--no-keychain`** neither reads nor writes the keychain.

macOS ties each keychain item's access list to the program that created it. A locally built `rvnc` binary is unsigned, so every rebuild counts as a new program. macOS may then ask whether to allow access the first time; "Always Allow" stops the question until the next rebuild.

## Failure and retry

After a failed login, the RFB server closes the connection, so trying again means reconnecting. rvnc retries when one of these is true:

- the password came from the keychain and a terminal is available, so it can prompt instead, or
- the password was typed in, and fewer than 3 prompts have been made.

A wrong `--password-file` or `RVNC_PASSWORD` is final.

TigerVNC and x11vnc temporarily block an address after several failures, so rvnc doesn't retry beyond that limit.

## Testing

- **Unit tests:** the DES known answer; an ARD round trip, where the test plays the server, decrypts the block and checks the fields; parameter and length limits; and the app's provider policy.
- **Fake-server tests** (`rvnc-client/tests/session.rs`):
  - VNC Authentication with the right password, the wrong password, and no password.
  - The 3.3 server-chosen type.
  - None preferred over password types.
  - ARD framing.
- **Dev servers:** `dev/compose.yaml` runs password-protected TigerVNC on port 5903 and x11vnc on port 5904, both with password `rvnc-dev`.
- **ARD** has been confirmed working against a real Mac running Screen Sharing (2026-11-17).
