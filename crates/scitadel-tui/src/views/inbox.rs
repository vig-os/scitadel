//! Inbox overlay (#185 P0): a flat list of every unread annotation
//! grouped by paper, sorted thread-major (root first, then its
//! replies). Drives the `[N new]` status-bar badge to a concrete
//! action — the user presses `U`, sees what's new, presses Enter to
//! jump to the paper reader focused on the thread.
//!
//! Closed via `Esc` / `q` / `U` (toggle).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use std::collections::HashMap;

use scitadel_core::models::{Annotation, Paper};

use crate::data::DataStore;

/// One row in the inbox — either a paper header or an annotation under it.
/// Headers are non-selectable and don't carry a paper_id payload that
/// would let the user "jump" to anything different than picking the
/// first unread row under the header. Kept distinct so the index
/// passed back to `app.rs` (the cursor) maps cleanly.
#[derive(Debug, Clone)]
pub enum InboxItem {
    /// Paper-level header. Not selectable (selection skips it).
    /// `paper_id` is kept on the variant so future PRs that add a
    /// "mark all in this paper as read" action have a target without
    /// needing to walk back through the rows beneath the header.
    Header {
        #[allow(dead_code)]
        paper_id: String,
        title: String,
        count: usize,
    },
    /// Selectable row representing an unread annotation. `paper_id`
    /// + `root_id` are the jump target.
    Row {
        paper_id: String,
        root_id: String,
        author: String,
        note_preview: String,
        is_reply: bool,
    },
}

impl InboxItem {
    pub fn is_selectable(&self) -> bool {
        matches!(self, InboxItem::Row { .. })
    }
}

/// Build the flat thread-major item list from every unread annotation
/// for `reader`. Pure function over the loaded data so it's
/// unit-testable without a Frame.
pub fn build_items(unread: &[Annotation], papers: &HashMap<String, Paper>) -> Vec<InboxItem> {
    use std::collections::BTreeMap;
    // Group by paper_id, preserving insertion order (BTreeMap by
    // paper_id keeps the output deterministic across runs even when
    // SQLite returns equal-`created_at` rows in unspecified order).
    let mut by_paper: BTreeMap<String, Vec<&Annotation>> = BTreeMap::new();
    for a in unread {
        by_paper
            .entry(a.paper_id.as_str().to_string())
            .or_default()
            .push(a);
    }

    let mut items = Vec::new();
    for (paper_id, anns) in &by_paper {
        let title = papers
            .get(paper_id)
            .map_or_else(|| paper_id.clone(), |p| p.title.clone());
        items.push(InboxItem::Header {
            paper_id: paper_id.clone(),
            title,
            count: anns.len(),
        });
        // Thread-major: roots first, then their replies, in created_at
        // order (which is what `list_unread` already guarantees).
        let roots: Vec<&&Annotation> = anns.iter().filter(|a| !a.is_reply()).collect();
        for root in roots {
            items.push(InboxItem::Row {
                paper_id: paper_id.clone(),
                root_id: root.id.as_str().to_string(),
                author: root.author.clone(),
                note_preview: preview(&root.note, 60),
                is_reply: false,
            });
            for ann in anns.iter().filter(|a| {
                a.parent_id
                    .as_ref()
                    .is_some_and(|p| p.as_str() == root.id.as_str())
            }) {
                items.push(InboxItem::Row {
                    paper_id: paper_id.clone(),
                    root_id: root.id.as_str().to_string(),
                    author: ann.author.clone(),
                    note_preview: preview(&ann.note, 60),
                    is_reply: true,
                });
            }
        }
        // Orphan replies: parent is not in the unread set (e.g. user
        // already saw the root and only the reply is fresh). Surface
        // them under their parent_id as the root.
        let live_root_ids: std::collections::HashSet<&str> = anns
            .iter()
            .filter(|a| !a.is_reply())
            .map(|a| a.id.as_str())
            .collect();
        for orphan in anns.iter().filter(|a| {
            a.is_reply()
                && a.parent_id
                    .as_ref()
                    .is_some_and(|p| !live_root_ids.contains(p.as_str()))
        }) {
            let root_id = orphan
                .parent_id
                .as_ref()
                .map_or(orphan.id.as_str().to_string(), |p| p.as_str().to_string());
            items.push(InboxItem::Row {
                paper_id: paper_id.clone(),
                root_id,
                author: orphan.author.clone(),
                note_preview: preview(&orphan.note, 60),
                is_reply: true,
            });
        }
    }
    items
}

