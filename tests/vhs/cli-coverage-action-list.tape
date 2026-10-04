# VHS tape: CLI `scitadel coverage` + `scitadel action_list` (#253, S2).
#
# These are the commands that *report* what is still missing, so what this
# tape pins is that they report it honestly:
#
#   - a work with a recorded want and no artefact is counted missing;
#   - a work whose full text is already held is NOT counted missing;
#   - a work nobody asked for is not counted missing;
#   - a publisher with no DOI is reported as unclassified, with no route verdict.
#
# One screenshot, so the distinct-snapshot gate skips it — this tape is here
# to exercise the command and record what a human reads, not to assert frame
# diversity. `scitadel action_list` is covered by the unit tests, which check
# the property that matters here: its totals equal `coverage`'s.
#
# Seeded with sqlite3 rather than a live network call, in the pattern of
# `annotations-in-detail.tape` — the point is the report, not the lookup.

Output "tests/vhs/snapshots/cli-coverage-action-list/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1500
Set Height 900
Set Padding 10
Set TypingSpeed 40ms

Hide
Type `export PATH="$PWD/target/release:$HOME/.cargo/bin:$PATH"`
Enter
Type `export PS1="$ "`
Enter
Type `export SCITADEL_DB=$(mktemp -d)/scitadel.db`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 800ms

# Seed: one paper (held), one paper (wanted, missing), and one paper nobody
# asked about. The held one is what proves the report does not count what we
# already have.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id,title,authors,year,created_at,updated_at) VALUES ('p-held','Held already','[\"A\"]',2020,datetime('now'),datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id,title,authors,year,created_at,updated_at) VALUES ('p-want','Wanted but missing','[\"B\"]',2021,datetime('now'),datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id,title,authors,year,created_at,updated_at) VALUES ('p-untouched','Never asked for','[\"C\"]',2022,datetime('now'),datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO acquisition_state (paper_id,kind,locator,wanted_version,status,reason,updated_at) VALUES ('p-held','fulltext','','vor','pending',NULL,datetime('now'));"`
Enter
Sleep 150ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO acquisition_state (paper_id,kind,locator,wanted_version,status,reason,updated_at) VALUES ('p-want','fulltext','','vor','needs_ill','no route evaluated',datetime('now'));"`
Enter
Sleep 150ms
# The held artefact. access_status must be 'full_text' — that is what "have"
# is derived from, not merely the existence of a row.
Type `sqlite3 $SCITADEL_DB "INSERT INTO artefacts (id,paper_id,kind,version,locator,access_status,access_basis,route,retrieved_at) VALUES ('a-held','p-held','fulltext_pdf','vor','','full_text','oa_license','publisher',datetime('now'));"`
Enter
Sleep 300ms
Type "clear"
Enter
Show

# `coverage` — one paper wanted and missing, one held, one never asked for.
Type `scitadel coverage`
Enter
Sleep 1500ms
Screenshot "tests/vhs/snapshots/cli-coverage-action-list/01-coverage.png"
