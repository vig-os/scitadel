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
