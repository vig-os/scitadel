use ratatui::style::Color;
use scitadel_db::sqlite::DownloadState;

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(3);
    let mut out: String = s.chars().take(keep).collect();
    out.push_str("...");
    out
}

/// The download-state cell for one row: `(glyph, colour)`.
///
/// # Derived, not stored (#253 S2e)
///
/// This used to read `papers.download_status`, a column that the download chain
/// dual-wrote alongside the `artefacts` row. That column is gone, so a reader
/// that wanted "did we get this paper" had to start reading the derivation ADR-007
/// §1 defines — and every view that wanted the answer had to reach for it
/// separately. So the answer is asked once, in `DataStore::load_download_states`,
/// and every view maps it through this one function.
///
/// `None` — a work with nothing recorded — is deliberately blank rather than a
/// symbol: ADR-007 §1 counts a work with no want row and no artefact as
/// *untracked*, and reporting "not downloaded" for a work nobody asked about would
/// be inventing a requirement.
///
/// `Missing` and `Gap` share the ✗ glyph on purpose. Both mean "we do not have it
/// and something is wrong", and the difference between "the bytes went" and "the
/// fetch failed" is a distinction `scitadel coverage` makes, not one a two-cell
/// column can.
pub fn download_state_cell(
    state: Option<DownloadState>,
    downloading: bool,
) -> (&'static str, Color) {
    let theme = crate::theme::theme();
    if downloading {
        return ("↻", theme.warning);
    }
    match state {
        Some(DownloadState::FullText) => ("✓", theme.success),
        Some(DownloadState::NotFullText) => ("⊘", theme.warning),
        Some(DownloadState::Missing | DownloadState::Gap) => ("✗", theme.danger),
        Some(DownloadState::Untracked) | None => (" ", theme.muted),
    }
}

#[cfg(test)]
mod tests {
    use super::{download_state_cell, truncate};
    use scitadel_db::sqlite::DownloadState;

    #[test]
    fn keeps_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncates_ascii() {
        assert_eq!(truncate("abcdefghij", 6), "abc...");
    }

    #[test]
    fn multi_byte_char_boundary_is_respected() {
        // Curly apostrophe U+2019 is 3 bytes; byte-slice of 27 would land mid-char.
        let s = "Isaac B. Hilton, Anthony D\u{2019}Ippolito et al.";
        let out = truncate(s, 30);
        assert!(out.ends_with("..."));
        assert!(out.chars().count() <= 30);
    }

    #[test]
    fn handles_zero_max() {
        assert_eq!(truncate("anything", 0), "...");
    }

    /// Every derived state has a glyph, and only a work that was obtained gets a
    /// ✓. The glyphs are what `tests/vhs/papers-download-state.tape` renders, so
    /// this pins them against the derivation rather than against a screenshot.
    #[test]
    fn every_derived_state_has_exactly_one_glyph() {
        let glyphs: Vec<&str> = DownloadState::ALL
            .iter()
            .map(|state| download_state_cell(Some(*state), false).0)
            .collect();
        for state in DownloadState::ALL {
            let glyph = download_state_cell(Some(state), false).0;
            assert!(
                matches!(glyph, "\u{2713}" | "\u{2298}" | "\u{2717}" | " "),
                "{state:?} renders {glyph:?}"
            );
        }
        assert_eq!(
            glyphs.iter().filter(|g| **g == "\u{2713}").count(),
            1,
            "only the full text is a ✓: {glyphs:?}"
        );
        assert_eq!(
            download_state_cell(None, false).0,
            " ",
            "an untracked work is blank, not a failure"
        );
        assert_eq!(
            download_state_cell(None, true).0,
            "\u{21bb}",
            "an in-flight download wins over whatever is on disk"
        );
    }
}
