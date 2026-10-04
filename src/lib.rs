//! A minimal [`iroh`](https://docs.rs/iroh) (QUIC, p2p-by-public-key)
//! messaging backend for [`bevy_replicon`](https://docs.rs/bevy_replicon).
//!
//! Short reality:
//!
//! - **Direct p2p by default, with optional relay.** [`IrohServer::bind`] and
//!   [`IrohClient::connect`] use iroh's [`iroh::endpoint::presets::Minimal`]
//!   preset which is direct or p2p only.
//!   [`IrohServer::bind_with`]/[`IrohClient::connect_with`] may instead take
//!   any [`iroh::endpoint::presets::Preset`], so you can pass in Iroh
//!   `presets::N0` or a preset of your own. Peers are still always dialed via a
//!   known [`iroh::EndpointId`] aka public key and a [`std::net::SocketAddr`];
//!   getting that address to a peer out of band is still a *you* problem
//!   regardless of preset.
//! - **All of replicon's own traffic shares a single priority tier.**
//!   I couldn't brain up a reason to make more tiers per channel so I didn't.
//!   Applications can abuse the connection directly anyway to do evil so it
//!   seems fine as-is. Read next bullet. - **Another ALPN/connection is easily
//!   used on the same bound port/identity**, on either: [`IrohServer::bind_with`]
//!   or [`IrohClient:: connect_with`]. Both sides can additionally negotiate
//!   their own `extra_alpns` and install your own
//!   `iroh::endpoint::EndpointHooks` directly on the Iroh endpoint like the hack
//!   that I am. Connections on these ALPNs are handed directly via `recv_extra`
//!   instead of being treated as replicon traffic over the replicon ALPN. The
//!   `endpoint` exposes the same bound `Endpoint` for dialing directly *out* to
//!   peers via those ALPNs. Which is useful for daemon to daemon applications
//!   wanting their own peer-to-peer side-channel traffic separate from any
//!   replicon client traffic. NB: This is deliberately symmetric on both
//!   client/server sides rather than only server side. "Weird shit" happened with
//!   multiple `Endpoints`. This is here to remind future me not to try that again
//!   but I probably will to spite past me.
//! - **No Iroh/Replicon connection stats reporting so far** `ClientStats` and
//!   `ConnectedClientStats` stay at zero, which `bevy_replicon`'s docs said was
//!   ok. I didn't look how much pain this might be to add. Pull requests
//!   welcome I'm lazy.
//! - **No cargo feature gating** I couldn't come up with a reason for that
//!   renet behavior. So never bothered making this more complicated than it
//!   isn't. Open an issue with a rationale and I'm willing to add it but not
//!   sure having both Resources in a bevy app is that big of a problem.
//!
//! # Tokio and iroh and bevy, or before you yell at mitch for tokio
//!
//! Every iroh I/O call is `async` unlike renet which is synchronous and polling
//! based. So this crate needs (for now, `tokio` can be an optional feature in
//! future) a `tokio` runtime handle from callers, ref: [`IrohTokioHandle`].
//! This intentionally depends on `tokio` to keep dependency footprints
//! minimal...ish.
//!
//! Receiving traffic is handled by background tasks that are spawned once per
//! connection that continuously read from the Iroh persistent stream and
//! datagrams and push `(channel_id, Bytes)` into an Iroh channel. The sync
//! `receive_packets` bevy systems just drain whatever has accumulated each Bevy
//! engine tick. Sending traffic is handled synchronously via `Handle::block_on`
//! in the `send_packets` bevy systems. Which I find simpler than a send-side
//! background task. But can be convinced I'm an idiot and did this all wrong
//! just open an issue and yell at me there I guess. Iroh handles the ordering
//! of packets at this point anyway so shrug.
pub mod client;
#[doc(hidden)] // If I bother yeeting this up to crates.io
pub mod examples_common;
pub mod frame;
pub mod server;

pub use iroh;

use bevy::prelude::*;

pub use client::{IrohClient, IrohClientConfig, RepliconIrohClientPlugin};
pub use server::{IrohConnection, IrohServer, IrohServerConfig, RepliconIrohServerPlugin};

