//! `bib rekey` orchestrator (#134 PR-B).
//!
//! Escape hatch for the stable-citation-key contract from #132:
//! re-run the algorithm against current paper metadata, or set an
//! explicit key chosen by the user. The pre-rekey key is preserved
//! as an alias so manuscripts that still cite the paper by the old
//! key continue to resolve.
//!
//! The op is audit-logged via `tracing::info!` matching the #100
//! convention used by annotation writes — every rekey leaves a
//! trail in the structured logs with `op`, `paper_id`, `old`, `new`,
//! and `reader`.

use scitadel_core::bibtex_key::{disambiguate, generate_key};
use scitadel_core::error::CoreError;
use scitadel_core::ports::PaperRepository;
use scitadel_db::sqlite::{SOURCE_REKEY, SqlitePaperAliasRepository, SqlitePaperRepository};

/// Result of a rekey op. `changed = false` when the algorithm or
/// explicit key resolves to the existing value — the call short-
/// circuits and skips the DB write, but is still audit-logged so
/// the trail records the no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RekeyOutcome {
    pub paper_id: String,
    pub old_key: Option<String>,
    pub new_key: String,
    pub changed: bool,
}

/// Failures specific to the rekey path. Wraps [`CoreError`] for I/O
/// errors and adds two domain-specific variants.
#[derive(Debug, thiserror::Error)]
pub enum RekeyError {
    #[error("paper '{0}' not found")]
    PaperNotFound(String),
    #[error("citation key '{key}' is already used by paper {owner}")]
    KeyCollision { key: String, owner: String },
    #[error("explicit key '{0}' is invalid: must match [a-zA-Z][a-zA-Z0-9-_:]*")]
    InvalidKey(String),
    #[error(transparent)]
    Core(#[from] CoreError),
}

/// Set or regenerate the citation key for `paper_id`.
///
/// - `explicit_key = Some(k)`: validate `k`'s shape, fail loudly if
///   another paper already owns `k`, write `k` to the row.
/// - `explicit_key = None`: snapshot the existing taken-key set
///   *excluding* this paper's current key, run the #132 algorithm
///   with the paper's *current* metadata. The exclusion lets the
///   disambiguator pick a different suffix when the old key was
///   suffixed; for unsuffixed keys with unchanged metadata, the
///   regenerated key matches the old one and the call is a no-op.
///
/// The pre-rekey key is recorded on `paper_aliases` (with
/// `source = "rekey"`) so a manuscript that still cites the paper
/// by `old_key` resolves via the alias step of the import-match
/// cascade. The alias-record is itself idempotent — re-running
/// rekey doesn't multiply rows.
pub fn rekey_paper(
    papers: &SqlitePaperRepository,
    aliases: &SqlitePaperAliasRepository,
    paper_id: &str,
    explicit_key: Option<&str>,
    reader: &str,
) -> Result<RekeyOutcome, RekeyError> {
    let paper = papers
        .get(paper_id)?
        .ok_or_else(|| RekeyError::PaperNotFound(paper_id.to_string()))?;
    let old_key = paper.bibtex_key.clone();

    let new_key = if let Some(k) = explicit_key {
        let trimmed = k.trim();
        if !is_valid_citekey(trimmed) {
            return Err(RekeyError::InvalidKey(trimmed.to_string()));
        }
        // Collision check: another paper already owns this key?
        if let Some(owner) = papers.find_id_by_bibtex_key(trimmed)?
            && owner != paper_id
        {
            return Err(RekeyError::KeyCollision {
                key: trimmed.to_string(),
                owner,
            });
        }
        trimmed.to_string()
    } else {
        // Recompute against current metadata. Exclude the paper's own
        // current key so the disambiguator can pick a fresh suffix;
        // if metadata still produces the same key, the call short-
        // circuits below as a no-op.
        let mut taken = papers.taken_bibtex_keys()?;
        if let Some(k) = old_key.as_deref() {
            taken.remove(k);
        }
        let base = generate_key(&paper);
        disambiguate(&base, &taken)
    };

    let changed = old_key.as_deref() != Some(new_key.as_str());

    if changed {
        papers.update_bibtex_key(paper_id, &new_key)?;
        // Preserve the prior key as an alias so old citations still
        // resolve. Skip when there was no prior key (paper predates
        // the #132 backfill in some edge case) — there's nothing to
        // alias from.
        if let Some(prev) = old_key.as_deref() {
            aliases
                .record(paper_id, prev, SOURCE_REKEY)
                .map_err(CoreError::from)?;
        }
    }

    tracing::info!(
        op = "bib_rekey",
        paper_id = %paper_id,
        old = ?old_key,
        new = %new_key,
        changed = changed,
        reader = %reader,
        "rekey op",
    );

    Ok(RekeyOutcome {
        paper_id: paper_id.to_string(),
        old_key,
        new_key,
        changed,
    })
}

/// BibTeX citation keys are conservative — letters, digits, and a
/// short list of separators biber accepts in practice. Reject
/// anything that would silently break `\cite{...}` parsing or that
/// embeds whitespace / braces.
fn is_valid_citekey(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::Paper;
    use scitadel_db::sqlite::Database;

    fn fresh() -> (SqlitePaperRepository, SqlitePaperAliasRepository) {
        let db = Database::open_in_memory().unwrap();
        db.migrate().unwrap();
        (
            SqlitePaperRepository::new(db.clone()),
            SqlitePaperAliasRepository::new(db),
        )
    }

    fn seed(papers: &SqlitePaperRepository, title: &str, author: &str, year: i32) -> Paper {
        let mut p = Paper::new(title);
        p.authors = vec![author.to_string()];
        p.year = Some(year);
        papers.save(&p).unwrap();
        // Seed a key the way migrate's backfill would.
        let key = generate_key(&p);
        papers.update_bibtex_key(p.id.as_str(), &key).unwrap();
        papers.get(p.id.as_str()).unwrap().unwrap()
    }

    #[test]
    fn explicit_key_replaces_existing_and_records_old_as_alias() {
        let (papers, aliases) = fresh();
        let p = seed(&papers, "Quantum Theory", "Curie, Marie", 1903);
        // Algorithm yields "curie1903quantum"
        assert_eq!(p.bibtex_key.as_deref(), Some("curie1903quantum"));

        let out = rekey_paper(
            &papers,
            &aliases,
            p.id.as_str(),
            Some("curie-radium"),
            "lars",
        )
        .unwrap();
        assert!(out.changed);
        assert_eq!(out.old_key.as_deref(), Some("curie1903quantum"));
        assert_eq!(out.new_key, "curie-radium");

        let resaved = papers.get(p.id.as_str()).unwrap().unwrap();
        assert_eq!(resaved.bibtex_key.as_deref(), Some("curie-radium"));

        // Old key is now an alias under source = "rekey".
        let alias_rows = aliases.list_for(p.id.as_str()).unwrap();
        assert!(
            alias_rows
                .iter()
                .any(|(a, s)| a == "curie1903quantum" && s == "rekey")
        );
    }

    #[test]
    fn explicit_key_collision_with_other_paper_errors() {
        let (papers, aliases) = fresh();
        let p1 = seed(&papers, "Paper One", "Smith, John", 2020);
        let _p2 = seed(&papers, "Paper Two", "Jones, Bob", 2021);

        let err = rekey_paper(
            &papers,
            &aliases,
            p1.id.as_str(),
            Some("jones2021paper"),
            "lars",
        )
        .unwrap_err();
        match err {
            RekeyError::KeyCollision { key, owner } => {
                assert_eq!(key, "jones2021paper");
                assert!(!owner.is_empty());
            }
            other => panic!("expected KeyCollision, got {other:?}"),
        }
    }

    #[test]
    fn explicit_key_matching_self_is_unchanged_noop() {
        let (papers, aliases) = fresh();
        let p = seed(&papers, "Same Key", "Author, A", 2024);
        let current = p.bibtex_key.clone().unwrap();

        let out = rekey_paper(&papers, &aliases, p.id.as_str(), Some(&current), "lars").unwrap();
        assert!(!out.changed);
        assert_eq!(out.new_key, current);
        // No alias should be recorded — nothing changed.
        assert!(aliases.list_for(p.id.as_str()).unwrap().is_empty());
    }

    #[test]
    fn algorithmic_rekey_with_unchanged_metadata_is_noop() {
        let (papers, aliases) = fresh();
        let p = seed(&papers, "Stable", "Author, A", 2024);

        let out = rekey_paper(&papers, &aliases, p.id.as_str(), None, "lars").unwrap();
        assert!(
            !out.changed,
            "algorithmic rekey on unchanged metadata is a no-op"
        );
        assert_eq!(out.new_key, p.bibtex_key.unwrap());
    }

    #[test]
    fn algorithmic_rekey_after_metadata_change_picks_new_key() {
        let (papers, aliases) = fresh();
        let p = seed(&papers, "Old Title Here", "Author, A", 2020);
        let old = p.bibtex_key.clone().unwrap();

        // Mutate paper metadata (simulating a title correction).
        let mut updated = p.clone();
        updated.title = "Brand New Title".into();
        papers.save(&updated).unwrap();

        let out = rekey_paper(&papers, &aliases, p.id.as_str(), None, "lars").unwrap();
        assert!(out.changed);
        assert_ne!(out.new_key, old);
        assert_eq!(out.old_key.as_deref(), Some(old.as_str()));
        // Old key preserved as alias.
        let alias_rows = aliases.list_for(p.id.as_str()).unwrap();
        assert!(alias_rows.iter().any(|(a, _)| a == &old));
    }

    #[test]
    fn missing_paper_yields_clear_error() {
        let (papers, aliases) = fresh();
        let err = rekey_paper(&papers, &aliases, "p-nope", Some("anything"), "lars").unwrap_err();
        assert!(matches!(err, RekeyError::PaperNotFound(_)));
    }

    #[test]
    fn invalid_explicit_key_rejected() {
        let (papers, aliases) = fresh();
        let p = seed(&papers, "T", "A, B", 2024);
        for bad in [
            "",
            "1leadingdigit",
            "with space",
            "has{brace}",
            "with,comma",
            "@special",
        ] {
            let err = rekey_paper(&papers, &aliases, p.id.as_str(), Some(bad), "lars")
                .expect_err(&format!("'{bad}' should be rejected"));
            assert!(matches!(err, RekeyError::InvalidKey(_)));
        }
    }

    #[test]
    fn rekey_op_is_idempotent_on_re_run() {
        let (papers, aliases) = fresh();
        let p = seed(&papers, "Idempotent", "Author, A", 2024);
        let target = "idempotent-target";

        let out1 = rekey_paper(&papers, &aliases, p.id.as_str(), Some(target), "lars").unwrap();
        assert!(out1.changed);
        let out2 = rekey_paper(&papers, &aliases, p.id.as_str(), Some(target), "lars").unwrap();
        assert!(!out2.changed, "second rekey to same target is a no-op");

        // Alias count should still be 1 (the original key) — not 2.
        let alias_count = aliases.list_for(p.id.as_str()).unwrap().len();
        assert_eq!(alias_count, 1);
    }
}
