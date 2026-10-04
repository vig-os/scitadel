//! ADR-007 §3, last paragraph: the identity matcher.
//!
//! > **Identity** is checked twice, and a mismatch blocks filing anything under
//! > that DOI.
//! >
//! > - **Pre-fetch**, in S2 for every route: the expected title against the
//! >   resolved one (OpenAlex → Crossref → DataCite), fuzzy-matched with
//! >   year ±1 and the first author as tie-breakers.
//! > - **Post-fetch**, in S2 for HTTP routes and S4 for the browser: the
//! >   served page or PDF title against OpenAlex. This catches redirects to
//! >   the wrong paper.
//!
//! This module is the "fuzzy-matched with year ±1 and the first author as
//! tie-breakers" half. The rows it produces live in `paper_identity_checks`
//! (`scitadel_db::sqlite::identity`); the gate that refuses to file is
//! `crate::download`.
//!
//! # The decision table
//!
//! One function, [`verify`], and no other place in the workspace decides
//! whether two titles are the same work. The thresholds below are not guesses —
//! each is a measured score on the pairs [`tests`] pins, and the bands are
//! chosen where the pairs stop being separable:
//!
//! | normalised similarity | verdict |
//! |---|---|
//! | ≥ [`MATCH_THRESHOLD`] (0.90) | `ok`, **if** a year or first author corroborates |
//! | < [`MISMATCH_FLOOR`] (0.70) | `mismatch`, unconditionally |
//! | between | the tie band: year ±1 and the first author decide |
//!
//! The bands are where the measurements put the real cases:
//!
//! | pair | score | band |
//! |---|---|---|
//! | identical | 1.000 | match |
//! | diacritics spelled differently (`Müller`/`Muller`) | 1.000 | match |
//! | transposed characters | 0.991 | match |
//! | one word changed (`for` → `in`) | 0.974 | match |
//! | served page kept its site suffix | 0.921 | match |
//! | **a different paper** with the same words + ` of the thyroid` | 0.885 | tie |
//! | served title is a short form of the stored one | 0.853 | tie |
//! | preprint title + `: a multicenter study` | 0.853 | tie |
//! | preprint title + a leading `A multicenter study of` | 0.853 | tie |
//! | stored short form, registry's longer title | 0.821 | tie |
//! | the stored title with `: methods, results and conclusions` | 0.784 | tie |
//! | a different paper in the same field | 0.683 | mismatch |
//! | the same subject, a completely different article | 0.646 | mismatch |
//! | word order swapped | 0.582 | mismatch |
//! | an unrelated paper | 0.538 | mismatch |
//!
//! The tie band's first row is the whole reason the band exists, and the reason is
//! worse than "the scores are close": **the wrong answer scores higher than the
//! right one.** 0.885 for a *different* work is 0.853 for the *same* work with a
//! shorter title, so no threshold orders them correctly — raising it far enough to
//! separate them also puts four same-work pairs into the mismatch band and refuses
//! correct fetches. The tie is therefore broken by the two facts that can, and a
//! missing one of those is [`Verdict::Unverified`] rather than a pass.
//!
//! # Why `unverified` is not `ok`
//!
//! Three ways to arrive at `ok` without having checked anything:
//!
//! - a title we could not read (a PDF with no `/Title`, a landing page with no
//!   `citation_title`) — nothing was compared;
//! - a title that half matched and a year nobody recorded — the corroboration
//!   ADR-007 §3 asks for was not available;
//! - a title that half matched and a first author nobody recorded — same.
//!
//! The second and third are the dangerous ones, because they are the common
//! ones: plenty of works in a bibliographic database have no year and no author
//! string. Reading "no corroboration available" as "confirmed" would make
//! `ok` mean "we did not find a reason to object", and then a `mismatch` — the
//! one verdict that must never be downgraded — would only be reachable by a
//! positive disproof. So `ok` requires **positive** corroboration and
//! `unverified` is the honest answer for its absence.
//!
//! # `unverified` does not block, and that is deliberate
//!
//! The opposite rule is in `crate::download`: `mismatch` blocks filing and
//! `unverified` lets the fetch proceed, with the `unverified` row written either
//! way. The reasoning is asymmetric on purpose:
//!
//! - A `mismatch` is **positive evidence** that the bytes are another work. The
//!   criterion #253 exists for — never file under a mismatched identity — is
//!   about that case, and the cost of getting it wrong is a silently wrong
//!   library.
//! - An `unverified` is **no evidence at all**. Blocking on it would mean a work
//!   whose PDF carries no `/Title` could never be acquired by any machine run,
//!   ever: `unverified` is a property of the *data*, not of the fetch, so a
//!   blocked fetch would return the same `unverified` forever. Acquisition
//!   would need a person for every PDF, which is the outcome ADR-007 §3's whole
//!   route ladder exists to avoid.
//!
//! So the gate blocks on evidence and records its absence. A reader who wants
//! the stricter rule can find it: the row says `unverified`, both titles are
//! stored, and `scitadel action_list` can be asked for them.
//!
//! # What a year disagreement means, and what a first author means
//!
//! Not symmetric, on purpose:
//!
//! - A **first author** is a property of the *work*. Two different works
//!   carrying the same title are different works, so a first-author
//!   disagreement contradicts the title even at [`MATCH_THRESHOLD`], where the
//!   year does not.
//! - A **year** is a property of a *version*. ADR-007 §3 ranks OA VoR > AM >
//!   preprint, and every one of those has its own year: a 2022 preprint
//!   published in 2024 has the same title and a first author, and a two-year
//!   gap. So the year can corroborate and, in the tie band, contradict — but it
//!   never overturns a title that matches outright.
//!
//! [tests]: self::tests

use std::borrow::Cow;

/// Similarity at or above which the titles are treated as the same string, and
/// `ok` follows once something corroborates.
///
/// **Measured**, not chosen: a one-word difference (`for`/`in`, 0.974), a
/// character transposition (0.991), `Müller` against `Muller` (1.000) and a
/// landing page that kept its site suffix (0.921) all land above it, while every
/// "same work, longer or shorter title" pair in [`tests`] lands below.
pub const MATCH_THRESHOLD: f64 = 0.90;

/// Similarity below which the titles are treated as different works, whatever
/// the year and the first author say.
///
/// **Measured**: two unrelated works in the same field score 0.683 and 0.646,
/// an unrelated pair 0.538.
///
/// The known false-positive class sits just above this line: a registry whose
/// title for a work is substantially longer than the stored one (0.646 for the
/// same subject, a different article-shaped rewrite) is refused rather than
/// filed. That is a deliberate choice, because the two errors are not equal — a
/// blocked fetch is an afternoon, a mis-filed full text is a wrong library that
/// nothing notices — and the human path exists for it:
/// `scitadel override-identity`, with both titles already in
/// `paper_identity_checks` for the reader to compare.
pub const MISMATCH_FLOOR: f64 = 0.70;

/// The most a year may differ and still corroborate (ADR-007 §3: "year ±1").
pub const YEAR_TOLERANCE: i32 = 1;

/// How similar two first authors' surnames must be.
///
/// High, because the surname is the one part of a name that is spelled the same
/// everywhere. It is compared rather than string-equal so that `"Müller"` and
/// `"Muller"`, or a diacritic that [`normalise`] stripped from one side but not
/// the other, still agree; a genuinely different surname (0.6 or lower) is two
/// works.
const AUTHOR_THRESHOLD: f64 = 0.90;

