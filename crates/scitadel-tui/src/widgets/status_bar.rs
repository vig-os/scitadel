use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub fn draw(frame: &mut Frame, area: Rect, help_text: &str, offline: bool, unread_count: i64) {
    let bar = Paragraph::new(Line::from(build_spans(help_text, offline, unread_count)));
    frame.render_widget(bar, area);
}

/// Build the status-bar spans without rendering. Factored out so the
/// composition (which badges show, in which order) is unit-testable
/// without a Frame.
fn build_spans(help_text: &str, offline: bool, unread_count: i64) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(4);
    if offline {
        spans.push(Span::styled(
            "[OFFLINE] ",
            Style::default()
                .fg(crate::theme::theme().emphasis)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if unread_count > 0 {
        spans.push(Span::styled(
            format!("[{unread_count} new] "),
            Style::default()
                .fg(crate::theme::theme().emphasis)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        help_text.to_string(),
        Style::default().fg(crate::theme::theme().muted),
    ));
    spans
}

#[cfg(test)]
mod tests {
    use super::build_spans;

    fn texts(spans: &[ratatui::text::Span<'_>]) -> Vec<String> {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn no_badges_when_quiet() {
        let spans = build_spans("[q] quit", false, 0);
        assert_eq!(texts(&spans), vec!["[q] quit".to_string()]);
    }

    #[test]
    fn offline_badge_renders_before_help() {
        let spans = build_spans("[q] quit", true, 0);
        assert_eq!(
            texts(&spans),
            vec!["[OFFLINE] ".to_string(), "[q] quit".into()]
        );
    }

    #[test]
    fn unread_badge_renders_when_positive() {
        let spans = build_spans("[q] quit", false, 3);
        assert_eq!(
            texts(&spans),
            vec!["[3 new] ".to_string(), "[q] quit".into()]
        );
    }

    #[test]
    fn offline_then_unread_then_help_in_that_order() {
        let spans = build_spans("[q] quit", true, 7);
        assert_eq!(
            texts(&spans),
            vec![
                "[OFFLINE] ".to_string(),
                "[7 new] ".into(),
                "[q] quit".into(),
            ]
        );
    }

    #[test]
    fn negative_or_zero_unread_is_hidden() {
        // count_unread can't actually return a negative, but the type
        // is i64 so be defensive — `[-1 new]` would be ugly.
        let spans = build_spans("[q] quit", false, -5);
        assert_eq!(texts(&spans), vec!["[q] quit".to_string()]);
    }
}
