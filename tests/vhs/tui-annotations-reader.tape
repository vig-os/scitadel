# VHS tape: TUI two-pane annotation reader (#97).
#
# Iter 1 of #97: split layout + per-thread background highlights +
# J/K hop between highlights. Multi-line gutter bars and PDF overlay
# stay deferred (separate iter once a real full_text is available).
#
# Seeds a paper with full_text + 3 overlapping annotations + 1
# spanning multiple lines so the highlight palette + multi-line
# wrapping are both exercised.

Output "tests/vhs/snapshots/tui-annotations-reader/run.gif"

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
Type `export SCITADEL_DB=$(mktemp -d)/scitadel.db`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 800ms

# Seed a paper whose `full_text` spans multiple sentences across
# multiple lines.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, full_text, created_at, updated_at) VALUES ('p-demo', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'short abstract here', 'The dominant sequence transduction models are based on complex recurrent or convolutional neural networks. We propose the Transformer, a new simple network architecture based solely on self-attention mechanisms, dispensing with recurrence and convolutions entirely.', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms

# Three overlapping annotations on the seeded full_text; one spans the
# multi-line "based solely on self-attention mechanisms" string so the
# wrapping branch in render_with_highlights gets exercised.
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, paper_id, quote, note, author, anchor_status, tags_json, created_at, updated_at) VALUES ('a-rec', 'p-demo', 'recurrent or convolutional neural networks', 'these are the prior art', 'lars', 'ok', '[]', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, paper_id, quote, note, author, anchor_status, tags_json, created_at, updated_at) VALUES ('a-trans', 'p-demo', 'the Transformer', 'foundational architecture choice', 'claude-opus-4-7', 'ok', '[]', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, paper_id, quote, note, author, anchor_status, tags_json, created_at, updated_at) VALUES ('a-attn', 'p-demo', 'self-attention mechanisms', 'core mechanism — see fig 1', 'lars', 'ok', '[]', datetime('now'), datetime('now'));"`
Enter
Sleep 200ms
# Reply to the Transformer annotation so the right-pane threading path
# is also exercised.
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, parent_id, paper_id, note, author, anchor_status, tags_json, created_at, updated_at) VALUES ('a-trans-reply', 'a-trans', 'p-demo', 'matches section 3.1 of the paper', 'claude-opus-4-7', 'ok', '[]', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms

Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 1500ms

# Tab → Papers, open the seeded paper.
Tab
Sleep 500ms
Enter
Sleep 1s
Screenshot "tests/vhs/snapshots/tui-annotations-reader/01-detail.png"

# R → enter the two-pane reader; left pane shows highlights.
Type "R"
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/02-reader-open.png"

# J / K → cycle through highlights; the focused one underlines.
Shift+J
Sleep 400ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/03-second-highlight.png"
Shift+J
Sleep 400ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/04-third-highlight.png"
Shift+K
Sleep 400ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/05-back-to-second.png"

# R → leave the reader, back to single-pane.
Type "R"
Sleep 400ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/06-back-to-detail.png"

Type "q"
Sleep 300ms
Type "q"
Sleep 300ms