/// What one identity check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The same work, corroborated.
    Ok,
    /// **Not** the same work. Nothing may be filed under this DOI until a person
    /// says otherwise.
    Mismatch,
    /// We could not check. Not a pass, and not a block: see the module docs.
    Unverified,
}

impl Verdict {
    /// The `paper_identity_checks.status` value this verdict writes.
    ///
    /// `Overridden` is unreachable from here on purpose: a machine may not
    /// record it, and `scitadel_db::sqlite::override_identity` is the only
    /// writer that may.
    #[must_use]
    pub fn status_label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Mismatch => "mismatch",
            Self::Unverified => "unverified",
        }
    }

    /// May these bytes be filed under this work?
    ///
    /// One place, so no caller can read "unverified" as a pass by accident:
    /// only [`Self::Mismatch`] stops a fetch.
    #[must_use]
    pub fn allows_filing(self) -> bool {
        !matches!(self, Self::Mismatch)
    }
}

/// One side of a title comparison, with the two facts that corroborate it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkIdentity {
    pub title: Option<String>,
    pub year: Option<i32>,
    /// The first author, however the source spells them — `"Young, Christopher
    /// J."`, `"Christopher J. Young"`, or OpenAlex's
    /// `"Young, Christopher J. [Sandia National Lab. …]"`. Only the surname is
    /// compared; see [`surname`].
    pub first_author: Option<String>,
}

impl WorkIdentity {
    /// A side with just a title — the common case, and the one that cannot
    /// corroborate anything.
    #[must_use]
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..Self::default()
        }
    }

    /// A side with a title, a year and a first author.
    #[must_use]
    pub fn full(title: impl Into<String>, year: Option<i32>, first_author: Option<String>) -> Self {
        Self {
            title: Some(title.into()),
            year,
            first_author,
        }
    }
}

/// A verdict with the evidence that produced it — what the recorded row carries
/// and what a human reads when the verdict is `mismatch`.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub verdict: Verdict,
    /// The title similarity, when two titles were actually compared. `None` for
    /// a check that never got that far, which is why `unverified` sometimes has
    /// no score to show.
    pub score: Option<f64>,
    /// One sentence, in the words `scitadel action_list` and the CLI print.
    pub why: String,
}

impl Checked {
    fn new(verdict: Verdict, score: Option<f64>, why: impl Into<String>) -> Self {
        Self {
            verdict,
            score,
            why: why.into(),
        }
    }
}

/// Compare two identities and return the verdict.
///
/// The whole decision table, in the order the module docs give it.
#[must_use]
pub fn verify(expected: &WorkIdentity, resolved: &WorkIdentity) -> Checked {
    let (Some(expected_title), Some(resolved_title)) = (
        expected.title.as_deref().and_then(non_empty),
        resolved.title.as_deref().and_then(non_empty),
    ) else {
        return Checked::new(
            Verdict::Unverified,
            None,
            "no title on one side, so there was nothing to compare — \
             `unverified` is not a pass",
        );
    };
    if expected_title.is_empty() || resolved_title.is_empty() {
        return Checked::new(
            Verdict::Unverified,
            None,
            "a title was blank, so there was nothing to compare — \
             `unverified` is not a pass",
        );
    }

    let score = title_similarity(&expected_title, &resolved_title);
    let year = YearAgreement::of(expected.year, resolved.year);
    let author = AuthorAgreement::of(
        expected.first_author.as_deref(),
        resolved.first_author.as_deref(),
    );
    let corroborated = year.agrees() || author.agrees();
    let contradicted = year.contradicts() || author.contradicts();

    if score >= MATCH_THRESHOLD {
        // A different first author contradicts even a title that matches
        // outright — two works really do share titles. A different *year* does
        // not: one work has several.
        return if author.contradicts() {
            Checked::new(
                Verdict::Mismatch,
                Some(score),
                format!(
                    "the titles match ({score:.2}) but the first authors do not \
                     ({} vs {}), so these are two different works",
                    quote(expected.first_author.as_deref()),
                    quote(resolved.first_author.as_deref())
                ),
            )
        } else if corroborated {
            Checked::new(
                Verdict::Ok,
                Some(score),
                format!(
                    "the titles match ({score:.2}) and {}",
                    corroboration(year, &author)
                ),
            )
        } else {
            Checked::new(
                Verdict::Unverified,
                Some(score),
                format!(
                    "the titles match ({score:.2}) but there is no year or first author \
                     on record to corroborate them — `unverified` is not a pass"
                ),
            )
        };
    }

    if score < MISMATCH_FLOOR {
        return Checked::new(
            Verdict::Mismatch,
            Some(score),
            format!(
                "the titles do not match ({score:.2}): {expected_title:?} vs \
                 {resolved_title:?} — and a year or first author agreeing does not make \
                 two different titles the same work"
            ),
        );
    }

    // The tie band: the titles are too close to call on their own, which is
    // exactly the preprint-versus-version-of-record case. ADR-007 §3's tie
    // breakers decide, and *absent* one is not agreement.
    if contradicted {
        return Checked::new(
            Verdict::Mismatch,
            Some(score),
            format!(
                "the titles are too close to call ({score:.2}) and {} disagrees, so this \
                 is a different work",
                contradiction(year, &author)
            ),
        );
    }
    if corroborated {
        return Checked::new(
            Verdict::Ok,
            Some(score),
            format!(
                "the titles are too close to call ({score:.2}) but {} settles it",
                corroboration(year, &author)
            ),
        );
    }
    Checked::new(
        Verdict::Unverified,
        Some(score),
        format!(
            "the titles are too close to call ({score:.2}) and there is no year or first \
             author on record to break the tie — `unverified` is not a pass"
        ),
    )
}

/// The phrase for a check that passed because of a corroborator.
fn corroboration(year: YearAgreement, author: &AuthorAgreement) -> String {
    match (year.agrees(), author.agrees()) {
        (true, true) => "both the year and the first author agree".to_string(),
        (true, false) => format!("the year agrees ({})", year.describe()),
        (false, true) => format!("the first author agrees ({})", author.describe()),
        (false, false) => "something agreed".to_string(),
    }
}

/// The phrase for the fact that settled it the other way.
fn contradiction(year: YearAgreement, author: &AuthorAgreement) -> String {
    match (year.contradicts(), author.contradicts()) {
        (true, true) => "both the year and the first author disagree".to_string(),
        (true, false) => format!("the year disagrees ({})", year.describe()),
        (false, true) => format!("the first author disagrees ({})", author.describe()),
        (false, false) => "something disagrees".to_string(),
    }
}

fn quote(value: Option<&str>) -> String {
    value.map_or_else(|| "none on record".to_string(), |v| format!("{v:?}"))
}

/// How two years relate — ADR-007 §3's "year ±1".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum YearAgreement {
    /// Both recorded, within [`YEAR_TOLERANCE`].
    Agrees,
    /// Both recorded, more than [`YEAR_TOLERANCE`] apart.
    Contradicts,
    /// At least one side has no year. **Never** agreement and never
    /// contradiction: see the module docs.
    Unknown,
}

