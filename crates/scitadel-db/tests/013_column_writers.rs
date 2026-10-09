//! #291's third proposal: the audit, folded into CI.
//!
//! Five slices in a row (#284, #285, #286, #290) each opened by finding a
//! migration-013 table or column with **no writer anywhere in the workspace**,
//! and #291 audited all nine tables by hand. This test is that audit, run on
//! every `cargo test`, so the sixth slice cannot open the same way: a column
//! added to a migration-013 table with neither a writer nor an allowlist entry
//! fails the build here rather than being discovered by a reader that emits
//! `null` for it forever.
//!
//! # Why it reads write structs, and why it must
//!
//! The first sweep at this audit was a regex over the SQL. It produced ~20
//! false positives, all on `artefacts`, for one reason: `artefacts_insert`
//! builds its column list as a **runtime `{columns}` variable**, so no static
//! text anywhere names the columns that statement writes. A regex over the
//! strings cannot see it, and a check that cries wolf is worse than no check —
//! the next person to see it fail would add the twenty entries to an allowlist
//! and the audit would be dead.
//!
//! So the evidence for a table with a write struct is the **struct's fields**,
//! read out of its definition. Every insert path in this crate is built from
//! those fields, which is what makes a column missing from the struct a column
//! nothing writes — including through the runtime formatting above, because
//! that formatting is derived from the struct rather than spelled twice.
//!
//! Three tables have no write struct at all: their writers are hand-written
//! statements, and a column list in a statement literal *is* visible to a
//! reader. Those use [`Writer::Statement`], and the reason that is sound for
//! them and not for `artefacts` is documented on the variant. One table
//! (`paper_identity_checks`) has **both**: the machine's checks arrive through
//! its write struct, and a person's override is written by its own statement,
//! which is the only place `override_reason` is set.
//!
//! # What it does with a column it cannot classify
//!
//! It **fails**, naming the table, the column, and the two ways to make it
//! pass. That is the only honest option: a column this test cannot classify is
//! exactly the column #291 exists to find.

use scitadel_db::sqlite::Database;

/// Migration 013's own SQL, for the list of tables it creates.
const MIGRATION_013: &str = include_str!("../migrations/013_acquisition.sql");

/// How a table's written columns are discovered.
enum Writer {
    /// The **public fields of a write struct**, read out of its definition.
    ///
    /// Sound for every table whose insert path is built from the struct.
    /// `non_columns` names the fields that are deliberately *not* columns
    /// (`ArtefactWrite::blob` is a second row, not a column), so a new
    /// non-column field has to be declared rather than silently ignored.
    Struct {
        file: &'static str,
        name: &'static str,
        non_columns: &'static [&'static str],
    },
    /// Columns named on the lines of an **INSERT or UPDATE statement** in the
    /// named file.
    ///
    /// Sound only where the statement spells its column list literally, with no
    /// runtime formatting — which is the case for the lease claim, the pacer
    /// ledger, the identity override and the targeted `papers` writers. It
    /// would be worthless for `artefacts`, whose column list is a `{columns}`
    /// variable interpolated at runtime; that table is [`Self::Struct`] for
    /// exactly this reason, and the ~20 false positives the first sweep
    /// produced are what the distinction costs to get wrong.
    Statement { file: &'static str },
}

/// One audited table: its writers, and which of its columns are migration
/// 013's.
struct Audited {
    table: &'static str,
    /// The columns of `table` that belong to migration 013.
    ///
    /// `None` when 013 created the table and therefore owns all of it. `Some`
    /// for `papers`, which 013 only added two columns to: auditing the other
    /// seventeen would be auditing migrations 001–012 under a test named after
    /// 013, and would report columns written by an upsert this check has no
    /// reason to read.
    scope: Option<&'static [&'static str]>,
    writers: &'static [Writer],
}

