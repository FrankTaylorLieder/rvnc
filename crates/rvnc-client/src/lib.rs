//! RFB (VNC) client session: TCP connection, handshake, reader/writer
//! threads and the shared framebuffer. Has no UI dependencies.
//!
//! See `docs/architecture.md` for the threading model.

mod address;
mod error;
mod framebuffer;
mod handshake;
mod session;

pub use address::{DEFAULT_PORT, parse_address};
pub use error::ConnectError;
pub use framebuffer::Framebuffer;
pub use handshake::{NoneSecurity, SecurityHandler};
pub use rvnc_proto as proto;
pub use rvnc_proto::Rect;
pub use session::{
    ConnectOptions, EventSink, InputSender, ServerInfo, Session, SessionEvent, connect,
    connect_stream,
};
