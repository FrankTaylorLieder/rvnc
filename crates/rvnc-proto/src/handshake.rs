//! Handshake messages (RFC 6143 §7.1–7.3).

use std::fmt;
use std::io::Read;

use crate::wire::{read_string, read_u8, read_u16, read_u32};
use crate::{MAX_DIMENSION, MAX_STRING_LEN, PixelFormat, ProtoError};

/// Protocol versions rvnc can speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Version {
    V3_3,
    V3_7,
    V3_8,
}

impl Version {
    pub const WIRE_LEN: usize = 12;

    /// The 12-byte version string we send for this version.
    pub fn wire(&self) -> &'static [u8; 12] {
        match self {
            Version::V3_3 => b"RFB 003.003\n",
            Version::V3_7 => b"RFB 003.007\n",
            Version::V3_8 => b"RFB 003.008\n",
        }
    }

    /// Parses a server's `ProtocolVersion` message, returning its
    /// `(major, minor)` numbers.
    pub fn parse_server(bytes: &[u8; 12]) -> Result<(u16, u16), ProtoError> {
        let bad = || ProtoError::BadVersion(String::from_utf8_lossy(bytes).into_owned());
        if &bytes[..4] != b"RFB " || bytes[7] != b'.' || bytes[11] != b'\n' {
            return Err(bad());
        }
        let num = |s: &[u8]| -> Result<u16, ProtoError> {
            if !s.iter().all(u8::is_ascii_digit) {
                return Err(bad());
            }
            // Three ASCII digits always fit in u16.
            Ok(s.iter().fold(0u16, |acc, d| acc * 10 + u16::from(d - b'0')))
        };
        Ok((num(&bytes[4..7])?, num(&bytes[8..11])?))
    }

    /// Picks the highest version we support that does not exceed the server's.
    ///
    /// Per RFC 6143 §7.1.1, unknown minor versions below 3.7 are treated as
    /// 3.3. Apple's "3.889" and anything newer than 3.8 is answered with 3.8.
    pub fn negotiate(major: u16, minor: u16) -> Result<Version, ProtoError> {
        match (major, minor) {
            (3, 0..=2) | (0..=2, _) => Err(ProtoError::UnsupportedVersion { major, minor }),
            (3, 3..=6) => Ok(Version::V3_3),
            (3, 7) => Ok(Version::V3_7),
            _ => Ok(Version::V3_8),
        }
    }

    pub fn read_server(r: &mut impl Read) -> Result<(u16, u16), ProtoError> {
        let mut buf = [0u8; 12];
        r.read_exact(&mut buf)?;
        Self::parse_server(&buf)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Version::V3_3 => "3.3",
            Version::V3_7 => "3.7",
            Version::V3_8 => "3.8",
        };
        f.write_str(s)
    }
}

/// Security type numbers (RFC 6143 §7.2, IANA registry).
pub mod security {
    pub const INVALID: u8 = 0;
    pub const NONE: u8 = 1;
    pub const VNC_AUTH: u8 = 2;
    pub const RA2: u8 = 5;
    pub const RA2NE: u8 = 6;
    pub const TIGHT: u8 = 16;
    pub const VENCRYPT: u8 = 19;
    pub const APPLE_ARD: u8 = 30;

    pub fn name(t: u8) -> &'static str {
        match t {
            NONE => "None",
            VNC_AUTH => "VNC Authentication",
            RA2 => "RA2",
            RA2NE => "RA2ne",
            TIGHT => "Tight",
            VENCRYPT => "VeNCrypt",
            APPLE_ARD => "Apple Remote Desktop",
            _ => "unknown",
        }
    }
}

/// The server's offer of security types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityOffer {
    /// The server offers these types; the client must choose one (3.7+), or
    /// the server has already chosen the single type (3.3).
    Types(Vec<u8>),
    /// The server refused the connection.
    Failed(String),
}

/// Reads the server's security offer for the negotiated version.
pub fn read_security_offer(
    r: &mut impl Read,
    version: Version,
) -> Result<SecurityOffer, ProtoError> {
    match version {
        Version::V3_3 => {
            let t = read_u32(r)?;
            if t == u32::from(security::INVALID) {
                return Ok(SecurityOffer::Failed(read_reason(r)?));
            }
            // 3.3 only defines 1 and 2; anything else is out of range for u8.
            let t = u8::try_from(t).unwrap_or(security::INVALID);
            Ok(SecurityOffer::Types(vec![t]))
        }
        Version::V3_7 | Version::V3_8 => {
            let n = read_u8(r)?;
            if n == 0 {
                return Ok(SecurityOffer::Failed(read_reason(r)?));
            }
            let mut types = vec![0u8; usize::from(n)];
            r.read_exact(&mut types)?;
            Ok(SecurityOffer::Types(types))
        }
    }
}

/// Reads a `SecurityResult`. Returns `Err(reason)` inside `Ok` on failure.
///
/// `version` controls whether a failure carries a reason string (3.8 only).
pub fn read_security_result(
    r: &mut impl Read,
    version: Version,
) -> Result<Result<(), String>, ProtoError> {
    match read_u32(r)? {
        0 => Ok(Ok(())),
        _ if version == Version::V3_8 => Ok(Err(read_reason(r)?)),
        _ => Ok(Err("authentication failed".to_owned())),
    }
}

