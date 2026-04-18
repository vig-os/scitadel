use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::data::DataStore;

pub fn draw(frame: &mut Frame, area: Rect, data: &DataStore, paper_id: &str, scroll: u16) {
    let Ok(Some(paper)) = data.load_paper(paper_id) else {
        let block = Block::default()
            .title(" Paper Detail ")
            .borders(Borders::ALL);
        let msg = Paragraph::new("Paper not found.").block(block);
        frame.render_widget(msg, area);
        return;
    };

    let assessments = data
        .load_assessments_for_paper(paper_id, None)
        .unwrap_or_default();

    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'_>> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("Title: ", label_style),
        Span::raw(&paper.title),
    ]));
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled("Authors: ", label_style),
        Span::raw(if paper.authors.is_empty() {
            "Unknown".to_string()
        } else {
            paper.authors.join(", ")
        }),
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

    if let Some(ref doi) = paper.doi {
        lines.push(Line::from(vec![
            Span::styled("DOI: ", label_style),
            Span::raw(doi.as_str()),
        ]));
    }

    if let Some(ref url) = paper.url {
        lines.push(Line::from(vec![
            Span::styled("URL: ", label_style),
            Span::raw(url.as_str()),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("ID: ", label_style),
        Span::raw(paper.id.as_str()),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("Abstract:", label_style)));

    if paper.r#abstract.is_empty() {
        lines.push(Line::from("  (no abstract available)"));
    } else {
        for line in paper.r#abstract.lines() {
            lines.push(Line::from(format!("  {line}")));
        }
    }

    if !assessments.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Assessments:", label_style)));
        for a in &assessments {
            lines.push(Line::from(format!(
                "  Score: {:.2}  Assessor: {}  Date: {}",
                a.score,
                if a.assessor.is_empty() {
                    "—"
                } else {
                    &a.assessor
                },
                a.created_at.format("%Y-%m-%d %H:%M"),
            )));
            if !a.reasoning.is_empty() {
                for rline in a.reasoning.lines() {
                    lines.push(Line::from(format!("    {rline}")));
                }
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Paper Detail ")
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(paragraph, area);
}
