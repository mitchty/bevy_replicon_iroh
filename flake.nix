{
  description = "bevy_replicon_iroh: minimal iroh network backend for bevy_replicon";
  outputs =
    {
      self,
      flakelight,
      flakelight-rust,
      nixpkgs,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      linuxSystems = builtins.filter (s: lib.hasSuffix "-linux" s) systems;

      rustFlake = flakelight ./. (
        { lib, ... }:
        {
          imports = [
            flakelight-rust.flakelightModules.default
          ];

          inputs.self = self;

          inherit systems;

          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./deny.toml
            ./src
            ./examples
          ];

          binaries.echo_server = {
            cargoExtraArgs = "--example echo_server";
          };
          binaries.echo_client = {
            cargoExtraArgs = "--example echo_client";
          };
          binaries.relay_server = {
            cargoExtraArgs = "--example relay_server";
          };

          treefmtConfig = {
            projectRootFile = "flake.nix";
            programs.nixfmt.enable = true;
            programs.rustfmt.enable = true;
            programs.taplo.enable = true;
          };

          apps.ci =
            { pkgs, ... }:
            {
              type = "app";
              program = "${
                pkgs.writeShellApplication {
                  name = "ci";
                  runtimeInputs = [ pkgs.nix ];
                  text = ''
                    nix flake check -L
                  '';
                }
              }/bin/ci";
            };
        }
      );

      # Stealing crap from my yeet nixos test setup. This isn't ideal but
      # whatever it works and is good enough for gov work as it were.
      integrationChecksFor =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          echo_server = rustFlake.packages.${system}.echo_server;
          echo_client = rustFlake.packages.${system}.echo_client;
          relay_server = rustFlake.packages.${system}.relay_server;
        in
        {
          bevy-replicon-iroh-int-echo = pkgs.callPackage ./nix/integration-00-echo.nix {
            inherit echo_server echo_client;
          };
          bevy-replicon-iroh-int-relay = pkgs.callPackage ./nix/integration-01-relay.nix {
            inherit echo_server echo_client relay_server;
          };
        };
    in
    lib.recursiveUpdate rustFlake { checks = lib.genAttrs linuxSystems integrationChecksFor; };

  inputs = {
    flakelight.url = "github:nix-community/flakelight";
    flakelight-rust.url = "github:mitchty/flakelight-rust";
    nixpkgs.follows = "flakelight-rust/nixpkgs";
  };
}
