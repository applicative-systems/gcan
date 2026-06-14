{
  description = "gcan — analyze, filter, and prune Nix GC roots";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    naersk.url = "github:nix-community/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";

    treefmt-nix.url = "github:numtide/treefmt-nix";
    treefmt-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    inputs:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];

      eachSystem =
        systems: f:
        builtins.foldl' (
          a: s: a // builtins.mapAttrs (k: v: (a.${k} or { }) // { ${s} = v; }) (f s)
        ) { } systems;
    in
    {
      overlays.default = import ./overlay.nix;
    }
    // eachSystem systems (
      system:
      let
        pkgs = import inputs.nixpkgs {
          inherit system;
          overlays = [
            inputs.naersk.overlays.default
            inputs.self.overlays.default
          ];
        };
        treefmtEval = inputs.treefmt-nix.lib.evalModule pkgs {
          projectRootFile = "flake.lock";

          programs = {
            deadnix.enable = true;
            nixfmt.enable = true;
            rustfmt.enable = true;
            shfmt.enable = true;
            statix.enable = true;
            prettier.enable = true;
          };
        };
      in
      {
        packages.default = pkgs.gcan;

        devShells.default = pkgs.mkShell {
          inputsFrom = [ inputs.self.packages.${system}.default ];

          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.nix
            treefmtEval.config.build.wrapper
          ];
        };

        formatter = treefmtEval.config.build.wrapper;

        checks = {
          formatting = treefmtEval.config.build.check inputs.self;

          inherit (pkgs.gcan.passthru.tests) clippy tests;
        };
      }
    );
}
