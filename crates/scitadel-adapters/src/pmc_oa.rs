//! The `pmc-oa-opendata` dataset, as a source of **per-version licence and
//! `is_manuscript`** (ADR-007 §3 step 2).
//!
//! # `oa.fcgi` is gone, and what replaced it
//!
//! ADR-007 §3 step 2 and `RouteId::PmcOa`'s own doc both say the PMC OA Web
//! Service was retired on 2026-08-25. Measured 2026-10-07, for the record:
//!
//! ```text
//! GET https://www.ncbi.nlm.nih.gov/pmc/utils/oa/oa.fcgi?id=PMC7759461
//!   -> 404 (NCBI "WWW Error 404 Diagnostic", text/html, on every host alias
//!      tried: www.ncbi.nlm.nih.gov, pmc.ncbi.nlm.nih.gov, eutils.ncbi.nlm.nih.gov)
//! https://pmc.ncbi.nlm.nih.gov/tools/oa-service/
//!   -> 200, and the page's own body: "the PMC OA Web Service is no longer
//!      available. Please see the PMC Article Datasets page for more information."
//! ```
//!
//! What replaced it is the **PMC Cloud Service**: an anonymous S3 bucket,
//! `pmc-oa-opendata`, documented at
//! `https://pmc-oa-opendata.s3.amazonaws.com/README.txt`. So `pmc-oa-opendata`
//! is a *bucket name*, not a service endpoint, and everything here is an S3
//! REST call against its HTTPS form.
//!
//! # Two requests per PMCID, and that is the cost
//!
//! A PMCID can have several stored versions (`PMC11370360.1`,
//! `PMC11370360.2`, …) and **the version number does not say which is which** —
//! the bucket's own README says so, and it is the reason this slice reads the
//! per-version JSON at all. Discovering them takes a delimiter listing and then
//! one JSON read per version:
//!
//! ```text
//! GET {base}/?list-type=2&prefix=PMC11370360.&delimiter=/
//!   -> <ListBucketResult><CommonPrefixes><Prefix>PMC11370360.1/</Prefix>…
//! GET {base}/metadata/PMC11370360.1.json
//!   -> {"pmcid":…,"version":1,"is_pmc_openaccess":true,"is_manuscript":false,
//!       "license_code":"CC BY","pdf_url":"s3://pmc-oa-opendata/…?md5=…"}
//! ```
//!
//! So **one listing plus one read per version** — the most expensive lookup in
//! the metadata pass, and the reason it runs only for a work that carries a
//! `pmcid` and only after Europe PMC has been asked whether the work is
//! open-access at all.
//!
//! # The `s3://` URLs must be rewritten
//!
//! Every `pdf_url` in the JSON is an `s3://` URI with an MD5 digest as a query
//! parameter, which `reqwest` cannot fetch and which [`crate::resolve`]'
//! `is_fetchable_url` correctly refuses (its scheme is not `http`/`https`).
//! The HTTPS form is the same bucket over the REST endpoint — measured, a
//! `HEAD` on it answers `200` with the same `ETag` as the digest in the query —
//! so [`PmcOaAdapter::object_url`] is a rewrite and not a guess. It rebases the
//! key onto the adapter's **configured** base rather than the bucket name the
//! JSON carries, which is what keeps the dataset from naming this process's
//! host; see that method for why that is not a detail.
//!
//! # `license_code` is a **code**, and `TDM` is not a licence
//!
//! `license_code` is `CC BY`, `CC BY-NC-ND`, `CC0`, or `TDM`. The last one is
//! the important one: the bucket's README says it marks author manuscripts
//! "available for text mining, and where the full text may also be used
//! consistent with the principles of fair use". That is a **text-mining
//! permission, not a reuse licence**, so it must not become
//! [`crate::resolve::LicenceStrength::OpenLicence`]. It is free to read and
//! nothing more, which is [`crate::resolve::LicenceStrength::FreeToRead`].

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::registry::RepositoryVersion;

/// The PMC Cloud Service bucket, over its HTTPS endpoint.
pub const PMC_OA_BUCKET_URL: &str = "https://pmc-oa-opendata.s3.amazonaws.com";

/// The S3 URI scheme the metadata JSON writes its object URLs in.
const S3_SCHEME: &str = "s3://";

