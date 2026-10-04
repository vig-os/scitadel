//! ADR-007 §1's **untrusted-content envelope**, in the smallest form that is a
//! boundary rather than a convention.
//!
//! > `read_paper` returns full text wrapped in an explicit **untrusted-content
//! > envelope**, with scripts and styles stripped. Fetched content may carry
//! > prompt injection.
//!
//! ADR-007 states the requirement in §5; #287 is the issue that found it did not
//! exist. The part implemented here is the part that has to exist before any
//! consumer can be written safely:
//!
//! 1. **A type, not a convention.** [`UntrustedText`] carries a [`Provenance`]
//!    next to the string, so "is this ours or the document's?" is answered by
//!    the value rather than by a doc comment somebody has to remember.
//! 2. **Neutralisation at the edge.** [`UntrustedText::rendered`] — and
//!    therefore [`std::fmt::Display`], which is what `{}` calls — strips
//!    terminal escape sequences, removes control and invisible formatting
//!    characters, collapses whitespace and caps the length. A caller cannot
//!    reach a terminal or a tool return with an unescaped publisher string by
//!    accident, because the default formatting *is* the escaped form.
//!
//! The raw string is still reachable, as [`UntrustedText::as_str`], and it has to
//! be: the identity matcher compares titles, the manifest mirror records what the
//! publisher said, and a checksum is taken over the stored bytes. Those are not
//! display paths, and the method name says so.
//!
//! # What is deliberately *not* here
//!
//! - **No redaction and no substitution of the text itself.** A caption that
//!   contains an escape sequence is shown as the caption minus the sequence, not
//!   as `[removed]` — a reader deciding whether a document is the one they asked
//!   for needs the words, and the sequence was never part of them.
//! - **No "safe to render" boolean.** [`Provenance::is_trusted`] answers *whose*
//!   text this is, which is a different question from whether it has been
//!   escaped, and a caller that wants the second one has [`UntrustedText::rendered`].
//! - **No prompt-injection envelope.** ADR-007 §5 asks for the *returned* full
//!   text to be wrapped and its scripts and styles stripped. That is a property
//!   of one consumer's output, not of this type, and it stays with #287.
//!
//! # Why the cap is here and not at each call site
//!
//! A publisher caption is not bounded by anything, and every surface that shows
//! one — a table cell, a terminal, an agent's context window — is finite. Capping
//! at the boundary means no surface has to remember to, and a caller that wants
//! the whole string has to say [`UntrustedText::as_str`] out loud.

use std::borrow::Cow;
use std::fmt;

/// The most characters any untrusted string renders as.
///
/// Chosen against the two surfaces that show one: a ratatui table cell (a
/// terminal column, which wraps or clips long before this) and an MCP tool
/// return (an agent's context, where a 40 kB caption costs a great deal and
/// carries nothing a reader did not ask for). A real figure caption is one line;
/// the longest in raid's corpus is 611 characters, so this keeps every genuine
/// caption whole.
pub const MAX_UNTRUSTED_CHARS: usize = 200;

/// The marker appended to a string [`MAX_UNTRUSTED_CHARS`] truncated.
///
/// `…` rather than `...`: one cell wide, so the cap is one character past the
/// limit rather than three, and it is a character no terminal has to be told
/// about. It is deliberately **not** stripped by [`UntrustedText::rendered`],
/// which is what makes truncation visible rather than silent.
const TRUNCATION_MARKER: char = '\u{2026}';

/// `ESC`, the byte every terminal control sequence starts with.
const ESC: char = '\u{1b}';

