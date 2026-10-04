use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use scitadel_core::models::Paper;
use scitadel_db::sqlite::DownloadState;

use crate::data::DataStore;
use crate::views::util::{download_state_cell, truncate};

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    area: Rect,
    data: &DataStore,
    selected: usize,
    starred: &HashSet<String>,
    downloading: &HashSet<String>,
    papers_with_unread: &HashSet<String>,
) {
    let papers = data.load_papers(1000, 0).unwrap_or_default();
    let states = data.load_download_states(&papers);
    render_paper_table(
        frame,
        area,
        &papers,
        &states,
        selected,
        " Papers ",
        starred,
        downloading,
        papers_with_unread,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn draw_for_search(
    frame: &mut Frame,
    area: Rect,
    data: &DataStore,
    search_id: &str,
    selected: usize,
    starred: &HashSet<String>,
    downloading: &HashSet<String>,
    papers_with_unread: &HashSet<String>,
) {
    let papers = data.load_papers_for_search(search_id).unwrap_or_default();
    let states = data.load_download_states(&papers);
    let title = format!(
        " Papers for search {} ",
        search_id.chars().take(8).collect::<String>()
    );
    render_paper_table(
        frame,
        area,
        &papers,
        &states,
        selected,
        &title,
        starred,
        downloading,
        papers_with_unread,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_paper_table(
    frame: &mut Frame,
    area: Rect,
    papers: &[Paper],
    states: &std::collections::HashMap<String, DownloadState>,
    selected: usize,
    title: &str,
    starred: &HashSet<String>,
    downloading: &HashSet<String>,
    papers_with_unread: &HashSet<String>,
) {
    if papers.is_empty() {
        let block = Block::default()
            .title(title.to_string())
            .borders(Borders::ALL);
        let empty = Paragraph::new("No papers found.").block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from(""),
        Cell::from(""),
        Cell::from(""),
        Cell::from("Title"),
        Cell::from("Authors"),
        Cell::from("Year"),
    ])
    .style(
        Style::default()
            .fg(crate::theme::theme().emphasis)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row<'_>> = papers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let authors = format_authors(&p.authors);
            let year = p.year.map_or_else(|| "—".to_string(), |y| y.to_string());
            let star = if starred.contains(p.id.as_str()) {
                "★"
            } else {
                " "
            };
            let unread = if papers_with_unread.contains(p.id.as_str()) {
                "●"
            } else {
                " "
            };
            let (dl_symbol, dl_color) = download_state_cell(
                states.get(p.id.as_str()).copied(),
                downloading.contains(p.id.as_str()),
            );

            Row::new(vec![
                Cell::from((i + 1).to_string()),
                Cell::from(star).style(Style::default().fg(crate::theme::theme().emphasis)),
                Cell::from(unread).style(Style::default().fg(crate::theme::theme().emphasis)),
                Cell::from(dl_symbol).style(Style::default().fg(dl_color)),
                Cell::from(truncate(&p.title, 60)),
                Cell::from(truncate(&authors, 30)),
                Cell::from(year),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(5),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(30),
        Constraint::Length(32),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(title.to_string())
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

fn format_authors(authors: &[String]) -> String {
    match authors.len() {
        0 => "Unknown".to_string(),
        1 => authors[0].clone(),
        2 => format!("{}, {}", authors[0], authors[1]),
        _ => format!("{}, {} et al.", authors[0], authors[1]),
    }
}
