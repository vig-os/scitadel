//! Question Dashboard — ranked scored papers + citation shortlist (#133).
//!
//! Split pane: left 40% shows the papers scored against the question,
//! ranked by score DESC; right 60% shows the focused paper's rationale,
//! abstract, and annotation count. `c` toggles shortlist membership
//! (marked with `●` prefix); Esc/q exits back to the Questions tab.

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};

use scitadel_core::models::{Assessment, Paper};

use crate::data::DataStore;
use crate::views::util::truncate;

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    data: &DataStore,
    question_id: &str,
    reader: &str,
    selected: usize,
) {
    let rows = data
        .load_question_dashboard(question_id)
        .unwrap_or_default();
    let shortlist = data
        .load_shortlist_set(question_id, reader)
        .unwrap_or_default();
    let question_text = data
        .load_question(question_id)
        .ok()
        .flatten()
        .map_or_else(|| "(unknown question)".to_string(), |q| q.text);

    if rows.is_empty() {
        let block = Block::default()
            .title(format!(" Dashboard — {question_text} "))
            .borders(Borders::ALL);
        let msg = Paragraph::new(
            "No scored papers yet for this question.\n\n\
             Score papers via MCP: `prepare_batch_assessments` → LLM → `save_assessment`,\n\
             then return here.",
        )
        .block(block)
        .wrap(Wrap { trim: false });
        frame.render_widget(msg, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    draw_list(
        frame,
        chunks[0],
        &rows,
        &shortlist,
        selected,
        &question_text,
    );
    draw_detail(frame, chunks[1], rows.get(selected));
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    rows: &[(Paper, Option<Assessment>)],
    shortlist: &HashSet<String>,
    selected: usize,
    question_text: &str,
) {
    let shortlist_count = shortlist.len();
    let title = format!(
        " Dashboard — {} · {} scored · {} shortlisted ",
        truncate(question_text, 40),
        rows.len(),
        shortlist_count
    );

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from("Score"),
        Cell::from("Title"),
        Cell::from("Year"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let table_rows: Vec<Row<'_>> = rows
        .iter()
        .map(|(paper, assessment)| {
            let marker = if shortlist.contains(paper.id.as_str()) {
                "●"
            } else {
                " "
            };
            let score = assessment
                .as_ref()
                .map_or("—".to_string(), |a| format!("{:.2}", a.score));
            let year = paper
                .year
                .map_or_else(|| "—".to_string(), |y| y.to_string());
            Row::new(vec![
                Cell::from(marker).style(Style::default().fg(Color::Yellow)),
                Cell::from(score),
                Cell::from(truncate(&paper.title, 40)),
                Cell::from(year),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Length(5),
        Constraint::Min(20),
        Constraint::Length(6),
    ];

    let table = Table::new(table_rows, widths)
        .header(header)
        .block(Block::default().title(title).borders(Borders::ALL))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_detail(frame: &mut Frame, area: Rect, focused: Option<&(Paper, Option<Assessment>)>) {
    let Some((paper, assessment)) = focused else {
        let empty = Paragraph::new("").block(Block::default().borders(Borders::ALL));
        frame.render_widget(empty, area);
        return;
    };

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            "Title: ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(paper.title.clone()),
    ]));
    if !paper.authors.is_empty() {
        let authors = paper
            .authors
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if paper.authors.len() > 4 {
            format!(" et al. ({} total)", paper.authors.len())
        } else {
            String::new()
        };
        lines.push(Line::from(vec![
            Span::styled("Authors: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{authors}{suffix}")),
        ]));
    }
    if let Some(year) = paper.year {
        lines.push(Line::from(vec![
            Span::styled("Year: ", Style::default().fg(Color::Yellow)),
            Span::raw(year.to_string()),
        ]));
    }
    if let Some(doi) = &paper.doi {
        lines.push(Line::from(vec![
            Span::styled("DOI: ", Style::default().fg(Color::Yellow)),
            Span::raw(doi.clone()),
        ]));
    }
    lines.push(Line::from(""));

    if let Some(a) = assessment {
        lines.push(Line::from(vec![
            Span::styled(
                "Score: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{:.2}", a.score)),
            Span::raw("  "),
            Span::styled(
                format!("({})", a.assessor),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
        if !a.reasoning.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Rationale:",
                Style::default().fg(Color::Yellow),
            )));
            for line in a.reasoning.lines() {
                lines.push(Line::from(line.to_string()));
            }
        }
    }

    if !paper.r#abstract.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Abstract:",
            Style::default().fg(Color::Yellow),
        )));
        for line in paper.r#abstract.lines() {
            lines.push(Line::from(line.to_string()));
        }
    }

    let para = Paragraph::new(lines)
        .block(Block::default().title(" Detail ").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);
}

/// How many ranked rows exist for a question. Used by the app handler
/// to clamp `selected` without re-running the full dashboard query.
pub fn row_count(data: &DataStore, question_id: &str) -> usize {
    data.load_question_dashboard(question_id)
        .map_or(0, |v| v.len())
}
