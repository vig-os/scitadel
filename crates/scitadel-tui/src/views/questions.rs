use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::data::DataStore;
use crate::views::util::truncate;

pub fn draw(frame: &mut Frame, area: Rect, data: &DataStore, selected: usize) {
    let questions = data.load_questions().unwrap_or_default();

    if questions.is_empty() {
        let block = Block::default()
            .title(" Research Questions ")
            .borders(Borders::ALL);
        let empty =
            Paragraph::new("No research questions yet. Run `scitadel question create` to add one.")
                .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("Date"),
        Cell::from("Text"),
        Cell::from("# Terms"),
    ])
    .style(
        Style::default()
            .fg(crate::theme::theme().emphasis)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row<'_>> = questions
        .iter()
        .map(|q| {
            let term_count = data.load_terms(q.id.as_str()).map_or(0, |t| t.len());

            Row::new(vec![
                Cell::from(q.id.short().to_string()),
                Cell::from(q.created_at.format("%Y-%m-%d %H:%M").to_string()),
                Cell::from(truncate(&q.text, 60)),
                Cell::from(term_count.to_string()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Length(18),
        Constraint::Min(20),
        Constraint::Length(9),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" Research Questions ")
                .borders(Borders::ALL),
        )
        .row_highlight_style(
            Style::default()
                .bg(crate::theme::theme().selection_bg)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}
