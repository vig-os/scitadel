//! Semantic color theming for the TUI (#136 / #137).
//!
//! Every TUI view pulls colors through named roles on [`Theme`] rather
//! than hardcoding palette (`Color::Yellow`) or RGB values. That lets
//! the theme be swapped wholesale at startup based on user preference.
//!
//! Dalton Dark / Dalton Bright are colorblind-friendly schemes tuned
//! for deuteranopia and protanopia. Source:
//! <https://github.com/gerchowl/dalton-colorscheme>.
//!
//! ## Runtime resolution (#137, #176)
//!
//! Theme is picked once at startup via [`init`] — call it before the
//! first frame is drawn. Order, highest precedence first:
//! 1. `--theme <name>` CLI flag (handled in caller)
//! 2. `SCITADEL_THEME` env var
//! 3. `[ui] theme = "..."` in `config.toml`
//! 4. Default: `auto` — probe terminal background:
//!    a. `COLORFGBG` env var (cheap, no I/O)
//!    b. OSC 11 query against the controlling tty (#176; 100–200ms timeout, skipped on non-tty)
//!    c. fall back to dark
//!
//! Mid-session theme change is intentionally not supported — if the
//! terminal flips light/dark at sunset, restart the TUI.

mod osc11;

use std::sync::OnceLock;

use ratatui::style::Color;

/// Semantic color roles used across all TUI views.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Borders, headers, status-bar mode labels, the active-tab marker.
    pub emphasis: Color,
    /// Secondary text — labels on form prompts, metadata under titles,
    /// empty-state messages, help-bar body text.
    pub muted: Color,
    /// Background of the currently-selected table row / list item.
    pub selection_bg: Color,
    /// Annotation quoted text (the literal quoted-passage spans in
    /// the paper detail and annotation prompt).
    pub quote: Color,
    /// Neutral informational colour — task-panel ref IDs, links.
    pub info: Color,
    /// Positive state — downloaded, task done, healthy.
    pub success: Color,
    /// Attention-worthy but not broken — paywall, in-flight download,
    /// [OFFLINE] badge.
    pub warning: Color,
    /// Error state — failed download, delete-confirm prompt cursor.
    pub danger: Color,
    /// 8 muted background tints used behind annotation highlights in the
    /// two-pane reader (#97). Palette is hashed by thread `root_id` so
    /// a thread keeps the same colour across renders.
    pub highlights: [Color; 8],
}

impl Theme {
    /// Colourblind-friendly dark theme. Foreground roles use Dalton's
    /// base palette; the 8 highlight backgrounds are muted chromatic
    /// variants tuned to sit behind default-foreground text without
    /// impairing readability.
    pub const DALTON_DARK: Theme = Theme {
        emphasis: Color::Rgb(0xc4, 0xc4, 0x0c),     // dalton yellow
        muted: Color::Rgb(0x56, 0x71, 0x7f), // dalton cyan (desaturated; works as secondary text on dark)
        selection_bg: Color::Rgb(0x33, 0x33, 0x33), // dalton selection
        quote: Color::Rgb(0x66, 0x91, 0xa7), // dalton bright-cyan
        info: Color::Rgb(0x7a, 0xa2, 0xf7),  // dalton blue
        success: Color::Rgb(0x5b, 0x91, 0x4e), // dalton green
        warning: Color::Rgb(0xc4, 0xc4, 0x0c), // dalton yellow (same as emphasis — attention-worthy)
        danger: Color::Rgb(0xd8, 0x50, 0x50),  // dalton red
        highlights: [
            Color::Rgb(0x56, 0x1f, 0x1f), // muted red
            Color::Rgb(0x24, 0x3a, 0x1f), // muted green
            Color::Rgb(0x4e, 0x4e, 0x05), // muted yellow
            Color::Rgb(0x30, 0x40, 0x60), // muted blue
            Color::Rgb(0x40, 0x20, 0x53), // muted magenta
            Color::Rgb(0x22, 0x2d, 0x33), // muted cyan
            Color::Rgb(0x3c, 0x48, 0x60), // muted bright-blue
            Color::Rgb(0x4d, 0x2d, 0x60), // muted bright-magenta
        ],
    };

