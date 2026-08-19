{
  pkgs,
  echo_server,
  echo_client,
  ...
}:
pkgs.testers.runNixOSTest {
  name = "bevy-replicon-iroh-integration-echo";
  globalTimeout = 5 * 60;

  nodes = {
    server = _: {
      virtualisation.cores = 2;
      environment.systemPackages = [ echo_server ];
      networking.firewall.allowedUDPPorts = [ 4433 ];
    };

    client = _: {
      virtualisation.cores = 2;
      environment.systemPackages = [ echo_client ];
    };
  };

  # TODO: Future mitch make a test-lib or lib.nix file when these tests get inappropriately long.
  testScript = ''
    start_all()

    server.wait_for_unit("multi-user.target")
    client.wait_for_unit("multi-user.target")
    server.wait_for_unit("network.target")
    client.wait_for_unit("network.target")

    server.succeed("mkdir -p /tmp/echo")
    server.succeed(
        "RUST_LOG=info echo_server --bind 0.0.0.0:4433 --id-out /tmp/echo/id "
        "< /dev/null > /tmp/echo/server.log 2>&1 & echo $! > /tmp/echo/server.pid"
    )

    server.wait_until_succeeds("test -s /tmp/echo/id")
    server_id = server.succeed("cat /tmp/echo/id").strip()

    # Like this, stop copy/pasting and make a lib.nix already.
    server_ip = server.succeed(
        "ip -4 addr show dev eth1 | grep -oP '(?<=inet\\s)\\d+(\\.\\d+){3}'"
    ).strip()
    print(f"server endpoint id: {server_id}, ip: {server_ip}")

    client.succeed("mkdir -p /tmp/echo")
    client.succeed(
        f"RUST_LOG=info echo_client --server-id {server_id} --server-addr {server_ip}:4433 "
        "< /dev/null > /tmp/echo/client.log 2>&1 & echo $! > /tmp/echo/client.pid"
    )

    # Probably too long but whatever I have no idea what kinda hardware this
    # will run on. If its a potato maybe not long enough but at 90 seconds
    # honestly, its time to retire the carrier pidgeon.
    CONNECT_TIMEOUT = 90

    client.wait_until_succeeds(
        "grep -E 'echo_client: connected' /tmp/echo/client.log",
        timeout=CONNECT_TIMEOUT,
    )

    # Prove that Bevy Component replication actually happened as a side effect.
    client.wait_until_succeeds(
        "grep -E 'state: role=client counter=[1-9]' /tmp/echo/client.log",
        timeout=CONNECT_TIMEOUT,
    )

    # Don't ask, it helped debug and I'm leaving it in as proof of past sins.
    server.fail("grep 'panicked at' /tmp/echo/server.log")
    client.fail("grep 'panicked at' /tmp/echo/client.log")

    print(
        "Echo server/client integration test passed. The client observed the server's "
        "replicated counter incrementing so its basically production quality code."
    )
  '';
}
