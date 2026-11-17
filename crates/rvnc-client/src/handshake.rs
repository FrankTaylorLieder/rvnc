use std::io::{Read, Write};

use rvnc_proto::auth::{
    ARD_CREDENTIALS_LEN, ArdParams, VNC_CHALLENGE_LEN, VNC_PASSWORD_MAX, ard_response,
    vnc_auth_response,
};
use rvnc_proto::client::ClientMessage;
use rvnc_proto::handshake::{
    SecurityOffer, ServerInit, Version, read_security_offer, read_security_result, security,
};
use rvnc_proto::{PixelFormat, encoding};
use tracing::{debug, info, warn};
use zeroize::Zeroizing;

use crate::ConnectError;
use crate::credentials::{CredentialProvider, CredentialRequest, Credentials};

/// A security type implementation (RFC 6143 §7.2).
pub trait SecurityHandler: Send {
    fn type_id(&self) -> u8;

    /// Runs the type-specific exchange after the type has been chosen and
    /// before `SecurityResult`. Asks `credentials` for a password if needed.
    fn authenticate(
        &mut self,
        stream: &mut dyn ReadWrite,
        version: Version,
        credentials: &mut dyn CredentialProvider,
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

    fn authenticate(
        &mut self,
        _: &mut dyn ReadWrite,
        _: Version,
        _: &mut dyn CredentialProvider,
    ) -> Result<(), ConnectError> {
        Ok(())
    }
}

/// Security type 2: VNC Authentication (DES challenge-response).
///
/// Only the first 8 bytes of the password are used, and nothing after the
/// handshake is encrypted.
#[derive(Debug, Default)]
pub struct VncAuth;

impl SecurityHandler for VncAuth {
    fn type_id(&self) -> u8 {
        security::VNC_AUTH
    }

    fn authenticate(
        &mut self,
        stream: &mut dyn ReadWrite,
        _: Version,
        credentials: &mut dyn CredentialProvider,
    ) -> Result<(), ConnectError> {
        let mut challenge = [0u8; VNC_CHALLENGE_LEN];
        stream.read_exact(&mut challenge)?;
        let creds = ask(credentials, security::VNC_AUTH, false)?;
        if creds.password.len() > VNC_PASSWORD_MAX {
            warn!(
                "VNC Authentication uses only the first {VNC_PASSWORD_MAX} bytes of the password"
            );
        }
        let response = vnc_auth_response(&challenge, creds.password.as_bytes());
        stream.write_all(&response)?;
        stream.flush()?;
        Ok(())
    }
}

/// Security type 30: Apple Remote Desktop (Diffie-Hellman key agreement,
/// then AES-128 encrypted username and password). Used by macOS Screen
/// Sharing with a macOS account. Only the credentials are encrypted.
#[derive(Debug, Default)]
pub struct AppleAuth;

impl SecurityHandler for AppleAuth {
    fn type_id(&self) -> u8 {
        security::APPLE_ARD
    }

    fn authenticate(
        &mut self,
        stream: &mut dyn ReadWrite,
        _: Version,
        credentials: &mut dyn CredentialProvider,
    ) -> Result<(), ConnectError> {
        let params = ArdParams::read(&mut &mut *stream)?;
        debug!(
            generator = params.generator,
            key_bits = params.key_len() * 8,
            "ARD Diffie-Hellman parameters"
        );
        let creds = ask(credentials, security::APPLE_ARD, true)?;
        let username = creds.username.as_deref().map_or("", String::as_str);

        let mut private = Zeroizing::new(vec![0u8; params.key_len()]);
        random(&mut private)?;
        let mut padding = Zeroizing::new([0u8; ARD_CREDENTIALS_LEN]);
        random(padding.as_mut_slice())?;

        let reply = ard_response(
            &params,
            username.as_bytes(),
            creds.password.as_bytes(),
            &private,
            &padding,
        )?;
        stream.write_all(&reply)?;
        stream.flush()?;
        Ok(())
    }
}

fn ask(
    provider: &mut dyn CredentialProvider,
    security_type: u8,
    username: bool,
) -> Result<Credentials, ConnectError> {
    provider
        .credentials(&CredentialRequest {
            security_type,
            username,
        })
        .map_err(|reason| ConnectError::NoCredentials {
            security_type,
            reason,
        })
}

fn random(buf: &mut [u8]) -> Result<(), ConnectError> {
    getrandom::fill(buf)
        .map_err(|e| ConnectError::Io(std::io::Error::other(format!("random numbers: {e}"))))
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
    credentials: &mut dyn CredentialProvider,
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
    handler.authenticate(stream, version, credentials)?;

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