/// Every table migration 013 created, plus the two columns it added to
/// `papers`, and what writes them.
///
/// A table missing here is caught by
/// `every_table_migration_013_created_is_audited`, so a new table cannot be
/// added un-audited; a new *column* is caught by the per-column check.
const AUDITED: &[Audited] = &[
    Audited {
        table: "blobs",
        scope: None,
        writers: &[Writer::Struct {
            file: "src/sqlite/artefacts.rs",
            name: "BlobWrite",
            non_columns: &[],
        }],
    },
    Audited {
        table: "artefacts",
        scope: None,
        writers: &[Writer::Struct {
            file: "src/sqlite/artefacts.rs",
            name: "ArtefactWrite",
            // The content-addressed copy, written as its own row by the same
            // writer. Not a column of `artefacts`.
            non_columns: &["blob"],
        }],
    },
    Audited {
        table: "acquisition_state",
        scope: None,
        writers: &[Writer::Struct {
            file: "src/sqlite/artefacts.rs",
            name: "StateWrite",
            non_columns: &[],
        }],
    },
    Audited {
        table: "acquisition_attempts",
        scope: None,
        writers: &[Writer::Struct {
            file: "src/sqlite/identity.rs",
            name: "AttemptWrite",
            non_columns: &[],
        }],
    },
    Audited {
        // One statement, in `leases::acquire`, and no struct: the claim and the
        // answer are one atomic fact, which is the reason there is no
        // intermediate type.
        table: "acquisition_leases",
        scope: None,
        writers: &[Writer::Statement {
            file: "src/sqlite/leases.rs",
        }],
    },
    Audited {
        table: "paper_identity_checks",
        scope: None,
        writers: &[
            Writer::Struct {
                file: "src/sqlite/identity.rs",
                name: "IdentityCheckWrite",
                non_columns: &[],
            },
            // The human override, written by `override_identity`. It is the
            // only writer of `override_reason`, and the reason this table has
            // two kinds of evidence.
            Writer::Statement {
                file: "src/sqlite/identity.rs",
            },
        ],
    },
    Audited {
        table: "pacer_buckets",
        scope: None,
        writers: &[Writer::Statement {
            file: "src/sqlite/pacer.rs",
        }],
    },
    Audited {
        table: "pacer_grants",
        scope: None,
        writers: &[Writer::Statement {
            file: "src/sqlite/pacer.rs",
        }],
    },
    Audited {
        // #258's substrate, and deliberately out of #291's scope: a column
        // arrives with the slice that has real values for it, and no slice has
        // values for a TDM authorisation yet. #258 is that slice.
        table: "tdm_authorisations",
        scope: None,
        writers: &[],
    },
    Audited {
        table: "papers",
        scope: Some(&["pmcid", "osti_id"]),
        writers: &[Writer::Statement {
            file: "src/sqlite/identity.rs",
        }],
    },
];

/// The columns nothing writes, with the owner of the slice that will.
///
/// `(table, column, owner)` where the owner is a tracking issue, or — for a
/// column the **database** fills — the schema itself. The check makes the list
/// honest in both directions: an entry for a column a writer *does* write
/// fails, and an entry for a column that does not exist fails. So it cannot
/// grow to hide a missing writer; the only entries it can hold are columns
/// nothing in this workspace writes.
const UNWRITTEN: &[(&str, &str, &str)] = &[
    // The rowid SQLite assigns on insert. Nothing in the workspace names it,
    // and nothing should: `record_attempt` and `write_identity_check` read it
    // back with `last_insert_rowid`.
    (
        "acquisition_attempts",
        "id",
        "the rowid SQLite assigns on insert",
    ),
    (
        "paper_identity_checks",
        "id",
        "the rowid SQLite assigns on insert",
    ),
    // Self-referential provenance: which full text a derived artefact (a JATS
    // `table-wrap`, an HTML table) was extracted from. #234's manifest shape
    // carries it, and ADR-007 §3's S3 acceptance criterion — "the number of JATS
    // `table-wrap` elements equals the number of table artefacts" — is the
    // slice that produces those rows, so the column is #254's to fill.
    ("artefacts", "derived_from", "#254"),
    // #258's tier-2 substrate: a TDM policy reference, the authentication
    // context a tier-2 fetch ran under, and the retention deadline for material
    // fetched that way. All three arrive with that slice.
    ("artefacts", "tdm_policy_ref", "#258"),
    ("artefacts", "auth_context", "#258"),
    ("artefacts", "retain_until", "#258"),
    // #258's substrate whole: no writer exists anywhere in the workspace, not
    // even for `publisher`, which #291's inventory recorded as written.
    ("tdm_authorisations", "publisher", "#258"),
    ("tdm_authorisations", "basis", "#258"),
    ("tdm_authorisations", "policy_ref", "#258"),
    ("tdm_authorisations", "allows_bulk_session", "#258"),
    ("tdm_authorisations", "max_works_per_day", "#258"),
    ("tdm_authorisations", "granted_by", "#258"),
    ("tdm_authorisations", "recorded_at", "#258"),
    ("tdm_authorisations", "expires_at", "#258"),
    // The PMC identifier, and the asymmetry that found it: `osti_id` has a
    // targeted writer (`write_osti_id`, used by `scitadel import-flat`) while
    // `pmcid` has only a reader. #254's metadata pass is the intended producer
    // — the sibling column's own docs name it.
    ("papers", "pmcid", "#254"),
];

