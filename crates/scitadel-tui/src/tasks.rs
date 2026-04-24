use std::path::PathBuf;
use std::time::{Duration, Instant};

use scitadel_adapters::download::{AccessStatus, DownloadFormat, PaperDownloader};
use scitadel_core::models::Paper;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

/// How long a `Done` task lingers in the panel after completing before
/// it auto-flushes. Long enough for the user to see the result, short
/// enough that it doesn't crowd out new work.
pub const DONE_LINGER: Duration = Duration::from_secs(5);
/// `Failed` tasks linger longer so the user has time to read the error.
pub const FAILED_LINGER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct Task {
    pub id: Uuid,
    pub kind: TaskKind,
    pub status: TaskStatus,
    /// Wall-clock instant when the task entered a terminal state
    /// (`Done` / `Failed`). `None` while it's still `Queued` or
    /// `Running`. Drives the auto-flush policy in `App::flush_completed_tasks`.
    pub terminal_at: Option<Instant>,
}

#[derive(Debug, Clone)]
pub enum TaskKind {
    /// `ref_id` is the best identifier we have to show the user (DOI, arxiv id, or paper UUID prefix).
    /// `paper_id` lets the drain loop persist the outcome back to the
    /// `papers.download_status` column (#112).
    Download {
        paper_id: String,
        ref_id: String,
        title: String,
    },
    /// Spawn the OS file viewer for an already-downloaded paper (#144).
    /// Fire-and-forget: success surfaces as the viewer opening, so this
    /// task only sticks around long enough to flash the failure message
    /// when no local file exists or the spawn errored.
    OpenExternal {
        paper_id: String,
        ref_id: String,
        title: String,
    },
}

#[derive(Debug, Clone)]
pub enum TaskStatus {
    Queued,
    Running,
    Done {
        path: PathBuf,
        format: DownloadFormat,
        access: AccessStatus,
        /// Live publisher URL if the download went through one; used by the
        /// view to show a "try publisher" hint when the result is paywalled.
        publisher_url: Option<String>,
    },
    Failed(String),
}

#[derive(Debug)]
pub enum TaskUpdate {
    New(Task),
    Status { id: Uuid, status: TaskStatus },
}

/// Drop terminal-state tasks whose linger window has elapsed (#113).
/// Pure helper called from `App::flush_completed_tasks` so the policy
/// is unit-testable without constructing the full TUI.
pub fn retain_recent_terminal(tasks: &mut Vec<Task>, now: Instant) {
    tasks.retain(|t| match (&t.status, t.terminal_at) {
        (TaskStatus::Done { .. }, Some(at)) => now.duration_since(at) < DONE_LINGER,
        (TaskStatus::Failed(_), Some(at)) => now.duration_since(at) < FAILED_LINGER,
        _ => true,
    });
}

/// Drop every Done/Failed task immediately. Bound to `c` (#113).
pub fn clear_terminal(tasks: &mut Vec<Task>) {
    tasks.retain(|t| matches!(t.status, TaskStatus::Queued | TaskStatus::Running));
}

/// Fail-fast variant for when the TUI knows the network is down (#51).
/// Instead of spawning a download that will time out on reqwest after
/// ~60s, synthesize a task that transitions immediately to `Failed`
/// with a clear offline message. Returns the task id so callers can
/// reference it.
pub fn synthesize_offline_failure(tx: UnboundedSender<TaskUpdate>, paper: &Paper) -> Uuid {
    let id = Uuid::new_v4();
    let ref_id = paper
        .doi
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| paper.arxiv_id.clone().filter(|s| !s.is_empty()))
        .or_else(|| paper.openalex_id.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| paper.id.as_str().chars().take(8).collect());
    let task = Task {
        id,
        kind: TaskKind::Download {
            paper_id: paper.id.as_str().to_string(),
            ref_id,
            title: paper.title.clone(),
        },
        status: TaskStatus::Queued,
        terminal_at: None,
    };
    let _ = tx.send(TaskUpdate::New(task));
    let _ = tx.send(TaskUpdate::Status {
        id,
        status: TaskStatus::Failed(
            "offline: download requires network — retry after reconnecting".to_string(),
        ),
    });
    id
}

/// Spawn the OS default viewer for `path` and return a task tracking the
/// outcome (#144). Success is implicit (the viewer pops up), so we
/// transition to `Done` immediately with a synthetic path/access on the
/// task — only failures (no opener on PATH, exec errored) actually need
/// a visible row. `paper` provides the ref_id/title for the panel.
pub fn spawn_open_external(tx: UnboundedSender<TaskUpdate>, paper: &Paper, path: PathBuf) -> Uuid {
    let id = Uuid::new_v4();
    let ref_id = paper
        .doi
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| paper.arxiv_id.clone().filter(|s| !s.is_empty()))
        .or_else(|| paper.openalex_id.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| paper.id.as_str().chars().take(8).collect());
    let task = Task {
        id,
        kind: TaskKind::OpenExternal {
            paper_id: paper.id.as_str().to_string(),
            ref_id,
            title: paper.title.clone(),
        },
        status: TaskStatus::Queued,
        terminal_at: None,
    };
    let _ = tx.send(TaskUpdate::New(task));

    let status = match open_with_system_viewer(&path) {
        Ok(()) => TaskStatus::Done {
            path,
            format: DownloadFormat::Pdf,
            access: AccessStatus::FullText,
            publisher_url: None,
        },
        Err(e) => TaskStatus::Failed(e),
    };
    let _ = tx.send(TaskUpdate::Status { id, status });
    id
}

