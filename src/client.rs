//! Client messaging backend
//! Dials one iroh peer and drives `bevy_replicon`'s `ClientMessages` `ClientState` directly.
//!
//! Mirrors `bevy_replicon_renet`'s `RepliconRenetClientPlugin` almost exactly for now.

use std::net::SocketAddr;
use std::time::Duration;

use bevy::prelude::*;
use bytes::Bytes;
use crossbeam_channel::{Receiver, unbounded};
use iroh::endpoint::{Connection, Endpoint, EndpointHooks, SendStream};
use iroh::{EndpointAddr, SecretKey};

use bevy_replicon::prelude::*;
use bevy_replicon::shared::backend::channels::Channel;

use crate::frame;
use crate::{ALPN, IrohTokioHandle, REPLICON_STREAM_PRIORITY};

/// No-op endpoint hooks for [`IrohClient::connect`] - matches
/// [`crate::server::IrohServer::bind`]'s own `NopHooks` (kept as a
/// separate copy per module rather than shared, since it's a trivial
/// empty trait impl either way).
#[derive(Debug, Clone, Copy)]
struct NopHooks;

impl EndpointHooks for NopHooks {}

/// Iroh client configuration for [`RepliconIrohClientPlugin`], also usable
/// standalone with [`IrohClient::connect`], [`IrohClient::connect_with`],
/// [`IrohClient::disconnect`] without adding the plugin.
#[derive(Resource, Debug, Clone, Copy)]
pub struct IrohClientConfig {
    /// Upper bound on how long [`IrohClient::connect`] and
    /// [`IrohClient::connect_with`] will block waiting for the endpoint to
    /// bind, the QUIC handshake to complete, and the replicon-marker
    /// handshake on top of it. Without this, dialing an unreachable/dead peer
    /// could block the calling thread indefinitely, since `connect` runs
    /// synchronously, typically from inside a bevy exclusive system.
    /// Iroh/QUIC itself has no built-in bound on a dial that never gets a
    /// response. The 15s default is chosen somewhat arbitrarily to be large
    /// enough for real-world NAT traversal/path negotiation rather than
    /// derived from any protocol-level deadline.
    pub connect_timeout: Duration,
    /// Upper bound [`IrohClient::disconnect`] waits for the connection,
    /// endpoint, and background accept task to close gracefully before giving
    /// up. Captured via [`IrohClient`] calling [`IrohClient::connect`] or
    /// [`IrohClient::connect_with`].
    pub disconnect_timeout: Duration,
}

impl Default for IrohClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            disconnect_timeout: Duration::from_secs(3),
        }
    }
}

/// `config` is inserted as a resource during [`Plugin::build`] time so it can
/// be read back out before calling [`IrohClient::connect`] and
/// [`IrohClient::connect_with`].
#[derive(Default)]
pub struct RepliconIrohClientPlugin {
    pub config: IrohClientConfig,
}

impl Plugin for RepliconIrohClientPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().contains_resource::<IrohTokioHandle>(),
            "RepliconIrohClientPlugin requires IrohTokioHandle to be inserted before it is added!"
        );

        app.insert_resource(self.config);

        app.add_systems(
            PreUpdate,
            (
                set_connected.run_if(resource_added::<IrohClient>),
                set_disconnected.run_if(resource_removed::<IrohClient>),
                (check_disconnected, receive_packets)
                    .chain()
                    .run_if(resource_exists::<IrohClient>),
            )
                .in_set(ClientSystems::ReceivePackets),
        );

        app.add_systems(
            PostUpdate,
            send_packets
                .in_set(ClientSystems::SendPackets)
                .run_if(resource_exists::<IrohClient>),
        );
    }
}

fn set_connected(mut state: ResMut<NextState<ClientState>>) {
    state.set(ClientState::Connected);
}

fn set_disconnected(mut state: ResMut<NextState<ClientState>>) {
    state.set(ClientState::Disconnected);
}

