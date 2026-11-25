{
  description = "gcan — analyze, filter, and prune Nix GC roots";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    naersk.url = "github:nix-community/naersk";
    naersk.flake = false;
  };

  outputs =
    inputs:
    let
      inherit (inputs.nixpkgs) lib;
    in
    {
      overlays = {
        gcan = import ./overlay.nix;
        naersk = import (inputs.naersk + "/overlay.nix");
        default = lib.composeManyExtensions [
          inputs.self.overlays.naersk
          inputs.self.overlays.gcan
        ];
      };

      packages = builtins.mapAttrs (
        system: pkgs':
        let
          pkgs = pkgs'.extend inputs.self.overlays.default;
        in
        {
          inherit (pkgs) gcan;
          default = inputs.self.packages.${system}.gcan;
        }
      ) inputs.nixpkgs.legacyPackages;

      devShells = builtins.mapAttrs (system: pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ inputs.self.packages.${system}.default ];

          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.nix
            inputs.self.formatter.${system}
          ];
        };
      }) inputs.nixpkgs.legacyPackages;

      formatter = builtins.mapAttrs (
        _system: pkgs:
        pkgs.treefmt.withConfig {
          settings = {
            tree-root-file = "flake.nix";
            on-unmatched = "info";
            formatter = {
              nixfmt = {
                command = lib.getExe pkgs.nixfmt;
                includes = [ "*.nix" ];
              };
              statix = {
                command = lib.getExe pkgs.statix;
                options = [ "fix" ];
                no-positional-arg-support = true;
                includes = [ "*.nix" ];
              };
              deadnix = {
                command = lib.getExe pkgs.deadnix;
                options = [ "--edit" ];
                includes = [ "*.nix" ];
              };
              rustfmt = {
                command = lib.getExe pkgs.rustfmt;
                options = [
                  "--config"
                  "skip_children=true"
                  "--edition"
                  "2024"
                ];
                includes = [ "*.rs" ];
              };
              taplo = {
                command = lib.getExe pkgs.taplo;
                options = [ "format" ];
                includes = [ "*.toml" ];
              };
              prettier = {
                command = lib.getExe pkgs.prettier;
                options = [ "--write" ];
                includes = [ "*.md" ];
              };
              shellcheck = {
                command = lib.getExe pkgs.shellcheck;
                includes = [ "*.sh" ];
              };
              shfmt = {
                command = lib.getExe pkgs.shfmt;
                options = [
                  "-w"
                  "-i"
                  "2"
                  "-s"
                ];
                includes = [
                  "*.sh"
                  "*.envrc"
                ];
              };
            };
          };
        }
      ) inputs.nixpkgs.legacyPackages;

      checks = builtins.mapAttrs (system: _pkgs: {
        formatting = inputs.self.formatter.${system}.check inputs.self;

        inherit (inputs.self.packages.${system}.default.passthru.tests) clippy tests;
      }) inputs.nixpkgs.legacyPackages;
    };
}
