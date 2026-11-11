use std::io::{Read, Write};

use rvnc_proto::client::ClientMessage;
use rvnc_proto::handshake::{
    SecurityOffer, ServerInit, Version, read_security_offer, read_security_result, security,
};
use rvnc_proto::{PixelFormat, encoding};
use tracing::{debug, info};

use crate::ConnectError;

/// A security type implementation (RFC 6143 §7.2).
pub trait SecurityHandler: Send {
    fn type_id(&self) -> u8;

    /// Runs the type-specific exchange after the type has been chosen and
    /// before `SecurityResult`.
    fn authenticate(
        &mut self,
        stream: &mut dyn ReadWrite,
        version: Version,
    ) -> Result<(), ConnectError>;
}

/// Object-safe `Read + Write`.
pub trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// Security type 1: no authentication.
#[derive(Debug, Default)]
pub struct NoneSecurity;

impl SecurityHandler for NoneSecurity {
    fn type_id(&self) -> u8 {
        security::NONE
    }

    fn authenticate(&mut self, _: &mut dyn ReadWrite, _: Version) -> Result<(), ConnectError> {
        Ok(())
    }
}

pub(crate) struct Negotiated {
    pub version: Version,
    pub init: ServerInit,
}

/// Runs the handshake from `ProtocolVersion` through `ServerInit`, then
/// sends our pixel format and encodings.
pub(crate) fn run(
    stream: &mut (impl Read + Write),
    handlers: &mut [Box<dyn SecurityHandler>],
    shared: bool,
    encodings: &[i32],
) -> Result<Negotiated, ConnectError> {
    let (major, minor) = Version::read_server(stream)?;
    let version = Version::negotiate(major, minor)?;
    info!("server speaks RFB {major}.{minor}; using {version}");
    stream.write_all(version.wire())?;

    let offered = match read_security_offer(stream, version)? {
        SecurityOffer::Failed(reason) => return Err(ConnectError::Refused(reason)),
        SecurityOffer::Types(types) => types,
    };
    debug!(?offered, "security types offered");

    let handler = handlers
        .iter_mut()
        .find(|h| offered.contains(&h.type_id()))
        .ok_or_else(|| ConnectError::NoSupportedSecurity(offered.clone()))?;
    let chosen = handler.type_id();
    if version >= Version::V3_7 {
        stream.write_all(&[chosen])?;
    }
    info!("using security type {} ({chosen})", security::name(chosen));
    handler.authenticate(stream, version)?;

    // 3.8 always sends SecurityResult; earlier versions omit it for None.
    if version == Version::V3_8 || chosen != security::NONE {
        if let Err(reason) = read_security_result(stream, version)? {
            return Err(ConnectError::AuthFailed(reason));
        }
    }

    stream.write_all(&[u8::from(shared)])?;
    let init = ServerInit::read(stream)?;
    info!(
        name = %init.name,
        width = init.width,
        height = init.height,
        server_format = ?init.pixel_format,
        "connected"
    );

    let mut out = Vec::new();
    ClientMessage::SetPixelFormat(PixelFormat::rvnc_default()).encode(&mut out);
    ClientMessage::SetEncodings(encodings.to_vec()).encode(&mut out);
    stream.write_all(&out)?;
    stream.flush()?;

    Ok(Negotiated { version, init })
}

/// Encodings to advertise by default.
pub(crate) fn default_encodings() -> Vec<i32> {
    encoding::SUPPORTED.to_vec()
}
