use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Tabs;
use tokio::sync::mpsc;

use crate::data::DataStore;
use crate::tasks::{Task, TaskUpdate, spawn_download_paper};
use crate::views::{detail, papers, questions, searches, tasks as tasks_view};
use crate::widgets::status_bar;

/// Active tab in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Searches,
    Papers,
    Questions,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Searches, Self::Papers, Self::Questions];

    fn index(self) -> usize {
        match self {
            Self::Searches => 0,
            Self::Papers => 1,
            Self::Questions => 2,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Searches => Self::Papers,
            Self::Papers => Self::Questions,
            Self::Questions => Self::Searches,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Searches => Self::Questions,
            Self::Papers => Self::Searches,
            Self::Questions => Self::Papers,
        }
    }
}

/// Overlay view shown on top of the current tab.
#[derive(Debug, Clone)]
pub enum Overlay {
    PaperDetail { paper_id: String, scroll: u16 },
    SearchPapers { search_id: String, selected: usize },
}

/// Main application state.
pub struct App {
    pub tab: Tab,
    pub running: bool,
    pub data: DataStore,
    pub overlay: Option<Overlay>,

    pub search_selected: usize,
    pub paper_selected: usize,
    pub question_selected: usize,

    pub tasks: Vec<Task>,
    pub unpaywall_email: String,
    pub papers_dir: PathBuf,

    task_tx: mpsc::UnboundedSender<TaskUpdate>,
    task_rx: mpsc::UnboundedReceiver<TaskUpdate>,
}

impl App {
    fn new(data: DataStore, unpaywall_email: String, papers_dir: PathBuf) -> Self {
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        Self {
            tab: Tab::Searches,
            running: true,
            data,
            overlay: None,
            search_selected: 0,
            paper_selected: 0,
            question_selected: 0,
            tasks: Vec::new(),
            unpaywall_email,
            papers_dir,
            task_tx,
            task_rx,
        }
    }

