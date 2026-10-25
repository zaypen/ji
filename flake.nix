{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    fenix.url = "github:nix-community/fenix";
  };

  outputs = { self, flake-utils, naersk, nixpkgs, fenix, }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = (import nixpkgs) {
          inherit system;
          overlays = [ fenix.overlays.default ];
        };

        naersk' = pkgs.callPackage naersk { };

        # opus binds bundled libopus, which is built with cmake.
        rustDeps = with pkgs; [
          opus
          pkg-config
          cmake
        ];

        package = naersk'.buildPackage {
          src = ./.;
          nativeBuildInputs = rustDeps;
          meta.mainProgram = "ji";
        };
      in rec {
        packages.default = package;

        devShell = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            alejandra
            rust-analyzer
            cmake
            opus
            pkg-config
            (pkgs.fenix.stable.withComponents [
              "cargo"
              "clippy"
              "rust-src"
              "rustc"
              "rustfmt"
            ])
          ];
        };
      });
}
