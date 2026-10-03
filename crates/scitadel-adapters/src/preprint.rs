//! Deterministic preprint routes: for a preprint DOI, where the full text
//! lives is a *function of the DOI* (#260).
//!
//! The acquisition ladder had no preprint route at all, so it could only find
//! a bioRxiv or medRxiv preprint by asking OpenAlex for
//! `best_oa_location.pdf_url`. When that field was null — which it was for
//! every `10.1101` DOI in the corpus that opened #260 — the work was reported
//! unreachable, even though the server serves it openly and always has. The
//! failure was a missing *route*, not a missing *access*, and it cost three
//! of three preprints in that run.
//!
//! These transforms need no index, no search and no credentials: one string
//! in, up to two URLs out. That is why this module is pure — no client, no
//! clock, no database — so every rule below is unit-testable without a
//! network, and so a rule's correctness is a question about string arithmetic
//! rather than about a server's mood on the day.
//!
//! ## Verification
//!
//! Each rule below records what was probed, against which URL, and what came
//! back — the discipline [`policy.rs`](scitadel_http) applies to its bucket
//! intervals, where a policy table with unattributed numbers is a policy
//! table nobody can audit. The three findings that shape this module:
//!
//! 1. **`10.1101` covers both servers, and only `biorxiv.org` serves them.**
//!    The prefix is shared by bioRxiv and medRxiv, so the DOI cannot say which
//!    one a preprint is on, and `www.medrxiv.org/content/<doi>v1.full.pdf`
//!    returns **403** for every `User-Agent` — its own `/content/<doi>` path
//!    does not exist there, so the request falls through to `/node/`, which
//!    refuses. `www.biorxiv.org/content/10.1101/2024.10.10.615955v1.full.pdf`
//!    — a **medRxiv** DOI — returns **200**. Every `10.1101` DOI therefore
//!    routes to `biorxiv.org`, and no candidate this module emits ever names
//!    `medrxiv.org`.
//! 2. **arXiv's `arxiv.` is part of the DOI suffix, not the identifier.**
//!    `https://arxiv.org/pdf/2301.00001` returns **200 application/pdf**
//!    (765 699 bytes); `https://arxiv.org/pdf/arXiv.2301.00001` returns
//!    **406**. The `10.48550` suffix keeps its `arxiv.` prefix and it must be
//!    stripped.
//! 3. **ChemRxiv has no deterministic transform** (see
//!    [`CHEMRXIV_NO_VERIFIED_TRANSFORM`]). Its content sits behind an
//!    `engage` API, so a `10.26434` DOI yields *no* candidate here rather
//!    than a guessed URL — an unverified URL is a 404 that looks like a
//!    negative finding, which is the mistake this module exists to remove.
//!
//! A transform that has not been probed does not go in this module. Every
//! absent prefix returns an empty list, and every such DOI simply falls
//! through to the ladder's index legs.

use scitadel_core::models::RouteId;
use scitadel_core::models::validate_doi_detailed;

/// The `10.1101` registrant: Cold Spring Harbor Laboratory, which registers
/// both bioRxiv and medRxiv preprints under one prefix.
///
/// One prefix, two servers, and the DOI does not say which — which is why
/// [`biorxiv_candidates`] sends everything here to `biorxiv.org`.
const BIORXIV_REGISTRANT: &str = "1101";

/// The `10.48550` registrant: arXiv's DOI prefix.
const ARXIV_REGISTRANT: &str = "48550";

/// The `10.26434` registrant: ChemRxiv.
const CHEMRXIV_REGISTRANT: &str = "26434";

/// Why a `10.26434` (ChemRxiv) DOI gets no candidate.
///
/// ChemRxiv's landing pages and full text sit behind an `engage` JSON API
/// rather than a derivable `/content/<doi>` path, and no DOI→URL transform
/// was verified against the live server for this slice. Rather than ship a
/// plausible-looking URL that would 404 — and a 404 on a guessed URL is
/// indistinguishable from a real negative finding, which is how #260
/// happened — the rule returns nothing and says so here, and the ladder
/// records this sentence as the reason the preprint leg was skipped.
///
/// Revisit when a ChemRxiv route has been probed the way the rules above
/// were; until then the honest answer is "no route I have verified".
pub const CHEMRXIV_NO_VERIFIED_TRANSFORM: &str = "ChemRxiv (10.26434) content sits behind an engage API; \
     no deterministic DOI->PDF transform has been verified, so this module proposes no URL rather than a guessed one";

