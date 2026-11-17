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
        s.write_all(&[2, 19, 16]).unwrap();
    });
    let err = connect("127.0.0.1", addr, ConnectOptions::default(), |_| {})
        .err()
        .unwrap();
    assert!(matches!(&err, ConnectError::NoSupportedSecurity(t) if t == &[19, 16]));
    let msg = err.to_string();
    assert!(msg.contains("VeNCrypt (19)"), "{msg}");
    assert!(msg.contains("Tight (16)"), "{msg}");
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

// ---- Authentication ------------------------------------------------------

use rvnc_client::proto::auth::vnc_auth_response;
use rvnc_client::{CredentialRequest, Credentials};

/// Sends the 3.8 version and security offer, reads the client's choice.
fn offer_38(s: &mut TcpStream, types: &[u8]) -> u8 {
    s.write_all(b"RFB 003.008\n").unwrap();
    assert_eq!(read_n(s, 12), b"RFB 003.008\n");
    let mut b = vec![types.len() as u8];
    b.extend_from_slice(types);
    s.write_all(&b).unwrap();
    read_n(s, 1)[0]
}

/// SecurityResult OK through the initial update request.
fn accept_38(s: &mut TcpStream) {
    s.write_all(&[0, 0, 0, 0]).unwrap();
    assert_eq!(read_n(s, 1), [1], "shared flag");
    server_init(s, 4, 2, "auth desk");
    after_init(s, 4, 2);
}

const CHALLENGE: [u8; 16] = *b"fedcba9876543210";

fn vnc_server(password: &'static str) -> (u16, thread::JoinHandle<()>) {
    serve(move |mut s| {
        assert_eq!(offer_38(&mut s, &[2]), 2);
        s.write_all(&CHALLENGE).unwrap();
        let response = read_n(&mut s, 16);
        if response == vnc_auth_response(&CHALLENGE, password.as_bytes()) {
            accept_38(&mut s);
        } else {
            let reason = b"wrong password";
            s.write_all(&[0, 0, 0, 1, 0, 0, 0, reason.len() as u8])
                .unwrap();
            s.write_all(reason).unwrap();
        }
    })
}

fn password_opts(password: &'static str, asked: mpsc::Sender<CredentialRequest>) -> ConnectOptions {
    ConnectOptions {
        credentials: Box::new(move |req: &CredentialRequest| {
            asked.send(*req).unwrap();
            Ok(Credentials::password(password))
        }),
        ..Default::default()
    }
}

#[test]
fn vnc_auth_succeeds_with_right_password() {
    let (port, server) = vnc_server("secret");
    let (tx, asked) = mpsc::channel();
    let session = connect("127.0.0.1", port, password_opts("secret", tx), |_| {}).unwrap();
    assert_eq!(session.info().name, "auth desk");
    assert_eq!(
        asked.try_recv().unwrap(),
        CredentialRequest {
            security_type: 2,
            username: false
        }
    );
    session.close();
    server.join().unwrap();
}

#[test]
fn vnc_auth_wrong_password_reports_reason() {
    let (port, server) = vnc_server("secret");
    let (tx, _asked) = mpsc::channel();
    let err = connect("127.0.0.1", port, password_opts("guess", tx), |_| {})
        .err()
        .unwrap();
    assert!(
        matches!(&err, ConnectError::AuthFailed(r) if r == "wrong password"),
        "{err}"
    );
    server.join().unwrap();
}

#[test]
fn vnc_auth_without_credentials_is_a_clear_error() {
    let (port, server) = serve(|mut s| {
        offer_38(&mut s, &[2]);
        s.write_all(&CHALLENGE).unwrap();
        // The client gives up without answering.
        let mut b = [0u8; 1];
        assert_eq!(s.read(&mut b).unwrap_or(0), 0);
    });
    let err = connect("127.0.0.1", port, ConnectOptions::default(), |_| {})
        .err()
        .unwrap();
    assert!(
        matches!(
            err,
            ConnectError::NoCredentials {
                security_type: 2,
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("VNC Authentication"), "{err}");
    server.join().unwrap();
}

#[test]
fn none_is_preferred_over_password_auth() {
    let (port, server) = serve(|mut s| {
        assert_eq!(offer_38(&mut s, &[30, 2, 1]), 1);
        accept_38(&mut s);
    });
    let (tx, asked) = mpsc::channel();
    let session = connect("127.0.0.1", port, password_opts("x", tx), |_| {}).unwrap();
    assert!(asked.try_recv().is_err(), "no password should be requested");
    session.close();
    server.join().unwrap();
}

#[test]
fn ard_sends_credentials_and_public_key() {
    const KEY_LEN: usize = 16;
    let (port, server) = serve(|mut s| {
        assert_eq!(offer_38(&mut s, &[30, 33, 36, 35]), 30);
        let mut b = vec![0, 2, 0, KEY_LEN as u8];
        // 2^127 - 1 and an arbitrary server public key.
        b.push(0x7f);
        b.extend_from_slice(&[0xff; KEY_LEN - 1]);
        b.extend_from_slice(&[0x11; KEY_LEN]);
        s.write_all(&b).unwrap();
        // 128-byte encrypted credentials, then our public key.
        let reply = read_n(&mut s, 128 + KEY_LEN);
        assert!(reply[..128].iter().any(|&b| b != 0));
        accept_38(&mut s);
    });
    let (tx, asked) = mpsc::channel();
    let opts = ConnectOptions {
        credentials: Box::new(move |req: &CredentialRequest| {
            tx.send(*req).unwrap();
            Ok(Credentials::with_username("alice", "pw"))
        }),
        ..Default::default()
    };
    let session = connect("127.0.0.1", port, opts, |_| {}).unwrap();
    assert_eq!(
        asked.try_recv().unwrap(),
        CredentialRequest {
            security_type: 30,
            username: true
        }
    );
    session.close();
    server.join().unwrap();
}

#[test]
fn version_33_vnc_auth() {
    let (port, server) = serve(|mut s| {
        s.write_all(b"RFB 003.003\n").unwrap();
        assert_eq!(read_n(&mut s, 12), b"RFB 003.003\n");
        s.write_all(&[0, 0, 0, 2]).unwrap();
        s.write_all(&CHALLENGE).unwrap();
        assert_eq!(read_n(&mut s, 16), vnc_auth_response(&CHALLENGE, b"pw"));
        s.write_all(&[0, 0, 0, 0]).unwrap();
        assert_eq!(read_n(&mut s, 1), [1]);
        server_init(&mut s, 4, 2, "old");
        after_init(&mut s, 4, 2);
    });
    let (tx, _asked) = mpsc::channel();
    let session = connect("127.0.0.1", port, password_opts("pw", tx), |_| {}).unwrap();
    session.close();
    server.join().unwrap();
}