/// ALPN identifying this protocol during iroh's TLS handshake. Roughly
/// equivalent to the renet/netcode `PROTOCOL_ID`. Peers using a different ALPN
/// will fail to connect.
// TODO: Configurable in future?
pub const ALPN: &[u8] = b"bevy-replicon-iroh/0";

/// Iroh stream Priority, ref `SendStream::set_priority` assigned to the streams
/// this crate opens for `bevy_replicon`'s own reserved channel traffic. Calling
/// applications that layer additional iroh streams on top of the same
/// `Connection` should keep their own stream priorities at or below this value
/// if replicon traffic should always win contention for any traffic contention.
/// iroh's own docs recommend keeping the *number* of distinct priority levels
/// per connection small, so treat this as "the highest priority tier".
pub const REPLICON_STREAM_PRIORITY: i32 = 100;

/// Builds an [`iroh::endpoint::presets::Preset`] based off
/// [`iroh::endpoint::presets::Minimal`] except it also applies `relay_mode`.
/// Mostly here just to use an http relay in the examples for now but if you
/// want to use it too its here.
pub fn minimal_with_relay(relay_mode: iroh::RelayMode) -> impl iroh::endpoint::presets::Preset {
    struct MinimalWithRelay(iroh::RelayMode);

    impl iroh::endpoint::presets::Preset for MinimalWithRelay {
        fn apply(self, builder: iroh::endpoint::Builder) -> iroh::endpoint::Builder {
            builder
                .preset(iroh::endpoint::presets::Minimal)
                .relay_mode(self.0)
        }
    }

    MinimalWithRelay(relay_mode)
}

/// `tokio::runtime::Handle` resource this crate needs the caller to provide and
/// insert before adding [`RepliconIrohServerPlugin`] and
/// [`RepliconIrohClientPlugin`], or before calling [`IrohServer::bind`]
/// [`IrohClient::connect`] directly.
///
// TODO: Add a unit test for this proof later.
#[derive(Resource, Clone)]
pub struct IrohTokioHandle(pub tokio::runtime::Handle);

/// RepliconIrohPlugins adds both Server and Client Bevy Plugin Resources. That
/// way if you wanted to you could swap a Client to Server or vice-versa. I
/// don't know why you would want to but it fit well with the mesh based Iroh
/// approach.
///
/// Just don't insert both `IrohServer` and `IrohClient`. I didn't plan for that
/// and honestly with the Iroh back channel see no need for it. Note no other
/// replicon crates can do this anyway so... yeah.
pub struct RepliconIrohPlugins;

impl PluginGroup for RepliconIrohPlugins {
    fn build(self) -> bevy::app::PluginGroupBuilder {
        bevy::app::PluginGroupBuilder::start::<Self>()
            .add(RepliconIrohServerPlugin::default())
            .add(RepliconIrohClientPlugin::default())
    }
}

#[cfg(test)]
mod tests {
    use bevy::MinimalPlugins;
    use bevy::state::app::StatesPlugin;

    use super::*;

    #[test]
    fn plugins_register_alongside_replicon_without_panicking() {
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let mut app = App::new();
        app.insert_resource(IrohTokioHandle(runtime.handle().clone()))
            .add_plugins((MinimalPlugins, StatesPlugin, bevy_replicon::RepliconPlugins))
            .add_plugins(RepliconIrohPlugins);

        app.update();
    }

    #[test]
    #[should_panic(expected = "RepliconIrohServerPlugin requires IrohTokioHandle")]
    fn server_plugin_panics_without_iroh_tokio_handle() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin, bevy_replicon::RepliconPlugins))
            .add_plugins(RepliconIrohServerPlugin::default());
    }

    #[test]
    #[should_panic(expected = "RepliconIrohClientPlugin requires IrohTokioHandle")]
    fn client_plugin_panics_without_iroh_tokio_handle() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin, bevy_replicon::RepliconPlugins))
            .add_plugins(RepliconIrohClientPlugin::default());
    }
}
