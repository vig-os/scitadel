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

/// Draw the two-pane reader.
///
/// `scroll` is `&mut` so the draw can **clamp in state**: a stale
/// `u16::MAX` sentinel from a prior `G`, or a scroll offset the viewport
/// outgrew on resize, is re-written to the current wrapped maximum here.
/// This is the "clamp in state on every scroll action and on resize"
/// half of #307 — `terminal.draw` runs between every key press and on
/// every terminal resize, so a `k` after `G` moves immediately rather
/// than walking down from the saturated sentinel for thousands of
/// presses. `text_pane_width` is written back so `line_of_highlight`
/// can map a highlight to its wrapped row when the next `J`/`K` fires.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    area: Rect,
    data: &DataStore,
    paper_id: &str,
    focus: Option<usize>,
    reader: &str,
    scroll: &mut u16,
    text_pane_width: &mut u16,
) {
    let Ok(Some(paper)) = data.load_paper(paper_id) else {
        let block = Block::default().title(" Reader ").borders(Borders::ALL);
        let msg = Paragraph::new("Paper not found.").block(block);
        frame.render_widget(msg, area);
        return;
    };

    let annotations = data
        .load_annotations_for_paper(paper_id)
        .unwrap_or_default();
    // Roots only on the left pane — replies don't carry their own
    // anchor. Paper-level notes (#185) are also excluded from the
    // anchored-roots set: they have no quote / char_range and don't
    // produce a body highlight; they render in their own section.
    let roots: Vec<&Annotation> = annotations
        .iter()
        .filter(|a| !a.is_reply() && !a.anchor.is_paper_note())
        .collect();
    let paper_notes: Vec<&Annotation> = annotations
        .iter()
        .filter(|a| !a.is_reply() && a.anchor.is_paper_note())
        .collect();
    // Set of annotation IDs `reader` hasn't acknowledged yet, used by
    // the right-pane to render `[unread]` markers per row. (#185)
    let unread: std::collections::HashSet<String> = data
        .load_unread_for_paper(reader, paper_id)
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.id.as_str().to_string())
        .collect();

    // The body is the document's own text, so it is neutralised like the title
    // — `UntrustedBody::rendered` is the security half alone, which is the
    // whole point of the type: escapes, control characters and invisible
    // formatting go, paragraph structure and every other space stay. The
    // reader pages rather than truncating, so nothing here bounds the length.
    let body = paper
        .full_text
        .as_ref()
        .map(|body| body.rendered())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| paper.r#abstract.rendered());

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

    // Expose the text pane's inner width so a later `J`/`K` can map the
    // hopped highlight to the matching wrapped row. Width is written
    // back every frame so a resize is picked up on the next keystroke.
    *text_pane_width = chunks[0].width;
    draw_text_pane(
        frame,
        chunks[0],
        &paper.title.rendered(),
        &body,
        &roots,
        focus,
        scroll,
    );
    draw_notes_pane(
        frame,
        chunks[1],
        &annotations,
        &roots,
        &paper_notes,
        focus,
        &unread,
    );
}

fn draw_text_pane(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    body: &str,
    roots: &[&Annotation],
    focus: Option<usize>,
    scroll: &mut u16,
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
    // Clamp the offset against the **wrapped** row count of the whole
    // body at the current viewport width. Counting source lines (the
    // pre-fix shape of this clamp) made `G` stop early on a long
    // single-paragraph body — the 40 kB case from #307 wraps to
    // hundreds of rows but has zero `\n`, so a source-line count gave
    // zero max-scroll and the reader got stuck at the top. The
    // `unstable-rendered-line-info` feature on the `ratatui` dep gives
    // us `line_count`, which runs ratatui's own word-wrapper — same
    // code the renderer uses — so the clamp lines up with what the
    // paragraph actually paints. The clamp is also written back to
    // `*scroll` so a key handler that saw the sentinel `u16::MAX`
    // after `G` isn't still subtracting from it on the next `k` /
    // `u`; the stored offset is always in range of the current
    // viewport. Resize is covered by the same write: `terminal.draw`
    // re-runs with the new area dimensions.
    *scroll = (*scroll).min(max_scroll(&para, area));
    let para = para.scroll((*scroll, 0));
    frame.render_widget(para, area);
}

