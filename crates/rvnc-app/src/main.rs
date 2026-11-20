//! rvnc: a VNC viewer for macOS.

mod app;
mod auth;
mod input;
mod keychain;
mod keymap;
mod present;
mod view;

use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use rvnc_client::proto::encoding;
use rvnc_client::{
    AppleAuth, ConnectError, ConnectOptions, DEFAULT_PORT, NoneSecurity, SecurityHandler,
    SessionEvent, VncAuth, connect, display_port, parse_address,
};
use tracing_subscriber::EnvFilter;
use winit::event_loop::EventLoop;
use zeroize::Zeroizing;

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

    /// Pixel encoding: zrle compresses (best on most networks); raw uses
    /// less CPU on very fast links.
    #[arg(long, value_enum, default_value_t = EncodingArg::Zrle)]
    encoding: EncodingArg,

    /// Ask the server to disconnect other viewers.
    #[arg(long)]
    exclusive: bool,

    /// Username for macOS Screen Sharing (Apple Remote Desktop auth). Also
    /// makes ARD preferred over a VNC password when the server offers both.
    #[arg(short, long)]
    user: Option<String>,

    /// Read the password from this file ("-" for stdin). The RVNC_PASSWORD
    /// environment variable works too.
    #[arg(long, value_name = "PATH")]
    password_file: Option<String>,

    /// Save the password in the macOS keychain after logging in.
    #[arg(long, conflicts_with = "no_keychain")]
    save_password: bool,

    /// Don't look up or save passwords in the keychain.
    #[arg(long)]
    no_keychain: bool,

    /// Remove saved passwords for this server from the keychain, then exit.
    #[arg(long)]
    forget_password: bool,

    /// Only use this security type, rather than the best one both sides
    /// support.
    #[arg(long, value_enum)]
    security: Option<SecurityArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum EncodingArg {
    Zrle,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SecurityArg {
    /// No authentication.
    None,
    /// VNC password.
    Vnc,
    /// Apple Remote Desktop (macOS account).
    Ard,
}

impl Args {
    fn security_handlers(&self) -> Vec<Box<dyn SecurityHandler>> {
        let order = match (self.security, &self.user) {
            (Some(only), _) => vec![only],
            (None, Some(_)) => vec![SecurityArg::None, SecurityArg::Ard, SecurityArg::Vnc],
            (None, None) => vec![SecurityArg::None, SecurityArg::Vnc, SecurityArg::Ard],
        };
        order
            .into_iter()
            .map(|t| -> Box<dyn SecurityHandler> {
                match t {
                    SecurityArg::None => Box::new(NoneSecurity),
                    SecurityArg::Vnc => Box::new(VncAuth),
                    SecurityArg::Ard => Box::new(AppleAuth),
                }
            })
            .collect()
    }
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

    let server = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };

    if args.forget_password {
        let n = keychain::forget(&server).map_err(anyhow::Error::msg)?;
        eprintln!("rvnc: removed {n} saved password(s) for {server}");
        return Ok(());
    }

    let explicit_password = match &args.password_file {
        Some(path) => Some(
            auth::read_password_file(path)
                .with_context(|| format!("reading password file {path}"))?,
        ),
        None => std::env::var("RVNC_PASSWORD").ok().map(Zeroizing::new),
    };
    let auth = auth::Auth::new(auth::Config {
        server,
        user: args.user.clone(),
        interactive: explicit_password.is_none() && std::io::stdin().is_terminal(),
        explicit_password,
        use_keychain: !args.no_keychain,
        save: args.save_password,
    });

    let event_loop = EventLoop::<SessionEvent>::with_user_event()
        .build()
        .context("creating event loop")?;
    let proxy = event_loop.create_proxy();

    let session = loop {
        let opts = ConnectOptions {
            shared: !args.exclusive,
            security: args.security_handlers(),
            encodings: match args.encoding {
                EncodingArg::Zrle => encoding::SUPPORTED.to_vec(),
                EncodingArg::Raw => encoding::UNCOMPRESSED.to_vec(),
            },
            credentials: auth.provider(),
            ..Default::default()
        };
        let proxy = proxy.clone();
        let result = connect(&host, port, opts, move |e| {
            // Fails only once the event loop has gone, when nobody cares.
            let _ = proxy.send_event(e);
        });
        match result {
            Ok(session) => {
                auth.succeeded();
                break session;
            }
            Err(ConnectError::AuthFailed(reason)) if auth.retry_after_failure() => {
                eprintln!("rvnc: authentication failed: {reason}");
            }
            Err(e) => return Err(e.into()),
        }
    };

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
