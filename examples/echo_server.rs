//! Minimal server example
//!
//! Simply binds an iroh `bevy_replicon` server and ticks a silly replicated
//! `Counter` entity once a second to prove this thing isn't blowing smoke up my
//! own butt.
//!
//! Run ye olde car go:
//! ```sh
//! cargo run --example echo_server -- --bind 127.0.0.1:4433
//! ```
//! and then paste the output `id=...` value into `echo_client`'s `--server-id`
//! with with the `--server-addr` that the socket was bound to.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::common_conditions::on_timer;
use bevy_replicon::prelude::*;
use bevy_replicon_iroh::examples_common::{self, Counter};
use bevy_replicon_iroh::{IrohServer, IrohTokioHandle, RepliconIrohPlugins};
use clap::Parser;

#[derive(Parser)]
struct Args {
    /// Local address to bind the iroh endpoint to.
    #[arg(long, default_value = "0.0.0.0:0")]
    bind: SocketAddr,

    /// Optional path to write this server's endpoint id to, once bound - handy
    /// for a test harness that needs to hand it to a client without scraping
    /// stdout like a hack. I might have only added this for the nixos vm test
    /// suite don't judge me.
    #[arg(long)]
    id_out: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Plain tokio, deliberately not a bevy-specific tokio-integration
    // crate - see the crate's own docs for why. This example owns the
    // `tokio::runtime::Runtime` outright and just hands a `Handle` to
    // bevy_replicon_iroh.
    let runtime = tokio::runtime::Runtime::new()?;
    let handle = IrohTokioHandle(runtime.handle().clone());

    // A real application should persist this key (see this crate's own
    // README/docs) so its identity survives a restart - kept ephemeral
    // here for simplicity.
    let secret_key = bevy_replicon_iroh::iroh::SecretKey::generate();

    let server = IrohServer::bind(&handle, secret_key, args.bind)
        .map_err(|e| anyhow::anyhow!("failed to bind iroh server on {}: {e:#}", args.bind))?;

    let id = server.id();
    let addrs = server.bound_sockets();
    tracing::info!(%id, ?addrs, "echo_server: listening");
    println!("id={id}");
    for addr in &addrs {
        println!("addr={addr}");
    }
    if let Some(path) = &args.id_out {
        std::fs::write(path, id.to_string())
            .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    }

    let mut app = App::new();
    // Order matters: `RepliconPlugins` needs `StatesPlugin` (for the
    // ServerState/ClientState `StateTransition` schedule) already present,
    // and `IrohTokioHandle` must be inserted before `RepliconIrohPlugins`
    // is added - see that resource's own doc comment.
    app.insert_resource(handle)
        .add_plugins(MinimalPlugins)
        .add_plugins(StatesPlugin)
        .add_plugins(RepliconPlugins)
        .add_plugins(RepliconIrohPlugins)
        .replicate::<Counter>()
        .insert_resource(server)
        .add_systems(Startup, spawn_counter)
        .add_systems(
            Update,
            (
                examples_common::tick.run_if(on_timer(Duration::from_secs(1))),
                log_state.run_if(on_timer(Duration::from_secs(1))),
            ),
        );

    app.run();

    drop(runtime);
    Ok(())
}

fn spawn_counter(mut commands: Commands) {
    // `Replicated` is REQUIRED here - by default no entities are
    // replicated in bevy_replicon, even if their component type is
    // registered via `.replicate::<T>()`. Without this marker the entity
    // is never sent to any client at all.
    commands.spawn((Counter(0), Replicated));
}

fn log_state(counters: Query<&Counter>) {
    let value = counters.iter().next().map(|c| c.0).unwrap_or(0);
    tracing::info!("state: role=server counter={value}");
}
