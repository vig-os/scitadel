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
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Tabs;
use tokio::sync::mpsc;

use crate::data::DataStore;
use crate::tasks::{Task, TaskKind, TaskStatus, TaskUpdate, spawn_download_paper};
use crate::views::annotation_prompt::{AnnotationPrompt, PromptCommit, PromptSubmission};
use crate::views::{
    annotation_prompt, dashboard, detail, papers, questions, queue, searches, tasks as tasks_view,
};
use crate::widgets::status_bar;

/// Active tab in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Searches,
    Papers,
    Questions,
    /// Cross-search aggregator showing all starred papers (#48).
    /// Reuses the Papers-tab rendering + Enter-to-open-detail flow
    /// but backs the data by `paper_state.starred = 1` instead of
    /// a single search.
    Queue,
}

impl Tab {
    const ALL: [Self; 4] = [Self::Searches, Self::Papers, Self::Questions, Self::Queue];

    fn index(self) -> usize {
        match self {
            Self::Searches => 0,
            Self::Papers => 1,
            Self::Questions => 2,
            Self::Queue => 3,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Searches => Self::Papers,
            Self::Papers => Self::Questions,
            Self::Questions => Self::Queue,
            Self::Queue => Self::Searches,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Searches => Self::Queue,
            Self::Papers => Self::Searches,
            Self::Questions => Self::Papers,
            Self::Queue => Self::Questions,
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
    /// Question Dashboard (#133). Split-pane ranked-by-score view of
    /// papers assessed against the question, with a citation shortlist
    /// curated via `c`.
    QuestionDashboard {
        question_id: String,
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
    pub queue_selected: usize,

    pub tasks: Vec<Task>,
    /// Last `TuiState` written to the DB, used by `publish_tui_state`
    /// to skip redundant UPDATEs when the user hasn't moved (#122).
    pub last_published_state: Option<scitadel_db::sqlite::TuiState>,
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
            queue_selected: 0,
            tasks: Vec::new(),
            last_published_state: None,
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
        self.flush_completed_tasks();
        self.publish_tui_state();
    }

    /// Write the TUI's current selection to the singleton `tui_state`
    /// row so an MCP-side agent can ask "what's the user looking at?"
    /// (#122). Runs every drain pass; only writes if the *selection*
    /// (not the timestamp) changed since last publish — otherwise the
    /// TUI would emit ~10 UPDATEs/sec on a static screen.
    fn publish_tui_state(&mut self) {
        let snapshot = self.current_tui_state();
        if let Some(prev) = &self.last_published_state
            && tui_state_key(prev) == tui_state_key(&snapshot)
        {
            return;
        }
        if let Err(e) = self.data.publish_tui_state(&snapshot) {
            tracing::warn!(error = %e, "failed to publish TUI state");
            return;
        }
        self.last_published_state = Some(snapshot);
    }

    fn current_tui_state(&self) -> scitadel_db::sqlite::TuiState {
        let tab = match self.tab {
            Tab::Searches => "Searches",
            Tab::Papers => "Papers",
            Tab::Questions => "Questions",
            Tab::Queue => "Queue",
        };
        let (paper_id, search_id, annotation_id) = match &self.overlay {
            Some(Overlay::PaperDetail {
                paper_id,
                annotation_focus,
                ..
            }) => {
                // Resolve the focused annotation's id only when the
                // user is actively in focus mode — otherwise leave None
                // so agents don't think the user is "on" an annotation
                // they happen to be scrolling past.
                let ann_id = annotation_focus.and_then(|i| {
                    self.data
                        .load_annotations_for_paper(paper_id)
                        .ok()
                        .and_then(|anns| anns.get(i).map(|a| a.id.as_str().to_string()))
                });
                (Some(paper_id.clone()), None, ann_id)
            }
            Some(Overlay::SearchPapers { search_id, .. }) => (None, Some(search_id.clone()), None),
            Some(Overlay::QuestionDashboard { .. }) | None => (None, None, None),
        };
        // Question id: from the overlay (dashboard open) or the
        // Questions-tab cursor. Dashboard wins since it's more specific.
        let question_id =
            if let Some(Overlay::QuestionDashboard { question_id, .. }) = &self.overlay {
                Some(question_id.clone())
            } else if matches!(self.tab, Tab::Questions) {
                self.data.load_questions().ok().and_then(|qs| {
                    qs.get(self.question_selected)
                        .map(|q| q.id.as_str().to_string())
                })
            } else {
                None
            };
        scitadel_db::sqlite::TuiState {
            tab: tab.to_string(),
            paper_id,
            search_id,
            question_id,
            annotation_id,
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn drain_task_updates(&mut self) {
        while let Ok(update) = self.task_rx.try_recv() {
            match update {
                TaskUpdate::New(task) => {
                    self.tasks.push(task);
                    // Evict the oldest *terminal* task only — never drop
                    // a Queued or Running download just because the panel
                    // is full. If everything in the panel is still active,
                    // grow past the cap rather than lose work.
                    if self.tasks.len() > 10
                        && let Some(idx) = self.tasks.iter().position(|t| t.terminal_at.is_some())
                    {
                        self.tasks.remove(idx);
                    }
                }
                TaskUpdate::Status { id, status } => {
                    // Persist Done/Failed back to papers.download_status
                    // (#112) before mutating the in-memory task — borrow
                    // the matching task once to read the paper_id.
                    let paper_id = self
                        .tasks
                        .iter()
                        .find(|t| t.id == id)
                        .map(|t| match &t.kind {
                            TaskKind::Download { paper_id, .. } => (paper_id.clone(), true),
                            TaskKind::OpenExternal { paper_id, .. } => (paper_id.clone(), false),
                        });
                    if let Some((pid, persist)) = paper_id
                        && persist
                    {
                        self.persist_download_outcome(&pid, &status);
                    }
                    if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
                        t.status = status;
                        if matches!(t.status, TaskStatus::Done { .. } | TaskStatus::Failed(_)) {
                            t.terminal_at = Some(std::time::Instant::now());
                        }
                    }
                }
            }
        }
    }

    /// Drop terminal tasks once their linger window has elapsed (5s for
    /// Done, 30s for Failed). Runs every drain pass — cheap.
    fn flush_completed_tasks(&mut self) {
        crate::tasks::retain_recent_terminal(&mut self.tasks, std::time::Instant::now());
    }

    /// Drop every terminal task immediately. Bound to `c` in tab mode (#113).
    fn clear_completed_tasks(&mut self) {
        crate::tasks::clear_terminal(&mut self.tasks);
    }

    /// Set of paper IDs that have a Queued or Running download task.
    /// Used by the Papers table to render the `↻` in-flight symbol.
    fn downloading_paper_ids(&self) -> std::collections::HashSet<String> {
        self.tasks
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Queued | TaskStatus::Running))
            .filter_map(|t| match &t.kind {
                TaskKind::Download { paper_id, .. } => Some(paper_id.clone()),
                TaskKind::OpenExternal { .. } => None,
            })
            .collect()
    }

    fn persist_download_outcome(&self, paper_id: &str, status: &TaskStatus) {
        use scitadel_adapters::download::AccessStatus;
        use scitadel_core::models::DownloadStatus;
        let outcome = match status {
            TaskStatus::Done { path, access, .. } => {
                let download_status = match access {
                    AccessStatus::FullText => DownloadStatus::Downloaded,
                    AccessStatus::Abstract | AccessStatus::Paywall | AccessStatus::Unknown => {
                        DownloadStatus::Paywall
                    }
                };
                Some((path.to_string_lossy().into_owned(), download_status))
            }
            TaskStatus::Failed(_) => Some((String::new(), DownloadStatus::Failed)),
            TaskStatus::Queued | TaskStatus::Running => None,
        };
        if let Some((path, ds)) = outcome {
            let path_arg = if matches!(ds, DownloadStatus::Failed) {
                None
            } else {
                Some(path.as_str())
            };
            if let Err(e) = self.data.record_download_outcome(paper_id, path_arg, ds) {
                tracing::warn!(
                    paper_id,
                    error = %e,
                    "failed to persist download outcome"
                );
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
            // `c` clears all terminal tasks (#113). Available globally
            // in tab mode; no-op when the panel is already empty.
            KeyCode::Char('c') => self.clear_completed_tasks(),
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
            Some(Overlay::QuestionDashboard {
                ref question_id,
                ref mut selected,
            }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    let count = dashboard::row_count(&self.data, question_id);
                    if count > 0 {
                        *selected = (*selected + 1).min(count - 1);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Char('c') => {
                    let qid = question_id.clone();
                    let sel = *selected;
                    if let Ok(rows) = self.data.load_question_dashboard(&qid)
                        && let Some((paper, _)) = rows.get(sel)
                    {
                        let pid = paper.id.as_str().to_string();
                        let _ = self.data.toggle_shortlist(&qid, &pid, &self.reader);
                    }
                }
                KeyCode::Enter => {
                    // Open the focused paper's detail overlay on top of
                    // the dashboard — same overlay mechanic as the
                    // SearchPapers flow. Esc returns to the dashboard.
                    let qid = question_id.clone();
                    let sel = *selected;
                    if let Ok(rows) = self.data.load_question_dashboard(&qid)
                        && let Some((paper, _)) = rows.get(sel)
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
        // Fail fast when we know the network is down — spawning the
        // download task anyway would waste ~60s on a reqwest timeout
        // before surfacing the failure in the task panel (#51). The
        // offline badge is already showing; the user can retry after
        // reconnecting.
        if self.offline {
            let _ = crate::tasks::synthesize_offline_failure(self.task_tx.clone(), &paper);
            return;
        }
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
            KeyCode::Char('O') => {
                // #144: open the locally downloaded file in the OS
                // default viewer. PDFs that are unreadable as plain
                // text (figures, math, tables) are the main motivator —
                // R is for the in-TUI reader, O escapes to the real one.
                let pid = paper_id.clone();
                self.open_paper_externally(&pid);
            }
            _ => {}
        }
    }

    /// Spawn the OS default viewer for the paper's local file. Surfaces
    /// errors (no local file, exec failure) via a transient task panel
    /// row — success is implicit because the user sees the viewer pop up.
    fn open_paper_externally(&self, paper_id: &str) {
        let Ok(Some(paper)) = self.data.load_paper(paper_id) else {
            return;
        };
        let path = paper.local_path.as_ref().map(std::path::PathBuf::from);
        if let Some(p) = path {
            crate::tasks::spawn_open_external(self.task_tx.clone(), &paper, p);
        } else {
            // Synthesize a Failed task so the user sees feedback rather
            // than a silent no-op when they haven't downloaded yet.
            let id = uuid::Uuid::new_v4();
            let ref_id = paper
                .doi
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| paper.arxiv_id.clone().filter(|s| !s.is_empty()))
                .unwrap_or_else(|| paper.id.as_str().chars().take(8).collect());
            let task = crate::tasks::Task {
                id,
                kind: TaskKind::OpenExternal {
                    paper_id: paper.id.as_str().to_string(),
                    ref_id,
                    title: paper.title.clone(),
                },
                status: TaskStatus::Queued,
                terminal_at: None,
            };
            let _ = self.task_tx.send(crate::tasks::TaskUpdate::New(task));
            let _ = self.task_tx.send(crate::tasks::TaskUpdate::Status {
                id,
                status: TaskStatus::Failed("no local file — press D to download first".to_string()),
            });
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
                    KeyCode::Enter => {
                        if let Ok(questions) = self.data.load_questions()
                            && let Some(q) = questions.get(self.question_selected)
                        {
                            self.overlay = Some(Overlay::QuestionDashboard {
                                question_id: q.id.as_str().to_string(),
                                selected: 0,
                            });
                        }
                    }
                    _ => {}
                }
            }
            Tab::Queue => {
                let count = self
                    .data
                    .load_starred_papers(&self.reader)
                    .map_or(0, |p| p.len());
                match code {
                    KeyCode::Char('j') | KeyCode::Down if count > 0 => {
                        self.queue_selected = (self.queue_selected + 1).min(count - 1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.queue_selected = self.queue_selected.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if let Ok(papers) = self.data.load_starred_papers(&self.reader)
                            && let Some(paper) = papers.get(self.queue_selected)
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
                        // Unstar: consistent with Papers tab behaviour.
                        if let Ok(papers) = self.data.load_starred_papers(&self.reader)
                            && let Some(paper) = papers.get(self.queue_selected)
                        {
                            let pid = paper.id.as_str().to_string();
                            self.toggle_star(&pid);
                            // Clamp cursor — list just shrank by one.
                            let new_count = self
                                .data
                                .load_starred_papers(&self.reader)
                                .map_or(0, |p| p.len());
                            if new_count > 0 {
                                self.queue_selected = self.queue_selected.min(new_count - 1);
                            } else {
                                self.queue_selected = 0;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Identity of a `TuiState` for dedup purposes — everything except
/// `updated_at`. Two snapshots with the same key represent the same
/// user-visible selection so we skip the redundant DB write (#122).
fn tui_state_key(
    s: &scitadel_db::sqlite::TuiState,
) -> (&str, Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
    (
        s.tab.as_str(),
        s.paper_id.as_deref(),
        s.search_id.as_deref(),
        s.question_id.as_deref(),
        s.annotation_id.as_deref(),
    )
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
                Tab::Queue => "Queue",
            };
            Line::from(Span::raw(label))
        })
        .collect();

    let tabs = Tabs::new(tab_titles)
        .select(app.tab.index())
        .highlight_style(
            Style::default()
                .fg(crate::theme::theme().emphasis)
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
                &app.downloading_paper_ids(),
            );
        }
        Some(Overlay::QuestionDashboard {
            question_id,
            selected,
        }) => {
            dashboard::draw(
                frame,
                chunks[1],
                &app.data,
                question_id,
                &app.reader,
                *selected,
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
                &app.downloading_paper_ids(),
            ),
            Tab::Questions => {
                questions::draw(frame, chunks[1], &app.data, app.question_selected);
            }
            Tab::Queue => queue::draw(
                frame,
                chunks[1],
                &app.data,
                &app.reader,
                app.queue_selected,
                &app.downloading_paper_ids(),
            ),
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
                        "Esc/q: back | j/k: scroll | d/u: page | D: download | O: open externally | R: reader | n: new | J: focus"
                    }
                }
            }
        }
        (Some(Overlay::SearchPapers { .. }), _) => {
            "Esc/q: back | j/k: navigate | Enter: open paper"
        }
        (Some(Overlay::QuestionDashboard { .. }), _) => {
            "Esc/q: back | j/k: navigate | Enter: open paper | c: toggle shortlist"
        }
        (None, Tab::Papers | Tab::Queue) => {
            "Tab: switch tabs | j/k: navigate | Enter: open | s: (un)star | c: clear tasks | q: quit"
        }
        (None, _) => {
            "Tab/Shift-Tab: switch tabs | j/k: navigate | Enter: select | c: clear tasks | q: quit"
        }
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
