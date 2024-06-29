{
  description = "Nostr Relay written in Rust";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    flake-utils.url = "github:numtide/flake-utils";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ self, ... }:
    inputs.flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = inputs.nixpkgs.legacyPackages.${system};
        craneLib = inputs.crane.mkLib pkgs;
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            (pkgs.lib.hasSuffix "\.proto" path) ||
            # Default filter from crane (allow .rs files)
            (craneLib.filterCargoSources path type)
          ;
        };
        crate = craneLib.buildPackage {
          name = "nostr-rs-relay";
          inherit src;
          nativeBuildInputs = [ pkgs.pkg-config pkgs.protobuf ];
        };
      in
      {
        checks = {
          inherit crate;
        };
        packages.default = crate;
        formatter = pkgs.nixpkgs-fmt;
        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
        };
      });
}
