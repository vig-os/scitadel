use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
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
    let (icon, color) = match &task.status {
        TaskStatus::Queued => ("…", Color::DarkGray),
        TaskStatus::Running => ("◐", Color::Yellow),
        TaskStatus::Done { .. } => ("✓", Color::Green),
        TaskStatus::Failed(_) => ("✗", Color::Red),
    };

    let (title, ref_id) = match &task.kind {
        TaskKind::Download { title, ref_id } => (truncate(title, 55), ref_id.as_str()),
    };

    let status_tail = status_tail_text(&task.status, show_institutional_hint);

    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(title),
        Span::raw("  "),
        Span::styled(format!("[{ref_id}] "), Style::default().fg(Color::Blue)),
        Span::styled(status_tail, Style::default().fg(Color::DarkGray)),
    ]))
}

fn status_tail_text(status: &TaskStatus, show_institutional_hint: bool) -> String {
    match status {
        TaskStatus::Queued => "queued".to_string(),
        TaskStatus::Running => "downloading…".to_string(),
        TaskStatus::Done {
            format,
            access,
            path,
            publisher_url,
        } => {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("saved");
            // On paywall, tell the user where to get the live page — an
            // institutional IP range will often grant access that the
            // headless fetcher can't.
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

    #[test]
    fn paywall_with_hint_includes_url() {
        let status = TaskStatus::Done {
            path: PathBuf::from("foo.html"),
            format: DownloadFormat::Html,
            access: AccessStatus::Paywall,
            publisher_url: Some("https://doi.org/10.1234/x".into()),
        };
        let text = status_tail_text(&status, true);
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
        let text = status_tail_text(&status, false);
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
        let text = status_tail_text(&status, true);
        assert!(!text.contains("institutional IP"));
    }
}
