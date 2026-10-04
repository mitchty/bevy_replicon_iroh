//! Minimal client example
//!
//! Simply connects to the  `echo_server` and logs `Counter`.
//!
//! Run via ye olde car go:
//! ```sh
//! cargo run --example echo_client -- --server-id IROH_ID --server-addr 127.0.0.1:4433
//!     ```
//!
//! Hint: just paste the Iroh identity `IROH_ID` from what `echo_server`'s
//! id=...` line or don't its your life. Pass `--relay` on both example binaries
//! to use the default iroh preset that does relay and discovery instead of the
//! direct-only default this library includes, or `--relay-url URL` on both ends
//! to point at your own relay which takes precedence over whatever `--relay` is
//! given.

use std::net::SocketAddr;
use std::time::Duration;

use bevy::MinimalPlugins;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::common_conditions::on_timer;
use bevy_replicon::prelude::*;
use bevy_replicon_iroh::examples_common::Counter;
use bevy_replicon_iroh::iroh::{EndpointAddr, EndpointId};
use bevy_replicon_iroh::{IrohClient, IrohClientConfig, IrohTokioHandle, RepliconIrohPlugins};
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

    /// Use iroh's "N0" preset with relay and discovery enabled instead of the
    /// direct-only "Minimal" preset `IrohClient::connect` uses by default.
    #[arg(long)]
    relay: bool,

    /// Point at a non default iroh relay instead of whatever iroh's "N0" preset
    /// has. e.g. `http://127.0.0.1:3340` ref the `relay_server` example. Takes
    /// precedence over `--relay` if both are given at the same time.
    #[arg(long)]
    relay_url: Option<String>,
}

/// No-op endpoint hooks, same trivial empty impl the crate itself uses
/// internally this example has no hooks of its own, it just needs something
/// implementing `EndpointHooks` to call `IrohClient::connect_with`.
#[derive(Debug, Clone, Copy)]
struct NopHooks;

impl bevy_replicon_iroh::iroh::endpoint::EndpointHooks for NopHooks {}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let runtime = tokio::runtime::Runtime::new()?;
    let handle = IrohTokioHandle(runtime.handle().clone());

    let secret_key = bevy_replicon_iroh::iroh::SecretKey::generate();

    // A plain IP-only EndpointAddr is enough when dialing directly, but with a
    // self-hosted relay there's no discovery service to tell the client the
    // server is reachable via that relay so we add that in as well.
    let mut peer_addrs = vec![bevy_replicon_iroh::iroh::TransportAddr::Ip(
        args.server_addr,
    )];
    if let Some(relay_url) = &args.relay_url {
        let relay_url: bevy_replicon_iroh::iroh::RelayUrl = relay_url
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid --relay-url {relay_url}: {e:#}"))?;
        peer_addrs.push(bevy_replicon_iroh::iroh::TransportAddr::Relay(relay_url));
    }
    let peer = EndpointAddr::from_parts(args.server_id, peer_addrs);

    let client = if let Some(relay_url) = &args.relay_url {
        let relay_map = bevy_replicon_iroh::iroh::RelayMap::try_from_iter([relay_url.as_str()])
            .map_err(|e| anyhow::anyhow!("invalid --relay-url {relay_url}: {e:#}"))?;
        IrohClient::connect_with(
            &handle,
            bevy_replicon_iroh::minimal_with_relay(bevy_replicon_iroh::iroh::RelayMode::Custom(
                relay_map,
            )),
            secret_key,
            args.bind,
            peer,
            Vec::new(),
            NopHooks,
            IrohClientConfig::default(),
        )
    } else if args.relay {
        IrohClient::connect_with(
            &handle,
            bevy_replicon_iroh::iroh::endpoint::presets::N0,
            secret_key,
            args.bind,
            peer,
            Vec::new(),
            NopHooks,
            IrohClientConfig::default(),
        )
    } else {
        IrohClient::connect(&handle, secret_key, args.bind, peer)
    }
    .map_err(|e| {
        anyhow::anyhow!(
            "failed to connect to {} ({}): {e:#}",
            args.server_addr,
            args.server_id
        )
    })?;
    tracing::info!(id = %args.server_id, addr = %args.server_addr, "echo_client: connected");

    let mut app = App::new();
    // IrohTokioHandle must be inserted before `RepliconIrohPlugins` is
    // added.
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

fn log_state(client: Option<Res<IrohClient>>, counters: Query<&Counter>) {
    let value = counters.iter().next().map(|c| c.0).unwrap_or(0);
    let path = client
        .and_then(|client| {
            client
                .connection()
                .paths()
                .iter()
                .find(|p| p.is_selected())
                .map(|p| if p.is_relay() { "relay" } else { "direct" })
        })
        .unwrap_or("unknown");
    tracing::info!("state: role=client counter={value} path={path}");
}
