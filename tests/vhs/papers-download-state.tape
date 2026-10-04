# VHS tape: Papers table renders the download-state column (#112, re-derived in
# #253's S2e).
#
# Seeds 4 papers so the symbol column renders all four glyphs side-by-side:
# blank, ✓, ⊘, ✗.
#
# The four states are now DERIVED from `artefacts` + `acquisition_state`
# (ADR-007 §1 "Have"), not read from `papers.download_status` — that column is no
# longer written by anything, so the seeding below records artefacts instead. The
# glyphs are unchanged, which is the point of the change: the same four answers,
# from one derivation.
#
#   blank  p-none  nothing recorded          -> untracked
#   ✓      p-ok    a `full_text` PDF artefact
#   ⊘      p-pay   a `fulltext_html` artefact with access_status = 'paywall'
#   ✗      p-fail  no artefact, and a recorded gap whose status is not 'pending'
#
# NOTE: this tape was updated but could not be rendered locally (no `ttyd`, no
# `ffmpeg` on this machine). CI renders it.

Output "tests/vhs/snapshots/papers-download-state/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 600
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

# Seed 4 papers, then the artefact rows (or the gap row) each one derives from.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, created_at, updated_at) VALUES ('p-none', 'Paper Never Tried', '[\"Alice\"]', '', datetime('now', '-4 hours'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, created_at, updated_at) VALUES ('p-ok', 'Paper Successfully Downloaded', '[\"Bob\"]', '', datetime('now', '-3 hours'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, created_at, updated_at) VALUES ('p-pay', 'Paper Behind Paywall', '[\"Carol\"]', '', datetime('now', '-2 hours'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, abstract, created_at, updated_at) VALUES ('p-fail', 'Paper Download Failed', '[\"Dave\"]', '', datetime('now', '-1 hours'), datetime('now'));"`
Enter
Sleep 200ms

# p-ok: we hold the full text. The blob row and the artefact row are both required —
# artefacts.sha256 is a foreign key into blobs.
Type `sqlite3 $SCITADEL_DB "INSERT INTO blobs (sha256, bytes, mime, rel_path, created_at) VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 4, 'application/pdf', 'blobs/aa/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.pdf', datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO artefacts (id, paper_id, kind, version, locator, sha256, format, access_status, route, access_basis, retrieved_at) VALUES ('a-ok', 'p-ok', 'fulltext_pdf', 'vor', '', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'pdf', 'full_text', 'publisher', 'oa_license', datetime('now'));"`
Enter
Sleep 100ms

# p-pay: we hold a landing page that is not the paper.
Type `sqlite3 $SCITADEL_DB "INSERT INTO blobs (sha256, bytes, mime, rel_path, created_at) VALUES ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 4, 'text/html', 'blobs/bb/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.html', datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO artefacts (id, paper_id, kind, version, locator, sha256, format, access_status, route, access_basis, retrieved_at) VALUES ('a-pay', 'p-pay', 'fulltext_html', 'vor', '', 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'html', 'paywall', 'publisher', 'subscription_read', datetime('now'));"`
Enter
Sleep 100ms

# p-fail: nothing held, and a recorded gap that concluded something. `pending` is
# the only status that means "never tried", so it would render blank instead.
Type `sqlite3 $SCITADEL_DB "INSERT INTO acquisition_state (paper_id, kind, locator, wanted_version, status, updated_at) VALUES ('p-fail', 'fulltext', '', 'vor', 'error', datetime('now'));"`
Enter
Sleep 200ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 1500ms

# Tab to Papers tab — table should show 4 rows with blank/✓/⊘/✗ symbols.
Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/papers-download-state/01-papers-table.png"

Type "q"
Sleep 300ms
Screenshot "tests/vhs/snapshots/papers-download-state/02-quit.png"