/// One stored version of one article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmcVersion {
    /// `PMC11370360`, from the key prefix.
    pub pmcid: String,
    /// The version number, from the key prefix. **Not** a recency or a
    /// preference: the bucket's README is explicit that only the JSON says
    /// which version this is.
    pub version: u64,
    /// `is_manuscript` — whether this version is an author manuscript. The
    /// strongest version evidence in the whole metadata pass, because it is
    /// PMC's statement about *this file* rather than about the work.
    pub is_manuscript: bool,
    /// `is_pmc_openaccess` — whether this version is in the OA subset. A
    /// manuscript can be in the dataset and not be open access (the `TDM`
    /// case), so this and `is_manuscript` are independent.
    pub is_pmc_openaccess: bool,
    /// `is_retracted`. Recorded, not acted on: whether a library should file a
    /// retracted article at all is a policy question this module does not
    /// answer, and dropping it here would make it unanswerable from the plan.
    pub is_retracted: bool,
    /// `license_code`, Europe PMC's short spelling: `CC BY`, `CC BY-NC-ND`,
    /// `CC0`, or `TDM`.
    pub license_code: Option<String>,
    /// The version's PDF, as an HTTPS URL, when the version has one.
    ///
    /// `None` is real and measured (`PMC8238499.1`, the Seurat paper, carries
    /// no `pdf_url`) — the publisher licensed the XML and not the PDF.
    pub pdf_url: Option<String>,
    /// The version's JATS XML, as an HTTPS URL.
    pub xml_url: Option<String>,
}

/// A `pmc-oa-opendata` client: a base URL and two URL builders, no state.
///
/// No `PacedClient` call lives here — the metadata pass owns its requests
/// through [`crate::resolve::MetadataPass::read`], so that every hop is one
/// permit out of one bucket and is refused if its URL leaves the configured
/// base. This type is URL construction and parsing only, which is what keeps
/// the "the metadata pass touches no publisher host" property a property of the
/// pass.
#[derive(Debug, Clone)]
pub struct PmcOaAdapter {
    base_url: String,
}

impl PmcOaAdapter {
    /// The production adapter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            base_url: PMC_OA_BUCKET_URL.to_string(),
        }
    }

    /// Point the adapter at a different bucket endpoint. Test seam.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// The bucket's HTTPS base.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The version listing for a PMCID, or `None` when the identifier is not a
    /// PMCID at all.
    ///
    /// `list-type=2` with a `delimiter` is what turns the eight-million-object
    /// bucket into a handful of `<CommonPrefixes>` — the same call the bucket's
    /// README documents.
    ///
    /// Built through [`reqwest::Url`] rather than a `format!`, so the query is
    /// **percent-encoded** by the same machinery every other adapter uses. The
    /// `delimiter` is the character that needs it: written literally the URL is
    /// valid, and a hand-spliced one is a `?`-in-a-value bug waiting for the day
    /// a parameter takes a DOI.
    pub fn versions_url(&self, pmcid: &str) -> Option<reqwest::Url> {
        let pmcid = normalise_pmcid(pmcid)?;
        let mut url = reqwest::Url::parse(self.base_url.trim_end_matches('/'))
            .ok()?
            .join("/")
            .ok()?;
        url.query_pairs_mut()
            .append_pair("list-type", "2")
            .append_pair("prefix", &format!("{pmcid}."))
            .append_pair("delimiter", "/");
        Some(url)
    }

    /// The per-version JSON URL, or `None` for an unusable identifier.
    ///
    /// One `Url` per stored version — the two-hop shape the bucket documents.
    #[must_use]
    ///
    /// A [`reqwest::Url`] rather than a `String` for the same reason as
    /// [`Self::versions_url`]: the PMCID is path data, and building a URL by
    /// hand is what lets a `?` or `#` in an identifier truncate the path.
    pub fn metadata_url(&self, pmcid: &str, version: u64) -> Option<reqwest::Url> {
        let pmcid = normalise_pmcid(pmcid)?;
        reqwest::Url::parse(self.base_url.trim_end_matches('/'))
            .ok()?
            .join(&format!("/metadata/{pmcid}.{version}.json"))
            .ok()
    }

    /// The HTTPS URL for an object the metadata JSON named as an `s3://` URI,
    /// **rebased onto this adapter's configured base**.
    ///
    /// Rebasing, not rewriting: the JSON's `s3://pmc-oa-opendata/…` supplies the
    /// *key* and the MD5 digest, and this adapter's base supplies the *host*. The
    /// distinction is the whole point, and it is the difference between a
    /// candidate this process can fetch and one whose host a third party's JSON
    /// chose — a `s3://` URI naming some other bucket would otherwise become a
    /// candidate on a host nobody configured, and `artefacts.source_url` would
    /// record it as where the bytes came from.
    ///
    /// `None` when the value is not an `s3://` URI, or when the key cannot be
    /// joined onto the base.
    #[must_use]
    pub fn object_url(&self, raw: &str) -> Option<reqwest::Url> {
        let key = s3_object_key(raw)?;
        let mut url = reqwest::Url::parse(self.base_url.trim_end_matches('/'))
            .ok()?
            .join(key)
            .ok()?;
        // The digest is a query parameter the bucket documents on every object
        // URL, so it travels with the key rather than being rebuilt — a URL
        // without it is fetched outside NCBI's integrity check.
        if let Some(digest) = s3_object_digest(raw)
            && !digest.is_empty()
        {
            url.set_query(Some(&format!("md5={digest}")));
        }
        Some(url)
    }
}

