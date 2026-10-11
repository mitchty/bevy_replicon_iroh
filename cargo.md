# bevy_replicon_iroh

A minimal [iroh](https://docs.rs/iroh) messaging backend for
[bevy_replicon](https://docs.rs/bevy_replicon).

## Compatibility

| Bevy | bevy_replicon | bevy_replicon_iroh |
|------|---------------|--------------------|
| 0.20 | 0.45          | 0.20.x             |
| 0.19 | 0.41          | 0.1.x              |

## Usage

Add an `IrohTokioHandle` before you add crate plugins (its asserted at `build()`
time), then bind a server or connect a client:

```rust
// Both sides
app.insert_resource(IrohTokioHandle(tokio_handle))
    .add_plugins(RepliconPlugins)
    .add_plugins(RepliconIrohPlugins);

// Server
let server = IrohServer::bind(&handle, secret_key, bind_addr)?;
app.insert_resource(server);

// Client
let client = IrohClient::connect(&handle, secret_key, bind_addr, peer_addr)?;
app.insert_resource(client);
```

Connection timeout defaults to 15 seconds and a corresponding disconnect/shutdown timeout of 3 seconds. Override either by setting the Plugin Config like so:

```rust
app.add_plugins(RepliconIrohServerPlugin {
    config: IrohServerConfig { shutdown_timeout: Duration::from_secs(10) },
})
.add_plugins(RepliconIrohClientPlugin {
    config: IrohClientConfig {
        connect_timeout: Duration::from_secs(30),
        ..Default::default()
    },
});
```

See `examples/echo_server.rs` and `examples/echo_client.rs` for a minimal
working example.