impl YearAgreement {
    fn of(expected: Option<i32>, resolved: Option<i32>) -> Self {
        match (expected, resolved) {
            (Some(a), Some(b)) if (a - b).abs() <= YEAR_TOLERANCE => Self::Agrees,
            (Some(_), Some(_)) => Self::Contradicts,
            _ => Self::Unknown,
        }
    }

    fn agrees(self) -> bool {
        matches!(self, Self::Agrees)
    }

    fn contradicts(self) -> bool {
        matches!(self, Self::Contradicts)
    }

    fn describe(self) -> String {
        match self {
            Self::Agrees => format!("within {YEAR_TOLERANCE} year"),
            Self::Contradicts => "more than one year apart".to_string(),
            Self::Unknown => "no year on one side".to_string(),
        }
    }
}

/// How two first authors relate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthorAgreement {
    /// Both recorded, same surname.
    Agrees(String),
    /// Both recorded, different surnames.
    Contradicts(String),
    /// At least one side has no author.
    Unknown,
}

impl AuthorAgreement {
    fn of(expected: Option<&str>, resolved: Option<&str>) -> Self {
        match (expected.and_then(surname), resolved.and_then(surname)) {
            (Some(expected), Some(resolved)) => {
                let similarity = strsim::normalized_levenshtein(&expected, &resolved);
                if similarity >= AUTHOR_THRESHOLD {
                    Self::Agrees(resolved)
                } else {
                    Self::Contradicts(resolved)
                }
            }
            _ => Self::Unknown,
        }
    }

    fn agrees(&self) -> bool {
        matches!(self, Self::Agrees(_))
    }

    fn contradicts(&self) -> bool {
        matches!(self, Self::Contradicts(_))
    }

    fn describe(&self) -> String {
        match self {
            Self::Agrees(surname) => format!("{surname:?}"),
            Self::Contradicts(surname) => format!("expected {surname:?} instead"),
            Self::Unknown => "no author on one side".to_string(),
        }
    }
}

/// The surname of a first-author string, normalised.
///
/// Three shapes have to land on the same value, because three registries spell
/// names differently and ADR-007 §3's tie-breaker is the surname:
///
/// - `"Young, Christopher J."` → `young` — after a comma is the surname, the
///   standard bibliographic order;
/// - `"Christopher J. Young"` → `young` — otherwise it is the last whitespace
///   token, which is right for every "Given … Family" shape;
/// - `"Young, Christopher J. [Sandia National Lab. (SNL-NM) …]"` → `young` —
///   OpenAlex appends the affiliation in brackets, so the affiliation is
///   dropped first.
///
/// `None` for a blank or unparseable name, which [`AuthorAgreement`] treats as
/// "no author on record" rather than as agreement with anything.
#[must_use]
pub fn surname(author: &str) -> Option<String> {
    let without_affiliation = author.split('[').next().unwrap_or(author);
    let name = without_affiliation
        .split(';')
        .next()
        .unwrap_or(without_affiliation);
    let token = match name.split_once(',') {
        Some((family, _rest)) => family,
        None => name.split_whitespace().next_back()?,
    };
    let normalised = normalise(token);
    if normalised.is_empty() {
        None
    } else {
        Some(normalised)
    }
}

/// The title this module would compare, from `title`.
///
/// `None` for a title nobody recorded. Casefolded, stripped of diacritics, with
/// every non-alphanumeric character turned into a space and whitespace
/// collapsed — so `Deep-Learning  for Radiopharmaceutical…` and
/// `deep learning for radiopharmaceutical…` are one string.
///
/// HTML entity references are dropped rather than decoded: `&alpha;` is markup,
/// not title content, and dropping it on both sides is symmetric, whereas
/// decoding a subset of entities and not others would be neither. A served page
/// whose title *is* entity-encoded is decoded at the point of extraction
/// instead, by [`decode_entities`].
///
/// `None` for a title that normalises to nothing — a blank string, or one made
/// entirely of punctuation and separators. That is not a title, and scoring it
/// against anything would produce a number that reads like a measurement.
#[must_use]
pub fn normalised(title: &str) -> Option<String> {
    non_empty(title)
}

fn non_empty(title: &str) -> Option<String> {
    let normalised = normalise(title);
    (!normalised.is_empty()).then_some(normalised)
}

/// Similarity of two titles, in `0.0..=1.0`.
///
/// **Optimal string alignment distance over the sum of both lengths**, with a
/// transposition counted as one edit:
///
/// ```text
/// 1 - d(a, b) / (len(a) + len(b))
/// ```
///
/// That is the shape of `difflib.SequenceMatcher.ratio()` for a substitution
/// model, chosen over three alternatives for reasons that are all about *length*
/// rather than about spelling:
///
/// - over `strsim::normalized_levenshtein`, which divides by the **longer**
///   string. A served title that is a short form of the stored one —
///   "Deep learning for radiopharmaceutical image reconstruction" against
///   "Deep learning for radiopharmaceutical image reconstruction in nuclear
///   medicine" — drops to 0.744 there and to 0.882 here, and the first number
///   is in a different band from the second purely because one side of the
///   comparison is longer.
/// - over a token-set measure, which ignores word order and would score
///   "Study of X in rats" against "Study of rats in X" as 1.0 — two different
///   works by title alone.
/// - over a substring measure, which would call a served page that kept its
///   site name an exact match and so lose every bit of the discrimination the
///   [`MISMATCH_FLOOR`] band is built on.
///
/// A transposition is one edit rather than two because a served title very
/// often differs from a stored one by exactly that (`pdding`/`padding`), and
/// `strsim` already computes the distance — nothing new is implemented here.
///
/// Empty on either side is `0.0`, not a claim: there was nothing to compare, and
/// [`verify`] reports that as [`Verdict::Unverified`] before getting here.
#[must_use]
pub fn title_similarity(expected: &str, resolved: &str) -> f64 {
    if expected.is_empty() || resolved.is_empty() {
        return 0.0;
    }
    let total = (expected.chars().count() + resolved.chars().count()) as f64;
    1.0 - (strsim::damerau_levenshtein(expected, resolved) as f64 / total)
}

/// Do these bytes begin with a PDF's magic number?
///
/// The check that fails closed. A place that answers `200 text/html` with an
/// error page is a real shape — OSTI's own `purl` endpoint answers a 404 for an
/// unknown id with a 265 kB HTML site page — and filing that as
/// `fulltext_pdf` would make coverage claim a full text nobody can read.
///
/// Content type is **not** enough on its own, and is not consulted here: a CDN
/// that mislabels a PDF as `application/octet-stream` would be refused by a
/// content-type-only check, and a proxy that rewrites an error page's type would
/// slip past one. The bytes do not lie about what they are.
#[must_use]
pub fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

