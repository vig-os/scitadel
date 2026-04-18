use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

pub fn draw(frame: &mut Frame, area: Rect, help_text: &str) {
    let bar = Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(bar, area);
}