/// Client-side iroh transport resource. Insert this after connecting via
/// [`IrohClient::connect`] to start exchanging replicon traffic over Iroh.
///
/// Typically you'd use a Startup system for this task.
#[derive(Resource)]
pub struct IrohClient {
    endpoint: Endpoint,
    connection: Connection,
    send_stream: SendStream,
    recv_rx: Receiver<(u8, Bytes)>,
    disconnected_rx: Receiver<()>,
    /// Here for connections that negotiate an ALPN other than this crate's own, ref:
    /// [`Self::connect_with`]/[`Self::recv_extra`].
    extra_rx: Receiver<(Vec<u8>, Connection)>,
    /// The background accept-loop task spawned in `connect_with`. This holds
    /// its own `Endpoint` clone(), separate from `self.endpoint` above.
    /// `disconnect` awaits directly upon this and `IrohServer::shutdown`'s as
    /// well so that the OS UDP socket is actually guaranteed free by the time
    /// this returns.
    accept_task: tokio::task::JoinHandle<()>,
    /// [`IrohClientConfig::disconnect_timeout`] captured at
    /// [`Self::connect`]/[`Self::connect_with`] time, used by
    /// [`Self::disconnect`].
    disconnect_timeout: Duration,
}

impl IrohClient {
    /// Binds a local iroh [`Endpoint`] to `bind_addr` using
    /// [`iroh::endpoint::presets::Minimal`] preset and dial `peer` directly.
    /// the `peer` *must* already contain a direct socket address via
    /// `EndpointAddr::new(id).with_ip_addr(addr)`. This blocks until the
    /// connection and persistent streams are fully established. Use
    /// [`Self::connect_with`] directly to provide a different preset, e.g. one
    /// of iroh's relay-enabled presets or your own tweaked config.
    pub fn connect(
        handle: &IrohTokioHandle,
        secret_key: SecretKey,
        bind_addr: SocketAddr,
        peer: EndpointAddr,
    ) -> anyhow::Result<Self> {
        Self::connect_with(
            handle,
            iroh::endpoint::presets::Minimal,
            secret_key,
            bind_addr,
            peer,
            Vec::new(),
            NopHooks,
            IrohClientConfig::default(),
        )
    }

