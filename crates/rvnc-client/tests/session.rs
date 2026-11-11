//! Integration tests against an in-process scripted RFB server.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rvnc_client::proto::PixelFormat;
use rvnc_client::proto::encoding;
use rvnc_client::{ConnectError, ConnectOptions, SessionEvent, connect};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Starts a one-shot server running `script`; returns its port.
fn serve(script: impl FnOnce(TcpStream) + Send + 'static) -> (u16, thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().port();
    let h = thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        s.set_read_timeout(Some(TIMEOUT)).unwrap();
        script(s);
    });
    (addr, h)
}

fn read_n(s: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut b = vec![0; n];
    s.read_exact(&mut b).unwrap();
    b
}

fn server_init(s: &mut TcpStream, w: u16, h: u16, name: &str) {
    let mut b = Vec::new();
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    // Deliberately not our format, to check we send SetPixelFormat.
    let mut pf = PixelFormat::rvnc_default();
    pf.big_endian = true;
    pf.encode(&mut b);
    b.extend_from_slice(&(name.len() as u32).to_be_bytes());
    b.extend_from_slice(name.as_bytes());
    s.write_all(&b).unwrap();
}

/// Server side of a 3.8 handshake with security None, up to and including
/// reading SetPixelFormat, SetEncodings and the first update request.
fn handshake_38(s: &mut TcpStream, w: u16, h: u16) -> Vec<i32> {
    s.write_all(b"RFB 003.008\n").unwrap();
    assert_eq!(read_n(s, 12), b"RFB 003.008\n");
    s.write_all(&[1, 1]).unwrap();
    assert_eq!(read_n(s, 1), [1]);
    s.write_all(&[0, 0, 0, 0]).unwrap();
    assert_eq!(read_n(s, 1), [1], "shared flag");
    server_init(s, w, h, "test desk");
    after_init(s, w, h)
}

/// Reads SetPixelFormat, SetEncodings and the initial full update request.
fn after_init(s: &mut TcpStream, w: u16, h: u16) -> Vec<i32> {
    let spf = read_n(s, 20);
    assert_eq!(spf[0], 0);
    let mut expect = Vec::new();
    PixelFormat::rvnc_default().encode(&mut expect);
    assert_eq!(&spf[4..], expect.as_slice());

    let hdr = read_n(s, 4);
    assert_eq!(hdr[0], 2);
    let n = u16::from_be_bytes([hdr[2], hdr[3]]);
    let encs: Vec<i32> = read_n(s, 4 * usize::from(n))
        .chunks(4)
        .map(|c| i32::from_be_bytes(c.try_into().unwrap()))
        .collect();

    expect_update_request(s, false, w, h);
    encs
}

fn expect_update_request(s: &mut TcpStream, incremental: bool, w: u16, h: u16) {
    let mut expect = vec![3, u8::from(incremental), 0, 0, 0, 0];
    expect.extend_from_slice(&w.to_be_bytes());
    expect.extend_from_slice(&h.to_be_bytes());
    assert_eq!(read_n(s, 10), expect);
}

fn rect(out: &mut Vec<u8>, x: u16, y: u16, w: u16, h: u16, enc: i32) {
    for v in [x, y, w, h] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&enc.to_be_bytes());
}

fn events() -> (mpsc::Sender<SessionEvent>, mpsc::Receiver<SessionEvent>) {
    mpsc::channel()
}

fn next(rx: &mpsc::Receiver<SessionEvent>) -> SessionEvent {
    rx.recv_timeout(TIMEOUT)
        .expect("timed out waiting for event")
}

