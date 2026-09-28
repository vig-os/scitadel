# VHS tape: CLI `scitadel search --field title` + `scitadel resolve-doi` (#210).
#
# Exercises the two additions in #210:
#   1. `--field title` routes OpenAlex through `filter=title.search:` so an
#      exact title reliably surfaces its target (Schwarz 1978, DOI
#      10.1214/aos/1176344136).
#   2. `resolve-doi <doi>` resolves a DOI to full metadata via OpenAlex
#      `/works/doi:<doi>` and persists the resulting paper.
#
# Uses OpenAlex live (no auth needed for a light probe with `mailto=`).
# Both queries are well-known and deterministic — the Schwarz 1978 BIC
# paper — so an OpenAlex ordering wobble on this specific title would
# have to be pretty severe to knock it out of the top 5, and the DOI
# resolves to a single canonical work id.

Output "tests/vhs/snapshots/cli-openalex-title-and-doi/run.gif"

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
Type "scitadel init --yes --email demo@example.org --sources openalex --db $SCITADEL_DB"
Enter
Sleep 800ms
Type "clear"
Enter
Show

# ---- 1) --field title: exact title finds Schwarz 1978 ----------------
Type `scitadel search "Estimating the Dimension of a Model" --sources openalex --field title -n 5`
Sleep 300ms
Enter
Sleep 8s
Screenshot "tests/vhs/snapshots/cli-openalex-title-and-doi/01-search-title-mode.png"

Type "clear"
Enter

# ---- 2) resolve-doi: same paper, addressed by DOI ---------------------
Type "scitadel resolve-doi 10.1214/aos/1176344136"
Sleep 300ms
Enter
Sleep 5s
Screenshot "tests/vhs/snapshots/cli-openalex-title-and-doi/02-resolve-doi.png"
