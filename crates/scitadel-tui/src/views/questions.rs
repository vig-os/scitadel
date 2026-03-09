use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use scitadel_core::models::ResearchQuestion;

use crate::data::DataStore;

/// Right-pane summary when "All" is selected at Questions level.
pub fn draw_summary(
    frame: &mut Frame,
    area: Rect,
    data: &DataStore,
    questions: &[ResearchQuestion],
) {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![Line::from(Span::styled(
        "All Research Questions",
        label_style,
    ))];
    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "Total questions: {}",
        questions.len()
    )));
    lines.push(Line::from(""));

    for q in questions {
        let term_count = data
            .load_terms(q.id.as_str())
            .map(|t| t.len())
            .unwrap_or(0);
        lines.push(Line::from(format!(
            "  {} — {} ({} terms)",
            q.id.short(),
            truncate(&q.text, 50),
            term_count,
        )));
    }

    let block = Block::default()
        .title(" Questions Overview ")
        .borders(Borders::ALL);
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Right-pane detail for a single question.
pub fn draw_detail(frame: &mut Frame, area: Rect, data: &DataStore, q: &ResearchQuestion) {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Question: ", label_style),
            Span::raw(&q.text),
        ]),
        Line::from(""),
    ];

    if !q.description.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Description: ", label_style),
            Span::raw(&q.description),
        ]));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![
        Span::styled("ID: ", label_style),
        Span::raw(q.id.as_str()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Created: ", label_style),
        Span::raw(q.created_at.format("%Y-%m-%d %H:%M").to_string()),
    ]));

    // Search terms
    let terms = data.load_terms(q.id.as_str()).unwrap_or_default();
    if !terms.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Search Terms:", label_style)));
        for t in &terms {
            if !t.query_string.is_empty() {
                lines.push(Line::from(format!("  Query: {}", t.query_string)));
            }
            if !t.terms.is_empty() {
                lines.push(Line::from(format!("  Terms: {}", t.terms.join(", "))));
            }
        }
    }

    // Search count
    let search_count = data
        .load_searches_for_question(q.id.as_str())
        .map(|s| s.len())
        .unwrap_or(0);
    lines.push(Line::from(""));
    lines.push(Line::from(format!("Searches linked: {search_count}")));

    let block = Block::default()
        .title(" Question Detail ")
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
