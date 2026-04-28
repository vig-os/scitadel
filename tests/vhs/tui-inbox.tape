# VHS tape: status-bar [N new] badge, papers-list ● glyph,
# inbox overlay (U), and Enter-to-jump (#185 P0).
#
# Seeds a paper + a root annotation + a reply via sqlite3 so the
# unread state is non-zero on first launch. Exercises:
#   1. [N new] badge in the status bar
#   2. ● glyph on the Papers row
#   3. U opens the inbox overlay
#   4. Enter jumps to two-pane reader focused on the thread
#   5. Esc out → focus-leave clears the badge

Output "tests/vhs/snapshots/tui-inbox/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
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

# Seed: one paper + a root annotation with a reply, both authored by
# `claude` so they're unread for the default reader (`lars`). The two
# rows mean the badge reads `[2 new]` and the inbox lists one thread
# with one reply under it.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-attn', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'The dominant sequence transduction models are based on complex recurrent or convolutional neural networks.', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, paper_id, quote, anchor_status, note, tags_json, author, created_at, updated_at) VALUES ('ann-root-1', 'p-attn', 'Attention', 'ok', 'Why is multi-head better than single?', '[\"transformer\"]', 'claude', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, parent_id, paper_id, anchor_status, note, tags_json, author, created_at, updated_at) VALUES ('ann-reply-1', 'ann-root-1', 'p-attn', 'ok', 'See section 3.2.2 — parallel subspaces.', '[]', 'claude', datetime('now', '+1 second'), datetime('now', '+1 second'));"`
Enter
Sleep 300ms
Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 1500ms
# Status bar should show `[2 new]`.
Screenshot "tests/vhs/snapshots/tui-inbox/01-badge-on-launch.png"

# Tab → Papers tab so the ● glyph row is visible.
Tab
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-inbox/02-papers-list-with-bullet.png"

# U → Inbox overlay opens with the thread + reply listed under the
# paper title.
Type "U"
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-inbox/03-inbox-open.png"

# Enter → jump to two-pane reader focused on the thread.
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/tui-inbox/04-reader-focused-on-thread.png"

# Esc out of the reader, then Esc to close the overlay. The
# focus-leave path marks both root + reply as seen, so the badge
# should clear and the ● glyph on the Papers row should disappear.
Escape
Sleep 300ms
Escape
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-inbox/05-badge-cleared.png"

Type "q"
Sleep 400ms
