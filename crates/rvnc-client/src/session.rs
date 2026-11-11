use std::io::{BufReader, BufWriter, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rvnc_proto::client::ClientMessage;
use rvnc_proto::handshake::Version;
use rvnc_proto::server::{Decoder, RectSink, ServerMessage};
use rvnc_proto::{ProtoError, Rect};
use tracing::{debug, info, trace, warn};

use crate::handshake::{self, NoneSecurity, SecurityHandler};
use crate::{ConnectError, Framebuffer};

/// Options for [`connect`].
pub struct ConnectOptions {
    /// Ask the server to leave other viewers connected.
    pub shared: bool,
    pub connect_timeout: Duration,
    /// Read timeout applied only during the handshake.
    pub handshake_timeout: Duration,
    /// Encodings to advertise, in preference order.
    pub encodings: Vec<i32>,
    /// Security types we can perform, in preference order.
    pub security: Vec<Box<dyn SecurityHandler>>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            shared: true,
            connect_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(10),
            encodings: handshake::default_encodings(),
            security: vec![Box::new(NoneSecurity)],
        }
    }
}

/// Facts about the server learned during the handshake.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub version: Version,
}

/// Notifications from the session to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// A complete framebuffer update was applied. Check
    /// [`Framebuffer::take_dirty`] for the changed region.
    FramebufferUpdated,
    /// The server changed the desktop size.
    Resized {
        width: u16,
        height: u16,
    },
    Bell,
    /// The session has ended. `error` is `None` if we closed it ourselves.
    Disconnected {
        error: Option<String>,
    },
}

/// Receives [`SessionEvent`]s on the reader thread.
pub trait EventSink: Send + 'static {
    fn send(&self, event: SessionEvent);
}

impl<F: Fn(SessionEvent) + Send + 'static> EventSink for F {
    fn send(&self, event: SessionEvent) {
        self(event)
    }
}

enum Command {
    Send(ClientMessage),
    Shutdown,
}

/// Cheap, clonable handle for sending input to the server. Sends after the
/// session has ended are silently dropped.
#[derive(Clone)]
pub struct InputSender {
    tx: Sender<Command>,
}

impl InputSender {
    pub fn send(&self, msg: ClientMessage) {
        let _ = self.tx.send(Command::Send(msg));
    }

    pub fn key(&self, keysym: u32, down: bool) {
        self.send(ClientMessage::KeyEvent { down, keysym });
    }

    pub fn pointer(&self, x: u16, y: u16, buttons: u8) {
        self.send(ClientMessage::PointerEvent { buttons, x, y });
    }
}

/// A connected VNC session. Dropping it disconnects.
pub struct Session {
    info: ServerInfo,
    framebuffer: Arc<Mutex<Framebuffer>>,
    input: InputSender,
    stream: TcpStream,
    closing: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

/// Connects to `host:port` and performs the handshake. Blocks until the
/// session is established or fails.
pub fn connect(
    host: &str,
    port: u16,
    opts: ConnectOptions,
    events: impl EventSink,
) -> Result<Session, ConnectError> {
    let connect_err = |source| ConnectError::Connect {
        addr: if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        },
        source,
    };
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(connect_err)?
        .collect();
    let mut last_err = std::io::Error::other("address resolved to nothing");
    for a in addrs {
        debug!(%a, "connecting");
        match TcpStream::connect_timeout(&a, opts.connect_timeout) {
            Ok(stream) => return connect_stream(stream, opts, events),
            Err(e) => last_err = e,
        }
    }
    Err(connect_err(last_err))
}

/// Performs the handshake over an already-connected stream and starts the
/// session threads.
pub fn connect_stream(
    mut stream: TcpStream,
    mut opts: ConnectOptions,
    events: impl EventSink,
) -> Result<Session, ConnectError> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(opts.handshake_timeout))?;
    let neg = handshake::run(
        &mut stream,
        &mut opts.security,
        opts.shared,
        &opts.encodings,
    )?;
    stream.set_read_timeout(None)?;

    let info = ServerInfo {
        name: neg.init.name,
        width: neg.init.width,
        height: neg.init.height,
        version: neg.version,
    };
    let framebuffer = Arc::new(Mutex::new(Framebuffer::new(info.width, info.height)));
    let closing = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    // Queue the first full update request before handing out the input
    // sender, so it is always the first message after the handshake.
    let _ = tx.send(update_request(false, info.width, info.height));

    let writer = {
        let stream = stream.try_clone()?;
        thread::Builder::new()
            .name("rvnc-writer".into())
            .spawn(move || writer_loop(stream, rx))?
    };
    let reader = {
        let stream = stream.try_clone()?;
        let fb = framebuffer.clone();
        let tx = tx.clone();
        let closing = closing.clone();
        let (w, h) = (info.width, info.height);
        thread::Builder::new()
            .name("rvnc-reader".into())
            .spawn(move || reader_loop(stream, fb, w, h, tx, closing, events))?
    };

    Ok(Session {
        info,
        framebuffer,
        input: InputSender { tx },
        stream,
        closing,
        reader: Some(reader),
        writer: Some(writer),
    })
}

