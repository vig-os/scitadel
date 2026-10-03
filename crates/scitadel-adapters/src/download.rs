//! The paper download chain, on `Route` + `PacedClient` (ADR-007 §3, S1).
//!
//! Four ladder steps in a fixed order — arXiv → OpenAlex → Unpaywall →
//! publisher HTML — then the paper record's own `url` as a last resort.
//! Two things changed shape in S1; the outputs did not.
//!
//! ## Every fetch is paced, and one work is one work
//!
//! Each step goes through [`PacedClient`], which spends one `Request`
//! permit per redirect hop keyed to that hop's own publisher-platform
//! bucket, and follows redirects itself so a five-hop chain cannot reach
//! four platforms for the price of one. The whole walk threads a **single**
//! [`WorkScope`], so the `Work` permit is charged once per bucket for the
//! work rather than once per leg (ADR-007 §4).
//!
//! The pacer is a [`SqlitePacer`] over the caller's `Database`. It is
//! **never** defaulted to an in-memory one: scitadel is routinely two
//! processes against one library file, and an in-memory ledger would give
//! the TUI and `scitadel mcp` a budget each — exactly the twice-the-traffic
//! failure ADR-007 §4 exists to prevent. That is why [`PaperDownloader::new`]
//! takes a `Database` and why [`PaperDownloader::with_client`] exists as the
//! test seam instead of a lenient default.
//!
//! Tiers are **per call**, not per route, which is the one place
//! [`RouteId::fetch_tier`] is not the answer: the Unpaywall/OpenAlex *API*
//! leg is a metadata call and charges [`PaceTier::Meta`], while the PDF it
//! resolves to is charged [`PaceTier::Oa`] — one `unpaywall` route, two
//! different budgets, because the bytes come off an OA host.
//!
//! ## Every successful download is dual-written
//!
//! S1 promises no behaviour change, so a successful download writes both
//! shapes of ADR-007 §1 "Legacy data" and writes them in **one
//! transaction**:
//!
//! - the `artefacts` row (+ its `blobs` row, via
//!   [`Database::record_download`]), with `route`, `version` and
//!   `access_basis` taken from the [`RouteId`] that served it, and
//!   `access_status` from the classification the chain has always made;
//! - the three legacy `papers` columns (`local_path`, `download_status`,
//!   `last_attempt_at`), which `find_cached_file`, `read_paper` and the
//!   TUI's state column still read until S2 moves them onto artefacts.
//!
//! The `papers/<stem>.<ext>` copy is still written exactly where it was,
//! under the name it always had. S2 retires both the columns and the copy.
//!
//! A download that **cannot** be recorded is not a completed download, so
//! every recording failure propagates rather than being logged and
//! dropped. The only place a failure is absorbed is a leg of the ladder
//! that did not win: those are `info!`-logged and the walk continues,
//! exactly as before.
//!
//! Two-phase, like the legacy backfill and the flat importer: hashing and
//! the copy into `blobs/` happen with no write lock held, then one short
//! `BEGIN IMMEDIATE` for the rows.
//!
//! ## The DOI gate (#262)
//!
//! `paper.doi` goes through [`validate_doi_detailed`] before it is
//! fetched. A rejected DOI is logged **with its reason** and never put on
//! the wire; the walk falls through to the paper's own `url`, so the only
//! user-visible difference from before is that one extra log line.
//!
//! ## Publisher honesty (#261)
//!
//! `artefacts.publisher` gets a name only when the DOI's registrant prefix
//! is in the table. When it is not, `publisher` stays `NULL` and
//! `publisher_note` carries [`RouteVerdict::PublisherUnknown`]'s note
//! **verbatim** — never "no TDM route available", because no route was
//! evaluated. See [`publisher_columns`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use reqwest::Url;
use scitadel_core::config::OpenAlexAuth;
use scitadel_core::models::{
    AccessStatus as StoredAccessStatus, Paper, RouteId, doi_to_filename, validate_doi,
    validate_doi_detailed,
};
use scitadel_core::ports::{Bucket, PaceTier, Pacer};
use scitadel_core::publisher::{PublisherVerdict, RouteVerdict, classify_publisher};
use scitadel_db::sqlite::{
    BlobWrite, Database, DownloadWrite, SqlitePacer, blob_rel_path, fulltext_kind, hash_file,
    store_blob,
};
use scitadel_http::{
    BucketPolicyTable, FetchError, PacedClient, PacedResponse, SafeHeaders, WorkScope,
};

use crate::error::AdapterError;
use crate::openalex::OPENALEX_API_URL;

/// The format of a downloaded paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadFormat {
    Pdf,
    Html,
}

impl std::fmt::Display for DownloadFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pdf => write!(f, "PDF"),
            Self::Html => write!(f, "HTML"),
        }
    }
}

/// Access level inferred from the downloaded content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessStatus {
    /// Full article content visible (OA PDF, or licensed-IP HTML).
    FullText,
    /// Abstract visible, body paywalled.
    Abstract,
    /// Hard paywall — only access options / login visible.
    Paywall,
    /// Heuristic couldn't classify.
    Unknown,
}

impl std::fmt::Display for AccessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FullText => write!(f, "full text"),
            Self::Abstract => write!(f, "abstract only"),
            Self::Paywall => write!(f, "paywall"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

impl From<AccessStatus> for scitadel_core::models::AccessStatus {
    fn from(status: AccessStatus) -> Self {
        match status {
            AccessStatus::FullText => Self::FullText,
            AccessStatus::Abstract => Self::Abstract,
            AccessStatus::Paywall => Self::Paywall,
            AccessStatus::Unknown => Self::Unknown,
        }
    }
}

/// Result of a successful paper download.
#[derive(Debug)]
pub struct DownloadResult {
    pub doi: String,
    pub path: PathBuf,
    pub format: DownloadFormat,
    /// Which ladder step produced these bytes (ADR-007 §3).
    ///
    /// The closed type, not a string: `artefacts.route` has to survive a
    /// write/read round trip through every other slice, and a string built
    /// per call site is exactly how two spellings of one route appear.
    pub route: RouteId,
    pub bytes: usize,
    pub access: AccessStatus,
    /// Canonical publisher/landing URL. Populated for publisher HTML and
    /// last-resort URL downloads so a paywall result can point the user at
    /// the live page (institutional IPs may grant access).
    pub publisher_url: Option<String>,
    /// The URL the bytes actually came from, after redirects — what
    /// `artefacts.source_url` records. `None` only where the fetch never
    /// left the ladder (never today).
    pub source_url: Option<String>,
    /// Why `artefacts.publisher` is empty, when it is.
    pub publisher_note: Option<String>,
}

impl DownloadResult {
    /// The route's stable label, for printed output.
    ///
    /// The only user-visible change S1 makes to a download line: the
    /// last-resort `url` route now reads `manual_url`, because `"url"` is
    /// not one of the four pseudo-route spellings migration 013 documents.
    #[must_use]
    pub fn source(&self) -> &'static str {
        self.route.label()
    }
}

/// Heuristic: classify publisher HTML as full-text / abstract / paywall.
///
/// Works for the common English-language publisher templates (Elsevier, Springer,
/// Wiley, ACM, IEEE, Nature, Science). Not every publisher normalizes the same way,
/// so when in doubt we return `Unknown` rather than misreporting.
pub fn detect_access_status(html: &str) -> AccessStatus {
    let lower = html.to_lowercase();

    let paywall_phrases = [
        "purchase access",
        "buy this article",
        "buy article",
        "subscribe to access",
        "subscribe to read",
        "subscribe to download",
        "get access to this article",
        "access this article",
        "obtain full access",
        "access options",
        "institutional sign in",
        "institutional login",
        "sign in to view full",
        "sign in to download",
        "subscribe for unlimited",
        "unlock this article",
    ];
    let paywall_hits = paywall_phrases
        .iter()
        .filter(|p| lower.contains(*p))
        .count();

    let full_text_markers = [
        "id=\"references\"",
        "id=\"bibliography\"",
        "id=\"acknowledgments\"",
        "id=\"acknowledgements\"",
        "class=\"references",
        "class=\"bibliography",
        ">references</h",
        ">acknowledgments</h",
        ">acknowledgements</h",
        ">supplementary information<",
        "<article",
    ];
    let full_text_hits = full_text_markers
        .iter()
        .filter(|p| lower.contains(*p))
        .count();

    let long_body = html.len() > 50_000;

    if paywall_hits >= 2 && full_text_hits == 0 {
        AccessStatus::Paywall
    } else if full_text_hits >= 2 && long_body {
        AccessStatus::FullText
    } else if paywall_hits > 0 {
        AccessStatus::Abstract
    } else if full_text_hits > 0 && long_body {
        AccessStatus::FullText
    } else {
        AccessStatus::Unknown
    }
}