/// One URL to try, with the route that would serve it and why it is listed.
///
/// `note` carries the candidate's provenance — which server, which path
/// shape, and what was observed when it was probed — because a caller that
/// logs or reports a route needs to say *why we believe this URL works* and
/// not merely which URL it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprintCandidate {
    /// The absolute URL to fetch.
    pub url: String,
    /// The route that serves it, which is what an artefact fetched from this
    /// URL is recorded as.
    pub route: RouteId,
    /// What this candidate is, attributed.
    pub note: &'static str,
}

/// The servers these transforms are built against.
///
/// A value rather than two `const`s for one reason: a downloader that cannot
/// be pointed at a `wiremock` server cannot have a permit counted against it,
/// and a transform that cannot be pointed at a test server is only testable
/// by hitting the real one. [`PreprintBases::default`] is the production
/// configuration and the only one the ladder uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreprintBases {
    /// Base of bioRxiv's `/content/<doi>v<n>.full.pdf` path.
    ///
    /// Serves **both** `10.1101` prefixes — see the module docs.
    pub biorxiv_content: String,
    /// Base of arXiv's `/pdf/<id>` path, with no file extension.
    pub arxiv_pdf: String,
}

impl Default for PreprintBases {
    fn default() -> Self {
        Self {
            biorxiv_content: "https://www.biorxiv.org/content".to_string(),
            arxiv_pdf: "https://arxiv.org/pdf".to_string(),
        }
    }
}

/// The candidate URLs to try for `doi`, best first.
///
/// An empty list means **either** "this DOI is not a preprint DOI" **or** "it
/// is, and we have no verified transform for it". Those are different
/// statements, so a caller that has to explain itself should ask
/// [`no_transform_reason`] rather than infer the difference from an empty
/// list.
///
/// # Errors
///
/// None: a DOI that cannot be transformed is an empty list, not a failure. A
/// malformed DOI is rejected here for the same reason it is rejected before
/// a fetch (#262) — a truncated identifier must not become a URL — and the
/// ladder falls through to its other legs either way.
#[must_use]
pub fn preprint_candidates(doi: &str) -> Vec<PreprintCandidate> {
    preprint_candidates_at(&PreprintBases::default(), doi)
}

/// [`preprint_candidates`] against an explicit set of servers.
///
/// The one rule implementation, two callers: the ladder passes its
/// configured endpoints, so a test can point the transform at a `wiremock`
/// server without the ladder growing a second, near-identical copy of the
/// transform that could drift from this one.
#[must_use]
pub fn preprint_candidates_at(bases: &PreprintBases, doi: &str) -> Vec<PreprintCandidate> {
    // #262's gate, and the only normalisation there is: a DOI list that
    // contains a truncated Elsevier identifier must not turn one into a
    // fetchable bioRxiv URL, and `validate_doi_detailed` returns the
    // canonical form, so a `doi:` CURIE and a `https://doi.org/…` URL reach
    // the same rule.
    let Ok(normalized) = validate_doi_detailed(doi) else {
        return Vec::new();
    };
    // `validate_doi_detailed` has already established the `10.` prefix, a `/`
    // and a non-empty suffix, so neither step below can fail; both are written
    // to return rather than to unwrap so a future relaxation of the validator
    // cannot panic a fetch path.
    let Some((registrant, suffix)) = normalized
        .strip_prefix("10.")
        .and_then(|rest| rest.split_once('/'))
    else {
        return Vec::new();
    };

    match registrant {
        BIORXIV_REGISTRANT => biorxiv_candidates(bases, &normalized),
        ARXIV_REGISTRANT => arxiv_candidates(bases, suffix),
        _ => Vec::new(),
    }
}

/// Why a preprint DOI has no candidate here, or `None` if it is not one.
///
/// The empty list from [`preprint_candidates`] is ambiguous on its own, and
/// an ambiguous "we could not find a route" is how a gap gets reported as a
/// wall. This gives the caller the sentence to write down.
#[must_use]
pub fn no_transform_reason(doi: &str) -> Option<&'static str> {
    let Ok(normalized) = validate_doi_detailed(doi) else {
        return None;
    };
    let (registrant, _) = normalized
        .strip_prefix("10.")
        .and_then(|rest| rest.split_once('/'))?;
    (registrant == CHEMRXIV_REGISTRANT).then_some(CHEMRXIV_NO_VERIFIED_TRANSFORM)
}