#[test]
fn full_session_38() {
    let (addr, server) = serve(|mut s| {
        let encs = handshake_38(&mut s, 4, 2);
        assert_eq!(encs, encoding::SUPPORTED);

        // Raw 2x1 at (1,0), then CopyRect it to (1,1).
        let mut b = vec![0, 0, 0, 2];
        rect(&mut b, 1, 0, 2, 1, encoding::RAW);
        b.extend_from_slice(&[0x30, 0x20, 0x10, 0xaa, 0x03, 0x02, 0x01, 0xbb]);
        rect(&mut b, 1, 1, 2, 1, encoding::COPY_RECT);
        b.extend_from_slice(&[0, 1, 0, 0]);
        s.write_all(&b).unwrap();
        expect_update_request(&mut s, true, 4, 2);

        // Bell, cut text and a colour map are accepted and ignored.
        s.write_all(&[2, 3, 0, 0, 0, 0, 0, 0, 1, b'x', 1, 0, 0, 0, 0, 0])
            .unwrap();

        // Input from the client.
        assert_eq!(read_n(&mut s, 8), [4, 1, 0, 0, 0, 0, 0xff, 0x0d]);
        assert_eq!(read_n(&mut s, 6), [5, 1, 0, 3, 0, 1]);

        // Resize: next request must be non-incremental at the new size.
        let mut b = vec![0, 0, 0, 1];
        rect(&mut b, 0, 0, 8, 6, encoding::DESKTOP_SIZE);
        s.write_all(&b).unwrap();
        expect_update_request(&mut s, false, 8, 6);
    });

    let (tx, rx) = events();
    let session = connect("127.0.0.1", addr, ConnectOptions::default(), move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    assert_eq!(session.info().name, "test desk");
    assert_eq!((session.info().width, session.info().height), (4, 2));

    assert_eq!(next(&rx), SessionEvent::FramebufferUpdated);
    {
        let fb = session.framebuffer().lock().unwrap();
        assert_eq!(fb.pixel(1, 0), 0x0010_2030);
        assert_eq!(fb.pixel(2, 0), 0x0001_0203);
        assert_eq!(fb.pixel(1, 1), 0x0010_2030);
        assert_eq!(fb.pixel(2, 1), 0x0001_0203);
        assert_eq!(fb.pixel(0, 0), 0);
    }
    assert_eq!(next(&rx), SessionEvent::Bell);

    session.input().key(0xff0d, true);
    session.input().pointer(3, 1, 1);

    assert_eq!(
        next(&rx),
        SessionEvent::Resized {
            width: 8,
            height: 6
        }
    );
    assert_eq!(next(&rx), SessionEvent::FramebufferUpdated);
    assert_eq!(session.framebuffer().lock().unwrap().width(), 8);

    server.join().unwrap();
    assert_eq!(
        next(&rx),
        SessionEvent::Disconnected {
            error: Some("server closed the connection".into())
        }
    );
    session.close();
}

#[test]
fn version_37_none_has_no_security_result() {
    let (addr, server) = serve(|mut s| {
        s.write_all(b"RFB 003.007\n").unwrap();
        assert_eq!(read_n(&mut s, 12), b"RFB 003.007\n");
        s.write_all(&[2, 2, 1]).unwrap();
        assert_eq!(read_n(&mut s, 1), [1]);
        // No SecurityResult in 3.7 for None.
        assert_eq!(read_n(&mut s, 1), [1]);
        server_init(&mut s, 2, 2, "v37");
        after_init(&mut s, 2, 2);
    });
    let s = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {}).unwrap();
    assert_eq!(s.info().name, "v37");
    server.join().unwrap();
}

#[test]
fn version_33_server_chooses_type() {
    let (addr, server) = serve(|mut s| {
        s.write_all(b"RFB 003.003\n").unwrap();
        assert_eq!(read_n(&mut s, 12), b"RFB 003.003\n");
        s.write_all(&[0, 0, 0, 1]).unwrap();
        assert_eq!(read_n(&mut s, 1), [1]);
        server_init(&mut s, 2, 2, "v33");
        after_init(&mut s, 2, 2);
    });
    let s = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {}).unwrap();
    assert_eq!(s.info().name, "v33");
    server.join().unwrap();
}

#[test]
fn apple_version_negotiates_38() {
    let (addr, server) = serve(|mut s| {
        s.write_all(b"RFB 003.889\n").unwrap();
        assert_eq!(read_n(&mut s, 12), b"RFB 003.008\n");
        s.write_all(&[2, 30, 2]).unwrap();
    });
    let err = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {})
        .err()
        .unwrap();
    assert!(matches!(&err, ConnectError::NoSupportedSecurity(t) if t == &[30, 2]));
    let msg = err.to_string();
    assert!(msg.contains("Apple Remote Desktop (30)"), "{msg}");
    assert!(msg.contains("VNC Authentication (2)"), "{msg}");
    server.join().unwrap();
}

#[test]
fn server_refusal_reason_is_reported() {
    let (addr, server) = serve(|mut s| {
        s.write_all(b"RFB 003.008\n").unwrap();
        read_n(&mut s, 12);
        s.write_all(&[0, 0, 0, 0, 4, b'b', b'u', b's', b'y'])
            .unwrap();
    });
    let err = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {})
        .err()
        .unwrap();
    assert!(matches!(err, ConnectError::Refused(r) if r == "busy"));
    server.join().unwrap();
}