/// The four services the ladder talks to.
///
/// A value rather than four `const`s so a test can point the whole chain
/// at one `wiremock` server. The defaults are byte-for-byte the URLs this
/// chain has always used, which is what keeps "the same outputs as today"
/// true of the request lines as well as of the printed lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// Base of `GET {base}/{doi}?email=…`.
    pub unpaywall_api: String,
    /// Base of `GET {base}/{doi}` — the DOI resolver.
    pub doi_resolver: String,
    /// Base of `GET {base}/{id}.pdf`.
    pub arxiv_pdf: String,
    /// Base of `GET {base}/{openalex_id}?mailto=…&api_key=…`.
    pub openalex_api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            unpaywall_api: "https://api.unpaywall.org/v2".to_string(),
            doi_resolver: "https://doi.org".to_string(),
            arxiv_pdf: "https://arxiv.org/pdf".to_string(),
            openalex_api: OPENALEX_API_URL.to_string(),
        }
    }
}

impl Endpoints {
    /// The direct arXiv PDF URL for whatever form of the id we were handed.
    #[must_use]
    pub fn arxiv_pdf_url(&self, id_or_url: &str) -> String {
        format!("{}/{}.pdf", self.arxiv_pdf, arxiv_id(id_or_url))
    }

    /// The Unpaywall lookup URL, with the polite-pool email.
    #[must_use]
    pub fn unpaywall_url(&self, doi: &str, email: &str) -> String {
        format!("{}/{doi}?email={email}", self.unpaywall_api)
    }

    /// The DOI resolver URL for a DOI — also the publisher page a human is
    /// pointed at.
    #[must_use]
    pub fn doi_url(&self, doi: &str) -> String {
        format!("{}/{doi}", self.doi_resolver)
    }
}

/// Downloads papers by DOI using Unpaywall (OA PDFs) with publisher HTML fallback.
pub struct PaperDownloader {
    /// The only way a request leaves this process (ADR-007 §3).
    client: PacedClient,
    db: Database,
    /// Polite-pool `mailto` plus the metered OpenAlex key. Unpaywall only
    /// ever needs the email; the OpenAlex leg needs both (#212).
    openalex: OpenAlexAuth,
    endpoints: Endpoints,
}

impl PaperDownloader {
    /// Build a downloader that paces against the shared SQLite ledger in
    /// `db`.
    ///
    /// `timeout_secs` is a per-request timeout on the transport: a
    /// publisher that accepts a connection and then stops writing would
    /// otherwise hold a four-leg walk open indefinitely.
    ///
    /// There is deliberately no overload that omits the database. An
    /// in-memory pacer is the precise failure ADR-007 §4 is about —
    /// scitadel is routinely two processes against one library, and two
    /// in-memory ledgers is twice the traffic a publisher agreed to carry.
    /// A caller that wants a different pacer (a test, an embedding) says so
    /// explicitly through [`Self::with_client`].
    ///
    /// # Errors
    ///
    /// Only if the transport cannot be built. The ledger is consulted
    /// lazily, per permit, and fails *closed* there rather than here.
    pub fn new(
        db: Database,
        openalex: OpenAlexAuth,
        timeout_secs: f64,
    ) -> Result<Self, AdapterError> {
        let table = BucketPolicyTable::new();
        // The pacer's policy lookup and the client's table are clones of
        // one value, so the ledger and the client can never disagree about
        // what a bucket costs.
        let lookup = Arc::new(table.clone());
        let pacer: Arc<dyn Pacer> = Arc::new(SqlitePacer::new(
            db.clone(),
            Arc::new(move |bucket: &Bucket| lookup.policy_for(bucket)),
        ));
        let client =
            PacedClient::with_timeout(Duration::from_secs_f64(timeout_secs), pacer, table)?;
        Ok(Self::with_client(
            db,
            openalex,
            client,
            Endpoints::default(),
        ))
    }

    /// Build a downloader over a caller-supplied [`PacedClient`] and
    /// endpoint set — the seam a test uses to inject a counting pacer and a
    /// `wiremock` server in place of the real ladder.
    ///
    /// The `Database` is still required: a download is dual-written, so
    /// there is no such thing as a downloader with nowhere to record what
    /// it fetched.
    pub fn with_client(
        db: Database,
        openalex: OpenAlexAuth,
        client: PacedClient,
        endpoints: Endpoints,
    ) -> Self {
        Self {
            client,
            db,
            openalex,
            endpoints,
        }
    }

    /// The endpoint set in force, for a test that asserts on it.
    #[must_use]
    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// Download by DOI only — kept for the CLI `download <doi>` path.
    /// Tries Unpaywall first, then publisher HTML fallback.
    ///
    /// Nothing is dual-written here: there is no `Paper` row to write
    /// against, and inventing one is not this function's job. The CLI's
    /// `persist_cli_download_outcome` still records the outcome against
    /// whatever row the DOI resolves to, exactly as before.
    pub async fn download(
        &self,
        doi: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        let normalized = validate_doi_detailed(doi)
            .map_err(|reason| AdapterError::Validation(format!("invalid DOI: {reason}")))?;

        tokio::fs::create_dir_all(output_dir).await.map_err(|e| {
            AdapterError::Io(format!(
                "failed to create output dir {}: {e}",
                output_dir.display()
            ))
        })?;

        let work = WorkScope::new();

        match self.try_unpaywall(&work, &normalized, output_dir).await {
            Ok(fetched) => return self.finish(None, fetched).await,
            Err(e) => {
                tracing::info!(doi = %normalized, error = %e, "Unpaywall lookup failed, falling back to publisher");
            }
        }

        let fetched = self
            .download_publisher_html(&work, &normalized, output_dir)
            .await?;
        self.finish(None, fetched).await
    }

    /// Download a paper using every identifier available on the `Paper` record.
    ///
    /// Priority — unchanged, and the whole point of a dual-write:
    /// 1. `arxiv_id` → direct arXiv PDF (no API call, always OA)
    /// 2. `openalex_id` → OpenAlex `/works` API for `best_oa_location.pdf_url`
    /// 3. `doi` → Unpaywall (existing path)
    /// 4. `url` → download the landing page as HTML (last resort)
    ///
    /// # Errors
    ///
    /// `AdapterError::NotFound` when the record offers nothing to try;
    /// otherwise whichever error stopped the walk, **including a failure to
    /// record a download that succeeded** — see the module docs.
    pub async fn download_paper(
        &self,
        paper: &Paper,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        tokio::fs::create_dir_all(output_dir).await.map_err(|e| {
            AdapterError::Io(format!(
                "failed to create output dir {}: {e}",
                output_dir.display()
            ))
        })?;

        let stem = file_stem_for(paper);
        // One scope for the whole walk: four legs of one work are one work
        // to the publisher, so the `Work` permit is charged once per bucket
        // and not once per leg.
        let work = WorkScope::new();

        if let Some(id) = paper.arxiv_id.as_deref().filter(|s| !s.is_empty()) {
            match self.try_arxiv(&work, id, &stem, output_dir).await {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => tracing::info!(arxiv_id = %id, error = %e, "arxiv fallback failed"),
            }
        }

        if let Some(id) = paper.openalex_id.as_deref().filter(|s| !s.is_empty()) {
            match self.try_openalex(&work, id, &stem, output_dir).await {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => tracing::info!(openalex_id = %id, error = %e, "openalex fallback failed"),
            }
        }

        // The DOI gate. `validate_doi_detailed` is the same check
        // `validate_doi` performs, so the *set* of DOIs that reach the wire
        // is unchanged; what is new is that a rejection says why (#262).
        if let Some(raw) = paper.doi.as_deref().filter(|s| !s.trim().is_empty()) {
            match validate_doi_detailed(raw) {
                Ok(normalized) => {
                    match self.try_unpaywall(&work, &normalized, output_dir).await {
                        Ok(fetched) => return self.finish(Some(paper), fetched).await,
                        Err(e) => {
                            tracing::info!(doi = %normalized, error = %e, "unpaywall fallback failed");
                        }
                    }
                    match self
                        .download_publisher_html(&work, &normalized, output_dir)
                        .await
                    {
                        Ok(fetched) => return self.finish(Some(paper), fetched).await,
                        Err(e) => {
                            tracing::info!(doi = %normalized, error = %e, "publisher html fallback failed");
                        }
                    }
                }
                Err(reason) => {
                    tracing::info!(
                        doi = %raw,
                        reason = %reason,
                        "DOI rejected before any fetch; skipping the Unpaywall and \
                         publisher legs (the paper's own URL is still tried)"
                    );
                }
            }
        }

        if let Some(url) = paper.url.as_deref().filter(|s| !s.is_empty()) {
            let fetched = self
                .download_url_as_html(&work, url, &stem, output_dir)
                .await?;
            return self.finish(Some(paper), fetched).await;
        }

        Err(AdapterError::NotFound(
            "no arxiv_id, openalex_id, doi, or url to try".into(),
        ))
    }

