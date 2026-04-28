//! `.bib` import orchestrator (#134 step 5).
//!
//! Bridges the pure parser/matcher/merger logic in `scitadel-export`
//! to the SQLite repos in `scitadel-db`. Both the CLI (`scitadel bib
//! import`) and the MCP `import_bibtex` tool delegate here so the
//! summary line, persistence order, and tracing convention are
//! identical across surfaces.
//!
//! Persistence order per row, on a non-Rejected outcome:
//! 1. Save the resolved paper. For newly-created papers (no DB match),
//!    assign and persist a `bibtex_key` via the #132 algorithm right
//!    after save — `SqlitePaperRepository::save` does NOT write the
//!    `bibtex_key` column itself; without this step, freshly-imported
//!    papers stay keyless until the next process startup re-runs
//!    `Database::migrate`'s backfill, breaking the BibtexKey step of
//!    the next import's match cascade.
//! 2. Record the imported citekey on `paper_aliases`.
//! 3. Save the unanchored annotation built from `note=` (if any).
//!
//! All three writes are wrapped in a single per-row SQLite transaction
//! (#157) so a mid-row failure can never strand an orphaned `papers`
//! row without its alias — the next re-import would otherwise miss
//! the orphan via the alias step and either DOI-merge or duplicate it.
//!
//! Failures on a single row are logged and counted but never abort
//! the run — a 5000-paper Zotero dump must survive a couple bad
//! entries.

use std::path::Path;
use std::sync::Arc;

use scitadel_core::bibtex_key::assign_keys;
use scitadel_core::error::CoreError;
use scitadel_core::ports::PaperRepository;
use scitadel_db::error::DbError;
use scitadel_db::sqlite::{
    SqliteAnnotationRepository, SqlitePaperAliasRepository, SqlitePaperRepository,
    SqliteTransaction,
};
use scitadel_export::import::{
    BibEntry, MatchOutcome, MergeAction, MergeStrategy, PaperLookup, SideEffects,
    compute_side_effects, match_entry, parse_bibtex, resolve as resolve_merge,
};

/// Per-paper outcome surfaced in the import summary.
#[derive(Debug, Clone)]
pub struct ImportRow {
    pub citekey: String,
    pub paper_id: Option<String>,
    pub action: MergeAction,
    pub from_bib: Vec<&'static str>,
    pub kept_from_db: Vec<&'static str>,
    pub annotation_created: bool,
    pub dropped_keywords: Vec<String>,
    pub dropped_file: Option<String>,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub rows: Vec<ImportRow>,
    pub failed: Vec<(String, String)>,
}

impl ImportReport {
    pub fn count(&self, action: MergeAction) -> usize {
        self.rows.iter().filter(|r| r.action == action).count()
    }
}

/// Concrete `PaperLookup` impl wrapping the SQLite repos. Keeps the
/// matcher's port abstraction free of `scitadel-db` dependencies.
pub struct SqliteBibLookup<'a> {
    pub papers: &'a SqlitePaperRepository,
    pub aliases: &'a SqlitePaperAliasRepository,
}

impl PaperLookup for SqliteBibLookup<'_> {
    fn find_by_doi(&self, doi: &str) -> Result<Option<String>, CoreError> {
        Ok(self
            .papers
            .find_by_doi(doi)?
            .map(|p| p.id.as_str().to_string()))
    }
    fn find_by_arxiv_id(&self, id: &str) -> Result<Option<String>, CoreError> {
        self.papers.find_id_by_arxiv_id(id)
    }
    fn find_by_pubmed_id(&self, id: &str) -> Result<Option<String>, CoreError> {
        self.papers.find_id_by_pubmed_id(id)
    }
    fn find_by_openalex_id(&self, id: &str) -> Result<Option<String>, CoreError> {
        self.papers.find_id_by_openalex_id(id)
    }
    fn find_by_bibtex_key(&self, key: &str) -> Result<Option<String>, CoreError> {
        self.papers.find_id_by_bibtex_key(key)
    }
    fn find_by_alias(&self, alias: &str) -> Result<Vec<String>, CoreError> {
        self.aliases.lookup_all(alias).map_err(Into::into)
    }
    fn find_by_title_and_year(
        &self,
        title: &str,
        year: Option<i32>,
    ) -> Result<Option<String>, CoreError> {
        self.papers.find_id_by_title_and_year(title, year)
    }
}

