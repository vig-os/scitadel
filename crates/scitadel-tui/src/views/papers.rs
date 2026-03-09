use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use scitadel_core::models::Paper;

use crate::data::DataStore;

/// Right-pane preview for a highlighted paper (not full detail, just key info).
pub fn draw_preview(
    frame: &mut Frame,
    area: Rect,
    data: &DataStore,
    paper: &Paper,
    question_id: Option<&str>,
) {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![Line::from(vec![
        Span::styled("Title: ", label_style),
        Span::raw(&paper.title),
    ])];
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled("Authors: ", label_style),
        Span::raw(format_authors(&paper.authors)),
    ]));

    if let Some(year) = paper.year {
        lines.push(Line::from(vec![
            Span::styled("Year: ", label_style),
            Span::raw(year.to_string()),
        ]));
    }

    if let Some(ref journal) = paper.journal {
        lines.push(Line::from(vec![
            Span::styled("Journal: ", label_style),
            Span::raw(journal.as_str()),
        ]));
    }

    // Score for selected question
    if let Some(qid) = question_id
        && let Ok(assessments) = data.load_assessments_for_paper(paper.id.as_str(), Some(qid))
        && let Some(a) = assessments.first()
    {
        lines.push(Line::from(vec![
            Span::styled("Score: ", label_style),
            Span::raw(format!("{:.2}", a.score)),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Abstract:", label_style)));
    if paper.r#abstract.is_empty() {
        lines.push(Line::from("  (no abstract available)"));
    } else {
        // Show truncated abstract for preview
        let abstract_preview: String = paper
            .r#abstract
            .chars()
            .take(500)
            .collect();
        for line in abstract_preview.lines() {
            lines.push(Line::from(format!("  {line}")));
        }
        if paper.r#abstract.len() > 500 {
            lines.push(Line::from("  ..."));
        }
    }

    let block = Block::default()
        .title(" Paper Preview ")
        .borders(Borders::ALL);
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn format_authors(authors: &[String]) -> String {
    match authors.len() {
        0 => "Unknown".to_string(),
        1 => authors[0].clone(),
        2 => format!("{}, {}", authors[0], authors[1]),
        _ => format!("{}, {} et al.", authors[0], authors[1]),
    }
}
