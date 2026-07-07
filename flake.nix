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

        # Build the `scitadel` binary with the pinned toolchain (edition 2024
        # needs rustc >= 1.85, which the stable rust-overlay provides).
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };
      in
      {
        packages.default = rustPlatform.buildRustPackage {
          pname = "scitadel";
          version = "0.7.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.pkg-config ];
          # Modern nixpkgs unifies the Apple SDK into the stdenv, so no explicit
          # Security/SystemConfiguration frameworks are needed on Darwin.
          buildInputs = [
            pkgs.openssl
            pkgs.sqlite
          ];

          # Use the nix-provided openssl/sqlite, not vendored copies.
          env.OPENSSL_NO_VENDOR = "1";

          # Only the `scitadel` binary (scitadel-cli) is wanted; it pulls in the
          # core/db/adapters/mcp crates transitively.
          cargoBuildFlags = [ "-p scitadel-cli" ];
          # Tests hit the network / a live SQLite DB — skip in the sandbox.
          doCheck = false;

          meta = {
            description = "Scitadel — programmable, reproducible scientific literature retrieval (CLI + TUI + MCP)";
            mainProgram = "scitadel";
          };
        };

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