/// The title a served HTML page claims for itself, or `None`.
///
/// Read in the order a publisher page's own metadata gives it, and every one of
/// these is a standard tag rather than a heuristic:
///
/// 1. `<meta name="citation_title">` — Highwire Press, which Elsevier, Springer,
///    Wiley, IEEE and ACM all emit, and which carries the article title and
///    nothing else;
/// 2. `<meta property="og:title">` — Open Graph, for a site that has no
///    Highwire tags;
/// 3. `<title>` — the last resort, and the one that needs cleaning: a site's
///    own name is appended after a `|`, an em dash or a middot in almost every
///    template, so the part before the **last** such separator is taken. That is
///    the article title in "Title | Journal Name" and in "Title – Journal",
///    which between them cover the shapes seen in the wild.
///
/// `None` when the page names no title: a paywall stub usually does not, and a
/// page with no title gives us [`Verdict::Unverified`], which is the honest
/// reading.
#[must_use]
pub fn html_title(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if let Some(title) = meta_content(text, "citation_title") {
        return Some(title);
    }
    // Open Graph spells the attribute `property`, not `name` — that is the
    // whole difference between the two tags and the reason both are tried.
    if let Some(title) = meta_content(text, "og:title") {
        return Some(title);
    }
    let open = find_ci(text, "<title")?;
    let body = &text[open + "<title".len()..];
    let body = body.trim_start();
    // Skip the rare `</title attr>`-shaped attributes before the text.
    let body = match body.strip_prefix('>') {
        Some(text) => text,
        None => match body.find('>') {
            Some(at) => &body[at + 1..],
            None => body,
        },
    };
    let end = find_ci(body, "</title")?;
    Some(strip_site_suffix(&body[..end]))
}

/// The title a served PDF claims for itself, or `None`.
///
/// The `/Title` entry of the document's information dictionary, which is where
/// `pdfTeX`, Word and most converters put the article's title.
///
/// **Best-effort by design, and that is the point.** A PDF that keeps its
/// metadata in a compressed object stream needs an inflater to read, and adding
/// one to look for a title nobody requires would be a dependency bought for a
/// check whose honest answer on failure is [`Verdict::Unverified`]. So the two
/// encodings that need no decompressor are handled — a literal `(…)` string and
/// a `<…>` hex string, the latter decoded as UTF-16BE when it carries a BOM —
/// and everything else reads as "no title", which costs corroboration but never
/// blocks a fetch. The title that *is* found still does its job: a PDF of a
/// different work carries a different `/Title`, which is the
/// [`MISMATCH_FLOOR`] case.
#[must_use]
pub fn pdf_title(bytes: &[u8]) -> Option<String> {
    const KEY: &[u8] = b"/Title";
    let start = find_bytes(bytes, KEY)?;
    // PDF allows whitespace between a key and its value — `/Title (…)` is as
    // common as `/Title(…)` — so it is skipped rather than read as a value type
    // this module does not handle.
    let rest = bytes[start + KEY.len()..]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&bytes[bytes.len()..], |at| &bytes[start + KEY.len() + at..]);
    match rest.first()? {
        b'(' => {
            // `literal_string_end` counts the closing paren, so the string body
            // is everything between the two.
            let end = literal_string_end(rest)?;
            clean_pdf_text(&decode_pdf_literal(&rest[1..end - 1]))
        }
        b'<' => {
            let end = rest.iter().position(|b| *b == b'>')?;
            clean_pdf_text(decode_pdf_hex(&rest[1..end]).as_bytes())
        }
        // `/Title` immediately followed by anything else is a key inside a
        // dictionary whose value is an indirect reference (or a name we do not
        // handle). No title, honestly.
        _ => None,
    }
}

/// What the served bytes say this document is: title, and where the format
/// carries them, a year and a first author too.
///
/// The three facts are read together because that is what makes the post-fetch
/// check worth running. A publisher landing page emits
/// `citation_publication_date` and `citation_author` as readily as
/// `citation_title`, so a served page arrives with all three and
/// [`verify`] can corroborate rather than merely compare — while a PDF carries
/// only a `/Title`, so a PDF's check compares titles and reports
/// [`Verdict::Unverified`] when they agree. That difference is the honest shape
/// of the evidence: the bytes of a PDF say much less about themselves than the
/// bytes of a page do.
///
/// `None` when the bytes name no title at all — a paywall stub usually does not.
/// The caller then runs no check rather than recording a comparison against
/// nothing.
#[must_use]
pub fn served_identity(bytes: &[u8]) -> Option<WorkIdentity> {
    if is_pdf(bytes) {
        return pdf_title(bytes).map(WorkIdentity::titled);
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let title = html_title(text.as_bytes())?;
    Some(WorkIdentity {
        title: Some(title),
        year: served_year(text),
        first_author: meta_content(text, "citation_author"),
    })
}

/// The year a served page states, from the two Highwire tags that carry one.
///
/// `citation_publication_date` first — it is the tag every publisher emits and
/// the one that names the article rather than the issue — then `dc.date` for the
/// pages that only carry Dublin Core.
///
/// The first four digits of whatever the tag holds, which covers
/// `2024/05/01`, `2024-05-01` and `2024-05-01T00:00:00Z` alike. A value with no
/// four-digit prefix is `None`: a year we cannot read is not a year.
fn served_year(html: &str) -> Option<i32> {
    let raw = meta_content(html, "citation_publication_date")
        .or_else(|| meta_content(html, "dc.date"))?;
    let digits: String = raw.chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

/// The `content` of `<meta name="…" content="…">`, matched on the identifying
/// attribute.
///
/// Case-insensitive on both the attribute and its value, because the tags are
/// written `name="citation_title"` by some publishers and `NAME="Citation_Title"`
/// by others. The value stops at the closing quote and is entity-decoded, since
/// `&amp;` inside an attribute is markup rather than title content.
///
/// Scanned against the original bytes rather than a lowercased copy: lowercasing
/// can change a string's byte length (`İ` becomes two characters), so a range
/// found in a lowercased copy does not index the original.
fn meta_content(html: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(start) = find_ci(&html[from..], "<meta").map(|offset| from + offset) {
        let end = html[start..]
            .find('>')
            .map_or(html.len(), |offset| start + offset);
        let tag = &html[start..end];
        // The identifying attribute is `name` for the Highwire tags and
        // `property` for Open Graph, so both spellings are accepted for either.
        let named = ["name", "property"].into_iter().any(|attribute| {
            attribute_value(tag, attribute).is_some_and(|value| value.eq_ignore_ascii_case(name))
        });
        if named && let Some(value) = attribute_value(tag, "content") {
            return Some(decode_entities(value).trim().to_string());
        }
        from = end + 1;
        if from >= html.len() {
            break;
        }
    }
    None
}

/// One attribute's quoted value from a tag, case-insensitive on the name.
///
/// The name has to be a whole word: `content` must not match inside a longer
/// attribute such as `contentshort`. Both quote styles are accepted, because
/// both appear, and an unquoted value runs to the next whitespace.
fn attribute_value<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(at) = find_ci(&tag[from..], name) {
        let start = from + at;
        let after = start + name.len();
        let is_whole_word = tag[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '-');
        let rest = tag[after..].trim_start_matches([' ', '\t', '\n', '\r']);
        if is_whole_word && let Some(equals) = rest.strip_prefix('=') {
            let value = equals.trim_start_matches([' ', '\t', '\n', '\r']);
            let quote = value.chars().next()?;
            if quote == '"' || quote == '\'' {
                let inner = &value[1..];
                return inner.find(quote).map(|end| &inner[..end]);
            }
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            if end > 0 {
                return Some(&value[..end]);
            }
        }
        from = after;
        if from >= tag.len() {
            break;
        }
    }
    None
}

/// The first occurrence of `needle` in `haystack`, comparing ASCII
/// case-insensitively.
///
/// A hand-rolled scan rather than a regex crate: the three things searched for
/// here (`<meta`, `<title`, `</title`) have fixed shapes, and an HTML page is
/// the one input in this crate most likely to be megabytes.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let ned = needle.as_bytes();
    if ned.is_empty() || ned.len() > hay.len() {
        return None;
    }
    hay.windows(ned.len())
        .position(|window| window.eq_ignore_ascii_case(ned))
}