/// `10.1101` → bioRxiv's `/content/<doi>v<n>.full.pdf`, then the un-versioned
/// form.
///
/// **Verified against the live server, 2026-10:**
///
/// - `https://www.biorxiv.org/content/10.1101/2025.06.14.659707v1.full.pdf`
///   → **200 application/pdf** (a bioRxiv DOI).
/// - `https://www.biorxiv.org/content/10.1101/2024.10.10.615955v1.full.pdf`
///   → **200** (a **medRxiv** DOI, served here without complaint — this is
///   the observation that licenses routing the whole prefix to one host).
/// - `https://www.biorxiv.org/content/10.1101/<id>.full.pdf` (no version)
///   → **200** as well, which is why it is the second candidate rather than
///   a fallback that was never tried.
/// - `https://www.medrxiv.org/content/10.1101/<id>v1.full.pdf` → **403**,
///   for every `User-Agent` tried, because medRxiv has no `/content/<doi>`
///   path of its own. It is therefore never named here.
///
/// The order is a choice, not a measured ranking: **both** URLs returned 200
/// when probed, so neither is a fallback that was never tried. The versioned
/// path goes first because it names one specific version of the posting,
/// which is the more conservative thing to cite, and the un-versioned path is
/// kept behind it as cheap insurance.
fn biorxiv_candidates(bases: &PreprintBases, doi: &str) -> Vec<PreprintCandidate> {
    vec![
        PreprintCandidate {
            url: format!("{}/{doi}v1.full.pdf", bases.biorxiv_content),
            route: RouteId::Biorxiv,
            note: "biorxiv.org /content/<doi>v1.full.pdf — probed 200 application/pdf, \
                   including for a medRxiv DOI; medrxiv.org 403s the same path",
        },
        PreprintCandidate {
            url: format!("{}/{doi}.full.pdf", bases.biorxiv_content),
            route: RouteId::Biorxiv,
            note: "biorxiv.org /content/<doi>.full.pdf (un-versioned) — probed 200; \
                   cheap insurance against a DOI whose v1 path differs",
        },
    ]
}

/// `10.48550` → arXiv's `/pdf/<id>`, with the DOI suffix's `arxiv.` prefix
/// stripped.
///
/// **Verified against the live server, 2026-10:**
///
/// - `https://arxiv.org/pdf/2301.00001` → **200 application/pdf**,
///   765 699 bytes.
/// - `https://arxiv.org/pdf/arXiv.2301.00001` → **406**. The DOI is
///   `10.48550/arXiv.2301.00001`, so the `arxiv.` belongs to the DOI's suffix
///   and not to arXiv's identifier; keeping it produces a URL the server
///   refuses.
///
/// No `.pdf` suffix is appended, unlike the record-level arXiv leg in
/// `download.rs`: the transform verified above is the bare `/pdf/<id>`
/// path, and this module does not decorate a verified URL with a guess.
fn arxiv_candidates(bases: &PreprintBases, suffix: &str) -> Vec<PreprintCandidate> {
    vec![PreprintCandidate {
        url: format!("{}/{}", bases.arxiv_pdf, arxiv_id_from_suffix(suffix)),
        route: RouteId::Arxiv,
        note: "arxiv.org /pdf/<id> with the DOI suffix's `arxiv.` prefix stripped — \
               probed 200 application/pdf (765699 bytes); the unstripped URL returns 406",
    }]
}