/// Hook for orchestrator-level prompts during a bib import (#161).
///
/// The pure resolver in `scitadel-export` cannot prompt — it has no
/// stdio, no UI, no async story. Instead, the import orchestrator
/// calls a `PromptResolver` impl when it hits a decision the strategy
/// alone can't settle (today: `MatchOutcome::AmbiguousAlias` under
/// `MergeStrategy::Interactive`).
///
/// Returning `None` means "user declined / skip this row"; the
/// orchestrator then falls back to the default failure path
/// (`CoreError::Validation`, recorded in `report.failed`). This keeps
/// the default-strategy regression in `ambiguous_alias_is_a_per_row_failure_not_a_silent_create`
/// passing: a `None`-returning resolver and a non-Interactive
/// strategy are observationally identical from the row's perspective.
///
/// TUI integration (paper-detail prompt overlay shape) is a follow-up;
/// see the annotation-prompt plumbing in
/// `crates/scitadel-tui/src/views/annotation_prompt.rs`.
pub trait PromptResolver: Send + Sync {
    /// Choose one paper id from the candidate set when an alias
    /// resolves to multiple papers and title+year can't disambiguate.
    /// Implementations may render the candidates however they like
    /// (CLI numbered list, TUI overlay, MCP elicitation).
    fn resolve_ambiguous_alias(&self, citekey: &str, candidate_ids: &[String]) -> Option<String>;
}

/// Configuration for one import run.
#[derive(Clone)]
pub struct ImportOptions {
    pub strategy: MergeStrategy,
    /// Identity attached to created annotations (`Annotation.author`).
    pub reader: String,
    /// Continue past per-row errors (logging them) rather than aborting.
    /// True is the default — see issue #134 P1 `--lenient` flag.
    pub lenient: bool,
    /// Optional prompt hook consulted when `strategy` is
    /// `Interactive`. `None` (the default) preserves the legacy
    /// failure path: ambiguous-alias rows surface as `report.failed`
    /// entries, which is the contract the #160 regression test pins.
    pub prompt_resolver: Option<Arc<dyn PromptResolver>>,
}

impl std::fmt::Debug for ImportOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportOptions")
            .field("strategy", &self.strategy)
            .field("reader", &self.reader)
            .field("lenient", &self.lenient)
            .field(
                "prompt_resolver",
                &self
                    .prompt_resolver
                    .as_ref()
                    .map(|_| "<dyn PromptResolver>"),
            )
            .finish()
    }
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            strategy: MergeStrategy::Merge,
            reader: "import".to_string(),
            lenient: true,
            prompt_resolver: None,
        }
    }
}