/// The first occurrence of `needle` in these bytes.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Drop the site or journal name a `<title>` template appends.
///
/// The part before the **last** separator, for the four that shapes it takes in
/// practice (`|`, an en dash, an em dash, a middot). The **last**, not the
/// first: "Nature | Radiology: Imaging" keeps everything up to the last one,
/// where a first-separator split would leave the journal name on.
///
/// A colon is deliberately *not* in the list — "First: A Subtitle That Is Part
/// Of It" is one title, and a template that appends ": Journal Name" is far rarer
/// than titles that use a colon themselves.
fn strip_site_suffix(title: &str) -> String {
    let trimmed = title.trim();
    let cut = trimmed
        .rfind(['|', '\u{2013}', '\u{2014}', '\u{b7}'])
        .unwrap_or(trimmed.len());
    trimmed[..cut].trim().to_string()
}

/// Where a PDF literal string `(…)` ends, counting the escapes.
fn literal_string_end(rest: &[u8]) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            b'\\' => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// Undo a PDF literal string's escaping.
///
/// The escapes a `/Title` realistically carries: `\n`, `\r`, `\t` and the octal
/// form `\ddd` that Word writes for any non-ASCII character. A backslash before
/// anything else is dropped, which is what the PDF spec says to do.
fn decode_pdf_literal(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] != b'\\' {
            out.push(raw[i]);
            i += 1;
            continue;
        }
        match raw.get(i + 1) {
            Some(b'n') => {
                out.push(b'\n');
                i += 2;
            }
            Some(b'r') => {
                out.push(b'\r');
                i += 2;
            }
            Some(b't') => {
                out.push(b'\t');
                i += 2;
            }
            Some(b'0'..=b'7') => {
                let digits = raw[i + 1..]
                    .iter()
                    .take(3)
                    .take_while(|byte| (b'0'..=b'7').contains(byte))
                    .count();
                let text = std::str::from_utf8(&raw[i + 1..i + 1 + digits]).unwrap_or("40");
                out.push(u8::from_str_radix(text, 8).unwrap_or(b'?'));
                i += 1 + digits;
            }
            Some(_) => i += 2,
            None => break,
        }
    }
    out
}

/// Decode a PDF hex string `<FEFF0044…>`.
///
/// A `FEFF` BOM means UTF-16BE, which is what Word writes for a non-ASCII title;
/// anything else is treated as PDFDocEncoding, which is ASCII for every character
/// that matters here.
fn decode_pdf_hex(raw: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(raw.len());
    let mut high: Option<u8> = None;
    for byte in raw {
        let nibble: u8 = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => continue,
        };
        match high {
            None => high = Some(nibble),
            Some(first) => {
                bytes.push(first << 4 | nibble);
                high = None;
            }
        }
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|byte| *byte as char).collect()
    }
}