    /// Same as [`Self::connect`], but additionally lets you add `extra_alpns`
    /// onto the same endpoint/port, and installs `hooks` on that endpoint the
    /// client-side mirror of [`crate::server::IrohServer::bind_with`], for
    /// applications that want their own peer-to-peer side-channel traffic
    /// reachable regardless of whether a given node is currently playing the
    /// client or server role for replicon itself. Use this as a backchannel
    /// over Iroh outside of replicon traffic for whatever nefarious purpose you
    /// can concoct.
    ///
    /// `preset` is simply given to [`iroh::Endpoint::builder`], mirroring
    /// [`crate::server::IrohServer::bind_with`] for the relay, discovery, and
    /// every other knob iroh exposes is entirely up to the caller to provide.
    /// Pass [`iroh::endpoint::presets::Minimal`] for direct, p2p only behavior,
    /// or one of iroh's presets e.g. `presets::N0` or implement
    /// [`iroh::endpoint::presets::Preset`] yourself for anything that falls
    /// outside of any config.
    ///
    /// A client's endpoint is bound at `bind_addr` exactly like a server is.
    /// Iroh doesn't distinguish "client" vs "server" at the `Endpoint` level,
    /// only this crate's cares for replicon. Just noe it's equally capable of
    /// *accepting* inbound connections on `extra_alpns` while this value is
    /// provided, via a stupid background accept loop spawned here mirroring
    /// `IrohServer::bind_with`'s approach. Any inbound connection negotiating
    /// this crate's own [`ALPN`] is unexpected and is rejected outright rather
    /// than silently accepted. A client only ever dials out once, for its
    /// single server connection, don't try to replicate the crates ALPN
    /// negotiation using this.
    ///
    /// Deliberately does *not* "dial from a second, throwaway `Endpoint`
    /// alongside this one" as in testing two live `Endpoint`s under the same
    /// identity seemed to a *peer's* own per-remote path/NAT-traversal
    /// tracking, which expects one logical connection per remote identity. One
    /// `Endpoint` per identity at a time, extended to carry extra ALPNs when
    /// needed seems to work the best under load. This extra alpn approach and
    /// `IrohServer::bind_with` are how callers get that without a second UDP
    /// socket/port in the mix.
    // TODO: too many args... probably worth a Default struct here.
    #[allow(clippy::too_many_arguments)]
    pub fn connect_with(
        handle: &IrohTokioHandle,
        preset: impl iroh::endpoint::presets::Preset,
        secret_key: SecretKey,
        bind_addr: SocketAddr,
        peer: EndpointAddr,
        extra_alpns: Vec<Vec<u8>>,
        hooks: impl EndpointHooks + 'static,
        config: IrohClientConfig,
    ) -> anyhow::Result<Self> {
        handle.0.block_on(async {
            // Bounded by config.connect_timeout so unreachable peers don't block forever.
            let peer_debug = format!("{peer:?}");
            let handshake = async {
                tracing::debug!("connect: building endpoint");
                let mut alpns = vec![ALPN.to_vec()];
                alpns.extend(extra_alpns);
                let builder = iroh::Endpoint::builder(preset)
                    .secret_key(secret_key)
                    .alpns(alpns)
                    .hooks(hooks)
                    .bind_addr(bind_addr)
                    .map_err(anyhow::Error::from)?;
                tracing::debug!("connect: binding endpoint");
                let endpoint = builder.bind().await?;
                tracing::debug!("connect: endpoint bound, dialing peer {peer:?}");

                let connection = endpoint.connect(peer, ALPN).await?;
                tracing::debug!("connect: quic connection established, opening uni stream");

                let mut send_stream = connection.open_uni().await?;
                tracing::debug!("connect: uni stream opened, writing marker");
                send_stream.set_priority(REPLICON_STREAM_PRIORITY)?;
                frame::write_marker(&mut send_stream).await?;
                tracing::debug!("connect: marker written, accepting peer's uni stream");

                let mut recv_stream = connection.accept_uni().await?;
                tracing::debug!("connect: peer uni stream accepted, reading marker");
                frame::read_marker(&mut recv_stream).await?;
                tracing::debug!("connect: marker read, handshake complete");

                let (tx, recv_rx) = unbounded::<(u8, Bytes)>();

                {
                    let tx = tx.clone();
                    handle.0.spawn(async move {
                        while let Some((channel_id, bytes)) =
                            frame::read_frame(&mut recv_stream).await
                        {
                            if tx.send((channel_id, bytes)).is_err() {
                                break;
                            }
                        }
                    });
                }

                {
                    let tx = tx.clone();
                    let connection = connection.clone();
                    handle.0.spawn(async move {
                        loop {
                            match connection.read_datagram().await {
                                Ok(datagram) => {
                                    if let Some((channel_id, payload)) =
                                        frame::decode_datagram(&datagram)
                                        && tx.send((channel_id, payload)).is_err()
                                    {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("iroh datagram reader ending: {e}");
                                    break;
                                }
                            }
                        }
                    });
                }

                let (dtx, disconnected_rx) = unbounded();
                {
                    let connection = connection.clone();
                    handle.0.spawn(async move {
                        let reason = connection.closed().await;
                        tracing::debug!("iroh server disconnected: {reason}");
                        let _ = dtx.send(());
                    });
                }

                // Background accept loop for inbound extra-ALPN connections.
                // Same crap as `IrohServer::bind_with`'s accept loop, except
                // this crate's own `ALPN` is unexpected if given here, a client
                // never expects inbound replicon connections and is rejected
                // outright. Don't do that.
                let (extra_tx, extra_rx) = unbounded();
                let accept_endpoint = endpoint.clone();
                let accept_handle = handle.0.clone();
                let accept_task = handle.0.spawn(async move {
                    loop {
                        let Some(incoming) = accept_endpoint.accept().await else {
                            tracing::debug!("iroh client endpoint closed, accept loop exiting");
                            break;
                        };
                        let extra_tx = extra_tx.clone();
                        accept_handle.spawn(async move {
                            let connection: Connection = match incoming.await {
                                Ok(connection) => connection,
                                Err(e) => {
                                    tracing::debug!(
                                        "iroh client: inbound connection failed: {e:#}"
                                    );
                                    return;
                                }
                            };
                            if connection.alpn() == ALPN {
                                tracing::debug!(
                                    "iroh client: rejecting unexpected inbound connection on \
                                     replicon ALPN. A replicon client should only dial out"
                                );
                                connection.close(0u32.into(), b"unexpected inbound connection");
                                return;
                            }
                            let alpn = connection.alpn().to_vec();
                            if extra_tx.send((alpn, connection)).is_err() {
                                tracing::debug!(
                                    "IrohClient dropped before an extra-ALPN connection could \
                                     be delivered"
                                );
                            }
                        });
                    }
                });

                Ok(Self {
                    endpoint,
                    connection,
                    send_stream,
                    recv_rx,
                    disconnected_rx,
                    extra_rx,
                    accept_task,
                    disconnect_timeout: config.disconnect_timeout,
                })
            };

            match tokio::time::timeout(config.connect_timeout, handshake).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!(
                    "connect: timed out after {:?} dialing {peer_debug}",
                    config.connect_timeout
                )),
            }
        })
    }

    /// The underlying iroh connection, exposed so applications can open
    /// additional streams beyond replicon's own reserved channel traffic.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The underlying iroh [`Endpoint`], exposed so applications can dial
    /// *out* to other peers themselves, reusing this same bound
    /// socket/identity.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Return one non-blocking pending connection that negotiated an ALPN other
    /// than this crate's own [`ALPN`], or `None` if there is none waiting. Only
    /// yields `Some` if [`Self::connect_with`] was given `extra_alpns`. Call
    /// this from an application `Update` system each tick, same as
    /// [`crate::server::IrohServer:: recv_extra`].
    pub fn recv_extra(&self) -> Option<(Vec<u8>, Connection)> {
        self.extra_rx.try_recv().ok()
    }

    /// Gracefully (hopefully...) close the connection and endpoint. Call via
    /// [`IrohTokioHandle`] before dropping and removing the
    /// `IrohClient` resource if a clean disconnect matters. Aka if you are
    /// switching this bevy App to/from a server role. NB: dropping it directly
    /// abandons the connection instead of closing it so you'll leak resources.
    ///
    /// Waits up to [`IrohClientConfig::disconnect_timeout`] as a best effort
    /// close timeout for the transport and os level to close. Otherwise the
    /// peer will eventually timeout once the connection is locally closed
    /// without this.
    ///
    /// Also waits in that same window for the background extra-ALPN
    /// accept-loop task spawned in `connect_with` to exit. Which is the same
    /// "Address already in use" race `IrohServer::shutdown` guards against as
    /// well as that task holds its own `Endpoint` clone, independent of the one
    /// `close()` above acts on.
    pub fn disconnect(self, handle: &IrohTokioHandle) {
        let disconnect_timeout = self.disconnect_timeout;
        handle.0.block_on(async {
            self.connection.close(0u32.into(), b"disconnecting");
            self.endpoint.close().await;

            if tokio::time::timeout(disconnect_timeout, self.accept_task)
                .await
                .is_err()
            {
                tracing::warn!(
                    "iroh client accept task didn't exit within {disconnect_timeout:?} of closing, the \
                     currently bound UDP port may not be immediately reusable"
                );
            }
        });
    }
}

