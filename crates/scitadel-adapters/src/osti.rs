//! ADR-007 §3 step 3: **DOE OSTI**, for national-lab reports that have no DOI.
//!
//! > 3. OA repository locations from OpenAlex and Unpaywall, arXiv for
//! >    preprints, and **DOE OSTI** for national-lab reports.
//!
//! A tier-1 route — `RouteId::Osti` is already `PaceTier::Oa` and
//! `access_basis = 'oa_license'` in [`RouteId`] — because OSTI's holdings are
//! US-government work: the site's own record calls them public domain, and
//! nothing gates them. It is a **transform of the identifier**, not an index
//! lookup, for the same reason `#260`'s preprint leg is: where a DOE report's
//! full text lives is a function of its `osti_id`, so there is nothing to look
//! up and no chance of an index answering "no location" for a report that is
//! served freely.
//!
//! # What the live service does (probed 2026-10-04)
//!
//! `https://www.osti.gov/servlets/purl/<osti_id>`:
//!
//! | probe | observed |
//! |---|---|
//! | `purl/1234567` | `200`, `content-type: application/pdf`, 317 973 bytes, `%PDF-` magic |
//! | `purl/1060205`, `purl/1561490` | `200 application/pdf`, `%PDF-` magic |
//! | `purl/999999999` | **`404`**, `content-type: text/html`, 264 918 bytes of the site's own 404 page |
//! | `purl/00000001` | `404 text/html`, same site page |
//! | `purl/abc` | `404 text/html` |
//! | `purl/1335400` | **`302`** to `https://www.bnl.gov/isd/documents/92760.pdf`, which serves a real PDF |
//!
//! Three of those rows are load-bearing for the implementation:
//!
//! - **The 404 is an HTML page, not an empty body.** So "the request succeeded"
//!   and "we got a document" are different questions, and the second one is
//!   answered by the magic bytes ([`crate::identity::is_pdf`]) rather than by
//!   the status code or the content type. See [`pdf_magic_bytes_fail_closed`].
//! - **A `purl` can redirect off `osti.gov` entirely.** The redirect is followed
//!   by [`PacedClient`](scitadel_http::PacedClient), which spends one `Request`
//!   permit per hop keyed to that hop's own bucket — so a national lab's host
//!   gets its own budget rather than being spent from OSTI's.
//! - **The id is the whole address.** No API call, no search, no lookup table:
//!   the same `osti_id` always yields the same URL.
//!
//! # Not in the bucket table
//!
//! `osti.gov` is deliberately left unrouted, so it takes ADR-007 §4's
//! **unknown-host** defaults: 5 s per request and 500 requests per rolling 24 h.
//! The 5 s is the conservative direction and needs no attribution. The 500 is a
//! scitadel policy choice, as every cap in that table is, and it is the one
//! number here worth revisiting: a national-lab corpus is small (OSTI holds
//! roughly 600 k records in total), but a campaign spanning several labs will
//! feel 5 s per record. The probe above saw no 429 and no rate-limit headers at
//! all across three hosts, which is an observation, not a published limit — and
//! the response to "we need more than 500" should be a routing entry with its
//! source attributed, exactly like the `doi.org` row, rather than a smaller
//! number nobody sourced.

/// OSTI's own site root. Both of its URL shapes are built from this, and it is
/// what [`crate::download::Endpoints::osti_base`] overrides so a test can point
/// the route at a `wiremock` server — the seam every other route already has.
pub const OSTI_BASE: &str = "https://www.osti.gov";

/// The path segment of the full-text endpoint.
const PURL_PATH: &str = "servlets/purl";

/// The path segment of a record's landing page — what a human is pointed at, and
/// what `artefacts.publisher_url` carries for this route.
const BIBLIO_PATH: &str = "biblio";

/// The bare OSTI id inside whatever form we were handed, or `None` when the
/// value cannot be one.
///
/// Accepts the shapes an `osti_id` arrives in, so a value pasted from anywhere in
/// OSTI's own URLs works:
///
/// - `1234567` — the id itself, which is what the column is for;
/// - `https://www.osti.gov/biblio/1234567` — a record page;
/// - `https://www.osti.gov/servlets/purl/1234567` — the full text, i.e. the
///   shape [`purl_url`] itself produces;
/// - `osti.gov/biblio/1234567` — the same without a scheme.
///
/// **Digits only, and from an OSTI address only.** The URL is built by string
/// concatenation, so a value carrying a path separator, a query or a fragment
/// would produce a request for something other than the record; refusing it here
/// is the same gate `#262`'s DOI check is, for the same reason. The observed
/// service agrees: `purl/abc` is a 404, not a redirect or a wildcard match — and
/// a value like `1234567/../999999999` must not resolve to `999999999` merely
/// because that is the last path segment.
#[must_use]
pub fn osti_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    // Strip a scheme and a query/fragment, leaving `host/path/…/id`.
    let without_scheme = match trimmed.split_once("://") {
        Some((scheme, rest))
            if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") =>
        {
            rest
        }
        Some(_) => return None,
        None => trimmed,
    };
    let candidate = match without_scheme.split_once('/') {
        Some((host, path)) => {
            if !is_osti_host(host) {
                return None;
            }
            path.trim_end_matches('/')
                .split(['?', '#'])
                .next()
                .unwrap_or_default()
                .rsplit('/')
                .next()
                .unwrap_or_default()
        }
        // No `/` at all: the value is the bare id, which is the column's own
        // shape. A value like `1234567/../999999999` has one, so it is checked
        // against the host and refused rather than resolved by its last segment.
        None => without_scheme.split(['?', '#']).next().unwrap_or_default(),
    };
    let candidate = candidate.trim();
    (!candidate.is_empty() && candidate.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| candidate.to_string())
}

