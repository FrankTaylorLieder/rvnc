//! rvnc: a VNC viewer for macOS.

mod app;
mod input;
mod keymap;
mod present;
mod view;

use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use rvnc_client::{
    ConnectOptions, DEFAULT_PORT, SessionEvent, connect, display_port, parse_address,
};
use tracing_subscriber::EnvFilter;
use winit::event_loop::EventLoop;

use crate::view::ScaleMode;

/// A VNC viewer.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Server address: host, host:port, [ipv6] or [ipv6]:port. The port
    /// defaults to 5900.
    address: String,

    /// TCP port to connect to. Overrides any port in the address.
    #[arg(short, long, conflicts_with = "display")]
    port: Option<u16>,

    /// VNC display number: connect to port 5900 + N. Overrides any port in
    /// the address.
    #[arg(short, long)]
    display: Option<u16>,

    /// Don't send keyboard or mouse input.
    #[arg(long)]
    view_only: bool,

    /// Initial scaling mode (toggle with Cmd-Option-S).
    #[arg(long, value_enum, default_value_t = ScaleArg::Fit)]
    scale: ScaleArg,

    /// Ask the server to disconnect other viewers.
    #[arg(long)]
    exclusive: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ScaleArg {
    Fit,
    #[value(name = "1:1", alias = "1")]
    OneToOne,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rvnc: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> anyhow::Result<()> {
    let (host, address_port) = parse_address(&args.address).map_err(anyhow::Error::msg)?;
    let port = match (args.port, args.display) {
        (Some(p), _) => p,
        (None, Some(d)) => display_port(d)
            .with_context(|| format!("display {d} is out of range (port would exceed 65535)"))?,
        (None, None) => address_port.unwrap_or(DEFAULT_PORT),
    };

    let event_loop = EventLoop::<SessionEvent>::with_user_event()
        .build()
        .context("creating event loop")?;
    let proxy = event_loop.create_proxy();

    let opts = ConnectOptions {
        shared: !args.exclusive,
        ..Default::default()
    };
    let session = connect(&host, port, opts, move |e| {
        // Fails only once the event loop has gone, when nobody cares.
        let _ = proxy.send_event(e);
    })?;

    let mut app = app::App::new(
        session,
        app::Options {
            view_only: args.view_only,
            scale: match args.scale {
                ScaleArg::Fit => ScaleMode::Fit,
                ScaleArg::OneToOne => ScaleMode::OneToOne,
            },
        },
    );
    event_loop.run_app(&mut app).context("running event loop")?;

    match app.error.take() {
        Some(e) => anyhow::bail!("disconnected: {e}"),
        None => Ok(()),
    }
}