    /// Write the legacy copy, store the blob, and dual-write the rows.
    ///
    /// `paper` is `None` for the DOI-only path, which has no work row to
    /// write against; the file is still written and the result returned.
    ///
    /// # Errors
    ///
    /// Every phase propagates. The phases are, in order: the legacy
    /// `papers/<stem>.<ext>` copy, the content hash and the copy into
    /// `blobs/`, then one transaction for the rows. A caller that got `Ok`
    /// can rely on all three having happened.
    async fn finish(
        &self,
        paper: Option<&Paper>,
        fetched: Fetched,
    ) -> Result<DownloadResult, AdapterError> {
        let Fetched {
            reference,
            bytes,
            ext,
            format,
            access,
            route,
            source_url,
            publisher_url,
            path,
        } = fetched;

        // The `kind` vocabulary is closed, so an extension outside it is a
        // refusal rather than a guess: filing a `.docx` as `fulltext_html`
        // would make coverage claim a full text we cannot read. These four
        // legs only ever produce `pdf` and `html`, so this cannot fire
        // today — which is exactly why it has to be an error rather than
        // an `.unwrap_or`.
        let kind = fulltext_kind(ext).ok_or_else(|| {
            AdapterError::Validation(format!(
                "{ext:?} is not a full-text kind, so this download cannot be recorded"
            ))
        })?;

        let result = DownloadResult {
            doi: reference,
            path: path.clone(),
            format,
            route,
            bytes: bytes.len(),
            access,
            publisher_url,
            source_url: Some(source_url),
            publisher_note: None,
        };

        let Some(paper) = paper else {
            // No work row to write against: the legacy copy is the whole
            // deliverable for the DOI-only path.
            write_legacy_copy(&path, &bytes)?;
            return Ok(result);
        };

        // Phase 1 — file work with no write lock held, and off the runtime:
        // hashing a 200-page PDF is not something a paced campaign should
        // feel. Same split as the legacy backfill and the flat importer —
        // file work first, one short transaction after.
        let db = self.db.clone();
        let bytes = Arc::new(bytes);
        let staging = path.clone();
        let (sha256, byte_len) = tokio::task::spawn_blocking(move || -> Result<_, AdapterError> {
            // The legacy copy goes down under the name it has always had,
            // because the TUI state column, `find_cached_file` and
            // `read_paper` are still reading it and S2 is what retires it.
            write_legacy_copy(&staging, &bytes)?;
            let library_root = db.library_root()?.ok_or_else(|| {
                AdapterError::Other(
                    "this library is in-memory, so it has no root and can hold no blobs \
                     (ADR-007 §1 \"Storage\"); a download cannot be recorded against it"
                        .to_string(),
                )
            })?;
            let (sha256, byte_len) = hash_file(&staging)?;
            store_blob(&staging, &sha256, ext, &library_root)?;
            Ok((sha256, byte_len))
        })
        .await
        .map_err(|e| AdapterError::Other(format!("download staging task failed: {e}")))??;

        let now = Utc::now().to_rfc3339();
        let (publisher, publisher_note) =
            publisher_columns(paper.doi.as_deref().filter(|d| validate_doi(d)));

        // Phase 2 — one transaction: `blobs`, the `artefacts` row, and the
        // three legacy `papers` columns.
        self.db.record_download(&DownloadWrite {
            paper_id: paper.id.as_str().to_string(),
            route,
            ext: ext.to_string(),
            access_status: access.into(),
            source_url: result.source_url.clone(),
            publisher,
            publisher_note: publisher_note.clone(),
            retrieved_at: now.clone(),
            blob: Some(BlobWrite {
                sha256: sha256.clone(),
                bytes: byte_len,
                mime: kind.mime.to_string(),
                rel_path: blob_rel_path(&sha256, ext),
                created_at: now,
            }),
            local_path: path.to_string_lossy().into_owned(),
            download_status: StoredAccessStatus::from(access).download_status(),
        })?;

        Ok(DownloadResult {
            publisher_note,
            ..result
        })
    }

    /// One paced fetch inside a work's ladder walk.
    ///
    /// `what` names the hop for the log line, and is also the shape of the
    /// message this chain has always logged. Every request here is
    /// anonymous: S1 wires no TDM credential, and the one credential that
    /// does travel — the OpenAlex key — goes in the query exactly as it did
    /// before (`#276` tracks moving it out of the URL).
    async fn get(
        &self,
        work: &WorkScope,
        raw_url: &str,
        tier: PaceTier,
        what: &str,
    ) -> Result<PacedResponse, AdapterError> {
        let url = Url::parse(raw_url).map_err(|e| {
            // Deliberately not echoing `raw_url`: the OpenAlex leg's URL
            // carries the API key as a query parameter, and an error string
            // is a log line.
            AdapterError::Other(format!("could not build a request URL: {e}"))
        })?;
        self.client
            .get_in_work(work, url, tier, SafeHeaders::unauthenticated())
            .await
            .map_err(|e| fetch_failure(what, e))
    }

    /// Query Unpaywall for an open-access PDF URL and download it.
    async fn try_unpaywall(
        &self,
        work: &WorkScope,
        doi: &str,
        output_dir: &Path,
    ) -> Result<Fetched, AdapterError> {
        // The polite-pool email is always present, even empty — that is the
        // request line this chain has always sent.
        let url = self.endpoints.unpaywall_url(doi, &self.openalex.email);
        let text = self
            .get(work, &url, PaceTier::Meta, "Unpaywall API")
            .await?
            .text()
            .await
            .map_err(|e| fetch_failure("Unpaywall JSON parse failed", e))?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AdapterError::Parse(format!("Unpaywall JSON parse failed: {e}")))?;

        let pdf_url = body
            .get("best_oa_location")
            .and_then(|loc| loc.get("url_for_pdf"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                AdapterError::NotFound("no open-access PDF found via Unpaywall".into())
            })?;

        tracing::info!(doi = %doi, pdf_url = %pdf_url, "found OA PDF via Unpaywall");

        // The resolved PDF is charged as `Oa`, not `Meta`: the metadata API
        // found it, but the bytes come off an OA host, and ADR-007 §4
        // budgets those separately.
        let (bytes, source_url) = self
            .download_body(work, pdf_url, PaceTier::Oa, "PDF download")
            .await?;

