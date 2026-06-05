//! Renderers for [`crate::diff::BibDiff`]: hand-rolled ANSI text and
//! structured JSON (#135 sub-feature C).
//!
//! Hand-rolled ANSI deliberately — adding a color crate for three
//! escape codes (green / red / yellow) would just be dependency churn.
//! Color is gated by the caller (TTY detection lives at the CLI layer
//! so we keep this module pure).

use std::fmt::Write as _;

use crate::diff::{BibDiff, ChangedEntry, Entry, FieldChange};

/// ANSI escape codes used by the text renderer. Matched per-call to
/// the user's color preference — when `use_color=false` the renderer
/// emits empty strings and the diff is plain text.
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

/// Bundle of ANSI codes the text renderer needs. Resolved once at the
/// top of [`render_text`] from `use_color`; sub-helpers take a
/// reference so we don't shuttle five `&str`s through every signature.
struct Palette {
    add: &'static str,
    remove: &'static str,
    change: &'static str,
    bold: &'static str,
    reset: &'static str,
}

impl Palette {
    fn new(use_color: bool) -> Self {
        if use_color {
            Self {
                add: GREEN,
                remove: RED,
                change: YELLOW,
                bold: BOLD,
                reset: RESET,
            }
        } else {
            Self {
                add: "",
                remove: "",
                change: "",
                bold: "",
                reset: "",
            }
        }
    }
}

/// Render a `BibDiff` as a human-readable text report. `header_a` /
/// `header_b` are free-form labels (e.g. file paths or "question
/// abc123") that go in the "=== bib diff: <a> vs <b> ===" line.
#[must_use]
pub fn render_text(diff: &BibDiff, header_a: &str, header_b: &str, use_color: bool) -> String {
    let mut out = String::new();
    let p = Palette::new(use_color);
    let bold = p.bold;
    let reset = p.reset;

    let _ = writeln!(
        out,
        "{bold}=== bib diff: {header_a} vs {header_b} ==={reset}"
    );
    if diff.is_empty() {
        out.push_str("\nNo differences.\n");
        return out;
    }

    let _ = writeln!(out, "\n{bold}ADDED ({}):{reset}", diff.added.len());
    if diff.added.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for e in &diff.added {
            let _ = writeln!(out, "  {}+ {}{}", p.add, entry_summary(e), p.reset);
        }
    }

    let _ = writeln!(out, "\n{bold}REMOVED ({}):{reset}", diff.removed.len());
    if diff.removed.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for e in &diff.removed {
            let _ = writeln!(out, "  {}- {}{}", p.remove, entry_summary(e), p.reset);
        }
    }

    let _ = writeln!(out, "\n{bold}CHANGED ({}):{reset}", diff.changed.len());
    if diff.changed.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for ce in &diff.changed {
            render_changed(&mut out, ce, &p);
        }
    }
    out
}

fn render_changed(out: &mut String, ce: &ChangedEntry, p: &Palette) {
    if let Some(prev) = &ce.before_citekey {
        let _ = writeln!(
            out,
            "  {}~ {} (was {prev}){}",
            p.change, ce.citekey, p.reset
        );
    } else {
        let _ = writeln!(out, "  {}~ {}{}", p.change, ce.citekey, p.reset);
    }
    let widest = ce
        .field_changes
        .iter()
        .map(|fc| fc.field.len())
        .max()
        .unwrap_or(0);
    for fc in &ce.field_changes {
        render_field_change(out, fc, widest, p);
    }
}

fn render_field_change(out: &mut String, fc: &FieldChange, widest: usize, p: &Palette) {
    let pad = " ".repeat(widest.saturating_sub(fc.field.len()));
    let _ = writeln!(
        out,
        "    {}{}{}{pad}: {} → {}",
        p.change,
        fc.field,
        p.reset,
        elide(fc.before.as_deref()),
        elide(fc.after.as_deref()),
    );
}

/// One-line summary like
/// `smith2024quantum  Smith, J. (2024) "Quantum X" — DOI:10.1234/...`.
/// Optional fields are gracefully omitted.
fn entry_summary(e: &Entry) -> String {
    let mut s = String::new();
    s.push_str(&e.citekey);
    s.push_str("  ");
    if let Some(first_author) = e.authors.first() {
        s.push_str(first_author);
    }
    if let Some(y) = e.year {
        if !s.ends_with("  ") {
            s.push(' ');
        }
        let _ = write!(s, "({y})");
    }
    if let Some(t) = &e.title {
        let _ = write!(s, " \"{}\"", truncate(t, 80));
    }
    if let Some(d) = &e.doi {
        let _ = write!(s, " — DOI:{d}");
    } else if let Some(a) = &e.arxiv_id {
        let _ = write!(s, " — arXiv:{a}");
    }
    s
}

