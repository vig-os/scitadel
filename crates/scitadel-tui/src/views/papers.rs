use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use scitadel_core::models::Paper;
use scitadel_db::sqlite::DownloadState;

use crate::data::DataStore;
use crate::views::util::{download_state_cell, format_authors, truncate};

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
                Cell::from(truncate(&p.title.rendered(), 60)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use scitadel_core::models::PaperId;
    use scitadel_core::ports::PaperRepository as _;
    use scitadel_core::untrusted::UntrustedText;

    /// The papers table is the *other* place a document's title reaches a
    /// screen, and it is the one a person looks at while deciding what to
    /// read. #287 closed the reader's title path; this closes the list's.
    ///
    /// The reader's payload is reused verbatim — a PDF `/Title` carrying
    /// colour, erase-display and an OSC 8 hyperlink — because a second, weaker
    /// fixture here would let the list be unsafe while the reader looks safe.
    const HOSTILE: &str = "Real Title\u{1b}[31m\u{1b}[2J\u{1b}]8;;https://attacker.example/\u{1b}\\click\u{1b}]8;;\u{1b}\\";

    #[test]
    fn a_hostile_document_title_is_neutralised_in_the_papers_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = DataStore::open(&dir.path().join("scitadel.db")).expect("open db");
        let (paper_repo, _, _, _, _) = data.db.repositories();

        let mut paper = Paper::new(HOSTILE);
        paper.id = PaperId::from("p-hostile");
        paper.authors = vec![UntrustedText::publisher_supplied("Attacker\u{1b}[2J, A.")];
        paper.title = UntrustedText::publisher_supplied(HOSTILE);
        paper_repo.save(&paper).expect("save paper");

        let mut terminal = Terminal::new(TestBackend::new(120, 8)).expect("terminal");
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    frame.area(),
                    &data,
                    0,
                    &HashSet::new(),
                    &HashSet::new(),
                    &HashSet::new(),
                );
            })
            .expect("draw");

        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        // Assert the *remnants*, not the ESC byte. ratatui drops the ESC itself
        // when it writes a string into the buffer — verified by probe — so
        // "no ESC reached the screen" is a claim about ratatui, not about this
        // code, and it would pass with `as_str()` at the call site. What
        // distinguishes `rendered()` from `as_str()` is what is left behind:
        // `[31m` and `[2J` rendered as visible garbage.
        for remnant in ["[31m", "[2J", "]8;;", "https://attacker.example"] {
            assert!(
                !screen.contains(remnant),
                "an escape sequence's remnant must not reach the screen: \
                 {remnant} in {screen:?}"
            );
        }
        assert!(
            screen.contains("Real Title") && screen.contains("Attacker"),
            "the words a person needs still render, neutralised rather than \
             dropped: {screen:?}"
        );
    }
}
