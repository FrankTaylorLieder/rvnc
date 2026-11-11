//! Connects to a VNC server, waits for the first full update and writes it
//! to a PNG.
//!
//! ```sh
//! cargo run -p rvnc-client --example screenshot -- localhost:1 out.png
//! ```

use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use rvnc_client::{ConnectOptions, SessionEvent, connect, parse_address};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args = std::env::args().skip(1);
    let (Some(addr), Some(out)) = (args.next(), args.next()) else {
        bail!("usage: screenshot <host[:display|::port]> <out.png>");
    };
    let (host, port) = parse_address(&addr).map_err(anyhow::Error::msg)?;

    let start = Instant::now();
    let (tx, rx) = mpsc::channel();
    let session = connect(&host, port, ConnectOptions::default(), move |e| {
        let _ = tx.send(e);
    })
    .with_context(|| format!("connecting to {addr}"))?;
    let info = session.info().clone();
    println!(
        "connected to {:?} ({}x{}, RFB {}) in {:?}",
        info.name,
        info.width,
        info.height,
        info.version,
        start.elapsed()
    );

    loop {
        match rx.recv_timeout(Duration::from_secs(10))? {
            SessionEvent::FramebufferUpdated => break,
            SessionEvent::Disconnected { error } => bail!("disconnected: {error:?}"),
            _ => {}
        }
    }
    println!("first update after {:?}", start.elapsed());

    let fb = session.framebuffer().lock().unwrap().clone();
    session.close();

    let mut rgb = Vec::with_capacity(fb.pixels().len() * 3);
    for p in fb.pixels() {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let file = std::fs::File::create(&out).with_context(|| format!("creating {out}"))?;
    let mut enc = png::Encoder::new(file, u32::from(fb.width()), u32::from(fb.height()));
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()?.write_image_data(&rgb)?;
    println!("wrote {out}");
    Ok(())
}
