use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem};
use ratatui::Frame;

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

pub fn draw(frame: &mut Frame, area: Rect, tasks: &[Task]) {
    let items: Vec<ListItem<'_>> = tasks.iter().map(render_row).collect();
    let list = List::new(items)
        .block(Block::default().title(" Tasks ").borders(Borders::ALL));
    frame.render_widget(list, area);
}

fn render_row(task: &Task) -> ListItem<'_> {
    let (icon, color) = match &task.status {
        TaskStatus::Queued => ("…", Color::DarkGray),
        TaskStatus::Running => ("◐", Color::Yellow),
        TaskStatus::Done { .. } => ("✓", Color::Green),
        TaskStatus::Failed(_) => ("✗", Color::Red),
    };

    let (title, ref_id) = match &task.kind {
        TaskKind::Download { title, ref_id } => (truncate(title, 55), ref_id.as_str()),
    };

    let status_tail = match &task.status {
        TaskStatus::Queued => "queued".to_string(),
        TaskStatus::Running => "downloading…".to_string(),
        TaskStatus::Done {
            format,
            access,
            path,
        } => {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("saved");
            format!("{format} · {access} · {name}")
        }
        TaskStatus::Failed(e) => format!("failed: {}", truncate(e, 60)),
    };

    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(title),
        Span::raw("  "),
        Span::styled(
            format!("[{ref_id}] "),
            Style::default().fg(Color::Blue),
        ),
        Span::styled(status_tail, Style::default().fg(Color::DarkGray)),
    ]))
}