/// Who put this string in the library.
///
/// The distinction is not "trusted" versus "untrusted" in the abstract — it is
/// *which side of the trust boundary the string is on*, which is what a reader
/// weighing a document needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provenance {
    /// **Ours.** scitadel's own statement about the work: `papers.title`, a
    /// reason a machine here wrote, a route label, a status.
    ///
    /// "Ours" means *we chose this string to describe the work*, not that we
    /// verified it. An arXiv title is `Ours` in this sense the moment it is
    /// stored on the work, and it is still rendered through
    /// [`UntrustedText::rendered`] — a feed can carry a hostile string, and
    /// `scitadel scan` lets a person edit a title.
    Ours,
    /// **Not ours.** Whatever produced the bytes — or the file — wrote this: a
    /// PDF's `/Title`, a served page's `citation_title`, a JATS `<article-title>`,
    /// a `meta.json` figure caption, a `tables.json` table caption, a
    /// `fig1.caption.txt` sidecar, a filename, a title in an NDJSON queue somebody
    /// handed us.
    ///
    /// The name says "the document's side of the boundary" because that is where
    /// these strings come from; it is **not** a claim about who typed this
    /// particular one. A file a person dropped on disk carries the same risk as one
    /// a publisher served, and neither is a third variant: the two differ in how
    /// they got here, not in who can set the string, and a marker that implied
    /// otherwise would be a distinction nothing could honour.
    PublisherSupplied,
}

impl Provenance {
    /// Did scitadel choose this string, rather than receive it with a document?
    ///
    /// The one question a consumer has to ask before treating text as scitadel's
    /// own. Note what it does *not* say: `true` is not a licence to render
    /// unescaped. [`UntrustedText::rendered`] is still the way to render either.
    #[must_use]
    pub fn is_trusted(self) -> bool {
        matches!(self, Self::Ours)
    }

    /// How a report or a reader names this provenance.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ours => "scitadel's own record",
            Self::PublisherSupplied => "from the document (untrusted)",
        }
    }
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A string that entered the library from outside scitadel, tagged with where it
/// came from and neutralised at the point it is shown.
///
/// Construct it through [`UntrustedText::ours`] or
/// [`UntrustedText::publisher_supplied`] so the marker is written at the call
/// site where the provenance is known, and render it through
/// [`Self::rendered`] or `Display`.
///
/// ```
/// use scitadel_core::untrusted::{Provenance, UntrustedText};
///
/// // A PDF's `/Title`, as `identity::pdf_title` hands it over: an OSC 8 hyperlink,
/// // which would otherwise make a title clickable in any terminal that honours it.
/// // The visible text sits between the link's opening and its closing, and the
/// // whole sequence — including both terminators — is what gets stripped.
/// // `\x1b` rather than `\u{1b}` because a doc comment turns the latter into a real
/// // escape byte before the doctest is ever compiled.
/// let raw = "Real Title\x1b]8;;https://x;;\x1b\\link\x1b]8;;\x1b\\";
/// let title = UntrustedText::publisher_supplied(raw);
///
/// assert_eq!(title.rendered(), "Real Title link");
/// assert!(!title.provenance().is_trusted());
/// assert_eq!(title.as_str(), raw, "the stored bytes are untouched");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedText {
    text: String,
    provenance: Provenance,
}

impl UntrustedText {
    /// Tag `text` with its provenance.
    pub fn new(text: impl Into<String>, provenance: Provenance) -> Self {
        Self {
            text: text.into(),
            provenance,
        }
    }

    /// scitadel's own statement about the work — see [`Provenance::Ours`].
    #[must_use]
    pub fn ours(text: impl Into<String>) -> Self {
        Self::new(text, Provenance::Ours)
    }

    /// A string that came out of a document or a sidecar beside one — see
    /// [`Provenance::PublisherSupplied`].
    #[must_use]
    pub fn publisher_supplied(text: impl Into<String>) -> Self {
        Self::new(text, Provenance::PublisherSupplied)
    }

    /// Where this string came from. The answer to "is this ours or the
    /// document's?", recoverable at the call site without re-reading the code
    /// that built it.
    #[must_use]
    pub fn provenance(&self) -> Provenance {
        self.provenance
    }

    /// Did scitadel choose this string?
    #[must_use]
    pub fn is_trusted(&self) -> bool {
        self.provenance.is_trusted()
    }

    /// The string **exactly as stored**, for matching, hashing and serialisation.
    ///
    /// Never render this. It carries whatever the publisher put in it — terminal
    /// escape sequences included — which is the whole reason this type exists.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the string changed at all under neutralisation: a hint for a
    /// caller that wants to say so ("this title contained control characters").
    #[must_use]
    pub fn needs_neutralising(&self) -> bool {
        self.rendered() != self.text
    }

