use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem};
use scitadel_adapters::download::AccessStatus;

use crate::tasks::{Task, TaskKind, TaskStatus};
use crate::views::util::truncate;

/// Height the task panel needs: 0 when no tasks, otherwise rows + 2 for borders,
/// clamped so it doesn't eat the whole screen.
pub fn panel_height(tasks: &[Task]) -> u16 {
    if tasks.is_empty() {
        0
    } else {
        (tasks.len() as u16 + 2).min(8)
    }
}

pub fn draw(frame: &mut Frame, area: Rect, tasks: &[Task], show_institutional_hint: bool) {
    let items: Vec<ListItem<'_>> = tasks
        .iter()
        .map(|t| render_row(t, show_institutional_hint))
        .collect();
    let list = List::new(items).block(Block::default().title(" Tasks ").borders(Borders::ALL));
    frame.render_widget(list, area);
}

fn render_row(task: &Task, show_institutional_hint: bool) -> ListItem<'_> {
    let t = crate::theme::theme();
    let (icon, color) = match &task.status {
        TaskStatus::Queued => ("…", t.muted),
        TaskStatus::Running => ("◐", t.warning),
        TaskStatus::Done { .. } => ("✓", t.success),
        TaskStatus::Failed(_) => ("✗", t.danger),
    };

    let (title, ref_id, kind_label) = match &task.kind {
        TaskKind::Download { title, ref_id, .. } => (truncate(title, 55), ref_id.as_str(), ""),
        TaskKind::OpenExternal { title, ref_id, .. } => {
            (truncate(title, 55), ref_id.as_str(), "open: ")
        }
    };

    let status_tail = status_tail_text(&task.status, show_institutional_hint, &task.kind);

    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(kind_label, Style::default().fg(crate::theme::theme().muted)),
        Span::raw(title),
        Span::raw("  "),
        Span::styled(
            format!("[{ref_id}] "),
            Style::default().fg(crate::theme::theme().info),
        ),
        Span::styled(
            status_tail,
            Style::default().fg(crate::theme::theme().muted),
        ),
    ]))
}

fn status_tail_text(status: &TaskStatus, show_institutional_hint: bool, kind: &TaskKind) -> String {
    match status {
        TaskStatus::Queued => "queued".to_string(),
        TaskStatus::Running => "downloading…".to_string(),
        TaskStatus::Done {
            format,
            access,
            path,
            publisher_url,
        } => {
            // OpenExternal "Done" just means "viewer was launched" — the
            // download metadata fields are placeholders, so don't print
            // them as if they describe a real download.
            if matches!(kind, TaskKind::OpenExternal { .. }) {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
                return format!("opened {name}");
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("saved");
            if *access == AccessStatus::Paywall
                && show_institutional_hint
                && let Some(url) = publisher_url
            {
                format!(
                    "{format} · {access} · try {url} (institutional IP may grant access) · {name}"
                )
            } else {
                format!("{format} · {access} · {name}")
            }
        }
        TaskStatus::Failed(e) => format!("failed: {}", truncate(e, 60)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use scitadel_adapters::download::DownloadFormat;

    use super::*;

    fn dl_kind() -> TaskKind {
        TaskKind::Download {
            paper_id: "p".into(),
            ref_id: "ref".into(),
            title: "title".into(),
        }
    }

    #[test]
    fn open_external_done_shows_opened_filename() {
        let status = TaskStatus::Done {
            path: PathBuf::from("/tmp/paper.pdf"),
            format: DownloadFormat::Pdf,
            access: AccessStatus::FullText,
            publisher_url: None,
        };
        let kind = TaskKind::OpenExternal {
            paper_id: "p".into(),
            ref_id: "ref".into(),
            title: "title".into(),
        };
        let text = status_tail_text(&status, true, &kind);
        assert_eq!(text, "opened paper.pdf");
    }

    #[test]
    fn paywall_with_hint_includes_url() {
        let status = TaskStatus::Done {
            path: PathBuf::from("foo.html"),
            format: DownloadFormat::Html,
            access: AccessStatus::Paywall,
            publisher_url: Some("https://doi.org/10.1234/x".into()),
        };
        let text = status_tail_text(&status, true, &dl_kind());
        assert!(text.contains("https://doi.org/10.1234/x"));
        assert!(text.contains("institutional IP may grant access"));
    }

    #[test]
    fn paywall_without_hint_omits_url() {
        let status = TaskStatus::Done {
            path: PathBuf::from("foo.html"),
            format: DownloadFormat::Html,
            access: AccessStatus::Paywall,
            publisher_url: Some("https://doi.org/10.1234/x".into()),
        };
        let text = status_tail_text(&status, false, &dl_kind());
        assert!(!text.contains("https://doi.org/10.1234/x"));
        assert!(text.contains("paywall"));
    }

    #[test]
    fn full_text_never_shows_hint() {
        let status = TaskStatus::Done {
            path: PathBuf::from("foo.pdf"),
            format: DownloadFormat::Pdf,
            access: AccessStatus::FullText,
            publisher_url: Some("https://example.org".into()),
        };
        let text = status_tail_text(&status, true, &dl_kind());
        assert!(!text.contains("institutional IP"));
    }
}
