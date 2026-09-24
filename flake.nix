{
  description = "Scitadel — programmable, reproducible scientific literature retrieval";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Shared vigOS devkit toolchain, pinned to the devkit release this repo
    # adopts in .vig-os. Its overlay supplies `vig-utils` — the console scripts
    # the devkit-managed ci.yml calls (`validate-commit-range`,
    # `check-pr-agent-fingerprints`) — plus a tracked `uv`/`gh`. In direnv mode
    # CI provisions itself from THIS dev-shell, so the tools have to be here.
    # Bump deliberately alongside DEVKIT_VERSION.
    vigos.url = "github:vig-os/devkit/1.6.0";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, vigos }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default vigos.overlays.default ];
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
          # Single shared workspace version — read it so release bumps carry over.
          version = (builtins.fromTOML (builtins.readFile ./crates/scitadel-cli/Cargo.toml)).package.version;
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
            # devkit >= 0.4.0 runs the hooks through `prek`; the `pre-commit`
            # binary is gone from the image and the .githooks shims call `prek`.
            prek
            # typos runs as a language:system hook (the upstream pre-commit repo
            # ships a generic-linux binary that NixOS hosts cannot exec).
            typos
            # devkit CI toolchain (from the vigos overlay): ci.yml's
            # commit-checks job runs `uv run validate-commit-range` and
            # `uv run check-pr-agent-fingerprints`, and in direnv mode it
            # resolves both off this dev-shell's PATH.
            uv
            vig-utils
          ]
          # pymarkdown CLI packaged by devkit (nix/pymarkdown.nix, #1170) so the
          # markdown hook runs language:system from PATH — the upstream
          # pre-commit repo's pyjson5 native extension cannot load on bare CI
          # host runners (libstdc++.so.6). Imported from the pinned vigos input;
          # not exported as a flake package at 1.6.0.
          ++ [
            (import "${vigos}/nix/pymarkdown.nix" pkgs)
          ];

          shellHook = ''
            echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            echo "  scitadel devshell"
            echo "  rust   $(rustc --version 2>/dev/null | cut -d' ' -f2)"
            echo "  vhs    $(vhs --version 2>/dev/null | head -1 || echo 'not ready')"
            echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
            # Pin the WRAPPED C toolchain by absolute path (vig-os/devkit#1351):
            # CI's setup-devkit-toolchain re-exports the dev-shell PATH via
            # GITHUB_PATH, whose per-line prepend reverses the order, letting the
            # raw (unwrapped) gcc shadow the cc-wrapper — libsqlite3-sys's
            # vendored sqlite build then fails to find libc. Absolute paths are
            # PATH-order-proof; the action forwards shellHook env to CI (#1180).
            export CC=${pkgs.stdenv.cc}/bin/cc
            export CXX=${pkgs.stdenv.cc}/bin/c++
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
