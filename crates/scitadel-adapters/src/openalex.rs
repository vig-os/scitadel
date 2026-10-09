use async_trait::async_trait;
use reqwest::Client;

use scitadel_core::config::OpenAlexAuth;
use scitadel_core::error::CoreError;
use scitadel_core::models::{CandidatePaper, Paper, normalize_doi, validate_doi};
use scitadel_core::ports::SourceAdapter;

pub const OPENALEX_API_URL: &str = "https://api.openalex.org/works";

/// Which OpenAlex field a search should target (#210).
///
/// OpenAlex's broad `search=` param is fulltext relevance: it drowns
/// exact-title queries in topically-adjacent noise (issue's Schwarz-1978
/// / bootstrap / FISTA repros). `filter=title.search:` scores against
/// the title only, so an exact title reliably surfaces its target near
/// the top. Neither is universally better — a topic query wants the
/// broad path, an exact title wants the narrow one — so this enum is
/// what the CLI flag and MCP param carry through to the adapter.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchField {
    /// `search=` — broad fulltext relevance. The pre-#210 default; kept
    /// as the default here so existing callers see no change.
    #[default]
    Any,
    /// `filter=title.search:` — title-only relevance. Preferred when the
    /// query is a paper's title.
    Title,
    /// Title-first with a broad fallback: if `filter=title.search:` runs
    /// dry, or returns fewer than `max_results`, the broad `search=` fills
    /// the tail. The union is de-duplicated by OpenAlex work id, order
    /// preserved (title hits first). This is the "cannot lose", "search
    /// still finds topics" mode.
    Auto,
}

impl SearchField {
    /// Parse a CLI/MCP field string. Case-insensitive; `""` means default.
    ///
    /// Returns the parsed variant or the offending input for a clear
    /// error message on the boundary.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "any" | "broad" | "all" | "search" | "fulltext" => Ok(Self::Any),
            "title" => Ok(Self::Title),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "unknown search field '{other}'; valid: any | title | auto"
            )),
        }
    }

    /// Machine-readable name (round-trips through `parse`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Title => "title",
            Self::Auto => "auto",
        }
    }
}

/// Longest error body excerpt echoed back to the user. Enough for
/// OpenAlex's "Insufficient budget…" text without dumping a whole page.
const MAX_ERROR_EXCERPT: usize = 200;

pub struct OpenAlexAdapter {
    auth: OpenAlexAuth,
    timeout: f64,
    base_url: String,
    /// Which OpenAlex relevance mode the trait-level `search()` uses.
    /// Defaults to `Any` (the pre-#210 broad `search=`); the federated
    /// orchestrator honours whatever the CLI/MCP caller passed via
    /// `with_default_field` on adapter construction.
    default_field: SearchField,
}

/// Build the query string for an OpenAlex request.
///
/// Both credentials go on *every* request: `mailto` for the polite pool
/// and `api_key` for the metered quota. Without the key OpenAlex bills
/// the call against a shared per-IP daily budget and starts answering
/// 429 once that is spent (#212). Empty credentials are omitted rather
/// than sent blank.
fn query_params(auth: &OpenAlexAuth, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = extra
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    if !auth.email.is_empty() {
        params.push(("mailto".into(), auth.email.clone()));
    }
    if !auth.api_key.is_empty() {
        params.push(("api_key".into(), auth.api_key.clone()));
    }
    params
}

/// Turn an OpenAlex error body into a short human-readable clause.
///
/// OpenAlex answers failures with `{"error": …, "message": …}`; anything
/// else is echoed verbatim. Truncated, and returns an empty string when
/// there is nothing useful to say, so callers can append it blindly.
fn error_detail(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let message = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            ["message", "error"]
                .iter()
                .find_map(|k| v.get(*k).and_then(|m| m.as_str()).map(str::to_string))
        })
        .unwrap_or_else(|| body.to_string());

    let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if message.is_empty() {
        return String::new();
    }
    if message.chars().count() > MAX_ERROR_EXCERPT {
        let truncated: String = message.chars().take(MAX_ERROR_EXCERPT).collect();
        format!(" — {truncated}…")
    } else {
        format!(" — {message}")
    }
}

impl OpenAlexAdapter {
    pub fn new(auth: OpenAlexAuth, timeout: f64) -> Self {
        Self {
            auth,
            timeout,
            base_url: OPENALEX_API_URL.to_string(),
            default_field: SearchField::default(),
        }
    }

    /// Point the adapter at a different `/works` endpoint. Test seam for
    /// the HTTP-behaviour suite; production always uses the default.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Set the relevance mode this adapter's `SourceAdapter::search`
    /// impl uses under the federated orchestrator (#210). Builders pass
    /// the value the CLI flag / MCP `field` param resolved to.
    #[must_use]
    pub fn with_default_field(mut self, field: SearchField) -> Self {
        self.default_field = field;
        self
    }

