# scitadel (Claude Code plugin)

Registers the **scitadel MCP server** in Claude Code with no manual PATH setup.

## Install

```text
/plugin marketplace add vig-os/scitadel
/plugin install scitadel@scitadel
```

On the first MCP launch the plugin fetches the pinned `scitadel` release binary
for your platform from [GitHub Releases](https://github.com/vig-os/scitadel/releases),
verifies its SHA-256, caches it under `bin/`, and runs `scitadel mcp`. Later
launches exec the cached binary directly.

Supported prebuilt platforms: macOS (arm64, x86_64) and Linux (x86_64). On any
other platform — or if the download fails — the launcher falls back to a
`scitadel` binary on your `PATH` (e.g. `cargo install scitadel-cli`).

## How it works

| File | Role |
| --- | --- |
| `.claude-plugin/plugin.json` | Plugin manifest (name, version). |
| `.mcp.json` | Declares the `scitadel` stdio server, command `${CLAUDE_PLUGIN_ROOT}/bin/scitadel-mcp`. |
| `bin/scitadel-mcp` | Launcher: resolves/fetches the binary, then `exec`s `scitadel mcp`. |
| `bin/VERSION` | Pinned release version the launcher fetches. Bump in lockstep with releases. |

### Overrides

- `SCITADEL_BIN=/path/to/scitadel` — skip fetching and use a specific binary
  (handy in dev: point at `target/debug/scitadel`).

## Maintainer notes

- `bin/VERSION` and `.claude-plugin/plugin.json`'s `version` must match the
  released tag whose assets the launcher downloads.
- The launcher depends on `binaries.yml` having attached per-platform tarballs
  (`scitadel-<version>-<target>.tar.gz` + `.sha256`) to the GitHub Release for
  that tag. If a release has no assets, first launch falls back to `PATH`.
- Test locally without pushing:
  `/plugin marketplace add /Users/larsgerchow/Projects/scitadel`