/// The title text out of raw bytes, or `None` when there is none.
///
/// PDF strings are byte strings, so a Latin-1 title comes back as invalid UTF-8
/// and is read as Latin-1 rather than dropped — `pdfTeX` writes the document title
/// as raw bytes more often than as UTF-16.
fn clean_pdf_text(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw).map_or_else(
        |_| raw.iter().map(|byte| *byte as char).collect::<String>(),
        str::to_string,
    );
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Decode `&amp;`/`&lt;`/`&#8230;`-style entities a served page leaves in a
/// title, so a title that reads `M&#252;ller` matches the stored `Müller`.
///
/// Only the entities a title realistically carries: the five XML specials, the
/// numeric forms, and the handful of named characters that appear in journal
/// titles (accents, dashes, primes, an ellipsis). Anything else is left alone,
/// because a wrong expansion is worse than a leftover entity — and
/// [`normalised`] drops `&…;` anyway, so the two halves agree on what matters.
#[must_use]
pub fn decode_entities(input: &str) -> Cow<'_, str> {
    if !input.contains('&') {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        if let Some(expanded) = expand_entity(entity) {
            out.push_str(&expanded);
            rest = &rest[semi + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The named entities worth expanding, longest-first so `&notit;` cannot match
/// `&not`.
const NAMED_ENTITIES: [(&str, &str); 12] = [
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("ndash", "\u{2013}"),
    ("mdash", "\u{2014}"),
    ("hellip", "\u{2026}"),
    ("middot", "\u{b7}"),
    ("times", "\u{d7}"),
    ("prime", "\u{2032}"),
    ("rsquo", "\u{2019}"),
];

fn expand_entity(entity: &str) -> Option<String> {
    if let Some(digits) = entity.strip_prefix('#') {
        let code = match digits.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => digits.parse::<u32>().ok()?,
        };
        return char::from_u32(code).map(String::from);
    }
    NAMED_ENTITIES
        .iter()
        .find(|(name, _)| *name == entity)
        .map(|(_, value)| (*value).to_string())
}

/// Casefold, strip diacritics, turn every non-alphanumeric run into one space.
///
/// The three steps, and why each:
/// - **casefold** — `titlecase` and `to_lowercase` differ for a handful of
///   characters; `to_lowercase` is what every registry has already applied.
/// - **strip combining marks** — `ü` and `u`, `é` and `e`, `Å` and `A` are the
///   same letter, and a bibliography that stores one spelling has to match a
///   page that serves the other. This is the one step that needs real Unicode
///   data, which is why `unicode-normalization` is a dependency: a hand-rolled
///   table of "the diacritics I thought of" would be a second, incomplete
///   answer to the question "what is a letter", and would quietly disagree
///   with the registry it is trying to match.
/// - **non-alphanumeric → one space** — so `state-of-the-art`,
///   `state of the art` and `State  of  the  Art` are one string, and
///   `COVID-19` and `covid 19` are one string.
fn normalise(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_space = false;
    let mut decomposed = String::with_capacity(4);
    for ch in input.chars().flat_map(char::to_lowercase) {
        // Canonical decomposition of one character, which is at most a handful
        // of bytes; a `String` per character would allocate on every title, so
        // the buffer is reused.
        decomposed.clear();
        unicode_normalization::char::decompose_canonical(ch, |part| decomposed.push(part));
        for part in decomposed.chars() {
            push_letter(&mut out, &mut pending_space, part);
        }
    }
    out.trim().to_string()
}

/// Append one decomposed character: drop the combining marks, and collapse every
/// run of everything else into a single space.
fn push_letter(out: &mut String, pending_space: &mut bool, ch: char) {
    // A combining mark carries no identity of its own — the base letter it sits
    // on does, and it is already in `out`.
    if unicode_normalization::char::is_combining_mark(ch) {
        return;
    }
    if ch.is_alphanumeric() {
        if *pending_space {
            out.push(' ');
            *pending_space = false;
        }
        out.push(ch);
    } else {
        *pending_space = true;
    }
}

impl Checked {
    /// The row this verdict writes, for `paper_id` at `phase`/`source`.
    #[must_use]
    pub fn to_write(
        &self,
        paper_id: impl Into<String>,
        phase: scitadel_db::sqlite::IdentityPhase,
        source: scitadel_db::sqlite::IdentitySource,
        expected_title: Option<String>,
        resolved_title: Option<String>,
        checked_at: String,
    ) -> scitadel_db::sqlite::IdentityCheckWrite {
        scitadel_db::sqlite::IdentityCheckWrite {
            paper_id: paper_id.into(),
            checked_at,
            phase,
            source,
            expected_title,
            resolved_title,
            score: self.score,
            // A total mapping with no wildcard arm on purpose: `Verdict` has
            // three variants and this has three answers, so adding a fourth to
            // either is a compile error rather than a silently wrong `ok`.
            // `Overridden` is deliberately absent from `Verdict`: a machine may
            // not record it, and `override_identity` is the only writer that
            // may.
            status: match self.verdict {
                Verdict::Ok => scitadel_db::sqlite::IdentityStatus::Ok,
                Verdict::Mismatch => scitadel_db::sqlite::IdentityStatus::Mismatch,
                Verdict::Unverified => scitadel_db::sqlite::IdentityStatus::Unverified,
            },
            doi_corrected_from: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pair in the module's table, as a measurement the thresholds are
    /// pinned to. Each carries the band it must land in, so a change to
    /// [`MATCH_THRESHOLD`] or [`MISMATCH_FLOOR`] that quietly reclassifies a real
    /// title pair fails here rather than in a corpus.
    const MEASURED: [(&str, &str, &str, Band); 12] = [
        (
            "identical",
            "Deep learning for radiopharmaceutical image reconstruction",
            "Deep learning for radiopharmaceutical image reconstruction",
            Band::Match,
        ),
        (
            "case and punctuation",
            "Deep Learning for Radiopharmaceutical Image Reconstruction.",
            "deep learning for radiopharmaceutical image reconstruction",
            Band::Match,
        ),
        (
            "diacritics",
            "Müller-Lüdenscheidt energy",
            "Muller Ludenscheidt energy",
            Band::Match,
        ),
        (
            "one word changed",
            "Deep learning for radiopharmaceutical image reconstruction",
            "Deep learning in radiopharmaceutical image reconstruction",
            Band::Match,
        ),
        (
            "transposed characters",
            "Deep learning for radiopharmaceutical image reconstruction",
            "Deep learning for radiopharmceutical image reconstruction",
            Band::Match,
        ),
        (
            "served page kept its site suffix",
            "Deep learning for radiopharmaceutical image reconstruction",
            "Deep learning for radiopharmaceutical image reconstruction | Radiology",
            Band::Match,
        ),
        (
            "preprint gained a subtitle",
            "Deep learning for radiopharmaceutical image reconstruction",
            "Deep learning for radiopharmaceutical image reconstruction: a multicenter study",
            Band::Tie,
        ),
        (
            "served title is a short form",
            "Deep learning for radiopharmaceutical image reconstruction in nuclear medicine",
            "Deep learning for radiopharmaceutical image reconstruction",
            Band::Tie,
        ),
        (
            "a DIFFERENT work with the same words",
            "Deep learning for radiopharmaceutical image reconstruction",
            "Deep learning for radiopharmaceutical image reconstruction of the thyroid",
            Band::Tie,
        ),
        (
            "a different work in the same field",
            "Deep learning for radiopharmaceutical image reconstruction",
            "Deep learning for thyroid nodule classification on ultrasound images",
            Band::Mismatch,
        ),
        (
            "an unrelated work",
            "68Ga-PSMA-11 PET/CT in biochemical recurrence of prostate cancer",
            "Deep learning for radiopharmaceutical image reconstruction",
            Band::Mismatch,
        ),
        (
            "word order swapped",
            "Total-body PET scanners for theranostics",
            "Theranostic applications of total-body PET scanners",
            Band::Mismatch,
        ),
    ];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Band {
        Match,
        Tie,
        Mismatch,
    }

    /// The bands are a claim about measured scores, so they are measured here.
    /// The table is the calibration set: a threshold change that moves a real
    /// title pair into the wrong band fails this, which is the only way the
    /// numbers in the docs stay true.
    #[test]
    fn the_thresholds_sit_where_the_documented_measurements_say_they_do() {
        for (name, expected, resolved, band) in MEASURED {
            let score = title_similarity(
                &non_empty(expected).expect("expected title is non-empty"),
                &non_empty(resolved).expect("resolved title is non-empty"),
            );
            let landed = if score >= MATCH_THRESHOLD {
                Band::Match
            } else if score < MISMATCH_FLOOR {
                Band::Mismatch
            } else {
                Band::Tie
            };
            assert_eq!(landed, band, "{name}: {score:.3} landed in {landed:?}");
        }
    }

    /// The reason the tie band exists, as an executable claim — and it is
    /// stronger than "the two scores are close". The **right** answer scores
    /// *lower* than the **wrong** one: a served title that is a short form of
    /// the stored one (0.853) sits below a different work that appends a clause
    /// to the same title (0.885). No threshold can be placed between them
    /// without also putting the same-work pairs at 0.853 and 0.784 into the
    /// mismatch band and refusing correct fetches.
    ///
    /// So the band is not a gap between two good answers; it is the region where
    /// the title alone cannot answer, and ADR-007 §3's year and first author are
    /// the only things that can.
    #[test]
    fn no_title_threshold_can_separate_these_two() {
        let same_work = title_similarity(
            &non_empty(
                "Deep learning for radiopharmaceutical image reconstruction in nuclear medicine",
            )
            .unwrap(),
            &non_empty("Deep learning for radiopharmaceutical image reconstruction").unwrap(),
        );
        let other_work = title_similarity(
            &non_empty("Deep learning for radiopharmaceutical image reconstruction").unwrap(),
            &non_empty("Deep learning for radiopharmaceutical image reconstruction of the thyroid")
                .unwrap(),
        );
        assert!(
            same_work < other_work,
            "the same work scores {same_work:.3} and the different work {other_work:.3}:              the wrong answer scores higher, so no threshold can order them correctly"
        );
        for score in [same_work, other_work] {
            assert!(
                (MISMATCH_FLOOR..MATCH_THRESHOLD).contains(&score),
                "{score:.3} belongs in the tie band, where the year and the author decide"
            );
        }
    }

    /// #253's criterion, in the matcher: only a `mismatch` stops a fetch, and
    /// an `unverified` never reads as a pass to a caller that asks.
    #[test]
    fn only_a_mismatch_refuses_to_file() {
        assert!(Verdict::Ok.allows_filing());
        assert!(Verdict::Unverified.allows_filing());
        assert!(!Verdict::Mismatch.allows_filing());
        assert_eq!(Verdict::Ok.status_label(), "ok");
        assert_eq!(Verdict::Unverified.status_label(), "unverified");
        assert_eq!(Verdict::Mismatch.status_label(), "mismatch");
    }

    // =====================================================================
    // The year and the first author, which is where ADR-007 §3's
    // "year ±1 and the first author as tie-breakers" becomes executable.
    // =====================================================================

    /// The expected side of every verdict test below: a long title against a
    /// preprint of the same work, which is the shape that lands in the tie band
    /// (0.853) and therefore the shape the tie-breakers exist for.
    const BASE: &str = "Deep learning for radiopharmaceutical image reconstruction";
    const SUBTITLE: &str =
        "Deep learning for radiopharmaceutical image reconstruction: a multicenter study";
    /// The *wrong* work for the tie: the same title plus a clause, 0.885.
    const OTHER_CLAUSE: &str =
        "Deep learning for radiopharmaceutical image reconstruction of the thyroid";

    /// The expected side of the year tests below, and the title a preprint of
    /// the same work carries: the shape that lands in the tie band (0.853) and is
    /// therefore the shape the tie-breakers exist for.
    const SUBTITLE_YEAR_OFF: [(&str, &str); 2] = [
        (
            "a_title_with_a_year_off_by_one_still_matches",
            "Deep learning for radiopharmaceutical image reconstruction: a multicenter study",
        ),
        (
            "a_title_with_a_year_off_by_two_does_not",
            "Deep learning for radiopharmaceutical image reconstruction: a multicenter study",
        ),
    ];

    /// ADR-007 §3's "year ±1", at the boundary.
    ///
    /// A tie-band pair, so the year is the only thing deciding: ±1 is the same
    /// work. Asserted as `ok` and not "not a mismatch", because a year
    /// tie-breaker that could only ever confirm would be half a rule.
    #[test]
    fn a_title_with_a_year_off_by_one_still_matches() {
        let (name, title) = SUBTITLE_YEAR_OFF[0];
        let tie = title_similarity(&non_empty(BASE).unwrap(), &non_empty(title).unwrap());
        assert!(
            (MISMATCH_FLOOR..MATCH_THRESHOLD).contains(&tie),
            "these two titles are in the tie band ({tie:.3}), or this test is not \
             testing the year: {name}"
        );

        let checked = verify(
            &WorkIdentity::full(BASE, Some(2020), Some("Young, Christopher J.".into())),
            &WorkIdentity::full(title, Some(2021), Some("Young, Christopher".into())),
        );
        assert_eq!(
            checked.verdict,
            Verdict::Ok,
            "a year within ±1 is the same work: {}",
            checked.why
        );
    }

    /// The far side of the same boundary: ±2 is a different work, and saying so
    /// is as load-bearing as confirming.
    ///
    /// A year that disagrees by two is not a rounding error in a citation — it is
    /// the signal ADR-007 §3's tie-breaker exists to give when two titles are too
    /// close to call on their own.
    #[test]
    fn a_title_with_a_year_off_by_two_does_not() {
        let (name, title) = SUBTITLE_YEAR_OFF[1];
        let checked = verify(
            &WorkIdentity::full(BASE, Some(2020), None),
            &WorkIdentity::full(title, Some(2022), None),
        );
        assert_eq!(
            checked.verdict,
            Verdict::Mismatch,
            "a year two out is a different work: {name} — {}",
            checked.why
        );
        assert!(!checked.verdict.allows_filing(), "and it blocks filing");
    }

    /// The *first author* breaks the tie, in the direction that matters: two
    /// candidates whose titles are both inside the threshold, one the right work
    /// and one not, and the author picks the right one in both directions.
    ///
    /// A tie-breaker that can only say "no" would be a second mismatch rule; one
    /// that can only say "yes" would be a rubber stamp. Both directions are here.
    #[test]
    fn the_first_author_breaks_a_tie() {
        let right = verify(
            &WorkIdentity::full(BASE, None, Some("Young, Christopher J.".into())),
            &WorkIdentity::full(
                "Deep learning for radiopharmaceutical image reconstruction of the thyroid",
                None,
                Some("Young, Christopher J.".into()),
            ),
        );
        assert_eq!(
            right.verdict,
            Verdict::Ok,
            "the right candidate's author matches: {}",
            right.why
        );

        let wrong = verify(
            &WorkIdentity::full(BASE, None, Some("Young, Christopher J.".into())),
            &WorkIdentity::full(OTHER_CLAUSE, None, Some("Harris, James M.".into())),
        );
        assert_eq!(
            wrong.verdict,
            Verdict::Mismatch,
            "the wrong candidate's author does not: {}",
            wrong.why
        );
    }

    /// The falsification case, and the one that decides the whole design: **a
    /// missing year or author is `unverified`, never `ok`.**
    ///
    /// The rule is precise, so it is worth stating precisely: `ok` needs **at
    /// least one** corroborator that is present and not contradicting. Every row
    /// below is a pair with *no* corroborator available, and each of them is a
    /// shape where a plausible implementation would reach `ok`:
    ///
    /// - titles matching outright, with no year and no author on record;
    /// - a tie-band title pair, where the tie-breaker ADR-007 §3 names is the
    ///   only thing that could decide and is not there;
    /// - a tie-band pair where a year exists on **one** side, which is not a year
    ///   of agreement.
    ///
    /// `allows_filing` is true for all of them: an `unverified` is not a block
    /// either, which is the deliberate asymmetry the module docs spell out.
    #[test]
    fn a_missing_year_or_author_is_unverified_not_ok() {
        let cases: [(&str, WorkIdentity, WorkIdentity); 4] = [
            (
                "titles match outright, no year and no author",
                WorkIdentity::titled(BASE),
                WorkIdentity::titled(BASE),
            ),
            (
                "a tie-band pair with nothing to break the tie",
                WorkIdentity::titled(BASE),
                WorkIdentity::titled(SUBTITLE),
            ),
            (
                "a tie-band pair with a year on one side only",
                WorkIdentity::full(BASE, Some(2020), None),
                WorkIdentity::titled(SUBTITLE),
            ),
            (
                "a tie-band pair with two unrelated authors and no year",
                WorkIdentity::full(BASE, None, Some("Young, C.".into())),
                WorkIdentity::titled(SUBTITLE),
            ),
        ];
        for (name, expected, resolved) in cases {
            let checked = verify(&expected, &resolved);
            assert_eq!(
                checked.verdict,
                Verdict::Unverified,
                "{name} must not read as a pass: {}",
                checked.why
            );
            assert!(
                checked.verdict.allows_filing(),
                "{name} must not block either — an unverifiable check is not evidence"
            );
        }
    }

    /// `ok` needs **one** corroborator, not both — and which one is present must
    /// not change the verdict. Pinning both halves keeps the rule from drifting
    /// into "a year is required" or "an author is required", which are different
    /// policies with different failure modes on real data.
    #[test]
    fn one_corroborator_is_enough_and_the_presence_of_the_other_is_irrelevant() {
        let year_only = verify(
            &WorkIdentity::full(BASE, Some(2020), None),
            &WorkIdentity::full(SUBTITLE, Some(2021), None),
        );
        assert_eq!(year_only.verdict, Verdict::Ok, "{}", year_only.why);

        let author_only = verify(
            &WorkIdentity::full(BASE, None, Some("Young, C.".into())),
            &WorkIdentity::full(SUBTITLE, None, Some("Young, C.".into())),
        );
        assert_eq!(author_only.verdict, Verdict::Ok, "{}", author_only.why);

        let both = verify(
            &WorkIdentity::full(BASE, Some(2020), Some("Young, C.".into())),
            &WorkIdentity::full(SUBTITLE, Some(2021), Some("Young, C.".into())),
        );
        assert_eq!(both.verdict, Verdict::Ok, "{}", both.why);
        assert_eq!(
            both.score, year_only.score,
            "the score is the title's alone"
        );
    }

    /// A title nobody could read is `unverified` with **no score**, because no
    /// comparison was made. Recording 0.0 would be a claim.
    #[test]
    fn a_title_nobody_could_read_is_unverified_with_no_score() {
        for (name, expected, resolved) in [
            (
                "no expected title",
                WorkIdentity::default(),
                WorkIdentity::titled(BASE),
            ),
            (
                "no served title",
                WorkIdentity::titled(BASE),
                WorkIdentity::default(),
            ),
            (
                "a punctuation-only title",
                WorkIdentity::titled("—"),
                WorkIdentity::titled(BASE),
            ),
        ] {
            let checked = verify(&expected, &resolved);
            assert_eq!(checked.verdict, Verdict::Unverified, "{name}");
            assert_eq!(checked.score, None, "{name}: no comparison was made");
        }
    }

    /// A year never overturns a title that matches outright, and a first author
    /// always can. The asymmetry is the ADR-007 §3 version model: one work has
    /// several years (preprint, online-first, issue) and exactly one first author.
    #[test]
    fn a_year_never_overturns_a_matching_title_but_an_author_does() {
        let years_apart = verify(
            &WorkIdentity::full(BASE, Some(2021), Some("Young, C.".into())),
            &WorkIdentity::full(BASE, Some(2024), Some("Young, C.".into())),
        );
        assert_eq!(
            years_apart.verdict,
            Verdict::Ok,
            "a 2021 preprint published in 2024 is one work: {}",
            years_apart.why
        );

        let other_author = verify(
            &WorkIdentity::full(BASE, Some(2021), Some("Young, C.".into())),
            &WorkIdentity::full(BASE, Some(2021), Some("Harris, J.".into())),
        );
        assert_eq!(
            other_author.verdict,
            Verdict::Mismatch,
            "identical titles by different first authors are two works: {}",
            other_author.why
        );
    }

    /// The name tie-breaker, over the three spellings a name arrives in.
    #[test]
    fn a_surname_is_the_same_under_three_spellings() {
        let expected = surname("Young, Christopher J.");
        assert_eq!(expected.as_deref(), Some("young"));
        assert_eq!(surname("Christopher J. Young").as_deref(), Some("young"));
        assert_eq!(
            surname("Young, Christopher J. [Sandia National Lab. (SNL-NM), Albuquerque, NM]")
                .as_deref(),
            Some("young"),
            "OpenAlex's affiliation suffix must not become part of the surname"
        );
        assert_eq!(
            surname("Young, Christopher J.; Harris, James M.").as_deref(),
            Some("young"),
            "a single field holding several names is compared on its first"
        );
        assert_eq!(surname("Müller").as_deref(), Some("muller"));
        assert_eq!(surname("  "), None, "a blank name is no name");
    }

    /// Entities a served page leaves in a title must not stop it matching the
    /// stored spelling.
    #[test]
    fn html_entities_are_decoded_and_matched() {
        assert_eq!(
            decode_entities("Tomographic &amp; imaging"),
            "Tomographic & imaging"
        );
        assert_eq!(decode_entities("M&#252;ller"), "Müller");
        assert_eq!(decode_entities("&#x3b1;-emitters"), "α-emitters");
        assert_eq!(decode_entities("no entities here"), "no entities here");
        // An unknown entity is left alone rather than guessed at.
        assert_eq!(
            decode_entities("a &notarealentity; b"),
            "a &notarealentity; b"
        );

        let served = r#"<html><head><meta name="citation_title" content="Tomographic &amp; imaging of &#945;-emitters"></head></html>"#;
        let title = html_title(served.as_bytes()).expect("a title");
        assert_eq!(
            normalise(&decode_entities(&title)),
            normalise("Tomographic imaging of α-emitters"),
            "a served page's entity-encoded title matches the stored one"
        );
    }

    /// The extractor reads the publisher's own metadata first and cleans the
    /// `<title>` fallback, because a site's name appended to a title would
    /// otherwise read as a different article.
    #[test]
    fn a_served_pages_title_is_read_in_priority_order() {
        assert_eq!(
            html_title(br#"<meta property="og:title" content="OG"><title>Doc</title>"#).as_deref(),
            Some("OG"),
            "og:title beats the document title"
        );
        assert_eq!(
            html_title(b"<title>Deep Learning | Radiology</title>").as_deref(),
            Some("Deep Learning"),
            "the site name after the last pipe is dropped"
        );
        assert_eq!(
            html_title("<title>Deep Learning – Journal of Nuclear Medicine</title>".as_bytes())
                .as_deref(),
            Some("Deep Learning"),
            "an en dash separates the title from the site in the other template"
        );
        assert_eq!(
            html_title(b"<title>First: A Subtitle That Is Part Of It</title>").as_deref(),
            Some("First: A Subtitle That Is Part Of It"),
            "a colon inside a title is not a site separator"
        );
        assert_eq!(html_title(b"<html><body>paywall</body></html>"), None);
        assert_eq!(
            html_title(&[0xff, 0xfe, 0x00]),
            None,
            "non-UTF-8 is no title"
        );
    }

    /// A PDF's `/Title` is read in both encodings that need no decompressor,
    /// and a PDF with no readable `/Title` honestly reports none rather than
    /// guessing — which is what makes it `unverified` downstream.
    #[test]
    fn a_pdfs_title_is_read_from_its_information_dictionary() {
        assert_eq!(
            pdf_title(b"%PDF-1.7\n/Title(Deep Learning for Imaging)\n%%EOF\n").as_deref(),
            Some("Deep Learning for Imaging")
        );
        assert_eq!(
            pdf_title(b"%PDF-1.7\n/Title <FEFF00440065> /Author(x)\n%%EOF\n").as_deref(),
            Some("De"),
            "a hex string with a UTF-16BE BOM is decoded as one"
        );
        assert_eq!(
            pdf_title(b"%PDF-1.7\n/Title 12 0 R\n%%EOF\n"),
            None,
            "an indirect reference is a title this module cannot read"
        );
        assert_eq!(pdf_title(b"%PDF-1.7\ntrailer\n%%EOF\n"), None);
    }

    /// The magic-number check, and the real shape it defends against: a place
    /// that answers a request it cannot satisfy with a full HTML site page.
    #[test]
    fn pdf_magic_bytes_are_the_test() {
        assert!(is_pdf(b"%PDF-1.7\nbody"));
        assert!(is_pdf(b"%PDF-2.0"));
        assert!(!is_pdf(b"<!DOCTYPE html><html>404</html>"));
        assert!(!is_pdf(b""));
        assert_eq!(
            served_identity(b"<!DOCTYPE html><title>Error 404</title>")
                .and_then(|served| served.title)
                .as_deref(),
            Some("Error 404")
        );
        assert_eq!(
            served_identity(b"%PDF-1.4\n/Title(The real one)\n")
                .and_then(|served| served.title)
                .as_deref(),
            Some("The real one"),
            "a PDF reports only a title: a PDF's bytes say far less about \
             themselves than a served page's do"
        );
    }
}
