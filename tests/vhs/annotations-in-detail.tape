# VHS tape: annotations visible in the paper detail view (#49 iter 3).
#
# Seeds a paper + one annotation directly via sqlite3, launches the
# TUI, opens the paper, screenshots the annotation listing.

Output "tests/vhs/snapshots/annotations-in-detail/run.gif"

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

# Seed a paper + a root annotation + a reply.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-demo', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'The dominant sequence transduction models are based on complex recurrent or convolutional neural networks that include an encoder and a decoder. We propose a new simple network architecture, the Transformer.', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, paper_id, quote, note, author, anchor_status, tags_json, created_at, updated_at) VALUES ('a-root', 'p-demo', 'the Transformer', 'foundational architecture choice - see fig 1', 'lars', 'ok', '[]', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms
Type `sqlite3 $SCITADEL_DB "INSERT INTO annotations (id, parent_id, paper_id, note, author, anchor_status, tags_json, created_at, updated_at) VALUES ('a-reply', 'a-root', 'p-demo', 'matches section 3.1 of the paper', 'claude-opus-4-7', 'ok', '[]', datetime('now'), datetime('now'));"`
Enter
Sleep 300ms
Type "clear"
Enter
Show

Type "scitadel tui"
Enter
Sleep 2500ms

Tab
Sleep 1500ms
Screenshot "tests/vhs/snapshots/annotations-in-detail/01-papers-tab.png"

Enter
Sleep 2500ms
Screenshot "tests/vhs/snapshots/annotations-in-detail/02-paper-detail-with-annotations.png"

Type "q"
Sleep 300ms
Type "q"
Sleep 300ms