        Ok(Fetched {
            reference: doi.to_string(),
            path: output_dir.join(format!("{}.pdf", doi_to_filename(doi))),
            bytes,
            ext: PDF_EXT,
            format: DownloadFormat::Pdf,
            access: AccessStatus::FullText,
            route: RouteId::Unpaywall,
            source_url,
            publisher_url: None,
        })
    }

    /// Resolve a DOI to its publisher page and download the HTML.
    async fn download_publisher_html(
        &self,
        work: &WorkScope,
        doi: &str,
        output_dir: &Path,
    ) -> Result<Fetched, AdapterError> {
        let doi_url = self.endpoints.doi_url(doi);

        let (bytes, source_url) = self
            .download_body(work, &doi_url, PaceTier::Oa, "DOI resolution")
            .await?;
        let access = classify_html(&bytes);

        Ok(Fetched {
            reference: doi.to_string(),
            path: output_dir.join(format!("{}.html", doi_to_filename(doi))),
            bytes,
            ext: HTML_EXT,
            format: DownloadFormat::Html,
            access,
            route: RouteId::Publisher,
            source_url,
            // The URL we asked for, not the one we landed on: this is the
            // canonical page a human should be pointed at, and a paywalled
            // article needs the DOI, not whichever CDN served the stub.
            publisher_url: Some(doi_url),
        })
    }

    /// Try the direct arXiv PDF URL. Free, no API call, always full-text.
    async fn try_arxiv(
        &self,
        work: &WorkScope,
        arxiv_id: &str,
        stem: &str,
        output_dir: &Path,
    ) -> Result<Fetched, AdapterError> {
        let (bytes, source_url) = self
            .download_body(
                work,
                &self.endpoints.arxiv_pdf_url(arxiv_id),
                PaceTier::Oa,
                "arXiv",
            )
            .await?;

        Ok(Fetched {
            reference: arxiv_id.to_string(),
            path: output_dir.join(format!("{stem}.pdf")),
            bytes,
            ext: PDF_EXT,
            format: DownloadFormat::Pdf,
            access: AccessStatus::FullText,
            route: RouteId::Arxiv,
            source_url,
            publisher_url: None,
        })
    }

    /// Query OpenAlex `/works/{id}` and download its `best_oa_location.pdf_url`.
    async fn try_openalex(
        &self,
        work: &WorkScope,
        openalex_id: &str,
        stem: &str,
        output_dir: &Path,
    ) -> Result<Fetched, AdapterError> {
        let id = openalex_id.trim_start_matches("https://openalex.org/");
        // Same credential pair as the search path: an unauthenticated
        // lookup is billed against the shared per-IP budget and starts
        // 429-ing once it's spent (#212). `Url`'s own encoder replaces the
        // hand-rolled `?{k}={v}` splicing, so the two parameters are sent
        // with the same names in the same order as before.
        let mut api_url = Url::parse(&format!("{}/{id}", self.endpoints.openalex_api))
            .map_err(|e| AdapterError::Other(format!("could not build the OpenAlex URL: {e}")))?;
        {
            let mut query = api_url.query_pairs_mut();
            if !self.openalex.email.is_empty() {
                query.append_pair("mailto", &self.openalex.email);
            }
            if !self.openalex.api_key.is_empty() {
                query.append_pair("api_key", &self.openalex.api_key);
            }
        }

        let text = self
            .get(work, api_url.as_str(), PaceTier::Meta, "OpenAlex API")
            .await?
            .text()
            .await
            .map_err(|e| fetch_failure("OpenAlex JSON parse failed", e))?;
        let body: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AdapterError::Parse(format!("OpenAlex JSON parse failed: {e}")))?;

        let pdf_url = body
            .get("best_oa_location")
            .and_then(|loc| loc.get("pdf_url"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                body.get("open_access")
                    .and_then(|oa| oa.get("oa_url"))
                    .and_then(serde_json::Value::as_str)
            })
            .ok_or_else(|| AdapterError::NotFound("OpenAlex reports no OA location".into()))?;

        // Metadata API to find it, OA host to serve it: two tiers, one route.
        let response = self.get(work, pdf_url, PaceTier::Oa, "OA URL").await?;
        let is_pdf = pdf_url.to_lowercase().ends_with(".pdf")
            || response
                .content_type()
                .is_some_and(|ct| ct.contains("application/pdf"));
        // Read the URL off before `bytes()` consumes the response: this is
        // the post-redirect URL, which is where the bytes actually live.
        let source_url = response.url.to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| fetch_failure("failed to read OA bytes", e))?;

        let (ext, format, access) = if is_pdf {
            (PDF_EXT, DownloadFormat::Pdf, AccessStatus::FullText)
        } else {
            (HTML_EXT, DownloadFormat::Html, classify_html(&bytes))
        };

        Ok(Fetched {
            reference: openalex_id.to_string(),
            path: output_dir.join(format!("{stem}.{ext}")),
            bytes,
            ext,
            format,
            access,
            route: RouteId::OpenAlex,
            source_url,
            publisher_url: (format == DownloadFormat::Html).then(|| pdf_url.to_string()),
        })
    }

    /// Last resort: fetch whatever URL the paper has, save as HTML.
    async fn download_url_as_html(
        &self,
        work: &WorkScope,
        url: &str,
        stem: &str,
        output_dir: &Path,
    ) -> Result<Fetched, AdapterError> {
        let (bytes, source_url) = self.download_body(work, url, PaceTier::Oa, "URL").await?;

        let access = classify_html(&bytes);
        Ok(Fetched {
            reference: url.to_string(),
            path: output_dir.join(format!("{stem}.html")),
            bytes,
            ext: HTML_EXT,
            format: DownloadFormat::Html,
            access,
            route: RouteId::ManualUrl,
            source_url,
            publisher_url: Some(url.to_string()),
        })
    }

    /// Fetch a body and hand back the bytes with the URL they came from.
    ///
    /// The returned URL is the **post-redirect** one, because that is where
    /// the bytes actually live: a landing page that redirects to a CDN is
    /// recorded as the CDN URL, which is what a later `gc` or a
    /// `source_url` consumer needs.
    async fn download_body(
        &self,
        work: &WorkScope,
        url: &str,
        tier: PaceTier,
        what: &str,
    ) -> Result<(Vec<u8>, String), AdapterError> {
        let response = self.get(work, url, tier, what).await?;
        let source_url = response.url.to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| fetch_failure("failed to read bytes", e))?;
        Ok((bytes, source_url))
    }
}

/// What one ladder step brought back, before anything is recorded.
///
/// Deliberately not a [`DownloadResult`]: the two shapes are built on
/// either side of the recording step, so a half-recorded download cannot be
/// handed back as a result.
struct Fetched {
    /// What `DownloadResult::doi` has always carried for this route — the
    /// DOI, or the arXiv / OpenAlex id, or the URL, for the routes that
    /// have none.
    reference: String,
    bytes: Vec<u8>,
    /// File extension of what we stored: `pdf` or `html`. Resolved into
    /// `kind` / `format` / `mime` by [`fulltext_kind`] on the way into the
    /// database, so there is one authority for "what is a `.pdf`".
    ext: &'static str,
    format: DownloadFormat,
    access: AccessStatus,
    route: RouteId,
    /// Post-redirect URL the bytes came from.
    source_url: String,
    publisher_url: Option<String>,
    path: PathBuf,
}

/// The two full-text serialisations these four legs have ever stored.
///
/// `fulltext_kind` turns each into the `kind` / `format` / `mime` triple on
/// the way into the database, so there is one authority for what a `.pdf`
/// is. These are the only two extensions the chain can produce, which is
/// why it never has to decide what to do about a `.docx`.
const PDF_EXT: &str = "pdf";
const HTML_EXT: &str = "html";

/// Write the legacy `papers/<stem>.<ext>` copy.
///
/// Kept exactly where ADR-007 §1 "Legacy data" says it is, under the name
/// it has always had: the TUI state column, `find_cached_file` and
/// `read_paper` all resolve it by that path, and S2 is what retires it.
fn write_legacy_copy(path: &Path, bytes: &[u8]) -> Result<(), AdapterError> {
    std::fs::write(path, bytes)
        .map_err(|e| AdapterError::Io(format!("failed to write {}: {e}", path.display())))
}

/// Classify a fetched body as full text / abstract / paywall.
///
/// Non-UTF-8 is `Unknown`, exactly as before: a PDF that was not
/// recognised as a PDF is not text we can classify.
fn classify_html(bytes: &[u8]) -> AccessStatus {
    std::str::from_utf8(bytes).map_or(AccessStatus::Unknown, detect_access_status)
}