#[test]
fn security_result_failure_is_reported() {
    let (addr, server) = serve(|mut s| {
        s.write_all(b"RFB 003.008\n").unwrap();
        read_n(&mut s, 12);
        s.write_all(&[1, 1]).unwrap();
        read_n(&mut s, 1);
        s.write_all(&[0, 0, 0, 1, 0, 0, 0, 2, b'n', b'o']).unwrap();
    });
    let err = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {})
        .err()
        .unwrap();
    assert!(matches!(err, ConnectError::AuthFailed(r) if r == "no"));
    server.join().unwrap();
}

#[test]
fn not_a_vnc_server() {
    let (addr, server) = serve(|mut s| {
        s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").unwrap();
    });
    let err = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {})
        .err()
        .unwrap();
    assert!(
        err.to_string().contains("invalid protocol version"),
        "{err}"
    );
    server.join().unwrap();
}

#[test]
fn handshake_timeout() {
    let (addr, server) = serve(|s| {
        // Say nothing; keep the socket open until the client gives up.
        let mut s = s;
        let mut b = [0u8; 1];
        let _ = s.read(&mut b);
    });
    let opts = ConnectOptions {
        handshake_timeout: Duration::from_millis(200),
        ..Default::default()
    };
    let err = connect("127.0.0.1", addr, opts, |_| {}).err().unwrap();
    assert!(matches!(err, ConnectError::Proto(_)), "{err}");
    server.join().unwrap();
}

#[test]
fn connection_refused() {
    // Bind then drop to get a port that is (almost certainly) closed.
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {})
        .err()
        .unwrap();
    assert!(matches!(err, ConnectError::Connect { .. }), "{err}");
}

#[test]
fn protocol_error_disconnects_with_reason() {
    let (addr, server) = serve(|mut s| {
        handshake_38(&mut s, 4, 4);
        let mut b = vec![0, 0, 0, 1];
        rect(&mut b, 3, 3, 2, 2, encoding::RAW);
        b.extend_from_slice(&[0; 16]);
        s.write_all(&b).unwrap();
        // Keep the connection open; the client must hang up on its own.
        let mut buf = [0u8; 64];
        while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
    });
    let (tx, rx) = events();
    let session = connect("127.0.0.1", addr, ConnectOptions::default(), move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    match next(&rx) {
        SessionEvent::Disconnected { error: Some(e) } => {
            assert!(e.contains("outside"), "{e}")
        }
        other => panic!("unexpected {other:?}"),
    }
    drop(session);
    server.join().unwrap();
}

#[test]
fn close_is_clean_and_flushes_input() {
    let (addr, server) = serve(|mut s| {
        handshake_38(&mut s, 2, 2);
        assert_eq!(read_n(&mut s, 8), [4, 0, 0, 0, 0, 0, 0, 0x61]);
        let mut b = [0u8; 1];
        assert_eq!(s.read(&mut b).unwrap(), 0, "expected EOF after close");
    });
    let (tx, rx) = events();
    let session = connect("127.0.0.1", addr, ConnectOptions::default(), move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    session.input().key(0x61, false);
    session.close();
    assert_eq!(next(&rx), SessionEvent::Disconnected { error: None });
    server.join().unwrap();
}

#[test]
fn pointer_moves_are_coalesced_but_clicks_are_not() {
    let (addr, server) = serve(|mut s| {
        handshake_38(&mut s, 100, 100);
        let mut got = Vec::new();
        let mut b = [0u8; 6];
        loop {
            s.read_exact(&mut b).unwrap();
            assert_eq!(b[0], 5);
            got.push((b[1], u16::from_be_bytes([b[2], b[3]])));
            if b[1] == 0 && got.len() > 1 && got[got.len() - 2].0 == 1 {
                break;
            }
        }
        // Whatever coalescing happened, the press and release must both be
        // present and the last move before the press must be at x=50.
        let press = got.iter().position(|(m, _)| *m == 1).unwrap();
        assert_eq!(got[press].1, 50);
        assert!(press >= 1 && got[press - 1].1 == 50, "{got:?}");
    });
    let session = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {}).unwrap();
    for x in 0..=50 {
        session.input().pointer(x, 0, 0);
    }
    session.input().pointer(50, 0, 1);
    session.input().pointer(50, 0, 0);
    server.join().unwrap();
    drop(session);
}