/// Render a `BibDiff` as structured JSON. Just `serde_json::to_string_pretty`
/// — kept here so the CLI / MCP have one canonical entry point.
pub fn render_json(diff: &BibDiff) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string_pretty(diff)?;
    s.push('\n');
    Ok(s)
}

/// Truncate a string to `max` chars (NOT bytes — Unicode-safe). Adds
/// an ellipsis if truncation actually happened.
fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// Render `Option<&str>` for the before/after columns. `None` prints
/// as `<none>` so the user can distinguish "field absent" from
/// "field is empty string".
fn elide(s: Option<&str>) -> String {
    match s {
        None => "<none>".to_string(),
        Some("") => "<empty>".to_string(),
        Some(v) => truncate(v, 80),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{ChangedEntry, FieldChange};

    fn full_entry(k: &str, t: &str) -> Entry {
        Entry {
            citekey: k.into(),
            title: Some(t.into()),
            year: Some(2024),
            authors: vec!["Smith, J.".into()],
            doi: Some("10.1/abc".into()),
            ..Default::default()
        }
    }

    fn sample_diff() -> BibDiff {
        BibDiff {
            added: vec![full_entry("a-add", "Added")],
            removed: vec![full_entry("a-rem", "Removed")],
            changed: vec![ChangedEntry {
                citekey: "k".into(),
                before_citekey: None,
                field_changes: vec![FieldChange {
                    field: "title".into(),
                    before: Some("Old".into()),
                    after: Some("New".into()),
                }],
            }],
        }
    }

    #[test]
    fn text_no_color_has_no_ansi_escapes() {
        let out = render_text(&sample_diff(), "A", "B", false);
        assert!(!out.contains('\x1b'), "ANSI must not appear: {out:?}");
    }

    #[test]
    fn text_with_color_has_ansi_escapes() {
        let out = render_text(&sample_diff(), "A", "B", true);
        assert!(out.contains("\x1b["), "ANSI required when use_color=true");
        assert!(out.contains(GREEN), "added uses green");
        assert!(out.contains(RED), "removed uses red");
        assert!(out.contains(YELLOW), "changed/field uses yellow");
        assert!(out.contains(RESET), "must reset color");
    }

    #[test]
    fn text_renders_section_counts() {
        let out = render_text(&sample_diff(), "A", "B", false);
        assert!(out.contains("ADDED (1):"));
        assert!(out.contains("REMOVED (1):"));
        assert!(out.contains("CHANGED (1):"));
    }

    #[test]
    fn text_renders_field_change_arrow() {
        let out = render_text(&sample_diff(), "A", "B", false);
        assert!(out.contains("title"), "field name present");
        assert!(out.contains("Old → New"), "before → after rendered");
    }

    #[test]
    fn text_renders_no_diff_message_when_empty() {
        let d = BibDiff::default();
        let out = render_text(&d, "A", "B", false);
        assert!(out.contains("No differences"));
    }

    #[test]
    fn text_renders_renamed_citekey() {
        let mut d = sample_diff();
        d.changed[0].before_citekey = Some("old-key".into());
        let out = render_text(&d, "A", "B", false);
        assert!(out.contains("(was old-key)"), "rename hint must appear");
    }

    #[test]
    fn json_round_trip() {
        let d = sample_diff();
        let json = render_json(&d).unwrap();
        let back: BibDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn elide_distinguishes_none_and_empty() {
        assert_eq!(elide(None), "<none>");
        assert_eq!(elide(Some("")), "<empty>");
        assert_eq!(elide(Some("hello")), "hello");
    }

    #[test]
    fn truncate_unicode_safe() {
        // Greek alpha is one char but two bytes — make sure we don't
        // panic on a byte-boundary slice.
        let s = "αβγδεζηθ";
        let out = truncate(s, 4);
        assert!(out.chars().count() <= 4);
        assert!(out.ends_with('…'));
    }
}