/// The tables migration 013 created, read out of its own SQL.
fn created_tables() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in MIGRATION_013.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("CREATE TABLE IF NOT EXISTS ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert!(!name.is_empty(), "no table name on: {line}");
            out.push(name);
        }
    }
    assert!(
        out.len() >= 9,
        "migration 013 is expected to create at least nine tables, found {}: {out:?}",
        out.len()
    );
    out
}

/// The audited columns of `table`, in the schema's own order.
fn columns(db: &Database, audited: &Audited) -> Vec<String> {
    let conn = db.conn().expect("a pooled connection");
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", audited.table))
        .unwrap_or_else(|e| panic!("{}: {e}", audited.table));
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap_or_else(|e| panic!("{}: {e}", audited.table))
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("{}: {e}", audited.table));
    assert!(
        !rows.is_empty(),
        "{} has no columns — the table does not exist, so the audit would pass \
         vacuously. Check the name against migration 013.",
        audited.table
    );
    match audited.scope {
        None => rows,
        Some(scope) => rows
            .into_iter()
            .filter(|column| scope.contains(&column.as_str()))
            .collect(),
    }
}

/// The public fields of the struct named `name`, in declaration order.
///
/// Deliberately a reader of the **struct definition** rather than of any SQL:
/// see the module docs for what a regex over the statements got wrong. Doc
/// comments, attributes and line comments are skipped, and the struct ends at
/// its closing brace — so nothing nested inside it can borrow this parser's
/// fields.
fn struct_fields(source: &str, name: &str) -> Vec<String> {
    let header = format!("pub struct {name} {{");
    let start = source
        .find(&header)
        .unwrap_or_else(|| panic!("no `{header}` in the source this check reads"));
    let mut fields: Vec<String> = Vec::new();
    for line in source[start..].lines().skip(1) {
        let line = line.trim();
        if line == "}" {
            break;
        }
        if line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        // `pub name: Type`, and the name is the first identifier after `pub`.
        if let Some(rest) = line.strip_prefix("pub ")
            && let Some(field) = rest
                .split(|c: char| c == ':' || c.is_whitespace())
                .find(|part| !part.is_empty())
        {
            fields.push(field.to_string());
        }
    }
    assert!(
        !fields.is_empty(),
        "`{name}` parsed as having no fields — the shape this check expects has \
         moved, and a reader that finds nothing would report every column unwritten"
    );
    fields
}

/// The columns a statement writer names, read off its own statement lines.
///
/// A column counts when it appears inside an `INSERT INTO <table>` or an
/// `UPDATE <table>` statement, from its first line through the line that closes
/// its column list — the `VALUES` line for an INSERT, the `WHERE` line for an
/// UPDATE. Multi-line because `papers`' own upsert spells its column list
/// across four lines under the `INSERT INTO papers` line, and a single-line
/// reader would report every one of those columns as unwritten.
///
/// For an UPDATE, only the `SET` side counts. The `WHERE` clause *reads*
/// columns — most of these statements end `WHERE id = ?n` or
/// `WHERE paper_id = ?1` — and a check that read a constraint as a write would
/// report `id` as written by the identity override, which assigns nothing of
/// the kind. That is the same failure as the `{columns}` regex, one clause
/// over, and it is the reason this function reads statements rather than
/// lines.
fn statement_columns(source: &str, table: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let head = if lines[at].contains(&format!("INSERT INTO {table}")) {
            "VALUES"
        } else if lines[at].contains(&format!("UPDATE {table}")) {
            "WHERE"
        } else {
            at += 1;
            continue;
        };
        let mut end = at;
        while end < lines.len() && !lines[end].contains(head) {
            end += 1;
        }
        let end = end.min(lines.len() - 1);
        for line in &lines[at..=end] {
            // The `WHERE` clause is a read, not a write.
            let written = line.split(head).next().unwrap_or(line);
            out.extend(column_words(written));
        }
        at = end + 1;
    }
    out
}

/// Every identifier-shaped word on `line`, which is what a SQL column list is.
fn column_words(line: &str) -> Vec<String> {
    line.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Every identifier the writers name, for the **membership** test: does this
/// column get written?
///
/// A superset for the statement writers, whose lines also carry SQL keywords
/// and placeholders — which is fine, because only membership is asked of them.
/// The **exactness** test is the struct one, below.
fn named_columns(audited: &Audited) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for writer in audited.writers {
        match writer {
            Writer::Struct {
                file,
                name,
                non_columns,
            } => {
                let source = read_source(file);
                out.extend(
                    struct_fields(&source, name)
                        .into_iter()
                        .filter(|field| !non_columns.contains(&field.as_str())),
                );
            }
            Writer::Statement { file } => {
                let source = read_source(file);
                out.extend(statement_columns(&source, audited.table));
            }
        }
    }
    out
}

