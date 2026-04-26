//! `bib watch` engine (#134 PR-C).
//!
//! Two-layer design so the gnarly bits stay testable without an
//! async runtime:
//!
//! 1. **`WatchEngine`** (this module, sync, pure-ish) — owns the
//!    decision state. Caller feeds it observed `MAX(updated_at)`
//!    timestamps and rebuilt-bib hashes; it answers "should I
//!    rebuild now?" and "should I write this hash?". Time is
//!    injected as `Instant` so tests don't sleep.
//!
//! 2. **`run_watch_loop`** (async, separate fn below) — the
//!    actual tokio loop that polls SQLite, calls into the engine,
//!    builds bib content, writes the file, and listens for SIGTERM.
//!
//! The pure layer is what guards the 🔴 issue pitfall: a Typst
//! recompile that triggers a downstream DB write must NOT cause
//! `bib watch` to spin in a write-loop. Hash-and-skip + debounce
//! are checked by unit tests with an adversarial scenario; no I/O
//! needed.

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Minimum-viable state the watch loop has to remember between ticks.
#[derive(Debug, Clone, Default)]
pub struct WatchState {
    /// Last `MAX(updated_at)` value observed. RFC3339 strings are
    /// lex-comparable per ISO 8601 so we keep them as `String` and
    /// avoid a parse on every tick.
    pub last_seen_ts: Option<String>,
    /// SHA-256 of the last-written `.bib` content. `None` until the
    /// first successful write.
    pub last_hash: Option<[u8; 32]>,
    /// `Some(t)` when a change was observed but the debounce window
    /// hasn't elapsed yet — used to coalesce bursts of edits into a
    /// single write.
    pub pending_since: Option<Instant>,
}

/// What the engine wants the caller to do next. The caller does the
/// I/O; the engine just decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextAction {
    /// Nothing changed since the last tick. Sleep a poll interval
    /// and check again.
    Idle,
    /// A change was observed; the debounce window is starting (or
    /// already running). Don't rebuild yet.
    PendingDebounce,
    /// The debounce window has elapsed since the most recent
    /// observed change. Caller should rebuild the bib content,
    /// hash it, and call [`WatchEngine::record_rebuild`].
    Rebuild,
}

/// What to do with a freshly-rebuilt hash. The engine compares it
/// against `last_hash` so the caller can decide between writing the
/// file (`Write`) or skipping the I/O (`Skip`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteDecision {
    /// Hash differs from last write — caller should `fs::write` and
    /// `record_write` to update internal state.
    Write,
    /// Hash matches last write — caller should NOT touch the file.
    /// `pending_since` is cleared internally so the engine returns
    /// `Idle` on the next observe call.
    Skip,
}

/// Pure decision engine. No clock, no SQLite, no filesystem — only
/// state transitions driven by external observations.
#[derive(Debug, Clone)]
pub struct WatchEngine {
    pub state: WatchState,
    pub debounce: Duration,
}

impl WatchEngine {
    pub fn new(debounce: Duration) -> Self {
        Self {
            state: WatchState::default(),
            debounce,
        }
    }

    /// Feed in the latest `MAX(updated_at)` for the watched question
    /// and the current monotonic instant. Updates `last_seen_ts` and
    /// `pending_since` and returns the action the orchestrator should
    /// take.
    pub fn observe(&mut self, current_ts: Option<&str>, now: Instant) -> NextAction {
        let changed = match (self.state.last_seen_ts.as_deref(), current_ts) {
            (Some(prev), Some(curr)) => prev != curr,
            (None, Some(_)) => true,
            // No data yet — nothing to watch. Stay idle until rows show up.
            (_, None) => false,
        };

        if changed {
            // Always update the seen timestamp to the latest, but keep
            // `pending_since` anchored at the FIRST change in the
            // window — that's what makes the debounce a coalescing
            // window rather than a moving target that bursts of
            // writes can keep pushing forward.
            self.state.last_seen_ts = current_ts.map(str::to_string);
            if self.state.pending_since.is_none() {
                self.state.pending_since = Some(now);
            }
        }

        match self.state.pending_since {
            None => NextAction::Idle,
            Some(start) if now.saturating_duration_since(start) >= self.debounce => {
                NextAction::Rebuild
            }
            Some(_) => NextAction::PendingDebounce,
        }
    }

