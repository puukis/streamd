mod capture;
mod encode;
mod input;
mod pipeline;
mod transport;

use anyhow::{Context, Result};
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    install_rustls_crypto_provider()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "streamd_server=debug,info".into()),
        )
        .init();

    let bind_addr: std::net::SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:9000".to_string())
        .parse()?;

    info!("streamd-server starting on {bind_addr}");
    transport::control::run_server(bind_addr).await
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
