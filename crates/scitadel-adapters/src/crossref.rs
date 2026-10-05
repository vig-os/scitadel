//! Crossref, as a source of identity: `GET /works/<doi>` (ADR-007 §3's
//! pre-fetch chain, second hop).
//!
//! # Why this is its own adapter and not a parameter of the DataCite one
//!
//! They are **different registries with different coverage**, and a probe of the
//! DOIs in this repo's own evidence settles which is which in a way no amount of
//! argument would have:
//!
//! | DOI | Crossref | DataCite |
//! |---|---|---|
//! | `10.1101/2025.06.14.659707` (bioRxiv, #260) | **200** | 404 |
//! | `10.18434/m32154` (OSTI/NIST, #260) | 404 | **200** |
//! | `10.5281/zenodo.23162961` (Zenodo) | 404 | **200** |
//! | `10.26434/chemrxiv.15000484/v1` (ChemRxiv) | **200** | 404 |
//!
//! Two of #260's four are Crossref's and two are DataCite's, so a chain that
//! only asked one of them would corroborate nothing. The asymmetry is in the
//! *data model* too — see [the field asymmetry](#the-field-asymmetry-is-not-smoothed-over).
//!
//! # The measured request shape
//!
//! ```text
//! GET https://api.crossref.org/works/<doi>
//!     ?mailto=<polite-pool address>
//! ```
//!
//! Measured 2026-10 against the live API:
//!
//! - the DOI goes in the **path**, percent-encoded by
//!   [`Url::path_segments_mut`] rather than by hand. A segmented identifier —
//!   `10.26434/chemrxiv.15000484/v1`, #261's trap — has a `/` inside the
//!   suffix, and both the raw and the `%2F`-encoded spellings answer `200`,
//!   so the encoding is a correctness choice rather than a compatibility one.
//!   A hand-spliced string would let a DOI containing `#` or `?` truncate the
//!   path, and the validator is deliberately permissive about suffix structure
//!   (#262), so it would not stop one.
//! - `mailto` is what puts the request in the **polite pool**, and the response
//!   says so: `x-api-pool: polite-single`. The rate-limit headers come back with
//!   it — `x-rate-limit-limit: 10`, `x-rate-limit-interval: 1s`,
//!   `x-concurrency-limit: 3` — for the single-record route. `policy.rs`'s
//!   `crossref` bucket is set from those, and the value and its provenance are
//!   recorded next to the row.
//!
//! # What a miss looks like, and the trap in it
//!
//! ```text
//! HTTP/2 404   content-type: text/plain   19 bytes
//! Resource not found.
//! ```
//!
//! Crossref's miss is **plain text, not JSON**. That is the lucky case, and it
//! is lucky by accident: `serde_json` refuses it, so a parser errors out rather
//! than inventing a record. It is not a defence, and it is not the shape the
//! chain can rely on, because DataCite's miss **is** JSON (see
//! [`crate::datacite`]) and because a lenient parser that swallows a parse
//! failure would still hand a `WorkIdentity` with three `None`s to the matcher,
//! which reads that as "no title, no year, no author" and answers
//! `unverified` — making every unregistered DOI look like a work we could not
//! read rather than a DOI this registry has never heard of.
//!
//! So "not registered" is decided by the **status code and nothing else**.
//! [`PacedClient`] refuses any non-2xx before a body can be read, so a 404
//! cannot reach [`parse_work`] at all; and [`parse_work`] independently requires
//! the `message` envelope, so even a 200 carrying `{"status":"failed"}` yields
//! no record. Two independent gates, and
//! `a_crossref_404_body_is_not_read_as_metadata` pins the pair.

use reqwest::Url;
use scitadel_core::models::{normalize_doi, validate_doi};
use scitadel_core::ports::PaceTier;
use scitadel_http::{PacedClient, PacedResponse, SafeHeaders, WorkScope};

use crate::error::AdapterError;
use crate::identity::WorkIdentity;

/// Base of the single-record route. A value, not a `const` spliced at the call
/// site, so a test can point one adapter at one `wiremock` server.
pub const CROSSREF_WORKS_URL: &str = "https://api.crossref.org/works";

