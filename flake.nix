{
  description = "gcan — analyze, filter, and prune Nix GC roots";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    naersk.url = "github:nix-community/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";
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
        inherit (pkgs) lib;
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
            inputs.self.formatter.${system}
          ];
        };

        formatter = pkgs.treefmt.withConfig {
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
                includes = [
                  "*.css"
                  "*.html"
                  "*.js"
                  "*.json"
                  "*.md"
                  "*.yaml"
                  "*.yml"
                ];
                # Askama templates use Jinja-like `{% ... %}` tags that
                # prettier doesn't understand — it reflows them across
                # lines and breaks rendering. Exclude the template folder.
                excludes = [ "argunix-web/templates/*" ];
              };
              shellcheck = {
                command = lib.getExe pkgs.shellcheck;
                includes = [
                  "*.sh"
                  "*.bash"
                ];
                excludes = [
                  ".envrc"
                ];
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
                  "*.bash"
                  "*.envrc"
                  "*.envrc.*"
                ];
              };
            };
          };
        };

        checks = {
          formatting = inputs.self.formatter.${system}.check inputs.self;

          inherit (pkgs.gcan.passthru.tests) clippy tests;
        };
      }
    );
}
