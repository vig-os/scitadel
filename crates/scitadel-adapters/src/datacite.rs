//! DataCite, as a source of identity: `GET /dois/<doi>` (ADR-007 §3's
//! pre-fetch chain, third hop).
//!
//! # Why this is its own adapter
//!
//! DataCite is a **different registry** from Crossref, not a second endpoint on
//! the same one, and the coverage split is not a rounding error. Probed 2026-10
//! against the live APIs:
//!
//! | DOI | Crossref | DataCite |
//! |---|---|---|
//! | `10.18434/m32154` (OSTI/NIST — one of #260's four) | 404 | **200** |
//! | `10.5281/zenodo.23162961` (Zenodo) | 404 | **200** |
//! | `10.1101/2025.06.14.659707` (bioRxiv — one of #260's four) | **200** | 404 |
//! | `10.26434/chemrxiv.15000484/v1` (ChemRxiv) | **200** | 404 |
//!
//! So of #260's four measured misses, **half are DataCite's alone**, and a
//! chain that stopped at Crossref would corroborate none of them. That is the
//! whole reason this module exists, and it is why the two adapters are separate
//! types rather than one parameterised call: a shared `fn lookup(registry: …)`
//! would have made the coverage difference a runtime flag instead of a
//! compile-time fact.
//!
//! ## A correction to the premise, because it is load-bearing
//!
//! The slice brief asserted that `10.1101` and `10.26434` are DataCite
//! prefixes. They are not, and the measurement is unambiguous: all three of
//! #260's bioRxiv/medRxiv DOIs answer `200` from Crossref and `404` from
//! DataCite, and `10.26434` is registered to the American Chemical Society as a
//! Crossref member with 56 845 `posted-content` records under it. The
//! conclusion this module supports is unchanged — both registries are needed and
//! a preprint identity cannot be corroborated by Crossref *alone* — but the
//! reason is that DataCite holds the repository DOIs, not that it holds the
//! preprint-server ones.
//!
//! # The measured request shape
//!
//! ```text
//! GET https://api.datacite.org/dois/<doi>
//!     ?mailto=<polite-pool address>
//! ```
//!
//! Measured 2026-10 against the live API:
//!
//! - the DOI goes in the **path**, percent-encoded by
//!   [`Url::path_segments_mut`] for the same reason as in [`crate::crossref`]:
//!   a segmented identifier has a `/` inside its suffix, and the encoding is
//!   what stops a `#` or `?` truncating the path.
//! - `mailto` qualifies the request for the **identified** tier, which is the
//!   tier with the larger allowance. The response confirms it arrived
//!   identified-capable: `x-anonymous-consumer: true` on the probe means the
//!   *tier* was granted; DataCite publishes no rate-limit headers on this
//!   route, so `policy.rs`'s `datacite` row is sourced from DataCite's
//!   published figure rather than from an observed header, and says so.
//!
//! # What a miss looks like — and this is the trap, on this registry
//!
//! ```text
//! HTTP/2 404   content-type: application/json; charset=utf-8   87 bytes
//! {"errors":[{"status":"404","title":"The resource you are looking for doesn't exist."}]}
//! ```
//!
//! **A 404 carrying a JSON error object.** That is the shape the slice brief
//! warned about, and it is the dangerous one: `serde_json` parses it
//! successfully, so a parser that runs before it looks at the status gets valid
//! JSON, finds no `data` key, and — if it defaults rather than fails — hands the
//! matcher a `WorkIdentity` of three `None`s. The matcher answers that
//! `unverified`, which would make every unregistered DOI read as "a work whose
//! metadata we could not read" and would end the chain one hop early, on
//! exactly the preprints the chain exists to corroborate.
//!
//! So "not registered" is decided by the **status code and nothing else**.
//! [`PacedClient`] refuses any non-2xx before a body can be read, so this body
//! cannot reach [`parse_doi`]; and [`parse_doi`] independently requires the
//! `data.attributes` envelope, so a 200 carrying `{"errors": […]}` yields no
//! record either. Two independent gates.
//!
//! # The field asymmetry is not smoothed over
//!
//! The two registries' records describe the same three facts and share almost
//! no field names or shapes. Both are read where they actually are, and neither
//! is normalised into a fictional common shape first:
//!
//! | fact | Crossref | DataCite |
//! |---|---|---|
//! | title | `title`: **array** of strings | `titles`: **array** of `{title}` objects |
//! | year | `published.date-parts[0][0]` — a **nested** array, no scalar | `publicationYear` — a **top-level scalar** |
//! | first author | `author[0]`: an **object** `{family, given}` | `creators[0]`: a **string** `name`, plus optional `familyName`/`givenName` |
//! | no author at all | `author` absent or `[]` | `creators` absent, `[]`, or a `nameType: Organizational` entry with no person |
//!
//! DataCite also models **non-person** creators and datasets/software, which
//! Crossref's journal-article model does not; and its `creators` can be an empty
//! array on a record that certainly has a creator somewhere upstream. Both
//! cases read as "no author on record" — `unverified`, never agreement.