/// What one Crossref lookup established.
#[derive(Debug, Clone, PartialEq)]
pub enum CrossrefAnswer {
    /// Crossref carries this DOI. The three facts [`crate::identity::verify`]
    /// consumes, however many of them the record actually has.
    Found(WorkIdentity),
    /// Crossref answered **404**: this DOI is not in Crossref's registry.
    ///
    /// A fact, not a failure, and the reason the chain keeps walking — a
    /// `10.18434` or `10.5281` DOI is registered here and only here. Distinct
    /// from [`Self::Unreadable`] on purpose: "not registered" is knowledge and
    /// "we could not read the answer" is not.
    NotRegistered,
    /// A 200 we could not read. We do not know whether the DOI is registered.
    Unreadable(String),
}

/// A Crossref client that spends [`PaceTier::Meta`] permits and carries no
/// credential.
///
/// Follows [`crate::openalex`]'s shape — a base URL, a `with_base_url` test
/// seam, and a single [`Self::lookup`] that owns the whole request — because
/// that is the pattern this codebase already uses for a metadata registry and a
/// third variant of it would be a second thing to get wrong.
#[derive(Debug, Clone)]
pub struct CrossrefAdapter {
    base_url: String,
    /// The polite-pool `mailto`. Empty omits the parameter rather than sending
    /// it blank; an empty `mailto` puts the request in the common pool, which
    /// is a rate-limit regression rather than a correctness one.
    mailto: String,
}

impl CrossrefAdapter {
    /// The production adapter.
    #[must_use]
    pub fn new(mailto: impl Into<String>) -> Self {
        Self {
            base_url: CROSSREF_WORKS_URL.to_string(),
            mailto: mailto.into(),
        }
    }

    /// Point the adapter at a different `/works` endpoint. Test seam; the
    /// production default is [`CROSSREF_WORKS_URL`].
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The lookup URL for `doi`, percent-encoded and polite-pool-tagged.
    ///
    /// Public because the request line is the thing a probe measures and the
    /// thing a reader of a captured request needs to check, and re-deriving it
    /// in a test would test a second implementation.
    ///
    /// # Errors
    ///
    /// Only if the base URL is unparseable — which for a caller-chosen test
    /// seam is the interesting failure, and for [`CROSSREF_WORKS_URL`] cannot
    /// happen.
    pub fn works_url(&self, doi: &str) -> Result<Url, AdapterError> {
        let mut url = Url::parse(&self.base_url)
            .map_err(|e| AdapterError::Validation(format!("invalid Crossref base URL: {e}")))?;
        // `path_segments_mut` rather than `format!("{base}/{doi}")`: it encodes
        // each segment, so a DOI's own `/` becomes `%2F` and a `#` or `?` cannot
        // truncate the path. Both spellings answer 200 against the live API —
        // measured — so this is safety, not compatibility.
        url.path_segments_mut()
            .map_err(|()| AdapterError::Validation("Crossref base URL cannot be a base".into()))?
            .extend([normalize_doi(doi)]);
        if !self.mailto.is_empty() {
            url.query_pairs_mut().append_pair("mailto", &self.mailto);
        }
        Ok(url)
    }

    /// Resolve `doi` against Crossref, spending one `Request` permit.
    ///
    /// `work` is the walk's [`WorkScope`], so this hop is charged the `Work`
    /// permit for the `crossref` bucket once per work rather than once per
    /// registry consulted (ADR-007 §4).
    ///
    /// The DOI is validated first, so a malformed one never touches the wire —
    /// the same gate [`crate::openalex`] applies.
    ///
    /// # Errors
    ///
    /// Only for a DOI the validator rejects, or a base URL that will not parse.
    /// A 404 is [`CrossrefAnswer::NotRegistered`], not an error: that is the
    /// case the whole chain exists to survive.
    pub async fn lookup(
        &self,
        client: &PacedClient,
        work: &WorkScope,
        doi: &str,
    ) -> Result<CrossrefAnswer, AdapterError> {
        if !validate_doi(doi) {
            return Err(AdapterError::Validation(format!("invalid DOI: {doi}")));
        }
        let url = self.works_url(doi)?;
        let response = match client
            .get_in_work(work, url, PaceTier::Meta, SafeHeaders::unauthenticated())
            .await
        {
            Ok(response) => response,
            Err(error) => return Ok(classify(error, doi)),
        };
        // `text()` then `from_str`, rather than a `json()` helper: this is the
        // idiom [`crate::download`]'s OpenAlex leg already uses. A read or a
        // parse failure is one of the ways a 200 is unreadable, and it belongs
        // in the *answer* rather than in the error channel — this function's
        // error channel is for "we could not even ask", and a body we could not
        // read is a fact about the answer.
        Ok(read_work(response).await)
    }
}

