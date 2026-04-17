use std::path::PathBuf;

use scitadel_adapters::download::{AccessStatus, DownloadFormat, PaperDownloader};
use scitadel_core::models::Paper;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Task {
    pub id: Uuid,
    pub kind: TaskKind,
    pub status: TaskStatus,
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
    },
    Failed(String),
}

#[derive(Debug)]
pub enum TaskUpdate {
    New(Task),
    Status { id: Uuid, status: TaskStatus },
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
            },
            Err(e) => TaskStatus::Failed(e.to_string()),
        };
        let _ = tx_bg.send(TaskUpdate::Status { id, status });
    });

    id
}
