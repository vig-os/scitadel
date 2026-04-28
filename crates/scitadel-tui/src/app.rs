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
use crate::views::bib_export_prompt::BibExportPrompt;
use crate::views::{
    annotation_prompt, bib_export_prompt, dashboard, detail, papers, questions, queue, searches,
    tasks as tasks_view,
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
    /// curated via `c`. The optional `export_prompt` field carries the
    /// state of the `E` path-prompt overlay (#135 sub-feature B); when
    /// `Some`, the prompt eats every keystroke until Enter / Esc.
    QuestionDashboard {
        question_id: String,
        selected: usize,
        export_prompt: Option<BibExportPrompt>,
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
    /// Transient toast shown in the status bar for the first N draws
    /// after launch (#137). `Some(label)` until cleared by
    /// `tick_startup_toast`. Used to flash e.g.
    /// `theme: dalton-dark (auto)` so users can verify which palette
    /// resolved without digging through `--list-themes`.
    startup_toast: Option<String>,
    /// Draw-tick counter for `startup_toast`. The toast is dropped once
    /// this exceeds `STARTUP_TOAST_FRAMES`. We count draws (~10 Hz on
    /// the 100 ms event-poll cadence, ~30 frames ≈ 3 s) rather than
    /// wall-clock so headless tape runs reproduce the same lifetime.
    startup_toast_frames: u32,
    /// Transient status-bar toast set by user-facing actions
    /// (currently: bib export success/failure, #135 sub-feature B).
    /// Shape mirrors the startup toast so we don't need a second
    /// rendering branch — both feed the same status-bar slot. Lifetime
    /// is per-toast so a long error can linger longer than a quick OK.
    status_toast: Option<StatusToast>,
    /// Cached count of annotations the current `reader` hasn't
    /// acknowledged. Refreshed on every draw via `data.load_unread_count`
    /// so the `[N new]` status-bar badge reflects MCP-side writes within
    /// one tick (~100 ms). Kept on `App` rather than recomputing inside
    /// the widget so a future event-driven refresh path can update the
    /// same field. (#185)
    pub unread_count: i64,

    task_tx: mpsc::UnboundedSender<TaskUpdate>,
    task_rx: mpsc::UnboundedReceiver<TaskUpdate>,
    offline_rx: mpsc::UnboundedReceiver<bool>,
}

/// How many draws the startup toast lingers for (#137). At the
/// 100 ms event-poll cadence this is ~3 seconds — long enough to read,
/// short enough to clear before the user does anything substantial.
const STARTUP_TOAST_FRAMES: u32 = 30;

/// Lifetime budget (in draws) for action-success toasts. ~30 frames at
/// the 100 ms poll cadence ≈ 3 s.
const STATUS_TOAST_OK_FRAMES: u32 = 30;

/// Lifetime budget (in draws) for action-failure toasts. Doubled so the
/// user has time to read a longer error message before it clears.
const STATUS_TOAST_ERR_FRAMES: u32 = 60;

/// Action-driven status toast (success or failure). Mirrors the shape
/// of `startup_toast` + `startup_toast_frames` but ships them in a
/// single struct so per-toast lifetime is configurable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusToast {
    pub message: String,
    pub frames_remaining: u32,
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
            startup_toast: None,
            startup_toast_frames: 0,
            status_toast: None,
            unread_count: 0,
            task_tx,
            task_rx,
            offline_rx,
        }
    }

    /// Refresh `unread_count` from the DB. Cheap (`COUNT(*)` over the
    /// annotations LEFT JOIN annotation_reads predicate), called on
    /// every render tick. Failures are silently swallowed — a stale
    /// badge is acceptable; an error toast every 100 ms is not. (#185)
    fn refresh_unread_count(&mut self) {
        self.unread_count = self.data.load_unread_count(&self.reader).unwrap_or(0);
    }

    /// Advance the startup-toast lifetime by one draw (#137). Call once
    /// per frame from the render path; clears `startup_toast` once
    /// `STARTUP_TOAST_FRAMES` have elapsed.
    fn tick_startup_toast(&mut self) {
        if self.startup_toast.is_none() {
            return;
        }
        self.startup_toast_frames = self.startup_toast_frames.saturating_add(1);
        if self.startup_toast_frames > STARTUP_TOAST_FRAMES {
            self.startup_toast = None;
        }
    }

    /// Decrement the action-toast lifetime by one draw. Mirrors
    /// `tick_startup_toast` so both toast slots share the same
    /// per-frame counter cadence — there is no second timing
    /// subsystem (#135 sub-feature B keeps the existing primitive).
    fn tick_status_toast(&mut self) {
        let Some(toast) = self.status_toast.as_mut() else {
            return;
        };
        if toast.frames_remaining == 0 {
            self.status_toast = None;
        } else {
            toast.frames_remaining -= 1;
        }
    }

    /// Show a success/info status-bar toast for ~30 frames.
    pub(crate) fn set_status_ok(&mut self, message: impl Into<String>) {
        self.status_toast = Some(StatusToast {
            message: message.into(),
            frames_remaining: STATUS_TOAST_OK_FRAMES,
        });
    }

    /// Show an error status-bar toast for ~60 frames (longer so the
    /// user has time to read it).
    pub(crate) fn set_status_err(&mut self, message: impl Into<String>) {
        self.status_toast = Some(StatusToast {
            message: message.into(),
            frames_remaining: STATUS_TOAST_ERR_FRAMES,
        });
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

    /// True when the currently-active overlay is consuming character
    /// keystrokes as text input — a per-paper annotation prompt or the
    /// question-dashboard bib-export path prompt. Used by the global
    /// theme-toggle hotkey (#175) to avoid hijacking `T` while the
    /// user is typing into a field. Mirrors the same gating pattern
    /// `handle_paper_detail_key` and `handle_question_dashboard_key`
    /// already use to layer their own keybinds beneath an active prompt.
    fn in_text_input_context(&self) -> bool {
        match &self.overlay {
            Some(Overlay::PaperDetail { prompt, .. }) => prompt.is_some(),
            Some(Overlay::QuestionDashboard { export_prompt, .. }) => export_prompt.is_some(),
            _ => false,
        }
    }

    /// Swap to the next registered theme and flash the new name in the
    /// status bar (#175). In-memory only — the user's `[ui] theme`
    /// config isn't touched, so the next launch resolves the same way
    /// it would have before the toggle. The `set_status_ok` slot reuses
    /// the existing toast subsystem (#135-B / #137 P1) so this doesn't
    /// add a second timing path.
    fn cycle_theme(&mut self) {
        let current = crate::theme::theme();
        let next = crate::theme::Theme::cycle_next(current);
        crate::theme::set(next);
        let label = crate::theme::Theme::name(&next);
        self.set_status_ok(format!("theme: {label}"));
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

        // #175 — runtime theme toggle. Uppercase `T` is unused
        // elsewhere in the keymap (lowercase `t` is also free; we
        // chose `T` so it parallels the other capital action keys
        // — `D` (download), `R` (reader), `O` (open), `E` (export)).
        // Gated to non-text-input contexts: text input is only active
        // when an export prompt or annotation prompt is open inside
        // an overlay (those layers consume `KeyCode::Char(_)`
        // wholesale before reaching here, so a top-level dispatch is
        // safe). If new top-level text inputs land later, gate them
        // here explicitly.
        if code == KeyCode::Char('T') && !self.in_text_input_context() {
            self.cycle_theme();
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
            Some(Overlay::QuestionDashboard { .. }) => {
                self.handle_question_dashboard_key(code);
            }
            None => {}
        }
    }

    /// Routes a key inside the QuestionDashboard overlay. Layered just
    /// like the PaperDetail handler so the active prompt (the `E`
    /// export prompt) eats every keystroke before falling through to
    /// the j/k/c/Enter list controls.
    fn handle_question_dashboard_key(&mut self, code: KeyCode) {
        let Some(Overlay::QuestionDashboard {
            question_id,
            selected,
            export_prompt,
        }) = self.overlay.as_mut()
        else {
            return;
        };

        // Layer 1: the path prompt is open — eat the keystroke here.
        if export_prompt.is_some() {
            self.handle_export_prompt_key(code);
            return;
        }

        // Layer 2: plain dashboard list controls.
        match code {
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
            // Open the export-path prompt. Uppercase `E` keeps the key
            // out of the namespace of the lowercase `e` (annotation-edit)
            // used inside PaperDetail and free of the global `c` we
            // already use in tab mode (#135 sub-feature B).
            KeyCode::Char('E') => {
                let qid = question_id.clone();
                let question_text = self
                    .data
                    .load_question(&qid)
                    .ok()
                    .flatten()
                    .map(|q| q.text)
                    .unwrap_or_default();
                *export_prompt = Some(BibExportPrompt::from_question(
                    &question_text,
                    scitadel_export::SnapshotFormat::BibTeX,
                ));
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
        }
    }

    /// Drive the export-path prompt's text edits + Enter/Esc. Pulled
    /// into its own method so the dispatch above stays readable.
    fn handle_export_prompt_key(&mut self, code: KeyCode) {
        let Some(Overlay::QuestionDashboard {
            question_id,
            export_prompt: Some(prompt),
            ..
        }) = self.overlay.as_mut()
        else {
            return;
        };

        match code {
            KeyCode::Esc => {
                if let Some(Overlay::QuestionDashboard { export_prompt, .. }) =
                    self.overlay.as_mut()
                {
                    *export_prompt = None;
                }
            }
            KeyCode::Char(c) => prompt.push_char(c),
            KeyCode::Backspace => prompt.backspace(),
            KeyCode::Enter => {
                let path = prompt.submit();
                let format = prompt.format;
                let qid = question_id.clone();
                // Drop the prompt before any DB / I/O so a long-running
                // write doesn't keep the overlay borrow alive.
                if let Some(Overlay::QuestionDashboard { export_prompt, .. }) =
                    self.overlay.as_mut()
                {
                    *export_prompt = None;
                }
                if let Some(path_str) = path {
                    self.run_bib_export(&qid, std::path::PathBuf::from(path_str), format);
                }
            }
            _ => {}
        }
    }

    /// Resolve the question's shortlist + tags and write the snapshot.
    ///
    /// Routes through the shared `scitadel_export::write_snapshot`
    /// helper so the on-disk `.bib` + `.scitadel-bib.lock` artifacts
    /// are byte-identical to what `bib snapshot` would write from the
    /// CLI. Surfaces the outcome as a status-bar toast — never panics,
    /// never bubbles I/O errors past the prompt.
    fn run_bib_export(
        &mut self,
        question_id: &str,
        output_path: std::path::PathBuf,
        format: scitadel_export::SnapshotFormat,
    ) {
        let (paper_ids, papers, tags) =
            match self.data.load_snapshot_inputs(question_id, &self.reader) {
                Ok(t) => t,
                Err(e) => {
                    self.set_status_err(format!("export failed: {e}"));
                    return;
                }
            };
        if papers.is_empty() {
            self.set_status_err("export failed: shortlist is empty");
            return;
        }
        let result = scitadel_export::write_snapshot(
            &output_path,
            question_id,
            &self.reader,
            &papers,
            &paper_ids,
            |id| tags.get(id).cloned().unwrap_or_default(),
            format,
            true,
        );
        match result {
            Ok(outcome) => {
                let display = outcome.output_path.file_name().map_or_else(
                    || outcome.output_path.display().to_string(),
                    |n| n.to_string_lossy().to_string(),
                );
                self.set_status_ok(format!(
                    "exported: {display} ({} entries)",
                    outcome.entry_count
                ));
            }
            Err(e) => self.set_status_err(format!("export failed: {e}")),
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
                                export_prompt: None,
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
    startup_toast: Option<String>,
) -> Result<()> {
    let data = DataStore::open(db_path)?;
    let mut app = App::new(
        data,
        unpaywall_email,
        papers_dir,
        show_institutional_hint,
        reader,
    );
    app.startup_toast = startup_toast;

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
    // Refresh the unread badge each tick so MCP-side annotation
    // writes show up in the status bar within ~100ms (the event-poll
    // cadence). (#185)
    app.refresh_unread_count();
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
            export_prompt,
        }) => {
            dashboard::draw(
                frame,
                chunks[1],
                &app.data,
                question_id,
                &app.reader,
                *selected,
            );
            if let Some(prompt) = export_prompt {
                bib_export_prompt::draw_overlay(frame, chunks[1], prompt);
            }
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
        (
            Some(Overlay::QuestionDashboard {
                export_prompt: Some(_),
                ..
            }),
            _,
        ) => "Enter: export | Backspace: delete char | Esc: cancel",
        (Some(Overlay::QuestionDashboard { .. }), _) => {
            "Esc/q: back | j/k: navigate | Enter: open paper | c: toggle shortlist | E: export bib"
        }
        (None, Tab::Papers | Tab::Queue) => {
            "Tab: switch tabs | j/k: navigate | Enter: open | s: (un)star | c: clear tasks | T: theme | q: quit"
        }
        (None, _) => {
            "Tab/Shift-Tab: switch tabs | j/k: navigate | Enter: select | c: clear tasks | T: theme | q: quit"
        }
    };
    // Startup toast (#137) hijacks the status bar for its lifetime so
    // the user sees `theme: dalton-bright (auto)` right after launch
    // without us having to add a second status row. Falls back to the
    // normal help_text once the toast expires. Action-driven toasts
    // (#135 sub-feature B — "exported: paper.bib (12 entries)") share
    // the same slot and take priority over the startup toast since
    // they're always more recent than launch.
    let toast_text;
    let bar_text: &str = if let Some(status) = app.status_toast.as_ref() {
        toast_text = status.message.clone();
        &toast_text
    } else if let Some(toast) = app.startup_toast.as_deref() {
        toast_text = toast.to_string();
        &toast_text
    } else {
        help_text
    };
    status_bar::draw(frame, chunks[3], bar_text, app.offline, app.unread_count);
    app.tick_startup_toast();
    app.tick_status_toast();
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

#[cfg(test)]
mod tests {
    //! End-to-end-ish tests for the export keybind (#135 sub-feature B).
    //!
    //! These tests exercise the App-level keystroke dispatch path with
    //! a real `DataStore` backed by a tempfile SQLite DB. We seed a
    //! research question + a shortlisted paper, then drive `handle_key`
    //! and inspect the overlay state + on-disk artifacts.

    use super::*;
    use scitadel_core::models::{Paper, PaperId, ResearchQuestion};
    use scitadel_core::ports::{PaperRepository, QuestionRepository};
    use std::path::PathBuf;

    /// Build an `App` with a fresh on-disk SQLite DB and a single
    /// shortlisted paper attached to one research question. Returns
    /// `(app, question_id, papers_dir)`. `papers_dir` is rooted in a
    /// `tempfile::TempDir` whose lifetime the caller must keep alive.
    fn fixture(tmp: &tempfile::TempDir) -> (App, String, PathBuf) {
        let db_path = tmp.path().join("scitadel.db");
        let data = DataStore::open(&db_path).unwrap();

        // Seed: a research question + one paper + shortlist row.
        let q = ResearchQuestion::new("What is the role of attention?");
        let qid = q.id.as_str().to_string();
        let mut p = Paper::new("Attention Is All You Need");
        p.id = PaperId::from("p-attn");
        p.authors = vec!["Vaswani, A.".into()];
        p.year = Some(2017);
        // Use the underlying repos directly — DataStore's API is read-
        // mostly so seeding goes through the lower-level handles.
        let db = scitadel_db::sqlite::Database::open(&db_path).unwrap();
        db.migrate().unwrap();
        let (paper_repo, _, q_repo, _, _) = db.repositories();
        q_repo.save_question(&q).unwrap();
        paper_repo.save(&p).unwrap();
        let shortlist = scitadel_db::sqlite::SqliteShortlistRepository::new(db);
        shortlist.toggle(&qid, p.id.as_str(), "lars").unwrap();

        let papers_dir = tmp.path().join("papers");
        std::fs::create_dir_all(&papers_dir).unwrap();

        let app = App::new(
            data,
            "demo@example.org".into(),
            papers_dir.clone(),
            false,
            "lars".into(),
        );
        (app, qid, papers_dir)
    }

    /// Pressing `E` on the QuestionDashboard opens the path-prompt
    /// overlay with the slugified default. The dashboard `selected`
    /// cursor is left untouched.
    #[tokio::test(flavor = "current_thread")]
    async fn e_on_question_dashboard_opens_export_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, qid, _) = fixture(&tmp);
        app.overlay = Some(Overlay::QuestionDashboard {
            question_id: qid.clone(),
            selected: 0,
            export_prompt: None,
        });

        app.handle_key(KeyCode::Char('E'), KeyModifiers::NONE);

        match app.overlay {
            Some(Overlay::QuestionDashboard {
                export_prompt: Some(ref p),
                ..
            }) => {
                assert_eq!(p.path_buf, "what-is-the-role-of-attention.bib");
            }
            other => panic!("expected QuestionDashboard with prompt, got {other:?}"),
        }
    }

    /// The `E` key must NOT be hijacked when no overlay is active —
    /// that namespace belongs to the underlying tab. We assert that
    /// pressing `E` on a plain tab leaves the overlay state alone (no
    /// QuestionDashboard appears out of nowhere).
    #[tokio::test(flavor = "current_thread")]
    async fn e_on_searches_tab_is_not_hijacked() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, _, _) = fixture(&tmp);
        // No overlay; default tab is Searches.
        assert!(app.overlay.is_none());

        app.handle_key(KeyCode::Char('E'), KeyModifiers::NONE);
        assert!(
            app.overlay.is_none(),
            "E on Searches tab must not pop a dashboard overlay"
        );
    }

    /// Drive the full happy path: open dashboard → press `E` → accept
    /// the default path → assert the `.bib` + `.scitadel-bib.lock`
    /// land on disk. The CWD is moved into the tempdir for the test
    /// so the slug-derived default (`what-is-the-role-of-attention.bib`)
    /// resolves to a sandboxed location.
    #[tokio::test(flavor = "current_thread")]
    async fn export_writes_bib_and_sidecar_on_default_path() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, qid, _) = fixture(&tmp);
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        app.overlay = Some(Overlay::QuestionDashboard {
            question_id: qid,
            selected: 0,
            export_prompt: None,
        });
        app.handle_key(KeyCode::Char('E'), KeyModifiers::NONE);
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE);

        let bib = tmp.path().join("what-is-the-role-of-attention.bib");
        let lock = tmp
            .path()
            .join("what-is-the-role-of-attention.bib.scitadel-bib.lock");

        // Restore CWD before any assertion so a panic doesn't pollute
        // the rest of the test runner.
        std::env::set_current_dir(prev_cwd).unwrap();

        assert!(bib.exists(), "{} missing", bib.display());
        assert!(lock.exists(), "{} missing", lock.display());
        let bib_bytes = std::fs::read_to_string(&bib).unwrap();
        assert!(
            bib_bytes.contains("Attention Is All You Need"),
            "bib didn't contain the seeded title; got:\n{bib_bytes}"
        );
        // Status toast confirms count + filename.
        let toast = app.status_toast.as_ref().expect("status toast set");
        assert!(
            toast.message.starts_with("exported: ") && toast.message.contains("(1 entries)"),
            "got toast: {}",
            toast.message
        );
    }

    /// Pressing `T` outside a text-input context cycles the active
    /// theme and flashes a status-bar toast naming the new palette
    /// (#175). We snapshot the toast slot rather than the global
    /// theme to keep the assertion independent of test ordering —
    /// other tests may mutate `crate::theme::ACTIVE` in parallel.
    #[tokio::test(flavor = "current_thread")]
    async fn t_cycles_theme_outside_input_context() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, _, _) = fixture(&tmp);
        // Reset the global theme to a known starting palette so the
        // post-toggle name is deterministic regardless of which other
        // test ran first.
        crate::theme::set(crate::theme::Theme::DALTON_DARK);

        app.handle_key(KeyCode::Char('T'), KeyModifiers::NONE);

        let toast = app.status_toast.as_ref().expect("toast set after T");
        assert_eq!(toast.message, "theme: dalton-bright");
    }

    /// `T` must NOT cycle the theme while the user is typing into a
    /// prompt — the keystroke would otherwise be eaten before it reached
    /// the export prompt's `push_char` handler. This guards the gating
    /// added alongside the toggle (#175).
    #[tokio::test(flavor = "current_thread")]
    async fn t_in_export_prompt_does_not_cycle_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, qid, _) = fixture(&tmp);
        app.overlay = Some(Overlay::QuestionDashboard {
            question_id: qid,
            selected: 0,
            export_prompt: Some(BibExportPrompt::from_question(
                "x",
                scitadel_export::SnapshotFormat::BibTeX,
            )),
        });

        app.handle_key(KeyCode::Char('T'), KeyModifiers::NONE);

        // The toast slot must NOT contain a "theme:" message. The
        // prompt should also have absorbed the keystroke as a 'T'
        // character (path edit), confirming it reached the prompt.
        let toast_msg = app
            .status_toast
            .as_ref()
            .map(|t| t.message.clone())
            .unwrap_or_default();
        assert!(
            !toast_msg.starts_with("theme:"),
            "theme toggle should not fire while a text-input prompt is open; got toast: {toast_msg}",
        );
        match &app.overlay {
            Some(Overlay::QuestionDashboard {
                export_prompt: Some(p),
                ..
            }) => assert!(
                p.path_buf.contains('T'),
                "T should have been pushed into prompt buffer, got {:?}",
                p.path_buf,
            ),
            other => panic!("expected dashboard with prompt still open, got {other:?}"),
        }
    }

    /// The toggle is session-only (#175) — it must not write back to
    /// the on-disk config. Cycle the theme and assert the config file
    /// the test fixture would have read is unchanged before/after.
    /// Today the TUI doesn't write the config from anywhere, but this
    /// test pins the contract so a future refactor can't quietly start
    /// persisting toggle state.
    #[tokio::test(flavor = "current_thread")]
    async fn theme_toggle_does_not_persist_to_config() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        let original = "[ui]\ntheme = \"dark\"\n";
        std::fs::write(&cfg_path, original).unwrap();
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        let (mut app, _, _) = fixture(&tmp);
        crate::theme::set(crate::theme::Theme::DALTON_DARK);
        app.handle_key(KeyCode::Char('T'), KeyModifiers::NONE);

        let after = std::fs::read_to_string(&cfg_path).unwrap();
        assert_eq!(before, after, "config.toml must not be rewritten by toggle");
    }

    /// Esc inside the prompt clears it without writing anything.
    #[tokio::test(flavor = "current_thread")]
    async fn esc_in_export_prompt_cancels() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut app, qid, _) = fixture(&tmp);
        app.overlay = Some(Overlay::QuestionDashboard {
            question_id: qid,
            selected: 0,
            export_prompt: Some(BibExportPrompt::from_question(
                "x",
                scitadel_export::SnapshotFormat::BibTeX,
            )),
        });
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);

        match app.overlay {
            Some(Overlay::QuestionDashboard {
                export_prompt: None,
                ..
            }) => {}
            other => panic!("expected dashboard with prompt cleared, got {other:?}"),
        }
        assert!(app.status_toast.is_none(), "no toast on cancel");
    }
}
