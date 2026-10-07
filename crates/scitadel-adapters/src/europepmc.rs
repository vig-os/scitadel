//! Europe PMC's REST search, as a source of **version and licence** (ADR-007
//! §3's resolve pass, fifth registry).
//!
//! # What this adapter is for
//!
//! #292 built the identity chain and #293 the ranker, and both work from the
//! three *registration* registries plus Unpaywall. Europe PMC is a fifth thing:
//! an aggregator that knows, per record, whether a work is open access, whether
//! the full text is in Europe PMC at all, and — the part nothing else has —
//! whether the copy Europe PMC holds is an **author manuscript**.
//!
//! # The measured request shape
//!
//! ```text
//! GET https://www.ebi.ac.uk/europepmc/webservices/rest/search
//!     ?query=DOI:"10.1038/s41586-020-2649-2"
//!     &resultType=core
//!     &format=json
//! ```
//!
//! `resultType=core` is **load-bearing**, not a preference. Measured 2026-10:
//! the default `lite` result carries no `fullTextUrlList`, no `license` and no
//! manuscript flag, so a `lite` request would answer every question this
//! adapter exists to ask with silence.
//!
//! # The manuscript flags, and why there are three of them
//!
//! A Europe PMC record carries three independent booleans, and they are not
//! synonyms (measured across a MED record, a PPR record and an author-manuscript
//! record):
//!
//! | field | what Europe PMC asserts |
//! |---|---|
//! | `authMan` | this record **is** an author manuscript |
//! | `epmcAuthMan` | Europe PMC holds an author manuscript of this work |
//! | `nihAuthMan` | the author manuscript is the NIH-submitted one |
//!
//! `epmcAuthMan` is the one that matters for a *location*: a `MED` record with
//! `authMan: N` and `epmcAuthMan: Y` is a journal article whose Europe PMC copy
//! is the manuscript. Treating the three as one boolean would either miss that
//! case or claim a manuscript for an article whose copy is the version of
//! record — and the second is the overclaim #261 is about.
//!
//! # What a record does **not** say, and what is not inferred from it
//!
//! ADR-007 §3 step 1: "`inEPMC = Y` alone means free to read, not a reuse
//! licence." So `inEPMC` earns a candidate nothing on its own; it is not a
//! licence and is not read as one. The `license` field, when present, is Europe
//! PMC's statement of the reuse terms, and is the only thing that can make a
//! candidate [`crate::resolve::LicenceStrength::OpenLicence`].
//!
//! And the absence of a manuscript flag is **not** a claim of a version of
//! record: a record with every flag `N` and no statement about which rendering
//! Europe PMC will serve yields [`crate::resolve::Version::Unstated`], which is
//! the ladder's bottom rung and not a guess. The one exception is a `PPR`
//! (preprint) record, which is a preprint by Europe PMC's own classification of
//! the record.

use reqwest::Url;
use scitadel_core::models::{normalize_doi, validate_doi};
use scitadel_core::ports::PaceTier;
use scitadel_http::{PacedClient, PacedResponse, SafeHeaders, WorkScope};

use crate::error::AdapterError;
use crate::identity::WorkIdentity;
use crate::registry::RepositoryVersion;

/// Base of the REST service, `/search` below it.
pub const EUROPE_PMC_REST_URL: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest";

/// One Europe PMC record's version and licence facts.
///
/// The three `*_auth_man` fields are kept as they arrive rather than collapsed
/// into one boolean: see the module docs for why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EuropePmcRecord {
    /// Europe PMC's own record id, e.g. `32939066` or `PPR110986`.
    pub id: String,
    /// The database the record came from: `MED`, `PPR`, `PMC`, `AGR`, …
    pub source: Option<String>,
    /// `pmcid`, when the record has one. The PMC OA dataset is keyed by this.
    pub pmcid: Option<String>,
    /// `isOpenAccess` — `Y`/`N`, which is Europe PMC's own statement that the
    /// work is open access.
    pub is_open_access: bool,
    /// `inEPMC` — the full text is in Europe PMC. **Free to read, not a reuse
    /// licence**: ADR-007 §3 step 1 says so explicitly.
    pub in_epmc: bool,
    /// `hasPDF`.
    pub has_pdf: bool,
    /// `license`, Europe PMC's own spelling: `cc by`, `cc by-nc-nd`, `cc0`.
    /// A **short code**, not a URL — the mapping to a Creative Commons URL is
    /// [`crate::registry`]'s, because the allow-list is one list.
    pub licence: Option<String>,
    /// Which rendering the record describes. See [`RepositoryVersion`].
    pub version: RepositoryVersion,
    /// Every `fullTextUrlList.fullTextUrl` entry, in the order Europe PMC gave
    /// them.
    pub full_text_urls: Vec<EuropePmcFullText>,
}

