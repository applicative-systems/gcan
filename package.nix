{
  lib,
  naersk,
}:
let
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
    ];
  };
in
naersk.buildPackage {
  inherit src;

  passthru.tests = {
    clippy = naersk.buildPackage {
      inherit src;
      mode = "clippy";
      cargoBuildOptions = x: x ++ [ "--all-targets" ];
    };

    tests = naersk.buildPackage {
      inherit src;
      mode = "test";
    };
  };
}
