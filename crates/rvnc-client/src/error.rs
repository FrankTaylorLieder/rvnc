use rvnc_proto::ProtoError;
use rvnc_proto::handshake::security;

/// Errors establishing a session.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    // `source` is reported through the error chain, not repeated here.
    #[error("could not connect to {addr}")]
    Connect {
        addr: String,
        source: std::io::Error,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("protocol error during handshake: {0}")]
    Proto(#[from] ProtoError),

    #[error("server refused the connection: {0}")]
    Refused(String),

    #[error("server requires a security type rvnc does not support yet (offered: {})", describe(.0))]
    NoSupportedSecurity(Vec<u8>),

    #[error("authentication failed: {0}")]
    AuthFailed(String),
}

fn describe(types: &[u8]) -> String {
    types
        .iter()
        .map(|t| format!("{} ({t})", security::name(*t)))
        .collect::<Vec<_>>()
        .join(", ")
}
