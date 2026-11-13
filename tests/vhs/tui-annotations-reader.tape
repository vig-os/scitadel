# VHS tape: TUI two-pane annotation reader (#97, #307).
#
# Iter 1 of #97: split layout + per-thread background highlights +
# J/K hop between highlights. Multi-line gutter bars and PDF overlay
# stay deferred (separate iter once a real full_text is available).
#
# #307 adds scroll keys (j/k/arrows for one row, d/u + the page
# keys for a half-page, g/G for top/bottom) and auto-scrolls on a
# J/K highlight hop. The longer body + scroll-screenshot block
# below exercises both the plain scroll and the auto-follow.
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

# Seed a paper whose `full_text` spans many paragraphs, so the
# reader's viewport is smaller than the body and scrolling actually
# moves something on screen (#307). The three quote targets below
# all live in the first paragraph; later paragraphs are filler so
# `below_fold` material is reachable through j/d/G.
Type `sqlite3 $SCITADEL_DB "INSERT INTO papers (id, title, authors, year, abstract, full_text, created_at, updated_at) VALUES ('p-demo', 'Attention Is All You Need', '[\"Vaswani, A.\"]', 2017, 'short abstract here', 'The dominant sequence transduction models are based on complex recurrent or convolutional neural networks. We propose the Transformer, a new simple network architecture based solely on self-attention mechanisms, dispensing with recurrence and convolutions entirely.' || X'0A' || X'0A' || 'The Transformer removes the recurrence and computes representations in parallel across positions, shortening the dependency path between any two tokens to a single attention step.' || X'0A' || X'0A' || 'Encoder and decoder are stacks of identical layers; each layer carries a multi-head self-attention sub-layer and a position-wise feed-forward sub-layer, each wrapped in a residual connection and layer normalization.' || X'0A' || X'0A' || 'Positional encodings replace the order the recurrence used to give for free. Sinusoidal encodings extrapolate to sequence lengths past the training set; learned positional embeddings are a drop-in alternative at the same cost.' || X'0A' || X'0A' || 'Training uses Adam with a warmup-then-decay schedule, dropout on the sub-layer outputs and embeddings, and label smoothing of the target distribution. Regularisation matters because the model has no inductive bias for locality.' || X'0A' || X'0A' || 'Evaluation on WMT 2014 English-to-German and English-to-French reaches new state-of-the-art BLEU scores at a fraction of the training cost of the best prior models, and the architecture generalises to English constituency parsing with limited task-specific tuning.', datetime('now'), datetime('now'));"`
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

# #307 scroll keys. The body is now tall enough that j/d/G move
# the viewport visibly; g snaps back to the top.
Type "j"
Sleep 200ms
Type "j"
Sleep 200ms
Type "j"
Sleep 300ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/06-scrolled-three-lines.png"
Type "d"
Sleep 300ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/07-scrolled-half-page.png"
Shift+G
Sleep 300ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/08-scrolled-bottom.png"
Type "g"
Sleep 300ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/09-scrolled-top.png"

# R → leave the reader, back to single-pane. `R` also resets the
# scroll offset (#307), so a reopen starts at the top.
Type "R"
Sleep 400ms
Screenshot "tests/vhs/snapshots/tui-annotations-reader/10-back-to-detail.png"

Type "q"
Sleep 300ms
Type "q"
Sleep 300ms