    /// Light companion to Dalton Dark. Source CSS variables ported
    /// from `dalton-bright.css` upstream. Background is the warm cream
    /// (`#f4f1eb`); foreground roles use the darker chromatic
    /// variants so they read against light without losing the
    /// colorblind-distinguishability the palette is designed for.
    /// Highlight slots are pale tints of the same hues — light enough
    /// to sit behind dark text without inverting contrast.
    pub const DALTON_BRIGHT: Theme = Theme {
        emphasis: Color::Rgb(0x7a, 0x6d, 0x00), // dalton bright yellow (darker for light bg)
        muted: Color::Rgb(0x70, 0x70, 0x7a),    // bright black (secondary text)
        selection_bg: Color::Rgb(0xd0, 0xcd, 0xc5), // dalton bright selection
        quote: Color::Rgb(0x2a, 0x68, 0x80),    // bright cyan (dark variant)
        info: Color::Rgb(0x30, 0x60, 0xc8),     // dalton bright blue
        success: Color::Rgb(0x3a, 0x75, 0x30),  // dalton bright green
        warning: Color::Rgb(0x8a, 0x7d, 0x00),  // bright yellow (slightly stronger)
        danger: Color::Rgb(0xb8, 0x30, 0x30),   // dalton bright red
        highlights: [
            Color::Rgb(0xf4, 0xd8, 0xd8), // pale red
            Color::Rgb(0xd9, 0xed, 0xd5), // pale green
            Color::Rgb(0xf2, 0xee, 0xc4), // pale yellow
            Color::Rgb(0xd6, 0xe0, 0xf4), // pale blue
            Color::Rgb(0xe7, 0xd6, 0xee), // pale magenta
            Color::Rgb(0xd6, 0xe6, 0xec), // pale cyan
            Color::Rgb(0xdc, 0xe6, 0xf4), // pale bright-blue
            Color::Rgb(0xe2, 0xd2, 0xee), // pale bright-magenta
        ],
    };

    /// Registered themes, surfaced by `scitadel tui --list-themes`
    /// (#137). Tuple is `(canonical-name, one-line description)`.
    /// `auto` is included as a meta-entry so the listing matches what
    /// users can actually pass — even though it doesn't map to a fixed
    /// palette. New named palettes plug in here.
    #[must_use]
    pub fn registry() -> &'static [(&'static str, &'static str)] {
        &[
            (
                "auto",
                "detect terminal background (COLORFGBG → fall back to dark)",
            ),
            (
                "dark",
                "alias for dalton-dark (default colourblind-friendly dark palette)",
            ),
            ("light", "alias for dalton-bright"),
            ("dalton-dark", "Dalton colourblind-friendly dark palette"),
            (
                "dalton-bright",
                "Dalton colourblind-friendly light palette (warm cream bg)",
            ),
            ("dalton-light", "alias for dalton-bright"),
        ]
    }

    /// Pick a highlight colour for a string key (e.g. annotation
    /// root_id). djb2 hash modulo palette size so the mapping is
    /// stable across runs.
    #[must_use]
    pub fn highlight_for(&self, key: &str) -> Color {
        let mut h: u64 = 5381;
        for b in key.as_bytes() {
            h = h.wrapping_mul(33).wrapping_add(u64::from(*b));
        }
        self.highlights[(h as usize) % self.highlights.len()]
    }
}

/// Process-wide active theme. Set once at startup via [`init`]; reads
/// after that go through [`theme()`]. `OnceLock` makes it cheap (no
/// lock contention on the hot draw path) and lets tests use the
/// default without setup.
static ACTIVE: OnceLock<Theme> = OnceLock::new();

/// Convenience accessor. `crate::theme::theme().emphasis` reads better
/// at call sites than reaching into `ACTIVE` directly. Falls back to
/// Dalton Dark if [`init`] was never called (e.g. unit tests rendering
/// a widget in isolation).
#[must_use]
pub fn theme() -> &'static Theme {
    ACTIVE.get().unwrap_or(&Theme::DALTON_DARK)
}