use reqwest::Url;
use scitadel_core::models::{normalize_doi, validate_doi};
use scitadel_core::ports::PaceTier;
use scitadel_http::{PacedClient, PacedResponse, SafeHeaders, WorkScope};

use crate::error::AdapterError;
use crate::identity::WorkIdentity;

/// Base of the single-DOI route. A value, not a `const` spliced at the call
/// site, so a test can point one adapter at one `wiremock` server.
pub const DATACITE_DOIS_URL: &str = "https://api.datacite.org/dois";

/// What one DataCite lookup established.
#[derive(Debug, Clone, PartialEq)]
pub enum DataCiteAnswer {
    /// DataCite carries this DOI. The three facts [`crate::identity::verify`]
    /// consumes, however many of them the record actually has.
    Found(WorkIdentity),
    /// DataCite answered **404**: this DOI is not in DataCite's registry.
    ///
    /// The common answer, and the one the chain is built around: every
    /// bioRxiv/medRxiv and ChemRxiv DOI in the measured set is a `404` here.
    /// A fact about coverage, not a failure.
    NotRegistered,
    /// A 200 we could not read. We do not know whether the DOI is registered.
    Unreadable(String),
}

/// A DataCite client that spends [`PaceTier::Meta`] permits and carries no
/// credential.
///
/// Follows [`crate::openalex`]'s shape and [`crate::crossref`]'s answer type, so
/// the chain holds one hop shape and reads the two registries' asymmetry off
/// the parsers rather than off a flag.
#[derive(Debug, Clone)]
pub struct DataCiteAdapter {
    base_url: String,
    /// The identified-tier `mailto`. Empty omits the parameter rather than
    /// sending it blank.
    mailto: String,
}

impl DataCiteAdapter {
    /// The production adapter.
    #[must_use]
    pub fn new(mailto: impl Into<String>) -> Self {
        Self {
            base_url: DATACITE_DOIS_URL.to_string(),
            mailto: mailto.into(),
        }
    }

