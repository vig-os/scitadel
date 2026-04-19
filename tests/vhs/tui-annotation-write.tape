# VHS tape: TUI-native annotation create / edit / delete / reply (#92).
#
# Iter 3b of #49: n / e / d / r keybindings on the paper detail
# overlay. Seeds a paper via sqlite3 (no network), opens the TUI,
# steps through the four-action lifecycle.

Output "tests/vhs/snapshots/tui-annotation-write/run.gif"

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 900
Set Padding 10
Set TypingSpeed 40ms

Hide
Type `export PATH="$HOME/.cargo/bin:$PWD/target/release:$PATH"`
Enter
Type `export PS1="$ "`
Enter
Type `export SCITADEL_DB=$(mktemp -d)/scitadel.db`
Enter
Type "scitadel init --yes --email demo@example.org --sources arxiv --db $SCITADEL_DB"
Enter
Sleep 800ms

# Seed a paper. Annotations will be created via the TUI itself so the
# tape exercises every keybinding listed in #92.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, created_at, updated_at) VALUES ('p-demo', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'The dominant sequence transduction models are based on complex recurrent or convolutional neural networks. We propose the Transformer, a new simple network architecture.', datetime('now'), datetime('now'));"`
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
Screenshot "tests/vhs/snapshots/tui-annotation-write/01-paper-detail.png"

# n → Create prompt, type quote, Enter, type note, Enter.
Type "n"
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/02-create-quote.png"
Type "the Transformer"
Sleep 300ms
Enter
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/03-create-note.png"
Type "foundational architecture choice"
Sleep 300ms
Enter
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/04-after-create.png"

# Shift+J → focus the new annotation.
Shift+J
Sleep 500ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/05-focused.png"

# e → Edit prompt with note pre-filled.
Type "e"
Sleep 400ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/06-edit-prefilled.png"
Type " — see fig 1"
Sleep 300ms
Enter
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/07-after-edit.png"

# r → Reply prompt.
Type "r"
Sleep 400ms
Type "agreed; matches section 3.1"
Sleep 300ms
Enter
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/08-after-reply.png"

# d → Delete confirm, y to confirm.
Type "d"
Sleep 400ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/09-delete-confirm.png"
Type "y"
Sleep 800ms
Screenshot "tests/vhs/snapshots/tui-annotation-write/10-after-delete.png"

Type "q"
Sleep 300ms
Type "q"
Sleep 300ms
