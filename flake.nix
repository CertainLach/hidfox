{
  description = "Jrsonnet";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
    jrsonnet = {
      url = "github:CertainLach/jrsonnet";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
  };
  outputs = {
    nixpkgs,
    flake-utils,
    rust-overlay,
    jrsonnet,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [rust-overlay.overlays.default];
        };
        rust =
          (pkgs.rustChannelOf {
            date = "2023-05-07";
            channel = "nightly";
          })
          .default
          .override {
            extensions = ["rust-src" "miri" "rust-analyzer"];
            targets = ["x86_64-unknown-linux-musl"];
          };
      in rec {
        packages = rec {
        };
        devShell = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            alejandra
            rust
            cargo-edit
            gnumake
            jrsonnet.packages.${system}.jrsonnet
            imagemagick
            tokio-console
            nodePackages_latest.web-ext
            nodePackages_latest.typescript
            nodePackages_latest.typescript-language-server
            nodePackages_latest.mermaid-cli
            nodePackages_latest.yarn
            nodejs_latest
            asciidoctor-with-extensions
            udev
            pkg-config
          ];
        };
      }
    );
}
