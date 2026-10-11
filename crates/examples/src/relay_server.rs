//! Minimal standalone Iroh relay server for test/ci use.
//!
//! Serves plain HTTP for a throwaway relay for mostly CI testing. Don't use it
//! on the internet unless you're crazy.
//!
//! ```sh
//! cargo run --example relay_server -- --bind 0.0.0.0:3340
//!     ```
//!
//! Point `echo_server` and `echo_client` at it via `--relay-url
//! http://<ip>:3340` to use.

use std::net::SocketAddr;

use clap::Parser;
use iroh_relay::server::{RelayConfig, Server, ServerConfig};

#[derive(Parser)]
struct Args {
    /// Address for the relay's endpoint to bind to.
    #[arg(long, default_value = "0.0.0.0:3340")]
    bind: SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // `ServerConfig` is `#[non_exhaustive]` so you can't struct-literal it even
    // with `..Default::default()`, so build the default then mutate it instead.
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new(args.bind));
    let _server = Server::spawn(config)
        .await
        .map_err(|e| anyhow::anyhow!("failed to spawn relay server on {}: {e:#}", args.bind))?;

    tracing::info!(bind = %args.bind, "relay_server: listening");
    println!("relay listening on http://{}", args.bind);

    // This doens't ever exit, acts like a daemon. Mostly just used for the
    // nixos ci test framework that is calling it.
    std::future::pending::<()>().await;
    Ok(())
}