/// Is this host OSTI's own? A bare `osti_id` has no host, so the host-less case is
/// handled by the caller never reaching here with a scheme — see [`osti_id`].
fn is_osti_host(host: &str) -> bool {
    let host = host.split('@').next_back().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    matches!(
        host.to_ascii_lowercase().as_str(),
        "osti.gov" | "www.osti.gov"
    )
}

/// The full-text URL for an `osti_id`, under [`OSTI_BASE`].
#[must_use]
pub fn purl_url(id: &str) -> Option<String> {
    purl_url_at(OSTI_BASE, id)
}

/// The full-text URL for an `osti_id`, under `base`.
#[must_use]
pub fn purl_url_at(base: &str, id: &str) -> Option<String> {
    osti_id(id).map(|id| format!("{}/{PURL_PATH}/{id}", base.trim_end_matches('/')))
}

/// The landing page for an `osti_id`, which is also what a human should be
/// pointed at for a report with no DOI to resolve.
#[must_use]
pub fn biblio_url(id: &str) -> Option<String> {
    biblio_url_at(OSTI_BASE, id)
}

/// The landing page for an `osti_id`, under `base`.
#[must_use]
pub fn biblio_url_at(base: &str, id: &str) -> Option<String> {
    osti_id(id).map(|id| format!("{}/{BIBLIO_PATH}/{id}", base.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL shape, which is the whole point of the route: the same id always
    /// yields the same address, with no lookup in between.
    #[test]
    fn the_full_text_url_is_a_function_of_the_id() {
        assert_eq!(
            purl_url("1234567").as_deref(),
            Some("https://www.osti.gov/servlets/purl/1234567")
        );
        assert_eq!(
            purl_url("1234567"),
            purl_url("  1234567  "),
            "a padded value is the same id"
        );
        assert_eq!(
            biblio_url("1234567").as_deref(),
            Some("https://www.osti.gov/biblio/1234567")
        );
        // The base is overridable so a test can point the route at a `wiremock`
        // server; the *shape* of the URL must not change when it is.
        assert_eq!(
            purl_url_at("http://127.0.0.1:9999/osti", "1234567").as_deref(),
            Some("http://127.0.0.1:9999/osti/servlets/purl/1234567")
        );
        assert_eq!(
            purl_url_at("http://127.0.0.1:9999/osti/", "1234567").as_deref(),
            purl_url_at("http://127.0.0.1:9999/osti", "1234567").as_deref(),
            "a trailing slash on the base must not double up"
        );
    }

    /// Every shape a value pasted out of OSTI works, and everything that is not
    /// an id is refused rather than concatenated into a URL — the same gate
    /// `#262`'s DOI check is, for the same reason.
    #[test]
    fn ids_are_recognised_in_every_shape_osti_hands_out() {
        for raw in [
            "1234567",
            " 1234567 ",
            "https://www.osti.gov/biblio/1234567",
            "https://www.osti.gov/servlets/purl/1234567",
            "http://www.osti.gov/biblio/1234567/",
            "1234567?utm_source=x",
            "1234567#abstract",
        ] {
            assert_eq!(osti_id(raw).as_deref(), Some("1234567"), "{raw}");
        }
        for raw in [
            "",
            "   ",
            "abc",
            "12a4567",
            "../../etc/passwd",
            "1234567/../999999999",
            "https://evil.example/biblio/1234567",
            "ftp://www.osti.gov/biblio/1234567",
        ] {
            assert_eq!(osti_id(raw), None, "{raw} must not become a URL");
            assert_eq!(purl_url(raw), None, "{raw} must not become a URL");
        }
    }

    /// The magic-byte rule, against the shapes the live service actually
    /// returned (probed 2026-10-04, see the module docs): a real PDF, and the
    /// site's own 404 page served with a `text/html` content type under a
    /// **404 status**.
    ///
    /// Asserted here rather than only through the ladder because the failure
    /// this prevents is invisible: an HTML error page filed as `fulltext_pdf`
    /// makes `coverage` claim a full text nobody can read, and nothing else in
    /// the system would notice.
    #[test]
    fn pdf_magic_bytes_fail_closed() {
        assert!(
            crate::identity::is_pdf(b"%PDF-1.6\nbody"),
            "the bytes OSTI serves for a known id"
        );
        assert!(
            !crate::identity::is_pdf(
                b"<!DOCTYPE html>\n<html lang=\"en\" class=\"html page-node-2524480847\
                  \n<title>Page not found</title>"
            ),
            "the 264 kB HTML page OSTI serves for an unknown id must never be filed"
        );
        assert!(!crate::identity::is_pdf(b""), "an empty body is not a PDF");
        assert!(
            !crate::identity::is_pdf(&[0x00, 0x01, 0x02]),
            "nor is a binary prefix that merely precedes one"
        );
    }
}