/// One `fullTextUrl` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EuropePmcFullText {
    pub url: String,
    /// `Europe_PMC`, `PubMedCentral`, `Unpaywall`, `DOI`, …
    pub site: Option<String>,
    /// `pdf`, `html` or `doi`.
    pub document_style: Option<String>,
    /// `OA`, `F` or `S` — open access, free, or subscription required.
    pub availability_code: Option<String>,
}

impl EuropePmcFullText {
    /// Is this entry a document Europe PMC or PMC serves, rather than a DOI?
    ///
    /// `site: DOI` is the DOI resolver's own landing page — which the pass
    /// already derives from the DOI under [`crate::resolve::RouteId::Publisher`]
    /// — and offering it again as a Europe PMC location would file a publisher
    /// fetch under the repository's route.
    #[must_use]
    pub fn is_repository_document(&self) -> bool {
        self.site.as_deref() != Some("DOI")
            && self
                .document_style
                .as_deref()
                .is_some_and(|style| matches!(style, "pdf" | "html"))
    }
}

/// A Europe PMC client that spends [`PaceTier::Meta`] permits.
///
/// Same shape as [`crate::crossref`] and [`crate::openalex`]: a base URL, a
/// `with_base_url` test seam, one [`Self::lookup`] owning the whole request.
#[derive(Debug, Clone)]
pub struct EuropePmcAdapter {
    base_url: String,
    /// Appended as `&email=`. Europe PMC documents no polite pool, so this is
    /// an identifier rather than a rate-limit claim — but it is what their
    /// support address asks for.
    mailto: String,
}

impl EuropePmcAdapter {
    /// The production adapter.
    #[must_use]
    pub fn new(mailto: impl Into<String>) -> Self {
        Self {
            base_url: EUROPE_PMC_REST_URL.to_string(),
            mailto: mailto.into(),
        }
    }

    /// Point the adapter at a different REST base. Test seam; the production
    /// default is [`EUROPE_PMC_REST_URL`].
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The search URL for `doi`.
    ///
    /// # Errors
    ///
    /// Only if the base URL will not parse — for [`EUROPE_PMC_REST_URL`],
    /// impossible.
    pub fn search_url(&self, doi: &str) -> Result<Url, AdapterError> {
        let mut url = Url::parse(&self.base_url)
            .map_err(|e| AdapterError::Validation(format!("invalid Europe PMC base URL: {e}")))?;
        url.path_segments_mut()
            .map_err(|()| AdapterError::Validation("Europe PMC base URL cannot be a base".into()))?
            .extend(["search"]);
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("query", &format!("DOI:\"{}\"", normalize_doi(doi)));
            query.append_pair("resultType", "core");
            query.append_pair("format", "json");
            if !self.mailto.trim().is_empty() {
                query.append_pair("email", self.mailto.trim());
            }
        }
        Ok(url)
    }

    /// Look `doi` up, spending one `Request` permit.
    ///
    /// # Errors
    ///
    /// Only for a DOI the validator rejects, or an unparseable base URL. A 404
    /// and a 5xx are [`EuropePmcAnswer`] outcomes, not errors: "Europe PMC has
    /// never heard of it" and "we could not ask" are different facts and the
    /// ranker reports them differently.
    pub async fn lookup(
        &self,
        client: &PacedClient,
        work: &WorkScope,
        doi: &str,
    ) -> Result<EuropePmcAnswer, AdapterError> {
        if !validate_doi(doi) {
            return Err(AdapterError::Validation(format!("invalid DOI: {doi}")));
        }
        let url = self.search_url(doi)?;
        let response = match client
            .get_in_work(work, url, PaceTier::Meta, SafeHeaders::unauthenticated())
            .await
        {
            Ok(response) => response,
            Err(error) => return Ok(classify(error, doi)),
        };
        Ok(read_search(response).await)
    }
}