    /// The string as it may be shown to a human or handed to an agent: escape
    /// sequences stripped, control and invisible formatting characters removed,
    /// whitespace collapsed, and the length capped at [`MAX_UNTRUSTED_CHARS`].
    ///
    /// Borrows when there is nothing to neutralise, so the common case costs no
    /// allocation.
    #[must_use]
    pub fn rendered(&self) -> Cow<'_, str> {
        match strip(&self.text) {
            // Nothing to neutralise and nothing to cut: the stored string is
            // already the rendered one, and it is borrowed rather than rebuilt.
            Cow::Borrowed(_) if !exceeds_cap(&self.text) => Cow::Borrowed(&self.text),
            // Nothing to neutralise but too long — which is the common shape of a
            // long caption, and the one where an early return would skip the cap.
            Cow::Borrowed(_) => {
                let mut owned = self.text.clone();
                cap(&mut owned);
                Cow::Owned(owned)
            }
            Cow::Owned(mut neutralised) => {
                cap(&mut neutralised);
                Cow::Owned(neutralised)
            }
        }
    }
}

/// `Display` is the neutralised form, deliberately.
///
/// This is the load-bearing choice of the module: `format!("{title}")`,
/// `writeln!(out, "{title}")` and a `format_args!` capture all go through here,
/// so the unsafe way to render an untrusted string is the one that requires
/// reaching for [`UntrustedText::as_str`] by name.
impl fmt::Display for UntrustedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.rendered())
    }
}

/// The most characters [`UntrustedText::rendered`] produces, marker included.
///
/// [`MAX_UNTRUSTED_CHARS`] plus one: a caller sizing a column from this knows
/// the width without having to reason about the marker.
pub const MAX_RENDERED_CHARS: usize = MAX_UNTRUSTED_CHARS + 1;

/// Remove everything from `text` that changes what a reader sees without
/// changing what it says.
///
/// Three classes, and each is here for its own reason:
///
/// - **Escape sequences.** A CSI (`ESC [ … m`, `ESC [ 2 J`) can move the cursor
///   and repaint the screen; an OSC (`ESC ] 8 ; … ST`) makes a title a clickable
///   link. This repository's TUI is a terminal application, so both reach a
///   human's screen.
/// - **Control characters**, via [`char::is_control`]: C0, `DEL`, and the C1
///   block. `newline` included — a caption with an embedded newline reflows a
///   table row, and the whitespace collapse below already handles it as a space.
/// - **Invisible formatting characters**, which are neither control characters
///   nor whitespace and so survive both of the checks above: the bidi overrides
///   (`U+202A`–`U+202E`) reorder what the string *looks* like, and the
///   zero-width characters (`U+200B`–`U+200D`, `U+2060`–`U+2064`, `U+FEFF`)
///   make two different strings render identically. A reader deciding whether a
///   document is the one they asked for is exactly the reader those attack.
///
/// Whitespace runs become one space and the ends are trimmed, so the result is
/// always a single line.
fn strip(text: &str) -> Cow<'_, str> {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    // Set by an escape sequence and spent by the next printable character. An
    // escape is a separator rather than nothing: `Title<OSC 8>link<ST>` is two
    // words once the sequence is gone, and joining them would invent a word
    // nobody wrote.
    let mut after_escape = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == ESC {
            changed = true;
            after_escape = true;
            skip_escape_sequence(&mut chars);
            continue;
        }
        // Before `is_invisible`, because the common control characters *are*
        // whitespace: a caption with an embedded newline becomes one line with a
        // single space where it was, rather than losing the word boundary.
        if c.is_whitespace() {
            let mut collapsed = false;
            while chars.peek().is_some_and(|next| next.is_whitespace()) {
                chars.next();
                collapsed = true;
            }
            if out.is_empty() || out.ends_with(' ') {
                // Leading whitespace is dropped rather than turned into a space,
                // and a run that follows one adds nothing.
                changed = true;
                continue;
            }
            // Three ways a whitespace character stops being itself: a run became
            // one space, a non-space (`\n`, `\t`, U+00A0) became a space at all,
            // or an escape left a separator behind.
            changed |= collapsed || c != ' ' || after_escape;
            after_escape = false;
            out.push(' ');
            continue;
        }
        if is_invisible(c) {
            changed = true;
            continue;
        }
        if after_escape && !out.ends_with(' ') {
            out.push(' ');
            changed = true;
        }
        after_escape = false;
        out.push(c);
    }
    // Trailing whitespace is trimmed the same way — including a space an escape
    // left behind, which would otherwise end the string.
    while out.ends_with(' ') {
        out.pop();
        changed = true;
    }

    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