/// The answer a 2xx body carries, or the reason it carries none.
///
/// Split out so the three ways a 200 can be unreadable — the body would not
/// read, the body is not JSON, the JSON is not a work — are one expression
/// rather than three `?`s in the request path.
async fn read_work(response: PacedResponse) -> CrossrefAnswer {
    let text = match response.text().await {
        Ok(text) => text,
        Err(e) => return CrossrefAnswer::Unreadable(format!("could not read the body: {e}")),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(body) => parse_work(&body),
        Err(e) => CrossrefAnswer::Unreadable(format!("JSON parse failed: {e}")),
    }
}

/// Turn a failed hop into the answer it establishes.
///
/// The one arm that matters: a **404 is "not registered"**, never an error and
/// never a parse attempt. `PacedClient` has already refused to hand back a body
/// for a non-2xx, so there is nothing here that could read a 404's error
/// document as metadata — the distinction is made on the status code, which is
/// the only part of the response that means anything here.
///
/// Everything else is [`CrossrefAnswer::Unreadable`]: a 5xx, a transport
/// failure, a pacing refusal. All three are "we could not find out", and
/// collapsing them into "not registered" would skip the rest of the chain on a
/// transient error.
fn classify(error: scitadel_http::FetchError, doi: &str) -> CrossrefAnswer {
    match error {
        scitadel_http::FetchError::Status { code: 404, .. } => {
            tracing::info!(doi = %doi, "crossref does not register this DOI");
            CrossrefAnswer::NotRegistered
        }
        other => CrossrefAnswer::Unreadable(other.to_string()),
    }
}

/// The three facts the matcher consumes, out of a Crossref `message` envelope.
///
/// `None` when the body is not that envelope — and that is the whole of the
/// 404 defence on this side. A 404 body is never parsed, but a **200** carrying
/// an error document (`{"status":"failed"}`, or a bare `{}` from a proxy) must
/// not become a record either, and the way to guarantee that is to insist on
/// the key the success shape has and an error document does not.
///
/// A missing *field* inside a real envelope is `None` rather than a placeholder,
/// for the reason [`crate::identity`] states throughout: an absent fact must
/// read as `unverified`, never as agreement.
#[must_use]
pub fn parse_work(body: &serde_json::Value) -> CrossrefAnswer {
    let Some(message) = body.get("message").filter(|m| m.is_object()) else {
        // Distinguishable in the log from a transport failure, because it is a
        // different bug: a 200 that is not a work.
        tracing::info!("crossref answered 200 without a `message` envelope");
        return CrossrefAnswer::Unreadable("no `message` envelope in the response".into());
    };
    CrossrefAnswer::Found(WorkIdentity {
        // An **array**, and the first entry. Crossref carries subtitles and
        // translated titles as later entries; the first is the canonical one.
        title: message
            .get("title")
            .and_then(serde_json::Value::as_array)
            .and_then(|titles| titles.first())
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        year: crossref_year(message),
        // An **object** `{family, given}`, not a string — the asymmetry with
        // DataCite's `creators[].name` that [`crate::datacite`] documents.
        first_author: message
            .get("author")
            .and_then(serde_json::Value::as_array)
            .and_then(|authors| authors.first())
            .and_then(crossref_author_name),
    })
}

/// The year, out of Crossref's nested date shape.
///
/// `{"date-parts": [[2025, 6, 18]]}` — a two-deep array, because a Crossref
/// date is a *range* and the inner array is one endpoint. There is no
/// `publicationYear` scalar anywhere in the record: DataCite has one, Crossref
/// does not, and pretending otherwise would be inventing a field.
///
/// Checked in the order that means most for this module:
///
/// 1. `published` — when the work was published, which is what ADR-007 §3's
///    "year ±1" is about;
/// 2. `issued` — Crossref's own preferred date, and identical for a preprint
///    (`posted-content`) and for a journal article in the cases probed;
/// 3. `created` — a **registration** date, so it is the last resort and not
///    the first: a preprint registered in 2026 for a 2022 posting would
///    corroborate a 2022 paper with 2026 and read as a four-year
///    disagreement, which is a *contradiction* and would turn a correct
///    preprint into a `mismatch`.
///
/// The first component of the first inner array is the year, or `None` if it is
/// not an integer — a date we cannot read is not a year.
fn crossref_year(message: &serde_json::Value) -> Option<i32> {
    ["published", "issued", "created"]
        .into_iter()
        .find_map(|key| year_from_date_parts(message.get(key)))
}