/// The `publisher` and `publisher_note` columns for a work.
///
/// A name only when the DOI's registrant prefix is classified. When it is
/// not, `publisher` stays `NULL` and the note is
/// [`RouteVerdict::PublisherUnknown`]'s, verbatim — the wording matters,
/// so it comes from the type rather than a second phrasing of it (#261).
///
/// `route_verdict` is deliberately *not* used here: for a classified
/// publisher with a TDM API it would answer `TdmKeyMissing`, and this
/// download evaluated no TDM route at all. Writing that would be #261's bug
/// in a new place — telling 249 papers to go and register a key because a
/// prefix was in a table. A classified publisher therefore gets the name
/// and no note; the note column exists for the gap in *our* coverage.
fn publisher_columns(doi: Option<&str>) -> (Option<String>, Option<String>) {
    match doi.map(classify_publisher) {
        Some(PublisherVerdict::Known { publisher, .. }) => {
            (Some(publisher.label().to_string()), None)
        }
        Some(PublisherVerdict::Unknown { prefix }) => {
            (None, Some(RouteVerdict::PublisherUnknown { prefix }.note()))
        }
        // No DOI at all: there is no prefix to be unknown *about*, so there
        // is nothing to explain.
        None => (None, None),
    }
}

/// Map a paced fetch failure to the message this chain has always logged.
///
/// The `Status` / `RateLimited` arms carry the status code and **nothing
/// else**. `FetchError`'s `Display` includes the request URL, and the
/// OpenAlex leg's URL carries `api_key` as a query parameter — forwarding
/// it verbatim would turn "the key is in the request line" (#276, which
/// tracks the real fix) into "the key is in every log line". The transport
/// arm keeps the URL, which is where `reqwest` has always put it and where
/// it earns its keep: an unreachable host.
fn fetch_failure(what: &str, error: FetchError) -> AdapterError {
    match error {
        FetchError::Status { code, .. } | FetchError::RateLimited { code, .. } => {
            AdapterError::Network(format!("{what} returned status {code}"))
        }
        other => AdapterError::Network(format!("{what}: {other}")),
    }
}

/// The bare arXiv identifier inside whatever form we were handed.
///
/// Handles `2005.07866`, `2005.07866v1` and full `abs`/`pdf` URLs.
fn arxiv_id(id_or_url: &str) -> &str {
    id_or_url
        .trim_start_matches("https://arxiv.org/abs/")
        .trim_start_matches("http://arxiv.org/abs/")
        .trim_start_matches("https://arxiv.org/pdf/")
        .trim_start_matches("http://arxiv.org/pdf/")
        .trim_end_matches(".pdf")
}

/// The direct arXiv PDF URL under the **default** endpoints — the shape
/// the four tests below pin, so a change to [`Endpoints::default`] that
/// moved arXiv off `arxiv.org` (or renamed the file) fails here.
///
/// Test-only: the chain itself always goes through
/// [`Endpoints::arxiv_pdf_url`], because a downloader that cannot be
/// pointed at a `wiremock` server cannot have a permit counted against it.
#[cfg(test)]
fn arxiv_pdf_url(id_or_url: &str) -> String {
    Endpoints::default().arxiv_pdf_url(id_or_url)
}