    /// Fetch the full Work JSON for a single paper by its short OpenAlex
    /// id (e.g. `W2741809807`). Returns the raw API payload so callers
    /// can pluck whatever fields they need (title, `referenced_works`,
    /// authorships, etc.). Used by the citation-graph service (#59).
    pub async fn fetch_work_by_id(
        &self,
        openalex_id: &str,
    ) -> Result<serde_json::Value, CoreError> {
        let url = format!("{}/{openalex_id}", self.base_url);
        self.fetch_from(&url, &[]).await
    }

    /// Fetch a batch of Works by their short OpenAlex ids (W…) in one
    /// request. OpenAlex's `openalex_id:W1|W2|…` filter accepts up to
    /// 50 entries per call; the caller is responsible for chunking
    /// larger lists. Returns the deserialised `results` array.
    pub async fn fetch_works_by_ids(
        &self,
        ids: &[String],
    ) -> Result<Vec<serde_json::Value>, CoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        if ids.len() > 50 {
            return Err(CoreError::Adapter(
                "openalex".into(),
                format!(
                    "fetch_works_by_ids requires <=50 ids per call, got {}",
                    ids.len()
                ),
            ));
        }
        let filter = format!("openalex_id:{}", ids.join("|"));
        let payload = self
            .fetch_from(
                &self.base_url,
                &[("filter", filter.as_str()), ("per_page", "50")],
            )
            .await?;
        Ok(payload
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Fetch Works that cite the given paper (the reverse-citation
    /// direction). `limit` defaults to 25 and is capped at 200 by the
    /// OpenAlex API.
    pub async fn fetch_cited_by(
        &self,
        openalex_id: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, CoreError> {
        let filter = format!("cites:{openalex_id}");
        let per_page = limit.clamp(1, 200).to_string();
        let payload = self
            .fetch_from(
                &self.base_url,
                &[("filter", filter.as_str()), ("per_page", per_page.as_str())],
            )
            .await?;
        Ok(payload
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// Resolve a DOI to a `Paper` via OpenAlex's `/works/doi:<doi>`
    /// endpoint (#210). Returns `Ok(None)` for a well-formed DOI that
    /// OpenAlex doesn't know about (HTTP 404); any other failure is
    /// surfaced verbatim so the caller can report a real error.
    ///
    /// The DOI is validated + canonicalised (URL prefix stripped, case
    /// lowered) before the request — a malformed DOI never touches the
    /// wire.
    pub async fn fetch_paper_by_doi(&self, doi: &str) -> Result<Option<Paper>, CoreError> {
        if !validate_doi(doi) {
            return Err(CoreError::Adapter(
                "openalex".into(),
                format!("invalid DOI: {doi}"),
            ));
        }
        let canonical = normalize_doi(doi);
        let url = format!("{}/doi:{canonical}", self.base_url);
        match self.fetch_from(&url, &[]).await {
            Ok(work) => Ok(Some(work_to_paper(&work))),
            Err(CoreError::Adapter(_, msg)) if msg.starts_with("HTTP 404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Search OpenAlex with an explicit relevance mode (#210).
    ///
    /// - `SearchField::Any` — the legacy `search=` (fulltext) path.
    /// - `SearchField::Title` — `filter=title.search:` for titles.
    /// - `SearchField::Auto` — title-first, broad-fallback: if title
    ///   returns fewer than `max_results`, the tail is padded from the
    ///   broad path, deduplicated by OpenAlex work id (title hits win).
    ///   A title-leg failure (any HTTP error) also falls through to the
    ///   broad leg — the intent of `Auto` is "cannot lose", so a syntax
    ///   quirk in the title filter must not blank a valid federated
    ///   search.
    pub async fn search_field(
        &self,
        query: &str,
        max_results: usize,
        field: SearchField,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        let per_page = max_results.to_string();
        match field {
            SearchField::Any => {
                let data = self
                    .fetch_from(
                        &self.base_url,
                        &[("search", query), ("per_page", per_page.as_str())],
                    )
                    .await?;
                Ok(candidates_from(&data))
            }
            SearchField::Title => {
                let filter = format!("title.search:{}", title_filter_value(query));
                let data = self
                    .fetch_from(
                        &self.base_url,
                        &[("filter", filter.as_str()), ("per_page", per_page.as_str())],
                    )
                    .await?;
                Ok(candidates_from(&data))
            }
            SearchField::Auto => {
                // Title first — the whole point of #210 is that an exact
                // title reliably surfaces its target here.
                let filter = format!("title.search:{}", title_filter_value(query));
                let title_result = self
                    .fetch_from(
                        &self.base_url,
                        &[("filter", filter.as_str()), ("per_page", per_page.as_str())],
                    )
                    .await;
                let mut merged = match title_result {
                    Ok(data) => candidates_from(&data),
                    Err(e) => {
                        // The point of `Auto` is "cannot lose" — a
                        // title-leg 400/429/timeout falls through to the
                        // broad leg rather than blanking the search.
                        // (#210 review) Logged so it stays diagnosable.
                        tracing::warn!(
                            error = %e,
                            "openalex title.search leg failed under Auto; falling through to broad search=",
                        );
                        Vec::new()
                    }
                };
                if merged.len() >= max_results {
                    return Ok(merged);
                }
                // Fall back to broad fulltext for the remainder. A
                // failure of the fallback leg does not cancel the title
                // hits we already have — a broad-fetch 429 is worse than
                // a shorter list. But if title also produced nothing,
                // surface the broad-leg error so the search-run record
                // has a real reason for "0 candidates" rather than a
                // silent Ok(vec![]).
                let broad_result = self
                    .fetch_from(
                        &self.base_url,
                        &[("search", query), ("per_page", per_page.as_str())],
                    )
                    .await;
                let broad = match broad_result {
                    Ok(d) => candidates_from(&d),
                    Err(e) if merged.is_empty() => return Err(e),
                    Err(_) => Vec::new(),
                };
                let mut seen: std::collections::HashSet<String> = merged
                    .iter()
                    .filter_map(|c| c.openalex_id.clone())
                    .collect();
                for mut c in broad {
                    if merged.len() >= max_results {
                        break;
                    }
                    if let Some(ref oa) = c.openalex_id
                        && !seen.insert(oa.clone())
                    {
                        continue;
                    }
                    c.rank = Some((merged.len() as i32) + 1);
                    merged.push(c);
                }
                Ok(merged)
            }
        }
    }

    /// Single HTTP path for every OpenAlex call, so the credentials and
    /// the status check can't be forgotten at one call site (they were:
    /// `search` used to skip the status check entirely and reported a
    /// 429 body as zero results — #212).
    async fn fetch_from(
        &self,
        url: &str,
        extra: &[(&str, &str)],
    ) -> Result<serde_json::Value, CoreError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs_f64(self.timeout))
            .build()
            .map_err(adapter_err)?;

        let resp = client
            .get(url)
            .query(&query_params(&self.auth, extra))
            .send()
            .await
            .map_err(adapter_err)?;

        let status = resp.status();
        if !status.is_success() {
            // Never interpolate the request URL: it carries `api_key`,
            // and this string lands in logs and the search-run record.
            let body = resp.text().await.unwrap_or_default();
            return Err(CoreError::Adapter(
                "openalex".into(),
                format!("HTTP {status}{}", error_detail(&body)),
            ));
        }

        resp.json().await.map_err(adapter_err)
    }
}

fn adapter_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Adapter("openalex".into(), e.to_string())
}

/// Extract the short OpenAlex id (`W2741809807`) from either a full URL
/// (`https://openalex.org/W2741809807`) or a bare id. Returns `None` if
/// the input doesn't end in a `W…` token.
#[must_use]
pub fn short_openalex_id(maybe_url: &str) -> Option<String> {
    let last = maybe_url.rsplit('/').next().unwrap_or(maybe_url);
    if last.starts_with('W') && last.len() > 1 {
        Some(last.to_string())
    } else {
        None
    }
}

/// Build a `Paper` (canonical record) from an OpenAlex Work JSON.
/// Useful when materialising referenced works as DB rows.
#[must_use]
pub fn work_to_paper(work: &serde_json::Value) -> scitadel_core::models::Paper {
    use scitadel_core::models::{Paper, PaperId};
    let candidate = work_to_candidate(work, 0);
    let mut paper = Paper::new(candidate.title);
    if !candidate.authors.is_empty() {
        paper.authors = candidate
            .authors
            .iter()
            .map(scitadel_core::untrusted::UntrustedText::ours)
            .collect();
    }
    paper.r#abstract = candidate.r#abstract;
    paper.doi = candidate.doi;
    paper.openalex_id.clone_from(&candidate.openalex_id);
    paper.pubmed_id = candidate.pubmed_id;
    paper.year = candidate.year;
    paper.journal = candidate.journal;
    paper.url = candidate.url;
    if let Some(short) = candidate.openalex_id {
        // Use the short OpenAlex id as the canonical paper id so
        // citation rows have a stable target_paper_id even when the
        // metadata gets re-fetched later.
        paper.id = PaperId::from(short);
    }
    paper
}

#[async_trait]
impl SourceAdapter for OpenAlexAdapter {
    fn name(&self) -> &str {
        "openalex"
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<CandidatePaper>, CoreError> {
        // The trait method honours whatever field mode the builder
        // configured — the default is `Any` (pre-#210 broad `search=`)
        // so unchanged call sites see no behaviour change. Callers that
        // need a per-call override still have `search_field` directly.
        self.search_field(query, max_results, self.default_field)
            .await
    }
}

/// Neutralise OpenAlex filter-syntax metachars in a `title.search:`
/// value so a title with a comma or a pipe survives the round trip
/// (#210 review). Documented + probe-verified metachars:
///
/// - `,` — filter separator; unescaped comma returns HTTP 400
///   "A filter value contains an unescaped comma".
/// - `|` — OR (`Bootstrap|jackknife` silently becomes
///   `Bootstrap OR jackknife`).
/// - `!` — NOT (a `!` starting any token silently negates it, per
///   probe: `Bootstrap !jackknife` → `Bootstrap AND NOT jackknife`).
/// - `"` — phrase boundary (unbalanced quotes fragment the value).
///
/// Each is replaced with a space; runs of whitespace are collapsed and
/// the value is trimmed. Nothing else is touched, so meaningful title
/// characters (`:` `;` `(` `)` `-` `.` etc.) pass through verbatim —
/// probes confirm the API tokenises them cleanly.
#[must_use]
pub fn title_filter_value(query: &str) -> String {
    let sanitised: String = query
        .chars()
        .map(|c| match c {
            ',' | '|' | '!' | '"' => ' ',
            _ => c,
        })
        .collect();
    sanitised.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decode a `/works` response into ranked candidates. Shared between
/// every `SearchField::*` branch so the parsing / rank-assignment logic
/// lives in one place.
fn candidates_from(data: &serde_json::Value) -> Vec<CandidatePaper> {
    let works = data
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    works
        .iter()
        .enumerate()
        .map(|(i, work)| work_to_candidate(work, (i + 1) as i32))
        .collect()
}

fn work_to_candidate(work: &serde_json::Value, rank: i32) -> CandidatePaper {
    let openalex_id = work.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let short_id = openalex_id.rsplit('/').next().unwrap_or("").to_string();

    let title = work
        .get("title")
        .or_else(|| work.get("display_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let authors: Vec<String> = work
        .get("authorships")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.get("author")
                        .and_then(|au| au.get("display_name"))
                        .and_then(|n| n.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    let abstract_text = work
        .get("abstract_inverted_index")
        .and_then(|v| v.as_object())
        .map(reconstruct_abstract)
        .unwrap_or_default();

    let doi_url = work.get("doi").and_then(|v| v.as_str()).unwrap_or("");
    let doi = if doi_url.is_empty() {
        None
    } else {
        Some(doi_url.replace("https://doi.org/", ""))
    };

    let year = work
        .get("publication_year")
        .and_then(|v| v.as_i64())
        .map(|y| y as i32);

    let journal = work
        .get("primary_location")
        .and_then(|loc| loc.get("source"))
        .and_then(|src| src.get("display_name"))
        .and_then(|n| n.as_str())
        .map(String::from);

    let pmid = work
        .get("ids")
        .and_then(|ids| ids.get("pmid"))
        .and_then(|v| v.as_str())
        .and_then(|url| url.rsplit('/').next())
        .map(String::from);

    CandidatePaper {
        source: "openalex".into(),
        source_id: short_id.clone(),
        title: title.clone(),
        authors,
        r#abstract: abstract_text,
        doi,
        openalex_id: Some(short_id),
        pubmed_id: pmid,
        year,
        journal,
        url: Some(openalex_id.to_string()),
        rank: Some(rank),
        raw_data: work.clone(),
        ..CandidatePaper::new("openalex", "", &title)
    }
}

/// What an OpenAlex `/works` record says the work is, for the pre-fetch
/// identity check (ADR-007 §3).
///
/// **One parser, two callers.** The download ladder reads it off the envelope
/// its OpenAlex leg already fetched, and [`crate::identity_chain`] reads it off
/// a by-DOI lookup when no leg asked OpenAlex — and the second reader is a
/// preprint or a repository DOI, which is exactly where a second, subtly
/// different copy of this would diverge and quietly report `unverified`. So it
/// lives here, next to the module that defines the shape, and both call it.
///
/// Read from the record rather than re-requested: `title` and
/// `publication_year` are top-level fields, and the first authorship's
/// `author.display_name` is the first author in the order the registry lists
/// them. A missing field yields `None` rather than a placeholder, because an
/// absent fact must read as [`crate::identity::Verdict::Unverified`] and never
/// as agreement.
#[must_use]
pub fn work_identity(work: &serde_json::Value) -> crate::identity::WorkIdentity {
    crate::identity::WorkIdentity {
        title: work
            .get("title")
            .or_else(|| work.get("display_name"))
            .and_then(serde_json::Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .map(str::to_string),
        year: work
            .get("publication_year")
            .and_then(serde_json::Value::as_i64)
            .and_then(|year| i32::try_from(year).ok()),
        first_author: work
            .get("authorships")
            .and_then(serde_json::Value::as_array)
            .and_then(|authorships| authorships.first())
            .and_then(|authorship| authorship.get("author"))
            .and_then(|author| author.get("display_name"))
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string),
    }
}

/// The `/works/doi:<doi>` URL, percent-encoded.
///
/// The encoded and the raw spellings both answer `200` against the live API
/// (measured 2026-10), so the encoding is a correctness choice: it keeps a
/// segmented identifier's `/` inside one segment and stops a `#` or `?` in a
/// suffix from truncating the path. `Url`'s own encoder is used rather than a
/// hand-spliced string for the reason [`crate::crossref`] gives.
///
/// # Errors
///
/// Only if `base_url` will not parse.
pub fn doi_works_url(
    base_url: &str,
    doi: &str,
) -> Result<reqwest::Url, scitadel_core::error::CoreError> {
    use scitadel_core::error::CoreError;
    let mut url = reqwest::Url::parse(base_url)
        .map_err(|e| CoreError::Adapter("openalex".into(), format!("invalid base URL: {e}")))?;
    url.path_segments_mut()
        .map_err(|()| {
            CoreError::Adapter(
                "openalex".into(),
                "the /works base URL cannot be a base".to_string(),
            )
        })?
        .extend([format!("doi:{}", scitadel_core::models::normalize_doi(doi))]);
    Ok(url)
}

// ============================================================================
// ADR-007 §3's resolve-then-rank: the fields the identity pass never read.
// ============================================================================

/// The version OpenAlex declares for a work, read from `type`.
///
/// OpenAlex maintains `type` itself (`article`, `preprint`, `book`, `dataset`,
/// …) and mirrors Crossref's word in `type_crossref`. `type` is read first and
/// `type_crossref` second, because OpenAlex's own classification is the one
/// OpenAlex indexes on — a work it files as `article` is the version of record
/// in OpenAlex's own account, whatever Crossref's deposit says.
///
/// `None` when neither field is present or is not a string: an absent `type`
/// is a gap in OpenAlex's record, and [`crate::resolve`] falls back to the next
/// registry or to the DOI-prefix inference and **records that it did**.
#[must_use]
pub fn type_signal(
    work: &serde_json::Value,
) -> Option<(&'static str, crate::registry::RegistryType)> {
    for (field, raw) in [
        ("type", work.get("type")),
        ("type_crossref", work.get("type_crossref")),
    ] {
        if let Some(word) = raw
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|w| !w.is_empty())
        {
            let classified =
                crate::registry::RegistryType::of(word, crate::registry::Dialect::OpenAlex);
            return Some((field, classified));
        }
    }
    None
}

/// One OA location OpenAlex names, with the two facts the ranker needs from it.
///
/// `version` is OpenAlex's own word for *which* rendering this location is —
/// `publishedVersion` (the version of record), `acceptedVersion` (the author
/// manuscript) or `submittedVersion` (a preprint). It is the single best
/// version signal available anywhere in this metadata pass, and it is
/// **per-location**: the same work routinely has a `publishedVersion` at the
/// publisher and a `submittedVersion` at arXiv, and treating the work's version
/// as the location's version would rank both identically and re-introduce
/// exactly the bug the ladder exists to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OaLocation {
    /// `pdf_url` when the location has one, else `landing_page_url`.
    pub url: String,
    /// `pdf_url` specifically, kept apart because ADR-007 §3's fetch-order step
    /// treats a landing page and a PDF as different things and because
    /// `looks_like_pdf` decides the stored extension from this one.
    pub pdf_url: Option<String>,
    /// OpenAlex's `version` word: `publishedVersion`, `acceptedVersion` or
    /// `submittedVersion`.
    pub version_word: Option<String>,
    /// The licence OpenAlex records for this location, when it records one.
    pub licence: Option<String>,
    /// `landing_page_url`, kept so a location with no PDF still yields a
    /// candidate (an HTML landing page is a candidate the ladder can take).
    pub landing_page_url: Option<String>,
    /// `locations[].id`, **OpenAlex's own typing of what the location is**.
    ///
    /// Not a decoration and not an OpenAlex primary key: it is a *prefixed
    /// identifier*, and the prefix names the scheme. Measured 2026-10-09 on
    /// `10.1101/2025.06.14.659707`, one work with three locations:
    ///
    /// | `id` | `landing_page_url` |
    /// |---|---|
    /// | `doi:10.1101/2025.06.14.659707` | `https://doi.org/10.1101/…` |
    /// | `pmid:40667369` | `https://pubmed.ncbi.nlm.nih.gov/40667369` |
    /// | `pmh:oai:pubmedcentral.nih.gov:12262699` | `https://www.ncbi.nlm.nih.gov/pmc/articles/12262699` |
    ///
    /// `pmid:` is the field that says *this is a PubMed record about the work*,
    /// where `pmh:` says the PMC copy and `doi:` says the DOI resolver. It is
    /// the only thing in the location that distinguishes them, and it is
    /// OpenAlex's own vocabulary rather than a host this module recognises — the
    /// distinction #298 turns on.
    pub id: Option<String>,
    /// `is_oa`, when the location declares it.
    pub is_oa: Option<bool>,
    /// Is this the `best_oa_location` rather than an entry from `locations[]`?
    ///
    /// Carried because the two deserve different treatment and only this field
    /// tells them apart: `best_oa_location` is OpenAlex's assertion that *this*
    /// is the open copy, while `locations[]` is every location OpenAlex
    /// indexes — OA or not. See [`crate::resolve::LicenceFloor`].
    pub is_best: bool,
}

impl OaLocation {
    /// Does this location carry a URL we could fetch?
    ///
    /// `pdf_url` first, then `landing_page_url`: the full text is the point,
    /// and a location with neither has nothing to offer. A location with neither
    /// is **omitted** rather than recorded as an empty candidate, because an
    /// unrankable candidate is supposed to mean "we found a place and could not
    /// place it", not "we found a place with no URL in it".
    #[must_use]
    pub fn fetchable(&self) -> Option<&str> {
        self.pdf_url.as_deref().or(self.landing_page_url.as_deref())
    }
}

/// Every OA location OpenAlex names for a work, `locations[]` plus
/// `best_oa_location`.
///
/// ADR-007 §3 names "OpenAlex (all `locations[]`)" explicitly — **all**, not
/// just the best one — and that is the whole point of the metadata pass: the
/// best location is whichever OpenAlex ranked first, and OpenAlex's ranking
/// knows nothing about version of record. A work with a publisher VoR at
/// location 4 and a preprint at location 1 has two candidates, and only the
/// ranker can say which is which.
///
/// `best_oa_location` is deduplicated against `locations[]` by URL rather than
/// appended unconditionally: OpenAlex usually lists the best location inside
/// `locations[]` too, and a duplicate would be two candidates for one file.
#[must_use]
pub fn oa_locations(work: &serde_json::Value) -> Vec<OaLocation> {
    let read = |value: &serde_json::Value| -> Option<OaLocation> {
        let landing = value
            .get("landing_page_url")
            .and_then(serde_json::Value::as_str);
        let pdf = value.get("pdf_url").and_then(serde_json::Value::as_str);
        if pdf.is_none() && landing.is_none() {
            return None;
        }
        Some(OaLocation {
            is_best: false,
            url: pdf.or(landing).unwrap_or_default().to_string(),
            version_word: value
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|word| !word.is_empty())
                .map(str::to_string),
            licence: value
                .get("license")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|licence| !licence.is_empty())
                .map(str::to_string),
            landing_page_url: landing.map(str::to_string),
            is_oa: value.get("is_oa").and_then(serde_json::Value::as_bool),
            id: value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            pdf_url: pdf.map(str::to_string),
        })
    };
    let mut out: Vec<OaLocation> = work
        .get("locations")
        .and_then(serde_json::Value::as_array)
        .map(|locations| locations.iter().filter_map(read).collect())
        .unwrap_or_default();
    if let Some(mut best) = work.get("best_oa_location").and_then(read) {
        best.is_best = true;
        if let Some(seen) = out.iter_mut().find(|seen| seen.url == best.url) {
            // The same URL under both keys: keep the `best_oa_location`
            // spelling and its credit, so there is one candidate for one file.
            seen.is_best = true;
            seen.version_word = seen.version_word.take().or(best.version_word);
            seen.licence = seen.licence.take().or(best.licence);
            seen.landing_page_url = seen.landing_page_url.take().or(best.landing_page_url);
            seen.is_oa = seen.is_oa.or(best.is_oa);
            // The typed identifier is the same fact twice when the URL is, so
            // either spelling is the right one — and the *prefix* is what
            // `crate::resolve` reads the role from, so losing it here would lose
            // the role.
            seen.id = seen.id.take().or(best.id);
            if seen.pdf_url.is_none() {
                seen.pdf_url = best.pdf_url.take();
                seen.url = seen.url.clone();
            }
        } else {
            out.push(best);
        }
    }
    out
}

/// Whether OpenAlex declares this work open access at all.
///
/// A *work*-level flag, not a location's: `open_access.is_oa` says the work has
/// some open copy somewhere. It cannot say which version that copy is, so it is
/// used only as the licence floor for a location OpenAlex has already said is
/// OA — never as a version claim.
#[must_use]
pub fn declares_open_access(work: &serde_json::Value) -> bool {
    work.get("open_access")
        .and_then(|oa| oa.get("is_oa"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Reconstruct abstract text from OpenAlex inverted index format.
pub fn reconstruct_abstract(inverted_index: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut word_positions: Vec<(i64, &str)> = Vec::new();

    for (word, positions) in inverted_index {
        if let Some(arr) = positions.as_array() {
            for pos in arr {
                if let Some(p) = pos.as_i64() {
                    word_positions.push((p, word));
                }
            }
        }
    }

    word_positions.sort_by_key(|(pos, _)| *pos);
    word_positions
        .iter()
        .map(|(_, word)| *word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert an OpenAlex work dict to Paper constructor kwargs (for citation fetching).
pub fn work_to_paper_dict(work: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let candidate = work_to_candidate(work, 0);
    let mut map = serde_json::Map::new();
    map.insert("title".into(), serde_json::Value::String(candidate.title));
    map.insert(
        "authors".into(),
        serde_json::Value::Array(
            candidate
                .authors
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    map.insert(
        "abstract".into(),
        serde_json::Value::String(candidate.r#abstract),
    );
    if let Some(doi) = candidate.doi {
        map.insert("doi".into(), serde_json::Value::String(doi));
    }
    if let Some(id) = candidate.openalex_id {
        map.insert("openalex_id".into(), serde_json::Value::String(id));
    }
    if let Some(pmid) = candidate.pubmed_id {
        map.insert("pubmed_id".into(), serde_json::Value::String(pmid));
    }
    if let Some(year) = candidate.year {
        map.insert("year".into(), serde_json::Value::Number(year.into()));
    }
    if let Some(journal) = candidate.journal {
        map.insert("journal".into(), serde_json::Value::String(journal));
    }
    if let Some(url) = candidate.url {
        map.insert("url".into(), serde_json::Value::String(url));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params_map(auth: &OpenAlexAuth, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        query_params(auth, extra)
    }

    fn lookup<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn query_params_send_the_api_key_on_every_request() {
        let auth = OpenAlexAuth {
            email: "me@example.org".into(),
            api_key: "oa-key-123".into(),
        };
        let params = params_map(&auth, &[("search", "DOTA lutetium"), ("per_page", "5")]);

        assert_eq!(lookup(&params, "api_key"), Some("oa-key-123"));
        assert_eq!(lookup(&params, "mailto"), Some("me@example.org"));
        assert_eq!(lookup(&params, "search"), Some("DOTA lutetium"));
        assert_eq!(lookup(&params, "per_page"), Some("5"));
    }

    #[test]
    fn query_params_keep_the_email_as_the_polite_pool_mailto() {
        // Regression guard for #212: the email must never be sent as the
        // API key, which is what the old single-field config implied.
        let auth = OpenAlexAuth {
            email: "me@example.org".into(),
            api_key: String::new(),
        };
        let params = params_map(&auth, &[]);
        assert_eq!(lookup(&params, "mailto"), Some("me@example.org"));
        assert_eq!(lookup(&params, "api_key"), None);
    }

    #[test]
    fn query_params_omit_blank_credentials() {
        let params = params_map(&OpenAlexAuth::default(), &[("per_page", "50")]);
        assert_eq!(params, vec![("per_page".to_string(), "50".to_string())]);
    }

    #[test]
    fn query_params_cover_the_filter_endpoints() {
        let auth = OpenAlexAuth {
            email: String::new(),
            api_key: "k".into(),
        };
        // cited_by / snowball / batch-fetch all funnel through `filter`.
        for extra in [
            vec![("filter", "cites:W1"), ("per_page", "25")],
            vec![("filter", "openalex_id:W1|W2"), ("per_page", "50")],
            vec![],
        ] {
            let params = params_map(&auth, &extra);
            assert_eq!(
                lookup(&params, "api_key"),
                Some("k"),
                "api_key missing for {extra:?}"
            );
        }
    }

    #[test]
    fn error_detail_extracts_the_openalex_message() {
        let body = r#"{"error":"Insufficient budget","message":"This request has no API key."}"#;
        assert_eq!(error_detail(body), " — This request has no API key.");
    }

    #[test]
    fn error_detail_falls_back_to_the_error_field_then_the_raw_body() {
        assert_eq!(
            error_detail(r#"{"error":"Insufficient budget"}"#),
            " — Insufficient budget"
        );
        assert_eq!(error_detail("upstream is down"), " — upstream is down");
        assert_eq!(error_detail("   "), "");
    }

    #[test]
    fn error_detail_truncates_a_long_body() {
        let detail = error_detail(&"x".repeat(1000));
        assert!(detail.ends_with('…'));
        assert_eq!(detail.chars().count(), MAX_ERROR_EXCERPT + 4); // " — " + text + "…"
    }

    #[test]
    fn test_reconstruct_abstract() {
        let mut index = serde_json::Map::new();
        index.insert(
            "Hello".into(),
            serde_json::Value::Array(vec![serde_json::Value::Number(0.into())]),
        );
        index.insert(
            "world".into(),
            serde_json::Value::Array(vec![serde_json::Value::Number(1.into())]),
        );

        let result = reconstruct_abstract(&index);
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_work_to_candidate() {
        let work = serde_json::json!({
            "id": "https://openalex.org/W1234567890",
            "title": "Test Paper",
            "doi": "https://doi.org/10.1234/test",
            "publication_year": 2024,
            "authorships": [
                {"author": {"display_name": "Alice Smith"}},
                {"author": {"display_name": "Bob Jones"}}
            ],
            "primary_location": {
                "source": {"display_name": "Nature"}
            }
        });

        let c = work_to_candidate(&work, 1);
        assert_eq!(c.source, "openalex");
        assert_eq!(c.title, "Test Paper");
        assert_eq!(c.doi, Some("10.1234/test".to_string()));
        assert_eq!(c.year, Some(2024));
        assert_eq!(c.authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(c.journal, Some("Nature".to_string()));
    }

    #[test]
    fn short_openalex_id_extracts_from_url_or_bare_id() {
        assert_eq!(
            short_openalex_id("https://openalex.org/W2741809807"),
            Some("W2741809807".into())
        );
        assert_eq!(short_openalex_id("W2741809807"), Some("W2741809807".into()));
        assert_eq!(short_openalex_id("not-a-work"), None);
        assert_eq!(short_openalex_id("https://openalex.org/A12345"), None);
        // Edge: just "W" alone is not a valid id.
        assert_eq!(short_openalex_id("W"), None);
    }

    #[test]
    fn work_to_paper_uses_openalex_id_as_canonical_paper_id() {
        let work = serde_json::json!({
            "id": "https://openalex.org/W2741809807",
            "title": "Foundational paper",
            "publication_year": 2017,
            "doi": "https://doi.org/10.5555/foo",
        });
        let paper = work_to_paper(&work);
        assert_eq!(paper.id.as_str(), "W2741809807");
        assert_eq!(paper.openalex_id.as_deref(), Some("W2741809807"));
        assert_eq!(paper.title, "Foundational paper");
        assert_eq!(paper.year, Some(2017));
        assert_eq!(paper.doi.as_deref(), Some("10.5555/foo"));
    }

    #[test]
    fn search_field_parse_accepts_the_documented_aliases() {
        // The CLI flag / MCP param values users will actually type.
        for s in [
            "any", "Any", "ANY", "broad", "all", "search", "fulltext", "",
        ] {
            assert_eq!(SearchField::parse(s).unwrap(), SearchField::Any, "{s:?}");
        }
        for s in ["title", "Title", "TITLE"] {
            assert_eq!(SearchField::parse(s).unwrap(), SearchField::Title, "{s:?}");
        }
        for s in ["auto", "Auto"] {
            assert_eq!(SearchField::parse(s).unwrap(), SearchField::Auto, "{s:?}");
        }
    }

    #[test]
    fn search_field_parse_rejects_unknown_with_a_readable_message() {
        let err = SearchField::parse("abstract").unwrap_err();
        assert!(err.contains("abstract"), "{err}");
        assert!(err.contains("title"), "{err}"); // valid options listed
        assert!(err.contains("any"), "{err}");
        assert!(err.contains("auto"), "{err}");
    }

    #[test]
    fn search_field_default_is_any_so_pre_210_callers_see_no_change() {
        // The federated orchestrator hits the SourceAdapter trait, which
        // routes through `search_field(_, _, SearchField::Any)` — the
        // pre-#210 broad path. This test is the guard rail for that
        // choice: if someone flips the default to Title/Auto, they must
        // also update the docs and the MCP tool description.
        assert_eq!(SearchField::default(), SearchField::Any);
    }

    #[test]
    fn search_field_as_str_round_trips_through_parse() {
        for f in [SearchField::Any, SearchField::Title, SearchField::Auto] {
            assert_eq!(SearchField::parse(f.as_str()).unwrap(), f);
        }
    }

    #[test]
    fn title_filter_value_replaces_filter_metachars_with_spaces() {
        // The whole point of the helper: comma-bearing titles must
        // survive as searchable text, not blow up with HTTP 400.
        assert_eq!(
            title_filter_value("Bootstrap methods, another look at the jackknife"),
            "Bootstrap methods another look at the jackknife"
        );
        assert_eq!(
            title_filter_value("Bootstrap|jackknife"),
            "Bootstrap jackknife"
        );
        assert_eq!(
            title_filter_value("Bootstrap !jackknife"),
            "Bootstrap jackknife"
        );
        assert_eq!(
            title_filter_value(r#"Estimating "Dimension" of a Model"#),
            "Estimating Dimension of a Model"
        );
    }

    #[test]
    fn title_filter_value_preserves_ordinary_title_punctuation() {
        // Probe evidence: `:` `;` `(` `)` `-` `.` all pass the API
        // tokeniser cleanly, so the helper must not eat them.
        let input = "Controlling the false discovery rate: a practical (and powerful) approach";
        assert_eq!(title_filter_value(input), input);
        assert_eq!(
            title_filter_value("A method; sub-title 2.0"),
            "A method; sub-title 2.0"
        );
    }

    #[test]
    fn title_filter_value_collapses_and_trims_whitespace() {
        // Metachar substitution leaves double spaces where a `,` was
        // preceded by a space (e.g. `"a, b"` → `"a  b"`); the collapser
        // gives us clean single-spaced output so the emitted filter is
        // stable and human-readable in logs.
        assert_eq!(title_filter_value("a,   b|c , d"), "a b c d");
        assert_eq!(
            title_filter_value("   leading and trailing   "),
            "leading and trailing"
        );
    }

    #[test]
    fn title_filter_value_handles_empty_and_metachar_only_inputs() {
        assert_eq!(title_filter_value(""), "");
        assert_eq!(title_filter_value(",|!\""), "");
    }

    #[test]
    fn candidates_from_reads_the_results_array() {
        // Guards against a rename of `results` in the API surface: an
        // adapter that silently returns [] on a schema drift is the
        // exact class of failure #212 was about.
        let data = serde_json::json!({
            "results": [
                {"id": "https://openalex.org/W1", "title": "A", "publication_year": 2020},
                {"id": "https://openalex.org/W2", "title": "B", "publication_year": 2021},
            ]
        });
        let cs = candidates_from(&data);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].openalex_id.as_deref(), Some("W1"));
        assert_eq!(cs[0].rank, Some(1));
        assert_eq!(cs[1].rank, Some(2));
    }
}