fn year_from_date_parts(date: Option<&serde_json::Value>) -> Option<i32> {
    let year = date?
        .get("date-parts")?
        .as_array()?
        .first()?
        .as_array()?
        .first()?
        .as_i64()?;
    i32::try_from(year).ok()
}

// ============================================================================
// ADR-007 §3's resolve-then-rank: the fields the identity pass never read.
// ============================================================================

/// The version Crossref declares for a record, from `message.type`.
///
/// This is the field #292's agents established and that the preprint leg's
/// DOI-prefix inference exists only as a stand-in for: Crossref carries
/// `type: posted-content` for every preprint its members deposit, which is a
/// **statement by the publisher about its own DOI**, where a `10.1101` prefix
/// is an inference from who registered it. The ranker prefers this and records
/// that it did; see [`crate::resolve`].
///
/// `None` when the field is absent or is not a string — which is a fact about
/// Crossref's deposit, not about the work, and is exactly the case the
/// prefix-inference fallback exists for.
#[must_use]
pub fn type_signal(message: &serde_json::Value) -> Option<crate::registry::RegistryType> {
    message
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(|word| crate::registry::RegistryType::of(word, crate::registry::Dialect::Crossref))
}

/// Every `link[]` entry Crossref carries, with the two facts the ranker reads.
///
/// `intended-application: text-mining` is the publisher's own statement that
/// this URL is the sanctioned TDM route, which ADR-007 §3 step 5 turns on.
/// `content-version` is the publisher's statement of *which* rendering the link
/// serves — `vor` or `am` — which is the per-location version signal, the same
/// role OpenAlex's `locations[].version` plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrossrefLink {
    pub url: String,
    pub content_type: Option<String>,
    /// `intended-application`, e.g. `text-mining` or `similarity-checking`.
    pub intended_application: Option<String>,
    /// `content-version`, e.g. `vor` or `am`.
    pub content_version: Option<String>,
    pub version: Option<String>,
}

