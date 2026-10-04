{
  pkgs,
  echo_server,
  echo_client,
  relay_server,
  ...
}:
pkgs.testers.runNixOSTest {
  name = "bevy-replicon-iroh-integration-relay";
  globalTimeout = 5 * 60;

  nodes = {
    relay = _: {
      virtualisation.cores = 2;
      environment.systemPackages = [ relay_server ];
      networking.firewall.allowedTCPPorts = [ 3340 ];
    };

    server = _: {
      virtualisation.cores = 2;
      environment.systemPackages = [ echo_server ];
      networking.firewall.enable = true;
      # Block ALL inbound UDP on eth1 so that we can prove the relay is
      # working.
      networking.firewall.extraCommands = ''
        iptables -I INPUT 1 -i eth1 -p udp -j DROP
        ip6tables -I INPUT 1 -i eth1 -p udp -j DROP
      '';
    };

    client = _: {
      virtualisation.cores = 2;
      environment.systemPackages = [ echo_client ];
    };
  };

  # Same TODO as integration-00-echo.nix - this duplicates its IP-discovery
  # boilerplate for now, future mitch can finally write that lib.nix.
  testScript = ''
    start_all()

    relay.wait_for_unit("multi-user.target")
    server.wait_for_unit("multi-user.target")
    client.wait_for_unit("multi-user.target")
    relay.wait_for_unit("network.target")
    server.wait_for_unit("network.target")
    client.wait_for_unit("network.target")

    relay_ip = relay.succeed(
        "ip -4 addr show dev eth1 | grep -oP '(?<=inet\\s)\\d+(\\.\\d+){3}'"
    ).strip()
    server_ip = server.succeed(
        "ip -4 addr show dev eth1 | grep -oP '(?<=inet\\s)\\d+(\\.\\d+){3}'"
    ).strip()
    print(f"relay ip: {relay_ip}, server ip: {server_ip}")

    relay.succeed("mkdir -p /tmp/relay")
    relay.succeed(
        "RUST_LOG=info relay_server --bind 0.0.0.0:3340 "
        "< /dev/null > /tmp/relay/relay.log 2>&1 & echo $! > /tmp/relay/relay.pid"
    )
    relay.wait_until_succeeds("grep -E 'relay_server: listening' /tmp/relay/relay.log")

    relay_url = f"http://{relay_ip}:3340"

    server.succeed("mkdir -p /tmp/echo")
    server.succeed(
        f"RUST_LOG=info echo_server --bind 0.0.0.0:4433 --id-out /tmp/echo/id "
        f"--relay-url {relay_url} "
        "< /dev/null > /tmp/echo/server.log 2>&1 & echo $! > /tmp/echo/server.pid"
    )

    server.wait_until_succeeds("test -s /tmp/echo/id")
    server_id = server.succeed("cat /tmp/echo/id").strip()
    print(f"server endpoint id: {server_id}")

    client.succeed("mkdir -p /tmp/echo")
    client.succeed(
        f"RUST_LOG=info echo_client --server-id {server_id} --server-addr {server_ip}:4433 "
        f"--relay-url {relay_url} "
        "< /dev/null > /tmp/echo/client.log 2>&1 & echo $! > /tmp/echo/client.pid"
    )

    # Relay round-trips add latency over the direct-dial test, give it more time
    # to be safe.
    CONNECT_TIMEOUT = 120

    client.wait_until_succeeds(
        "grep -E 'echo_client: connected' /tmp/echo/client.log",
        timeout=CONNECT_TIMEOUT,
    )

    # Prove the selected path is actually relay for this test to succeed, since
    # the server's firewall never opened direct UDP at all.
    client.wait_until_succeeds(
        "grep -E 'state: role=client counter=[1-9].*path=relay' /tmp/echo/client.log",
        timeout=CONNECT_TIMEOUT,
    )

    relay.fail("grep 'panicked at' /tmp/relay/relay.log")
    server.fail("grep 'panicked at' /tmp/echo/server.log")
    client.fail("grep 'panicked at' /tmp/echo/client.log")

    print(
        "Relay integration test passed and validated not to go directly"
        "to the nodes but via the example relay daemon."
    )
  '';
}