/// What one Europe PMC lookup established.
#[derive(Debug, Clone, PartialEq)]
pub enum EuropePmcAnswer {
    /// Europe PMC has at least one record for the DOI.
    Found(Vec<EuropePmcRecord>),
    /// 404: Europe PMC does not index this DOI. Knowledge, not failure.
    NotRegistered,
    /// A 2xx we could not read as a search envelope.
    Unreadable(String),
}

async fn read_search(response: PacedResponse) -> EuropePmcAnswer {
    let text = match response.text().await {
        Ok(text) => text,
        Err(e) => return EuropePmcAnswer::Unreadable(format!("could not read the body: {e}")),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(body) => parse_search(&body),
        Err(e) => EuropePmcAnswer::Unreadable(format!("JSON parse failed: {e}")),
    }
}

fn classify(error: scitadel_http::FetchError, doi: &str) -> EuropePmcAnswer {
    match error {
        scitadel_http::FetchError::Status { code: 404, .. } => {
            tracing::info!(doi = %doi, "europe pmc does not index this DOI");
            EuropePmcAnswer::NotRegistered
        }
        other => EuropePmcAnswer::Unreadable(other.to_string()),
    }
}

/// The records a Europe PMC search body carries.
///
/// Requires the `resultList.result` envelope **and** a `hitCount`, for the
/// reason [`crate::crossref`] documents: Europe PMC answers a query it did not
/// understand with `200` and an error document, and a lenient parser would turn
/// that into "Europe PMC has nothing", which is a claim about the work rather
/// than about our query. `hitCount: 0` with a well-formed envelope *is* a
/// knowledge answer and yields [`EuropePmcAnswer::NotRegistered`].
#[must_use]
pub fn parse_search(body: &serde_json::Value) -> EuropePmcAnswer {
    let Some(results) = body
        .pointer("/resultList/result")
        .and_then(|r| r.as_array())
    else {
        return EuropePmcAnswer::Unreadable("no `resultList.result` in the response".into());
    };
    if body
        .get("hitCount")
        .and_then(serde_json::Value::as_i64)
        .is_none()
    {
        return EuropePmcAnswer::Unreadable("no `hitCount` in the response".into());
    }
    if results.is_empty() {
        return EuropePmcAnswer::NotRegistered;
    }
    EuropePmcAnswer::Found(results.iter().filter_map(record).collect())
}

fn record(value: &serde_json::Value) -> Option<EuropePmcRecord> {
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())?
        .to_string();
    let auth_man = flag(value, "authMan");
    let epmc_auth_man = flag(value, "epmcAuthMan");
    let nih_auth_man = flag(value, "nihAuthMan");
    let source = value
        .get("source")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(EuropePmcRecord {
        id,
        source: source.clone(),
        pmcid: value
            .get("pmcid")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|pmcid| !pmcid.is_empty())
            .map(str::to_string),
        is_open_access: flag(value, "isOpenAccess"),
        in_epmc: flag(value, "inEPMC"),
        has_pdf: flag(value, "hasPDF"),
        licence: value
            .get("license")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|licence| !licence.is_empty())
            .map(str::to_ascii_lowercase),
        // The ladder order, and each rung's reason it is not the next one up.
        // A manuscript flag wins over the record's own source because it is a
        // statement about *the copy Europe PMC will serve*, where `source` is a
        // statement about which database the record was harvested from — the
        // same record-level-versus-location distinction `VersionSource` is built
        // around.
        version: if auth_man || epmc_auth_man || nih_auth_man {
            RepositoryVersion::AuthorManuscript
        } else if source.as_deref() == Some("PPR") {
            RepositoryVersion::Preprint
        } else {
            RepositoryVersion::Unstated
        },
        full_text_urls: full_text_urls(value),
    })
}

