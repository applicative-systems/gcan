{
  description = "gcan — analyze, filter, and prune Nix GC roots";

  inputs = {
    naersk.url = "github:nix-community/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";

    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = inputs:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAll = inputs.nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAll (system:
        let
          pkgs = import inputs.nixpkgs { inherit system; };
          naersk = pkgs.callPackage inputs.naersk { };
        in
        {
          default = naersk.buildPackage {
            src = ./.;
            # gcan shells out to `nix-store` at runtime; it is meaningless on a host
            # without nix, so we intentionally do not bundle nix into the closure.
          };
        });

      devShells = forAll (system:
        let pkgs = import inputs.nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo
              pkgs.rustc
              pkgs.clippy
              pkgs.rustfmt
              pkgs.rust-analyzer
              pkgs.nix
            ];
          };
        });
    };
}
