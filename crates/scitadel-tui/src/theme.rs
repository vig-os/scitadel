//! Semantic color theming for the TUI (#136).
//!
//! Every TUI view pulls colors through named roles on [`Theme`] rather
//! than hardcoding palette (`Color::Yellow`) or RGB values. That lets
//! the theme be swapped wholesale — currently Dalton Dark by default;
//! light mode + auto-detection ships in #137.
//!
//! Dalton Dark is a colorblind-friendly scheme tuned for deuteranopia
//! and protanopia. Source: <https://github.com/gerchowl/dalton-colorscheme>.

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

/// Process-wide active theme. Currently a const alias for Dalton Dark;
/// #137 replaces this with a runtime-resolved value (config, flag,
/// env, auto-detect).
pub const ACTIVE: &Theme = &Theme::DALTON_DARK;

/// Convenience accessor. `crate::theme::theme().emphasis` reads better
/// at call sites than `crate::theme::ACTIVE.emphasis`.
#[must_use]
pub fn theme() -> &'static Theme {
    ACTIVE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_for_is_stable() {
        let t = Theme::DALTON_DARK;
        // Same input → same colour, every call, every process.
        let a1 = t.highlight_for("ann-root-1");
        let a2 = t.highlight_for("ann-root-1");
        assert_eq!(a1, a2);
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
