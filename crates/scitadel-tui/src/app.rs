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
use crate::views::annotation_prompt::{AnnotationPrompt, PromptCommit, PromptSubmission};
use crate::views::{annotation_prompt, detail, papers, questions, searches, tasks as tasks_view};
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
    PaperDetail {
        paper_id: String,
        scroll: u16,
        /// `None` = scroll mode; `Some(i)` = focused on annotation `i`
        /// in the paper's annotation list. Set by `Shift+J` and
        /// cleared by `Esc`. Required so e/d/r (#92) have a target.
        annotation_focus: Option<usize>,
        /// Active n/e/r/d prompt overlay (#92). Drawn on top of the
        /// detail view; eats keystrokes until submitted or cancelled.
        prompt: Option<AnnotationPrompt>,
        /// True = two-pane reader mode (#97); false = the existing
        /// single-pane metadata + annotation list. Toggled with `R`.
        /// Mutually exclusive with `annotation_focus`/`prompt`.
        reader: bool,
        /// Index into the paper's root annotations while the reader
        /// is open; cycles via `J` / `K` to hop between highlights.
        highlight_focus: Option<usize>,
    },
    SearchPapers {
        search_id: String,
        selected: usize,
    },
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
    pub show_institutional_hint: bool,
    pub reader: String,
    pub starred: std::collections::HashSet<String>,
    /// True when the startup network probe failed. Purely advisory — reads
    /// always work from SQLite; downloads still run (and can succeed via
    /// cache layers), but the status bar shows a visible OFFLINE badge.
    pub offline: bool,

    task_tx: mpsc::UnboundedSender<TaskUpdate>,
    task_rx: mpsc::UnboundedReceiver<TaskUpdate>,
    offline_rx: mpsc::UnboundedReceiver<bool>,
}

