//! Server messaging backend
//! Simple Bevy plugin that accepts incoming iroh connections and drives
//! `bevy_replicon`'s `ServerMessages`, `ConnectedClient`, `ServerState` like renet for now too.
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bevy::prelude::*;
use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender, unbounded};
use iroh::endpoint::{Connection, Endpoint, EndpointHooks};
use iroh::{EndpointId, SecretKey};

use bevy_replicon::prelude::*;
use bevy_replicon::shared::backend::channels::Channel;
use bevy_replicon::shared::backend::connected_client::{NetworkId, NetworkIdMap};

use crate::frame;
use crate::{ALPN, IrohTokioHandle, REPLICON_STREAM_PRIORITY};

pub struct RepliconIrohServerPlugin;

impl Plugin for RepliconIrohServerPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.world().contains_resource::<IrohTokioHandle>(),
            "RepliconIrohServerPlugin requires IrohTokioHandle to be inserted before it is added!"
        );

        app.add_observer(disconnect_client).add_systems(
            PreUpdate,
            (
                set_running.run_if(resource_added::<IrohServer>),
                set_stopped.run_if(resource_removed::<IrohServer>),
                (accept_connections, check_disconnects, receive_packets)
                    .chain()
                    .run_if(resource_exists::<IrohServer>),
            )
                .in_set(ServerSystems::ReceivePackets),
        );

        app.add_systems(
            PostUpdate,
            (
                send_packets
                    .in_set(ServerSystems::SendPackets)
                    .run_if(resource_exists::<IrohServer>),
                disconnect_by_request,
            ),
        );
    }
}

fn set_running(mut state: ResMut<NextState<ServerState>>) {
    state.set(ServerState::Running);
}

fn set_stopped(mut state: ResMut<NextState<ServerState>>) {
    state.set(ServerState::Stopped);
}

/// The underlying iroh connection behind [`ConnectedClient`], exposed so
/// clients can open additional streams beyond replicon's own reserved channel
/// traffic and share the underlying connection rather than opening another one
/// which broke so much stuff. Don't do that learn from my mistakes.
#[derive(Component, Clone)]
pub struct IrohConnection(pub Connection);

/// Server to client persistent stream. Internal crate use only. Clients
/// wanting extra streams or to share data should use [`IrohConnection`], not this!
#[derive(Component)]
struct IrohSendStream(iroh::endpoint::SendStream);

/// For receiving half of the bridge from a client's background reader tasks
/// data (persistent stream + datagrams) into the sync `receive_packets` system.
#[derive(Component)]
struct IrohRecvRx(Receiver<(u8, Bytes)>);

struct AcceptedPeer {
    network_id: NetworkId,
    connection: Connection,
    send_stream: iroh::endpoint::SendStream,
    recv_rx: Receiver<(u8, Bytes)>,
}

/// Server side iroh transport resource. Insert this to start accepting
/// connections, same as `renet`'s server.
#[derive(Resource)]
pub struct IrohServer {
    endpoint: Endpoint,
    accepted_rx: Receiver<AcceptedPeer>,
    disconnected_rx: Receiver<NetworkId>,
    /// Here for connections that negotiated an ALPN other than this crate's own.
    extra_rx: Receiver<(Vec<u8>, Connection)>,
    /// The background accept-loop task spawned in `bind_with`. This holds its
    /// own `Endpoint` clone ref; `accept_endpoint`, separate from
    /// `self.endpoint` above. `shutdown` awaits directly upon this so the OS
    /// UDP socket is actually guaranteed free and not simply "closed" at the
    /// QUIC protocol level when it returns.
    accept_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy)]
struct NopHooks;

impl EndpointHooks for NopHooks {}

