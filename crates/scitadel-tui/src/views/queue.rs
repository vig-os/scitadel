//! Queue tab — cross-search aggregator of starred papers (#48).
//!
//! A deliberately minimal view: one table of every paper the current
//! reader has starred, sorted most-recent-star-first. No `to_read`
//! flag (cut per the 0.6.0 researcher review — "will rot within a
//! week"); no filter toggles (cut per the maintenance review —
//! filter+selection-index state desync is an unforced error).
//!
//! The rendering reuses the Papers-tab table so a future 0.7.0 pass
//! can consolidate the two views behind a shared component; right now
//! the Queue's data source is `DataStore::load_starred_papers` and the
//! rendering is otherwise identical (same download-state column, same
//! star indicator, same Enter-to-open-detail flow).

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use scitadel_core::models::{DownloadStatus, Paper};

use crate::data::DataStore;
use crate::views::util::truncate;

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    data: &DataStore,
    reader: &str,
    selected: usize,
    downloading: &HashSet<String>,
) {
    let papers = data.load_starred_papers(reader).unwrap_or_default();
    let title = format!(" Queue — {} starred ", papers.len());

    if papers.is_empty() {
        let block = Block::default().title(title).borders(Borders::ALL);
        let empty = Paragraph::new(
            "No starred papers yet. Press `s` on the Papers tab to star one.\n\
             The Queue aggregates stars across every search and question.",
        )
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from(""),
        Cell::from("Title"),
        Cell::from("Authors"),
        Cell::from("Year"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row<'_>> = papers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let authors = format_authors(&p.authors);
            let year = p.year.map_or_else(|| "—".to_string(), |y| y.to_string());
            let (dl_symbol, dl_color) = download_cell(p, downloading);

            Row::new(vec![
                Cell::from((i + 1).to_string()),
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
        Constraint::Min(30),
        Constraint::Length(32),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
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

/// Mirror of the Papers-tab download-state column so the Queue feels
/// consistent. Copied-not-shared for now; a future refactor can lift
/// this into `views/util` if a third view ever needs it.
fn download_cell(
    paper: &Paper,
    downloading: &HashSet<String>,
) -> (&'static str, ratatui::style::Color) {
    if downloading.contains(paper.id.as_str()) {
        return ("↻", Color::Yellow);
    }
    match paper.download_status {
        Some(DownloadStatus::Downloaded) => ("✓", Color::Green),
        Some(DownloadStatus::Paywall) => ("⊘", Color::Yellow),
        Some(DownloadStatus::Failed) => ("✗", Color::Red),
        None => (" ", Color::DarkGray),
    }
}

fn format_authors(authors: &[String]) -> String {
    match authors.len() {
        0 => "Unknown".to_string(),
        1 => authors[0].clone(),
        2 => format!("{}, {}", authors[0], authors[1]),
        _ => format!("{}, {} et al.", authors[0], authors[1]),
    }
}