impl Session {
    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// The shared framebuffer. Hold the lock only briefly.
    pub fn framebuffer(&self) -> &Arc<Mutex<Framebuffer>> {
        &self.framebuffer
    }

    pub fn input(&self) -> &InputSender {
        &self.input
    }

    /// Disconnects and waits for the session threads to finish. Pending
    /// input is flushed first.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.closing.store(true, Ordering::SeqCst);
        let _ = self.input.tx.send(Command::Shutdown);
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Locks a mutex, ignoring poisoning (the framebuffer is always in a
/// consistent state between operations).
pub(crate) fn lock(fb: &Mutex<Framebuffer>) -> MutexGuard<'_, Framebuffer> {
    fb.lock().unwrap_or_else(|e| e.into_inner())
}

/// Adapts the shared framebuffer to the decoder, locking per rectangle.
struct SharedSink {
    fb: Arc<Mutex<Framebuffer>>,
    width: u16,
    height: u16,
}

impl RectSink for SharedSink {
    fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    fn raw(&mut self, rect: Rect, pixels: &[u8]) {
        lock(&self.fb).put_raw(rect, pixels);
    }

    fn copy_rect(&mut self, dst: Rect, src_x: u16, src_y: u16) {
        lock(&self.fb).copy_rect(dst, src_x, src_y);
    }

    fn desktop_size(&mut self, width: u16, height: u16) {
        info!(width, height, "desktop resized");
        self.width = width;
        self.height = height;
        lock(&self.fb).resize(width, height);
    }
}

fn update_request(incremental: bool, width: u16, height: u16) -> Command {
    Command::Send(ClientMessage::FramebufferUpdateRequest {
        incremental,
        rect: Rect::new(0, 0, width, height),
    })
}

fn reader_loop(
    stream: TcpStream,
    fb: Arc<Mutex<Framebuffer>>,
    width: u16,
    height: u16,
    tx: Sender<Command>,
    closing: Arc<AtomicBool>,
    events: impl EventSink,
) {
    let mut r = BufReader::with_capacity(256 * 1024, stream);
    let mut sink = SharedSink { fb, width, height };
    let mut decoder = Decoder::new();

    let error = loop {
        match decoder.read_message(&mut r, &mut sink) {
            Ok(ServerMessage::FramebufferUpdate { rects, resized }) => {
                trace!(rects, resized, "framebuffer update");
                let (w, h) = sink.size();
                if resized {
                    events.send(SessionEvent::Resized {
                        width: w,
                        height: h,
                    });
                }
                // One outstanding request at a time: ask for the next update
                // as soon as this one is complete. After a resize the whole
                // screen is needed.
                let _ = tx.send(update_request(!resized, w, h));
                events.send(SessionEvent::FramebufferUpdated);
            }
            Ok(ServerMessage::Bell) => events.send(SessionEvent::Bell),
            Ok(ServerMessage::ServerCutText(text)) => {
                debug!(len = text.len(), "ignoring server cut text");
            }
            Ok(ServerMessage::SetColourMapEntries { first, count }) => {
                debug!(first, count, "ignoring colour map entries");
            }
            Err(e) => break e,
        }
    };

    let error = if closing.load(Ordering::SeqCst) {
        None
    } else {
        Some(describe_disconnect(&error))
    };
    match &error {
        Some(e) => warn!("disconnected: {e}"),
        None => info!("disconnected"),
    }
    let _ = tx.send(Command::Shutdown);
    events.send(SessionEvent::Disconnected { error });
}

fn describe_disconnect(e: &ProtoError) -> String {
    match e {
        e if e.is_eof() => "server closed the connection".to_owned(),
        ProtoError::Io(io) if io.kind() == std::io::ErrorKind::ConnectionReset => {
            "connection reset by server".to_owned()
        }
        e => e.to_string(),
    }
}

fn writer_loop(stream: TcpStream, rx: Receiver<Command>) {
    let mut w = BufWriter::with_capacity(16 * 1024, &stream);
    let mut buf = Vec::with_capacity(4096);
    'outer: while let Ok(first) = rx.recv() {
        buf.clear();
        // Byte offset of a trailing pointer event and its button mask, so
        // consecutive moves with unchanged buttons can be coalesced.
        let mut last_pointer: Option<(usize, u8)> = None;
        let mut shutdown = false;
        for cmd in std::iter::once(first).chain(rx.try_iter()) {
            match cmd {
                Command::Shutdown => {
                    shutdown = true;
                    break;
                }
                Command::Send(msg) => {
                    if let ClientMessage::PointerEvent { buttons, .. } = msg {
                        if let Some((at, b)) = last_pointer {
                            if b == buttons {
                                buf.truncate(at);
                            }
                        }
                        last_pointer = Some((buf.len(), buttons));
                    } else {
                        last_pointer = None;
                    }
                    msg.encode(&mut buf);
                }
            }
        }
        if let Err(e) = w.write_all(&buf).and_then(|()| w.flush()) {
            debug!("write failed: {e}");
            // Wake the reader so the session ends promptly.
            let _ = stream.shutdown(Shutdown::Both);
            break 'outer;
        }
        if shutdown {
            break;
        }
    }
    trace!("writer exiting");
}
