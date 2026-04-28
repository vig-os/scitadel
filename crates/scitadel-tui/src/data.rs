use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};

use scitadel_core::models::{
    Anchor, AnchorStatus, Annotation, Assessment, Paper, PaperId, ResearchQuestion, Search,
    SearchTerm,
};
use scitadel_core::ports::{
    AssessmentRepository, PaperRepository, QuestionRepository, SearchRepository,
};
use scitadel_db::sqlite::{
    Database, SqliteAnnotationRepository, SqlitePaperStateRepository, SqlitePaperTagRepository,
    SqliteShortlistRepository,
};

/// Wrapper around the database that loads data for each TUI view.
pub struct DataStore {
    db: Database,
}

impl DataStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        let db = Database::open(db_path).context("failed to open database")?;
        db.migrate().context("migration failed")?;
        Ok(Self { db })
    }

    pub fn load_searches(&self, limit: i64) -> Result<Vec<Search>> {
        let (_, search_repo, _, _, _) = self.db.repositories();
        Ok(search_repo.list_searches(limit)?)
    }

    pub fn load_papers(&self, limit: i64, offset: i64) -> Result<Vec<Paper>> {
        let (paper_repo, _, _, _, _) = self.db.repositories();
        Ok(paper_repo.list_all(limit, offset)?)
    }

    pub fn load_paper(&self, paper_id: &str) -> Result<Option<Paper>> {
        let (paper_repo, _, _, _, _) = self.db.repositories();
        Ok(paper_repo.get(paper_id)?)
    }

    pub fn load_papers_for_search(&self, search_id: &str) -> Result<Vec<Paper>> {
        let (paper_repo, search_repo, _, _, _) = self.db.repositories();
        let results = search_repo.get_results(search_id)?;
        let mut papers = Vec::new();
        for r in &results {
            if let Ok(Some(paper)) = paper_repo.get(r.paper_id.as_str()) {
                papers.push(paper);
            }
        }
        Ok(papers)
    }

    pub fn load_questions(&self) -> Result<Vec<ResearchQuestion>> {
        let (_, _, q_repo, _, _) = self.db.repositories();
        Ok(q_repo.list_questions()?)
    }

    /// Fetch a single research question by id. Used by the dashboard
    /// title bar (#133).
    pub fn load_question(&self, question_id: &str) -> Result<Option<ResearchQuestion>> {
        let (_, _, q_repo, _, _) = self.db.repositories();
        Ok(q_repo.get_question(question_id)?)
    }

    pub fn load_terms(&self, question_id: &str) -> Result<Vec<SearchTerm>> {
        let (_, _, q_repo, _, _) = self.db.repositories();
        Ok(q_repo.get_terms(question_id)?)
    }

    pub fn load_assessments_for_paper(
        &self,
        paper_id: &str,
        question_id: Option<&str>,
    ) -> Result<Vec<Assessment>> {
        let (_, _, _, a_repo, _) = self.db.repositories();
        Ok(a_repo.get_for_paper(paper_id, question_id)?)
    }

    /// Load the set of paper IDs this reader has starred.
    pub fn load_starred_ids(&self, reader: &str) -> Result<HashSet<String>> {
        Ok(SqlitePaperStateRepository::new(self.db.clone()).starred_ids(reader)?)
    }

    /// Load starred papers for this reader in most-recently-starred-first
    /// order, fully hydrated. Drives the Queue tab (#48) — a cross-
    /// search aggregator of starred papers across all questions.
    pub fn load_starred_papers(&self, reader: &str) -> Result<Vec<Paper>> {
        let ids = SqlitePaperStateRepository::new(self.db.clone()).starred_ids_ordered(reader)?;
        let (paper_repo, _, _, _, _) = self.db.repositories();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(Some(p)) = paper_repo.get(&id) {
                out.push(p);
            }
        }
        Ok(out)
    }

    /// Load live annotations for a paper (roots + replies), ordered oldest-first.
    pub fn load_annotations_for_paper(&self, paper_id: &str) -> Result<Vec<Annotation>> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).list_by_paper(paper_id)?)
    }

    /// Total annotations `reader` hasn't acknowledged across all papers.
    /// Drives the status-bar `[N new]` badge — called every TUI draw,
    /// so it goes through the SQL `COUNT(*)` rather than materialising
    /// the rows. (#185)
    pub fn load_unread_count(&self, reader: &str) -> Result<i64> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).count_unread(reader, None)?)
    }

    /// Annotations `reader` hasn't acknowledged on a specific paper.
    /// Used by the per-row `●` glyph in the Papers list and the
    /// `[unread]` markers in the reader-view thread pane. (#185)
    pub fn load_unread_for_paper(&self, reader: &str, paper_id: &str) -> Result<Vec<Annotation>> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).list_unread(reader, Some(paper_id))?)
    }

    /// Set of paper IDs `reader` has at least one unread annotation
    /// on. Drives the per-row `●` glyph in the Papers list. (#185)
    pub fn load_papers_with_unread(&self, reader: &str) -> Result<HashSet<String>> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).papers_with_unread(reader)?)
    }

    /// Every unread annotation across all papers for `reader`,
    /// oldest-first per the underlying `list_unread` query. Used by
    /// the inbox overlay (#185 P0) which then groups by paper for
    /// display.
    pub fn load_all_unread(&self, reader: &str) -> Result<Vec<Annotation>> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).list_unread(reader, None)?)
    }

    /// Mark a thread (root + replies) as seen by `reader`. Called from
    /// the TUI on focus-leave / overlay-close so the badge and
    /// `[unread]` markers clear without a manual action. (#185)
    pub fn mark_thread_seen(&self, reader: &str, root_id: &str) -> Result<()> {
        SqliteAnnotationRepository::new(self.db.clone()).mark_thread_seen(root_id, reader)?;
        Ok(())
    }

    /// Toggle the starred flag for a paper and return the new value.
    pub fn toggle_starred(&self, paper_id: &str, reader: &str) -> Result<bool> {
        Ok(SqlitePaperStateRepository::new(self.db.clone()).toggle_starred(paper_id, reader)?)
    }

    /// Create a root annotation anchored to a quoted passage. Anchors
    /// from the TUI carry only the quote (no offsets), so the resolver
    /// will substring-match on next paper-open.
    pub fn create_root_annotation(
        &self,
        paper_id: &str,
        quote: &str,
        note: &str,
        author: &str,
    ) -> Result<String> {
        let anchor = Anchor {
            quote: Some(quote.to_string()),
            status: AnchorStatus::Ok,
            ..Anchor::default()
        };
        let ann = Annotation::new_root(
            PaperId::from(paper_id),
            author.to_string(),
            note.to_string(),
            anchor,
        );
        SqliteAnnotationRepository::new(self.db.clone()).create(&ann)?;
        Ok(ann.id.as_str().to_string())
    }

    /// Reply to an existing annotation; inherits paper_id + question_id
    /// from the parent.
    pub fn reply_annotation(&self, parent_id: &str, note: &str, author: &str) -> Result<String> {
        let repo = SqliteAnnotationRepository::new(self.db.clone());
        let parent = repo
            .get(parent_id)?
            .with_context(|| format!("annotation {parent_id} not found"))?;
        let reply = Annotation::new_reply(&parent, author.to_string(), note.to_string());
        repo.create(&reply)?;
        Ok(reply.id.as_str().to_string())
    }

    /// Update the note body of an existing annotation; preserves color +
    /// tags as-is.
    pub fn update_annotation_note(&self, annotation_id: &str, note: &str) -> Result<()> {
        let repo = SqliteAnnotationRepository::new(self.db.clone());
        let existing = repo
            .get(annotation_id)?
            .with_context(|| format!("annotation {annotation_id} not found"))?;
        repo.update_note(
            annotation_id,
            note,
            existing.color.as_deref(),
            &existing.tags,
        )?;
        Ok(())
    }

    /// Soft-delete an annotation (tombstone — replies stay readable).
    pub fn delete_annotation(&self, annotation_id: &str) -> Result<()> {
        Ok(SqliteAnnotationRepository::new(self.db.clone()).soft_delete(annotation_id)?)
    }

    /// Load a question's papers ranked by assessment score (DESC) for
    /// the Question Dashboard (#133). Returns `(Paper, Option<Assessment>)`
    /// so the dashboard can show both metadata and the LLM score +
    /// rationale in one render pass. Papers without an assessment
    /// sort to the end.
    pub fn load_question_dashboard(
        &self,
        question_id: &str,
    ) -> Result<Vec<(Paper, Option<Assessment>)>> {
        let (paper_repo, _, _, a_repo, _) = self.db.repositories();
        let assessments = a_repo.get_for_question(question_id)?;
        let mut out: Vec<(Paper, Option<Assessment>)> = Vec::with_capacity(assessments.len());
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for a in assessments {
            let pid = a.paper_id.as_str().to_string();
            if seen.contains(&pid) {
                continue;
            }
            if let Ok(Some(paper)) = paper_repo.get(&pid) {
                seen.insert(pid);
                out.push((paper, Some(a)));
            }
        }
        // Sort by score DESC, then by paper_id for tie stability.
        out.sort_by(|a, b| {
            let sa = a.1.as_ref().map_or(0.0, |x| x.score);
            let sb = b.1.as_ref().map_or(0.0, |x| x.score);
            sb.partial_cmp(&sa)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.id.as_str().cmp(b.0.id.as_str()))
        });
        Ok(out)
    }

    /// Toggle a paper's shortlist membership for the current reader.
    /// Returns the post-toggle state (true = on shortlist). (#133)
    pub fn toggle_shortlist(
        &self,
        question_id: &str,
        paper_id: &str,
        reader: &str,
    ) -> Result<bool> {
        Ok(
            scitadel_db::sqlite::SqliteShortlistRepository::new(self.db.clone()).toggle(
                question_id,
                paper_id,
                reader,
            )?,
        )
    }

    /// Members of `reader`'s shortlist for `question_id` as a set.
    /// Used by the dashboard render to mark shortlisted rows. (#133)
    pub fn load_shortlist_set(
        &self,
        question_id: &str,
        reader: &str,
    ) -> Result<std::collections::HashSet<String>> {
        Ok(
            scitadel_db::sqlite::SqliteShortlistRepository::new(self.db.clone())
                .members_set(question_id, reader)?,
        )
    }

    /// Persist the outcome of a download attempt for the Papers-table
    /// state column (#112). `local_path` is the absolute path to the
    /// saved file on success, `None` on failure.
    pub fn record_download_outcome(
        &self,
        paper_id: &str,
        local_path: Option<&str>,
        status: scitadel_core::models::DownloadStatus,
    ) -> Result<()> {
        let (paper_repo, _, _, _, _) = self.db.repositories();
        Ok(paper_repo.update_download_state(paper_id, local_path, status)?)
    }

    /// Persist the TUI's current selection so an MCP-side agent can
    /// query it (#122).
    pub fn publish_tui_state(&self, state: &scitadel_db::sqlite::TuiState) -> Result<()> {
        Ok(scitadel_db::sqlite::SqliteTuiStateRepository::new(self.db.clone()).set(state)?)
    }

    /// Test-only: construct a `DataStore` around an in-memory DB so
    /// unread/mark-seen plumbing can be exercised without a temp dir.
    #[cfg(test)]
    fn for_tests() -> Self {
        let db = Database::open_in_memory().expect("open in-memory db");
        db.migrate().expect("migrate");
        Self { db }
    }

    /// Load the inputs needed by `scitadel_export::write_snapshot` for a
    /// question + reader (#135 sub-feature B). Returns the shortlist's
    /// paper IDs in DB order, the hydrated `Paper` rows, and a
    /// `paper_id → tags` map. Mirrors `load_shortlist` in `scitadel-cli`
    /// so the CLI and TUI surfaces produce byte-identical snapshots.
    pub fn load_snapshot_inputs(
        &self,
        question_id: &str,
        reader: &str,
    ) -> Result<(
        Vec<String>,
        Vec<scitadel_core::models::Paper>,
        std::collections::HashMap<String, Vec<String>>,
    )> {
        let (paper_repo, _, _, _, _) = self.db.repositories();
        let shortlist = SqliteShortlistRepository::new(self.db.clone());
        let tag_repo = SqlitePaperTagRepository::new(self.db.clone());

        let paper_ids = shortlist
            .list(question_id, reader)
            .context("failed to read shortlist")?;
        let papers: Vec<_> = paper_ids
            .iter()
            .filter_map(|id| paper_repo.get(id).ok().flatten())
            .collect();
        let mut tags = std::collections::HashMap::new();
        for id in &paper_ids {
            let t = tag_repo.tags_for(id).unwrap_or_default();
            tags.insert(id.clone(), t);
        }
        Ok((paper_ids, papers, tags))
    }
}

