use std::io;
use std::path::Path;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::data::DataStore;
use crate::views::{detail, nav_tree, papers, questions, searches};
use crate::widgets::status_bar;

/// Current hierarchy level in the navigation tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavLevel {
    Questions,
    Searches,
    Papers,
}

/// Main application state.
pub struct App {
    pub nav_level: NavLevel,
    pub selected_question: Option<String>,
    pub selected_search: Option<String>,
    pub selected_paper: Option<String>,
    pub left_index: usize,
    pub right_scroll: u16,
    pub running: bool,
    pub data: DataStore,
}

impl App {
    fn new(data: DataStore) -> Self {
        Self {
            nav_level: NavLevel::Questions,
            selected_question: None,
            selected_search: None,
            selected_paper: None,
            left_index: 0,
            right_scroll: 0,
            running: true,
            data,
        }
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // Global quit
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            self.running = false;
            return;
        }
        if code == KeyCode::Char('q') {
            self.running = false;
            return;
        }

        match code {
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Enter => self.enter(),
            KeyCode::Esc => self.go_back(),
            KeyCode::Char('d') => self.right_scroll = self.right_scroll.saturating_add(10),
            KeyCode::Char('u') => self.right_scroll = self.right_scroll.saturating_sub(10),
            _ => {}
        }
    }

    fn move_down(&mut self) {
        let count = self.left_item_count();
        if count > 0 {
            self.left_index = (self.left_index + 1).min(count - 1);
        }
        self.right_scroll = 0;
        self.selected_paper = None;
    }

    fn move_up(&mut self) {
        self.left_index = self.left_index.saturating_sub(1);
        self.right_scroll = 0;
        self.selected_paper = None;
    }

    fn enter(&mut self) {
        match self.nav_level {
            NavLevel::Questions => {
                let questions = self.data.load_questions().unwrap_or_default();
                // Index 0 = "All", rest = questions offset by 1
                if self.left_index == 0 {
                    self.selected_question = None;
                } else if let Some(q) = questions.get(self.left_index - 1) {
                    self.selected_question = Some(q.id.as_str().to_string());
                }
                self.nav_level = NavLevel::Searches;
                self.left_index = 0;
                self.right_scroll = 0;
                self.selected_paper = None;
            }
            NavLevel::Searches => {
                let all_searches = self.current_searches();
                // Index 0 = "All", rest offset by 1
                if self.left_index == 0 {
                    self.selected_search = None;
                } else if let Some(s) = all_searches.get(self.left_index - 1) {
                    self.selected_search = Some(s.id.as_str().to_string());
                }
                self.nav_level = NavLevel::Papers;
                self.left_index = 0;
                self.right_scroll = 0;
                self.selected_paper = None;
            }
            NavLevel::Papers => {
                let papers = self.current_papers();
                if let Some(p) = papers.get(self.left_index) {
                    self.selected_paper = Some(p.id.as_str().to_string());
                    self.right_scroll = 0;
                }
            }
        }
    }

    fn go_back(&mut self) {
        if self.selected_paper.is_some() {
            self.selected_paper = None;
            self.right_scroll = 0;
            return;
        }
        match self.nav_level {
            NavLevel::Questions => {}
            NavLevel::Searches => {
                self.nav_level = NavLevel::Questions;
                self.left_index = 0;
                self.selected_question = None;
                self.right_scroll = 0;
            }
            NavLevel::Papers => {
                self.nav_level = NavLevel::Searches;
                self.left_index = 0;
                self.selected_search = None;
                self.right_scroll = 0;
            }
        }
    }

    fn left_item_count(&self) -> usize {
        match self.nav_level {
            NavLevel::Questions => {
                // +1 for "All"
                self.data.load_questions().map(|q| q.len()).unwrap_or(0) + 1
            }
            NavLevel::Searches => self.current_searches().len() + 1, // +1 for "All"
            NavLevel::Papers => self.current_papers().len(),
        }
    }

    fn current_searches(&self) -> Vec<scitadel_core::models::Search> {
        match &self.selected_question {
            Some(qid) => self.data.load_searches_for_question(qid).unwrap_or_default(),
            None => self.data.load_searches(100).unwrap_or_default(),
        }
    }

    fn current_papers(&self) -> Vec<scitadel_core::models::Paper> {
        match &self.selected_search {
            Some(sid) => self.data.load_papers_for_search(sid).unwrap_or_default(),
            None => self.data.load_papers(1000, 0).unwrap_or_default(),
        }
    }
}