impl IrohServer {
    /// Bind an iroh [`Endpoint`] to `bind_addr`. Direct P2P only, no
    /// relay or address-lookup/discovery services yet. Spawns a
    /// background accept loop on `handle`.
    ///
    /// `secret_key` is this server's persistent identity. Callers are
    /// responsible for generating and persisting the key and getting
    /// `secret_key.public()` to peers out of band so they can construct an
    /// `EndpointAddr` to dial into.
    ///
    /// Key exchange is a *you* problem, this crate makes *no* attempt to
    /// simplify key exchange in iroh.
    pub fn bind(
        handle: &IrohTokioHandle,
        secret_key: SecretKey,
        bind_addr: SocketAddr,
    ) -> anyhow::Result<Self> {
        Self::bind_with(handle, secret_key, bind_addr, Vec::new(), NopHooks)
    }

    /// Same as [`Self::bind`], but negotiates `extra_alpns` on the
    /// same bound endpoint and port, and installs `hooks` on the iroh endpoint.
    ///
    /// This lets a client layer its own peer-to-peer side-channel traffic on
    /// the very same bound endpoint and identity alongside this crate's own
    /// replicon control-plane connections without needing a second UDP
    /// socket/port. Should simplify setting up p2p meshes betwixt daemons which
    /// is what I abuse this for. So you can embrace the madness too!
    ///
    /// `hooks` applies to all connections on this endpoint regardless of the
    /// ALPN. Iroh hooks are endpoint-wide, not per-ALPN, including connections
    /// this crate dials out to itself. If the ALPN matters, `connection.alpn()`
    /// should be checked by the caller this crate has no opinion on the matter
    /// outside of its own ALPN's.
    pub fn bind_with(
        handle: &IrohTokioHandle,
        secret_key: SecretKey,
        bind_addr: SocketAddr,
        extra_alpns: Vec<Vec<u8>>,
        hooks: impl EndpointHooks + 'static,
    ) -> anyhow::Result<Self> {
        let (accepted_tx, accepted_rx) = unbounded();
        let (disconnected_tx, disconnected_rx) = unbounded();
        let (extra_tx, extra_rx) = unbounded();
        let next_network_id = std::sync::Arc::new(AtomicU64::new(0));

        let mut alpns = vec![ALPN.to_vec()];
        alpns.extend(extra_alpns);

        let endpoint = handle.0.block_on(async {
            let builder = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
                .secret_key(secret_key)
                .alpns(alpns)
                .hooks(hooks)
                .bind_addr(bind_addr)
                .map_err(anyhow::Error::from)?;
            builder.bind().await.map_err(anyhow::Error::from)
        })?;

        tracing::info!(
            endpoint_id = %endpoint.id().fmt_short(),
            addr = %bind_addr,
            "iroh server listening"
        );

        let accept_endpoint = endpoint.clone();
        let accept_handle = handle.0.clone();
        let accept_task = handle.0.spawn(async move {
            loop {
                let Some(incoming) = accept_endpoint.accept().await else {
                    tracing::debug!("iroh server endpoint closed, accept loop exiting");
                    break;
                };
                let tx = accepted_tx.clone();
                let dtx = disconnected_tx.clone();
                let extra_tx = extra_tx.clone();
                let task_handle = accept_handle.clone();
                let next_id = next_network_id.clone();
                accept_handle.spawn(async move {
                    let connection: Connection = match incoming.await {
                        Ok(connection) => connection,
                        Err(e) => {
                            tracing::debug!("iroh incoming connection failed: {e:#}");
                            return;
                        }
                    };

                    // Route by negotiated ALPN: this crate's own traffic keeps
                    // going through the replicon marker handshake below.
                    // Everything else is handed straight to the callers via
                    // `recv_extra` to deal with themselves. That stuffs a
                    // *them* problem.
                    if connection.alpn() != ALPN {
                        let alpn = connection.alpn().to_vec();
                        if extra_tx.send((alpn, connection)).is_err() {
                            tracing::debug!(
                                "IrohServer dropped before an extra-ALPN connection could be delivered"
                            );
                        }
                        return;
                    }

                    match accept_peer(connection, &task_handle, &next_id, dtx).await {
                        Ok(peer) => {
                            if tx.send(peer).is_err() {
                                tracing::debug!(
                                    "IrohServer dropped before peer handshake finished"
                                );
                            }
                        }
                        Err(e) => tracing::warn!("iroh peer handshake failed: {e:#}"),
                    }
                });
            }
        });

        Ok(Self {
            endpoint,
            accepted_rx,
            disconnected_rx,
            extra_rx,
            accept_task,
        })
    }

