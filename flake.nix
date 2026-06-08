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
    eachSystem systems (
      system:
      let
        pkgs = import inputs.nixpkgs { inherit system; };
        naersk = pkgs.callPackage inputs.naersk { };
        treefmtEval = inputs.treefmt-nix.lib.evalModule pkgs ./treefmt.nix;
      in
      {
        packages.default = naersk.buildPackage {
          src = ./.;
          # gcan shells out to `nix-store` at runtime; it is meaningless on a
          # host without nix, so we intentionally do not bundle nix.
        };

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

        checks.formatting = treefmtEval.config.build.check inputs.self;
      }
    );
}
