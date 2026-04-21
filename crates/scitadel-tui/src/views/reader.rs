//! Two-pane annotation reader (#97).
//!
//! Left pane: paper text (`full_text` if cached, otherwise the
//! abstract) with background-color highlights over annotated ranges.
//! Right pane: the annotation list synced with the focused highlight.
//!
//! Falls back to a single-pane "no body text" notice when neither
//! `full_text` nor a non-empty abstract is available.
//!
//! Out of scope (future iterations of #97):
//! - Multi-line gutter bars when several threads overlap on the same span
//! - Variable-width font / proper PDF overlay
//! - Fuzzy-orphan re-anchor prompt (resolver lives in scitadel-db; the
//!   reader currently shows the resolver-derived `anchor.status`).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::data::DataStore;
use scitadel_core::models::Annotation;

// Highlight palette lives in `crate::theme` (#136). This keeps the 8
// annotation-background tints swappable alongside the rest of the UI
// when light-mode / auto-detect ships in #137.

pub fn draw(frame: &mut Frame, area: Rect, data: &DataStore, paper_id: &str, focus: Option<usize>) {
    let Ok(Some(paper)) = data.load_paper(paper_id) else {
        let block = Block::default().title(" Reader ").borders(Borders::ALL);
        let msg = Paragraph::new("Paper not found.").block(block);
        frame.render_widget(msg, area);
        return;
    };

    let annotations = data
        .load_annotations_for_paper(paper_id)
        .unwrap_or_default();
    // Roots only on the left pane — replies don't carry their own anchor.
    let roots: Vec<&Annotation> = annotations.iter().filter(|a| !a.is_reply()).collect();

    let body = paper
        .full_text
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(&paper.r#abstract);

    if body.trim().is_empty() {
        let block = Block::default()
            .title(format!(" Reader — {} ", paper.title))
            .borders(Borders::ALL);
        let msg = Paragraph::new(
            "No body text available yet. Run `scitadel download <id>` then \
             open the paper via MCP `read_paper` to populate full_text.",
        )
        .block(block)
        .wrap(Wrap { trim: false });
        frame.render_widget(msg, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    draw_text_pane(frame, chunks[0], &paper.title, body, &roots, focus);
    draw_notes_pane(frame, chunks[1], &annotations, &roots, focus);
}

fn draw_text_pane(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    body: &str,
    roots: &[&Annotation],
    focus: Option<usize>,
) {
    let highlights = build_highlights(body, roots);
    let lines = render_with_highlights(body, &highlights, focus);
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Reader — {title} "))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

fn draw_notes_pane(
    frame: &mut Frame,
    area: Rect,
    annotations: &[Annotation],
    roots: &[&Annotation],
    focus: Option<usize>,
) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    for (idx, root) in roots.iter().enumerate() {
        let color = color_for(root.id.as_str());
        let is_focused = focus == Some(idx);
        let marker = if is_focused { "▶ " } else { "  " };
        let marker_style = if is_focused {
            Style::default()
                .fg(crate::theme::theme().emphasis)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        if let Some(quote) = root.anchor.quote.as_deref() {
            lines.push(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(
                    format!(" \"{quote}\" "),
                    Style::default().bg(color).fg(Color::White),
                ),
                Span::styled(
                    format!(" — {}", root.anchor.status.as_str()),
                    Style::default().fg(crate::theme::theme().muted),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled("(no quote)", Style::default().fg(crate::theme::theme().muted)),
            ]));
        }
        lines.push(Line::from(format!(
            "    {} ({}): {}",
            root.author,
            root.created_at.format("%Y-%m-%d"),
            root.note
        )));
        // Replies threaded under the root.
        for ann in annotations.iter().filter(|a| {
            a.parent_id
                .as_ref()
                .is_some_and(|p| p.as_str() == root.id.as_str())
        }) {
            lines.push(Line::from(vec![
                Span::raw("    └ "),
                Span::styled(
                    format!("{}: ", ann.author),
                    Style::default().fg(crate::theme::theme().emphasis),
                ),
                Span::raw(ann.note.clone()),
            ]));
        }
        lines.push(Line::from(""));
    }
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Notes ({}) ", roots.len()))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

/// Resolved highlight: where a root's quote currently sits in the body
/// text + which color to paint it. None of the work below mutates the
/// paper text — we only style over it.
#[derive(Debug, Clone)]
struct Highlight {
    char_start: usize,
    char_end: usize,
    color: Color,
    /// Index into the parent `roots` slice, used to pair with the focus
    /// cursor.
    root_index: usize,
}

/// Best-effort positions for each root annotation in the body. Tries
/// `anchor.char_range` first, falls back to a substring lookup of the
/// quote. Anchors that don't match are silently skipped (they're
/// already surfaced as `orphan` in the right pane).
fn build_highlights(body: &str, roots: &[&Annotation]) -> Vec<Highlight> {
    let mut out = Vec::new();
    for (idx, root) in roots.iter().enumerate() {
        let color = color_for(root.id.as_str());
        if let Some((s, e)) = root.anchor.char_range {
            // Bounds-check; skip if offsets don't fit the current body.
            let total_chars = body.chars().count();
            if e <= total_chars && s < e {
                out.push(Highlight {
                    char_start: s,
                    char_end: e,
                    color,
                    root_index: idx,
                });
                continue;
            }
        }
        if let Some(quote) = root.anchor.quote.as_deref()
            && let Some(byte_pos) = body.find(quote)
        {
            let start_char = body[..byte_pos].chars().count();
            let end_char = start_char + quote.chars().count();
            out.push(Highlight {
                char_start: start_char,
                char_end: end_char,
                color,
                root_index: idx,
            });
        }
    }
    // Sort + dedupe by start so the renderer can walk them in order.
    out.sort_by_key(|h| h.char_start);
    out
}

/// Render `body` as styled lines, painting background colors over
/// highlight spans. The focused highlight gets an underline so it's
/// distinguishable from the others.
fn render_with_highlights(
    body: &str,
    highlights: &[Highlight],
    focus: Option<usize>,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut current_pos = 0;

    for raw_line in body.split('\n') {
        let line_start = current_pos;
        let line_end = current_pos + raw_line.chars().count();
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut cursor = line_start;

        // Walk highlights that overlap this line, in order.
        for h in highlights
            .iter()
            .filter(|h| h.char_start < line_end && h.char_end > line_start)
        {
            // Plain text up to the highlight's start.
            let plain_end = h.char_start.max(line_start);
            if cursor < plain_end {
                spans.push(Span::raw(slice_chars(&chars, cursor, plain_end)));
            }
            // The highlight, clipped to this line.
            let hi_start = h.char_start.max(line_start);
            let hi_end = h.char_end.min(line_end);
            if hi_start < hi_end {
                let mut style = Style::default().bg(h.color).fg(Color::White);
                if focus == Some(h.root_index) {
                    style = style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
                }
                spans.push(Span::styled(slice_chars(&chars, hi_start, hi_end), style));
            }
            cursor = hi_end;
        }
        if cursor < line_end {
            spans.push(Span::raw(slice_chars(&chars, cursor, line_end)));
        }
        if spans.is_empty() {
            spans.push(Span::raw(String::new()));
        }
        lines.push(Line::from(spans));
        current_pos = line_end + 1; // +1 for the consumed '\n'
    }
    lines
}

fn slice_chars(chars: &[char], start: usize, end: usize) -> String {
    let start = start.min(chars.len());
    let end = end.min(chars.len());
    if end <= start {
        return String::new();
    }
    chars[start..end].iter().collect()
}

/// Stable-per-thread color: delegates to `Theme::highlight_for` on the
/// active theme so this module stays palette-agnostic (#136).
fn color_for(root_id: &str) -> Color {
    crate::theme::theme().highlight_for(root_id)
}

/// How many root annotations does this paper have? Used by the app
/// layer to know whether `J`/`K` should hop between highlights.
pub fn highlight_count(data: &DataStore, paper_id: &str) -> usize {
    data.load_annotations_for_paper(paper_id)
        .map_or(0, |anns| anns.iter().filter(|a| !a.is_reply()).count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::{Anchor, AnchorStatus, Annotation, PaperId};

    fn root_at(quote: &str, range: Option<(usize, usize)>) -> Annotation {
        let mut a = Annotation::new_root(
            PaperId::from("p1"),
            "lars".into(),
            "note".into(),
            Anchor {
                char_range: range,
                quote: Some(quote.into()),
                status: AnchorStatus::Ok,
                ..Anchor::default()
            },
        );
        // Force a stable id so color_for() is deterministic across test runs.
        a.id = scitadel_core::models::AnnotationId::from(format!("ann-{quote}"));
        a
    }

    #[test]
    fn build_highlights_uses_char_range_when_in_bounds() {
        let body = "hello world, this is a test.";
        let r1 = root_at("world", Some((6, 11)));
        let highlights = build_highlights(body, &[&r1]);
        assert_eq!(highlights.len(), 1);
        assert_eq!((highlights[0].char_start, highlights[0].char_end), (6, 11));
    }

    #[test]
    fn build_highlights_falls_back_to_substring_when_offsets_oob() {
        let body = "hello world";
        let r1 = root_at("world", Some((9000, 9100)));
        let highlights = build_highlights(body, &[&r1]);
        assert_eq!(highlights.len(), 1);
        assert_eq!((highlights[0].char_start, highlights[0].char_end), (6, 11));
    }

    #[test]
    fn build_highlights_skips_unmatched_quotes() {
        let body = "hello";
        let r1 = root_at("nowhere", None);
        assert!(build_highlights(body, &[&r1]).is_empty());
    }

    #[test]
    fn render_with_highlights_paints_background_only_over_quote() {
        let body = "hello world";
        let r1 = root_at("world", Some((6, 11)));
        let highlights = build_highlights(body, &[&r1]);
        let lines = render_with_highlights(body, &highlights, None);
        assert_eq!(lines.len(), 1);
        // Spans: ["hello ", "world"] — the second one carries a bg.
        let line = &lines[0];
        assert!(
            line.spans.iter().any(|s| s.style.bg.is_some()),
            "expected at least one styled span with a background"
        );
        let total: String = line
            .spans
            .iter()
            .map(|s| s.content.clone().into_owned())
            .collect();
        assert_eq!(total, "hello world");
    }

    #[test]
    fn focus_adds_underline_to_focused_highlight() {
        let body = "hello world";
        let r1 = root_at("world", Some((6, 11)));
        let highlights = build_highlights(body, &[&r1]);
        let lines = render_with_highlights(body, &highlights, Some(0));
        let styled = lines[0]
            .spans
            .iter()
            .find(|s| s.style.bg.is_some())
            .expect("styled span");
        assert!(
            styled.style.add_modifier.contains(Modifier::UNDERLINED),
            "focused highlight should be underlined"
        );
    }

    #[test]
    fn color_for_is_deterministic() {
        assert_eq!(color_for("ann-foo"), color_for("ann-foo"));
    }

    #[test]
    fn render_handles_multiline_body_without_panic() {
        let body = "line one\nline two\nline three";
        let r1 = root_at("two", None); // substring match → middle line
        let highlights = build_highlights(body, &[&r1]);
        let lines = render_with_highlights(body, &highlights, None);
        assert_eq!(lines.len(), 3);
    }
}