#[cfg(test)]
mod tests {
    use super::DataStore;
    use scitadel_core::models::{Anchor, Paper, PaperId};
    use scitadel_core::ports::PaperRepository;

    fn save_paper(store: &DataStore, id: &str, title: &str) {
        let (paper_repo, _, _, _, _) = store.db.repositories();
        let mut p = Paper::new(title);
        p.id = PaperId::from(id);
        paper_repo.save(&p).unwrap();
    }

    #[test]
    fn unread_count_zero_on_empty_db() {
        let store = DataStore::for_tests();
        assert_eq!(store.load_unread_count("lars").unwrap(), 0);
    }

    #[test]
    fn unread_count_counts_root_and_replies_then_clears_after_mark_thread_seen() {
        use scitadel_core::models::Annotation;
        use scitadel_db::sqlite::SqliteAnnotationRepository;

        let store = DataStore::for_tests();
        save_paper(&store, "p-1", "T");
        let repo = SqliteAnnotationRepository::new(store.db.clone());
        let root = Annotation::new_root(
            PaperId::from("p-1"),
            "claude".into(),
            "claim".into(),
            Anchor {
                quote: Some("Q".into()),
                ..Anchor::default()
            },
        );
        repo.create(&root).unwrap();
        let reply = Annotation::new_reply(&root, "claude".into(), "follow".into());
        repo.create(&reply).unwrap();

        assert_eq!(store.load_unread_count("lars").unwrap(), 2);
        assert_eq!(store.load_unread_for_paper("lars", "p-1").unwrap().len(), 2);

        store.mark_thread_seen("lars", root.id.as_str()).unwrap();
        assert_eq!(store.load_unread_count("lars").unwrap(), 0);
        assert!(
            store
                .load_unread_for_paper("lars", "p-1")
                .unwrap()
                .is_empty()
        );

        // Other reader still sees them as unread.
        assert_eq!(store.load_unread_count("alice").unwrap(), 2);
    }

    #[test]
    fn unread_for_paper_scopes_correctly() {
        use scitadel_core::models::Annotation;
        use scitadel_db::sqlite::SqliteAnnotationRepository;

        let store = DataStore::for_tests();
        save_paper(&store, "p-a", "A");
        save_paper(&store, "p-b", "B");
        let repo = SqliteAnnotationRepository::new(store.db.clone());
        let on_a = Annotation::new_root(
            PaperId::from("p-a"),
            "claude".into(),
            "a-note".into(),
            Anchor {
                quote: Some("q".into()),
                ..Anchor::default()
            },
        );
        let on_b = Annotation::new_root(
            PaperId::from("p-b"),
            "claude".into(),
            "b-note".into(),
            Anchor {
                quote: Some("q".into()),
                ..Anchor::default()
            },
        );
        repo.create(&on_a).unwrap();
        repo.create(&on_b).unwrap();

        assert_eq!(store.load_unread_for_paper("lars", "p-a").unwrap().len(), 1);
        assert_eq!(store.load_unread_for_paper("lars", "p-b").unwrap().len(), 1);
        assert_eq!(store.load_unread_count("lars").unwrap(), 2);
    }
}