/// Cross-platform "open this file with the OS default app". Returns a
/// human-readable error string so failures can land in the task panel.
fn open_with_system_viewer(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("file not found: {}", path.display()));
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    cmd.arg(path);
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch viewer: {e}"))
}

/// Spawn a download for a full `Paper` (uses all available identifiers).
pub fn spawn_download_paper(
    tx: UnboundedSender<TaskUpdate>,
    paper: Paper,
    email: String,
    out_dir: PathBuf,
) -> Uuid {
    let id = Uuid::new_v4();
    let ref_id = paper
        .doi
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| paper.arxiv_id.clone().filter(|s| !s.is_empty()))
        .or_else(|| paper.openalex_id.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| paper.id.as_str().chars().take(8).collect());

    let task = Task {
        id,
        kind: TaskKind::Download {
            paper_id: paper.id.as_str().to_string(),
            ref_id,
            title: paper.title.clone(),
        },
        status: TaskStatus::Queued,
        terminal_at: None,
    };
    let _ = tx.send(TaskUpdate::New(task));

    let tx_bg = tx.clone();
    tokio::spawn(async move {
        let _ = tx_bg.send(TaskUpdate::Status {
            id,
            status: TaskStatus::Running,
        });

        let downloader = PaperDownloader::new(email, 60.0);
        let status = match downloader.download_paper(&paper, &out_dir).await {
            Ok(result) => TaskStatus::Done {
                path: result.path,
                format: result.format,
                access: result.access,
                publisher_url: result.publisher_url,
            },
            Err(e) => TaskStatus::Failed(e.to_string()),
        };
        let _ = tx_bg.send(TaskUpdate::Status { id, status });
    });

    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(status: TaskStatus, terminal_at: Option<Instant>) -> Task {
        Task {
            id: Uuid::new_v4(),
            kind: TaskKind::Download {
                paper_id: "p".into(),
                ref_id: "ref".into(),
                title: "title".into(),
            },
            status,
            terminal_at,
        }
    }

    fn done() -> TaskStatus {
        TaskStatus::Done {
            path: PathBuf::from("/tmp/x"),
            format: DownloadFormat::Pdf,
            access: AccessStatus::FullText,
            publisher_url: None,
        }
    }

    #[test]
    fn retain_keeps_active_tasks() {
        let now = Instant::now();
        let mut tasks = vec![
            t(TaskStatus::Queued, None),
            t(TaskStatus::Running, None),
            t(done(), Some(now)),
        ];
        retain_recent_terminal(&mut tasks, now);
        assert_eq!(
            tasks.len(),
            3,
            "fresh terminal task stays alongside actives"
        );
    }

    #[test]
    fn retain_drops_done_after_linger() {
        // Use a fake "now" that's well past Instant::now so subtraction is safe.
        let now = Instant::now() + Duration::from_mins(1);
        let stale = Instant::now();
        let mut tasks = vec![t(TaskStatus::Running, None), t(done(), Some(stale))];
        retain_recent_terminal(&mut tasks, now);
        assert_eq!(tasks.len(), 1, "stale Done task is flushed");
        assert!(matches!(tasks[0].status, TaskStatus::Running));
    }

    #[test]
    fn retain_keeps_failed_longer_than_done() {
        // 10s past mid: past DONE_LINGER (5s) but before FAILED_LINGER (30s).
        let mid = Instant::now();
        let now = mid + Duration::from_secs(10);
        let mut tasks = vec![
            t(done(), Some(mid)),
            t(TaskStatus::Failed("oops".into()), Some(mid)),
        ];
        retain_recent_terminal(&mut tasks, now);
        assert_eq!(tasks.len(), 1, "Done flushed but Failed stays");
        assert!(matches!(tasks[0].status, TaskStatus::Failed(_)));
    }

    #[test]
    fn clear_drops_all_terminal_keeps_active() {
        let now = Instant::now();
        let mut tasks = vec![
            t(TaskStatus::Queued, None),
            t(TaskStatus::Running, None),
            t(done(), Some(now)),
            t(TaskStatus::Failed("x".into()), Some(now)),
        ];
        clear_terminal(&mut tasks);
        assert_eq!(tasks.len(), 2);
        assert!(
            tasks
                .iter()
                .all(|t| matches!(t.status, TaskStatus::Queued | TaskStatus::Running))
        );
    }
}
