use crate::Rect;

/// Errors produced while encoding or decoding RFB messages.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid protocol version string {0:?}")]
    BadVersion(String),

    #[error("unsupported protocol version {major}.{minor}")]
    UnsupportedVersion { major: u16, minor: u16 },

    #[error("unknown server message type {0}")]
    UnknownMessage(u8),

    #[error("server sent encoding {0}, which was not requested")]
    UnsupportedEncoding(i32),

    #[error("rectangle {rect:?} lies outside the {width}x{height} framebuffer")]
    RectOutOfBounds { rect: Rect, width: u16, height: u16 },

    #[error("{what} length {len} exceeds limit of {limit}")]
    TooLong {
        what: &'static str,
        len: u64,
        limit: u64,
    },

    #[error("invalid framebuffer size {width}x{height}")]
    BadSize { width: u16, height: u16 },

    #[error("unsupported pixel format: {0}")]
    BadPixelFormat(&'static str),
}

impl ProtoError {
    /// True if this error is the peer closing the connection.
    pub fn is_eof(&self) -> bool {
        matches!(self, ProtoError::Io(e) if e.kind() == std::io::ErrorKind::UnexpectedEof)
    }
}
