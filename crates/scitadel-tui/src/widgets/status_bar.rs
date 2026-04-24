use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub fn draw(frame: &mut Frame, area: Rect, help_text: &str, offline: bool) {
    let mut spans = Vec::with_capacity(3);
    if offline {
        spans.push(Span::styled(
            "[OFFLINE] ",
            Style::default()
                .fg(crate::theme::theme().emphasis)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        help_text.to_string(),
        Style::default().fg(crate::theme::theme().muted),
    ));
    let bar = Paragraph::new(Line::from(spans));
    frame.render_widget(bar, area);
}
