//! Path prompt overlay for the Question Dashboard `E` keybind
//! (#135 sub-feature B).
//!
//! Single-buffer text prompt pre-filled with a question-derived default
//! filename. Mirrors the shape of [`crate::views::annotation_prompt`]
//! but with one buffer instead of a multi-stage state machine — the
//! whole UX is "accept the default, or edit the path, then Enter".

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use scitadel_export::{SnapshotFormat, slugify};

/// State for the export-path prompt. The format is fixed at construction
/// time (default = BibTeX); a future iteration may add a toggle inside
/// the overlay, but that's out of scope for sub-feature B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibExportPrompt {
    /// Mutable path buffer — pre-filled with the slugified question.
    pub path_buf: String,
    /// Output format. Routes the export to BibTeX vs CSL-JSON; baked in
    /// at construction so the slugified default ends with the right
    /// extension.
    pub format: SnapshotFormat,
}

impl BibExportPrompt {
    /// Build a prompt pre-filled with `<slug>.bib` (or `.json`) derived
    /// from the question text. Empty / unhelpful text falls back to
    /// `paper.bib` to mirror the CLI's `--output` default.
    #[must_use]
    pub fn from_question(question_text: &str, format: SnapshotFormat) -> Self {
        let stem = slugify(question_text);
        let path_buf = format!("{stem}{}", format.extension());
        Self { path_buf, format }
    }

    /// Append a typed character to the path buffer.
    pub fn push_char(&mut self, ch: char) {
        self.path_buf.push(ch);
    }

    /// Pop the last character from the path buffer (Backspace).
    pub fn backspace(&mut self) {
        self.path_buf.pop();
    }

    /// Snapshot of the path the user has typed. Returns `None` when the
    /// buffer is empty or whitespace-only — Enter on an empty buffer
    /// should cancel rather than write a file at `.`.
    #[must_use]
    pub fn submit(&self) -> Option<String> {
        let trimmed = self.path_buf.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

/// Centred modal renderer; matches the shape of
/// [`crate::views::annotation_prompt::draw_overlay`].
pub fn draw_overlay(frame: &mut Frame, area: Rect, prompt: &BibExportPrompt) {
    let modal = centered_rect(area, 70, 30);
    frame.render_widget(Clear, modal);

    let title = match prompt.format {
        SnapshotFormat::BibTeX => " Export Bibliography (BibTeX) ",
        SnapshotFormat::CslJson => " Export Bibliography (CSL-JSON) ",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().fg(crate::theme::theme().emphasis));
    frame.render_widget(block, modal);

    let body_area = Rect {
        x: modal.x + 2,
        y: modal.y + 1,
        width: modal.width.saturating_sub(4),
        height: modal.height.saturating_sub(2),
    };

    let lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "Write the shortlist's bibliography to:",
            Style::default().fg(crate::theme::theme().muted),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("path: ", Style::default().fg(crate::theme::theme().muted)),
            Span::styled(
                prompt.path_buf.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(crate::theme::theme().emphasis)),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "A `.scitadel-bib.lock` sidecar is written next to it.",
            Style::default().fg(crate::theme::theme().muted),
        )),
    ];

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, body_area);
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_uses_slug_and_bib_extension() {
        let p = BibExportPrompt::from_question(
            "What is the role of attention?",
            SnapshotFormat::BibTeX,
        );
        assert_eq!(p.path_buf, "what-is-the-role-of-attention.bib");
    }

    #[test]
    fn default_path_uses_json_extension_for_csl_json() {
        let p = BibExportPrompt::from_question("Long Short-Term Memory", SnapshotFormat::CslJson);
        assert_eq!(p.path_buf, "long-short-term-memory.json");
    }

    #[test]
    fn empty_question_falls_back_to_untitled_with_extension() {
        let p = BibExportPrompt::from_question("???", SnapshotFormat::BibTeX);
        assert_eq!(p.path_buf, "untitled.bib");
    }

    #[test]
    fn push_and_backspace_edit_the_buffer() {
        let mut p = BibExportPrompt::from_question("x", SnapshotFormat::BibTeX);
        p.path_buf.clear();
        for c in "out/foo".chars() {
            p.push_char(c);
        }
        assert_eq!(p.path_buf, "out/foo");
        p.backspace();
        assert_eq!(p.path_buf, "out/fo");
    }

    #[test]
    fn submit_returns_trimmed_path_or_none_on_empty() {
        let mut p = BibExportPrompt::from_question("x", SnapshotFormat::BibTeX);
        assert_eq!(p.submit(), Some("x.bib".to_string()));
        p.path_buf = "  out/file.bib  ".into();
        assert_eq!(p.submit(), Some("out/file.bib".to_string()));
        p.path_buf.clear();
        assert!(p.submit().is_none());
        p.path_buf = "   ".into();
        assert!(p.submit().is_none());
    }
}
