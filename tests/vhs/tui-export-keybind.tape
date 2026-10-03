# VHS tape: TUI export keybind — `E` on Question Dashboard (#135 sub-feature B).
#
# Seeds a question + a single shortlisted paper, opens the Question
# Dashboard, presses `E`, accepts the slug-derived default path, and
# screenshots the success toast.

Output "tests/vhs/snapshots/tui-export-keybind/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1600
Set Height 900
Set Padding 10
Set TypingSpeed 40ms

Hide
Type `export PATH="$PWD/target/release:$HOME/.cargo/bin:$PATH"`
Enter
Type `export PS1="$ "`
Enter
Type `export TMPDIR=$(mktemp -d)`
Enter
# Pin the reader identity rather than inheriting it.
#
# `scitadel tui` reads the reader from `$USER`, falling back to the literal
# "unknown" when it is unset — and a vhs shell does not reliably have `USER`
# exported. Seeding the shortlist with `$USER` could therefore write
# reader='' while the TUI looked for reader='unknown', and the export would
# fail with "export failed: shortlist is empty" for a reason that reads like a
# product bug and is really an unset variable in the test.
Type `export USER=demo`
Enter
Type `export SCITADEL_DB=$TMPDIR/scitadel.db`
Enter
Type `cd $TMPDIR`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 1s

# Seed: 1 question + 1 paper + 1 assessment + 1 shortlist row.
#
# The table is `shortlist_members`, not `shortlist` — migration 010 introduced
# shortlists and named it that. Inserting into `shortlist` fails with "no such
# table", the error scrolls past inside the tape, and the run continues to
# export an empty shortlist, which fails with "export failed: shortlist is
# empty" and renders a frame that looks like it worked.
Type `sqlite3 $SCITADEL_DB "INSERT INTO research_questions (id, text, description, created_at, updated_at) VALUES ('q-attn', 'What is the role of attention?', 'For a lit review section.', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-a', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'The dominant sequence transduction models are based on complex RNNs or CNNs.', datetime('now'), datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO assessments (id, paper_id, question_id, score, reasoning, assessor, created_at) VALUES ('a-1', 'p-a', 'q-attn', 0.95, 'Landmark Transformer paper.', 'claude-opus-4-7', datetime('now'));"`
Enter
Sleep 100ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO shortlist_members (question_id, paper_id, reader, added_at) VALUES ('q-attn', 'p-a', '$USER', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2500ms

# Tab twice to land on Questions.
#
# Twice, not three times: the tab strip is Searches | Papers | Questions |
# Queue, so Questions is the third tab and three presses overshoot onto
# Queue. That overshoot is invisible in the snapshots — `E` and `Enter` are
# both no-ops on Queue, so frames 01-03 come out byte-identical and the
# distinct-snapshot gate fails with a message that points at rendering rather
# than at navigation. Matches `question-dashboard.tape`.
Tab
Sleep 400ms
Tab
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/01-questions-tab.png"

Enter
Sleep 1200ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/02-dashboard-open.png"

# Press `E` (uppercase) to open the export-path prompt.
Type "E"
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/03-export-prompt.png"

# Accept the slug-derived default (`what-is-the-role-of-attention.bib`).
Enter
Sleep 1000ms
Screenshot "tests/vhs/snapshots/tui-export-keybind/04-export-success-toast.png"

Type "q"
Sleep 200ms
Type "q"
Sleep 200ms
