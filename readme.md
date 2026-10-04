# bevy-replicon-iroh

A minimal [iroh](https://docs.rs/iroh) network backend for
[bevy-replicon](https://docs.rs/bevy_replicon).

Note: I *only* implemented what I need here. Which is basically almost nothing
special as far as replicon is concerned. I only needed simple sync between
server to clients so this is basically the bare minimum an Iroh backend would
need to run in replicon.

The other thing I added is the ability to get to the underlying Iroh transport
for back channel communication. I am using this for daemon \<-\> daemon
communication and \"clients\" in my use case is simply local TUI/GUI clients
connecting to localhost ultimately.

I\'m willing to add more here as I might need it but just be aware the
constraints and use cases I started from. They likely differ from what a
\"real\" video game programmer would need like prediction et al which this
literally does not care about. Yet.

**IMPORTANT** Unlike renet, all the Iroh stuff is async here via tokio for now.

Also note, this crate follows [Pride Versioning](https://pridever.org) not
[Semver](https://semver.org).

Expect only embarassing shame updates for a while until it graduates to truly
Proud 1.x release. It barely/mostly works so good enough for production!

## Scope

- **Direct P2P by default, relay optional.** `IrohServer::bind` and
  `IrohClient::connect` use iroh\'s `presets::Minimal` which is direct
  p2p, no relay or address-lookup/discovery.
  `IrohServer::bind_with~/~IrohClient::connect_with` allow any
  `iroh::endpoint::Preset` so you can tune this yourself. Peers are
  always dialed via a known `iroh::EndpointId` e.g. public key plus an
  explicit direct `SocketAddr`, exchanged out of band regardless of
  preset. Again reference the original purpose here, I am abusing this
  for both local TUI/cli clients and daemon to daemon communication.
- **A single shared priority tier** for all of replicon\'s traffic, ref
  `REPLICON_STREAM_PRIORITY`. Applications wanting additional,
  independently-prioritized streams should open them directly on the
  exposed `iroh::endpoint::Connection` themselves via
  `server::IrohConnection` `client::IrohClient::connection`.
- **A \"get out of jail free\" ALPN/connection is supported on the same
  bound port/identity** via `IrohServer::bind_with`
  `IrohClient::connect_with`\'s `extra_alpns` + `EndpointHooks`, so that
  an application can layer its own side-channel traffic directly on top
  of the same Iroh endpoint rather than another network
  socket/connection.
- No Iroh connection stats reporting yet. I\'m a hack.
  `ClientStats~/~ConnectedClientStats` stay at defaults. \"I\'ll fix it
  in post\" (I might, I\'m lazy and am not entirely sure if I need to
  add this pr\'s welcome)
- No bs client/server gating, its \"just one crate\".

## Usage

Insert ye olde Bevy Resources n such, nothing special here tbh its mostly drole
boilerplate.

``` rust
app.insert_resource(IrohTokioHandle(tokio_handle))
    .add_plugins(RepliconPlugins)
    .add_plugins(RepliconIrohPlugins);

// Ye server does:
let server = IrohServer::bind(&handle, secret_key, bind_addr)?;
app.insert_resource(server);

// Ye client does:
let client = IrohClient::connect(&handle, secret_key, bind_addr, peer_addr)?;
app.insert_resource(client);
```

Both `RepliconIrohServerPlugin` and `RepliconIrohClientPlugin` are always added
by `RepliconIrohPlugins` as of readme writing cause I can\'t come up with a good
reason to separate them.

Timeouts aka connect, disconnect, shutdown default to 15s and 3s but are
configurable via `IrohClientConfig/IrohServerConfig`, provided at plugin build
time if you skip adding `RepliconIrohPlugins` and add the plugins yourself like
so:

``` rust
app.add_plugins(RepliconIrohServerPlugin {
    config: IrohServerConfig { shutdown_timeout: Duration::from_secs(10) },
})
.add_plugins(RepliconIrohClientPlugin {
    config: IrohClientConfig { connect_timeout: Duration::from_secs(30), ..Default::default() },
});
```

Since `IrohServer::bind/bind_with` and `IrohClient::connect/connect_with` are
plain functions, not systems, the plugin itself can\'t reach into an
already-bound server/already-open client to pull the config resource back out.
Alternatively build your own `IrohServerConfig/IrohClientConfig` preset
directly, and hand it to `bind_with/connect_with` yourself.

The Bevy systems are only gated upon `IrohServer` or `IrohClient` presence as
resources. This means an application can swap which resource is present at
runtime like the weirdo I am. Aka remove one Resource, add the other and kablamo
you\'re now a client and not a server. Why you wnt to do this I have no idea. I
barely understand replicon man. Suggestions by non hacks who do welcome. It made
sense at the time as I was thinking of a single bevy App moving betwixt Server
and Client. I\'ve abandonded that for my own use case but left this distinction
in. I\'m easily convinced past me is an idiot though.

Want an evil self-hosted relay that uses http and not https?
`bevy_replicon_iroh::minimal_with_relay` builds a `Preset` just for that
purpose. You're crazy to use it but can, its simply `presets::Minimal` plus
whatever `iroh::RelayMode` you provide:

``` rust
let relay_map = bevy_replicon_iroh::iroh::RelayMap::try_from_iter(["http://my-relay:3340"])?;
let server = IrohServer::bind_with(
    &handle,
    bevy_replicon_iroh::minimal_with_relay(bevy_replicon_iroh::iroh::RelayMode::Custom(relay_map)),
    secret_key,
    bind_addr,
    Vec::new(),
    hooks,
    IrohServerConfig::default(),
)?;
```

Both `examples/echo_server.rs` and `examples/echo_client.rs` take a
`--relay-url` flag demonstrating exactly this, and `examples/relay_server.rs` is
a tiny standalone relay plain HTTP, no TLS encryption good enough to point them
at for local testing - see `nix/integration-01-relay.nix` for a full three-VM
example with a firewall forcing the relay path.

Ref `examples/echo_server.rs` and `examples/echo_client.rs` for a stupid/minimal
example of how you can use this to echo stuff between a server and client. High
technology crap I tells ya.

Its *just* a replicated counter ticking on the server that is directly observed
by client(s).

For a more \"complect\" (its an old english word and I\'m bringing it back)
nixos/vm based example ref: `nix/integration-00-echo.nix` for those same two
examples but \"as two nixos tests\" instead.

```sh
# A
cargo run --example echo_server -- --bind 127.0.0.1:4433

# B, prolly another terminal
cargo run --example echo_client -- --server-id $IROH_ID --server-addr 127.0.0.1:4433
```

## TODO

An imperial *and* metric butt ton(ne?) either way a lot to do. Off the
top of my head:

- I have a lot of things I abuse on top of this that make using Iroh in
  this way easier. Not sure where else to stick em so maybe here is a
  good spot. Mostly relating to swapping betwixt paths and ensuring a
  direct connection and retry and other shenanigans.
- Adding more \"game\" related code, this literally is \"what is the
  minimum a replicon client/server need be\".
- I need to figure out client/server restart behavior right now its
  pretty derp.
- Non async approach for Iroh?
- At least non Tokio approach. I avoided the Bevy i/o stuff cause I
  abuse os level threads with async tokio and channel communication in
  my own apps.
- I dunno insert \"mitch forgot to add xyz\" here. I\'m sure it\'ll fit.

## Status

I mean it \"works\", I am abusing this for \"real\" code that isn\'t
open source. But I\'m sure bugs abound. Treat it like it is, at best
MVP. I have beaten the piss out of it in internal tests so the overall
approach works fine for daemon use cases that need to talk directly to
each other. Beyond that, only the shadow knows...

## Development Shenanigans

I abuse both [flakelight](https://github.com/nix-community/flakelight) and my
own crap on top of it for the rust bits ref
[flakelight-rust](https://github.com/mitchty/flakelight-rust) for all me own
testing and whatnot.

``` {.bash org-language="sh"}
nix develop
nix flake check
```

Its a cargo crate though so you don\'t *need* nix but... it helps being
reproducible.

## Avian Intelligence Disclaimer

I made Claude build most of the original tests and nixos tests and some of the
readme nonsense. Beyond that I fixed its derp and hopefully improved upon its
bird brained output. But if that offends you, well now you have me telling you I
abused the Frenchman for some bits of this. Not all though so I dunno if that
matters don't use this I guess.