impl Default for PmcOaAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// A PMCID in the `PMC1234567` spelling the bucket keys on, or `None`.
///
/// Rejects anything else rather than passing it into a URL: the listing is a
/// prefix query, so a `pmcid` that is actually a DOI would list a different
/// work's versions under this one.
#[must_use]
pub fn normalise_pmcid(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let digits = trimmed
        .strip_prefix("PMC")
        .or_else(|| trimmed.strip_prefix("pmc"))?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("PMC{digits}"))
}

/// Every stored version of a PMCID, from a delimiter listing body.
///
/// Only `<CommonPrefixes>/<Prefix>` entries are read, and each must be exactly
/// `PMC<digits>.<digits>/`. A `<Key>` line is an object, not a version — the
/// listing with the delimiter returns only prefixes for that prefix — and
/// accepting one would read `PMC…1/PMC…1.pdf` as a version.
#[must_use]
pub fn parse_versions(body: &str) -> Vec<PmcVersionRef> {
    let mut reader = Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut out: Vec<PmcVersionRef> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    // `<Prefix>` carries its value as character data, so the element and the
    // text are two events. Matching on the element alone would read the start
    // tag's own bytes and match nothing — and a fixture written from the same
    // wrong assumption would hide it.
    let mut inside_prefix = false;
    loop {
        match reader.read_event_into(&mut buf) {
            // One arm for "no more events" and "the XML stopped making sense":
            // both end the walk, and both leave `out` holding whatever was
            // already read. A listing that fails to parse is *no* versions, and
            // the caller's hop was already recorded as answered — so an empty
            // list here reads as "this PMCID has no stored versions we could
            // name", which is what a malformed listing is worth knowing.
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(ref e)) => {
                // The XML is namespaced (`<Prefix>` here, `<s3:Prefix>` on some
                // responses), so the local name is what is compared.
                inside_prefix = local_name(e.name().as_ref()) == b"Prefix";
            }
            Ok(Event::Text(ref text)) if inside_prefix => {
                if let Ok(text) = text.unescape()
                    && let Some(version) = parse_version_prefix(text.trim())
                {
                    out.push(version);
                }
            }
            Ok(Event::End(_)) => inside_prefix = false,
            Ok(_) => {}
        }
        buf.clear();
    }
    out
}

/// A `PMC<digits>.<digits>` key prefix, split into its two parts.
///
/// A struct rather than a `String` because the pass immediately needs both
/// halves — the PMCID to name the candidate and the version to build the
/// metadata URL — and a `String` would leave that split to a `rsplit_once` at
/// the call site, where a key with two dots would split differently from the one
/// that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmcVersionRef {
    pub pmcid: String,
    pub version: u64,
}

impl std::fmt::Display for PmcVersionRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.pmcid, self.version)
    }
}

/// A `PMC<digits>.<digits>` key prefix, or `None`.
fn parse_version_prefix(raw: &str) -> Option<PmcVersionRef> {
    let (pmcid, version) = raw.trim_end_matches('/').rsplit_once('.')?;
    Some(PmcVersionRef {
        pmcid: normalise_pmcid(pmcid)?,
        version: version.parse::<u64>().ok()?,
    })
}

