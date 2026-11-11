//! Encoding and pseudo-encoding numbers (RFC 6143 §7.7, rfbproto).

pub const RAW: i32 = 0;
pub const COPY_RECT: i32 = 1;
pub const RRE: i32 = 2;
pub const HEXTILE: i32 = 5;
pub const TIGHT: i32 = 7;
pub const TRLE: i32 = 15;
pub const ZRLE: i32 = 16;

pub const DESKTOP_SIZE: i32 = -223;
pub const LAST_RECT: i32 = -224;
pub const CURSOR: i32 = -239;
pub const QEMU_EXTENDED_KEY_EVENT: i32 = -258;
pub const EXTENDED_DESKTOP_SIZE: i32 = -308;

/// Encodings the MVP decoder understands, in order of preference.
pub const SUPPORTED: &[i32] = &[COPY_RECT, RAW, DESKTOP_SIZE];