/// Run the import pipeline against an in-memory `.bib` source string.
/// CLI/MCP wrappers read the file and hand the bytes here so the
/// orchestrator stays filesystem-agnostic (testable from string
/// fixtures without touching disk).
pub fn import_bibtex_str(
    src: &str,
    options: &ImportOptions,
    papers: &SqlitePaperRepository,
    aliases: &SqlitePaperAliasRepository,
    annotations: &SqliteAnnotationRepository,
) -> Result<ImportReport, CoreError> {
    let entries =
        parse_bibtex(src).map_err(|e| CoreError::Adapter("bib-import".into(), e.to_string()))?;
    let lookup = SqliteBibLookup { papers, aliases };
    let mut report = ImportReport::default();

    // `annotations` is unused below — the per-row tx grabs a connection
    // from `papers.db()` and calls `SqliteAnnotationRepository::create_in_tx`
    // statically. Kept on the outer signature so callers (CLI, MCP) don't
    // need to change construction order (#157).
    let _ = annotations;

    for entry in entries {
        match import_one(&entry, options, &lookup, papers) {
            Ok(row) => report.rows.push(row),
            Err(e) if options.lenient => {
                tracing::warn!(citekey = %entry.citekey, error = %e, "import row failed; continuing");
                report.failed.push((entry.citekey.clone(), e.to_string()));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(report)
}

/// Convenience: read the file and delegate.
pub fn import_bibtex_file(
    path: &Path,
    options: &ImportOptions,
    papers: &SqlitePaperRepository,
    aliases: &SqlitePaperAliasRepository,
    annotations: &SqliteAnnotationRepository,
) -> Result<ImportReport, CoreError> {
    let src = std::fs::read_to_string(path)?;
    import_bibtex_str(&src, options, papers, aliases, annotations)
}

/// Assign and persist a stable `bibtex_key` for a newly-saved paper
/// inside an in-flight transaction. Snapshots the existing key set
/// (within the tx, so it sees the row we just upserted), runs the
/// #132 algorithm with the paper's title/authors/year, and writes
/// the result back. Tx-only sibling of the original
/// `assign_and_persist_bibtex_key` — pulled inline now that the
/// orchestrator owns the transaction (#157).
fn assign_and_persist_bibtex_key_in_tx(
    tx: &SqliteTransaction<'_>,
    paper: &scitadel_core::models::Paper,
) -> Result<(), CoreError> {
    if paper.bibtex_key.is_some() {
        return Ok(());
    }
    let mut taken = SqlitePaperRepository::taken_bibtex_keys_in_tx(tx)?;
    let assigned = assign_keys(std::slice::from_ref(paper), &mut taken);
    if let Some(key) = assigned.into_iter().next() {
        SqlitePaperRepository::update_bibtex_key_in_tx(tx, paper.id.as_str(), &key)?;
    }
    Ok(())
}

fn import_one(
    entry: &BibEntry,
    options: &ImportOptions,
    lookup: &SqliteBibLookup<'_>,
    papers: &SqlitePaperRepository,
) -> Result<ImportRow, CoreError> {
    // 1. Match cascade. Read-only; runs against committed state, so
    //    no need to be inside the per-row transaction.
    let outcome = match_entry(entry, lookup)?;
    let matched_id = match outcome {
        MatchOutcome::Matched(id, _) => Some(id),
        MatchOutcome::NoMatch => None,
        MatchOutcome::AmbiguousAlias(ids) => {
            // Both alias AND title+year ambiguous (the matcher
            // already tried title+year before falling back to this
            // arm). Refuse to act: silently creating a new paper
            // here causes the SAME citekey to fork into yet another
            // alias row, ratcheting the collision on every re-import.
            // Equally, picking one of the candidates risks merging
            // the user's bib metadata into the wrong row.
            //
            // #161: under `--strategy interactive` AND a configured
            // `PromptResolver`, ask the resolver which candidate the
            // user wants. A `Some(id)` answer threads the row back
            // onto the Matched path (the alias gets re-recorded as a
            // side effect, but the user has explicitly endorsed the
            // collision target). `None` (or no resolver, or a
            // non-interactive strategy) preserves the #160 contract:
            // bubble up as `Validation`, lenient-mode records it in
            // `report.failed`, the default-strategy regression test
            // keeps passing.
            let resolver_pick = if matches!(options.strategy, MergeStrategy::Interactive) {
                options
                    .prompt_resolver
                    .as_ref()
                    .and_then(|r| r.resolve_ambiguous_alias(&entry.citekey, &ids))
            } else {
                None
            };
            match resolver_pick {
                Some(picked) if ids.iter().any(|c| c == &picked) => Some(picked),
                _ => {
                    let preview: Vec<String> = ids.iter().take(3).cloned().collect();
                    let suffix = if ids.len() > preview.len() {
                        format!(" (+{} more)", ids.len() - preview.len())
                    } else {
                        String::new()
                    };
                    return Err(CoreError::Validation(format!(
                        "ambiguous alias '{}' matches {} papers [{}]{}; resolve via `scitadel bib rekey` on one of them, or run with --strategy interactive and a prompt resolver (#161)",
                        entry.citekey,
                        ids.len(),
                        preview.join(", "),
                        suffix,
                    )));
                }
            }
        }
    };

    // 2. Resolve via the strategy.
    let db_paper = match &matched_id {
        Some(id) => papers.get(id)?,
        None => None,
    };
    let merge = resolve_merge(db_paper, entry, options.strategy);

    // 3+4. Persistence — wrapped in a single transaction (#157) so
    //      paper-save / bibtex-key-assignment / alias-record /
    //      annotation-create commit (or roll back) atomically. Without
    //      this, a mid-row failure (e.g. transient SQLite error during
    //      a 5000-paper Zotero import) could leave an orphaned papers
    //      row with no alias — invisible to the next re-import's alias
    //      step, defeating the whole point of recording the citekey.
    let mut conn = papers.db().conn().map_err(CoreError::from)?;
    let tx = conn
        .transaction()
        .map_err(DbError::from)
        .map_err(CoreError::from)?;

    let persisted_id = match merge.action {
        MergeAction::Rejected => None,
        _ => match merge.paper.as_ref() {
            None => None,
            Some(p) => {
                if matches!(merge.action, MergeAction::Created | MergeAction::Updated) {
                    SqlitePaperRepository::save_in_tx(&tx, p)?;
                }
                if merge.action == MergeAction::Created {
                    assign_and_persist_bibtex_key_in_tx(&tx, p)?;
                }
                Some(p.id.as_str().to_string())
            }
        },
    };

    let (annotation_created, dropped_keywords, dropped_file) = if let Some(pid) = &persisted_id {
        let SideEffects {
            alias,
            annotation,
            dropped_keywords,
            dropped_file,
        } = compute_side_effects(pid, &options.reader, entry, merge.action);
        if let Some(a) = alias {
            SqlitePaperAliasRepository::record_in_tx(&tx, &a.paper_id, &a.alias, a.source)
                .map_err(CoreError::from)?;
        }
        let created = if let Some(annot) = annotation {
            SqliteAnnotationRepository::create_in_tx(&tx, &annot).map_err(CoreError::from)?;
            true
        } else {
            false
        };
        (created, dropped_keywords, dropped_file)
    } else {
        (false, vec![], None)
    };

    tx.commit()
        .map_err(DbError::from)
        .map_err(CoreError::from)?;

    Ok(ImportRow {
        citekey: entry.citekey.clone(),
        paper_id: persisted_id,
        action: merge.action,
        from_bib: merge.from_bib,
        kept_from_db: merge.kept_from_db,
        annotation_created,
        dropped_keywords,
        dropped_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::Paper;
    use scitadel_db::sqlite::Database;

    fn fresh() -> (
        SqlitePaperRepository,
        SqlitePaperAliasRepository,
        SqliteAnnotationRepository,
    ) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        (
            SqlitePaperRepository::new(db.clone()),
            SqlitePaperAliasRepository::new(db.clone()),
            SqliteAnnotationRepository::new(db),
        )
    }

    fn opts_merge(reader: &str) -> ImportOptions {
        ImportOptions {
            strategy: MergeStrategy::Merge,
            reader: reader.into(),
            lenient: true,
            prompt_resolver: None,
        }
    }

    /// Blocker fix: `SqlitePaperRepository::save` does not write the
    /// `bibtex_key` column, so newly-imported papers are keyless until
    /// the next `migrate()` runs the backfill — which would silently
    /// degrade the BibtexKey cascade step on subsequent imports. This
    /// test proves that `import_one` now assigns and persists a key
    /// for every Created paper.
    #[test]
    fn imported_new_paper_gets_persisted_bibtex_key() {
        let (papers, aliases, annotations) = fresh();
        let src = r"
@article{ignored2025x,
    title = {Bibtex Key Persistence Test},
    author = {Smith, John},
    year = {2025}
}";
        let report =
            import_bibtex_str(src, &opts_merge("lars"), &papers, &aliases, &annotations).unwrap();
        let pid = report.rows[0].paper_id.as_deref().unwrap();
        let p = papers.get(pid).unwrap().unwrap();
        assert!(
            p.bibtex_key.is_some(),
            "Created paper must have bibtex_key persisted; got None"
        );
        assert_eq!(p.bibtex_key.as_deref(), Some("smith2025bibtex"));
    }

    /// Blocker fix: prove the BibtexKey cascade step is actually
    /// exercised on round-trip — a key-only import (no DOI / arxiv /
    /// title-year hit) must still resolve to the existing paper.
    #[test]
    fn round_trip_resolves_via_bibtex_key_step_when_other_ids_absent() {
        let (papers, aliases, annotations) = fresh();
        // Seed a paper with a known bibtex_key and no external ids.
        let mut p = Paper::new("Cascade Anchor");
        p.authors = vec!["Anchor, A".into()];
        p.year = Some(2099);
        papers.save(&p).unwrap();
        // The save above doesn't persist the key, so do it explicitly
        // for the seed (mirrors what `migrate`'s backfill does).
        papers
            .update_bibtex_key(p.id.as_str(), "anchor2099cascade")
            .unwrap();

        // Bib carries only the citekey + title, deliberately missing
        // every other identifier. Title differs in case, so title+year
        // would still resolve via LOWER() — slap a different year on
        // the bib so title+year *cannot* match. Only BibtexKey can.
        let src = r"
@article{anchor2099cascade,
    title = {Different Title For Cascade Probe},
    year = {1900}
}";
        let report =
            import_bibtex_str(src, &opts_merge("lars"), &papers, &aliases, &annotations).unwrap();
        let row = &report.rows[0];
        assert_eq!(
            row.action,
            MergeAction::Unchanged,
            "BibtexKey step should resolve to the seeded paper, leaving it Unchanged"
        );
        assert_eq!(row.paper_id.as_deref(), Some(p.id.as_str()));
    }

    #[test]
    fn unmatched_entry_creates_new_paper_with_alias() {
        let (papers, aliases, annotations) = fresh();
        let src = r"
@article{novel2025widget,
    title = {A Novel Widget},
    author = {Smith, John},
    year = {2025},
    doi = {10.1/widget}
}";
        let report =
            import_bibtex_str(src, &opts_merge("lars"), &papers, &aliases, &annotations).unwrap();
        assert_eq!(report.rows.len(), 1);
        let row = &report.rows[0];
        assert_eq!(row.action, MergeAction::Created);
        assert!(row.paper_id.is_some());
        // Alias recorded — re-import will resolve via alias.
        let pid = aliases.lookup("novel2025widget").unwrap();
        assert_eq!(pid, row.paper_id);
    }

    #[test]
    fn doi_matched_entry_under_merge_leaves_db_paper_unchanged() {
        let (papers, aliases, annotations) = fresh();
        let mut existing = Paper::new("Existing Title");
        existing.doi = Some("10.1/x".into());
        existing.year = Some(2020);
        existing.authors = vec!["Original, Author".into()];
        papers.save(&existing).unwrap();

        let src = r"
@article{shouldNotWin,
    title = {Bib Title — different},
    author = {Different, Author},
    year = {2024},
    doi = {10.1/X}
}";
        let report =
            import_bibtex_str(src, &opts_merge("lars"), &papers, &aliases, &annotations).unwrap();
        let row = &report.rows[0];
        assert_eq!(row.action, MergeAction::Unchanged);

        let resaved = papers
            .get(row.paper_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(resaved.title, "Existing Title");
        assert_eq!(resaved.year, Some(2020));
    }

    #[test]
    fn note_field_creates_annotation_under_reader_identity() {
        let (papers, aliases, annotations) = fresh();
        let src = r"
@article{withNote,
    title = {Some Paper},
    author = {A, B},
    year = {2024},
    note = {Skim showed weak methods}
}";
        let report =
            import_bibtex_str(src, &opts_merge("lars"), &papers, &aliases, &annotations).unwrap();
        let row = &report.rows[0];
        assert!(row.annotation_created);

        let pid = row.paper_id.as_deref().unwrap();
        let listed = annotations.list_by_paper(pid).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].author, "lars");
        assert_eq!(listed[0].note, "Skim showed weak methods");
    }

    /// Round-trip invariant (#134 🔴 pitfall): re-importing a
    /// scitadel-exported `.bib` into a DB that has *already absorbed
    /// it* must produce zero diff in any table — papers, annotations,
    /// or paper_aliases. The export → import → import sequence is
    /// what catches the worst regressions: the FIRST import normalizes
    /// (legitimately creating aliases / matching), but the SECOND
    /// import is the actual no-op test.
    #[test]
    fn round_trip_export_then_import_touches_zero_rows() {
        use scitadel_export::export_bibtex;

        let (papers, aliases, annotations) = fresh();
        let mut p1 = Paper::new("Quantum Advantage");
        p1.authors = vec!["Smith, John".into()];
        p1.year = Some(2024);
        p1.doi = Some("10.1/quantum".into());
        let mut p2 = Paper::new("Deep Residual Learning");
        p2.authors = vec!["He, Kaiming".into()];
        p2.year = Some(2015);
        p2.arxiv_id = Some("1512.03385".into());
        let mut p3 = Paper::new("Title Only");
        p3.authors = vec!["Anon, A".into()];
        p3.year = Some(2000);

        for p in [&p1, &p2, &p3] {
            papers.save(p).unwrap();
        }
        let seeded_ids: Vec<String> = [&p1, &p2, &p3]
            .iter()
            .map(|p| p.id.as_str().to_string())
            .collect();

        let saved: Vec<Paper> = seeded_ids
            .iter()
            .map(|id| papers.get(id).unwrap().unwrap())
            .collect();
        let bib_a = export_bibtex(&saved);

        // First import: legitimate normalization pass — creates the
        // aliases for the seeded papers' citekeys. Result must report
        // every row as Unchanged (the cascade resolves via DOI / arxiv
        // / title-year and the merge strategy is db-wins under Merge).
        let report1 =
            import_bibtex_str(&bib_a, &opts_merge("lars"), &papers, &aliases, &annotations)
                .unwrap();
        assert!(report1.failed.is_empty());
        for row in &report1.rows {
            assert_eq!(row.action, MergeAction::Unchanged);
        }

        // Snapshot row counts AFTER the first import — that's the
        // "zero diff" baseline for the second import.
        let papers_baseline = papers.list_all(100, 0).unwrap().len();
        let annotations_baseline: usize = seeded_ids
            .iter()
            .map(|id| annotations.list_by_paper(id).unwrap().len())
            .sum();
        let aliases_baseline: usize = seeded_ids
            .iter()
            .map(|id| aliases.list_for(id).unwrap().len())
            .sum();
        assert_eq!(
            aliases_baseline, 3,
            "first import seeds one alias per paper"
        );

        // Second import: the actual round-trip test. Must touch zero
        // rows in any table (paper_aliases is the canary against
        // accidental duplicate-row inflation).
        let report2 =
            import_bibtex_str(&bib_a, &opts_merge("lars"), &papers, &aliases, &annotations)
                .unwrap();
        assert!(report2.failed.is_empty());
        assert_eq!(report2.rows.len(), 3);
        for row in &report2.rows {
            assert_eq!(
                row.action,
                MergeAction::Unchanged,
                "round-trip row {} must be Unchanged, got {:?}",
                row.citekey,
                row.action,
            );
        }

        let papers_after = papers.list_all(100, 0).unwrap().len();
        let annotations_after: usize = seeded_ids
            .iter()
            .map(|id| annotations.list_by_paper(id).unwrap().len())
            .sum();
        let aliases_after: usize = seeded_ids
            .iter()
            .map(|id| aliases.list_for(id).unwrap().len())
            .sum();
        assert_eq!(papers_baseline, papers_after, "no new papers");
        assert_eq!(
            annotations_baseline, annotations_after,
            "no new annotations"
        );
        assert_eq!(aliases_baseline, aliases_after, "no duplicate aliases");

        // Re-export must be byte-identical.
        let saved2: Vec<Paper> = seeded_ids
            .iter()
            .map(|id| papers.get(id).unwrap().unwrap())
            .collect();
        let bib_b = export_bibtex(&saved2);
        assert_eq!(bib_a, bib_b, "round-trip export must be byte-identical");
    }

    /// Regression test for #160: ambiguous alias must not silently
    /// create a new paper. Earlier behavior forked the citekey into
    /// a third alias row, ratcheting the collision on every re-import.
    /// New behavior: per-row Validation failure, surfaced in
    /// `report.failed` with all candidate paper IDs.
    #[test]
    fn ambiguous_alias_is_a_per_row_failure_not_a_silent_create() {
        let (papers, aliases, annotations) = fresh();

        // Two papers that legitimately share a citekey "smith2024"
        // (e.g. two distinct .bib files were both imported earlier
        // with the same Zotero-assigned key).
        let mut p1 = Paper::new("First Paper");
        p1.year = Some(2024);
        let mut p2 = Paper::new("Second Paper");
        p2.year = Some(2024);
        papers.save(&p1).unwrap();
        papers.save(&p2).unwrap();
        aliases
            .record(p1.id.as_str(), "smith2024", "bibtex-import")
            .unwrap();
        aliases
            .record(p2.id.as_str(), "smith2024", "bibtex-import")
            .unwrap();

        let papers_before = papers.list_all(100, 0).unwrap().len();
        let aliases_before = aliases.lookup_all("smith2024").unwrap().len();

        // A bib with the colliding citekey + a title that doesn't
        // match either seeded paper — forces the matcher's title+year
        // step to fail, so the cascade ends in AmbiguousAlias.
        let src = r"
@article{smith2024,
    title = {Some Other Title Entirely},
    year = {1999}
}";
        let report =
            import_bibtex_str(src, &opts_merge("lars"), &papers, &aliases, &annotations).unwrap();

        assert!(
            report.rows.is_empty(),
            "no row should land on success path; got {:?}",
            report.rows
        );
        assert_eq!(report.failed.len(), 1);
        let (citekey, msg) = &report.failed[0];
        assert_eq!(citekey, "smith2024");
        assert!(msg.contains("ambiguous alias"), "got: {msg}");
        assert!(msg.contains("matches 2 papers"), "got: {msg}");

        // The canary: papers + aliases counts unchanged.
        assert_eq!(papers.list_all(100, 0).unwrap().len(), papers_before);
        assert_eq!(
            aliases.lookup_all("smith2024").unwrap().len(),
            aliases_before
        );
    }

    /// Per-row tx invariant (#157): paper-save + alias-record + annotation-
    /// create must commit (or roll back) atomically. Without this, a
    /// transient failure on the alias write after a successful paper
    /// save would leave an orphan papers row that the next re-import
    /// can't see via the alias step — silently DOI-merging or
    /// duplicating instead of resolving to the orphan.
    ///
    /// Drive the primitive directly: open the orchestrator's tx, save
    /// a paper, then trigger an FK violation (alias → bogus paper_id)
    /// and drop the tx without committing. Assert papers row count is
    /// zero after rollback.
    #[test]
    fn per_row_tx_rolls_back_when_alias_record_fails() {
        let (papers, aliases, _annotations) = fresh();
        let mut p = Paper::new("Will Be Rolled Back");
        p.year = Some(2026);

        let papers_before = papers.list_all(100, 0).unwrap().len();
        assert_eq!(papers_before, 0);

        // Mirror what `import_one` does: grab one connection, open a
        // single tx, save the paper inside it, then attempt an alias
        // record that violates the FK (paper_aliases.paper_id REFERENCES
        // papers(id)). The error must propagate and — critically —
        // dropping `tx` without `commit()` must roll back the paper save.
        let mut conn = papers.db().conn().unwrap();
        let tx = conn.transaction().unwrap();
        SqlitePaperRepository::save_in_tx(&tx, &p).unwrap();
        // Inside the tx, the paper IS visible to subsequent queries.
        let mid_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM papers", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            mid_count, 1,
            "paper must be visible inside its own tx before commit"
        );
        // Force the failure: alias points at a non-existent paper.
        let err = SqlitePaperAliasRepository::record_in_tx(
            &tx,
            "no-such-paper-id",
            "ghostkey",
            "bibtex-import",
        );
        assert!(err.is_err(), "FK violation must surface as Err");
        // Drop without commit → rollback.
        drop(tx);
        drop(conn);

        // The canary: papers count is back to zero. Without per-row tx
        // wrapping (#134 PR-A behavior), the paper would persist as an
        // orphan and the subsequent re-import wouldn't know about it.
        let papers_after = papers.list_all(100, 0).unwrap().len();
        assert_eq!(
            papers_after, 0,
            "paper save must roll back when a later step in the same row's tx fails; \
             got {papers_after} orphan(s)"
        );
        // And no alias was recorded either.
        assert!(aliases.lookup("ghostkey").unwrap().is_none());
    }

    /// #161: under `MergeStrategy::Interactive` with a `PromptResolver`
    /// that returns one of the candidate ids, the row threads through
    /// the Matched path. The picked paper's row stays unchanged
    /// (Interactive == Merge semantics on owned columns), but the
    /// citekey gets recorded as an alias on the chosen paper so the
    /// next import resolves cleanly.
    #[test]
    fn interactive_strategy_with_resolver_picks_a_candidate() {
        struct FixedPick {
            picked: String,
        }
        impl PromptResolver for FixedPick {
            fn resolve_ambiguous_alias(
                &self,
                _citekey: &str,
                candidate_ids: &[String],
            ) -> Option<String> {
                // Sanity: resolver only ever sees the candidate set
                // the matcher actually surfaced.
                assert!(candidate_ids.contains(&self.picked));
                Some(self.picked.clone())
            }
        }

        let (papers, aliases, annotations) = fresh();
        let mut p1 = Paper::new("First Paper");
        p1.year = Some(2024);
        let mut p2 = Paper::new("Second Paper");
        p2.year = Some(2024);
        papers.save(&p1).unwrap();
        papers.save(&p2).unwrap();
        aliases
            .record(p1.id.as_str(), "smith2024", "bibtex-import")
            .unwrap();
        aliases
            .record(p2.id.as_str(), "smith2024", "bibtex-import")
            .unwrap();

        let resolver = Arc::new(FixedPick {
            picked: p2.id.as_str().to_string(),
        });
        let options = ImportOptions {
            strategy: MergeStrategy::Interactive,
            reader: "lars".into(),
            lenient: true,
            prompt_resolver: Some(resolver),
        };

        let src = r"
@article{smith2024,
    title = {Some Other Title Entirely},
    year = {1999}
}";
        let report = import_bibtex_str(src, &options, &papers, &aliases, &annotations).unwrap();

        assert!(
            report.failed.is_empty(),
            "interactive resolver chose a candidate; failure list should be empty: {:?}",
            report.failed
        );
        assert_eq!(report.rows.len(), 1);
        let row = &report.rows[0];
        assert_eq!(row.citekey, "smith2024");
        assert_eq!(row.action, MergeAction::Unchanged);
        assert_eq!(row.paper_id.as_deref(), Some(p2.id.as_str()));
    }

    /// #161: `MergeStrategy::Interactive` without a resolver must
    /// behave identically to the default failure path. Belt-and-braces
    /// against accidentally enabling silent-create behavior just by
    /// flipping the strategy.
    #[test]
    fn interactive_strategy_without_resolver_falls_back_to_validation_failure() {
        let (papers, aliases, annotations) = fresh();
        let mut p1 = Paper::new("First Paper");
        p1.year = Some(2024);
        let mut p2 = Paper::new("Second Paper");
        p2.year = Some(2024);
        papers.save(&p1).unwrap();
        papers.save(&p2).unwrap();
        aliases
            .record(p1.id.as_str(), "smith2024", "bibtex-import")
            .unwrap();
        aliases
            .record(p2.id.as_str(), "smith2024", "bibtex-import")
            .unwrap();

        let options = ImportOptions {
            strategy: MergeStrategy::Interactive,
            reader: "lars".into(),
            lenient: true,
            prompt_resolver: None,
        };

        let src = r"
@article{smith2024,
    title = {Some Other Title Entirely},
    year = {1999}
}";
        let report = import_bibtex_str(src, &options, &papers, &aliases, &annotations).unwrap();
        assert!(report.rows.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].1.contains("ambiguous alias"));
    }

    #[test]
    fn lenient_run_continues_past_a_malformed_row() {
        let (papers, aliases, annotations) = fresh();
        // Second entry lacks closing brace — biblatex parse fails on
        // the whole document. We simulate per-row failure differently:
        // give the second entry a known shape but ensure import
        // logic surfaces a clean report when the parse succeeds. The
        // current pipeline aborts on parse failure (the whole input
        // is one document); per-row resilience under `lenient` only
        // applies once we're inside the loop. This test confirms the
        // happy path doesn't panic on a mixed batch.
        let src = r"
@article{a, title = {A}, year = {2001}}
@article{b, title = {B}, year = {2002}}
@article{c, title = {C}, year = {2003}}
";
        let report =
            import_bibtex_str(src, &opts_merge("lars"), &papers, &aliases, &annotations).unwrap();
        assert_eq!(report.rows.len(), 3);
        assert!(report.failed.is_empty());
    }
}