/// A per-version JSON body for the version `asked_for`, or `None` when it is
/// not one.
///
/// Requires `pmcid`, `version` and `is_manuscript`: the first two because a
/// version record without them cannot be attributed to a stored version, and
/// the third because `is_manuscript` is the whole point of reading this file.
/// A version whose `is_manuscript` is absent is **not** read as `false` — see
/// [`PmcVersion::is_manuscript`].
///
/// The body's own `pmcid` must be the one we asked for, and that is a check
/// rather than a formality: the candidate's URL and its recorded version come
/// from two different requests, and a bucket (or anything in front of it)
/// answering `metadata/PMC1.2.json` with `PMC3.4`'s body would otherwise file
/// article PMC3.4's PDF under PMC1.2's name. Two hops agreeing is the only
/// evidence there is.
#[must_use]
pub fn parse_version(
    adapter: &PmcOaAdapter,
    asked_for: &str,
    version: u64,
    body: &serde_json::Value,
) -> Option<PmcVersion> {
    let asked_for = normalise_pmcid(asked_for)?;
    let pmcid = normalise_pmcid(body.get("pmcid")?.as_str()?)?;
    if pmcid != asked_for {
        tracing::warn!(
            asked = %asked_for,
            answered = %pmcid,
            "pmc oa answered one version's URL with another version's record; \
             it is not offered as a candidate"
        );
        return None;
    }
    Some(PmcVersion {
        pmcid,
        version: body
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(version),
        is_manuscript: body.get("is_manuscript")?.as_bool()?,
        is_pmc_openaccess: body
            .get("is_pmc_openaccess")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        is_retracted: body
            .get("is_retracted")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        license_code: body
            .get("license_code")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .map(str::to_string),
        pdf_url: object_url(adapter, body, "pdf_url"),
        xml_url: object_url(adapter, body, "xml_url"),
    })
}

fn object_url(adapter: &PmcOaAdapter, body: &serde_json::Value, key: &str) -> Option<String> {
    body.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .and_then(|url| adapter.object_url(url))
        .map(|url| url.to_string())
}

/// The object key of an `s3://pmc-oa-opendata/<key>?md5=<digest>` URI, or
/// `None`.
///
/// Deliberately narrow: only an `s3://` URI with a bucket **and** a key, and
/// it returns the *key* rather than a URL. A rewrite that produced a host
/// would be a host the dataset chose; pairing the key with
/// [`PmcOaAdapter::object_url`]'s configured base keeps the authority with
/// the configuration and the object identity with the dataset.
#[must_use]
pub fn s3_object_key(raw: &str) -> Option<&str> {
    let rest = raw.trim().strip_prefix(S3_SCHEME)?;
    let (bucket, key) = rest.split_once('/')?;
    if bucket.trim().is_empty() {
        return None;
    }
    let key = key.split_once('?').map_or(key, |(key, _)| key);
    (!key.trim().is_empty()).then_some(key)
}

/// The MD5 digest of an `s3://…?md5=` URI, or `None` when it carries none.
#[must_use]
pub fn s3_object_digest(raw: &str) -> Option<&str> {
    let rest = raw.trim().strip_prefix(S3_SCHEME)?;
    let (_, key_and_query) = rest.split_once('/')?;
    let (_, query) = key_and_query.split_once('?')?;
    let digest = query.strip_prefix("md5=")?;
    (!digest.trim().is_empty()).then_some(digest)
}

/// The XML local name, with any namespace prefix stripped.
fn local_name(qualified: &[u8]) -> &[u8] {
    match qualified.iter().rposition(|b| *b == b':') {
        Some(at) => &qualified[at + 1..],
        None => qualified,
    }
}