/// Read `message.link[]`, in Crossref's own array-of-objects shape.
#[must_use]
pub fn links(message: &serde_json::Value) -> Vec<CrossrefLink> {
    message
        .get("link")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let url = entry.get("URL").and_then(serde_json::Value::as_str)?;
                    Some(CrossrefLink {
                        url: url.trim().to_string(),
                        content_type: entry
                            .get("content-type")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        intended_application: entry
                            .get("intended-application")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        content_version: entry
                            .get("content-version")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        version: entry
                            .get("version")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One Crossref author's name, in the `Family, Given` order
/// [`crate::identity::surname`] reads.
///
/// `family` alone is enough for a surname and is what the matcher compares, so
/// it is preferred; a record carrying only `given` (an unparsed single-name
/// author) contributes nothing rather than contributing the wrong token. The
/// `given` is folded in so a reader looking at the stored row sees the name as
/// the registry spells it, which for a mononym is the same string.
fn crossref_author_name(author: &serde_json::Value) -> Option<String> {
    let family = author.get("family").and_then(serde_json::Value::as_str);
    let given = author
        .get("given")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|given| !given.is_empty());
    match (family.map(str::trim).filter(|f| !f.is_empty()), given) {
        (Some(family), Some(given)) => Some(format!("{family}, {given}")),
        (Some(family), None) => Some(family.to_string()),
        (None, Some(given)) => Some(given.to_string()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::ports::{Bucket, Cost, PaceDenied, Pacer, Permit};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Grants everything, immediately. The ledger's behaviour is
    /// `scitadel-db`'s to test.
    #[derive(Debug, Default)]
    struct GrantingPacer;

    #[async_trait::async_trait]
    impl Pacer for GrantingPacer {
        async fn acquire(
            &self,
            bucket: &Bucket,
            tier: PaceTier,
            _cost: Cost,
        ) -> Result<Permit, PaceDenied> {
            Ok(Permit {
                bucket: bucket.clone(),
                tier,
                not_before: std::time::Instant::now(),
            })
        }
    }

    /// The measured Crossref envelope for `10.1101/2025.06.14.659707`, reduced
    /// to the fields this module reads and left otherwise as Crossref sends it
    /// — the nesting is the point, and flattening it in a fixture would stop
    /// testing the thing that differs from DataCite.
    const BIO_RXIV_MESSAGE: &str = r#"{
      "status": "ok",
      "message-type": "work",
      "message": {
        "DOI": "10.1101/2025.06.14.659707",
        "type": "posted-content",
        "subtype": "preprint",
        "title": ["Boltz-2: Towards Accurate and Efficient Binding Affinity Prediction"],
        "container-title": [],
        "published": {"date-parts": [[2025, 6, 18]]},
        "issued": {"date-parts": [[2025, 6, 18]]},
        "created": {"date-parts": [[2025, 6, 19]], "date-time": "2025-06-19T01:00:16Z"},
        "posted": {"date-parts": [[2025, 6, 18]]},
        "author": [
          {"given": "Saro", "family": "Passaro", "sequence": "first", "affiliation": []}
        ],
        "publisher": "openRxiv",
        "institution": [{"name": "bioRxiv"}]
      }
    }"#;

    /// A policy table that routes `server`'s address to `bucket`, so a test can
    /// assert on the **bucket** a hop spends rather than on the
    /// `127.0.0.1:<port>` bucket an unrouted local address would otherwise get.
    ///
    /// The routing is the honest shape: production sends `api.openalex.org`,
    /// `api.crossref.org` and `api.datacite.org` to three named buckets, and a
    /// test that only counted permits would never notice if one of those hosts
    /// were misrouted in `policy.rs`.
    fn routing_table(server: &MockServer, bucket: &str) -> scitadel_http::BucketPolicyTable {
        let mut table = scitadel_http::BucketPolicyTable::new();
        table.route(&format!("127.0.0.1:{}", server.address().port()), bucket);
        table
    }

    fn adapter_for(server: &MockServer) -> CrossrefAdapter {
        CrossrefAdapter::new("polite@example.org").with_base_url(format!("{}/works", server.uri()))
    }

    fn client() -> PacedClient {
        PacedClient::new(
            PacedClient::default_transport().expect("transport builds"),
            Arc::new(GrantingPacer),
            scitadel_http::BucketPolicyTable::new(),
        )
    }

    // =====================================================================
    // The request shape, which is what the probes measured.
    // =====================================================================

    /// The measured line: `/works/<doi>` in the path, `mailto` in the query.
    ///
    /// Both halves are asserted because both are load-bearing and neither is
    /// derivable: a `?doi=` query route would 404, and a missing `mailto`
    /// silently drops the request out of the polite pool into a shared
    /// per-IP budget that answers 429 once spent.
    ///
    /// The path is asserted **percent-encoded**, because that is what goes on
    /// the wire: `Url::path` reports the encoded form, and the live API
    /// answers `200` to it (measured).
    #[test]
    fn the_request_shape_is_the_measured_one() {
        let adapter = CrossrefAdapter::new("polite@example.org");
        let url = adapter
            .works_url("10.1101/2025.06.14.659707")
            .expect("url builds");
        assert_eq!(url.host_str(), Some("api.crossref.org"));
        assert_eq!(url.path(), "/works/10.1101%2F2025.06.14.659707");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("mailto".into(), "polite@example.org".into())]
        );
    }

    /// The DOI is percent-encoded into its path segment, because a segmented
    /// identifier has a `/` inside the suffix and a `#` or `?` would otherwise
    /// truncate the path. #261 measured the family; this is the mechanical guard
    /// that keeps a permissive validator (#262) from producing a wrong URL.
    #[test]
    fn a_segmented_identifier_is_encoded_into_one_path_segment() {
        let adapter = CrossrefAdapter::new("polite@example.org");
        let url = adapter
            .works_url("10.26434/chemrxiv.15000484/v1")
            .expect("url builds");
        assert_eq!(
            url.path(),
            "/works/10.26434%2Fchemrxiv.15000484%2Fv1",
            "the suffix's own slash must not become a new path segment"
        );
        // Decoded back to exactly what was asked for, which is what the live
        // API resolved as `200`.
        assert_eq!(
            url.path().to_string().replace("%2F", "/"),
            "/works/10.26434/chemrxiv.15000484/v1"
        );
    }

    /// A DOI given as a `doi.org` URL or with stray case is canonicalised
    /// first, so the same work does not produce two request lines — and two
    /// spends of the same bucket's budget.
    #[test]
    fn the_doi_is_canonicalised_before_it_reaches_the_wire() {
        let adapter = CrossrefAdapter::new("");
        let url = adapter
            .works_url("https://doi.org/10.1101/2025.06.14.659707")
            .expect("url builds");
        assert_eq!(url.path(), "/works/10.1101%2F2025.06.14.659707");
        assert!(
            url.query().is_none(),
            "no mailto configured means the parameter is omitted, not sent blank"
        );
    }

    // =====================================================================
    // Parsing a well-formed answer.
    // =====================================================================

    /// A real record, and the three facts the matcher consumes.
    ///
    /// The point of the fixture is that it is **not flattened**: `title` is an
    /// array, the year is a nested `date-parts` array, and the author is an
    /// object. A parser written against a simplified shape would pass a
    /// hand-written test and fail here.
    #[test]
    fn a_well_formed_record_parses_into_title_year_and_first_author() {
        let body: serde_json::Value = serde_json::from_str(BIO_RXIV_MESSAGE).expect("json");
        let CrossrefAnswer::Found(identity) = parse_work(&body) else {
            panic!(
                "a real Crossref work must parse, got {:?}",
                parse_work(&body)
            );
        };
        assert_eq!(
            identity.title.as_deref(),
            Some("Boltz-2: Towards Accurate and Efficient Binding Affinity Prediction")
        );
        assert_eq!(identity.year, Some(2025));
        assert_eq!(identity.first_author.as_deref(), Some("Passaro, Saro"));
        // And the surname the matcher will actually compare.
        assert_eq!(
            crate::identity::surname(identity.first_author.as_deref().unwrap_or("")).as_deref(),
            Some("passaro")
        );
    }

    /// The year is read from `published` first, and only falls through to
    /// `created` when there is nothing better.
    ///
    /// The second half is the substantive claim: `created` is a *registration*
    /// date, so a 2022 preprint deposited into a 2026 Crossref must corroborate
    /// as 2022. Reading `created` first would put a correct preprint four years
    /// out — and a year that far apart is a **contradiction**, which is the one
    /// verdict that blocks filing.
    #[test]
    fn the_year_prefers_published_over_the_registration_date() {
        let body = serde_json::json!({
            "message": {
                "title": ["A preprint"],
                "published": {"date-parts": [[2022, 3, 14]]},
                "created":   {"date-parts": [[2026, 10, 5]]},
                "author": [{"family": "Young"}]
            }
        });
        let CrossrefAnswer::Found(identity) = parse_work(&body) else {
            panic!("parses");
        };
        assert_eq!(
            identity.year,
            Some(2022),
            "the publication year, not the year the record was deposited"
        );
    }

    /// A record with no `published` and no `issued` still has a year, and the
    /// ladder is better served by `created` than by nothing — with the title
    /// present, a year is one of the two corroborators.
    #[test]
    fn the_year_falls_through_to_created_when_nothing_earlier_exists() {
        let body = serde_json::json!({
            "message": {"title": ["A thing"], "created": {"date-parts": [[2019]]}}
        });
        let CrossrefAnswer::Found(identity) = parse_work(&body) else {
            panic!("parses");
        };
        assert_eq!(identity.year, Some(2019));
    }

    /// A date we cannot read is `None`, not a guess.
    ///
    /// `date-parts: [[null]]` and `date-parts: []` both occur in real records,
    /// and both are the honest "no year" — which the matcher turns into
    /// `unverified` rather than into agreement.
    #[test]
    fn an_unreadable_date_is_no_year() {
        for date in [
            serde_json::json!({"date-parts": [[null]]}),
            serde_json::json!({"date-parts": []}),
            serde_json::json!({"date-parts": [[]]}),
            serde_json::json!({"date-parts": "2025"}),
            serde_json::json!({}),
        ] {
            let body = serde_json::json!({ "message": { "title": ["T"], "published": date } });
            let CrossrefAnswer::Found(identity) = parse_work(&body) else {
                panic!("parses");
            };
            assert_eq!(identity.year, None, "{date} must not yield a year");
        }
    }

    // =====================================================================
    // The 404 trap: a miss must never read as metadata.
    // =====================================================================

    /// The reason this slice exists, as an executable claim.
    ///
    /// A 404 whose body is a JSON error document is the trap: a parser that
    /// runs before it looks at the status sees valid JSON, finds no `message`
    /// key, and — if it defaults rather than fails — hands the matcher a
    /// `WorkIdentity` of three `None`s. That is not "not registered", it is
    /// "a work with no title, no year and no author", and the matcher's honest
    /// answer to *that* is `unverified`. So a DOI Crossref has never heard of
    /// would read as a work whose metadata we could not read, and the chain
    /// would stop on it instead of asking DataCite.
    ///
    /// Asserted both ways: the live route yields [`CrossrefAnswer::NotRegistered`],
    /// and the parser independently refuses the same body — so neither the
    /// status check nor the envelope check is load-bearing alone.
    #[tokio::test]
    async fn a_crossref_404_body_is_not_read_as_metadata() {
        let server = MockServer::start().await;
        // The trap's exact shape: a **valid JSON** body on a 404.
        Mock::given(method("GET"))
            .and(path("/works/10.18434%2Fm32154"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"status":"failed","message-type":"work"}"#),
            )
            .mount(&server)
            .await;

        let answer = adapter_for(&server)
            .lookup(
                &client(),
                &WorkScope::new(),
                "10.18434/m32154", // a DOI Crossref does not register
            )
            .await
            .expect("a 404 is an answer, not a failure");

        assert_eq!(
            answer,
            CrossrefAnswer::NotRegistered,
            "a 404 carrying a JSON error document is \"not registered here\", \
             not a record with null fields"
        );

        // The second, independent gate: handed that exact body, the parser
        // still refuses to invent a record.
        let body: serde_json::Value =
            serde_json::from_str(r#"{"status":"failed","message-type":"work"}"#).expect("json");
        assert!(matches!(parse_work(&body), CrossrefAnswer::Unreadable(_)));

        // And what a lenient parser *would* have produced, held up as the thing
        // being prevented: three `None`s, which the matcher answers
        // `unverified` for, which is a different and much less useful claim.
        let lenient = WorkIdentity {
            title: None,
            year: None,
            first_author: None,
        };
        assert_eq!(
            crate::identity::verify(&WorkIdentity::titled("NIST SRD 46"), &lenient).verdict,
            crate::identity::Verdict::Unverified,
            "the fabricated record would have read as `unverified` instead of \
             \"this registry has never heard of it\""
        );
    }

    /// Crossref's own measured miss is `text/plain` — 19 bytes, no JSON at
    /// all. Pinned because it is the reason the status check is the primary
    /// gate: the parse would fail here by luck, and the design must not depend
    /// on the luck.
    #[tokio::test]
    async fn the_measured_plain_text_miss_is_also_not_registered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works/10.5281%2Fzenodo.23162961"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("Resource not found.\n"),
            )
            .mount(&server)
            .await;

        assert_eq!(
            adapter_for(&server)
                .lookup(&client(), &WorkScope::new(), "10.5281/zenodo.23162961",)
                .await
                .expect("answer"),
            CrossrefAnswer::NotRegistered
        );
    }

    // =====================================================================
    // Malformed answers: `unverified`, never a panic and never a record.
    // =====================================================================

    /// A 200 that is not a Crossref work is [`CrossrefAnswer::Unreadable`], and
    /// the chain reads that as "we could not find out" — which becomes
    /// `unverified`, not `NotRegistered` and not a fabricated identity.
    ///
    /// Both requirements at once, because they are different: *no panic* is the
    /// obvious one, and *no fabricated record* is the one that actually matters.
    #[tokio::test]
    async fn a_malformed_answer_is_unreadable_rather_than_a_fabricated_record() {
        let server = MockServer::start().await;
        for (label, body) in [
            (
                "not json at all",
                "<!doctype html><html>502 Bad Gateway</html>",
            ),
            ("an empty object", "{}"),
            ("an error envelope", r#"{"status":"failed"}"#),
            (
                "a message that is not an object",
                r#"{"message":"Resource not found."}"#,
            ),
            ("a null message", r#"{"message":null}"#),
        ] {
            Mock::given(method("GET"))
                .and(path("/works/10.1038%2Fx"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "application/json")
                        .set_body_string(body),
                )
                .mount(&server)
                .await;

            let answer = adapter_for(&server)
                .lookup(&client(), &WorkScope::new(), "10.1038/x")
                .await
                .expect("a 200 is never a request failure");
            assert!(
                matches!(answer, CrossrefAnswer::Unreadable(_)),
                "{label}: a 200 that is not a work must be unreadable, got {answer:?}"
            );
            assert!(
                !matches!(answer, CrossrefAnswer::Found(_)),
                "{label}: nothing may be fabricated from it"
            );
        }
    }

    /// A transport failure and a 5xx are both [`CrossrefAnswer::Unreadable`]:
    /// we did not learn whether Crossref registers the DOI, and reporting
    /// "not registered" would make a transient error end the chain.
    #[tokio::test]
    async fn a_server_error_is_unreadable_not_not_registered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works/10.1038%2Fy"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .mount(&server)
            .await;

        assert!(matches!(
            adapter_for(&server)
                .lookup(&client(), &WorkScope::new(), "10.1038/y")
                .await
                .expect("a 5xx is still an answer"),
            CrossrefAnswer::Unreadable(_)
        ));
    }

    /// A DOI the validator rejects never reaches the wire — same gate as
    /// [`crate::openalex`], and the reason a permissive suffix rule (#262)
    /// cannot turn into an arbitrary request.
    #[tokio::test]
    async fn an_invalid_doi_is_refused_before_any_request() {
        let server = MockServer::start().await;
        let answer = adapter_for(&server)
            .lookup(&client(), &WorkScope::new(), "not a doi")
            .await;
        assert!(
            matches!(answer, Err(AdapterError::Validation(_))),
            "{answer:?}"
        );
        assert_eq!(
            server.received_requests().await.expect("recorded").len(),
            0,
            "nothing was put on the wire"
        );
    }

    /// The happy path over a real `wiremock` server, so the URL the adapter
    /// builds and the request the client makes are known to agree.
    #[tokio::test]
    async fn a_hit_over_the_wire_parses_into_the_three_facts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works/10.1101%2F2025.06.14.659707"))
            .and(wiremock::matchers::query_param(
                "mailto",
                "polite@example.org",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(BIO_RXIV_MESSAGE),
            )
            .mount(&server)
            .await;

        let CrossrefAnswer::Found(identity) = adapter_for(&server)
            .lookup(&client(), &WorkScope::new(), "10.1101/2025.06.14.659707")
            .await
            .expect("answer")
        else {
            panic!("a served record must be found");
        };
        assert_eq!(identity.year, Some(2025));
        assert_eq!(identity.first_author.as_deref(), Some("Passaro, Saro"));

        let requests = server.received_requests().await.expect("recorded");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].url.path(),
            "/works/10.1101%2F2025.06.14.659707",
            "the measured path, with the DOI percent-encoded into one path segment \
             and not in the query"
        );
    }

    /// The one `Request` permit per hop, asserted through the pacer rather than
    /// through the request count — this is what ADR-007 §4 is about, and a
    /// chain that consulted three registries would be three permits against
    /// three separate buckets, which is correct, but each hop must still be
    /// one.
    #[tokio::test]
    async fn a_lookup_charges_the_meta_tier_of_the_crossref_bucket() {
        #[derive(Debug, Default)]
        struct RecordingPacer(std::sync::Mutex<Vec<(String, PaceTier)>>);

        #[async_trait::async_trait]
        impl Pacer for RecordingPacer {
            async fn acquire(
                &self,
                bucket: &Bucket,
                tier: PaceTier,
                cost: Cost,
            ) -> Result<Permit, PaceDenied> {
                if cost == Cost::Request {
                    self.0.lock().expect("lock").push((bucket.0.clone(), tier));
                }
                Ok(Permit {
                    bucket: bucket.clone(),
                    tier,
                    not_before: std::time::Instant::now(),
                })
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/works/10.1038%2Fz"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Resource not found.\n"))
            .mount(&server)
            .await;

        let pacer = Arc::new(RecordingPacer::default());
        let client = PacedClient::new(
            PacedClient::default_transport().expect("transport"),
            Arc::clone(&pacer) as Arc<dyn Pacer>,
            routing_table(&server, "crossref"),
        );
        adapter_for(&server)
            .lookup(&client, &WorkScope::new(), "10.1038/z")
            .await
            .expect("answer");

        let charged = pacer.0.lock().expect("lock").clone();
        assert_eq!(
            charged,
            vec![("crossref".to_string(), PaceTier::Meta)],
            "one `Request` permit, at the `Meta` tier, out of the `crossref` \
             bucket — the host the policy table routes api.crossref.org to"
        );
    }
}