    /// Point the adapter at a different `/dois` endpoint. Test seam; the
    /// production default is [`DATACITE_DOIS_URL`].
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The lookup URL for `doi`, percent-encoded and identified-tier-tagged.
    ///
    /// # Errors
    ///
    /// Only if the base URL is unparseable or cannot be a base — which for
    /// [`DATACITE_DOIS_URL`] cannot happen.
    pub fn dois_url(&self, doi: &str) -> Result<Url, AdapterError> {
        let mut url = Url::parse(&self.base_url)
            .map_err(|e| AdapterError::Validation(format!("invalid DataCite base URL: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| AdapterError::Validation("DataCite base URL cannot be a base".into()))?
            .extend([normalize_doi(doi)]);
        if !self.mailto.is_empty() {
            url.query_pairs_mut().append_pair("mailto", &self.mailto);
        }
        Ok(url)
    }

    /// Resolve `doi` against DataCite, spending one `Request` permit.
    ///
    /// `work` is the walk's [`WorkScope`], so this hop shares the per-work
    /// `Work` permit accounting with the rest of the walk (ADR-007 §4) while
    /// spending its own bucket's `Request` permit — DataCite is a different
    /// platform from Crossref, and one chain leg must not spend another's
    /// budget.
    ///
    /// # Errors
    ///
    /// Only for a DOI the validator rejects, or a base URL that will not parse.
    /// A 404 is [`DataCiteAnswer::NotRegistered`].
    pub async fn lookup(
        &self,
        client: &PacedClient,
        work: &WorkScope,
        doi: &str,
    ) -> Result<DataCiteAnswer, AdapterError> {
        if !validate_doi(doi) {
            return Err(AdapterError::Validation(format!("invalid DOI: {doi}")));
        }
        let url = self.dois_url(doi)?;
        let response = match client
            .get_in_work(work, url, PaceTier::Meta, SafeHeaders::unauthenticated())
            .await
        {
            Ok(response) => response,
            Err(error) => return Ok(classify(error, doi)),
        };
        // `text()` then `from_str`, the idiom [`crate::download`]'s OpenAlex
        // leg already uses. See [`read_doi`] for why an unreadable body is an
        // answer rather than an error.
        Ok(read_doi(response).await)
    }
}

/// The answer a 2xx body carries, or the reason it carries none.
///
/// Split out so the three ways a 200 can be unreadable — the body would not
/// read, the body is not JSON, the JSON is not a record — are one expression
/// rather than three `?`s in the request path.
async fn read_doi(response: PacedResponse) -> DataCiteAnswer {
    let text = match response.text().await {
        Ok(text) => text,
        Err(e) => return DataCiteAnswer::Unreadable(format!("could not read the body: {e}")),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(body) => parse_doi(&body),
        Err(e) => DataCiteAnswer::Unreadable(format!("JSON parse failed: {e}")),
    }
}

/// Turn a failed hop into the answer it establishes.
///
/// A **404 is "not registered"**, and it is decided on the status code alone.
/// `PacedClient` has already refused to hand back a body for a non-2xx, so the
/// JSON error document DataCite puts on its 404 is never parsed here — which is
/// the only reason the 87-byte body in the module docs cannot be mistaken for a
/// record.
///
/// Everything else is [`DataCiteAnswer::Unreadable`]: a 5xx, a transport
/// failure, a pacing refusal. All three are "we could not find out".
fn classify(error: scitadel_http::FetchError, doi: &str) -> DataCiteAnswer {
    match error {
        scitadel_http::FetchError::Status { code: 404, .. } => {
            tracing::info!(doi = %doi, "datacite does not register this DOI");
            DataCiteAnswer::NotRegistered
        }
        other => DataCiteAnswer::Unreadable(other.to_string()),
    }
}

/// The three facts the matcher consumes, out of a DataCite `data.attributes`
/// envelope.
///
/// `None` — [`DataCiteAnswer::Unreadable`] — when the body is not that
/// envelope, and this is the gate that matters on this registry. DataCite's 404
/// **is** valid JSON, so if a body ever reached this function carrying
/// `{"errors": […]}` and the function defaulted rather than refusing, the
/// result would be a `WorkIdentity` of three `None`s: a fabricated record, and
/// `unverified` at the matcher where the truth is "not registered here".
///
/// A missing *field* inside a real envelope is `None` rather than a
/// placeholder, for the reason [`crate::identity`] states throughout: an absent
/// fact must read as `unverified`, never as agreement.
#[must_use]
pub fn parse_doi(body: &serde_json::Value) -> DataCiteAnswer {
    let Some(attributes) = body.pointer("/data/attributes").filter(|a| a.is_object()) else {
        tracing::info!("datacite answered 200 without a `data.attributes` envelope");
        return DataCiteAnswer::Unreadable("no `data.attributes` envelope in the response".into());
    };
    DataCiteAnswer::Found(WorkIdentity {
        // An array of **objects**, `{"title": "…"}` — not of strings, which is
        // Crossref's shape. `title` singular appears on some records instead;
        // read but not relied on, because the documented shape is the array.
        title: attributes
            .get("titles")
            .and_then(serde_json::Value::as_array)
            .and_then(|titles| titles.first())
            .and_then(|first| first.get("title"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                attributes
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
        // A **top-level scalar**, where Crossref's is a nested array under
        // `published`. There is no `date-parts` on a DataCite record at all.
        year: attributes
            .get("publicationYear")
            .and_then(serde_json::Value::as_i64)
            .and_then(|year| i32::try_from(year).ok()),
        first_author: attributes
            .get("creators")
            .and_then(serde_json::Value::as_array)
            .and_then(|creators| creators.first())
            .and_then(datacite_creator_name),
    })
}

/// One DataCite creator's name, in the `Family, Given` order
/// [`crate::identity::surname`] reads.
///
/// `familyName` is preferred and is the same field the measured OSTI record
/// carries; `name` is the fallback and is already in `Family, Given` form
/// (`"Burgess, Donald R."`). A creator with neither is not a person — an
/// `nameType: Organizational` entry names an institution, and
/// [`crate::identity::surname`] would take its last whitespace token as a family
/// name and compare *"National Institute of Standards and Technology"* against
/// a person's surname. So the absence is reported honestly instead.
fn datacite_creator_name(creator: &serde_json::Value) -> Option<String> {
    let family = creator
        .get("familyName")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|family| !family.is_empty());
    let given = creator
        .get("givenName")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|given| !given.is_empty());
    let name = creator
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(family) = family {
        return Some(match given {
            Some(given) => format!("{family}, {given}"),
            None => family.to_string(),
        });
    }
    // `name` alone is only usable when it carries a comma — that is what makes
    // it "Family, Given" rather than an institution's name. See the doc comment.
    name.filter(|name| name.contains(',')).map(str::to_string)
}

// ============================================================================
// ADR-007 §3's resolve-then-rank: the fields the identity pass never read.
// ============================================================================

/// What DataCite says the record is, and which of its two type fields said so.
///
/// DataCite carries **two**: `types.resourceTypeGeneral` (a controlled
/// vocabulary — `Text`, `Preprint`, `Dataset`) and `types.resourceType` (the
/// submitter's own finer word — `JournalArticle`, `Book`, `Preprint`). The
/// controlled one is read first because it is the one DataCite validates, and
/// the finer one second because it is the one a repository actually types.
///
/// Which field produced the answer is carried in the return value rather than
/// discarded, because the ranker records the signal it used: a record typed
/// `Preprint` in the controlled vocabulary is a stronger statement than one
/// where the submitter happened to write "preprint" in a free-text field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataCiteType {
    pub classified: crate::registry::RegistryType,
    /// The raw word, verbatim.
    pub word: String,
    /// Which field it came from: `resourceTypeGeneral` or `resourceType`.
    pub field: &'static str,
}

/// Read `data.attributes.types`, or `None` when neither field is usable.
///
/// `None` is not "a dataset" and not "an article": it is this record carrying
/// no type we can act on, which is what makes the ranker fall back and say so.
#[must_use]
pub fn type_signal(body: &serde_json::Value) -> Option<DataCiteType> {
    let attributes = body.pointer("/data/attributes")?;
    let types = attributes.get("types")?;
    for field in ["resourceTypeGeneral", "resourceType"] {
        if let Some(word) = types
            .get(field)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|word| !word.is_empty())
        {
            return Some(DataCiteType {
                classified: crate::registry::RegistryType::of(
                    word,
                    crate::registry::Dialect::DataCite,
                ),
                word: word.to_string(),
                field,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::ports::{Bucket, Cost, PaceDenied, Pacer, Permit};
    use std::sync::Arc;
    use wiremock::matchers::{method, path, query_param};
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

    /// The measured DataCite envelope for `10.18434/m32154`, reduced to the
    /// fields this module reads. The nesting is the point: `data.attributes`,
    /// a scalar `publicationYear`, `titles[0].title`, and a creator carrying
    /// both a `name` string and the split `familyName`/`givenName`.
    const OSTI_DOI: &str = r#"{
      "data": {
        "id": "10.18434/m32154",
        "type": "dois",
        "attributes": {
          "doi": "10.18434/m32154",
          "prefix": "10.18434",
          "suffix": "m32154",
          "types": {
            "ris": "DATA", "bibtex": "misc", "citeproc": "dataset",
            "schemaOrg": "Dataset", "resourceType": "Dataset",
            "resourceTypeGeneral": "Dataset"
          },
          "titles": [
            {"title": "NIST SRD 46. Critically Selected Stability Constants of Metal Complexes: Version 8.0 for Windows"}
          ],
          "publicationYear": 2020,
          "creators": [
            {
              "name": "Burgess, Donald R.",
              "nameType": "Personal",
              "givenName": "Donald R.",
              "familyName": "Burgess",
              "affiliation": ["National Institute of Standards and Technology"]
            }
          ],
          "publisher": "National Institute for Standards and Technology",
          "dates": [
            {"date": "2004-05-01", "dateType": "Updated"},
            {"date": "2020", "dateType": "Issued"}
          ],
          "container": {},
          "relatedIdentifiers": []
        }
      }
    }"#;

    /// DataCite's measured 404 body, byte for byte.
    const MEASURED_404_BODY: &str = r#"{"errors":[{"status":"404","title":"The resource you are looking for doesn't exist."}]}"#;

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

    fn adapter_for(server: &MockServer) -> DataCiteAdapter {
        DataCiteAdapter::new("polite@example.org").with_base_url(format!("{}/dois", server.uri()))
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

    /// The measured line: `/dois/<doi>` in the path, `mailto` in the query.
    #[test]
    fn the_request_shape_is_the_measured_one() {
        let adapter = DataCiteAdapter::new("polite@example.org");
        let url = adapter.dois_url("10.18434/m32154").expect("url builds");
        assert_eq!(url.host_str(), Some("api.datacite.org"));
        assert_eq!(url.path(), "/dois/10.18434%2Fm32154");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![("mailto".into(), "polite@example.org".into())]
        );
    }

    /// A DOI given as a `doi.org` URL is canonicalised first, so the same work
    /// cannot produce two request lines against the same bucket.
    #[test]
    fn the_doi_is_canonicalised_before_it_reaches_the_wire() {
        let url = DataCiteAdapter::new("")
            .dois_url("https://doi.org/10.5281/zenodo.23162961")
            .expect("url builds");
        assert_eq!(url.path(), "/dois/10.5281%2Fzenodo.23162961");
        assert!(url.query().is_none(), "a blank mailto is omitted");
    }

    // =====================================================================
    // Parsing a well-formed answer.
    // =====================================================================

    /// A real record, and the three facts the matcher consumes — read from
    /// where DataCite actually keeps them, which is nowhere near where Crossref
    /// keeps its equivalents.
    #[test]
    fn a_well_formed_record_parses_into_title_year_and_first_author() {
        let body: serde_json::Value = serde_json::from_str(OSTI_DOI).expect("json");
        let DataCiteAnswer::Found(identity) = parse_doi(&body) else {
            panic!(
                "a real DataCite record must parse, got {:?}",
                parse_doi(&body)
            );
        };
        assert_eq!(
            identity.title.as_deref(),
            Some(
                "NIST SRD 46. Critically Selected Stability Constants of Metal Complexes: Version 8.0 for Windows"
            )
        );
        assert_eq!(identity.year, Some(2020));
        assert_eq!(identity.first_author.as_deref(), Some("Burgess, Donald R."));
        assert_eq!(
            crate::identity::surname(identity.first_author.as_deref().unwrap_or("")).as_deref(),
            Some("burgess")
        );
    }

    /// The asymmetry, pinned as a test rather than left to a comment: a
    /// **Crossref-shaped** body is not a DataCite record, and a
    /// **DataCite-shaped** body is not a Crossref one.
    ///
    /// This is what stops a "they are parallel APIs" reading from quietly
    /// working: it does not, and the two parsers disagree about which fields
    /// exist. If either registry changed shape, this fails here rather than as a
    /// corpus of `unverified`.
    #[test]
    fn the_two_registries_shapes_are_not_interchangeable() {
        let crossref_shaped = serde_json::json!({
            "message": {"title": ["A title"], "author": [{"family": "Passaro", "given": "Saro"}]}
        });
        assert!(
            matches!(parse_doi(&crossref_shaped), DataCiteAnswer::Unreadable(_)),
            "a Crossref `message` envelope is not a DataCite record"
        );

        let datacite_shaped: serde_json::Value = serde_json::from_str(OSTI_DOI).expect("json");
        assert!(
            matches!(
                crate::crossref::parse_work(&datacite_shaped),
                crate::crossref::CrossrefAnswer::Unreadable(_)
            ),
            "a DataCite `data.attributes` envelope is not a Crossref work"
        );
    }

    /// A record whose only creator is an **organisation** has no first author,
    /// and reporting one would be worse than reporting none: `surname` takes the
    /// last whitespace token, so "National Institute of Standards and
    /// Technology" would be compared as a surname and could contradict a real
    /// author — turning a correct record into a `mismatch`.
    #[test]
    fn an_organisational_creator_is_no_author_rather_than_a_wrong_one() {
        for creator in [
            serde_json::json!({
                "name": "National Institute of Standards and Technology",
                "nameType": "Organizational"
            }),
            // A `name` with no comma is an institution, not a "Family, Given".
            serde_json::json!({"name": "CERN", "nameType": "Organizational"}),
            // Nothing usable at all.
            serde_json::json!({"nameIdentifiers": []}),
        ] {
            let body = serde_json::json!({
                "data": {"attributes": {"titles": [{"title": "T"}], "creators": [creator.clone()]}}
            });
            let DataCiteAnswer::Found(identity) = parse_doi(&body) else {
                panic!("parses");
            };
            assert_eq!(
                identity.first_author, None,
                "{creator} must not become a surname"
            );
        }
    }

    /// An absent, empty or non-person `creators` is the same honest `None`, and
    /// the year is a scalar here — there is no `date-parts` to look for.
    #[test]
    fn a_record_with_no_creators_parses_with_a_title_and_a_year() {
        for creators in [Some(serde_json::json!([])), None] {
            let mut attributes = serde_json::json!({
                "titles": [{"title": "A dataset"}],
                "publicationYear": 2019
            });
            if let Some(creators) = creators {
                attributes["creators"] = creators;
            }
            let body = serde_json::json!({ "data": { "attributes": attributes } });
            let DataCiteAnswer::Found(identity) = parse_doi(&body) else {
                panic!("parses");
            };
            assert_eq!(identity.title.as_deref(), Some("A dataset"));
            assert_eq!(identity.year, Some(2019));
            assert_eq!(identity.first_author, None);
        }
    }

    /// A `publicationYear` that is not an integer is `None`, not a guess. A
    /// string or array year would be a *number that reads like a measurement*
    /// and could corroborate — or contradict — a real one.
    #[test]
    fn an_unusable_publication_year_is_no_year() {
        for year in [
            serde_json::json!(null),
            serde_json::json!("2020"),
            serde_json::json!([2020]),
            serde_json::json!(2020.5),
        ] {
            let body = serde_json::json!({
                "data": {"attributes": {"titles": [{"title": "T"}], "publicationYear": year}}
            });
            let DataCiteAnswer::Found(identity) = parse_doi(&body) else {
                panic!("parses");
            };
            assert_eq!(identity.year, None, "{year} must not yield a year");
        }
    }

    // =====================================================================
    // The 404 trap: a JSON error document must never read as metadata.
    // =====================================================================

    /// The reason this adapter is not a parameter of the Crossref one, as an
    /// executable claim: DataCite's 404 is **valid JSON**, so the trap is real
    /// here and hypothetical there.
    ///
    /// The live route yields [`DataCiteAnswer::NotRegistered`], and the parser
    /// independently refuses the same body — so the status check and the
    /// envelope check are two gates, neither load-bearing alone.
    #[tokio::test]
    async fn a_datacite_404_json_error_body_is_not_read_as_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/dois/10.1101%2F2025.06.14.659707"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "application/json; charset=utf-8")
                    .set_body_string(MEASURED_404_BODY),
            )
            .mount(&server)
            .await;

        let answer = adapter_for(&server)
            .lookup(
                &client(),
                &WorkScope::new(),
                "10.1101/2025.06.14.659707", // a DOI DataCite does not register
            )
            .await
            .expect("a 404 is an answer, not a failure");

        assert_eq!(
            answer,
            DataCiteAnswer::NotRegistered,
            "a 404 carrying a JSON error document is \"not registered here\""
        );

        // The body parses as JSON — that is the trap, and it is why the
        // envelope check is not optional.
        let body: serde_json::Value = serde_json::from_str(MEASURED_404_BODY).expect("it is JSON");
        assert!(matches!(parse_doi(&body), DataCiteAnswer::Unreadable(_)));

        // And the fabricated record a defaulting parser would produce, held up
        // as the thing being prevented.
        let lenient = WorkIdentity {
            title: None,
            year: None,
            first_author: None,
        };
        assert_eq!(
            crate::identity::verify(&WorkIdentity::titled("Boltz-2"), &lenient).verdict,
            crate::identity::Verdict::Unverified,
            "the fabricated record would have read as `unverified` and stopped the chain"
        );
    }

    /// A 200 that is not a DataCite record is [`DataCiteAnswer::Unreadable`]:
    /// no panic, and above all no fabricated identity.
    #[tokio::test]
    async fn a_malformed_answer_is_unreadable_rather_than_a_fabricated_record() {
        let server = MockServer::start().await;
        for (label, body) in [
            (
                "not json at all",
                "<!doctype html><html>502 Bad Gateway</html>",
            ),
            ("an empty object", "{}"),
            ("the measured 404 body on a 200", MEASURED_404_BODY),
            (
                "data without attributes",
                r#"{"data":{"id":"10.1/x","type":"dois"}}"#,
            ),
            (
                "attributes that is not an object",
                r#"{"data":{"attributes":"nope"}}"#,
            ),
        ] {
            Mock::given(method("GET"))
                .and(path("/dois/10.5281%2Fx"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "application/json")
                        .set_body_string(body),
                )
                .mount(&server)
                .await;

            let answer = adapter_for(&server)
                .lookup(&client(), &WorkScope::new(), "10.5281/x")
                .await
                .expect("a 200 is never a request failure");
            assert!(
                matches!(answer, DataCiteAnswer::Unreadable(_)),
                "{label}: a 200 that is not a record must be unreadable, got {answer:?}"
            );
        }
    }

    /// A 5xx and a transport failure are both "we could not find out", never
    /// "not registered" — reporting the latter would end the chain on a
    /// transient error, which is the one thing a fall-through chain must not
    /// do.
    #[tokio::test]
    async fn a_server_error_is_unreadable_not_not_registered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/dois/10.5281%2Fy"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        assert!(matches!(
            adapter_for(&server)
                .lookup(&client(), &WorkScope::new(), "10.5281/y")
                .await
                .expect("a 5xx is still an answer"),
            DataCiteAnswer::Unreadable(_)
        ));
    }

    /// A DOI the validator rejects never reaches the wire.
    #[tokio::test]
    async fn an_invalid_doi_is_refused_before_any_request() {
        let server = MockServer::start().await;
        let answer = adapter_for(&server)
            .lookup(&client(), &WorkScope::new(), "nonsense")
            .await;
        assert!(
            matches!(answer, Err(AdapterError::Validation(_))),
            "{answer:?}"
        );
        assert_eq!(server.received_requests().await.expect("recorded").len(), 0);
    }

    /// The happy path over a real `wiremock` server.
    #[tokio::test]
    async fn a_hit_over_the_wire_parses_into_the_three_facts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/dois/10.18434%2Fm32154"))
            .and(query_param("mailto", "polite@example.org"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json; charset=utf-8")
                    .set_body_string(OSTI_DOI),
            )
            .mount(&server)
            .await;

        let DataCiteAnswer::Found(identity) = adapter_for(&server)
            .lookup(&client(), &WorkScope::new(), "10.18434/m32154")
            .await
            .expect("answer")
        else {
            panic!("a served record must be found");
        };
        assert_eq!(identity.year, Some(2020));
        assert_eq!(identity.first_author.as_deref(), Some("Burgess, Donald R."));

        let requests = server.received_requests().await.expect("recorded");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].url.path(),
            "/dois/10.18434%2Fm32154",
            "the measured path, percent-encoded into one path segment"
        );
    }

    /// One `Request` permit, at the `Meta` tier, out of the `datacite` bucket.
    ///
    /// The second half of the claim matters as much as the first: a chain that
    /// consulted Crossref and then DataCite spends **two different buckets**,
    /// because they are two different platforms with two different allowances.
    #[tokio::test]
    async fn a_lookup_charges_the_meta_tier_of_the_datacite_bucket() {
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
            .and(path("/dois/10.1101%2F2025.06.14.659707"))
            .respond_with(
                ResponseTemplate::new(404)
                    .insert_header("content-type", "application/json")
                    .set_body_string(MEASURED_404_BODY),
            )
            .mount(&server)
            .await;

        let pacer = Arc::new(RecordingPacer::default());
        let client = PacedClient::new(
            PacedClient::default_transport().expect("transport"),
            Arc::clone(&pacer) as Arc<dyn Pacer>,
            routing_table(&server, "datacite"),
        );
        adapter_for(&server)
            .lookup(&client, &WorkScope::new(), "10.1101/2025.06.14.659707")
            .await
            .expect("answer");

        assert_eq!(
            pacer.0.lock().expect("lock").clone(),
            vec![("datacite".to_string(), PaceTier::Meta)],
            "one `Request` permit, at the `Meta` tier, out of the `datacite` bucket"
        );
    }
}
