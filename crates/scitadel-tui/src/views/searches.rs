use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use scitadel_core::models::{Search, SourceStatus};

/// Right-pane summary when "All" is selected at Searches level.
pub fn draw_summary(frame: &mut Frame, area: Rect, searches: &[Search]) {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![Line::from(Span::styled("All Searches", label_style))];
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Total searches: {}", searches.len())));
    lines.push(Line::from(""));

    for s in searches {
        let sources_ok = s
            .source_outcomes
            .iter()
            .filter(|o| o.status == SourceStatus::Success)
            .count();
        lines.push(Line::from(format!(
            "  {} — {} ({} papers, {}/{} sources)",
            s.id.short(),
            truncate(&s.query, 40),
            s.total_papers,
            sources_ok,
            s.source_outcomes.len(),
        )));
    }

    let block = Block::default()
        .title(" Searches Overview ")
        .borders(Borders::ALL);
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Right-pane detail for a single search.
pub fn draw_detail(frame: &mut Frame, area: Rect, s: &Search) {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let sources_ok = s
        .source_outcomes
        .iter()
        .filter(|o| o.status == SourceStatus::Success)
        .count();

    let lines = vec![
        Line::from(vec![
            Span::styled("Query: ", label_style),
            Span::raw(&s.query),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("ID: ", label_style),
            Span::raw(s.id.as_str()),
        ]),
        Line::from(vec![
            Span::styled("Date: ", label_style),
            Span::raw(s.created_at.format("%Y-%m-%d %H:%M").to_string()),
        ]),
        Line::from(vec![
            Span::styled("Sources: ", label_style),
            Span::raw(if s.sources.is_empty() {
                "—".to_string()
            } else {
                s.sources.join(", ")
            }),
        ]),
        Line::from(vec![
            Span::styled("Source Results: ", label_style),
            Span::raw(format!(
                "{}/{} succeeded",
                sources_ok,
                s.source_outcomes.len()
            )),
        ]),
        Line::from(vec![
            Span::styled("Papers Found: ", label_style),
            Span::raw(s.total_papers.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Candidates: ", label_style),
            Span::raw(s.total_candidates.to_string()),
        ]),
    ];

    let block = Block::default()
        .title(" Search Detail ")
        .borders(Borders::ALL);
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
