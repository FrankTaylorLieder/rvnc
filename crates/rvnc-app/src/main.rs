//! rvnc: a VNC viewer for macOS.

mod app;
mod input;
mod keymap;
mod present;
mod view;

use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use rvnc_client::{ConnectOptions, SessionEvent, connect, parse_address};
use tracing_subscriber::EnvFilter;
use winit::event_loop::EventLoop;

use crate::view::ScaleMode;

/// A VNC viewer.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Server address: host, host:display, host::port, or [ipv6]:display.
    address: String,

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
    let (host, port) = parse_address(&args.address).map_err(anyhow::Error::msg)?;

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
    })
    .with_context(|| format!("connecting to {}", args.address))?;

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