fn full_text_urls(value: &serde_json::Value) -> Vec<EuropePmcFullText> {
    value
        .pointer("/fullTextUrlList/fullTextUrl")
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let url = entry
                        .get("url")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|url| !url.is_empty())?;
                    Some(EuropePmcFullText {
                        url: url.to_string(),
                        site: entry
                            .get("site")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        document_style: entry
                            .get("documentStyle")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                        availability_code: entry
                            .get("availabilityCode")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A Europe PMC `Y`/`N` flag.
///
/// Anything else — absent, `"true"`, a number — is `false`, because a flag we
/// cannot read is a flag that did not say yes. The alternative, treating an
/// unreadable flag as "unknown" and letting it fall through, would reach the
/// same answer by a longer route; the difference is that this way there is one
/// place where a flag is interpreted.
fn flag(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|raw| raw.trim().eq_ignore_ascii_case("y"))
}

/// The three identity facts, out of a Europe PMC record.
///
/// Europe PMC is **not** in ADR-007 §3's pre-fetch identity chain
/// (`OpenAlex → Crossref → DataCite`), and this exists only so the record's
/// `title` and `authorString` are available to a caller that wants them — it is
/// deliberately not wired into the identity answer, for the reason
/// [`crate::registry::Registry::identity_source`] is `None` for Europe PMC.
#[must_use]
pub fn work_identity(record: &serde_json::Value) -> WorkIdentity {
    WorkIdentity {
        title: record
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(|title| title.trim_end_matches('.').to_string()),
        year: record
            .get("pubYear")
            .and_then(serde_json::Value::as_str)
            .and_then(|year| year.parse().ok()),
        first_author: record
            .get("authorString")
            .and_then(serde_json::Value::as_str)
            .and_then(|authors| authors.split(',').next())
            .map(str::trim)
            .filter(|author| !author.is_empty())
            .map(|author| author.replace(' ', ", ")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The measured NumPy envelope, re-exported for `crate::resolve`'s pass-level
    /// tests.
    ///
    /// One fixture in one place: a pass-level test that embedded its own copy
    /// would be a second statement of what Europe PMC sends, and the two would
    /// drift the first time a field was renamed.
    #[cfg(test)]
    pub(crate) const NUMPY_ENVELOPE: &str = NUMPY;
    use crate::registry::RepositoryVersion;
    use scitadel_core::ports::{Bucket, Cost, PaceDenied, Pacer, Permit};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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

    fn client(table: scitadel_http::BucketPolicyTable) -> PacedClient {
        PacedClient::new(
            PacedClient::default_transport().expect("transport"),
            Arc::new(RecordingPacer::default()),
            table,
        )
    }

    fn routing_table(server: &MockServer, bucket: &str) -> scitadel_http::BucketPolicyTable {
        let mut table = scitadel_http::BucketPolicyTable::new();
        table.route(&format!("127.0.0.1:{}", server.address().port()), bucket);
        table
    }

    /// The measured envelope for `10.1038/s41586-020-2649-2`, reduced to the
    /// fields this module reads and left otherwise as Europe PMC sends it — the
    /// nesting is the point.
    ///
    /// Every flag here is `N`/`Y` **as a string**, which is the shape a parser
    /// written against booleans would fail on, and `license` is the short code
    /// `cc by` rather than a URL, which is the other half of the same point.
    const NUMPY: &str = r#"{
      "version": "6.9",
      "hitCount": 1,
      "resultList": {
        "result": [
          {
            "id": "32939066",
            "source": "MED",
            "pmcid": "PMC7759461",
            "doi": "10.1038/s41586-020-2649-2",
            "title": "Array programming with NumPy.",
            "authorString": "Harris CR, Millman KJ, van der Walt SJ",
            "pubYear": "2020",
            "isOpenAccess": "Y",
            "inEPMC": "Y",
            "inPMC": "Y",
            "hasPDF": "Y",
            "license": "cc by",
            "authMan": "N",
            "epmcAuthMan": "N",
            "nihAuthMan": "N",
            "fullTextUrlList": {
              "fullTextUrl": [
                {"availability": "Subscription required", "availabilityCode": "S",
                 "documentStyle": "doi", "site": "DOI",
                 "url": "https://doi.org/10.1038/s41586-020-2649-2"},
                {"availability": "Open access", "availabilityCode": "OA",
                 "documentStyle": "html", "site": "Europe_PMC",
                 "url": "https://europepmc.org/articles/PMC7759461"},
                {"availability": "Open access", "availabilityCode": "OA",
                 "documentStyle": "pdf", "site": "Europe_PMC",
                 "url": "https://europepmc.org/articles/PMC7759461?pdf=render"}
              ]
            }
          }
        ]
      }
    }"#;

    /// The measured envelope for the bioRxiv preprint
    /// `10.1101/2020.01.30.927871`: a `PPR` record whose Europe PMC copy is an
    /// author manuscript (`epmcAuthMan: Y`, `authMan: N`).
    const BIORXIV_PPR: &str = r#"{
      "version": "6.9",
      "hitCount": 1,
      "resultList": {
        "result": [
          {
            "id": "PPR110986",
            "source": "PPR",
            "doi": "10.1101/2020.01.30.927871",
            "title": "Uncanny similarity of unique inserts",
            "authorString": "Pradhan P, Pandey AK",
            "pubYear": "2020",
            "isOpenAccess": "Y",
            "inEPMC": "Y",
            "inPMC": "N",
            "hasPDF": "Y",
            "license": "cc by-nc-nd",
            "authMan": "N",
            "epmcAuthMan": "Y",
            "nihAuthMan": "N",
            "fullTextUrlList": {
              "fullTextUrl": [
                {"availability": "Open access", "availabilityCode": "OA",
                 "documentStyle": "pdf", "site": "Unpaywall",
                 "url": "https://www.biorxiv.org/content/biorxiv/early/2020/02/02/2020.01.30.927871.full.pdf"},
                {"availability": "Free", "availabilityCode": "F",
                 "documentStyle": "doi", "site": "DOI",
                 "url": "https://doi.org/10.1101/2020.01.30.927871"},
                {"availability": "Open access", "availabilityCode": "OA",
                 "documentStyle": "html", "site": "Europe_PMC",
                 "url": "https://europepmc.org/article/PPR/PPR110986"},
                {"availability": "Open access", "availabilityCode": "OA",
                 "documentStyle": "pdf", "site": "Europe_PMC",
                 "url": "https://europepmc.org/api/fulltextRepo?pprId=PPR110986&type=FILE&fileName=EMS159615-pdf.pdf&mimeType=application/pdf"}
              ]
            }
          }
        ]
      }
    }"#;

    fn records(body: &str) -> Vec<EuropePmcRecord> {
        let value: serde_json::Value = serde_json::from_str(body).expect("fixture is json");
        match parse_search(&value) {
            EuropePmcAnswer::Found(records) => records,
            other => panic!("expected records, got {other:?}"),
        }
    }

    /// The request line, and the `resultType` that makes it worth making.
    ///
    /// `resultType=core` is asserted because without it Europe PMC answers
    /// every question this adapter asks with a `lite` record that carries no
    /// `fullTextUrlList`, no `license` and no manuscript flag — a green request
    /// and an empty answer, which is the failure mode a parser test cannot see.
    #[test]
    fn the_request_asks_for_the_core_result_type() {
        let url = EuropePmcAdapter::new("polite@example.org")
            .search_url("10.1038/s41586-020-2649-2")
            .expect("url builds");
        assert_eq!(url.host_str(), Some("www.ebi.ac.uk"));
        assert_eq!(url.path(), "/europepmc/webservices/rest/search");
        let query: std::collections::BTreeMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["query"], "DOI:\"10.1038/s41586-020-2649-2\"");
        assert_eq!(
            query["resultType"], "core",
            "the lite result carries no full-text list, no licence and no \
             manuscript flag, so a lite request answers nothing"
        );
        assert_eq!(query["format"], "json");
        assert_eq!(query["email"], "polite@example.org");
    }

    /// A DOI is put in the **query string**, so the path cannot be truncated by
    /// a `#` or `?` inside it (#262's permissive suffix rule).
    #[test]
    fn a_segmented_doi_is_encoded_into_the_query_and_not_the_path() {
        let url = EuropePmcAdapter::new("")
            .search_url("10.26434/chemrxiv.15000484/v1")
            .expect("url builds");
        assert_eq!(url.path(), "/europepmc/webservices/rest/search");
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "query" && v == "DOI:\"10.26434/chemrxiv.15000484/v1\""),
            "the whole DOI is one query value: {url}"
        );
    }

    /// The version claim, which is the whole reason this adapter exists: a
    /// `MED` record with every manuscript flag `N` is **not** a version of
    /// record, and saying so is the difference between an unstated rung and a
    /// `vor` claim nobody made.
    #[test]
    fn a_published_record_with_no_manuscript_flag_states_no_version() {
        let record = records(NUMPY).remove(0);
        assert_eq!(record.version, RepositoryVersion::Unstated);
        assert_eq!(record.source.as_deref(), Some("MED"));
        assert!(record.is_open_access && record.in_epmc && record.has_pdf);
        assert_eq!(record.licence.as_deref(), Some("cc by"));
    }

    /// A preprint record *is* a preprint, by Europe PMC's own `source`, and an
    /// author-manuscript flag beats it — the flag is about the copy Europe PMC
    /// serves, `source` is about which database harvested the record.
    #[test]
    fn a_manuscript_flag_outranks_the_records_own_source() {
        let record = records(BIORXIV_PPR).remove(0);
        assert_eq!(record.source.as_deref(), Some("PPR"));
        assert_eq!(
            record.version,
            RepositoryVersion::AuthorManuscript,
            "`epmcAuthMan: Y` says Europe PMC's copy is the manuscript, which is \
             a statement about the bytes rather than about the record"
        );
        assert_eq!(record.licence.as_deref(), Some("cc by-nc-nd"));
        assert!(
            record.is_open_access,
            "and it is open access, so the location is offered at all"
        );
    }

    /// `source: PPR` with no manuscript flag is a preprint — the one case where
    /// the record's database is itself the version evidence.
    #[test]
    fn a_preprint_record_with_no_manuscript_flag_is_a_preprint() {
        let body = r#"{"hitCount":1,"resultList":{"result":[
            {"id":"PPR1316957","source":"PPR","doi":"10.64898/x","isOpenAccess":"Y",
             "inEPMC":"Y","hasPDF":"Y","authMan":"N","epmcAuthMan":"N","nihAuthMan":"N"}]}}"#;
        assert_eq!(records(body).remove(0).version, RepositoryVersion::Preprint);
    }

    /// Every one of the three flags, individually, is enough — because
    /// collapsing them into one boolean is how a `MED` record whose Europe PMC
    /// copy is a manuscript gets filed as a version of record.
    #[test]
    fn each_manuscript_flag_alone_is_enough() {
        for flag in ["authMan", "epmcAuthMan", "nihAuthMan"] {
            let body = format!(
                r#"{{"hitCount":1,"resultList":{{"result":[
                    {{"id":"1","source":"MED","doi":"10.1/x","{flag}":"Y"}}]}}}}"#
            );
            assert_eq!(
                records(&body).remove(0).version,
                RepositoryVersion::AuthorManuscript,
                "{flag} is Europe PMC's own statement about the copy it serves"
            );
        }
    }

    /// A flag we cannot read is a flag that did not say yes.
    ///
    /// The trap is `"true"`: a parser using `value == "Y"` or
    /// `as_bool().unwrap_or(false)` gets `false` here by luck, and one using
    /// `!= "N"` claims a manuscript for every record Europe PMC ever sent.
    #[test]
    fn an_unreadable_manuscript_flag_is_not_a_manuscript() {
        // Trailing space is *not* in this list: `flag` trims, and a padded `"Y "`
        // is Europe PMC's `Y`.
        for raw in [r#""true""#, "1", r#""""#, r#""Yes""#, "null"] {
            let body = format!(
                r#"{{"hitCount":1,"resultList":{{"result":[
                    {{"id":"1","source":"MED","doi":"10.1/x","authMan":{raw}}}]}}}}"#
            );
            assert_eq!(
                records(&body).remove(0).version,
                RepositoryVersion::Unstated,
                "authMan: {raw} is not `Y`, and Europe PMC spells yes `Y`"
            );
        }
        for absent in ["", r#","doi":"10.1/x""#] {
            let body = format!(
                r#"{{"hitCount":1,"resultList":{{"result":[{{"id":"1","source":"MED" {absent}}}]}}}}"#
            );
            assert_eq!(
                records(&body).remove(0).version,
                RepositoryVersion::Unstated
            );
        }
    }

    /// The two document styles Europe PMC serves, and the one it does not.
    ///
    /// `site: DOI` is excluded because the pass already derives that URL from
    /// the DOI under the publisher route, and offering it here would record a
    /// publisher fetch under the repository's route.
    #[test]
    fn a_doi_landing_page_is_not_a_europe_pmc_location() {
        let record = records(NUMPY).remove(0);
        let documents: Vec<&str> = record
            .full_text_urls
            .iter()
            .filter(|entry| entry.is_repository_document())
            .map(|entry| entry.url.as_str())
            .collect();
        assert_eq!(
            documents,
            vec![
                "https://europepmc.org/articles/PMC7759461",
                "https://europepmc.org/articles/PMC7759461?pdf=render",
            ],
            "both served styles, and not the DOI resolver's page"
        );
        // And the styles are read as Europe PMC wrote them, not normalised.
        let styles: Vec<&str> = record
            .full_text_urls
            .iter()
            .filter_map(|entry| entry.document_style.as_deref())
            .collect();
        assert_eq!(styles, vec!["doi", "html", "pdf"]);
        let availability: Vec<&str> = record
            .full_text_urls
            .iter()
            .filter_map(|entry| entry.availability_code.as_deref())
            .collect();
        assert_eq!(availability, vec!["S", "OA", "OA"]);
    }

    /// A Unpaywall-named URL is a location Europe PMC *reports*, not one Europe
    /// PMC serves, and it stays in the list: dropping it would lose a real
    /// repository copy that the ranker can weigh.
    #[test]
    fn a_location_named_by_another_service_is_kept_and_labelled() {
        let record = records(BIORXIV_PPR).remove(0);
        let biorxiv = record
            .full_text_urls
            .iter()
            .find(|entry| entry.url.contains("biorxiv.org"))
            .expect("the bioRxiv copy");
        assert_eq!(biorxiv.site.as_deref(), Some("Unpaywall"));
        assert!(biorxiv.is_repository_document());
        assert_eq!(biorxiv.document_style.as_deref(), Some("pdf"));
    }

    /// `hitCount: 0` is knowledge; a 200 that is not a search envelope is not.
    ///
    /// Both halves, because they are different facts and only the first one is
    /// about the work. Europe PMC answers a malformed `query` with 200 and an
    /// error document, and reading that as "Europe PMC has nothing" would
    /// report a bad request as an absence.
    #[test]
    fn a_zero_hit_count_is_not_registered_and_a_bad_envelope_is_unreadable() {
        // The measured shape for a DOI Europe PMC does not index, verbatim.
        let empty: serde_json::Value = serde_json::from_str(
            r#"{"version":"6.9","hitCount":0,"request":{"queryString":"DOI:\"10.99999/no.such.doi\"","resultType":"core","cursorMark":"*","pageSize":25,"sort":"","synonym":false},"resultList":{"result":[]}}"#,
        )
        .expect("json");
        assert_eq!(parse_search(&empty), EuropePmcAnswer::NotRegistered);

        for body in [
            r#"{"error":"Bad request","message":"invalid query"}"#,
            r#"{"hitCount":1}"#,
            r#"{"resultList":{"result":[]}}"#,
            "[]",
            r#"{"hitCount":"one","resultList":{"result":[{"id":"1"}]}}"#,
            r#"{"hitCount":0,"resultList":{}}"#,
            r#"{"resultList":{"result":[{"id":"1"}]}}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(body).expect("json");
            assert!(
                matches!(parse_search(&value), EuropePmcAnswer::Unreadable(_)),
                "{body} is not a search envelope"
            );
        }
    }

    /// A record with no `id` is skipped rather than half-built.
    #[test]
    fn a_record_without_an_id_contributes_nothing() {
        let body = r#"{"hitCount":1,"resultList":{"result":[
            {"source":"MED","doi":"10.1/x"},
            {"id":"2","source":"MED","doi":"10.1/y","authMan":"Y"}]}}"#;
        let found = records(body);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].id, "2");
    }

    /// The happy path over a `wiremock` server, so the URL the adapter builds
    /// and the request the client makes are known to agree — and so the bucket
    /// the hop spends is asserted through the pacer.
    ///
    /// #261's rule applies to the *test* as much as the code: without a fixture
    /// a request that escapes to `www.ebi.ac.uk` returns 200 and the test
    /// passes having tested nothing.
    #[tokio::test]
    async fn a_lookup_over_wiremock_charges_the_europepmc_bucket_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(NUMPY),
            )
            .mount(&server)
            .await;

        let pacer = Arc::new(RecordingPacer::default());
        let client = PacedClient::new(
            PacedClient::default_transport().expect("transport"),
            Arc::clone(&pacer) as Arc<dyn Pacer>,
            routing_table(&server, "europepmc"),
        );
        let answer = EuropePmcAdapter::new("polite@example.org")
            .with_base_url(server.uri())
            .lookup(&client, &WorkScope::new(), "10.1038/s41586-020-2649-2")
            .await
            .expect("a 200 is never a request failure");
        assert_eq!(
            answer,
            EuropePmcAnswer::Found(records(NUMPY)),
            "the wire answer and the parsed answer are one answer"
        );
        assert_eq!(
            *pacer.0.lock().expect("lock"),
            vec![("europepmc".to_string(), PaceTier::Meta)],
            "one `Request` permit at the `Meta` tier out of the `europepmc` \
             bucket the policy table routes the address to"
        );
    }

    /// A 404 is "not indexed here", not a failure and not unreadable — the same
    /// split `identity_chain` makes, and the reason a Europe-PMC miss does not
    /// read as "the registry could not be reached".
    #[tokio::test]
    async fn a_404_is_not_registered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
            .mount(&server)
            .await;
        let answer = EuropePmcAdapter::new("")
            .with_base_url(server.uri())
            .lookup(
                &client(routing_table(&server, "europepmc")),
                &WorkScope::new(),
                "10.1038/x",
            )
            .await
            .expect("answer");
        assert_eq!(answer, EuropePmcAnswer::NotRegistered);
    }

    /// A 5xx is *we could not find out*, and must never be reported as an
    /// absence: that is what would end the pass on a transient error.
    #[tokio::test]
    async fn a_5xx_is_unreadable_not_not_registered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .mount(&server)
            .await;
        let answer = EuropePmcAdapter::new("")
            .with_base_url(server.uri())
            .lookup(
                &client(routing_table(&server, "europepmc")),
                &WorkScope::new(),
                "10.1038/x",
            )
            .await
            .expect("answer");
        assert!(
            matches!(answer, EuropePmcAnswer::Unreadable(_)),
            "{answer:?}"
        );
    }

    /// An invalid DOI never reaches the wire.
    #[tokio::test]
    async fn an_invalid_doi_is_refused_before_any_request() {
        let server = MockServer::start().await;
        let answer = EuropePmcAdapter::new("")
            .with_base_url(server.uri())
            .lookup(
                &client(routing_table(&server, "europepmc")),
                &WorkScope::new(),
                "not a doi",
            )
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
}
