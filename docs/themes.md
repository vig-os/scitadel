# TUI Themes

Scitadel ships colourblind-friendly Dalton palettes for both dark and
light terminals (#136, #137). The active palette is picked once at
startup and held for the rest of the session — there is no
mid-session re-detection.

## Resolution order

Highest precedence wins. The first source that is set decides the theme;
unset sources fall through to the next.

1. **`--theme <name>` CLI flag** on `scitadel tui`. Highest precedence;
   used for one-off overrides without touching env or config.
2. **`SCITADEL_THEME` environment variable**. Useful for tmux/zellij
   `set-environment` users and for forcing a palette in containers
   without rewriting config.
3. **`[ui] theme = "..."` in `config.toml`**. Persistent per-installation
   preference. Written by `scitadel init` if you answer the theme
   prompt.
4. **`auto`** (default). Probes the terminal background; see
   [auto-detect](#auto-detect) below.

Accepted names (run `scitadel tui --list-themes` for the live list):

| name            | meaning                                                           |
| --------------- | ----------------------------------------------------------------- |
| `auto`          | detect terminal background, fall back to dark                     |
| `dark`          | alias for `dalton-dark`                                           |
| `light`         | alias for `dalton-bright`                                          |
| `dalton-dark`   | Dalton colourblind-friendly dark palette (default)               |
| `dalton-bright` | Dalton colourblind-friendly light palette (warm cream background) |
| `dalton-light`  | alias for `dalton-bright`                                          |

Unknown / typo'd values fold to `auto` rather than panicking the
session — a misspelled `--theme darj` is treated as "let scitadel pick".

## Auto-detect

`auto` resolution reads `COLORFGBG` (set by most mature terminals;
format `"<fg>;<bg>"` where each is an ANSI 0–15 colour index):

- `bg` index 0–6 → dark background → `dalton-dark`.
- `bg` index 7–15 → light background → `dalton-bright`.
- Unset or unparseable → **dark fallback**. Dark is the safer default
  since most dev terminals are dark, and the previous (single-theme)
  behaviour was Dalton Dark.

OSC 11 query fallback (issue [#176](https://github.com/vig-os/scitadel/issues/176))
is **not** implemented yet. If your terminal does not set `COLORFGBG`
(notably some iTerm2 / Ghostty configs) and you want light mode, set
`SCITADEL_THEME=light` or pass `--theme light`.

## Restart-required for change

Theme is resolved exactly once when the TUI launches and is held in a
process-wide `OnceLock` for the lifetime of the session. If your
terminal flips light/dark mid-session — for example macOS auto-dark-mode
flipping at sunset — scitadel will keep using the palette it picked at
launch. Quit (`q`) and relaunch to pick up the new value.

A runtime hotkey to toggle the theme without restart is tracked in
[#175](https://github.com/vig-os/scitadel/issues/175); the resolver-side
work in this iter is the prerequisite for it.

## Startup toast

On launch the status bar briefly shows the resolved theme — e.g.
`theme: dalton-bright (auto)` — for the first few seconds, then fades
back to the normal help text. Use this to verify the resolver picked
what you expected without re-running `--list-themes`.

## Override escape hatch

If `COLORFGBG` lies (some iTerm2 versions don't update it after a
profile change) or you want to force a palette regardless:

```sh
# one session
scitadel tui --theme light

# this shell, forever
export SCITADEL_THEME=light

# per-installation
# add to <db_dir>/config.toml:
[ui]
theme = "dalton-bright"
```

## Pitfalls

- **`COLORFGBG` is a de-facto standard, not RFC'd.** Some terminals
  set it wrong; iTerm2 doesn't always update it after a theme change.
  Trust it but allow override.
- **Mid-session terminal theme change** is intentionally not handled.
  Restart the TUI.
- **Light-mode highlight readability** — the 8 annotation-highlight
  slots use pale tints against dark text. If a highlight is nearly
  invisible on your terminal's background, file an issue with the
  exact bg colour and we'll tune the palette.

## Deferred work

- **OSC 11 fallback** — issue [#176](https://github.com/vig-os/scitadel/issues/176).
- **Runtime theme-toggle hotkey** — issue [#175](https://github.com/vig-os/scitadel/issues/175).