/// The arXiv identifier inside a `10.48550` suffix.
///
/// `normalize_doi` lowercases, so by the time a suffix arrives here it is
/// almost always already `arxiv.…`; the case-insensitive comparison is
/// belt-and-braces for a caller that reaches this function by another route,
/// and it costs nothing. The `.get(..6)` keeps the slice from panicking on a
/// short or non-ASCII suffix — a byte-index panic in a fetch path is not a
/// thing this module gets to introduce.
fn arxiv_id_from_suffix(suffix: &str) -> &str {
    const ARXIV_PREFIX: &str = "arxiv.";
    let trimmed = suffix.trim();
    match trimmed.get(..ARXIV_PREFIX.len()) {
        Some(head) if head.eq_ignore_ascii_case(ARXIV_PREFIX) => &trimmed[ARXIV_PREFIX.len()..],
        _ => trimmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact transform from the measured probe, for the DOI that opened
    /// #260. If this fails, the URL is not the one that returned 200
    /// application/pdf, and the leg above it is decoration.
    #[test]
    fn a_biorxiv_doi_maps_to_the_verified_url() {
        let candidates = preprint_candidates("10.1101/2025.06.14.659707");
        assert!(
            candidates
                .iter()
                .any(|c| c.url
                    == "https://www.biorxiv.org/content/10.1101/2025.06.14.659707v1.full.pdf"),
            "the verified 200 URL must be a candidate: {candidates:?}"
        );
        assert_eq!(candidates[0].route, RouteId::Biorxiv);
        assert!(
            candidates.iter().all(|c| c.url.starts_with("https://")),
            "candidates are absolute URLs: {candidates:?}"
        );
    }

    /// **The prefix cannot pick a server, so the suffix must not appear in the
    /// URL at all.** `10.1101/2024.10.10.615955` is a *medRxiv* DOI and
    /// `www.medrxiv.org/content/10.1101/<id>v1.full.pdf` returns **403** for
    /// every User-Agent — medRxiv has no `/content/<doi>` path of its own —
    /// while `www.biorxiv.org` serves that same DOI with **200**. A candidate
    /// naming `medrxiv.org` is therefore not a cheaper miss; it is a route
    /// known to refuse, and this asserts none is ever proposed.
    #[test]
    fn a_medrxiv_doi_also_maps_to_biorxiv() {
        let candidates = preprint_candidates("10.1101/2024.10.10.615955");
        assert!(
            candidates
                .iter()
                .any(|c| c.url
                    == "https://www.biorxiv.org/content/10.1101/2024.10.10.615955v1.full.pdf"),
            "a medRxiv DOI is served by bioRxiv: {candidates:?}"
        );
        for candidate in &candidates {
            assert!(
                !candidate.url.contains("medrxiv"),
                "medRxiv 403s its own /content path, so no candidate may name it: {}",
                candidate.url
            );
            assert!(
                candidate.url.contains("biorxiv.org"),
                "every 10.1101 DOI routes to biorxiv.org: {}",
                candidate.url
            );
        }
    }

    /// `10.48550/arXiv.2301.00001` must become `/pdf/2301.00001`:
    /// `https://arxiv.org/pdf/2301.00001` is **200 application/pdf** (765 699
    /// bytes) and `https://arxiv.org/pdf/arXiv.2301.00001` is **406**.
    #[test]
    fn an_arxiv_doi_has_the_arxiv_prefix_stripped() {
        for raw in [
            "10.48550/arXiv.2301.00001",
            "10.48550/arxiv.2301.00001",
            "10.48550/ARXIV.2301.00001",
            "  10.48550/arXiv.2301.00001  ",
            "https://doi.org/10.48550/arXiv.2301.00001",
        ] {
            let candidates = preprint_candidates(raw);
            assert!(
                candidates
                    .iter()
                    .any(|c| c.url == "https://arxiv.org/pdf/2301.00001"),
                "{raw} must yield the verified 200 URL: {candidates:?}"
            );
            for candidate in &candidates {
                assert!(
                    !candidate.url.contains("/pdf/arxiv."),
                    "the `arxiv.` DOI-suffix prefix 406s and must be stripped: {}",
                    candidate.url
                );
            }
            assert_eq!(candidates[0].route, RouteId::Arxiv);
        }
    }

    /// A publisher DOI has no preprint route, and must not be given one: the
    /// ladder's next question is an index question, not a URL guess.
    #[test]
    fn a_publisher_doi_yields_no_preprint_candidate() {
        for doi in [
            "10.1016/0003-2670(93)90142-7",
            "10.3390/pharmaceutics14051098",
            "10.1039/qr9581200265",
            "10.18434/m32154",
            "10.1073/pnas.2415658122",
            "10.1093/nar/gkv1075",
        ] {
            assert!(
                preprint_candidates(doi).is_empty(),
                "{doi} is not a preprint DOI and must yield nothing"
            );
            assert_eq!(
                no_transform_reason(doi),
                None,
                "a publisher DOI has no transform to explain"
            );
        }
    }

    /// #262's trap, in this module's own doorway: a truncated DOI must produce
    /// no URL at all. `10.1016/0003-2670(93` is a real shape from the raid
    /// corpus — an Elsevier compact DOI cut at the parenthesis — and turning
    /// one into a fetchable URL is the same class of bug as putting one on
    /// the wire.
    #[test]
    fn a_malformed_doi_yields_no_preprint_candidate() {
        for bad in [
            "10.1016/0003-2670(93",
            "10.1016/0016-7037(84",
            "10.1101/0003-2670(93",
            "10.1234/...",
            "",
            "   ",
            "not-a-doi",
            "10.1101",
            "10.1101/",
            "10.1101/2025.06.14.659707?x=1",
        ] {
            let candidates = preprint_candidates(bad);
            assert!(
                candidates.is_empty(),
                "{bad:?} must yield no candidate, got {candidates:?}"
            );
        }
        // The gate is the DOI validator's, not this module's: the truncated
        // Elsevier DOI is rejected as an unbalanced bracket and the complete
        // one next to it is accepted (and then found not to be a preprint).
        assert_eq!(
            validate_doi_detailed("10.1016/0003-2670(93"),
            Err(scitadel_core::models::DoiRejection::UnbalancedBracket)
        );
        assert!(validate_doi_detailed("10.1016/0003-2670(93)90142-7").is_ok());
    }

    /// ChemRxiv is a real preprint server we deliberately do not serve, and
    /// the difference from "not a preprint DOI" has to survive in the output:
    /// an empty list with no explanation is how a missing route gets reported
    /// as a missing access.
    #[test]
    fn chemrxiv_has_no_deterministic_transform_and_says_so() {
        assert!(preprint_candidates("10.26434/chemrxiv.2024.01.01.123456.v1").is_empty());
        let reason = no_transform_reason("10.26434/chemrxiv.2024.01.01.123456.v1")
            .expect("a ChemRxiv DOI must be able to explain its own empty list");
        assert_eq!(reason, CHEMRXIV_NO_VERIFIED_TRANSFORM);
        assert!(
            reason.contains("engage"),
            "the reason names the obstacle: {reason}"
        );
        assert!(
            reason.contains("10.26434"),
            "the reason names the prefix it is about: {reason}"
        );
        // And it is a claim about ChemRxiv only: a prefix with no transform
        // reason is not "no route because ChemRxiv".
        assert_eq!(no_transform_reason("10.1101/2025.06.14.659707"), None);
        assert_eq!(no_transform_reason("10.1016/0003-2670(93)90142-7"), None);
    }

    /// The endpoints are a seam, not a second rule: pointing them elsewhere
    /// must change the host and nothing else. A downloader that cannot be
    /// aimed at a `wiremock` server cannot have a permit counted against it.
    #[test]
    fn the_bases_change_the_host_and_nothing_else() {
        let bases = PreprintBases {
            biorxiv_content: "http://127.0.0.1:38121/content".to_string(),
            arxiv_pdf: "http://127.0.0.1:38121/pdf".to_string(),
        };
        let candidates = preprint_candidates_at(&bases, "10.1101/2025.06.14.659707");
        assert_eq!(
            candidates[0].url,
            "http://127.0.0.1:38121/content/10.1101/2025.06.14.659707v1.full.pdf"
        );
        let arxiv = preprint_candidates_at(&bases, "10.48550/arXiv.2301.00001");
        assert_eq!(arxiv[0].url, "http://127.0.0.1:38121/pdf/2301.00001");
    }

    /// The production defaults are the URLs that were probed. A default that
    /// drifted to another host would make every rule above pass while the
    /// ladder fetched something else, so this pins the literal strings.
    #[test]
    fn the_default_bases_are_the_probed_hosts() {
        let bases = PreprintBases::default();
        assert_eq!(bases.biorxiv_content, "https://www.biorxiv.org/content");
        assert_eq!(bases.arxiv_pdf, "https://arxiv.org/pdf");
        assert!(
            !bases.biorxiv_content.contains("medrxiv"),
            "medRxiv 403s the transform this module uses, so it cannot be a base"
        );
    }

    /// Every candidate carries a note that attributes it, and a note that
    /// merely repeats the URL is not an attribution.
    #[test]
    fn every_candidate_says_why_it_is_believed() {
        for doi in ["10.1101/2025.06.14.659707", "10.48550/arXiv.2301.00001"] {
            for candidate in preprint_candidates(doi) {
                assert!(
                    candidate.note.contains("probed"),
                    "{doi}: {} has no probe behind it",
                    candidate.url
                );
                assert!(
                    !candidate.note.contains(candidate.url.as_str()),
                    "{}: the note must attribute the transform, not echo the URL",
                    candidate.url
                );
            }
        }
    }
}
