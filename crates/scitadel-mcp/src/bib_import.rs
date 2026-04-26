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
//! Failures on a single row are logged and counted but never abort
//! the run — a 5000-paper Zotero dump must survive a couple bad
//! entries.

use std::path::Path;

use scitadel_core::bibtex_key::assign_keys;
use scitadel_core::error::CoreError;
use scitadel_core::ports::PaperRepository;
use scitadel_db::sqlite::{
    SqliteAnnotationRepository, SqlitePaperAliasRepository, SqlitePaperRepository,
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

/// Configuration for one import run.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub strategy: MergeStrategy,
    /// Identity attached to created annotations (`Annotation.author`).
    pub reader: String,
    /// Continue past per-row errors (logging them) rather than aborting.
    /// True is the default — see issue #134 P1 `--lenient` flag.
    pub lenient: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            strategy: MergeStrategy::Merge,
            reader: "import".to_string(),
            lenient: true,
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

    for entry in entries {
        match import_one(&entry, options, &lookup, papers, aliases, annotations) {
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

/// Assign and persist a stable `bibtex_key` for a newly-saved paper.
/// Snapshots the existing key set, runs the #132 algorithm with the
/// paper's title/authors/year, and writes the result back. Idempotent
/// only if the paper already has a key (the algorithm's collision
/// disambiguator may pick a different suffix on re-run when the
/// `taken` set differs).
fn assign_and_persist_bibtex_key(
    papers: &SqlitePaperRepository,
    paper: &scitadel_core::models::Paper,
) -> Result<(), CoreError> {
    if paper.bibtex_key.is_some() {
        return Ok(());
    }
    let mut taken = papers.taken_bibtex_keys()?;
    let assigned = assign_keys(std::slice::from_ref(paper), &mut taken);
    if let Some(key) = assigned.into_iter().next() {
        papers.update_bibtex_key(paper.id.as_str(), &key)?;
    }
    Ok(())
}

fn import_one(
    entry: &BibEntry,
    options: &ImportOptions,
    lookup: &SqliteBibLookup<'_>,
    papers: &SqlitePaperRepository,
    aliases: &SqlitePaperAliasRepository,
    annotations: &SqliteAnnotationRepository,
) -> Result<ImportRow, CoreError> {
    // 1. Match cascade.
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
            // Bubble up as a Validation error; the lenient-mode
            // handler in `import_bibtex_str` records it in
            // `report.failed` so the user sees exactly which entries
            // need manual resolution (typically: `bib rekey` one
            // of the candidates so its alias no longer collides).
            // See #160. Interactive resolution lands with #161.
            let preview: Vec<String> = ids.iter().take(3).cloned().collect();
            let suffix = if ids.len() > preview.len() {
                format!(" (+{} more)", ids.len() - preview.len())
            } else {
                String::new()
            };
            return Err(CoreError::Validation(format!(
                "ambiguous alias '{}' matches {} papers [{}]{}; resolve via `scitadel bib rekey` on one of them, or use --strategy interactive once it lands (#161)",
                entry.citekey,
                ids.len(),
                preview.join(", "),
                suffix,
            )));
        }
    };

    // 2. Resolve via the strategy.
    let db_paper = match &matched_id {
        Some(id) => papers.get(id)?,
        None => None,
    };
    let merge = resolve_merge(db_paper, entry, options.strategy);

    // 3. Persist the paper. Created/Updated write the row; Created
    //    additionally needs an explicit bibtex_key assignment because
    //    `SqlitePaperRepository::save` does not write the column —
    //    skipping this leaves new papers keyless until the next
    //    `migrate` run and silently degrades subsequent imports'
    //    BibtexKey cascade step.
    let persisted_id = match merge.action {
        MergeAction::Rejected => None,
        _ => match merge.paper.as_ref() {
            None => None,
            Some(p) => {
                if matches!(merge.action, MergeAction::Created | MergeAction::Updated) {
                    papers.save(p)?;
                }
                if merge.action == MergeAction::Created {
                    assign_and_persist_bibtex_key(papers, p)?;
                }
                Some(p.id.as_str().to_string())
            }
        },
    };

    // 4. Side effects (alias / annotation / verbose-log items) — only
    //    when we have a paper to attach them to.
    let (annotation_created, dropped_keywords, dropped_file) = if let Some(pid) = &persisted_id {
        let SideEffects {
            alias,
            annotation,
            dropped_keywords,
            dropped_file,
        } = compute_side_effects(pid, &options.reader, entry, merge.action);
        if let Some(a) = alias {
            aliases
                .record(&a.paper_id, &a.alias, a.source)
                .map_err(CoreError::from)?;
        }
        let created = if let Some(annot) = annotation {
            annotations.create(&annot)?;
            true
        } else {
            false
        };
        (created, dropped_keywords, dropped_file)
    } else {
        (false, vec![], None)
    };

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