/// Consume the rest of an escape sequence, `ESC` already taken.
///
/// Every branch is best-effort: a truncated sequence consumes what is left and
/// stops. That is the conservative direction — an unterminated OSC must not
/// leave its payload behind as if it were text the publisher meant a reader to
/// see, and the alternative (stopping at a boundary we cannot find) is how a
/// "harmless" escape becomes a partial repaint.
fn skip_escape_sequence<I: Iterator<Item = char>>(chars: &mut I) {
    let Some(intro) = chars.next() else { return };
    match intro {
        // CSI: parameter bytes 0x30–0x3F, intermediate bytes 0x20–0x2F, then
        // exactly one final byte 0x40–0x7E. Consuming the final byte is what
        // stops `ESC [ 3 1 m` from leaving its parameters as text.
        '[' => {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        // OSC (`]`), DCS (`P`), SOS (`^`), PM (`X`) and APC (`_`): a string
        // running to `ST` (`ESC \`) or `BEL`. OSC 8 hyperlinks are the reason
        // this list starts with `]`.
        ']' | 'P' | '^' | 'X' | '_' => skip_string_sequence(chars),
        // A character-set designation — `ESC ( B`, `ESC ) 0` — is `ESC`, an
        // intermediate byte, and the designation itself.
        '(' | ')' | '*' | '+' | '-' | '.' | '/' => {
            chars.next();
        }
        // `ESC 7` / `ESC 8` (save/restore cursor) and every other two-byte
        // escape: nothing more to consume.
        _ => {}
    }
}

/// Consume a control *string* up to `ST` (`ESC \`) or `BEL`, including the
/// terminator.
fn skip_string_sequence<I: Iterator<Item = char>>(chars: &mut I) {
    while let Some(c) = chars.next() {
        if c == '\u{7}' {
            return;
        }
        if c == ESC {
            // `ESC \` is ST. Anything else after an ESC inside a string is the
            // start of a nested sequence, and the safest reading is that the
            // string never terminated — so keep consuming rather than return
            // and let a nested payload reach the output.
            if chars.next() == Some('\\') {
                return;
            }
        }
    }
}

/// Control or invisible-formatting character.
fn is_invisible(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            // Bidi embedding/override and the marks that go with them.
            '\u{200e}' | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
            // Zero-width space/non-joiner/joiner and word joiner.
                | '\u{200b}'..='\u{200d}'
                | '\u{2060}'..='\u{2064}'
            // A byte-order mark: it is a signature, never content.
                | '\u{feff}'
        )
}

/// Is `text` longer than [`MAX_UNTRUSTED_CHARS`]?
///
/// Counting to the limit and stopping, rather than counting the whole string: a
/// short title is the common case and must not cost a scan of a 40 kB caption to
/// find out that it is short.
fn exceeds_cap(text: &str) -> bool {
    text.char_indices().nth(MAX_UNTRUSTED_CHARS).is_some()
}

