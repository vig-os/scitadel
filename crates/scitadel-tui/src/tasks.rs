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
    Download { ref_id: String, title: String },
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