/// Largest in-range scroll offset for `para` rendered at `area`.
///
/// `Paragraph::line_count(width)` returns the number of wrapped **content**
/// rows plus the paragraph's block vertical padding (two for a bordered
/// block). The max offset leaves the last content row on the last
/// visible content row of the viewport: `total_rows - area.height`
/// (saturating at 0), using the outer `area.height` to match the +2
/// included in `line_count`. Width passed is the inner width
/// (`area.width - 2`), which is what the word-wrapper wraps against.
fn max_scroll(para: &Paragraph<'_>, area: Rect) -> u16 {
    let inner_width = area.width.saturating_sub(2);
    if inner_width == 0 {
        return 0;
    }
    let total_rows = para.line_count(inner_width);
    let max = total_rows.saturating_sub(usize::from(area.height));
    u16::try_from(max).unwrap_or(u16::MAX)
}

#[allow(clippy::too_many_arguments)]
fn draw_notes_pane(
    frame: &mut Frame,
    area: Rect,
    annotations: &[Annotation],
    roots: &[&Annotation],
    paper_notes: &[&Annotation],
    focus: Option<usize>,
    unread: &std::collections::HashSet<String>,
) {
    let mut lines: Vec<Line<'_>> = Vec::new();
    let unread_span = || {
        Span::styled(
            "  [unread]",
            Style::default()
                .fg(crate::theme::theme().emphasis)
                .add_modifier(Modifier::BOLD),
        )
    };

    // Paper-level notes section first — quote-less commentary on the
    // publication as a whole. Drawn above the threaded annotations
    // so the human sees overall reactions before passage-anchored
    // ones. Replies to a paper-level note are still rendered as
    // children, since the data model permits replying to anything. (#185)
    if !paper_notes.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "── Paper-level notes ──",
            Style::default()
                .fg(crate::theme::theme().muted)
                .add_modifier(Modifier::BOLD),
        )]));
        for note in paper_notes {
            let mut head = vec![Span::raw(format!(
                "  ¶ {} ({}): {}",
                note.author,
                note.created_at.format("%Y-%m-%d"),
                note.note,
            ))];
            if unread.contains(note.id.as_str()) {
                head.push(unread_span());
            }
            lines.push(Line::from(head));
            for ann in annotations.iter().filter(|a| {
                a.parent_id
                    .as_ref()
                    .is_some_and(|p| p.as_str() == note.id.as_str())
            }) {
                let mut spans = vec![
                    Span::raw("    └ "),
                    Span::styled(
                        format!("{}: ", ann.author),
                        Style::default().fg(crate::theme::theme().emphasis),
                    ),
                    Span::raw(ann.note.clone()),
                ];
                if unread.contains(ann.id.as_str()) {
                    spans.push(unread_span());
                }
                lines.push(Line::from(spans));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "── Threaded annotations ──",
            Style::default()
                .fg(crate::theme::theme().muted)
                .add_modifier(Modifier::BOLD),
        )]));
    }

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
                Span::styled(
                    "(no quote)",
                    Style::default().fg(crate::theme::theme().muted),
                ),
            ]));
        }
        let mut root_meta = vec![Span::raw(format!(
            "    {} ({}): {}",
            root.author,
            root.created_at.format("%Y-%m-%d"),
            root.note
        ))];
        if unread.contains(root.id.as_str()) {
            root_meta.push(unread_span());
        }
        lines.push(Line::from(root_meta));
        // Replies threaded under the root.
        for ann in annotations.iter().filter(|a| {
            a.parent_id
                .as_ref()
                .is_some_and(|p| p.as_str() == root.id.as_str())
        }) {
            let mut spans = vec![
                Span::raw("    └ "),
                Span::styled(
                    format!("{}: ", ann.author),
                    Style::default().fg(crate::theme::theme().emphasis),
                ),
                Span::raw(ann.note.clone()),
            ];
            if unread.contains(ann.id.as_str()) {
                spans.push(unread_span());
            }
            lines.push(Line::from(spans));
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

/// Post-wrap row of the `idx`-th root highlight in the paper body at
/// the current `text_pane_width`, or `None` when the paper is missing,
/// has no body, or the index does not resolve to a placed highlight.
///
/// The app layer uses this to auto-scroll the reader to a highlight
/// below the fold when the user hops onto it with `J`/`K` (#307).
/// `text_pane_width` is the reader's text-pane outer width (what
/// `draw` writes back); we wrap against its inner width (`width - 2`
/// for the block's left+right borders), matching `draw_text_pane`.
///
/// Counts **wrapped** rows rather than `\n` boundaries so a highlight
/// that lives past a long wrapped paragraph lands inside the viewport
/// — a source-line count puts the highlight below the fold on a 40 kB
/// paragraph with zero `\n` (the #307 case). Builds a prefix
/// paragraph with the same `Wrap { trim: false }` the renderer uses,
/// so the two agree on where the highlight falls.
pub fn line_of_highlight(
    data: &DataStore,
    paper_id: &str,
    idx: usize,
    text_pane_width: u16,
) -> Option<u16> {
    let paper = data.load_paper(paper_id).ok().flatten()?;
    let body = paper
        .full_text
        .as_ref()
        .map(|b| b.rendered().into_owned())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| paper.r#abstract.rendered().into_owned());
    if body.trim().is_empty() {
        return None;
    }
    let annotations = data.load_annotations_for_paper(paper_id).ok()?;
    let roots: Vec<&Annotation> = annotations
        .iter()
        .filter(|a| !a.is_reply() && !a.anchor.is_paper_note())
        .collect();
    let highlights = build_highlights(&body, &roots);
    // `build_highlights` is sorted by char_start, and sorted order has
    // no useful meaning for an app-layer index that walks the ROOTS
    // list in its original order — match by `root_index` so the hop
    // lands on the same highlight J/K selected.
    let target = highlights.iter().find(|h| h.root_index == idx)?;
    // Take `char_start + 1` so the quote's first char sits in the
    // prefix — `ratatui::Text::raw` uses `str::lines()` and so drops
    // a trailing empty line, which otherwise puts a quote that sits
    // right after a `\n` one row short. Including the next char
    // forces `line_count` to open a fresh row for it, and
    // `saturating_sub(1)` turns the resulting count into the row
    // index the hop should land on. A `line_count` of 0 (empty
    // document) folds to row 0.
    let prefix_incl_next: String = body.chars().take(target.char_start + 1).collect();
    // A key handler running before the first draw sees `text_pane_width
    // = 0`. Fall back to the source-line count in that case so the hop
    // still moves the viewport toward the quote — a close-enough start
    // that the first wrap puts it on screen — rather than snapping to
    // row 0 and losing the signal.
    let row = if text_pane_width <= 2 {
        body.chars()
            .take(target.char_start)
            .filter(|c| *c == '\n')
            .count()
    } else {
        wrapped_row_count(&prefix_incl_next, text_pane_width).saturating_sub(1)
    };
    u16::try_from(row).ok()
}

/// Number of wrapped rows `text` occupies when rendered as a
/// `Paragraph` with `Wrap { trim: false }` at the given
/// `text_pane_width` (the reader's **outer** text-pane width, same
/// value `line_of_highlight` takes). Returns 0 when the pane is too
/// narrow to hold any content.
///
/// Delegates to `ratatui`'s own wrapper via `Paragraph::line_count` so
/// the clamp in `draw_text_pane` and the hop target here can never
/// disagree about where a row falls. The empty-body row (no `\n`,
/// still one visible row) is accounted for by `line_count` itself.
fn wrapped_row_count(text: &str, text_pane_width: u16) -> usize {
    let inner_width = text_pane_width.saturating_sub(2);
    if inner_width == 0 {
        return 0;
    }
    Paragraph::new(text.to_string())
        .wrap(Wrap { trim: false })
        .line_count(inner_width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use scitadel_core::models::{Anchor, AnchorStatus, Annotation, Paper, PaperId};
    use scitadel_core::ports::PaperRepository as _;
    use scitadel_core::untrusted::{UntrustedBody, UntrustedText};

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

    /// Render `paper` in the reader and hand back every character the terminal
    /// would be asked to print.
    fn rendered_screen(paper: &Paper) -> String {
        rendered_rows(paper).concat()
    }

    /// Render `paper` in the reader, one string per terminal row.
    ///
    /// Row structure is the only way to assert that a body's *paragraphs*
    /// survived: a body collapsed to one line and a body that kept its newlines
    /// both contain the same words, and only one of them puts them on separate
    /// rows.
    fn rendered_rows(paper: &Paper) -> Vec<String> {
        rendered_rows_scrolled(paper, 0)
    }

    /// `rendered_rows` with an explicit scroll offset and no annotations seeded
    /// — the acceptance assertion for #307 is that scrolling the reader
    /// surfaces text that lives below the fold.
    fn rendered_rows_scrolled(paper: &Paper, scroll: u16) -> Vec<String> {
        rendered_rows_with(paper, scroll, &[])
    }

    /// Full-fat helper: render `paper` in the reader with `annotations`
    /// seeded against it and a given scroll offset. Returns the terminal
    /// buffer as one `String` per row, so a test can walk them and
    /// locate text or highlight styling.
    fn rendered_rows_with(paper: &Paper, scroll: u16, annotations: &[Annotation]) -> Vec<String> {
        let (_dir, data) = seed_paper(paper, annotations);
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("terminal");
        let mut scroll = scroll;
        let mut pane_width = 0u16;
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    &data,
                    paper.id.as_str(),
                    None,
                    "lars",
                    &mut scroll,
                    &mut pane_width,
                );
            })
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    /// Open a fresh DB, save `paper`, and seed `annotations`. Keeps the
    /// `TempDir` alive by returning it: the DB file lives there.
    fn seed_paper(paper: &Paper, annotations: &[Annotation]) -> (tempfile::TempDir, DataStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = DataStore::open(&dir.path().join("scitadel.db")).expect("open db");
        let (paper_repo, _, _, _, _) = data.db.repositories();
        paper_repo.save(paper).expect("save paper");
        if !annotations.is_empty() {
            let repo = scitadel_db::sqlite::SqliteAnnotationRepository::new(data.db.clone());
            for ann in annotations {
                repo.create(ann).expect("seed annotation");
            }
        }
        (dir, data)
    }

    /// Which row a phrase landed on, so a test can ask whether two phrases are
    /// on the same one.
    fn row_of(rows: &[String], needle: &str) -> Option<usize> {
        rows.iter().position(|row| row.contains(needle))
    }

    /// How many of the reader test's three short paragraphs one terminal row
    /// holds. Two on one row means the body was collapsed; a newline is the
    /// only thing that keeps them apart.
    fn phrases_on(row: &str) -> usize {
        ["Background.", "click me", "Findings."]
            .iter()
            .filter(|p| row.contains(**p))
            .count()
    }

    /// #287's TUI half: the reader is a terminal application, so a crafted
    /// `/Title` must not reach the screen intact. Asserted on the rendered
    /// buffer, not on a method having been called.
    ///
    /// The payload is the one `identity::pdf_title` really extracts from a
    /// UTF-16BE hex `/Title` — the encoding Word writes, and the only `/Title`
    /// encoding that can hold a literal backslash. The same payload is asserted
    /// through the MCP return path in `scitadel-mcp`.
    #[test]
    fn a_hostile_document_title_is_neutralised_in_the_rendered_buffer() {
        const HOSTILE: &str = "Real Title\u{1b}[31m\u{1b}[2J\u{1b}]8;;https://attacker.example/\u{1b}\\click\u{1b}]8;;\u{1b}\\";
        let mut paper = Paper::new(HOSTILE);
        paper.id = PaperId::from("p-hostile");
        // An abstract but no full text, so the reader takes its two-pane branch
        // — the one that puts the title in a block header *and* passes it into
        // the text pane, which is where an escape would reach the terminal.
        paper.r#abstract = UntrustedBody::publisher_supplied("An abstract.");
        paper.title = UntrustedText::publisher_supplied(HOSTILE);
        paper.authors = vec![UntrustedText::publisher_supplied("Attacker\u{1b}[2J, A.")];

        let screen = rendered_screen(&paper);

        assert!(
            !screen.contains('\u{1b}'),
            "no escape byte may reach the terminal: {screen:?}"
        );
        assert!(
            !screen.chars().any(char::is_control),
            "no control character may reach the terminal: {screen:?}"
        );
        for (label, needle) in [
            ("CSI", "31m"),
            ("erase-display", "2J"),
            ("OSC 8", "attacker.example"),
        ] {
            assert!(
                !screen.contains(needle),
                "the {label} payload must not reach the terminal: {screen:?}"
            );
        }
        assert!(
            screen.contains("Real Title click"),
            "the words survive, so a reader can still weigh the document: {screen:?}"
        );
    }

    /// #287's body half: the abstract is the document's text, and the reader is
    /// a terminal application, so an escape carried in the *body* must not
    /// reach the screen either — while the paragraph structure the reader came
    /// for does.
    ///
    /// Asserted on the payload remnants (`31m`, `2J`, the OSC 8 URL) rather than
    /// on "no ESC byte survives": ratatui drops escape bytes itself, so an
    /// assertion on the byte alone passes with the render path disabled. The
    /// payload is what a byte that never arrived leaves behind.
    ///
    /// And on the paragraph structure. The three phrases are short enough that
    /// none of them wraps in a 60%-wide pane, so the only way two of them can
    /// share a row is if the body was collapsed to one line — which is the
    /// assertion that cannot pass on a no-op, and cannot pass on a title wrapper
    /// either.
    #[test]
    fn a_hostile_body_is_neutralised_and_keeps_its_paragraph_structure() {
        const HOSTILE: &str = "Background.\n\n\
             \u{1b}[31m\u{1b}[2J\u{1b}[H\
             \u{1b}]8;;https://attacker.example/\u{1b}\\click me\u{1b}]8;;\u{1b}\\\n\n\
             \u{202e}reversed\u{202c} Findings.";
        let mut paper = Paper::new("A Paper");
        paper.id = PaperId::from("p-body-hostile");
        paper.r#abstract = UntrustedBody::publisher_supplied(HOSTILE);
        paper.title = UntrustedText::ours("A Paper");
        assert!(
            HOSTILE.contains("\n\n"),
            "the fixture really does carry paragraph breaks, which is what \
             makes the paragraph assertion below more than a restatement"
        );

        let screen = rendered_screen(&paper);
        let rows = rendered_rows(&paper);

        assert!(
            !screen.contains('\u{1b}'),
            "no escape byte may reach the terminal: {screen:?}"
        );
        for (label, needle) in [
            ("CSI colour", "31m"),
            ("erase-display", "2J"),
            ("cursor-home", "[H"),
            ("OSC 8", "attacker.example"),
            ("a bidi override", "\u{202e}"),
        ] {
            assert!(
                !screen.contains(needle),
                "the {label} payload must not reach the terminal: {screen:?}"
            );
        }
        assert!(
            screen.contains("click me"),
            "the link text survives, without the link: {screen:?}"
        );

        for phrase in ["Background.", "click me", "Findings."] {
            let row = row_of(&rows, phrase);
            assert!(row.is_some(), "{phrase:?} reached the screen: {rows:?}");
            // No row holds two of them: collapsed to one line they would share
            // one, and the only thing that keeps them apart is the newline.
            assert!(
                rows.iter().all(|r| phrases_on(r) < 2),
                "the {phrase:?} paragraph is on its own row, not joined to the \
                 next one: {rows:?}"
            );
        }
    }

    // ---- #307: reader scroll ----

    /// A body with 60 `line N` rows, so the text terminal (12 rows tall,
    /// of which 10 are body once the `Reader` block border lands) only
    /// ever shows a window into it. The `below_fold` line sits well
    /// past the initial window — asserting it appears after a scroll is
    /// what proves the viewport moved, not just that the body rendered.
    fn long_body_paper() -> Paper {
        use std::fmt::Write as _;
        let mut body = String::new();
        for i in 0..60 {
            if i == 30 {
                body.push_str("below_fold_token\n");
            } else {
                writeln!(body, "line {i}").unwrap();
            }
        }
        let mut p = Paper::new("Scrolling");
        p.id = PaperId::from("p-scroll");
        p.r#abstract = UntrustedBody::publisher_supplied("abs");
        p.full_text = Some(UntrustedBody::publisher_supplied(body));
        p
    }

    /// The body pane is the left 60% of the 100-col screen; the notes
    /// pane is the remaining 40 columns. The notes pane shows the
    /// quote, so an "is the token in view" check that scans the whole
    /// row flips on the notes column and reads false for the body.
    /// Clip to the body pane before asking.
    fn body_rows(rows: &[String]) -> Vec<String> {
        rows.iter().map(|r| r.chars().take(60).collect()).collect()
    }

    /// Scroll = 0 leaves the fold token below the viewport; a scroll
    /// of ~30 brings it into view. The acceptance half of #307.
    #[test]
    fn reader_scroll_moves_the_viewport_down_the_body() {
        let paper = long_body_paper();

        let rows_top = body_rows(&rendered_rows_scrolled(&paper, 0));
        assert!(
            !rows_top.iter().any(|r| r.contains("below_fold_token")),
            "the fold token lives past line 10 and must not be in view \
             before a scroll: {rows_top:?}"
        );

        let rows_scrolled = body_rows(&rendered_rows_scrolled(&paper, 30));
        assert!(
            rows_scrolled.iter().any(|r| r.contains("below_fold_token")),
            "scrolling must bring content below the fold into view: \
             {rows_scrolled:?}"
        );
    }

    /// Scrolling must not desync from highlights (`root_at`): the quote
    /// spans a line below the fold, and *where* its background color
    /// lands on screen tracks the scroll offset exactly. If the body
    /// moved but the highlight did not, this fails.
    ///
    /// Covers the acceptance item "a search match below the fold is
    /// reachable" and the TUI test called out in #307 ("add a test
    /// proving the highlight positions stay correct after scrolling").
    #[test]
    fn reader_scroll_keeps_highlights_aligned_with_root_at_offsets() {
        let paper = long_body_paper();
        let body = paper.full_text.as_ref().unwrap().as_str();
        // Place the highlight on the fold token so its row is far
        // enough below the top that scrolling actually moves it.
        let token_start = body
            .find("below_fold_token")
            .expect("fold token lives in the body");
        let token_char_start = body[..token_start].chars().count();
        let token_char_end = token_char_start + "below_fold_token".chars().count();
        let ann = {
            let mut a = Annotation::new_root(
                paper.id.clone(),
                "lars".into(),
                "note".into(),
                Anchor {
                    char_range: Some((token_char_start, token_char_end)),
                    quote: Some("below_fold_token".into()),
                    status: AnchorStatus::Ok,
                    ..Anchor::default()
                },
            );
            a.id = scitadel_core::models::AnnotationId::from("ann-fold");
            a
        };

        // Line number the quote sits on in the pre-wrap body — the
        // scroll offset that puts the highlight at (or near) the top.
        let expected_line =
            u16::try_from(body[..token_start].matches('\n').count()).expect("line fits u16");

        // No scroll: the highlight lives off-screen. Stronger than the
        // text alone: assert the painted cells at the expected
        // highlight row are still background-less (nothing painted
        // yet). Cells on an unstyled row have `bg == Reset` (the
        // ratatui default), so finding `Some(_)` means something
        // painted a highlight on the unscrolled view.
        let (_dir, data) = seed_paper(&paper, std::slice::from_ref(&ann));
        let mut term = Terminal::new(TestBackend::new(100, 12)).expect("terminal");
        let mut scroll_top = 0u16;
        let mut pane_width = 0u16;
        term.draw(|frame| {
            draw(
                frame,
                frame.area(),
                &data,
                paper.id.as_str(),
                None,
                "lars",
                &mut scroll_top,
                &mut pane_width,
            );
        })
        .expect("draw");
        let rows_top: Vec<String> = term
            .backend()
            .buffer()
            .content()
            .chunks(100)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        let body_top = body_rows(&rows_top);
        assert!(
            !body_top.iter().any(|r| r.contains("below_fold_token")),
            "the fold-aligned highlight must not be in view before a scroll: {body_top:?}"
        );

        // After scrolling to the highlight's line, the token reaches the
        // screen AND lands on a row whose cells carry the highlight's
        // background color — the "root_at offsets stay correct" check.
        let mut term = Terminal::new(TestBackend::new(100, 12)).expect("terminal");
        let mut scroll_at = expected_line;
        term.draw(|frame| {
            draw(
                frame,
                frame.area(),
                &data,
                paper.id.as_str(),
                None,
                "lars",
                &mut scroll_at,
                &mut pane_width,
            );
        })
        .expect("draw");
        let buffer = term.backend().buffer().clone();
        let rows_scrolled: Vec<String> = buffer
            .content()
            .chunks(100)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        let body_scrolled = body_rows(&rows_scrolled);
        let row = body_scrolled
            .iter()
            .position(|r| r.contains("below_fold_token"))
            .unwrap_or_else(|| panic!("fold token on screen after scroll: {body_scrolled:?}"));
        // Scan the body pane's columns for any painted bg cell — the
        // highlight styled the quote, and that style has to survive the
        // scroll offset. The notes pane is clipped off by the column
        // range so a background on *its* side can't satisfy the check.
        let painted = (0..60u16).any(|col| {
            let cell = &buffer[(col, u16::try_from(row).unwrap())];
            !matches!(cell.bg, Color::Reset)
        });
        assert!(
            painted,
            "the highlight's background must survive the scroll on the \
             row carrying its quote: row {row} contents {:?}",
            body_scrolled[row]
        );

        // `line_of_highlight` must agree with the scroll offset the
        // test derived, so the app layer's auto-scroll (J/K) lands on
        // the same line. Short source lines never wrap at the 60-col
        // pane width, so the wrapped row and the pre-wrap line match.
        assert_eq!(
            line_of_highlight(&data, paper.id.as_str(), 0, 60),
            Some(expected_line),
            "line_of_highlight agrees with the pre-wrap line, so J/K \
             scroll lands on the quote",
        );
    }

    /// A short body (one row) renders fine at `scroll = u16::MAX` — the
    /// `G`-to-bottom key. Nothing crashes; the single body row still
    /// shows up. Clamping lives in `draw_text_pane`; this is the test
    /// that a stale scroll cannot blank the pane.
    #[test]
    fn reader_scroll_past_the_end_is_clamped() {
        let mut p = Paper::new("Short");
        p.id = PaperId::from("p-short");
        p.r#abstract = UntrustedBody::publisher_supplied("abs");
        p.full_text = Some(UntrustedBody::publisher_supplied("one line only"));

        let rows = rendered_rows_scrolled(&p, u16::MAX);
        assert!(
            rows.iter().any(|r| r.contains("one line only")),
            "scroll clamped to the last line, so the sole row stays on screen: {rows:?}"
        );
    }

    // ---- #307 review: wrapped-row clamping ----

    /// A long single-paragraph body (the 40 kB case from #307) must
    /// still let `G` reach text past the first wrap. Before the fix
    /// the clamp counted source lines (`lines.len()`), so a body
    /// with no `\n` got `max_scroll == 0` and `G` stuck at the top.
    /// The paragraph ends with a tail token that only reaches the
    /// screen after a `u16::MAX` scroll is clamped to the real last
    /// wrapped row — the assertion the source-line clamp fails.
    #[test]
    fn g_reaches_the_end_of_a_long_wrapped_single_paragraph() {
        use std::fmt::Write as _;
        let mut body = String::new();
        // ~40 kB of text, no newlines. Roughly 300 words so word-wrap
        // actually breaks the paragraph, matching the #307 shape.
        for i in 0..4_000 {
            write!(body, "word{i} ").unwrap();
        }
        body.push_str("END_OF_PAPER");
        assert!(
            !body.contains('\n'),
            "fixture has no hard breaks, so the clamp has to wrap",
        );
        let mut p = Paper::new("Wrapped");
        p.id = PaperId::from("p-wrap");
        p.r#abstract = UntrustedBody::publisher_supplied("abs");
        p.full_text = Some(UntrustedBody::publisher_supplied(body));

        let rows = body_rows(&rendered_rows_scrolled(&p, u16::MAX));
        assert!(
            rows.iter().any(|r| r.contains("END_OF_PAPER")),
            "`G` past the end must land on the last wrapped row: {rows:?}",
        );
    }

    /// `line_of_highlight` maps a quote **past wrapped text** to the
    /// post-wrap row the highlight actually falls on, so a J/K hop
    /// lands inside the viewport. The source-line version (pre-fix)
    /// put the highlight below the fold when the paragraph above it
    /// wrapped.
    #[test]
    fn line_of_highlight_maps_past_a_wrapped_paragraph() {
        use std::fmt::Write as _;
        let pane_width: u16 = 60;
        // Inner width is pane_width - 2 borders.
        let inner_width = usize::from(pane_width - 2);
        // A wrap-worthy preamble of ~5 inner-width rows, then the
        // highlight target. The target's byte offset gives the row it
        // falls on so the test can assert on an exact wrapped row.
        let mut body = String::new();
        for i in 0..(inner_width * 5 / 6) {
            write!(body, "w{i} ").unwrap();
        }
        let prefix_len_before_newline = body.chars().count();
        body.push_str("\nTARGET_QUOTE after the wraps");

        let mut p = Paper::new("Wrapped highlight");
        p.id = PaperId::from("p-wrap-h");
        p.r#abstract = UntrustedBody::publisher_supplied("abs");
        p.full_text = Some(UntrustedBody::publisher_supplied(body.clone()));

        let target_start = body.find("TARGET_QUOTE").expect("target in body");
        let target_char_start = body[..target_start].chars().count();
        let target_char_end = target_char_start + "TARGET_QUOTE".chars().count();
        let ann = {
            let mut a = Annotation::new_root(
                p.id.clone(),
                "lars".into(),
                "note".into(),
                Anchor {
                    char_range: Some((target_char_start, target_char_end)),
                    quote: Some("TARGET_QUOTE".into()),
                    status: AnchorStatus::Ok,
                    ..Anchor::default()
                },
            );
            a.id = scitadel_core::models::AnnotationId::from("ann-wrap");
            a
        };

        let (_dir, data) = seed_paper(&p, std::slice::from_ref(&ann));
        let line = line_of_highlight(&data, p.id.as_str(), 0, pane_width).expect("mapped");
        // The preamble takes multiple wrapped rows (strictly more than
        // one, which is the whole point); a source-line count would
        // return 1 here. The fix returns the wrapped row count of
        // the preamble + newline.
        assert!(
            line >= 2,
            "line must be past the wrapped preamble, not the pre-wrap \
             `\\n` count (1): got {line}, preamble chars {prefix_len_before_newline}, pane_width {pane_width}",
        );

        // And the quote actually lands in the viewport when we scroll
        // the reader to that line — the J/K acceptance.
        let mut terminal = Terminal::new(TestBackend::new(pane_width + 40, 12)).expect("terminal");
        let mut scroll = line;
        let mut width_back = 0u16;
        terminal
            .draw(|frame| {
                let area = frame.area();
                let chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .split(area);
                let _ = chunks;
                draw(
                    frame,
                    area,
                    &data,
                    p.id.as_str(),
                    None,
                    "lars",
                    &mut scroll,
                    &mut width_back,
                );
            })
            .expect("draw");
        let rows: Vec<String> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(usize::from(pane_width + 40))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        assert!(
            rows.iter().any(|r| r.contains("TARGET_QUOTE")),
            "J-hop lands the highlight in view past the wrapped preamble: {rows:?}",
        );
    }

    /// Resize re-clamps the stored scroll: a user who scrolled to the
    /// bottom at narrow width, then widened the terminal, must not be
    /// left with a stale offset that stays saturated past the new
    /// wrapped last row. The clamp is in `draw`, so a second draw at
    /// the wider size writes the clamped value back.
    #[test]
    fn resize_reclamps_the_stored_scroll_offset() {
        use std::fmt::Write as _;
        let mut body = String::new();
        for i in 0..500 {
            writeln!(body, "line {i}").unwrap();
        }
        let mut p = Paper::new("Resize");
        p.id = PaperId::from("p-resize");
        p.r#abstract = UntrustedBody::publisher_supplied("abs");
        p.full_text = Some(UntrustedBody::publisher_supplied(body));

        let (_dir, data) = seed_paper(&p, &[]);

        // Narrow draw — many wrapped rows; `G` sentinel clamps high.
        let mut scroll = u16::MAX;
        let mut pane_width = 0u16;
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).expect("narrow terminal");
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    &data,
                    p.id.as_str(),
                    None,
                    "lars",
                    &mut scroll,
                    &mut pane_width,
                );
            })
            .expect("narrow draw");
        let narrow_max = scroll;
        assert!(
            narrow_max > 0 && narrow_max < u16::MAX,
            "narrow draw clamps the stored scroll to a real max: {narrow_max}",
        );

        // Wider terminal: the max drops, and the second draw rewrites
        // the stored offset to the new (smaller) max — not just at
        // render time, but in `scroll` itself.
        let mut terminal = Terminal::new(TestBackend::new(200, 24)).expect("wide terminal");
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    &data,
                    p.id.as_str(),
                    None,
                    "lars",
                    &mut scroll,
                    &mut pane_width,
                );
            })
            .expect("wide draw");
        assert!(
            scroll < narrow_max,
            "resize to a bigger pane re-clamps the stored scroll to the \
             new wrapped max: was {narrow_max}, now {scroll}",
        );
    }

    /// Resize re-clamps the stored scroll when the **wrap** changes —
    /// i.e. when pane WIDTH changes on a single long paragraph (the
    /// 40 kB zero-`\n` shape from #307), not just when pane HEIGHT
    /// changes on a line-separated body. A wider pane fits more
    /// characters per row, so the wrapped row count drops and the
    /// `G`-sentinel offset must drop with it. (#307 review nit.)
    #[test]
    fn resize_wider_reclamps_scroll_on_a_wrapping_paragraph() {
        // A single long paragraph: no `\n`, so the whole body wraps
        // against the pane's inner width. 4 000 chars is plenty to
        // outrun a 24-row viewport at any reasonable pane width.
        let body: String = "word ".repeat(800);
        let mut p = Paper::new("Wrap resize");
        p.id = PaperId::from("p-wrap-resize");
        p.r#abstract = UntrustedBody::publisher_supplied("abs");
        p.full_text = Some(UntrustedBody::publisher_supplied(body));

        let (_dir, data) = seed_paper(&p, &[]);

        // Narrow terminal, same height as the wide one below so we
        // isolate the width-driven wrap change from any height
        // contribution. `G`'s `u16::MAX` sentinel clamps to the
        // narrow-pane row max on the first draw.
        let mut scroll = u16::MAX;
        let mut pane_width = 0u16;
        let mut terminal = Terminal::new(TestBackend::new(40, 24)).expect("narrow terminal");
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    &data,
                    p.id.as_str(),
                    None,
                    "lars",
                    &mut scroll,
                    &mut pane_width,
                );
            })
            .expect("narrow draw");
        let narrow_max = scroll;
        let narrow_pane_width = pane_width;
        assert!(
            narrow_max > 0 && narrow_max < u16::MAX,
            "narrow draw clamps the sentinel to a real wrapped max: \
             {narrow_max}",
        );

        // Wide terminal at the SAME height. Fewer wrap breaks per
        // row means a smaller row count, and therefore a smaller
        // max offset. The clamp rewrites `scroll` in place.
        let mut terminal = Terminal::new(TestBackend::new(200, 24)).expect("wide terminal");
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    &data,
                    p.id.as_str(),
                    None,
                    "lars",
                    &mut scroll,
                    &mut pane_width,
                );
            })
            .expect("wide draw");
        assert!(
            pane_width > narrow_pane_width,
            "sanity: the wide draw records a wider text pane \
             ({pane_width} > {narrow_pane_width})",
        );
        assert!(
            scroll < narrow_max,
            "width-driven re-wrap re-clamps the stored scroll on a \
             single wrapping paragraph: was {narrow_max}, now {scroll}",
        );
    }
}