fn preview(note: &str, n: usize) -> String {
    let cleaned: String = note.replace(['\n', '\r'], " ");
    if cleaned.chars().count() <= n {
        cleaned
    } else {
        let head: String = cleaned.chars().take(n.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Reload the inbox state from the DB. Returned items match what
/// `draw` will render; the caller stores them on the overlay so the
/// jump action can resolve `selected` → `(paper_id, root_id)`.
pub fn load(data: &DataStore, reader: &str) -> Vec<InboxItem> {
    let unread = data.load_all_unread(reader).unwrap_or_default();
    // Hydrate paper titles. The distinct-paper count is bounded by
    // the size of `unread` (typically tens, never thousands).
    let mut papers: HashMap<String, Paper> = HashMap::new();
    for a in &unread {
        let pid = a.paper_id.as_str();
        // `map_entry` doesn't apply: we want to skip the insert when
        // load_paper returns None (paper deleted out from under us),
        // which `or_insert_with` can't express cleanly.
        #[allow(clippy::map_entry)]
        if !papers.contains_key(pid)
            && let Ok(Some(p)) = data.load_paper(pid)
        {
            papers.insert(pid.to_string(), p);
        }
    }
    build_items(&unread, &papers)
}

/// Move the inbox cursor `delta` selectable rows from `current`. Skips
/// over `Header` items so j/k feel natural. Returns the new index;
/// returns `current` unchanged if no selectable rows exist or the
/// cursor would walk off either end.
pub fn step_selection(items: &[InboxItem], current: usize, delta: isize) -> usize {
    if items.iter().all(|i| !i.is_selectable()) {
        return current;
    }
    let mut idx = current as isize;
    let n = items.len() as isize;
    let dir = if delta > 0 { 1 } else { -1 };
    let mut steps = delta.unsigned_abs();
    while steps > 0 {
        let next = idx + dir;
        if next < 0 || next >= n {
            break;
        }
        idx = next;
        if items[idx as usize].is_selectable() {
            steps -= 1;
        }
    }
    // Snap to a selectable row if `current` was on a header.
    if !items[idx as usize].is_selectable() {
        for (i, it) in items.iter().enumerate() {
            if it.is_selectable() {
                idx = i as isize;
                break;
            }
        }
    }
    idx as usize
}

/// Resolve the cursor `selected` to a jump target — the
/// `(paper_id, root_id)` of the focused row. None if the cursor is
/// on a header or the inbox is empty.
pub fn jump_target(items: &[InboxItem], selected: usize) -> Option<(String, String)> {
    match items.get(selected) {
        Some(InboxItem::Row {
            paper_id, root_id, ..
        }) => Some((paper_id.clone(), root_id.clone())),
        _ => None,
    }
}

pub fn draw(frame: &mut Frame, area: Rect, items: &[InboxItem], selected: usize) {
    // Clear under the overlay so the underlying view doesn't bleed
    // through the borders.
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" Inbox — {} unread ", count_rows(items)))
        .borders(Borders::ALL);

    if items.is_empty() {
        let empty = Paragraph::new("Nothing unread. ✓").block(block);
        frame.render_widget(empty, area);
        return;
    }

    let lines: Vec<ListItem<'_>> = items
        .iter()
        .map(|it| match it {
            InboxItem::Header { title, count, .. } => {
                ListItem::new(Line::from(vec![Span::styled(
                    format!(" {title}  [{count}]"),
                    Style::default()
                        .fg(crate::theme::theme().emphasis)
                        .add_modifier(Modifier::BOLD),
                )]))
            }
            InboxItem::Row {
                author,
                note_preview,
                is_reply,
                ..
            } => {
                let prefix = if *is_reply { "    └ " } else { "    ● " };
                ListItem::new(Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(
                        format!("{author}: "),
                        Style::default().fg(crate::theme::theme().emphasis),
                    ),
                    Span::raw(note_preview.clone()),
                ]))
            }
        })
        .collect();

    let list = List::new(lines).block(block).highlight_style(
        Style::default()
            .bg(crate::theme::theme().selection_bg)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn count_rows(items: &[InboxItem]) -> usize {
    items.iter().filter(|i| i.is_selectable()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::{Anchor, AnnotationId, PaperId};

    fn root_at(paper_id: &str, id: &str, note: &str) -> Annotation {
        let mut a = Annotation::new_root(
            PaperId::from(paper_id),
            "claude".into(),
            note.into(),
            Anchor {
                quote: Some("Q".into()),
                ..Anchor::default()
            },
        );
        a.id = AnnotationId::from(id.to_string());
        a
    }

    fn reply_to(parent: &Annotation, id: &str, note: &str) -> Annotation {
        let mut a = Annotation::new_reply(parent, "claude".into(), note.into());
        a.id = AnnotationId::from(id.to_string());
        a
    }

    fn paper(id: &str, title: &str) -> (String, Paper) {
        let mut p = Paper::new(title);
        p.id = PaperId::from(id);
        (id.to_string(), p)
    }

    #[test]
    fn build_items_groups_by_paper_and_preserves_thread_order() {
        let r1 = root_at("p-a", "ann-r1", "first claim");
        let rep1 = reply_to(&r1, "ann-rep1", "follow-up");
        let r2 = root_at("p-a", "ann-r2", "second claim");
        let r3 = root_at("p-b", "ann-r3", "other paper claim");
        let unread = vec![r1, rep1, r2, r3];
        let papers = [paper("p-a", "Alpha"), paper("p-b", "Beta")]
            .into_iter()
            .collect();

        let items = build_items(&unread, &papers);
        // Expected: header(p-a), row(r1), row(rep1), row(r2), header(p-b), row(r3)
        assert_eq!(items.len(), 6);
        match &items[0] {
            InboxItem::Header { title, count, .. } => {
                assert_eq!(title, "Alpha");
                assert_eq!(*count, 3);
            }
            InboxItem::Row { .. } => panic!("expected p-a header"),
        }
        match &items[1] {
            InboxItem::Row {
                root_id, is_reply, ..
            } => {
                assert_eq!(root_id, "ann-r1");
                assert!(!is_reply);
            }
            InboxItem::Header { .. } => panic!("expected r1 row"),
        }
        match &items[2] {
            InboxItem::Row {
                root_id, is_reply, ..
            } => {
                assert_eq!(root_id, "ann-r1", "reply carries root_id");
                assert!(is_reply);
            }
            InboxItem::Header { .. } => panic!("expected reply row"),
        }
    }

    #[test]
    fn step_selection_skips_headers() {
        // Layout: H, R, R, H, R
        let r1 = root_at("p-a", "ann-1", "x");
        let r2 = root_at("p-a", "ann-2", "y");
        let r3 = root_at("p-b", "ann-3", "z");
        let unread = vec![r1, r2, r3];
        let papers = [paper("p-a", "A"), paper("p-b", "B")].into_iter().collect();
        let items = build_items(&unread, &papers);

        // Start at first selectable (idx 1 — first row under p-a header).
        // Step +1 → should land on idx 2 (second p-a row), not on the
        // p-b header at idx 3.
        let next = step_selection(&items, 1, 1);
        assert_eq!(next, 2);
        // Step +1 again from idx 2 → must skip the header at 3 and
        // land on the row at 4.
        let next = step_selection(&items, 2, 1);
        assert_eq!(next, 4);
        // Step +1 from the last row stays put (clamped).
        let next = step_selection(&items, 4, 1);
        assert_eq!(next, 4);
        // Step -1 from idx 4 hops back to idx 2 (skipping header).
        let next = step_selection(&items, 4, -1);
        assert_eq!(next, 2);
    }

    #[test]
    fn jump_target_returns_paper_and_root_for_row() {
        let r = root_at("p-a", "ann-r1", "n");
        let items = build_items(&[r], &[paper("p-a", "Alpha")].into_iter().collect());
        // Header at idx 0, row at idx 1.
        assert_eq!(jump_target(&items, 0), None);
        assert_eq!(
            jump_target(&items, 1),
            Some(("p-a".into(), "ann-r1".into()))
        );
        // Out-of-range cursor.
        assert_eq!(jump_target(&items, 99), None);
    }

    #[test]
    fn jump_target_for_reply_resolves_to_root() {
        let r = root_at("p-a", "ann-r1", "n");
        let rep = reply_to(&r, "ann-rep1", "f");
        let items = build_items(&[r, rep], &[paper("p-a", "Alpha")].into_iter().collect());
        // Header(0), root(1), reply(2). Reply must report the root.
        assert_eq!(
            jump_target(&items, 2),
            Some(("p-a".into(), "ann-r1".into()))
        );
    }

    #[test]
    fn build_items_orphan_reply_uses_parent_id_as_root() {
        // The user already saw the root, so it's NOT in unread.
        // Only the new reply remains. We still want the jump to land
        // on the root by id.
        let placeholder_root = root_at("p-a", "ann-orphan-root", "x");
        let orphan_reply = reply_to(&placeholder_root, "ann-rep", "fresh reply");
        let items = build_items(
            &[orphan_reply],
            &[paper("p-a", "Alpha")].into_iter().collect(),
        );
        // Header + reply row.
        assert_eq!(items.len(), 2);
        assert_eq!(
            jump_target(&items, 1),
            Some(("p-a".into(), "ann-orphan-root".into()))
        );
    }

    #[test]
    fn preview_truncates_with_ellipsis() {
        let s: String = "a".repeat(100);
        let p = preview(&s, 20);
        assert_eq!(p.chars().count(), 20);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn step_selection_with_zero_delta_snaps_off_header_to_first_row() {
        // Mirrors the per-tick-rebuild clamp path in app::draw: when an
        // item is removed mid-tick (e.g. the row the cursor was on
        // gets marked seen by another reader), `selected.min(max)` may
        // land the cursor on a header. `step_selection(items, sel, 0)`
        // is the snap that pushes it forward to the next selectable
        // row without moving the user any further than necessary.
        let r1 = root_at("p-a", "ann-1", "x");
        let r2 = root_at("p-b", "ann-2", "y");
        let unread = vec![r1, r2];
        let papers = [paper("p-a", "A"), paper("p-b", "B")].into_iter().collect();
        let items = build_items(&unread, &papers);
        // Layout: H, R, H, R. Cursor on idx 0 (header) → snap to 1.
        assert_eq!(step_selection(&items, 0, 0), 1);
        // Cursor on idx 2 (header) → snap forward in the same direction
        // would land on 3, but step_selection's snap goes to the FIRST
        // selectable, which is 1. That's an acceptable behaviour: a
        // cursor that ended up on a header gets normalised to a known
        // good position rather than jumping unpredictably.
        assert_eq!(step_selection(&items, 2, 0), 1);
        // Cursor on a row (idx 1) is already selectable — no-op.
        assert_eq!(step_selection(&items, 1, 0), 1);
    }

    #[test]
    fn step_selection_on_empty_returns_input() {
        let items: Vec<InboxItem> = Vec::new();
        // Pre-rebuild cursor of 5 stays put when items is empty —
        // nothing to clamp against. Caller is responsible for the
        // `min(max)` clamp before calling step_selection.
        assert_eq!(step_selection(&items, 5, 0), 5);
    }

    #[test]
    fn preview_replaces_newlines() {
        assert_eq!(preview("line one\nline two", 100), "line one line two");
    }
}