/// Entry point: set up terminal, run event loop, restore terminal.
pub fn run(db_path: &Path) -> Result<()> {
    let data = DataStore::open(db_path)?;
    let mut app = App::new(data);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    while app.running {
        terminal.draw(|frame| draw(frame, app))?;

        if event::poll(std::time::Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == event::KeyEventKind::Press
        {
            app.handle_key(key.code, key.modifiers);
        }
    }
    Ok(())
}

fn draw(frame: &mut ratatui::Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header
            Constraint::Min(0),   // content (left + right)
            Constraint::Length(1), // status bar
        ])
        .split(frame.area());

    // Header
    draw_header(frame, chunks[0]);

    // Split content: left nav + right detail
    let content = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(28), // left pane
            Constraint::Min(0),    // right pane
        ])
        .split(chunks[1]);

    draw_left_pane(frame, content[0], app);
    draw_right_pane(frame, content[1], app);

    // Status bar
    let help = match app.nav_level {
        NavLevel::Questions => "j/k: nav | Enter: drill down | q: quit",
        NavLevel::Searches => "j/k: nav | Enter: drill down | Esc: back | q: quit",
        NavLevel::Papers => {
            if app.selected_paper.is_some() {
                "j/k: nav | d/u: scroll | Esc: back | q: quit"
            } else {
                "j/k: nav | Enter: select | Esc: back | q: quit"
            }
        }
    };
    status_bar::draw(frame, chunks[2], help);
}

fn draw_header(frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    let title_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let quit_style = Style::default().fg(Color::DarkGray);

    let line = Line::from(vec![
        Span::styled(" SCITADEL", title_style),
        Span::raw("  "),
        Span::styled("[q]uit", quit_style),
    ]);

    let header = Paragraph::new(line).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(header, area);
}

fn draw_left_pane(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
    match app.nav_level {
        NavLevel::Questions => {
            let questions = app.data.load_questions().unwrap_or_default();
            nav_tree::draw_questions(frame, area, &questions, app.left_index);
        }
        NavLevel::Searches => {
            let search_list = app.current_searches();
            let breadcrumb = match &app.selected_question {
                Some(qid) => format!("Q:{}", &qid[..qid.len().min(8)]),
                None => "All Questions".to_string(),
            };
            nav_tree::draw_searches(frame, area, &search_list, app.left_index, &breadcrumb);
        }
        NavLevel::Papers => {
            let paper_list = app.current_papers();
            let breadcrumb = match &app.selected_search {
                Some(sid) => format!("S:{}", &sid[..sid.len().min(8)]),
                None => "All Searches".to_string(),
            };
            let scores = paper_list
                .iter()
                .map(|p| {
                    app.selected_question.as_ref().and_then(|qid| {
                        app.data
                            .load_assessments_for_paper(p.id.as_str(), Some(qid))
                            .ok()
                            .and_then(|a| a.first().map(|a| a.score))
                    })
                })
                .collect::<Vec<_>>();
            nav_tree::draw_papers(frame, area, &paper_list, &scores, app.left_index, &breadcrumb);
        }
    }
}

fn draw_right_pane(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, app: &mut App) {
    // If a paper is selected, show full detail
    if let Some(ref paper_id) = app.selected_paper {
        let question_id = app.selected_question.as_deref();
        detail::draw(frame, area, &app.data, paper_id, app.right_scroll, question_id);
        return;
    }

    match app.nav_level {
        NavLevel::Questions => {
            let qs = app.data.load_questions().unwrap_or_default();
            // Index 0 = "All", offset by 1 for actual questions
            if app.left_index == 0 {
                questions::draw_summary(frame, area, &app.data, &qs);
            } else if let Some(q) = qs.get(app.left_index - 1) {
                questions::draw_detail(frame, area, &app.data, q);
            }
        }
        NavLevel::Searches => {
            let ss = app.current_searches();
            if app.left_index == 0 {
                searches::draw_summary(frame, area, &ss);
            } else if let Some(s) = ss.get(app.left_index - 1) {
                searches::draw_detail(frame, area, s);
            }
        }
        NavLevel::Papers => {
            let ps = app.current_papers();
            if let Some(p) = ps.get(app.left_index) {
                papers::draw_preview(frame, area, &app.data, p, app.selected_question.as_deref());
            } else {
                let block = Block::default()
                    .title(" Papers ")
                    .borders(Borders::ALL);
                let empty = Paragraph::new("No papers found.").block(block);
                frame.render_widget(empty, area);
            }
        }
    }
}