impl App {
    fn new(
        data: DataStore,
        unpaywall_email: String,
        papers_dir: PathBuf,
        show_institutional_hint: bool,
        reader: String,
    ) -> Self {
        let (task_tx, task_rx) = mpsc::unbounded_channel();
        let (offline_tx, offline_rx) = mpsc::unbounded_channel();
        let starred = data.load_starred_ids(&reader).unwrap_or_default();

        // Kick off a one-shot network probe. The result arrives on
        // `offline_rx` and we flip the status-bar badge on first draw.
        tokio::spawn(async move {
            let online = probe_network().await;
            let _ = offline_tx.send(!online);
        });

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
            show_institutional_hint,
            reader,
            starred,
            offline: false,
            task_tx,
            task_rx,
            offline_rx,
        }
    }

    fn drain_offline(&mut self) {
        while let Ok(value) = self.offline_rx.try_recv() {
            self.offline = value;
        }
    }

    /// Toggle the starred state for a paper and refresh the cached set.
    fn toggle_star(&mut self, paper_id: &str) {
        if let Ok(now_starred) = self.data.toggle_starred(paper_id, &self.reader) {
            if now_starred {
                self.starred.insert(paper_id.to_string());
            } else {
                self.starred.remove(paper_id);
            }
        }
    }

    fn drain_all_channels(&mut self) {
        self.drain_task_updates();
        self.drain_offline();
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
            Some(Overlay::PaperDetail { .. }) => self.handle_paper_detail_key(code),
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
                            annotation_focus: None,
                            prompt: None,
                            reader: false,
                            highlight_focus: None,
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

    /// Routes a key inside the PaperDetail overlay through four layers:
    /// (0) reader mode (#97) — its own R/Esc/J/K loop;
    /// (1) an active prompt (#92);
    /// (2) annotation-focus mode (#92);
    /// (3) plain scroll mode.
    /// Each layer eats its own keys and falls through otherwise.
    fn handle_paper_detail_key(&mut self, code: KeyCode) {
        let Some(Overlay::PaperDetail {
            paper_id,
            scroll,
            annotation_focus,
            prompt,
            reader,
            highlight_focus,
        }) = self.overlay.as_mut()
        else {
            return;
        };

        // Layer 0: reader mode owns the keystroke loop while it's open.
        if *reader {
            let count = crate::views::reader::highlight_count(&self.data, paper_id);
            match code {
                KeyCode::Esc | KeyCode::Char('q' | 'R') => {
                    *reader = false;
                    *highlight_focus = None;
                }
                KeyCode::Char('J') | KeyCode::Down if count > 0 => {
                    *highlight_focus = Some(highlight_focus.map_or(0, |f| (f + 1).min(count - 1)));
                }
                KeyCode::Char('K') | KeyCode::Up if count > 0 => {
                    *highlight_focus = Some(highlight_focus.map_or(0, |f| f.saturating_sub(1)));
                }
                KeyCode::Char('D') => {
                    let pid = paper_id.clone();
                    self.queue_download(&pid);
                }
                _ => {}
            }
            return;
        }

        // Layer 1: an active prompt eats every keystroke.
        if prompt.is_some() {
            let submission = step_prompt(prompt, code);
            if let Some(s) = submission {
                self.dispatch_submission(s);
            }
            return;
        }

        // Layer 2: annotation focus mode (e/d/r operate on a focused row).
        if let Some(focus) = annotation_focus {
            let pid = paper_id.clone();
            let annotations = self
                .data
                .load_annotations_for_paper(&pid)
                .unwrap_or_default();
            if annotations.is_empty() {
                *annotation_focus = None;
            } else {
                let count = annotations.len();
                match code {
                    KeyCode::Esc => *annotation_focus = None,
                    KeyCode::Char('j') | KeyCode::Down => {
                        *focus = (*focus + 1).min(count - 1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        *focus = focus.saturating_sub(1);
                    }
                    KeyCode::Char('e') => {
                        let target = &annotations[*focus];
                        *prompt = Some(AnnotationPrompt::edit(
                            target.id.as_str(),
                            target.note.clone(),
                        ));
                    }
                    KeyCode::Char('r') => {
                        let target = &annotations[*focus];
                        // Replies anchor to the root; if the focused row is
                        // already a reply, attach the new reply to its root.
                        let parent = target
                            .parent_id
                            .as_ref()
                            .map_or(target.id.as_str(), |p| p.as_str());
                        *prompt = Some(AnnotationPrompt::reply(parent));
                    }
                    KeyCode::Char('d') => {
                        *prompt = Some(AnnotationPrompt::delete_confirm(
                            annotations[*focus].id.as_str(),
                        ));
                    }
                    KeyCode::Char('n') => {
                        *prompt = Some(AnnotationPrompt::create());
                    }
                    _ => {}
                }
                return;
            }
        }

        // Layer 3: scroll mode.
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
            KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
            KeyCode::Char('d') => *scroll = scroll.saturating_add(10),
            KeyCode::Char('u') => *scroll = scroll.saturating_sub(10),
            KeyCode::Char('D') => {
                let pid = paper_id.clone();
                self.queue_download(&pid);
            }
            KeyCode::Char('n') => {
                *prompt = Some(AnnotationPrompt::create());
            }
            KeyCode::Char('J') => {
                // Enter annotation focus mode if the paper has any.
                let pid = paper_id.clone();
                let count = self
                    .data
                    .load_annotations_for_paper(&pid)
                    .map_or(0, |a| a.len());
                if count > 0 {
                    *annotation_focus = Some(0);
                }
            }
            KeyCode::Char('R') => {
                // Enter reader mode (#97). Highlight cursor starts at
                // 0 if the paper has any root annotations.
                let pid = paper_id.clone();
                let count = crate::views::reader::highlight_count(&self.data, &pid);
                *reader = true;
                *highlight_focus = if count > 0 { Some(0) } else { None };
            }
            _ => {}
        }
    }

    /// Translate a prompt submission into the matching repository call
    /// and reconcile UI state afterwards (e.g. clamp annotation focus
    /// when a delete shrinks the list).
    fn dispatch_submission(&mut self, submission: PromptSubmission) {
        let Some(Overlay::PaperDetail {
            paper_id,
            annotation_focus,
            ..
        }) = self.overlay.as_mut()
        else {
            return;
        };
        let pid = paper_id.clone();
        let reader = self.reader.clone();
        match submission {
            PromptSubmission::Create { quote, note } => {
                let _ = self
                    .data
                    .create_root_annotation(&pid, &quote, &note, &reader);
            }
            PromptSubmission::Edit {
                annotation_id,
                note,
            } => {
                let _ = self.data.update_annotation_note(&annotation_id, &note);
            }
            PromptSubmission::Reply { parent_id, note } => {
                let _ = self.data.reply_annotation(&parent_id, &note, &reader);
            }
            PromptSubmission::Delete { annotation_id } => {
                let _ = self.data.delete_annotation(&annotation_id);
                // Keep focus inside the new (smaller) list.
                if let Some(focus) = annotation_focus {
                    let new_count = self
                        .data
                        .load_annotations_for_paper(&pid)
                        .map_or(0, |a| a.len());
                    if new_count == 0 {
                        *annotation_focus = None;
                    } else if *focus >= new_count {
                        *focus = new_count - 1;
                    }
                }
            }
        }
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
                                annotation_focus: None,
                                prompt: None,
                                reader: false,
                                highlight_focus: None,
                            });
                        }
                    }
                    KeyCode::Char('s') => {
                        if let Ok(papers) = self.data.load_papers(1000, 0)
                            && let Some(paper) = papers.get(self.paper_selected)
                        {
                            let pid = paper.id.as_str().to_string();
                            self.toggle_star(&pid);
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

pub fn run(
    db_path: &Path,
    unpaywall_email: String,
    papers_dir: PathBuf,
    show_institutional_hint: bool,
    reader: String,
) -> Result<()> {
    let data = DataStore::open(db_path)?;
    let mut app = App::new(
        data,
        unpaywall_email,
        papers_dir,
        show_institutional_hint,
        reader,
    );

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
        app.drain_all_channels();
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
        Some(Overlay::PaperDetail {
            paper_id,
            scroll,
            annotation_focus,
            prompt,
            reader,
            highlight_focus,
        }) => {
            if *reader {
                crate::views::reader::draw(frame, chunks[1], &app.data, paper_id, *highlight_focus);
            } else {
                detail::draw(
                    frame,
                    chunks[1],
                    &app.data,
                    paper_id,
                    *scroll,
                    *annotation_focus,
                );
                if let Some(active) = prompt {
                    annotation_prompt::draw_overlay(frame, chunks[1], active);
                }
            }
        }
        Some(Overlay::SearchPapers {
            search_id,
            selected,
        }) => {
            papers::draw_for_search(
                frame,
                chunks[1],
                &app.data,
                search_id,
                *selected,
                &app.starred,
            );
        }
        None => match app.tab {
            Tab::Searches => searches::draw(frame, chunks[1], &app.data, app.search_selected),
            Tab::Papers => papers::draw(
                frame,
                chunks[1],
                &app.data,
                app.paper_selected,
                &app.starred,
            ),
            Tab::Questions => {
                questions::draw(frame, chunks[1], &app.data, app.question_selected);
            }
        },
    }

    if task_panel_height > 0 {
        tasks_view::draw(frame, chunks[2], &app.tasks, app.show_institutional_hint);
    }

    let help_text = match (&app.overlay, app.tab) {
        (
            Some(Overlay::PaperDetail {
                annotation_focus,
                prompt,
                reader,
                ..
            }),
            _,
        ) => {
            if *reader {
                "Esc/R: leave reader | J/K: hop highlight | D: download"
            } else {
                match (prompt, annotation_focus) {
                    (Some(p), _) => match p {
                        AnnotationPrompt::DeleteConfirm { .. } => "y: confirm | n/Esc: cancel",
                        AnnotationPrompt::Create { .. } => {
                            "Enter: next/submit | Backspace: delete char | Esc: cancel"
                        }
                        _ => "Enter: submit | Backspace: delete char | Esc: cancel",
                    },
                    (None, Some(_)) => {
                        "Esc: leave focus | j/k: navigate | n: new | e: edit | r: reply | d: delete"
                    }
                    (None, None) => {
                        "Esc/q: back | j/k: scroll | d/u: page | D: download | n: new | J: focus | R: reader"
                    }
                }
            }
        }
        (Some(Overlay::SearchPapers { .. }), _) => {
            "Esc/q: back | j/k: navigate | Enter: open paper"
        }
        (None, Tab::Papers) => "Tab: switch tabs | j/k: navigate | Enter: open | s: star | q: quit",
        (None, _) => "Tab/Shift-Tab: switch tabs | j/k: navigate | Enter: select | q: quit",
    };
    status_bar::draw(frame, chunks[3], help_text, app.offline);
}

/// Step the active annotation prompt by one keystroke. Mutates the
/// `prompt` slot (clears it on cancel/submit, advances stage on Enter
/// in a Create's Quote stage). Returns `Some(submission)` when the
/// prompt resolved to a write the app should dispatch.
fn step_prompt(prompt: &mut Option<AnnotationPrompt>, code: KeyCode) -> Option<PromptSubmission> {
    let active = prompt.as_mut()?;
    match code {
        KeyCode::Esc => {
            *prompt = None;
            None
        }
        KeyCode::Enter => match active.submit() {
            PromptCommit::AdvanceStage(next) => {
                *prompt = Some(next);
                None
            }
            PromptCommit::Submit(s) => {
                *prompt = None;
                Some(s)
            }
            PromptCommit::Cancel => {
                *prompt = None;
                None
            }
        },
        KeyCode::Backspace => {
            active.backspace();
            None
        }
        KeyCode::Char(ch) => {
            // Confirm prompts route y/n through their own handler.
            if let Some(commit) = active.confirm(ch) {
                let result = match commit {
                    PromptCommit::Submit(s) => Some(s),
                    PromptCommit::Cancel | PromptCommit::AdvanceStage(_) => None,
                };
                *prompt = None;
                return result;
            }
            active.push_char(ch);
            None
        }
        _ => None,
    }
}

/// One-shot check: is the internet reachable? Uses a 3s HEAD to a stable
/// endpoint. Any failure (DNS, timeout, non-2xx) is treated as offline.
///
/// Tests and tape runs can force the offline branch by setting
/// `SCITADEL_FORCE_OFFLINE=1`.
async fn probe_network() -> bool {
    if std::env::var("SCITADEL_FORCE_OFFLINE")
        .ok()
        .is_some_and(|v| !v.is_empty() && v != "0")
    {
        return false;
    }
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    client
        .head("https://api.openalex.org/")
        .send()
        .await
        .is_ok_and(|r| r.status().is_success() || r.status().is_redirection())
}
