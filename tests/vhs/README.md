# VHS walkthrough tapes

Scripted terminal recordings that demonstrate scitadel features end-to-end.
Each tape is a [charmbracelet/vhs](https://github.com/charmbracelet/vhs) file
that plays the TUI through a realistic user flow. Tapes double as:

1. **Tests** — the feature still works if the tape runs to completion.
2. **Documentation** — the rendered GIF shows the workflow without words.
3. **Milestone gates** — each 0.X.0 has a `0.X.0-walkthrough.tape` that
   exercises the whole milestone's feature set in one sitting.

## Running tapes locally

Requires `vhs`, available from the devshell (`nix develop` or `direnv allow`).

```sh
just vhs           # run all tapes, emit PNGs/GIFs under tests/vhs/snapshots/
just vhs-one NAME  # run just tests/vhs/NAME.tape
just vhs-update    # re-run all tapes and overwrite committed snapshots
```

Each tape writes artifacts next to itself under `snapshots/<tape-name>/`.

## Authoring conventions

- **Isolate state.** Every tape sets `SCITADEL_DB` to a temp path (see
  `example.tape`) so user data is never touched.
- **Fixture DBs** live under `fixtures/` (SQLite files ≤ 1 MB). Seed them
  with small, deterministic datasets.
- **Screenshot at inflection points.** Use `Screenshot path/name.png`
  directives after each meaningful keystroke so regressions show up as
  pixel/byte diffs in PR review.
- **Keep tapes under 30s** so the full suite stays fast in CI.

## CI

Today `rust-ci.yml` does **not** run tapes (needs a Linux runner with `vhs`
+ `ttyd` + `ffmpeg` installed). That wiring is tracked as a follow-up once
golden-file comparison is in place. For now, run tapes locally as part of
your PR review and attach a fresh GIF to the description.