    fn drain_task_updates(&mut self) {
        while let Ok(update) = self.task_rx.try_recv() {
            match update {
                TaskUpdate::New(task) => {
                    self.tasks.push(task);
                    if self.tasks.len() > 10 {
                        self.tasks.remove(0);
                    }
                }
                TaskUpdate::Status { id, status } => {
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                        t.status = status;
                    }
                }
            }
        }
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        if code == KeyCode::Char('q') && self.overlay.is_none() {
            self.running = false;
            return;
        }
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            self.running = false;
            return;
        }

        if self.overlay.is_some() {
            self.handle_overlay_key(code);
            return;
        }

        match code {
            KeyCode::Tab => self.tab = self.tab.next(),
            KeyCode::BackTab => self.tab = self.tab.prev(),
            _ => self.handle_tab_key(code),
        }
    }

    fn handle_overlay_key(&mut self, code: KeyCode) {
        match self.overlay {
            Some(Overlay::PaperDetail {
                ref paper_id,
                ref mut scroll,
            }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
                KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Char('d') => *scroll = scroll.saturating_add(10),
                KeyCode::Char('u') => *scroll = scroll.saturating_sub(10),
                KeyCode::Char('D') => {
                    let paper_id = paper_id.clone();
                    self.queue_download(&paper_id);
                }
                _ => {}
            },
            Some(Overlay::SearchPapers {
                ref search_id,
                ref mut selected,
            }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    let count = self
                        .data
                        .load_papers_for_search(search_id)
                        .map_or(0, |p| p.len());
                    if count > 0 {
                        *selected = (*selected + 1).min(count - 1);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Enter => {
                    let search_id_clone = search_id.clone();
                    let sel = *selected;
                    if let Ok(papers) = self.data.load_papers_for_search(&search_id_clone)
                        && let Some(paper) = papers.get(sel)
                    {
                        self.overlay = Some(Overlay::PaperDetail {
                            paper_id: paper.id.as_str().to_string(),
                            scroll: 0,
                        });
                    }
                }
                _ => {}
            },
            None => {}
        }
    }

    fn queue_download(&mut self, paper_id: &str) {
        let paper = match self.data.load_paper(paper_id) {
            Ok(Some(p)) => p,
            _ => return,
        };
        spawn_download_paper(
            self.task_tx.clone(),
            paper,
            self.unpaywall_email.clone(),
            self.papers_dir.clone(),
        );
    }

    fn handle_tab_key(&mut self, code: KeyCode) {
        match self.tab {
            Tab::Searches => {
                let count = self.data.load_searches(100).map_or(0, |s| s.len());
                match code {
                    KeyCode::Char('j') | KeyCode::Down if count > 0 => {
                        self.search_selected = (self.search_selected + 1).min(count - 1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.search_selected = self.search_selected.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if let Ok(searches) = self.data.load_searches(100)
                            && let Some(search) = searches.get(self.search_selected)
                        {
                            self.overlay = Some(Overlay::SearchPapers {
                                search_id: search.id.as_str().to_string(),
                                selected: 0,
                            });
                        }
                    }
                    _ => {}
                }
            }
            Tab::Papers => {
                let count = self.data.load_papers(1000, 0).map_or(0, |p| p.len());
                match code {
                    KeyCode::Char('j') | KeyCode::Down if count > 0 => {
                        self.paper_selected = (self.paper_selected + 1).min(count - 1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.paper_selected = self.paper_selected.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if let Ok(papers) = self.data.load_papers(1000, 0)
                            && let Some(paper) = papers.get(self.paper_selected)
                        {
                            self.overlay = Some(Overlay::PaperDetail {
                                paper_id: paper.id.as_str().to_string(),
                                scroll: 0,
                            });
                        }
                    }
                    _ => {}
                }
            }
            Tab::Questions => {
                let count = self.data.load_questions().map_or(0, |q| q.len());
                match code {
                    KeyCode::Char('j') | KeyCode::Down if count > 0 => {
                        self.question_selected = (self.question_selected + 1).min(count - 1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.question_selected = self.question_selected.saturating_sub(1);
                    }
                    _ => {}
                }
            }
        }
    }
}

pub fn run(db_path: &Path, unpaywall_email: String, papers_dir: PathBuf) -> Result<()> {
    let data = DataStore::open(db_path)?;
    let mut app = App::new(data, unpaywall_email, papers_dir);

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
        app.drain_task_updates();
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
    let task_panel_height = tasks_view::panel_height(&app.tasks);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),                 // tabs
            Constraint::Min(0),                    // content
            Constraint::Length(task_panel_height), // task panel (0 when empty)
            Constraint::Length(1),                 // status bar
        ])
        .split(frame.area());

    let tab_titles: Vec<Line<'_>> = Tab::ALL
        .iter()
        .map(|t| {
            let label = match t {
                Tab::Searches => "Searches",
                Tab::Papers => "Papers",
                Tab::Questions => "Questions",
            };
            Line::from(Span::raw(label))
        })
        .collect();

    let tabs = Tabs::new(tab_titles)
        .select(app.tab.index())
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" | "));

    frame.render_widget(tabs, chunks[0]);

    match &app.overlay {
        Some(Overlay::PaperDetail { paper_id, scroll }) => {
            detail::draw(frame, chunks[1], &app.data, paper_id, *scroll);
        }
        Some(Overlay::SearchPapers {
            search_id,
            selected,
        }) => {
            papers::draw_for_search(frame, chunks[1], &app.data, search_id, *selected);
        }
        None => match app.tab {
            Tab::Searches => searches::draw(frame, chunks[1], &app.data, app.search_selected),
            Tab::Papers => papers::draw(frame, chunks[1], &app.data, app.paper_selected),
            Tab::Questions => {
                questions::draw(frame, chunks[1], &app.data, app.question_selected);
            }
        },
    }

    if task_panel_height > 0 {
        tasks_view::draw(frame, chunks[2], &app.tasks);
    }

    let help_text = match &app.overlay {
        Some(Overlay::PaperDetail { .. }) => "Esc/q: back | j/k: scroll | d/u: page | D: download",
        Some(Overlay::SearchPapers { .. }) => "Esc/q: back | j/k: navigate | Enter: open paper",
        None => "Tab/Shift-Tab: switch tabs | j/k: navigate | Enter: select | q: quit",
    };
    status_bar::draw(frame, chunks[3], help_text);
}
