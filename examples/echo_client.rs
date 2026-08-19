//! Minimal client example
//!
//! Simply connects to the  `echo_server` and logs `Counter`.
//!
//! Run via ye olde car go:
//! ```sh
//! cargo run --example echo_client -- --server-id IROH_ID --server-addr 127.0.0.1:4433
//! ```
//! Hint: just paste the Iroh identity `IROH_ID` from what `echo_server`'s id=...` line or don't its your life.

use std::net::SocketAddr;
use std::time::Duration;

use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::common_conditions::on_timer;
use bevy_replicon::prelude::*;
use bevy_replicon_iroh::examples_common::Counter;
use bevy_replicon_iroh::iroh::{EndpointAddr, EndpointId};
use bevy_replicon_iroh::{IrohClient, IrohTokioHandle, RepliconIrohPlugins};
use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:0")]
    bind: SocketAddr,

    /// The server's iroh endpoint identifier.
    #[arg(long)]
    server_id: EndpointId,

    /// The server's dialable direct socket address aka ip address.
    #[arg(long)]
    server_addr: SocketAddr,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let runtime = tokio::runtime::Runtime::new()?;
    let handle = IrohTokioHandle(runtime.handle().clone());

    let secret_key = bevy_replicon_iroh::iroh::SecretKey::generate();
    let peer = EndpointAddr::new(args.server_id).with_ip_addr(args.server_addr);

    let client = IrohClient::connect(&handle, secret_key, args.bind, peer).map_err(|e| {
        anyhow::anyhow!(
            "failed to connect to {} ({}): {e:#}",
            args.server_addr,
            args.server_id
        )
    })?;
    tracing::info!(id = %args.server_id, addr = %args.server_addr, "echo_client: connected");

    let mut app = App::new();
    // IrohTokioHandle must be inserted before `RepliconIrohPlugins` is
    // added - see that resource's own doc comment.
    app.insert_resource(handle)
        .add_plugins(MinimalPlugins)
        .add_plugins(StatesPlugin)
        .add_plugins(RepliconPlugins)
        .add_plugins(RepliconIrohPlugins)
        .replicate::<Counter>()
        .insert_resource(client)
        .add_systems(Update, log_state.run_if(on_timer(Duration::from_secs(1))));

    app.run();

    drop(runtime);
    Ok(())
}

fn log_state(counters: Query<&Counter>) {
    let value = counters.iter().next().map(|c| c.0).unwrap_or(0);
    tracing::info!("state: role=client counter={value}");
}