    /// This server's dialable identity so peers can construct an
    /// `EndpointAddr` to connect to.
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// The actual local socket addresses this server is actually bound to.
    /// Useful when binding to an ephemeral port (`:0`) and the caller needs to
    /// learn the port the OS actually gave. Mostlyhere to hand to peers
    /// alongside [`Self::id`] or in os level integration tests. Ref the nix dir
    /// for the nixos os tests this gets abused with.
    pub fn bound_sockets(&self) -> Vec<SocketAddr> {
        self.endpoint.bound_sockets()
    }

    /// The iroh [`Endpoint`], exposed so clients can dial *out* to other iroh
    /// peers and to allow reusing the same bound socket/identity rather than
    /// needing a second endpoint/port per peer which is silly for things like
    /// daemons.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Return a single non-blocking pending connection that negotiated an ALPN
    /// other than the crate's ALPN, `None` if no connections are waiting. Only
    /// `Some` when [`Self::bind_with`] was given `extra_alpns`. Call this from
    /// a bevy `Update` system each tick. That is what this crate's own systems
    /// do to drain their receivers.
    pub fn recv_extra(&self) -> Option<(Vec<u8>, Connection)> {
        self.extra_rx.try_recv().ok()
    }

    /// Note: This and the client behavior is pulled from the iroh docs, so this
    /// behavior shouldn't be a surprise.
    ///
    /// Gracefully (hopefully...) close the connection
    /// and endpoint. Call via [`IrohTokioHandle`] before dropping and removing
    /// the `IrohClient` resource if a clean disconnect matters. Aka if you are
    /// switching this bevy App to/from a server role. NB: dropping it directly
    /// abandons the connection instead of closing it so you'll leak resources.
    ///
    /// For now we wait for up to 3 seconds as a best effort close for the
    /// transport and os level to close out. Otherwise the peer will eventually
    /// timeout once this is locally closed.
    ///
    /// Also waits in that 3 second window for the background extra-ALPN
    /// accept-loop task spawned in `bind_with` to exit.
    pub fn shutdown(self, handle: &IrohTokioHandle) {
        handle.0.block_on(async {
            self.endpoint.close().await;

            if tokio::time::timeout(Duration::from_secs(3), self.accept_task)
                .await
                .is_err()
            {
                tracing::warn!(
                    "iroh server accept task didn't exit within 3s of closing, the currently \
                     bound UDP port may not be immediately reusable"
                );
            }
        });
    }
}