/// The struct fields, for the **exactness** test.
///
/// A struct field is not a keyword or a placeholder: it is either a column of
/// this table, or it is declared in `non_columns`. Anything else means the
/// audit has not followed a rename, or a field was added that is not a column
/// and was not declared — and a check that let either pass would be a check
/// that had stopped reading the struct.
fn struct_fields_of(audited: &Audited) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for writer in audited.writers {
        if let Writer::Struct { file, name, .. } = writer {
            let source = read_source(file);
            out.extend(struct_fields(&source, name));
        }
    }
    out
}

/// A crate source file, as this check reads it.
///
/// A runtime read rather than `include_str!` so the failure names the missing
/// file: a check that could not read its evidence would report every column
/// unwritten, which is the cry-wolf failure the module docs exist to prevent.
fn read_source(relative: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

#[test]
fn every_migration_013_column_is_written_or_allowlisted_with_its_owner() {
    let db = Database::open_in_memory().expect("an in-memory library");
    db.migrate().expect("migrations");
    let mut unclassified: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut dishonest: Vec<String> = Vec::new();

    for audited in AUDITED {
        let columns = columns(&db, audited);
        let named = named_columns(audited);
        for column in &columns {
            if named.iter().any(|name| name == column) {
                continue;
            }
            if UNWRITTEN
                .iter()
                .any(|(table, allow_column, _)| *table == audited.table && allow_column == column)
            {
                continue;
            }
            unclassified.push(format!(
                "{}.{column}: no writer fills it and no allowlist entry names it. Either \
                 the writer for it has arrived (add the field to the write struct, or add the \
                 column to the statement), or it belongs to a later slice (add an UNWRITTEN \
                 entry with the issue that owns it).",
                audited.table
            ));
        }
        // The exactness half, and it applies to the struct writers only: a
        // struct field is a name, not a keyword, so every one is either a
        // column of this table or a declared `non_columns` field.
        for field in struct_fields_of(audited) {
            if !columns.iter().any(|name| name == &field)
                && !audited.writers.iter().any(|writer| match writer {
                    Writer::Struct { non_columns, .. } => non_columns.contains(&field.as_str()),
                    Writer::Statement { .. } => false,
                })
            {
                stale.push(format!(
                    "{}.{field}: a write struct has a field the schema has no column for, and \
                     it is not declared in `non_columns`. A rename the audit has not followed, \
                     or a field that is not a column at all.",
                    audited.table
                ));
            }
        }
    }

    // The allowlist cannot hide a missing writer: every entry must still be a
    // real, in-scope column, and must still be unwritten.
    for (table, column, owner) in UNWRITTEN {
        let Some(audited) = AUDITED.iter().find(|entry| entry.table == *table) else {
            dishonest.push(format!("{table}.{column}: no such table in AUDITED"));
            continue;
        };
        let columns = columns(&db, audited);
        if !columns.iter().any(|name| name == column) {
            stale.push(format!(
                "{table}.{column}: allowlisted against {owner}, but the column does not exist \
                 in this audit's scope. Remove the entry."
            ));
            continue;
        }
        if named_columns(audited).iter().any(|name| name == column) {
            dishonest.push(format!(
                "{table}.{column}: allowlisted as unwritten against {owner}, but a writer \
                 fills it. Remove the allowlist entry."
            ));
        }
    }

    assert!(
        unclassified.is_empty(),
        "migration-013 columns with neither a writer nor an allowlist entry:\n  {}",
        unclassified.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "allowlist or write surface out of step with the schema:\n  {}",
        stale.join("\n  ")
    );
    assert!(
        dishonest.is_empty(),
        "the allowlist is out of step with the writers:\n  {}",
        dishonest.join("\n  ")
    );
}
/// A table 013 created but this audit does not cover would pass vacuously, so
/// the created-table list and the audited list have to be the same list.
///
/// The counterpart to the column check, and the half that catches a *table*
/// added without an owner: `AUDITED` is what this test reads, and a new
/// `CREATE TABLE` in 013 that nobody added there would leave every one of its
/// columns unchecked while the column check above reported success.
#[test]
fn every_table_migration_013_created_is_audited() {
    let created = created_tables();
    let audited: Vec<&str> = AUDITED.iter().map(|entry| entry.table).collect();
    let missing: Vec<&str> = created
        .iter()
        .map(String::as_str)
        .filter(|table| !audited.contains(table))
        .collect();
    assert!(
        missing.is_empty(),
        "migration 013 created tables this audit does not cover: {missing:?}. \
         A table the audit does not read is a table whose columns nobody checks."
    );
}