/// Set the process-wide theme. Must be called before the first frame
/// is drawn; subsequent calls are no-ops (`OnceLock::set` returns Err).
/// Pair with [`resolve`] to compute the right value from the layered
/// preference sources.
pub fn init(t: Theme) {
    let _ = ACTIVE.set(t);
}

/// User's stated preference, in increasing precedence: config →
/// `SCITADEL_THEME` env → `--theme` CLI flag. The caller threads the
/// flag value here; we read the env var directly. `auto` (or any
/// unrecognised string) defers to terminal probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    /// `auto` — probe terminal background, fall back to dark.
    Auto,
    /// Forced dark — Dalton Dark today.
    Dark,
    /// Forced light — Dalton Bright today.
    Light,
}

impl ThemeChoice {
    /// Parse a layered preference string into a choice. Accepts
    /// `auto`, `dark`, `light`, and the named variants
    /// `dalton-dark` / `dalton-bright`. Anything else (including
    /// empty string) folds to `Auto` so a typo can't take a session
    /// down — it just falls through to detection.
    #[must_use]
    pub fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "dark" | "dalton-dark" => Self::Dark,
            "light" | "bright" | "dalton-bright" | "dalton-light" => Self::Light,
            _ => Self::Auto,
        }
    }
}

/// Resolve the layered user preference + auto-detection into a concrete
/// [`Theme`]. `cli` and `config_value` are the strings exactly as the
/// user wrote them (or empty if unset). The env var
/// `SCITADEL_THEME` sits between them in precedence.
#[must_use]
pub fn resolve(cli: Option<&str>, config_value: &str) -> Theme {
    resolve_with_label(cli, config_value).0
}

/// Like [`resolve`] but also returns a short label describing the
/// resolved palette and how it was picked (e.g.
/// `"dalton-dark (auto)"`). Used by the startup status-bar toast (#137).
#[must_use]
pub fn resolve_with_label(cli: Option<&str>, config_value: &str) -> (Theme, String) {
    let raw = cli
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("SCITADEL_THEME").ok())
        .unwrap_or_else(|| config_value.to_string());
    match ThemeChoice::parse(&raw) {
        ThemeChoice::Dark => (Theme::DALTON_DARK, "dalton-dark".into()),
        ThemeChoice::Light => (Theme::DALTON_BRIGHT, "dalton-bright".into()),
        ThemeChoice::Auto => match detect_terminal_background() {
            Some(TerminalBackground::Light) => {
                (Theme::DALTON_BRIGHT, "dalton-bright (auto)".into())
            }
            // Dark or unknown → dark. Dark is the safer default
            // because most dev terminals are dark and Dalton Dark
            // was the previous behaviour.
            _ => (Theme::DALTON_DARK, "dalton-dark (auto)".into()),
        },
    }
}

/// Whether the terminal background is light or dark. Returned as
/// `Option` from [`detect_terminal_background`] so callers can
/// distinguish "probed and got light" from "couldn't tell".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalBackground {
    Dark,
    Light,
}

/// Probe the terminal for its background luminance. Tries:
///
/// 1. `COLORFGBG` env var — cheap, no I/O. Format `"<fg>;<bg>"` where
///    each is an ANSI 0–15 colour index. Set by VTE-family terminals
///    and rxvt-unicode; widely respected.
/// 2. OSC 11 query (#176) — emit `\x1b]11;?\x07` to the controlling
///    tty, read back the RGB reply with a 150ms timeout, classify
///    by luminance. Works on foot, kitty, alacritty, modern xterm.
///    Skipped if stdin/stdout isn't a tty (test runner, redirected
///    I/O) or on non-unix platforms.
///
/// Returns `None` if both probes fail; the caller then falls back to
/// the dark default. Both probes run in priority order — COLORFGBG
/// first because it's free, OSC 11 second because it's authoritative
/// when it works.
#[must_use]
pub fn detect_terminal_background() -> Option<TerminalBackground> {
    if let Some(bg) = detect_via_colorfgbg() {
        return Some(bg);
    }
    detect_via_osc11()
}

