mod capture;
mod encode;
mod input;
mod pipeline;
mod transport;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
#[command(
    name = "streamd-server",
    version,
    about = "Low-latency QUIC remote desktop host for Linux Wayland and Windows."
)]
struct Args {
    /// Socket address to bind the QUIC listener to.
    #[arg(value_name = "BIND_ADDR", default_value = "0.0.0.0:9000")]
    bind_addr: std::net::SocketAddr,

    /// Tracing filter passed to tracing-subscriber.
    #[arg(long, env = "RUST_LOG", default_value = "streamd_server=debug,info")]
    log_filter: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    install_rustls_crypto_provider()?;

    tracing_subscriber::fmt()
        .with_env_filter(args.log_filter.clone())
        .init();

    info!("streamd-server starting on {}", args.bind_addr);
    transport::control::run_server(args.bind_addr).await
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

#[cfg(test)]
mod tests {
    use super::Args;
    use clap::Parser;

    #[test]
    fn parses_default_values() {
        let args = Args::try_parse_from(["streamd-server"]).expect("args should parse");
        assert_eq!(args.bind_addr.to_string(), "0.0.0.0:9000");
        assert!(!args.log_filter.is_empty());
    }

    #[test]
    fn parses_bind_addr_and_log_filter() {
        let args = Args::try_parse_from([
            "streamd-server",
            "127.0.0.1:9443",
            "--log-filter",
            "info,streamd_server=trace",
        ])
        .expect("args should parse");
        assert_eq!(args.bind_addr.to_string(), "127.0.0.1:9443");
        assert_eq!(args.log_filter, "info,streamd_server=trace");
    }
}
