mod cursor;
mod decode;
mod input;
mod render;
mod transport;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;

#[cfg(target_os = "macos")]
use std::net::SocketAddr;

#[derive(Debug, Parser)]
#[command(
    name = "streamd-client",
    version,
    about = "Low-latency macOS QUIC remote desktop client for streamd."
)]
struct Args {
    /// Address of the streamd server.
    #[arg(value_name = "SERVER_ADDR", default_value = "127.0.0.1:9000")]
    server_addr: std::net::SocketAddr,

    /// Select a display by index, stable id, exact name, or exact description.
    #[arg(long, value_name = "ID|INDEX|NAME")]
    display: Option<String>,

    /// List displays exported by the server and exit.
    #[arg(long)]
    list_displays: bool,

    /// Tracing filter passed to tracing-subscriber.
    #[arg(long, env = "RUST_LOG", default_value = "streamd_client=debug,info")]
    log_filter: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    install_rustls_crypto_provider()?;

    tracing_subscriber::fmt()
        .with_env_filter(args.log_filter.clone())
        .init();

    let (server_addr, options) = client_options_from_args(args);

    info!("streamd-client connecting to {server_addr}");

    #[cfg(target_os = "macos")]
    if !options.list_displays {
        return run_macos_client(server_addr, options);
    }

    let runtime = build_runtime()?;
    runtime.block_on(transport::control::run_client(server_addr, options))
}

fn client_options_from_args(
    args: Args,
) -> (std::net::SocketAddr, transport::control::ClientOptions) {
    let options = transport::control::ClientOptions {
        list_displays: args.list_displays,
        display_selector: args.display,
    };
    (args.server_addr, options)
}

fn install_rustls_crypto_provider() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| {
            anyhow::anyhow!(
                "failed to install rustls ring CryptoProvider; another provider may already be active"
            )
        })
        .context("install rustls CryptoProvider")?;
    Ok(())
}

fn build_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build Tokio runtime")
}

#[cfg(target_os = "macos")]
fn run_macos_client(
    server_addr: SocketAddr,
    options: transport::control::ClientOptions,
) -> Result<()> {
    let runtime = build_runtime()?;
    let Some(mut session) = runtime.block_on(transport::control::connect_client_session(
        server_addr,
        options,
    ))?
    else {
        return Ok(());
    };

    let render_rx = session.take_render_rx()?;
    info!("starting macOS Metal renderer on the main thread");
    let render_result = render::metal::VideoRenderer::run(
        render_rx,
        session.width,
        session.height,
        session.cursor_store(),
        session.shutdown_signal(),
    );
    let shutdown_result = runtime.block_on(session.shutdown());

    render_result.and(shutdown_result)
}

#[cfg(test)]
mod tests {
    use super::{client_options_from_args, Args};
    use clap::Parser;

    #[test]
    fn parses_default_values() {
        let args = Args::try_parse_from(["streamd-client"]).expect("args should parse");
        let (server_addr, options) = client_options_from_args(args);
        assert_eq!(server_addr.to_string(), "127.0.0.1:9000");
        assert!(options.display_selector.is_none());
        assert!(!options.list_displays);
    }

    #[test]
    fn parses_display_selection() {
        let args = Args::try_parse_from([
            "streamd-client",
            "192.168.1.50:9000",
            "--display",
            "wayland:68",
            "--list-displays",
            "--log-filter",
            "info,streamd_client=trace",
        ])
        .expect("args should parse");
        let (server_addr, options) = client_options_from_args(args);
        assert_eq!(server_addr.to_string(), "192.168.1.50:9000");
        assert_eq!(options.display_selector.as_deref(), Some("wayland:68"));
        assert!(options.list_displays);
    }
}