/// Cap `text` at [`MAX_UNTRUSTED_CHARS`] in place, marking the cut.
///
/// The cut is on a **character** boundary, never a byte one, so the result is
/// always valid UTF-8 — a borrowed `Cow` must stay valid, and a caption in a
/// non-Latin script is the case where a byte cut would panic.
fn cap(text: &mut String) {
    let Some(cut) = text
        .char_indices()
        .nth(MAX_UNTRUSTED_CHARS)
        .map(|(at, _)| at)
    else {
        return;
    };
    text.truncate(cut);
    text.push(TRUNCATION_MARKER);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_distinguishes_our_title_from_a_publisher_supplied_one() {
        let ours = UntrustedText::ours("Deep Learning for Imaging");
        let theirs = UntrustedText::publisher_supplied("Deep Learning for Imaging");

        // Same words, different answers — which is the entire point of the type:
        // the two are distinguishable by value, not by convention.
        assert_eq!(ours.as_str(), theirs.as_str());
        assert_ne!(ours.provenance(), theirs.provenance());

        assert!(ours.provenance().is_trusted());
        assert!(!theirs.provenance().is_trusted());
        assert_eq!(ours.is_trusted(), ours.provenance().is_trusted());

        // Neither is trusted-by-omission, and `Display` names which is which,
        // so a report can say it rather than implying it.
        assert_eq!(
            ours.provenance().label(),
            "scitadel's own record",
            "our own title is labelled as ours"
        );
        assert_eq!(
            theirs.provenance().label(),
            "from the document (untrusted)",
            "a title out of a PDF is labelled as the document's"
        );
        assert_ne!(
            ours.provenance().to_string(),
            theirs.provenance().to_string()
        );

        // And neither changes its bytes on the way through: neutralisation is a
        // rendering concern, not a storage one.
        assert_eq!(ours.as_str(), "Deep Learning for Imaging");
        assert_eq!(theirs.as_str(), "Deep Learning for Imaging");
    }

    /// The security-relevant one: a title out of a document's bytes must not be
    /// able to reach a terminal intact.
    #[test]
    fn an_artefact_title_with_terminal_escapes_is_neutralised() {
        // Every shape a terminal actually honours, from the one that can repaint
        // the screen to the one that makes a title a clickable link.
        let hostile = concat!(
            "Real Title",
            "\u{1b}[31m",
            "\u{1b}[2J\u{1b}[H",
            "\u{1b}]8;;https://attacker.example/\u{1b}\\click me\u{1b}]8;;\u{1b}\\",
            "\u{1b}(B",
            "\u{1b}]0;window title\u{07}",
            " \u{7} \u{9}\n",
            "\u{202e}reversed\u{202c}",
        );
        let title = UntrustedText::publisher_supplied(hostile);

        let rendered = title.rendered();
        assert!(
            !rendered.contains('\u{1b}'),
            "no escape byte may survive: {rendered:?}"
        );
        assert!(
            !rendered.chars().any(char::is_control),
            "no control character may survive: {rendered:?}"
        );
        for (label, needle) in [
            ("CSI", "31m"),
            ("erase-display", "2J"),
            ("cursor-home", "[H"),
            ("OSC 8", "https://attacker.example/"),
            ("OSC 0", "window title"),
            ("a bidi override", "\u{202e}"),
        ] {
            assert!(
                !rendered.contains(needle),
                "the {label} payload must not survive: {rendered:?}"
            );
        }

        // The words are kept: a reader deciding whether this is the paper they
        // asked for needs the caption, and the escape was never part of it.
        assert!(
            rendered.starts_with("Real Title click me reversed"),
            "the text survives and reads as one line: {rendered:?}"
        );

        // `Display` goes through the same path, which is what makes `{}` safe.
        assert_eq!(rendered, title.to_string());
        assert_eq!(rendered, format!("{title}"));
        assert_ne!(
            title.as_str(),
            rendered,
            "the stored string is untouched: neutralisation is a rendering concern"
        );
        assert!(
            title.needs_neutralising(),
            "and the caller can tell that it changed something"
        );
    }

    /// A caption is not bounded by anything and every surface showing one is.
    #[test]
    fn an_artefact_title_is_capped_in_length() {
        let long = "word ".repeat(500);
        let title = UntrustedText::publisher_supplied(&long);

        let rendered = title.rendered();
        assert_eq!(
            rendered.chars().count(),
            MAX_RENDERED_CHARS,
            "the cap is the limit plus the marker"
        );
        assert!(rendered.ends_with(TRUNCATION_MARKER), "{rendered:?}");
        assert!(
            rendered.starts_with("word word word"),
            "what is kept is the beginning of the caption, in order: {rendered:?}"
        );
        assert_eq!(
            title.as_str().chars().count(),
            2_500,
            "the stored caption is not truncated"
        );

        // The cap is on characters, not bytes, so a multi-byte caption is cut on
        // a boundary and the result is still valid UTF-8.
        let wide = UntrustedText::publisher_supplied("\u{1f9ea}".repeat(400)); // 🧪
        let capped = wide.rendered();
        assert_eq!(capped.chars().count(), MAX_RENDERED_CHARS);
        assert!(std::str::from_utf8(capped.as_bytes()).is_ok());

        // Exactly at the limit is not truncated, and one over is — the boundary
        // is tested from both sides rather than asserted once.
        for (n, truncated) in [
            (MAX_UNTRUSTED_CHARS, false),
            (MAX_UNTRUSTED_CHARS + 1, true),
            (MAX_UNTRUSTED_CHARS + 2, true),
        ] {
            let exact = UntrustedText::publisher_supplied("x".repeat(n));
            let rendered = exact.rendered();
            assert_eq!(
                rendered.ends_with(TRUNCATION_MARKER),
                truncated,
                "{n} characters"
            );
            assert_eq!(
                rendered.chars().count(),
                if truncated { MAX_RENDERED_CHARS } else { n },
                "{n} characters"
            );
        }
        let short = UntrustedText::publisher_supplied("A short title");
        assert!(
            matches!(short.rendered(), Cow::Borrowed(_)),
            "a clean string is borrowed, not rebuilt"
        );
    }

    /// Our own strings go through the same neutraliser. They are less dangerous,
    /// not exempt: `papers.title` arrived from a feed once, and `scan` lets a
    /// person write one.
    #[test]
    fn our_own_text_is_neutralised_too() {
        let ours = UntrustedText::ours("A\u{1b}[2Jtitle");
        assert_eq!(ours.rendered(), "A title");
        assert!(ours.provenance().is_trusted());
    }

    /// Whitespace: collapsed, trimmed, and one line — a caption with an embedded
    /// newline must not reflow a table row.
    #[test]
    fn whitespace_is_collapsed_and_the_ends_are_trimmed() {
        for (raw, expected) in [
            ("  leading and trailing  ", "leading and trailing"),
            ("a\n\nb\t\tc", "a b c"),
            ("a\u{a0}b", "a b"),
            (
                "\u{2028}line and paragraph separators\u{2029}",
                "line and paragraph separators",
            ),
            // An escape separates rather than vanishes: the words either side of
            // it were two words, and joining them would invent one nobody wrote.
            ("Title\u{1b}[0m click", "Title click"),
        ] {
            let text = UntrustedText::publisher_supplied(raw);
            let rendered = text.rendered();
            assert_eq!(rendered, expected, "{raw:?}");
            assert!(
                !rendered.contains("  "),
                "no interior whitespace runs survive: {rendered:?}"
            );
            assert_eq!(
                rendered.trim(),
                rendered,
                "and neither end carries one: {rendered:?}"
            );
        }
    }

    /// A truncated escape must not leave its payload behind as text. An
    /// unterminated OSC is the case that matters: dropping it at a boundary we
    /// cannot find is how a partial sequence becomes a partial repaint.
    #[test]
    fn a_truncated_escape_sequence_is_consumed_to_the_end() {
        for hostile in [
            "Title\u{1b}]8;;https://attacker.example/never closed",
            "Title\u{1b}[31",
            "Title\u{1b}",
            "Title\u{1b}]",
            "Title\u{1b}]0;window\u{1b}\\still open",
        ] {
            let text = UntrustedText::publisher_supplied(hostile);
            let rendered = text.rendered();
            assert!(
                !rendered.contains('\u{1b}'),
                "no escape survives: {hostile:?} → {rendered:?}"
            );
            assert!(
                !rendered.contains("attacker.example"),
                "an unterminated payload is not left as text: {rendered:?}"
            );
        }
    }

    /// Storage, serialisation and matching all take the raw string, and it is
    /// the one accessor that hands it over — so "raw reached the output" is
    /// always a named call rather than a default.
    #[test]
    fn as_str_hands_over_the_stored_bytes_verbatim() {
        let hostile = "Title\u{1b}[2J\u{1b}]8;;https://attacker.example/\u{1b}\\";
        let title = UntrustedText::publisher_supplied(hostile);
        assert_eq!(title.as_str(), hostile);
        assert_eq!(
            serde_json::to_string(title.as_str()).expect("serialise"),
            "\"Title\\u001b[2J\\u001b]8;;https://attacker.example/\\u001b\\\\\"",
            "and a serialiser writes the stored bytes, because the mirror's \
             fidelity is a separate requirement from display safety"
        );
    }
}