fn check_disconnected(mut commands: Commands, client: Res<IrohClient>) {
    if client.disconnected_rx.try_recv().is_ok() {
        commands.remove_resource::<IrohClient>();
    }
}

fn receive_packets(client: ResMut<IrohClient>, mut messages: ResMut<ClientMessages>) {
    while let Ok((channel_id, bytes)) = client.recv_rx.try_recv() {
        messages.insert_received(channel_id as usize, bytes);
    }
}

fn send_packets(
    handle: Res<IrohTokioHandle>,
    channels: Res<RepliconChannels>,
    mut client: ResMut<IrohClient>,
    mut messages: ResMut<ClientMessages>,
) {
    let client_channels = channels.client_channels();
    let IrohClient {
        connection,
        send_stream,
        ..
    } = &mut *client;

    handle.0.block_on(async {
        for (channel_id, bytes) in messages.drain_sent() {
            if matches!(client_channels.get(channel_id), Some(Channel::Unreliable)) {
                let datagram = frame::encode_datagram(channel_id as u8, &bytes);
                if let Err(e) = connection.send_datagram(datagram) {
                    tracing::debug!("failed to send datagram: {e}");
                }
            } else if let Err(e) = frame::write_frame(send_stream, channel_id as u8, &bytes).await {
                tracing::debug!("failed to write frame: {e:#}");
            }
        }
    });
}