    /// Caller has rebuilt the bib content; hand back the SHA-256.
    /// Engine compares against `last_hash` and tells the caller
    /// whether to write or skip. Either path clears `pending_since`.
    pub fn record_rebuild(&mut self, new_hash: [u8; 32]) -> WriteDecision {
        self.state.pending_since = None;
        match self.state.last_hash {
            Some(prev) if prev == new_hash => WriteDecision::Skip,
            _ => {
                self.state.last_hash = Some(new_hash);
                WriteDecision::Write
            }
        }
    }

    /// True when the engine has observed a change that the caller
    /// hasn't acted on yet. Used by the SIGTERM path to flush a
    /// final snapshot before exit.
    pub fn has_pending(&self) -> bool {
        self.state.pending_since.is_some()
    }
}

/// Compute the SHA-256 of the rendered bib content.
pub fn hash_bib(content: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    h.finalize().into()
}

// ---------- async orchestrator ----------

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use scitadel_core::error::CoreError;
use scitadel_core::ports::{AssessmentRepository, PaperRepository};
use scitadel_db::sqlite::{
    SqliteAssessmentRepository, SqlitePaperRepository, SqliteShortlistRepository,
};
use scitadel_export::export_bibtex;

/// Configuration for one `scitadel bib watch` run.
#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub question_id: String,
    pub reader: String,
    pub output: std::path::PathBuf,
    pub debounce: Duration,
    pub poll_interval: Duration,
    pub min_score: Option<f64>,
}

/// Run the watch loop until `shutdown` flips to true (typically by a
/// SIGTERM handler installed by the CLI). On shutdown, if a pending
/// change has not been written, do one final flush so the user
/// doesn't lose the in-flight edit.
pub async fn run_watch_loop(
    options: WatchOptions,
    papers: SqlitePaperRepository,
    assessments: SqliteAssessmentRepository,
    shortlist: SqliteShortlistRepository,
    shutdown: Arc<AtomicBool>,
) -> Result<(), CoreError> {
    let mut engine = WatchEngine::new(options.debounce);

    while !shutdown.load(Ordering::SeqCst) {
        let now = Instant::now();
        let current_ts = shortlist
            .max_updated_at_for_question(&options.question_id)
            .map_err(CoreError::from)?;
        let action = engine.observe(current_ts.as_deref(), now);

        match action {
            NextAction::Idle | NextAction::PendingDebounce => {
                tokio::time::sleep(options.poll_interval).await;
            }
            NextAction::Rebuild => {
                rebuild_and_maybe_write(&options, &papers, &assessments, &shortlist, &mut engine)?;
                tokio::time::sleep(options.poll_interval).await;
            }
        }
    }

    // Final flush on shutdown — only if there's a pending observation
    // we haven't acted on yet. Avoids the "user pressed Ctrl-C two
    // seconds after editing" data-loss footgun.
    if engine.has_pending() {
        rebuild_and_maybe_write(&options, &papers, &assessments, &shortlist, &mut engine)?;
    }
    Ok(())
}

fn rebuild_and_maybe_write(
    options: &WatchOptions,
    papers: &SqlitePaperRepository,
    assessments: &SqliteAssessmentRepository,
    shortlist: &SqliteShortlistRepository,
    engine: &mut WatchEngine,
) -> Result<(), CoreError> {
    let content = build_question_bib(options, papers, assessments, shortlist)?;
    let new_hash = hash_bib(&content);
    match engine.record_rebuild(new_hash) {
        WriteDecision::Skip => Ok(()),
        WriteDecision::Write => {
            std::fs::write(&options.output, &content)?;
            tracing::info!(
                op = "bib_watch_write",
                question_id = %options.question_id,
                output = %options.output.display(),
                bytes = content.len(),
                "wrote bib snapshot",
            );
            Ok(())
        }
    }
}

