//! Minimal server example
//!
//! Simply binds an iroh `bevy_replicon` server and ticks a silly replicated
//! `Counter` entity once a second to prove this thing isn't blowing smoke up my
//! own butt.
//!
//! Run ye olde car go:
//! ```sh
//! cargo run --example echo_server -- --bind 127.0.0.1:4433
//!     ```
//!
//! And then paste the output `id=...` value into `echo_client`'s `--server-id`
//! with with the `--server-addr` that the socket was bound to. Pass `--relay`
//! on both ends to use iroh default preset with relay and discovery instead of
//! the direct-only default, or `--relay-url URL` on both ends of the echo
//! processes to point at a specific relay instead of the Iroh default. Note
//! takes precedence over `--relay` if both given.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::common_conditions::on_timer;
use bevy_replicon::prelude::*;
use bevy_replicon_iroh::examples_common::{self, Counter};
use bevy_replicon_iroh::{IrohServer, IrohServerConfig, IrohTokioHandle, RepliconIrohPlugins};
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

    /// Use iroh's "N0" preset with relay and discovery enabled instead of the
    /// direct-only "Minimal" preset `IrohServer::connect` uses by default.
    #[arg(long)]
    relay: bool,

    /// Point at a non default iroh relay instead of whatever iroh's "N0" preset
    /// has. e.g. `http://127.0.0.1:3340` ref the `relay_server` example. Takes
    /// precedence over `--relay` if both are given at the same time.
    #[arg(long)]
    relay_url: Option<String>,
}

/// No-op endpoint hooks, same trivial empty impl the crate itself uses
/// internally - this example has no hooks of its own, it just needs
/// *something* implementing `EndpointHooks` to call `IrohServer::bind_with`.
#[derive(Debug, Clone, Copy)]
struct NopHooks;

impl bevy_replicon_iroh::iroh::endpoint::EndpointHooks for NopHooks {}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let runtime = tokio::runtime::Runtime::new()?;
    let handle = IrohTokioHandle(runtime.handle().clone());

    // A real application should persist this key so its identity survives a
    // restart kept ephemeral here for simplicity as an example. Note you can
    // use iroh to generate this too.
    let secret_key = bevy_replicon_iroh::iroh::SecretKey::generate();

    let server = if let Some(relay_url) = &args.relay_url {
        let relay_map = bevy_replicon_iroh::iroh::RelayMap::try_from_iter([relay_url.as_str()])
            .map_err(|e| anyhow::anyhow!("invalid --relay-url {relay_url}: {e:#}"))?;
        IrohServer::bind_with(
            &handle,
            bevy_replicon_iroh::minimal_with_relay(bevy_replicon_iroh::iroh::RelayMode::Custom(
                relay_map,
            )),
            secret_key,
            args.bind,
            Vec::new(),
            NopHooks,
            IrohServerConfig::default(),
        )
    } else if args.relay {
        IrohServer::bind_with(
            &handle,
            bevy_replicon_iroh::iroh::endpoint::presets::N0,
            secret_key,
            args.bind,
            Vec::new(),
            NopHooks,
            IrohServerConfig::default(),
        )
    } else {
        IrohServer::bind(&handle, secret_key, args.bind)
    }
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
    // `Replicated` is REQUIRED here, by default no entities are replicated in
    // bevy_replicon, even if component type is registered via
    // `.replicate::<T>()`, Without this marker the entity is never sent to any
    // client. Don't forget it.
    commands.spawn((Counter(0), Replicated));
}

fn log_state(counters: Query<&Counter>) {
    let value = counters.iter().next().map(|c| c.0).unwrap_or(0);
    tracing::info!("state: role=server counter={value}");
}