/// Complete the replicon handshake for one QUIC `connection`. ALPN routing and
/// `incoming.await` already happened in the caller side, ref: `bind_with`.
/// Opens and tag our own outgoing persistent stream, then accept and tag peer,
/// and spawns a background reader task that feed `recv_rx` data.
async fn accept_peer(
    connection: Connection,
    handle: &tokio::runtime::Handle,
    next_network_id: &AtomicU64,
    disconnected_tx: Sender<NetworkId>,
) -> anyhow::Result<AcceptedPeer> {
    let mut send_stream = connection.open_uni().await?;
    send_stream.set_priority(REPLICON_STREAM_PRIORITY)?;
    frame::write_marker(&mut send_stream).await?;

    let mut recv_stream = connection.accept_uni().await?;
    frame::read_marker(&mut recv_stream).await?;

    let network_id = NetworkId::new(next_network_id.fetch_add(1, Ordering::Relaxed));

    let (tx, recv_rx) = unbounded::<(u8, Bytes)>();

    {
        let tx = tx.clone();
        handle.spawn(async move {
            while let Some((channel_id, bytes)) = frame::read_frame(&mut recv_stream).await {
                if tx.send((channel_id, bytes)).is_err() {
                    break;
                }
            }
        });
    }

    // Datagram reader for Unreliable channels, mostly just here for clients to
    // yeet stuff like heartbeats or other shenanigans on their own across the connection.
    {
        let tx = tx.clone();
        let connection = connection.clone();
        handle.spawn(async move {
            loop {
                match connection.read_datagram().await {
                    Ok(datagram) => {
                        if let Some((channel_id, payload)) = frame::decode_datagram(&datagram)
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

    // Disconnect the watcher in the dark...
    {
        let connection = connection.clone();
        handle.spawn(async move {
            let reason = connection.closed().await;
            tracing::debug!(?network_id, "iroh client disconnected: {reason}");
            let _ = disconnected_tx.send(network_id);
        });
    }

    Ok(AcceptedPeer {
        network_id,
        connection,
        send_stream,
        recv_rx,
    })
}

fn accept_connections(mut commands: Commands, server: Res<IrohServer>) {
    while let Ok(peer) = server.accepted_rx.try_recv() {
        let max_size = peer.connection.max_datagram_size().unwrap_or(1200);
        let client_entity = commands
            .spawn((
                ConnectedClient { max_size },
                peer.network_id,
                IrohConnection(peer.connection),
                IrohSendStream(peer.send_stream),
                IrohRecvRx(peer.recv_rx),
            ))
            .id();
        debug!(
            "spawning client `{client_entity}` with `{:?}`",
            peer.network_id
        );
    }
}

fn check_disconnects(
    mut commands: Commands,
    server: Res<IrohServer>,
    network_map: Res<NetworkIdMap>,
) {
    while let Ok(network_id) = server.disconnected_rx.try_recv() {
        if let Some(&client_entity) = network_map.get(&network_id) {
            // NB: The entity could have already despawned by the caller at this point.
            commands.entity(client_entity).despawn();
            debug!("despawning client `{client_entity}` with `{network_id:?}`");
        }
    }
}

fn receive_packets(mut messages: ResMut<ServerMessages>, clients: Query<(Entity, &IrohRecvRx)>) {
    for (client_entity, rx) in &clients {
        while let Ok((channel_id, bytes)) = rx.0.try_recv() {
            messages.insert_received(client_entity, channel_id as usize, bytes);
        }
    }
}

fn send_packets(
    handle: Res<IrohTokioHandle>,
    channels: Res<RepliconChannels>,
    mut messages: ResMut<ServerMessages>,
    mut clients: Query<(&IrohConnection, &mut IrohSendStream)>,
) {
    let server_channels = channels.server_channels();
    handle.0.block_on(async {
        for (client_entity, channel_id, bytes) in messages.drain_sent() {
            let Ok((connection, mut send_stream)) = clients.get_mut(client_entity) else {
                // Client disconnected this tick.
                continue;
            };

            if matches!(server_channels.get(channel_id), Some(Channel::Unreliable)) {
                let datagram = frame::encode_datagram(channel_id as u8, &bytes);
                if let Err(e) = connection.0.send_datagram(datagram) {
                    tracing::debug!("failed to send datagram to `{client_entity}`: {e}");
                }
            } else if let Err(e) =
                frame::write_frame(&mut send_stream.0, channel_id as u8, &bytes).await
            {
                tracing::debug!("failed to write frame to `{client_entity}`: {e:#}");
            }
        }
    });
}

fn disconnect_by_request(
    mut commands: Commands,
    mut disconnects: MessageReader<DisconnectRequest>,
) {
    for disconnect in disconnects.read() {
        debug!(
            "despawning client `{}` by disconnect request",
            disconnect.client
        );
        commands.entity(disconnect.client).despawn();
    }
}

fn disconnect_client(remove: On<Remove, ConnectedClient>, clients: Query<&IrohConnection>) {
    if let Ok(connection) = clients.get(remove.entity) {
        debug!("disconnecting despawned client `{}`", remove.entity);
        connection.0.close(0u32.into(), b"disconnected");
    }
}