/// Reads a `u32`-length-prefixed failure reason.
pub fn read_reason(r: &mut impl Read) -> Result<String, ProtoError> {
    read_string(r, "failure reason", MAX_STRING_LEN)
}

/// The `ServerInit` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInit {
    pub width: u16,
    pub height: u16,
    pub pixel_format: PixelFormat,
    pub name: String,
}

impl ServerInit {
    pub fn read(r: &mut impl Read) -> Result<Self, ProtoError> {
        let width = read_u16(r)?;
        let height = read_u16(r)?;
        check_size(width, height)?;
        let pixel_format = PixelFormat::read(r)?;
        let name = read_string(r, "desktop name", MAX_STRING_LEN)?;
        Ok(Self {
            width,
            height,
            pixel_format,
            name,
        })
    }
}

/// Validates a framebuffer size received from a server.
///
/// Zero-sized framebuffers are allowed (some servers start headless).
pub fn check_size(width: u16, height: u16) -> Result<(), ProtoError> {
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(ProtoError::BadSize { width, height });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_versions() {
        assert_eq!(Version::parse_server(b"RFB 003.008\n").unwrap(), (3, 8));
        assert_eq!(Version::parse_server(b"RFB 003.889\n").unwrap(), (3, 889));
        assert!(Version::parse_server(b"RFB 003.00x\n").is_err());
        assert!(Version::parse_server(b"HTTP/1.1 200").is_err());
    }

    #[test]
    fn negotiate() {
        assert_eq!(Version::negotiate(3, 3).unwrap(), Version::V3_3);
        assert_eq!(Version::negotiate(3, 5).unwrap(), Version::V3_3);
        assert_eq!(Version::negotiate(3, 7).unwrap(), Version::V3_7);
        assert_eq!(Version::negotiate(3, 8).unwrap(), Version::V3_8);
        assert_eq!(Version::negotiate(3, 889).unwrap(), Version::V3_8);
        assert_eq!(Version::negotiate(4, 1).unwrap(), Version::V3_8);
        assert!(Version::negotiate(3, 2).is_err());
        assert!(Version::negotiate(2, 0).is_err());
    }

    #[test]
    fn security_offer_38() {
        let buf = [2u8, 1, 2];
        assert_eq!(
            read_security_offer(&mut buf.as_slice(), Version::V3_8).unwrap(),
            SecurityOffer::Types(vec![1, 2])
        );
        let buf = [0u8, 0, 0, 0, 3, b'b', b'a', b'd'];
        assert_eq!(
            read_security_offer(&mut buf.as_slice(), Version::V3_8).unwrap(),
            SecurityOffer::Failed("bad".into())
        );
    }

    #[test]
    fn security_offer_33() {
        let buf = [0u8, 0, 0, 1];
        assert_eq!(
            read_security_offer(&mut buf.as_slice(), Version::V3_3).unwrap(),
            SecurityOffer::Types(vec![1])
        );
        let buf = [0u8, 0, 0, 0, 0, 0, 0, 2, b'n', b'o'];
        assert_eq!(
            read_security_offer(&mut buf.as_slice(), Version::V3_3).unwrap(),
            SecurityOffer::Failed("no".into())
        );
    }

    #[test]
    fn security_result() {
        let ok = [0u8; 4];
        assert_eq!(
            read_security_result(&mut ok.as_slice(), Version::V3_8).unwrap(),
            Ok(())
        );
        let fail = [0u8, 0, 0, 1, 0, 0, 0, 1, b'x'];
        assert_eq!(
            read_security_result(&mut fail.as_slice(), Version::V3_8).unwrap(),
            Err("x".into())
        );
        assert!(
            read_security_result(&mut fail.as_slice(), Version::V3_7)
                .unwrap()
                .is_err()
        );
    }

    #[test]
    fn server_init() {
        let mut buf = vec![0x07, 0x80, 0x04, 0x38];
        PixelFormat::rvnc_default().encode(&mut buf);
        buf.extend_from_slice(&[0, 0, 0, 4]);
        buf.extend_from_slice(b"desk");
        let si = ServerInit::read(&mut buf.as_slice()).unwrap();
        assert_eq!((si.width, si.height), (1920, 1080));
        assert_eq!(si.name, "desk");
    }

    #[test]
    fn server_init_rejects_huge_name() {
        let mut buf = vec![0, 10, 0, 10];
        PixelFormat::rvnc_default().encode(&mut buf);
        buf.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
        assert!(matches!(
            ServerInit::read(&mut buf.as_slice()),
            Err(ProtoError::TooLong { .. })
        ));
    }

    #[test]
    fn server_init_rejects_huge_size() {
        let mut buf = vec![0xff, 0xff, 0, 10];
        PixelFormat::rvnc_default().encode(&mut buf);
        buf.extend_from_slice(&[0, 0, 0, 0]);
        assert!(matches!(
            ServerInit::read(&mut buf.as_slice()),
            Err(ProtoError::BadSize { .. })
        ));
    }
}