/// Locate an already-downloaded file for this paper. Returns the path if the
/// expected `.pdf` or `.html` exists under `papers_dir`.
pub fn find_cached_file(paper: &Paper, papers_dir: &Path) -> Option<PathBuf> {
    let stem = file_stem_for(paper);
    for ext in ["pdf", "html"] {
        let path = papers_dir.join(format!("{stem}.{ext}"));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Pick a safe filename stem preferring DOI, then arxiv, then openalex, then the paper's UUID.
pub fn file_stem_for(paper: &Paper) -> String {
    if let Some(doi) = paper.doi.as_deref().filter(|s| validate_doi(s)) {
        return doi_to_filename(&scitadel_core::models::normalize_doi(doi));
    }
    if let Some(id) = paper.arxiv_id.as_deref().filter(|s| !s.is_empty()) {
        return sanitize_filename(&format!("arxiv_{id}"));
    }
    if let Some(id) = paper.openalex_id.as_deref().filter(|s| !s.is_empty()) {
        return sanitize_filename(&format!("openalex_{id}"));
    }
    sanitize_filename(paper.id.as_str())
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use scitadel_core::ports::{Cost, PaceDenied, Permit};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---------------------------------------------------------------------
    // A pacer that makes the acquisition observable.
    //
    // These tests are about *which permits the download chain asks for* and
    // in which tier — not about what a ledger does with them. A real
    // `SqlitePacer` would answer the same question, but only after a
    // migration, a temp library and a policy table per test, and it would
    // answer it by asserting on rows rather than on the request that caused
    // them. It is exercised for real in `scitadel-db`'s own tests and in the
    // two-process cap test.
    // ---------------------------------------------------------------------

    /// One acquisition, as the chain asked for it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Grant {
        bucket: String,
        tier: PaceTier,
        cost: Cost,
    }

    /// Grants everything, immediately, and remembers.
    #[derive(Debug, Default)]
    struct CountingPacer {
        grants: std::sync::Mutex<Vec<Grant>>,
    }

    impl CountingPacer {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn grants(&self) -> Vec<Grant> {
            self.grants.lock().expect("uncontended").clone()
        }

        fn spending(&self, cost: Cost) -> Vec<Grant> {
            self.grants()
                .into_iter()
                .filter(|g| g.cost == cost)
                .collect()
        }
    }

    #[async_trait]
    impl Pacer for CountingPacer {
        async fn acquire(
            &self,
            bucket: &Bucket,
            tier: PaceTier,
            cost: Cost,
        ) -> std::result::Result<Permit, PaceDenied> {
            self.grants.lock().expect("uncontended").push(Grant {
                bucket: bucket.as_str().to_string(),
                tier,
                cost,
            });
            Ok(Permit {
                bucket: bucket.clone(),
                tier,
                not_before: std::time::Instant::now(),
            })
        }
    }

    // ---------------------------------------------------------------------
    // Fixture: a real library on disk, and the whole ladder pointed at one
    // `wiremock` server.
    // ---------------------------------------------------------------------

    const PDF_BYTES: &[u8] = b"%PDF-1.7\ndownloaded body\n%%EOF\n";
    const HTML_BYTES: &[u8] = b"<!doctype html><html><body><p>landing</p></body></html>";

    struct Fixture {
        /// The library root. Held for its lifetime because the pool opens
        /// connections lazily: dropping the directory mid-test would let a
        /// later `conn()` create a fresh, empty database.
        dir: tempfile::TempDir,
        db: Database,
        server: MockServer,
        pacer: Arc<CountingPacer>,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let db = Database::open(&dir.path().join("scitadel.db")).expect("open db");
            db.migrate().expect("migrate");
            Self {
                dir,
                db,
                server: MockServer::start().await,
                pacer: CountingPacer::new(),
            }
        }

        /// The library root: the absolute parent of the database file, and
        /// therefore where `blobs/` is created.
        fn root(&self) -> &Path {
            self.dir.path()
        }

        /// Where the legacy `papers/<stem>.<ext>` copy is written.
        fn papers_dir(&self) -> PathBuf {
            self.root().join("papers")
        }

        /// The whole ladder pointed at the mock server, paced by the
        /// counting pacer. One server for every endpoint, so every leg lands
        /// in the same bucket and the `Work` accounting is the only variable.
        fn downloader(&self) -> PaperDownloader {
            let client = PacedClient::new(
                PacedClient::default_transport().expect("transport builds"),
                self.pacer.clone(),
                BucketPolicyTable::new(),
            );
            let base = self.server.uri();
            PaperDownloader::with_client(
                self.db.clone(),
                OpenAlexAuth {
                    email: "polite@example.org".to_string(),
                    api_key: "openalex-secret-key".to_string(),
                },
                client,
                Endpoints {
                    unpaywall_api: format!("{base}/v2"),
                    doi_resolver: format!("{base}/doi"),
                    arxiv_pdf: format!("{base}/pdf"),
                    openalex_api: format!("{base}/works"),
                },
            )
        }

        async fn serve(&self, route: &str, body: impl Into<Vec<u8>>) {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body.into()))
                .mount(&self.server)
                .await;
        }

        /// Mount the Unpaywall lookup and the PDF it resolves to — the shape
        /// of a real OA hit, on one server.
        async fn serve_unpaywall_pdf(&self, doi: &str) -> String {
            let pdf_route = "/oa/paper.pdf";
            let pdf_url = format!("{}/oa/paper.pdf", self.server.uri());
            self.serve(&format!("/v2/{doi}"), unpaywall_json(&pdf_url))
                .await;
            self.serve(pdf_route, PDF_BYTES).await;
            pdf_url
        }

        /// Every request the chain put on the wire, as full URLs.
        async fn requests(&self) -> Vec<String> {
            self.server
                .received_requests()
                .await
                .expect("wiremock recorded")
                .iter()
                .map(|r| r.url.to_string())
                .collect()
        }

        fn artefact(&self, paper_id: &str) -> Option<ArtefactRow> {
            self.artefacts()
                .into_iter()
                .find(|a| a.paper_id == paper_id)
        }

        fn artefacts(&self) -> Vec<ArtefactRow> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare(
                    "SELECT paper_id, kind, version, sha256, format, access_status, route,
                            access_basis, source_url, publisher, publisher_note, retrieved_at
                     FROM artefacts ORDER BY paper_id",
                )
                .expect("prepare");
            stmt.query_map([], |r| {
                Ok(ArtefactRow {
                    paper_id: r.get(0)?,
                    kind: r.get(1)?,
                    version: r.get(2)?,
                    sha256: r.get(3)?,
                    format: r.get(4)?,
                    access_status: r.get(5)?,
                    route: r.get(6)?,
                    access_basis: r.get(7)?,
                    source_url: r.get(8)?,
                    publisher: r.get(9)?,
                    publisher_note: r.get(10)?,
                    retrieved_at: r.get(11)?,
                })
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
        }

        /// `(local_path, download_status, last_attempt_at)` for a work.
        fn legacy(&self, paper_id: &str) -> (Option<String>, Option<String>, Option<String>) {
            let conn = self.db.conn().expect("conn");
            conn.query_row(
                "SELECT local_path, download_status, last_attempt_at FROM papers WHERE id = ?1",
                [paper_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("legacy row")
        }

        fn count(&self, table: &str) -> i64 {
            let conn = self.db.conn().expect("conn");
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .expect("count")
        }
    }

    /// A saved `Paper` with the identifiers a test needs, plus its row.
    fn save(
        fx: &Fixture,
        id: &str,
        doi: Option<&str>,
        arxiv: Option<&str>,
        openalex: Option<&str>,
        url: Option<&str>,
    ) -> Paper {
        let mut p = Paper::new(format!("Paper {id}"));
        p.id = scitadel_core::models::PaperId::from(id.to_string());
        p.doi = doi.map(str::to_string);
        p.arxiv_id = arxiv.map(str::to_string);
        p.openalex_id = openalex.map(str::to_string);
        p.url = url.map(str::to_string);
        let conn = fx.db.conn().expect("conn");
        conn.execute(
            "INSERT INTO papers (id, title, authors, doi, created_at, updated_at)
             VALUES (?1, ?2, '[]', ?3, '2020-01-01T00:00:00+00:00', '2020-01-01T00:00:00+00:00')",
            rusqlite::params![id, p.title, p.doi],
        )
        .expect("insert paper");
        drop(conn);
        p
    }

    #[derive(Debug)]
    struct ArtefactRow {
        paper_id: String,
        kind: String,
        version: String,
        sha256: Option<String>,
        format: Option<String>,
        access_status: String,
        route: String,
        access_basis: String,
        source_url: Option<String>,
        publisher: Option<String>,
        publisher_note: Option<String>,
        retrieved_at: String,
    }

    fn unpaywall_json(pdf_url: &str) -> String {
        format!(r#"{{"best_oa_location":{{"url_for_pdf":"{pdf_url}"}}}}"#)
    }

    #[test]
    fn classifies_obvious_paywall() {
        let html = r"<html><body>
            <h1>Access options</h1>
            <button>Purchase access</button>
            <button>Institutional sign in</button>
            </body></html>";
        assert_eq!(detect_access_status(html), AccessStatus::Paywall);
    }

    #[test]
    fn classifies_full_text_with_references() {
        let body = "<p>body</p>".repeat(5000);
        let html = format!(
            r#"<html><body>
            <article>{body}</article>
            <section id="references"><h2>References</h2></section>
            <section id="acknowledgments"><h2>Acknowledgments</h2></section>
            </body></html>"#
        );
        assert_eq!(detect_access_status(&html), AccessStatus::FullText);
    }

    #[test]
    fn abstract_only_when_paywall_with_some_content() {
        let html = r#"<html><body>
            <h1>Abstract</h1>
            <p>Some abstract text.</p>
            <div class="paywall">
              <button>Access this article</button>
            </div>
            </body></html>"#;
        assert_eq!(detect_access_status(html), AccessStatus::Abstract);
    }

    #[test]
    fn unknown_when_no_markers() {
        let html = "<html><body><p>hello world</p></body></html>";
        assert_eq!(detect_access_status(html), AccessStatus::Unknown);
    }

    #[test]
    fn arxiv_pdf_url_from_bare_id() {
        assert_eq!(
            arxiv_pdf_url("2005.07866"),
            "https://arxiv.org/pdf/2005.07866.pdf"
        );
    }

    #[test]
    fn arxiv_pdf_url_with_version() {
        assert_eq!(
            arxiv_pdf_url("2005.07866v1"),
            "https://arxiv.org/pdf/2005.07866v1.pdf"
        );
    }

    #[test]
    fn arxiv_pdf_url_from_abs_url() {
        assert_eq!(
            arxiv_pdf_url("http://arxiv.org/abs/2005.07866v1"),
            "https://arxiv.org/pdf/2005.07866v1.pdf"
        );
    }

    #[test]
    fn arxiv_pdf_url_idempotent_on_pdf_url() {
        assert_eq!(
            arxiv_pdf_url("https://arxiv.org/pdf/2005.07866v1.pdf"),
            "https://arxiv.org/pdf/2005.07866v1.pdf"
        );
    }
    // =====================================================================
    // ADR-007 S1 acceptance: "`download_paper` runs on `Route` +
    // `PacedClient` with the same outputs as today (dual-write)".
    //
    // Every test below drives the real `download_paper` against a real
    // on-disk library and a real `PacedClient`; only the pacer (counting)
    // and the network (wiremock) are substituted.
    // =====================================================================

    /// ADR-007 §1 "Legacy data": S1 promises no behaviour change, so a
    /// successful download has to land in **both** shapes — the
    /// `artefacts` row with the route's own `version` and `access_basis`,
    /// and the three legacy `papers` columns the TUI state column,
    /// `find_cached_file` and `read_paper` still read.
    ///
    /// Asserting only the artefact row would pass against a "migrate and
    /// stop" implementation, and only the legacy columns would pass against
    /// the pre-S1 code. Both are the point.
    #[tokio::test]
    async fn a_successful_download_writes_an_artefact_and_the_legacy_columns() {
        let fx = Fixture::new().await;
        fx.serve("/pdf/2005.07866.pdf", PDF_BYTES).await;
        let paper = save(
            &fx,
            "p-arxiv",
            Some("10.48550/arxiv.2005.07866"),
            Some("2005.07866"),
            None,
            None,
        );
        let out_dir = fx.papers_dir();

        let result = fx
            .downloader()
            .download_paper(&paper, &out_dir)
            .await
            .expect("the arXiv leg should serve the PDF");

        // --- the artefacts row ---
        let row = fx.artefact("p-arxiv").expect("an artefact row");
        assert_eq!(row.kind, "fulltext_pdf");
        assert_eq!(row.format.as_deref(), Some("pdf"));
        assert_eq!(row.route, "arxiv", "the route is recorded, not guessed");
        assert_eq!(
            row.version, "preprint",
            "an arXiv PDF is a preprint by construction"
        );
        assert_eq!(
            row.access_basis, "oa_license",
            "arXiv is free to read by construction"
        );
        assert_eq!(row.access_status, "full_text");
        assert!(row.sha256.is_some(), "the bytes are hashed");
        assert_eq!(
            row.source_url.as_deref(),
            Some(format!("{}/pdf/2005.07866.pdf", fx.server.uri()).as_str()),
            "source_url is where the bytes actually came from"
        );

        // --- the legacy columns, in the same breath ---
        let (local_path, status, attempted) = fx.legacy("p-arxiv");
        assert_eq!(
            local_path.as_deref(),
            Some(result.path.to_string_lossy().as_ref()),
            "local_path is still the papers/ copy, under its old name"
        );
        assert_eq!(status.as_deref(), Some("downloaded"));
        let attempted = attempted.expect("last_attempt_at is stamped");
        assert_eq!(
            row.retrieved_at, attempted,
            "the artefact and the legacy row carry one retrieval stamp, \
             because they are written by the same transaction"
        );
        assert!(result.path.exists(), "the legacy copy is on disk");
        assert_eq!(
            find_cached_file(&paper, &out_dir).as_deref(),
            Some(result.path.as_path()),
            "find_cached_file still resolves the download"
        );

        // --- and the bytes are in the content-addressed store ---
        let sha = row.sha256.expect("hashed");
        let stored = fx
            .root()
            .join("blobs")
            .join(&sha[..2])
            .join(format!("{sha}.pdf"));
        assert_eq!(
            std::fs::read(stored).expect("stored blob").as_slice(),
            PDF_BYTES,
            "the blob store holds the bytes, at a root-relative path"
        );
        assert_eq!(fx.count("blobs"), 1);
    }

    /// #262's ingest half: a malformed DOI is refused **before** anything is
    /// put on the wire, and the walk falls through to the paper's own `url`
    /// so no reachable work stops being reachable.
    ///
    /// The discriminating assertion is `requests.is_empty()`: the old code
    /// would have sent this DOI to Unpaywall and then to doi.org.
    #[tokio::test]
    async fn an_invalid_doi_is_never_fetched() {
        let fx = Fixture::new().await;
        // The exact shape from the #261 audit: an Elsevier compact DOI
        // truncated mid-parenthesis.
        let paper = save(
            &fx,
            "p-truncated",
            Some("10.1016/0003-2670(93"),
            None,
            None,
            None,
        );
        let out_dir = fx.papers_dir();

        // Nothing was mounted, so any request would 404 — but the point is
        // that none is made at all.
        let err = fx
            .downloader()
            .download_paper(&paper, &out_dir)
            .await
            .expect_err("a DOI with nothing else to try cannot succeed");
        assert!(
            matches!(err, AdapterError::NotFound(_)),
            "the fall-through must still end in the same NotFound: {err}"
        );

        assert_eq!(
            fx.requests().await,
            Vec::<String>::new(),
            "a rejected DOI is never fetched"
        );
        assert!(
            fx.pacer.grants().is_empty(),
            "so no permit is spent either: {:?}",
            fx.pacer.grants()
        );
        assert_eq!(fx.count("artefacts"), 0);
    }

    /// The whole point of the closed `RouteId` type: which route served the
    /// bytes is recorded once, in one spelling, and comes back out of the
    /// database as that route.
    #[tokio::test]
    async fn the_route_that_succeeded_is_recorded() {
        let fx = Fixture::new().await;
        let pdf_url = fx.serve_unpaywall_pdf("10.1038/s41586-020-2649-2").await;
        let paper = save(
            &fx,
            "p-unpaywall",
            Some("10.1038/s41586-020-2649-2"),
            None,
            None,
            None,
        );
        let out_dir = fx.papers_dir();

        let result = fx
            .downloader()
            .download_paper(&paper, &out_dir)
            .await
            .expect("the Unpaywall leg should serve the PDF");

        let row = fx.artefact("p-unpaywall").expect("an artefact row");
        assert_eq!(row.route, "unpaywall");
        assert_eq!(
            row.access_basis, "oa_license",
            "Unpaywall only ever serves free-to-read material"
        );
        assert_eq!(row.version, "unknown", "Unpaywall claims no version");
        assert_eq!(row.source_url.as_deref(), Some(pdf_url.as_str()));
        assert_eq!(result.route, RouteId::Unpaywall);
        assert_eq!(result.source(), "unpaywall");
        assert_eq!(result.publisher_url, None);
        assert_eq!(
            row.publisher.as_deref(),
            Some("nature"),
            "10.1038 is Nature, and this prefix is classified"
        );
    }

    /// #261: for 249 papers the two-valued verdict reported "no TDM route
    /// available" for a publisher the ladder had never classified. The fix
    /// is a `NULL` and a note that says what we actually know — no route was
    /// *evaluated* — which is why this asserts both the absence of the
    /// overclaim and the presence of the exact wording.
    #[tokio::test]
    async fn an_unknown_publisher_leaves_publisher_null() {
        let fx = Fixture::new().await;
        // `10.99999` is deliberately not in the registry.
        fx.serve_unpaywall_pdf("10.99999/some.suffix.12345").await;
        let paper = save(
            &fx,
            "p-unknown",
            Some("10.99999/some.suffix.12345"),
            None,
            None,
            None,
        );
        let out_dir = fx.papers_dir();

        let result = fx
            .downloader()
            .download_paper(&paper, &out_dir)
            .await
            .expect("the Unpaywall leg should serve the PDF");

        let row = fx.artefact("p-unknown").expect("an artefact row");
        assert_eq!(
            row.publisher, None,
            "an unclassified prefix must never get a guessed publisher"
        );
        let note = row.publisher_note.expect("an explanation is recorded");
        assert_eq!(
            note,
            RouteVerdict::PublisherUnknown {
                prefix: "99999".to_string()
            }
            .note(),
            "the note is that type's wording, verbatim"
        );
        assert!(
            !note.contains("no TDM route available"),
            "an unclassified prefix must not claim a TDM route was evaluated: {note}"
        );
        assert!(
            !note.contains("TDM route available"),
            "no route was evaluated at all, so no availability claim: {note}"
        );
        for name in ["Elsevier", "Springer", "Wiley", "Nature", "Unknown"] {
            assert!(!note.contains(name), "{note} names a publisher: {name}");
        }
        assert_eq!(result.publisher_note.as_deref(), Some(note.as_str()));
    }

    /// The blob store is content-addressed, so two works holding identical
    /// bytes cost one row and one file. This is what makes "re-download the
    /// same PDF from two places" cheap rather than quadratic.
    #[tokio::test]
    async fn two_downloads_of_identical_bytes_share_one_blob_row() {
        let fx = Fixture::new().await;
        fx.serve("/pdf/2005.07866.pdf", PDF_BYTES).await;
        fx.serve("/pdf/2005.07866v1.pdf", PDF_BYTES).await;
        let first = save(&fx, "p-one", None, Some("2005.07866"), None, None);
        let second = save(&fx, "p-two", None, Some("2005.07866v1"), None, None);
        let out_dir = fx.papers_dir();
        let downloader = fx.downloader();

        downloader
            .download_paper(&first, &out_dir)
            .await
            .expect("first download");
        downloader
            .download_paper(&second, &out_dir)
            .await
            .expect("second download");

        let rows = fx.artefacts();
        assert_eq!(rows.len(), 2, "one artefact row per work: {rows:?}");
        let (a, b) = (
            rows.iter().find(|r| r.paper_id == "p-one").expect("p-one"),
            rows.iter().find(|r| r.paper_id == "p-two").expect("p-two"),
        );
        assert_eq!(a.sha256, b.sha256, "identical bytes, identical digest");
        let sha = a.sha256.clone().expect("hashed");
        let blob: (String, i64, Option<String>, String) = fx
            .db
            .conn()
            .expect("conn")
            .query_row("SELECT sha256, bytes, mime, rel_path FROM blobs", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .expect("one blob row");
        assert_eq!(blob.0, sha);
        assert_eq!(blob.1, PDF_BYTES.len() as i64);
        assert_eq!(blob.2.as_deref(), Some("application/pdf"));
        assert_eq!(
            blob.3,
            format!("blobs/{}/{sha}.pdf", &sha[..2]),
            "stored root-relative, so a moved library still resolves it"
        );
        assert_eq!(
            fx.count("blobs"),
            1,
            "the second download must not add a second blob row"
        );
    }

    /// **A download that cannot be recorded is not a completed download.**
    ///
    /// `record_download` writes the `artefacts` row and the legacy columns
    /// in one transaction, so a recording failure has to roll the whole
    /// thing back — including the `blobs` row it had already inserted —
    /// rather than leave a half-written artefact behind. Here the work is
    /// not in the `papers` table, so the foreign key refuses the insert.
    ///
    /// The bytes were fetched and the legacy copy was written; the error
    /// still propagates, which is the behaviour being pinned: before S1 this
    /// returned `Ok`, and the library was left knowing nothing about a file
    /// the user was told had been downloaded.
    #[tokio::test]
    async fn a_failed_download_records_nothing() {
        let fx = Fixture::new().await;
        fx.serve("/pdf/2005.07866.pdf", PDF_BYTES).await;
        // Deliberately *not* saved: a `Paper` value that names a row which
        // does not exist.
        let mut paper = Paper::new("unsaved");
        paper.id = scitadel_core::models::PaperId::from("p-unsaved".to_string());
        paper.arxiv_id = Some("2005.07866".to_string());
        let out_dir = fx.papers_dir();

        let err = fx
            .downloader()
            .download_paper(&paper, &out_dir)
            .await
            .expect_err("a download that cannot be recorded is not a download");
        assert!(
            matches!(err, AdapterError::Db(_)),
            "the recording failure must propagate, not be logged away: {err}"
        );
        assert_eq!(
            fx.count("artefacts"),
            0,
            "no partial artefact row survives a failed recording"
        );
        assert_eq!(
            fx.count("blobs"),
            0,
            "and the transaction rolled the blobs row back with it"
        );
    }

    /// ADR-007 §4: "one `Work` grant per work per bucket". Four legs of one
    /// ladder walk is one work, so the walk must charge a `Work` permit
    /// once — not once per leg, which is what a caller that forgot to thread
    /// the scope ends up doing.
    #[tokio::test]
    async fn one_work_scope_charges_a_work_permit_once_across_the_whole_walk() {
        let fx = Fixture::new().await;
        // arXiv 404, OpenAlex 404, Unpaywall 404, doi.org serves the HTML.
        Mock::given(method("GET"))
            .and(path("/pdf/2005.07866.pdf"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&fx.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/works/W123"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&fx.server)
            .await;
        fx.serve("/v2/10.1038/s41586-020-2649-2", unpaywall_json("{}"))
            .await;
        fx.serve("/doi/10.1038/s41586-020-2649-2", HTML_BYTES).await;

        let paper = save(
            &fx,
            "p-walk",
            Some("10.1038/s41586-020-2649-2"),
            Some("2005.07866"),
            Some("W123"),
            None,
        );
        let result = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect("the publisher leg is the one that answers");

        assert_eq!(result.route, RouteId::Publisher);
        let requests = fx.pacer.spending(Cost::Request);
        assert_eq!(
            requests.len(),
            4,
            "four legs, one Request permit each: {:?}",
            fx.pacer.grants()
        );
        assert_eq!(
            fx.pacer.spending(Cost::Work).len(),
            1,
            "one Work permit for the whole walk, not one per leg: {:?}",
            fx.pacer.grants()
        );
        assert!(
            requests
                .iter()
                .all(|g| g.bucket == fx.pacer.spending(Cost::Work)[0].bucket),
            "every hop landed in the one bucket the work was charged for"
        );
        assert_eq!(fx.artefact("p-walk").expect("row").route, "publisher");
    }

    /// Tiers are per **call**, not per route. The Unpaywall leg is two
    /// requests on one route and two different budgets: the API lookup is
    /// metadata (`Meta`), the PDF it resolves to is an OA host (`Oa`).
    /// Spending the repositories budget on a metadata API is the mistake
    /// ADR-007 §4's table exists to prevent, in the other direction.
    #[tokio::test]
    async fn the_api_leg_is_meta_and_the_pdf_it_resolves_is_oa() {
        let fx = Fixture::new().await;
        fx.serve_unpaywall_pdf("10.1038/s41586-020-2649-2").await;
        let paper = save(
            &fx,
            "p-tiers",
            Some("10.1038/s41586-020-2649-2"),
            None,
            None,
            None,
        );

        fx.downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect("download");

        let requests: Vec<_> = fx.pacer.spending(Cost::Request);
        assert_eq!(requests.len(), 2, "lookup then PDF");
        assert_eq!(requests[0].tier, PaceTier::Meta, "the API lookup");
        assert_eq!(
            requests[1].tier,
            PaceTier::Oa,
            "the resolved PDF, off an OA host"
        );
        // `RouteId::Unpaywall::fetch_tier()` is `Meta` — one value for the
        // whole route, which is exactly why the chain cannot derive the
        // tier and has to state it per call.
        assert_eq!(RouteId::Unpaywall.fetch_tier(), Some(PaceTier::Meta));
    }

    /// The last-resort `url` leg is a real fetch that merely got its bytes
    /// from outside the chain, so it is `ManualUrl` — and the printed source
    /// line changes from `"url"` to `"manual_url"` because `"url"` is not one
    /// of the four pseudo-route spellings migration 013 documents.
    #[tokio::test]
    async fn the_last_resort_url_leg_is_recorded_as_manual_url() {
        let fx = Fixture::new().await;
        fx.serve("/record/10.1234/thing.pdf", HTML_BYTES).await;
        let url = format!("{}/record/10.1234/thing.pdf", fx.server.uri());
        let paper = save(&fx, "p-url", None, None, None, Some(&url));

        let result = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect("the record URL should serve HTML");

        assert_eq!(result.route, RouteId::ManualUrl);
        assert_eq!(result.source(), "manual_url");
        let row = fx.artefact("p-url").expect("row");
        assert_eq!(row.route, "manual_url");
        assert_eq!(row.kind, "fulltext_html");
        assert_eq!(
            row.access_basis, "subscription_read",
            "read under whatever entitlement the caller had"
        );
        assert_eq!(row.source_url.as_deref(), Some(url.as_str()));
        assert_eq!(result.publisher_url.as_deref(), Some(url.as_str()));
        assert_eq!(
            result.access,
            AccessStatus::Unknown,
            "a stub with no markers stays unknown, as before"
        );
    }

    /// A payload whose magic bytes are HTML but whose URL ends in `.pdf` is
    /// stored as the PDF it claimed to be — the chain's existing
    /// URL-extension heuristic, unchanged. Recorded here because it is the
    /// one place `format` and `bytes` can disagree, and the artefact row has
    /// to record what the *chain believed*, not what re-inspection would say.
    #[tokio::test]
    async fn a_publisher_stub_is_recorded_with_the_classification_it_got() {
        let fx = Fixture::new().await;
        let stub = "<html><body>
            <h1>Access options</h1>
            <button>Purchase access</button>
            <button>Institutional sign in</button>
            </body></html>";
        fx.serve("/doi/10.99999/some.suffix.12345", stub).await;
        let paper = save(
            &fx,
            "p-paywall",
            Some("10.99999/some.suffix.12345"),
            None,
            None,
            None,
        );

        let result = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect("the publisher page should be served");

        assert_eq!(result.access, AccessStatus::Paywall);
        let row = fx.artefact("p-paywall").expect("row");
        assert_eq!(row.access_status, "paywall");
        assert_eq!(row.kind, "fulltext_html");
        // The legacy column and the artefact row must agree, or "have" and
        // the TUI's state column would tell two different stories.
        assert_eq!(fx.legacy("p-paywall").1.as_deref(), Some("paywall"));
        assert_eq!(
            result.publisher_url.as_deref(),
            Some(format!("{}/doi/10.99999/some.suffix.12345", fx.server.uri()).as_str()),
            "a paywall result points the human at the DOI, not a CDN"
        );
    }
}
