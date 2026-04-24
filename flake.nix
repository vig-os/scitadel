{
  description = "Scitadel — programmable, reproducible scientific literature retrieval";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust
            rustToolchain
            pkg-config
            openssl
            sqlite

            # Cargo extras
            cargo-deny
            cargo-watch
            cargo-nextest
            cargo-edit

            # Build/runtime
            just

            # TUI / terminal debugging
            vhs
            charm-freeze
            asciinema

            # Git / CI
            git
            gh
            pre-commit
          ];

          shellHook = ''
            echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            echo "  scitadel devshell"
            echo "  rust   $(rustc --version 2>/dev/null | cut -d' ' -f2)"
            echo "  vhs    $(vhs --version 2>/dev/null | head -1 || echo 'not ready')"
            echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