/// Read `COLORFGBG` and classify the bg-index field. Pure env-var
/// read; isolated so the resolver can call it without dragging in the
/// OSC 11 fallback during tests that mutate env state.
fn detect_via_colorfgbg() -> Option<TerminalBackground> {
    let raw = std::env::var("COLORFGBG").ok()?;
    let bg = raw.split(';').nth(1)?.trim();
    let idx: u8 = bg.parse().ok()?;
    // ANSI base palette: 0–7 are dark variants, 8–15 are bright.
    // Bg index 0–6 = dark background; 7 (white) and 8+ = light.
    // The classic check: bg 15 = white = light, bg 0 = black = dark.
    if idx >= 7 {
        Some(TerminalBackground::Light)
    } else {
        Some(TerminalBackground::Dark)
    }
}

/// OSC 11 fallback. Returns `None` on any failure path (non-tty,
/// timeout, malformed reply) so the resolver can fall through to the
/// dark default. See `theme::osc11` for protocol detail.
fn detect_via_osc11() -> Option<TerminalBackground> {
    match osc11::detect()? {
        osc11::Luminance::Light => Some(TerminalBackground::Light),
        osc11::Luminance::Dark => Some(TerminalBackground::Dark),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize tests that mutate `SCITADEL_THEME` / `COLORFGBG`.
    /// Both vars are process-global; without this lock, the two
    /// env-mutating tests below race and `resolve()` may observe
    /// the other test's transient state mid-mutation. Surfaced as
    /// the #165 flake (Rgb(122, 109, 0) vs Rgb(196, 196, 12) on CI).
    /// Plain `Mutex<()>` beats `serial_test` for one isolated module.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn highlight_for_is_stable() {
        let t = Theme::DALTON_DARK;
        // Same input → same colour, every call, every process.
        let a1 = t.highlight_for("ann-root-1");
        let a2 = t.highlight_for("ann-root-1");
        assert_eq!(a1, a2);
    }

    #[test]
    fn theme_choice_parses_aliases() {
        assert_eq!(ThemeChoice::parse("dark"), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::parse("DARK"), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::parse("dalton-dark"), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::parse("light"), ThemeChoice::Light);
        assert_eq!(ThemeChoice::parse("bright"), ThemeChoice::Light);
        assert_eq!(ThemeChoice::parse("dalton-bright"), ThemeChoice::Light);
        assert_eq!(ThemeChoice::parse("dalton-light"), ThemeChoice::Light);
        assert_eq!(ThemeChoice::parse("auto"), ThemeChoice::Auto);
        // Typos fall back to Auto rather than panicking the session.
        assert_eq!(ThemeChoice::parse("darj"), ThemeChoice::Auto);
        assert_eq!(ThemeChoice::parse(""), ThemeChoice::Auto);
    }

    #[test]
    fn cli_flag_overrides_env_and_config() {
        // Hold ENV_LOCK across the whole body. cargo test runs in
        // parallel by default; without serialization the env mutations
        // here race with `auto_with_colorfgbg_picks_correct_theme`.
        // SAFETY: env var mutation is process-global; the lock ensures
        // no other test in this module observes our transient state.
        // Use `into_inner` on poison so a panicked sibling test doesn't
        // permanently disable this one.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Clear sibling-test state so we start from a known baseline.
        unsafe {
            std::env::remove_var("COLORFGBG");
            std::env::set_var("SCITADEL_THEME", "light");
        }
        // CLI dark beats env light beats config bright.
        let t = resolve(Some("dark"), "dalton-bright");
        assert_eq!(t.emphasis, Theme::DALTON_DARK.emphasis);
        // No CLI → env wins over config.
        let t = resolve(None, "dalton-dark");
        assert_eq!(t.emphasis, Theme::DALTON_BRIGHT.emphasis);
        // Empty CLI is treated as unset.
        let t = resolve(Some(""), "dalton-dark");
        assert_eq!(t.emphasis, Theme::DALTON_BRIGHT.emphasis);
        unsafe {
            std::env::remove_var("SCITADEL_THEME");
        }
        // No CLI, no env → config wins.
        let t = resolve(None, "dalton-dark");
        assert_eq!(t.emphasis, Theme::DALTON_DARK.emphasis);
    }

    #[test]
    fn auto_with_colorfgbg_picks_correct_theme() {
        // See ENV_LOCK rationale on `cli_flag_overrides_env_and_config`.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::remove_var("SCITADEL_THEME");
            std::env::set_var("COLORFGBG", "0;15"); // dark fg, white bg → light
        }
        let t = resolve(None, "auto");
        assert_eq!(t.emphasis, Theme::DALTON_BRIGHT.emphasis);
        unsafe {
            std::env::set_var("COLORFGBG", "15;0"); // white fg, black bg → dark
        }
        let t = resolve(None, "auto");
        assert_eq!(t.emphasis, Theme::DALTON_DARK.emphasis);
        unsafe {
            std::env::remove_var("COLORFGBG");
        }
        // No probe signal → dark fallback.
        let t = resolve(None, "auto");
        assert_eq!(t.emphasis, Theme::DALTON_DARK.emphasis);
    }

    #[test]
    fn registry_lists_every_user_facing_name() {
        // Every name advertised by `--list-themes` must round-trip
        // through `ThemeChoice::parse` to a non-Auto variant *or* be
        // the literal "auto" entry. Otherwise `--list-themes` would
        // print a value that the resolver silently folds to Auto.
        let names: Vec<&str> = Theme::registry().iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"auto"));
        assert!(names.contains(&"dark"));
        assert!(names.contains(&"light"));
        assert!(names.contains(&"dalton-dark"));
        assert!(names.contains(&"dalton-bright"));
        for name in &names {
            if *name == "auto" {
                assert_eq!(ThemeChoice::parse(name), ThemeChoice::Auto);
            } else {
                assert_ne!(
                    ThemeChoice::parse(name),
                    ThemeChoice::Auto,
                    "registry name '{name}' parses to Auto — listing it would mislead users",
                );
            }
        }
    }

    #[test]
    fn resolve_with_label_marks_auto_branch() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::remove_var("SCITADEL_THEME");
            std::env::remove_var("COLORFGBG");
        }
        let (_, label) = resolve_with_label(None, "auto");
        assert!(label.contains("(auto)"), "got label {label:?}");
        let (_, label) = resolve_with_label(Some("dark"), "auto");
        assert!(
            !label.contains("(auto)"),
            "explicit cli should not be tagged auto: {label:?}"
        );
        assert!(label.contains("dalton-dark"));
    }

    #[test]
    fn osc11_skipped_when_stdin_not_a_tty() {
        // Under cargo test stdin is a pipe, not a tty. The resolver
        // must therefore short-circuit OSC 11 and fall through to the
        // dark default rather than hanging on a terminal that will
        // never reply. This test fences the contract: if the auto
        // branch ever started spawning a real OSC 11 query in a
        // non-tty context, it would either hang the suite (timeout)
        // or print stray escape bytes — both unacceptable.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::remove_var("SCITADEL_THEME");
            std::env::remove_var("COLORFGBG");
        }
        let started = std::time::Instant::now();
        let t = resolve(None, "auto");
        // Resolver must return well under the OSC 11 timeout —
        // anything in the multi-tens-of-ms range means we actually
        // hit the polled read path.
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "auto resolution took {}ms — OSC 11 must not run on non-tty stdin",
            started.elapsed().as_millis(),
        );
        // Final fallback is dark.
        assert_eq!(t.emphasis, Theme::DALTON_DARK.emphasis);
    }

    #[test]
    fn highlight_for_covers_palette() {
        let t = Theme::DALTON_DARK;
        // A small sample of distinct keys should hit multiple slots —
        // not all 8, but > 1 — otherwise the hash is broken.
        let distinct: std::collections::HashSet<_> = (0..50)
            .map(|i| t.highlight_for(&format!("root-{i}")))
            .collect();
        assert!(
            distinct.len() > 3,
            "expected hash to spread across palette; got {} distinct slots",
            distinct.len()
        );
    }
}
