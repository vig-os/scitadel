use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};

use scitadel_core::models::SourceStatus;

use crate::data::DataStore;
use crate::views::util::truncate;

pub fn draw(frame: &mut Frame, area: Rect, data: &DataStore, selected: usize) {
    let searches = data.load_searches(100).unwrap_or_default();

    if searches.is_empty() {
        let block = Block::default().title(" Searches ").borders(Borders::ALL);
        let empty = ratatui::widgets::Paragraph::new(
            "No searches yet. Run `scitadel search` to get started.",
        )
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("Date"),
        Cell::from("Query"),
        Cell::from("Papers"),
        Cell::from("Sources"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row<'_>> = searches
        .iter()
        .map(|s| {
            let success = s
                .source_outcomes
                .iter()
                .filter(|o| o.status == SourceStatus::Success)
                .count();
            let total = s.source_outcomes.len();

            Row::new(vec![
                Cell::from(s.id.short().to_string()),
                Cell::from(s.created_at.format("%Y-%m-%d %H:%M").to_string()),
                Cell::from(truncate(&s.query, 50)),
                Cell::from(s.total_papers.to_string()),
                Cell::from(format!("{success}/{total}")),
            ])
        })
        .collect();

    let widths = [
        ratatui::layout::Constraint::Length(10),
        ratatui::layout::Constraint::Length(18),
        ratatui::layout::Constraint::Min(20),
        ratatui::layout::Constraint::Length(8),
        ratatui::layout::Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().title(" Searches ").borders(Borders::ALL))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}