/// The version this record describes, in [`crate::resolve`]'s vocabulary.
///
/// `None` when the file says nothing, which is [`crate::resolve::Version::Unstated`]
/// at the call site — not a version of record. PMC says `is_manuscript:
/// false` for a version of record, so `false` maps to one; but that mapping
/// lives here rather than in the ranker because it is PMC's vocabulary and
/// nobody else's.
#[must_use]
pub fn version_of(record: &PmcVersion) -> RepositoryVersion {
    if record.is_manuscript {
        RepositoryVersion::AuthorManuscript
    } else {
        RepositoryVersion::VersionOfRecord
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The measured listing for a PMCID with two stored versions, verbatim
    /// including the XML declaration and the namespace on the root.
    ///
    /// Two versions is the point: the version numbers run `1, 2` and the README
    /// says the higher number is **not** the better one, so a parser that took
    /// the last would pick on a rule that does not exist.
    const TWO_VERSIONS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>pmc-oa-opendata</Name><Prefix>PMC11370360.</Prefix><KeyCount>1</KeyCount><MaxKeys>1000</MaxKeys><Delimiter>/</Delimiter><IsTruncated>false</IsTruncated><CommonPrefixes><Prefix>PMC11370360.1/</Prefix></CommonPrefixes><CommonPrefixes><Prefix>PMC11370360.2/</Prefix></CommonPrefixes></ListBucketResult>"#;

    /// The measured `metadata/PMC7759461.1.json`, and the whole point of the
    /// module: `is_manuscript: false` on a Nature version of record, and the
    /// `pdf_url` in `s3://` form with an MD5 digest.
    const NUMPY_V1: &str = r#"{
      "pmcid": "PMC7759461",
      "version": 1,
      "pmid": 32939066,
      "doi": "10.1038/s41586-020-2649-2",
      "title": "Array programming with NumPy",
      "citation": "Nature. 2020 Sep 16;585(7825):357-362.",
      "is_pmc_openaccess": true,
      "is_manuscript": false,
      "is_historical_ocr": false,
      "is_retracted": false,
      "license_code": "CC BY",
      "xml_url": "s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.xml?md5=2ba36e61fa9eb4950c9daef9c9f58812",
      "text_url": "s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.txt?md5=fc83ba0d98bb1d742486396c8df6e4a8",
      "pdf_url": "s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.pdf?md5=4b3fc3a0a63cf2c7c4df3cc1b3d3d25e"
    }"#;

    fn json(body: &str) -> serde_json::Value {
        serde_json::from_str(body).expect("fixture is json")
    }

    /// The listing URL shape, and the fact that it is **delimited** — without
    /// the delimiter the bucket answers with eight million `<Key>` lines and no
    /// versions at all.
    #[test]
    fn the_listing_is_a_delimited_prefix_query() {
        let adapter = PmcOaAdapter::new();
        let url = adapter.versions_url("PMC11370360").expect("a pmcid");
        assert_eq!(url.host_str(), Some("pmc-oa-opendata.s3.amazonaws.com"));
        let query: std::collections::BTreeMap<String, String> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query["list-type"], "2");
        assert_eq!(
            query["prefix"], "PMC11370360.",
            "the trailing `.` on the prefix is what matches every version"
        );
        assert_eq!(
            query["delimiter"], "/",
            "the delimiter is what turns the objects into version prefixes"
        );
        assert!(
            url.as_str().contains("delimiter=%2F"),
            "and it is percent-encoded on the wire, because the URL is built by \
             `reqwest::Url` rather than spliced: {url}"
        );
    }

    /// The metadata URL shape, one file per version.
    #[test]
    fn the_metadata_url_is_one_file_per_version() {
        let adapter = PmcOaAdapter::new();
        assert_eq!(
            adapter
                .metadata_url("PMC7759461", 1)
                .map(|url| url.path().to_string()),
            Some("/metadata/PMC7759461.1.json".to_string())
        );
    }

    /// Only a PMCID may reach the bucket.
    ///
    /// The listing is a **prefix query**, so anything else would list a
    /// different work's versions under this one — the same class of bug as a
    /// DOI spliced into a path.
    #[test]
    fn only_a_pmcid_reaches_the_bucket() {
        for junk in [
            "",
            "PMC",
            "pmc",
            "10.1038/s41586-020-2649-2",
            "PMC7759461.1",
            "PMC 7759461",
            "PMCabc",
        ] {
            assert_eq!(
                normalise_pmcid(junk),
                None,
                "{junk:?} is not a PMCID and must not become a prefix query"
            );
        }
        assert_eq!(normalise_pmcid("pmc7759461").as_deref(), Some("PMC7759461"));
        assert_eq!(
            normalise_pmcid(" PMC7759461 ").as_deref(),
            Some("PMC7759461")
        );
    }

    /// Both versions come back, in the bucket's order, and **both** are
    /// returned. A parser that kept the last, or the first, would be applying
    /// a preference rule the README says does not exist.
    #[test]
    fn every_stored_version_is_returned_not_just_the_best_looking_one() {
        assert_eq!(
            parse_versions(TWO_VERSIONS),
            vec![
                PmcVersionRef {
                    pmcid: "PMC11370360".to_string(),
                    version: 1
                },
                PmcVersionRef {
                    pmcid: "PMC11370360".to_string(),
                    version: 2
                },
            ],
        );
    }

    /// Only `<Prefix>` entries are versions. A `<Key>` is an object, and
    /// `PMC…1/PMC…1.pdf` read as a version would name a file, not an article.
    #[test]
    fn an_object_key_is_not_a_version() {
        let body = "<ListBucketResult><Contents><Key>PMC1.1/PMC1.1.pdf</Key></Contents></ListBucketResult>";
        assert_eq!(parse_versions(body), Vec::<PmcVersionRef>::new());
        assert_eq!(
            parse_versions(
                "<ListBucketResult><CommonPrefixes><Prefix>oanda/</Prefix></CommonPrefixes></ListBucketResult>"
            ),
            Vec::<PmcVersionRef>::new(),
            "and neither is an unrelated bucket's prefix"
        );
        // Truncated mid-element is a partial answer, not a panic.
        assert_eq!(
            parse_versions("<ListBucketResult><CommonPrefixes>"),
            Vec::<PmcVersionRef>::new()
        );
    }

    /// The version claim: `is_manuscript` decides, and `false` is the version
    /// of record — because PMC says so per file, which is stronger than any
    /// record-level type a DOI registry carries.
    #[test]
    fn is_manuscript_is_the_version_evidence() {
        let vor = parse_version(&PmcOaAdapter::new(), "PMC7759461", 1, &json(NUMPY_V1))
            .expect("a version");
        assert!(!vor.is_manuscript);
        assert!(vor.is_pmc_openaccess);
        assert!(!vor.is_retracted);
        assert_eq!(vor.license_code.as_deref(), Some("CC BY"));
        assert_eq!(version_of(&vor), RepositoryVersion::VersionOfRecord);

        let mut manuscript = json(NUMPY_V1);
        manuscript["is_manuscript"] = serde_json::json!(true);
        manuscript["license_code"] = serde_json::json!("TDM");
        let am =
            parse_version(&PmcOaAdapter::new(), "PMC7759461", 2, &manuscript).expect("a version");
        assert_eq!(version_of(&am), RepositoryVersion::AuthorManuscript);
        assert_eq!(
            am.license_code.as_deref(),
            Some("TDM"),
            "a text-mining-only manuscript carries TDM, which is not a licence"
        );
    }

    /// A version record without `is_manuscript` is not read as `false`.
    ///
    /// `false` means "this is the version of record", and inferring it from an
    /// absent field would rank a file nobody has described as the article of
    /// record — the whole overclaim this slice exists to prevent.
    #[test]
    fn a_version_record_without_is_manuscript_is_not_read() {
        let mut body = json(NUMPY_V1);
        body.as_object_mut()
            .expect("an object")
            .remove("is_manuscript");
        assert_eq!(
            parse_version(&PmcOaAdapter::new(), "PMC7759461", 1, &body),
            None
        );

        for absent in ["is_manuscript", "pmcid"] {
            let mut body = json(NUMPY_V1);
            body.as_object_mut().expect("an object").remove(absent);
            assert_eq!(
                parse_version(&PmcOaAdapter::new(), "PMC7759461", 1, &body),
                None,
                "a version record without `{absent}` cannot be attributed to a \
                 stored version"
            );
        }
        // And a non-boolean `is_manuscript` is the same absence.
        let mut body = json(NUMPY_V1);
        body["is_manuscript"] = serde_json::json!("false");
        assert_eq!(
            parse_version(&PmcOaAdapter::new(), "PMC7759461", 1, &body),
            None
        );
    }

    /// The `s3://` rewrite, which is what makes a version fetchable at all —
    /// and the rebasing, which is what makes it *ours*.
    ///
    /// Asserted on the digest because the bucket's README says every object URL
    /// carries one, and a URL that drops it is fetched outside NCBI's integrity
    /// check — a small thing that is invisible in a plan line. Asserted on the
    /// host because the JSON names a bucket and the rewrite must not: a candidate
    /// whose host came from a third party's JSON is a stranger choosing this
    /// process's route.
    #[test]
    fn an_s3_object_url_is_rebased_onto_the_configured_bucket() {
        let adapter = PmcOaAdapter::new().with_base_url("http://127.0.0.1:8080/bucket");
        let raw = "s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.pdf?md5=4b3fc3a0a63cf2c7c4df3cc1b3d3d25e";
        assert_eq!(
            adapter
                .object_url(raw)
                .map(|url| url.to_string())
                .as_deref(),
            Some(
                "http://127.0.0.1:8080/PMC7759461.1/PMC7759461.1.pdf?md5=4b3fc3a0a63cf2c7c4df3cc1b3d3d25e"
            ),
            "the key and the digest come from the JSON; the host comes from the \
             configuration"
        );

        // Production shape, on the real base.
        assert_eq!(
            PmcOaAdapter::new()
                .object_url(raw)
                .map(|url| url.to_string())
                .as_deref(),
            Some(
                "https://pmc-oa-opendata.s3.amazonaws.com/PMC7759461.1/PMC7759461.1.pdf?md5=4b3fc3a0a63cf2c7c4df3cc1b3d3d25e"
            )
        );

        // A JSON field naming a *different* bucket still lands on ours.
        assert_eq!(
            PmcOaAdapter::new()
                .with_base_url("http://127.0.0.1:8080/bucket")
                .object_url("s3://some-other-bucket/PMC1.1/x.pdf?md5=ab")
                .map(|url| url.to_string())
                .as_deref(),
            Some("http://127.0.0.1:8080/PMC1.1/x.pdf?md5=ab"),
            "the dataset supplies an object identity, not a network target"
        );

        // A key with a slash in it survives whole, and the digest is not treated
        // as part of the key.
        assert_eq!(
            s3_object_key("s3://pmc-oa-opendata/PMC1.1/figs/a.jpg?md5=ab"),
            Some("PMC1.1/figs/a.jpg")
        );
        assert_eq!(
            s3_object_digest("s3://pmc-oa-opendata/PMC1.1/x.pdf?md5=ab"),
            Some("ab")
        );
        assert_eq!(s3_object_digest("s3://pmc-oa-opendata/PMC1.1/x.pdf"), None);
    }

    /// A rewrite that guessed would put a URL on the wire nobody vouched for,
    /// so only `s3://` URIs with a bucket **and** a key are rewritten.
    #[test]
    fn nothing_but_an_s3_uri_with_a_key_is_rewritten() {
        for junk in [
            "",
            "   ",
            "https://example.org/x.pdf",
            "s3://",
            "s3://pmc-oa-opendata",
            "s3://pmc-oa-opendata/",
            "s3:///PMC1.1/PMC1.1.pdf",
            "file:///etc/passwd",
        ] {
            assert_eq!(s3_object_key(junk), None, "{junk:?} is not an s3 object");
            assert_eq!(
                PmcOaAdapter::new().object_url(junk),
                None,
                "{junk:?} yields no candidate URL"
            );
        }
    }

    /// A version with no `pdf_url` is real, not a parse failure — the Seurat
    /// paper is measured that way — so the record still parses and the PDF
    /// simply is not there.
    #[test]
    fn a_version_with_no_pdf_still_parses() {
        let body = r#"{"pmcid":"PMC8238499","version":1,"is_manuscript":false,
            "is_pmc_openaccess":true,"is_retracted":false,"license_code":"CC BY",
            "xml_url":"s3://pmc-oa-opendata/PMC8238499.1/PMC8238499.1.xml?md5=18308a8abd56e47aff91a2fc795ff140"}"#;
        let record =
            parse_version(&PmcOaAdapter::new(), "PMC8238499", 1, &json(body)).expect("a version");
        assert_eq!(record.pdf_url, None, "licensed the XML, not the PDF");
        assert!(record.xml_url.is_some());
    }

    /// The `is_manuscript`-independent OA flag, and a `null` licence.
    ///
    /// `PMC7123456.1` is measured with `license_code: null` and a PDF: in the
    /// dataset, not necessarily under a licence the JSON states.
    #[test]
    fn an_open_access_version_may_state_no_licence_code() {
        let body = r#"{"pmcid":"PMC7123456","version":1,"is_manuscript":false,
            "is_pmc_openaccess":true,"license_code":null,
            "pdf_url":"s3://pmc-oa-opendata/PMC7123456.1/PMC7123456.1.pdf?md5=27df2fd6d9febc7211b2a85cca8ded00"}"#;
        let record =
            parse_version(&PmcOaAdapter::new(), "PMC7123456", 1, &json(body)).expect("a version");
        assert!(record.is_pmc_openaccess);
        assert_eq!(record.license_code, None);
        assert!(record.pdf_url.is_some());
    }
}