/// Build the bib content for `(question_id, reader)`'s shortlist,
/// filtered by `min_score` if set. Pure-ish: runs DB reads but no
/// I/O on the watch's output path. Exposed for both the watch loop
/// and adversarial tests.
pub fn build_question_bib(
    options: &WatchOptions,
    papers: &SqlitePaperRepository,
    assessments: &SqliteAssessmentRepository,
    shortlist: &SqliteShortlistRepository,
) -> Result<String, CoreError> {
    let paper_ids = shortlist
        .list(&options.question_id, &options.reader)
        .map_err(CoreError::from)?;

    let mut selected: Vec<scitadel_core::models::Paper> = Vec::with_capacity(paper_ids.len());
    for pid in &paper_ids {
        if let Some(paper) = papers.get(pid)? {
            if let Some(min) = options.min_score {
                let mut max_score: Option<f64> = None;
                for a in assessments.get_for_paper(pid, Some(&options.question_id))? {
                    max_score = Some(max_score.map_or(a.score, |m| m.max(a.score)));
                }
                if max_score.unwrap_or(f64::NEG_INFINITY) < min {
                    continue;
                }
            }
            selected.push(paper);
        }
    }
    Ok(export_bibtex(&selected))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dur_ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn no_changes_stays_idle() {
        let mut e = WatchEngine::new(dur_ms(300));
        let t0 = Instant::now();
        assert_eq!(
            e.observe(Some("2026-04-25T10:00:00Z"), t0),
            NextAction::PendingDebounce
        );
        // Same timestamp again should NOT extend the window — first
        // observation already started it. After debounce: rebuild.
        let later = t0 + dur_ms(350);
        assert_eq!(
            e.observe(Some("2026-04-25T10:00:00Z"), later),
            NextAction::Rebuild
        );
    }

    #[test]
    fn debounce_coalesces_bursts() {
        let mut e = WatchEngine::new(dur_ms(300));
        let t0 = Instant::now();
        assert_eq!(e.observe(Some("ts1"), t0), NextAction::PendingDebounce);
        // Second change inside the window must NOT reset pending_since.
        let t1 = t0 + dur_ms(100);
        assert_eq!(e.observe(Some("ts2"), t1), NextAction::PendingDebounce);
        let t2 = t0 + dur_ms(200);
        assert_eq!(e.observe(Some("ts3"), t2), NextAction::PendingDebounce);
        // After the original-window expires (t0 + 300ms), rebuild fires
        // even though the latest change was seen at t2.
        let t3 = t0 + dur_ms(310);
        assert_eq!(e.observe(Some("ts3"), t3), NextAction::Rebuild);
    }

    #[test]
    fn record_rebuild_writes_when_hash_changes() {
        let mut e = WatchEngine::new(dur_ms(300));
        e.observe(Some("ts"), Instant::now());
        let h1 = hash_bib("@article{a,...}");
        assert_eq!(e.record_rebuild(h1), WriteDecision::Write);
        assert_eq!(e.state.last_hash, Some(h1));
        assert!(!e.has_pending(), "rebuild clears pending");
    }

    /// 🔴 issue pitfall: a Typst recompile (or any downstream
    /// process) that triggers a paper-row `updated_at` bump on every
    /// file-written event must NOT cause `bib watch` to spin in a
    /// write-loop. Hash-and-skip catches this — the rebuilt content
    /// is byte-identical, the hash matches, the file isn't touched.
    #[test]
    fn tight_loop_is_broken_by_hash_skip() {
        let mut e = WatchEngine::new(dur_ms(300));
        let bib_content = "@article{a, title = {Same}, year = {2024}}\n";
        let h = hash_bib(bib_content);

        // Iteration 1: timestamp moves, debounce elapses, write happens.
        let t0 = Instant::now();
        assert_eq!(e.observe(Some("t1"), t0), NextAction::PendingDebounce);
        let t1 = t0 + dur_ms(310);
        assert_eq!(e.observe(Some("t2"), t1), NextAction::Rebuild);
        assert_eq!(e.record_rebuild(h), WriteDecision::Write);

        // Iterations 2..N: simulate the adversarial loop — every
        // tick the DB ts moves but the bib content is byte-
        // identical (e.g. an unrelated `paper_state` write). Engine
        // signals Rebuild but record_rebuild returns Skip every time.
        let mut now = t1;
        let mut writes = 1;
        let mut skips = 0;
        for i in 0..50 {
            now += dur_ms(50);
            let _ = e.observe(Some(&format!("ts-{i}")), now);
            now += dur_ms(310);
            assert_eq!(
                e.observe(Some(&format!("ts-{i}-late")), now),
                NextAction::Rebuild
            );
            match e.record_rebuild(h) {
                WriteDecision::Write => writes += 1,
                WriteDecision::Skip => skips += 1,
            }
        }
        assert_eq!(writes, 1, "tight loop must produce only the initial write");
        assert_eq!(skips, 50, "every subsequent rebuild is a hash-skip no-op");
    }

    #[test]
    fn recovers_when_content_actually_changes_after_skip_streak() {
        // Even after a skip streak, a real content change still
        // produces a Write — hash-and-skip doesn't latch.
        let mut e = WatchEngine::new(dur_ms(300));
        let h_a = hash_bib("A");
        let h_b = hash_bib("B");

        let t0 = Instant::now();
        e.observe(Some("ts1"), t0);
        let t1 = t0 + dur_ms(310);
        e.observe(Some("ts2"), t1);
        assert_eq!(e.record_rebuild(h_a), WriteDecision::Write);

        let t2 = t1 + dur_ms(50);
        e.observe(Some("ts3"), t2);
        let t3 = t2 + dur_ms(310);
        e.observe(Some("ts4"), t3);
        assert_eq!(e.record_rebuild(h_a), WriteDecision::Skip, "same content");

        let t4 = t3 + dur_ms(50);
        e.observe(Some("ts5"), t4);
        let t5 = t4 + dur_ms(310);
        e.observe(Some("ts6"), t5);
        assert_eq!(
            e.record_rebuild(h_b),
            WriteDecision::Write,
            "real change after a skip streak still writes"
        );
    }

    #[test]
    fn no_rows_yet_stays_idle() {
        let mut e = WatchEngine::new(dur_ms(300));
        // SQLite returned None (empty shortlist / no rows).
        assert_eq!(e.observe(None, Instant::now()), NextAction::Idle);
        assert_eq!(e.state.last_seen_ts, None);
        assert!(!e.has_pending());
    }

    #[test]
    fn first_observation_marks_pending_immediately() {
        let mut e = WatchEngine::new(dur_ms(300));
        let t = Instant::now();
        assert_eq!(e.observe(Some("ts1"), t), NextAction::PendingDebounce);
        assert!(e.has_pending());
    }

    #[test]
    fn hash_bib_is_deterministic_and_distinct() {
        assert_eq!(hash_bib("foo"), hash_bib("foo"));
        assert_ne!(hash_bib("foo"), hash_bib("bar"));
        // Sanity: matches a known SHA-256 prefix for "foo".
        let h = hash_bib("foo");
        assert_eq!(h[0], 0x2c);
        assert_eq!(h[1], 0x26);
    }

    // ---------- async orchestrator integration ----------

    use scitadel_core::models::{Paper, ResearchQuestion};
    use scitadel_core::ports::QuestionRepository;
    use scitadel_db::sqlite::{Database, SqliteQuestionRepository};

    fn fresh_db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        db
    }

    fn seed_question_and_papers(db: &Database, question_id: &str, reader: &str, n_papers: usize) {
        let papers = SqlitePaperRepository::new(db.clone());
        let questions = SqliteQuestionRepository::new(db.clone());
        let shortlist = SqliteShortlistRepository::new(db.clone());

        let q = ResearchQuestion {
            id: scitadel_core::models::QuestionId::from(question_id),
            text: "Test question".into(),
            description: String::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        questions.save_question(&q).unwrap();

        for i in 0..n_papers {
            let mut p = Paper::new(format!("Paper {i}"));
            p.authors = vec![format!("Author{i}, A.")];
            p.year = Some(2020 + i as i32);
            p.doi = Some(format!("10.1/p{i}"));
            papers.save(&p).unwrap();
            shortlist
                .toggle(question_id, p.id.as_str(), reader)
                .unwrap();
        }
    }

    fn watch_options(output: std::path::PathBuf, question_id: &str, reader: &str) -> WatchOptions {
        WatchOptions {
            question_id: question_id.into(),
            reader: reader.into(),
            output,
            debounce: dur_ms(50),
            poll_interval: dur_ms(20),
            min_score: None,
        }
    }

    /// End-to-end happy path: seed → run loop → file appears with
    /// expected content → shutdown flushes.
    #[tokio::test(flavor = "current_thread")]
    async fn watch_loop_writes_initial_snapshot_then_shuts_down_cleanly() {
        let db = fresh_db();
        seed_question_and_papers(&db, "q1", "lars", 2);
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("paper.bib");

        let papers = SqlitePaperRepository::new(db.clone());
        let assessments = SqliteAssessmentRepository::new(db.clone());
        let shortlist = SqliteShortlistRepository::new(db);
        let opts = watch_options(output.clone(), "q1", "lars");

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let task = tokio::spawn(async move {
            run_watch_loop(opts, papers, assessments, shortlist, shutdown_flag).await
        });

        // Wait for the first write — the loop's debounce + poll add up
        // to ~70ms, so 500ms is generous.
        for _ in 0..50 {
            if output.exists() {
                break;
            }
            tokio::time::sleep(dur_ms(20)).await;
        }
        assert!(
            output.exists(),
            "watch should have written initial snapshot"
        );
        let written = std::fs::read_to_string(&output).unwrap();
        assert!(written.contains("@article"), "got: {written}");
        assert!(
            written.contains("Paper 0") || written.contains("paper 0"),
            "{written}"
        );

        shutdown.store(true, Ordering::SeqCst);
        let res = tokio::time::timeout(dur_ms(500), task).await;
        assert!(res.is_ok(), "loop should exit promptly on shutdown");
    }

    /// 🔴 issue pitfall, end-to-end: simulate a Typst recompile loop
    /// that bumps `paper_state` on every iteration. The bib content
    /// stays byte-identical. Expectation: the output file's mtime
    /// stops changing after the initial write.
    #[tokio::test(flavor = "current_thread")]
    async fn adversarial_paper_state_churn_does_not_thrash_output() {
        let db = fresh_db();
        seed_question_and_papers(&db, "q1", "lars", 2);
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("paper.bib");

        let papers = SqlitePaperRepository::new(db.clone());
        let assessments = SqliteAssessmentRepository::new(db.clone());
        let shortlist = SqliteShortlistRepository::new(db.clone());
        let opts = watch_options(output.clone(), "q1", "lars");

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let task = tokio::spawn(async move {
            run_watch_loop(opts, papers, assessments, shortlist, shutdown_flag).await
        });

        // Wait for the initial write.
        for _ in 0..50 {
            if output.exists() {
                break;
            }
            tokio::time::sleep(dur_ms(20)).await;
        }
        assert!(output.exists());
        let mtime_initial = std::fs::metadata(&output).unwrap().modified().unwrap();
        let bytes_initial = std::fs::read(&output).unwrap();

        // Adversarial churn: bump `paper_state` rows directly so
        // shortlist.max_updated_at_for_question changes on every
        // iteration, but the rendered bib stays identical (paper
        // metadata fields the export reads aren't touched).
        let paper_state_repo = scitadel_db::sqlite::SqlitePaperStateRepository::new(db.clone());
        // Find a paper_id from the shortlist.
        let pids = SqliteShortlistRepository::new(db.clone())
            .list("q1", "lars")
            .unwrap();
        let target_pid = pids.first().expect("at least one paper").clone();

        for i in 0..20 {
            // Bumping `to_read` just to move `paper_state.updated_at`.
            // The bib export doesn't read paper_state, so content is
            // byte-identical across these writes.
            let state = scitadel_db::sqlite::PaperState {
                paper_id: target_pid.clone(),
                reader: "lars".into(),
                starred: false,
                to_read: i % 2 == 0,
                read_at: None,
            };
            let _ = paper_state_repo.set(&state);
            tokio::time::sleep(dur_ms(15)).await;
        }
        // Let the loop finish processing the last burst.
        tokio::time::sleep(dur_ms(150)).await;

        let mtime_after = std::fs::metadata(&output).unwrap().modified().unwrap();
        let bytes_after = std::fs::read(&output).unwrap();
        assert_eq!(
            bytes_initial, bytes_after,
            "content must not change under churn"
        );
        assert_eq!(
            mtime_initial, mtime_after,
            "hash-and-skip must prevent the file from being rewritten"
        );

        shutdown.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(dur_ms(500), task).await;
    }

    /// SIGTERM path: shutdown flag flips while the engine has a
    /// pending observation; the loop must flush before exiting.
    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_flushes_pending_change() {
        let db = fresh_db();
        seed_question_and_papers(&db, "q1", "lars", 1);
        let tmp = tempfile::tempdir().unwrap();
        let output = tmp.path().join("paper.bib");

        let papers = SqlitePaperRepository::new(db.clone());
        let assessments = SqliteAssessmentRepository::new(db.clone());
        let shortlist = SqliteShortlistRepository::new(db);
        // Long debounce: ensures the loop is in PendingDebounce
        // state when we trigger shutdown.
        let opts = WatchOptions {
            question_id: "q1".into(),
            reader: "lars".into(),
            output: output.clone(),
            debounce: Duration::from_secs(5),
            poll_interval: dur_ms(20),
            min_score: None,
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let task = tokio::spawn(async move {
            run_watch_loop(opts, papers, assessments, shortlist, shutdown_flag).await
        });

        // Give the loop time to register the pending state.
        tokio::time::sleep(dur_ms(80)).await;
        assert!(!output.exists(), "shouldn't write yet — debounce is 5s");

        // SIGTERM: shutdown should flush the pending change.
        shutdown.store(true, Ordering::SeqCst);
        let _ = tokio::time::timeout(dur_ms(500), task).await;
        assert!(
            output.exists(),
            "SIGTERM must flush pending change before exit"
        );
    }
}
