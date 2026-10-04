//! The paper download chain, on `Route` + `PacedClient` (ADR-007 §3, S1).
//!
//! Six ladder steps in a fixed order — arXiv id → **preprint transform** →
//! OpenAlex → Unpaywall → publisher HTML — then the paper record's own `url`
//! as a last resort. Two things changed shape in S1; the outputs did not.
//!
//! ## The preprint leg runs before every index leg (#260)
//!
//! The step between arXiv and OpenAlex is not a search. For a `10.1101` or
//! `10.48550` DOI it is a pure function of the DOI, in
//! [`crate::preprint`], and it needs no index, no credential and no search —
//! which is the point, because the run that opened #260 missed three of three
//! bioRxiv preprints for exactly the opposite reason: OpenAlex's
//! `best_oa_location.pdf_url` was null and the paper was declared
//! unreachable by a publisher that serves it openly. It is charged
//! [`PaceTier::Oa`] like any other OA host and records
//! [`RouteId::Biorxiv`] or [`RouteId::Arxiv`].
//!
//! ## "No route tried" is not "no access" (#260)
//!
//! A walk that ends without bytes records an `acquisition_state` row, and the
//! status is derived from *what the legs actually concluded* rather than from
//! the fact that the walk ended. The distinction is the whole semantic fix:
//! an index that answers "I have no location for this" is not evidence that
//! the work is unobtainable, and a paper whose only leg was refused at the
//! DOI gate has not been looked for at all. See `Ladder` below.
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
    BlobWrite, Database, DownloadWrite, FULLTEXT_LOCATOR, SqlitePacer, StateWrite, blob_rel_path,
    fulltext_kind, hash_file, store_blob,
};
use scitadel_http::{
    BucketPolicyTable, FetchError, PacedClient, PacedResponse, SafeHeaders, WorkScope,
};

use crate::error::AdapterError;
use crate::openalex::OPENALEX_API_URL;
use crate::preprint::{self, PreprintBases};

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

/// The services the ladder talks to.
///
/// A value rather than several `const`s so a test can point the whole chain
/// at one `wiremock` server. The defaults are byte-for-byte the URLs this
/// chain has always used, which is what keeps "the same outputs as today"
/// true of the request lines as well as of the printed lines.
///
/// `arxiv_pdf` serves two different legs — the record's own `arxiv_id`
/// (which appends `.pdf`, the shape that chain has always sent) and the
/// `10.48550` DOI transform (which does not, because that is the URL that
/// was verified). One base, two paths, and the difference lives in
/// [`crate::preprint`] where it belongs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    /// Base of `GET {base}/{doi}?email=…`.
    pub unpaywall_api: String,
    /// Base of `GET {base}/{doi}` — the DOI resolver.
    pub doi_resolver: String,
    /// Base of `GET {base}/{id}.pdf`.
    pub arxiv_pdf: String,
    /// Base of `GET {base}/<doi>v<n>.full.pdf` — bioRxiv, which serves **both**
    /// `10.1101` prefixes (#260).
    pub biorxiv_content: String,
    /// Base of `GET {base}/{openalex_id}?mailto=…&api_key=…`.
    pub openalex_api: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            unpaywall_api: "https://api.unpaywall.org/v2".to_string(),
            doi_resolver: "https://doi.org".to_string(),
            arxiv_pdf: "https://arxiv.org/pdf".to_string(),
            biorxiv_content: PreprintBases::default().biorxiv_content,
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
    /// whatever row the DOI resolves to, exactly as before. For the same
    /// reason there is no `acquisition_state` row either: the gap table is
    /// keyed by `paper_id`.
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

        // The same transform, in the same place relative to the index legs,
        // for the same reason as in `download_paper`: `scitadel download <doi>`
        // is a user-facing way to ask for exactly the DOIs in #260, and
        // sending those to Unpaywall first is what reported them unreachable.
        let stem = doi_to_filename(&normalized);
        match self
            .try_preprint(&work, &normalized, &stem, output_dir)
            .await
        {
            Ok(fetched) => return self.finish(None, fetched).await,
            Err(e) => {
                tracing::info!(doi = %normalized, error = %e, "preprint transform failed, falling back to Unpaywall");
            }
        }

        match self.try_unpaywall(&work, &normalized, output_dir).await {
            Ok(fetched) => return self.finish(None, fetched).await,
            Err(e) => {
                tracing::info!(doi = %normalized, error = %e, "Unpaywall lookup failed, falling back to publisher");
            }
        }

        // The DOI-only path has no `papers` row to record a gap against, so
        // the leg error is simply propagated — exactly the error this
        // function returned before #260.
        let fetched = self
            .download_publisher_html(&work, &normalized, output_dir)
            .await
            .map_err(LegError::into_adapter)?;
        self.finish(None, fetched).await
    }

    /// Download a paper using every identifier available on the `Paper` record.
    ///
    /// Priority — the four original steps, with the preprint transform spliced
    /// in ahead of every index lookup (#260):
    /// 1. `arxiv_id` → direct arXiv PDF (no API call, always OA)
    /// 2. `doi` → the preprint transform, when the DOI belongs to a preprint
    ///    server we have a verified rule for (also no API call, always OA)
    /// 3. `openalex_id` → OpenAlex `/works` API for `best_oa_location.pdf_url`
    /// 4. `doi` → Unpaywall (existing path)
    /// 5. `url` → download the landing page as HTML (last resort)
    ///
    /// Step 2 sits ahead of step 3 because it needs no index at all: a
    /// preprint is free by construction, so asking an index where it might be
    /// free is the wrong first question for it — and an index that answers
    /// "no location" must never become the verdict, which is what #260 was.
    ///
    /// # Errors
    ///
    /// `AdapterError::NotFound` when the record offers nothing to try;
    /// otherwise whichever error stopped the walk, **including a failure to
    /// record a download that succeeded** — see the module docs.
    ///
    /// A walk that ends without bytes also records an `acquisition_state` row
    /// whose status is derived from what the legs concluded, so "nothing was
    /// tried" is never reported as "nothing is available". That record is
    /// best-effort and never masks the error the caller is about to receive.
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
        // One scope for the whole walk: every leg of one work is one work to
        // the publisher, so the `Work` permit is charged once per bucket and
        // not once per leg.
        let work = WorkScope::new();
        // What the legs concluded, for the `acquisition_state` row this walk
        // writes if it ends without bytes (#260). See `Ladder` below.
        let mut ladder = Ladder::default();

        // The DOI gate, hoisted above every leg that reads it, so the preprint
        // leg is gated by the same authority (#262) as the index legs instead
        // of by a second opinion of its own. The *set* of DOIs that reach the
        // wire is unchanged; what is new is that a rejection says why, and
        // that it is recorded as a route that was refused before it could be
        // tried rather than as a route that came back empty.
        let doi = match paper.doi.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(raw) => match validate_doi_detailed(raw) {
                Ok(normalized) => Some(normalized),
                Err(reason) => {
                    tracing::info!(
                        doi = %raw,
                        reason = %reason,
                        "DOI rejected before any fetch; skipping the preprint, Unpaywall \
                         and publisher legs (the paper's own URL is still tried)"
                    );
                    for leg in [Leg::Preprint, Leg::Unpaywall, Leg::Publisher] {
                        ladder.gate(
                            leg,
                            format!("the DOI was rejected before any fetch: {reason}"),
                        );
                    }
                    None
                }
            },
            None => None,
        };

        if let Some(id) = paper.arxiv_id.as_deref().filter(|s| !s.is_empty()) {
            match self.try_arxiv(&work, id, &stem, output_dir).await {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => {
                    tracing::info!(arxiv_id = %id, error = %e, "arxiv fallback failed");
                    ladder.record(Leg::ArxivId, e.outcome);
                }
            }
        } else {
            ladder.skip(Leg::ArxivId, "the record carries no arxiv_id");
        }

        // Ahead of every index leg (#260): where a preprint's full text lives
        // is a function of its DOI, so there is nothing to look up.
        if let Some(normalized) = doi.as_deref() {
            match self
                .try_preprint(&work, normalized, &stem, output_dir)
                .await
            {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => {
                    tracing::info!(doi = %normalized, error = %e, "preprint transform failed");
                    ladder.record(Leg::Preprint, e.outcome);
                }
            }
        } else {
            ladder.skip(Leg::Preprint, "the record carries no usable doi");
        }

        if let Some(id) = paper.openalex_id.as_deref().filter(|s| !s.is_empty()) {
            match self.try_openalex(&work, id, &stem, output_dir).await {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => {
                    tracing::info!(openalex_id = %id, error = %e, "openalex fallback failed");
                    ladder.record(Leg::OpenAlex, e.outcome);
                }
            }
        } else {
            ladder.skip(Leg::OpenAlex, "the record carries no openalex_id");
        }

        if let Some(normalized) = doi.as_deref() {
            match self.try_unpaywall(&work, normalized, output_dir).await {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => {
                    tracing::info!(doi = %normalized, error = %e, "unpaywall fallback failed");
                    ladder.record(Leg::Unpaywall, e.outcome);
                }
            }
            match self
                .download_publisher_html(&work, normalized, output_dir)
                .await
            {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => {
                    tracing::info!(doi = %normalized, error = %e, "publisher html fallback failed");
                    ladder.record(Leg::Publisher, e.outcome);
                }
            }
        } else {
            // Recorded in walk order, so the reason enumerates every leg the
            // walk considered — including the two it could not even start.
            ladder.skip(Leg::Unpaywall, "the record carries no usable doi");
            ladder.skip(Leg::Publisher, "the record carries no usable doi");
        }

        if let Some(url) = paper.url.as_deref().filter(|s| !s.is_empty()) {
            match self
                .download_url_as_html(&work, url, &stem, output_dir)
                .await
            {
                Ok(fetched) => return self.finish(Some(paper), fetched).await,
                Err(e) => {
                    // The same error this walk has always returned for a
                    // failing last resort. The state row is extra
                    // information about *why*, not a substitute for it, so it
                    // is written first and can never replace this.
                    let (error, outcome) = e.into_parts();
                    ladder.record(Leg::ManualUrl, outcome);
                    self.record_gap(paper, &ladder);
                    return Err(error);
                }
            }
        }
        ladder.skip(Leg::ManualUrl, "the record carries no url");

        self.record_gap(paper, &ladder);
        Err(AdapterError::NotFound(
            "no arxiv_id, openalex_id, doi, or url to try".into(),
        ))
    }

    /// Record what the walk concluded as an `acquisition_state` gap.
    ///
    /// Best-effort by design, and the reason is worth stating: this row is
    /// *about a failure*, so it must never become the failure the caller sees.
    /// A work that is not in `papers` (a hand-built `Paper`, an MCP agent
    /// racing a delete) makes the foreign key refuse, and the walk's own
    /// error is the one worth returning. So a failure here is logged and
    /// dropped, where a failure in [`Self::finish`] propagates — that one is
    /// about a download that *succeeded*, which is a different thing.
    fn record_gap(&self, paper: &Paper, ladder: &Ladder) {
        // Ownership, not a cache. `status` and `reason` are this walk's to
        // establish; `next_attempt_at` belongs to whoever scheduled the retry
        // (ADR-007 §2's "retried with backoff by `acquire`"), and a single walk
        // has no reason to cancel a schedule a campaign set. Carrying it over
        // is what keeps a re-run a no-op: clearing it here would make every
        // second walk differ from the last, so `write_acquisition_states`'s
        // `WHERE` guard would fire and move `updated_at` on every re-run.
        let scheduled_retry = self
            .db
            .acquisition_state(paper.id.as_str(), GAP_FULLTEXT_KIND, FULLTEXT_LOCATOR)
            .ok()
            .flatten()
            .and_then(|row| row.next_attempt_at);
        let row = StateWrite {
            paper_id: paper.id.as_str().to_string(),
            kind: GAP_FULLTEXT_KIND.to_string(),
            locator: FULLTEXT_LOCATOR.to_string(),
            wanted_version: WANTED_VERSION_VOR.to_string(),
            status: ladder.status().to_string(),
            reason: Some(ladder.reason()),
            // `None` on purpose: this walk evaluated *no* publisher's routes
            // and named no publisher, and `classify_publisher` is the one
            // authority allowed to fill that column (#261). The report-time
            // grouping in `sqlite::coverage` derives it from the work's DOI
            // registry answer rather than reading it back from here, so
            // storing it would add a second, unchecked copy of the same fact.
            publisher: None,
            hint_url: self.doi_hint_url(paper),
            // Deliberately `None`: there is no file for a human to drop here.
            // That absence is also what distinguishes this row from the flat
            // importer's, which is retracted by its exact `drop_path`.
            drop_path: None,
            // Deliberately `None` too: a single walk does not know when the
            // work should be tried again. Deciding that is the *campaign*'s
            // job (`acquire`, ADR-007 §2's "retried with backoff"), and it is
            // the campaign that can read a bucket's ledger — so what this row
            // carries is whatever the campaign already scheduled, never a
            // fresh guess.
            next_attempt_at: scheduled_retry,
            updated_at: Utc::now().to_rfc3339(),
        };
        if let Err(e) = self.db.upsert_acquisition_state(&row) {
            tracing::warn!(
                paper_id = %paper.id,
                status = %row.status,
                error = %e,
                "could not record the acquisition gap for this work; the walk's own \
                 outcome is unaffected"
            );
        }
    }

    /// The DOI resolver page for this work, as the `hint_url` a human should
    /// be pointed at — the one URL that is canonical for a paper whatever
    /// route the ladder took or failed to take.
    fn doi_hint_url(&self, paper: &Paper) -> Option<String> {
        let raw = paper.doi.as_deref().filter(|s| !s.trim().is_empty())?;
        validate_doi_detailed(raw)
            .ok()
            .map(|doi| self.endpoints.doi_url(&doi))
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
    ) -> Result<PacedResponse, LegError> {
        let url = Url::parse(raw_url).map_err(|e| {
            // Deliberately not echoing `raw_url`: the OpenAlex leg's URL
            // carries the API key as a query parameter, and an error string
            // is a log line.
            LegError::gated(AdapterError::Other(format!(
                "could not build a request URL: {e}"
            )))
        })?;
        self.client
            .get_in_work(work, url, tier, SafeHeaders::unauthenticated())
            .await
            .map_err(|e| fetch_failure(what, e))
    }

    /// Try the DOI's deterministic preprint transform (#260).
    ///
    /// Ahead of every index leg, and for a reason that is about *access*
    /// rather than about ordering: a preprint server serves its postings
    /// openly and always has, so the ladder's first question about a `10.1101`
    /// or `10.48550` DOI is not "where might this be free?" — which is what
    /// OpenAlex was asked, and which answered null for every preprint in the
    /// corpus that opened the issue — but "where does this DOI certainly
    /// live?", which is a function of the DOI.
    ///
    /// Each candidate is charged [`PaceTier::Oa`] like any other OA host and
    /// records the route that served it, so an artefact from this leg carries
    /// `RouteId::Biorxiv` (or `Arxiv`) and inherits `version = 'preprint'` and
    /// `access_basis = 'oa_license'` from the route rather than from a guess
    /// made here.
    ///
    /// # Errors
    ///
    /// [`LegError`], with the *last* candidate's outcome when every candidate
    /// missed: the un-versioned bioRxiv URL is the more permissive of the two,
    /// so its verdict is the one that describes the leg as a whole.
    async fn try_preprint(
        &self,
        work: &WorkScope,
        doi: &str,
        stem: &str,
        output_dir: &Path,
    ) -> Result<Fetched, LegError> {
        let candidates = preprint::preprint_candidates_at(&self.preprint_bases(), doi);
        let mut last: Option<LegError> = None;
        for candidate in candidates {
            match self
                .preprint_candidate(work, &candidate, stem, output_dir)
                .await
            {
                Ok(fetched) => {
                    tracing::info!(
                        doi = %doi,
                        url = %candidate.url,
                        route = candidate.route.label(),
                        note = candidate.note,
                        "preprint transform served the PDF with no index lookup"
                    );
                    return Ok(fetched);
                }
                Err(e) => {
                    tracing::info!(
                        doi = %doi,
                        url = %candidate.url,
                        error = %e,
                        "preprint candidate missed"
                    );
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| {
            LegError::skipped(preprint::no_transform_reason(doi).unwrap_or(NO_PREPRINT_ROUTE))
        }))
    }

    /// Fetch one preprint candidate and classify what came back.
    async fn preprint_candidate(
        &self,
        work: &WorkScope,
        candidate: &preprint::PreprintCandidate,
        stem: &str,
        output_dir: &Path,
    ) -> Result<Fetched, LegError> {
        let url = &candidate.url;
        let response = self
            .get(work, url, PaceTier::Oa, candidate.route.label())
            .await?;
        // The arXiv transform carries no `.pdf` extension, so the URL cannot
        // decide the format on its own — the same rule the OpenAlex leg
        // already applies, shared rather than re-derived.
        let is_pdf = looks_like_pdf(url, response.content_type());
        // Read the URL off before `bytes()` consumes the response: this is the
        // post-redirect URL, which is where the bytes actually live.
        let source_url = response.url.to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| fetch_failure("failed to read preprint bytes", e))?;

        let (ext, format, access) = if is_pdf {
            (PDF_EXT, DownloadFormat::Pdf, AccessStatus::FullText)
        } else {
            (HTML_EXT, DownloadFormat::Html, classify_html(&bytes))
        };

        Ok(Fetched {
            reference: candidate.url.clone(),
            path: output_dir.join(format!("{stem}.{ext}")),
            bytes,
            ext,
            format,
            access,
            // A preprint server serves preprints: the route says so, and the
            // `preprint` version claim comes from the route rather than from a
            // judgement made here.
            route: candidate.route,
            source_url,
            // The PDF we asked for, which is also the posting's canonical
            // page — a human handed this URL sees the preprint.
            publisher_url: Some(candidate.url.clone()),
        })
    }

    /// The servers the preprint transforms are built against, taken from this
    /// downloader's endpoints so one rule serves production and tests alike.
    fn preprint_bases(&self) -> PreprintBases {
        PreprintBases {
            biorxiv_content: self.endpoints.biorxiv_content.clone(),
            arxiv_pdf: self.endpoints.arxiv_pdf.clone(),
        }
    }

    /// Query Unpaywall for an open-access PDF URL and download it.
    async fn try_unpaywall(
        &self,
        work: &WorkScope,
        doi: &str,
        output_dir: &Path,
    ) -> Result<Fetched, LegError> {
        // The polite-pool email is always present, even empty — that is the
        // request line this chain has always sent.
        let url = self.endpoints.unpaywall_url(doi, &self.openalex.email);
        let text = self
            .get(work, &url, PaceTier::Meta, "Unpaywall API")
            .await?
            .text()
            .await
            .map_err(|e| fetch_failure("Unpaywall JSON parse failed", e))?;
        let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            LegError::inconclusive(AdapterError::Parse(format!(
                "Unpaywall JSON parse failed: {e}"
            )))
        })?;

        let pdf_url = body
            .get("best_oa_location")
            .and_then(|loc| loc.get("url_for_pdf"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                // An index that answers "no location" is **not** evidence that
                // the work is unobtainable. That single null field is what made
                // #260 report three free preprints as unreachable, so this
                // outcome is classified separately from a refusal and can never
                // become a `no access` verdict.
                LegError::no_location(AdapterError::NotFound(
                    "no open-access PDF found via Unpaywall".into(),
                ))
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
    ) -> Result<Fetched, LegError> {
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
    ) -> Result<Fetched, LegError> {
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
    ) -> Result<Fetched, LegError> {
        let id = openalex_id.trim_start_matches("https://openalex.org/");
        // Same credential pair as the search path: an unauthenticated
        // lookup is billed against the shared per-IP budget and starts
        // 429-ing once it's spent (#212). `Url`'s own encoder replaces the
        // hand-rolled `?{k}={v}` splicing, so the two parameters are sent
        // with the same names in the same order as before.
        let mut api_url =
            Url::parse(&format!("{}/{id}", self.endpoints.openalex_api)).map_err(|e| {
                LegError::gated(AdapterError::Other(format!(
                    "could not build the OpenAlex URL: {e}"
                )))
            })?;
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
        let body: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            LegError::inconclusive(AdapterError::Parse(format!(
                "OpenAlex JSON parse failed: {e}"
            )))
        })?;

        let pdf_url = body
            .get("best_oa_location")
            .and_then(|loc| loc.get("pdf_url"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                body.get("open_access")
                    .and_then(|oa| oa.get("oa_url"))
                    .and_then(serde_json::Value::as_str)
            })
            // As with Unpaywall: a null `pdf_url` is a fact about OpenAlex's
            // index, not about the work's accessibility. It must never become
            // the verdict (#260).
            .ok_or_else(|| {
                LegError::no_location(AdapterError::NotFound(
                    "OpenAlex reports no OA location".into(),
                ))
            })?;

        // Metadata API to find it, OA host to serve it: two tiers, one route.
        let response = self.get(work, pdf_url, PaceTier::Oa, "OA URL").await?;
        let is_pdf = looks_like_pdf(pdf_url, response.content_type());
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
    ) -> Result<Fetched, LegError> {
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
    ) -> Result<(Vec<u8>, String), LegError> {
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

/// The two full-text serialisations these legs have ever stored.
///
/// `fulltext_kind` turns each into the `kind` / `format` / `mime` triple on
/// the way into the database, so there is one authority for what a `.pdf`
/// is. These are the only two extensions the chain can produce, which is
/// why it never has to decide what to do about a `.docx`.
const PDF_EXT: &str = "pdf";
const HTML_EXT: &str = "html";

/// The `acquisition_state.kind` the ladder records its gap against.
///
/// The want is "a full text", not "a PDF": a work satisfied by an HTML
/// landing page with its abstract has not had its full text obtained, and the
/// gap must survive that. The serialisation belongs on the artefact row, which
/// is where `kind = 'fulltext_pdf'` already lives.
const GAP_FULLTEXT_KIND: &str = "fulltext";

/// `acquisition_state.wanted_version` for a ladder gap.
///
/// The version of record, matching ADR-007 §2's default and what the flat
/// importer records: a ladder gap is a gap against the published article,
/// even when the only thing we could find was a preprint.
const WANTED_VERSION_VOR: &str = "vor";

/// The reason recorded when a DOI belongs to no preprint server this module
/// has a rule for.
const NO_PREPRINT_ROUTE: &str = "the DOI belongs to no preprint server with a verified route";

/// One step of the ladder, named for a report rather than for a call site.
///
/// The reason this is an enum and not a `&'static str` per log line: the
/// `acquisition_state.reason` this walk writes has to enumerate the routes it
/// considered, and a set of hand-written strings cannot be counted on to stay
/// in step with the walk that produces them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Leg {
    /// The record's own `arxiv_id`, fetched directly.
    ArxivId,
    /// The DOI's preprint transform (#260).
    Preprint,
    /// OpenAlex `/works/{id}`.
    OpenAlex,
    /// Unpaywall `/v2/{doi}` and the PDF it resolves to.
    Unpaywall,
    /// The publisher page the DOI resolver lands on.
    Publisher,
    /// The paper record's own `url`.
    ManualUrl,
}

impl Leg {
    /// How the leg is named in the recorded reason.
    fn name(self) -> &'static str {
        match self {
            Self::ArxivId => "arxiv id",
            Self::Preprint => "preprint transform",
            Self::OpenAlex => "openalex",
            Self::Unpaywall => "unpaywall",
            Self::Publisher => "doi.org publisher page",
            Self::ManualUrl => "record url",
        }
    }
}

/// What one leg of the ladder concluded.
///
/// The classification is the whole of #260's semantic fix, so each variant
/// answers a *different* question. The two that must never be confused are
/// [`Self::NoLocation`] — an index that answered and named nothing, which is
/// a fact about the index — and [`Self::Refused`] — a place that was asked and
/// said no. Reading the first as the second is what reported three free
/// bioRxiv preprints as unreachable, and it is why a walk cannot derive its
/// status from the fact that it ended without bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LegOutcome {
    /// No route applied to this work: no identifier to try, or a DOI no
    /// verified transform claims. Contributes nothing either way — there was
    /// nothing to evaluate.
    Skipped { why: String },
    /// A route applied and was refused before it could be tried: a DOI
    /// rejected at the gate, a candidate URL that would not parse. The route
    /// exists and is unevaluated, so no verdict may rest on it.
    Gated { why: String },
    /// Asked, and the place refused: a 4xx that is not 408 or 429.
    Refused { status: u16 },
    /// Asked, and the route had nothing to give: an index answered 200 and
    /// named no OA location. **Not** evidence about access.
    NoLocation { detail: String },
    /// Asked, and we could not find out: transport failure, pacing refusal, a
    /// login redirect, a 5xx, an unparseable body.
    Inconclusive { detail: String },
}

impl LegOutcome {
    /// Did this leg put something on the wire?
    fn was_attempted(&self) -> bool {
        matches!(
            self,
            Self::Refused { .. } | Self::NoLocation { .. } | Self::Inconclusive { .. }
        )
    }

    /// Did the place itself say no?
    fn was_refused(&self) -> bool {
        matches!(self, Self::Refused { .. })
    }

    /// The clause this outcome contributes to the recorded reason.
    fn clause(&self) -> String {
        match self {
            Self::Skipped { why } => format!("not applicable: {why}"),
            Self::Gated { why } => format!("refused before it could be tried: {why}"),
            Self::Refused { status } => format!("refused with HTTP {status}"),
            Self::NoLocation { detail } => {
                // Spelled out because this is the sentence a reader most needs
                // to be told is *not* a verdict about the work.
                format!("named no location ({detail}) — not evidence that the work is unavailable")
            }
            Self::Inconclusive { detail } => format!("could not be resolved: {detail}"),
        }
    }
}

/// A failed leg, carrying both the error the walk has always returned and the
/// classification the ladder records.
///
/// Deliberately *not* a replacement for [`AdapterError`]: the caller of
/// `download_paper` gets exactly the error it got before #260, because this
/// slice must not change what a caller sees — only what the library records
/// about the walk.
#[derive(Debug)]
struct LegError {
    error: AdapterError,
    outcome: LegOutcome,
}

impl LegError {
    /// A place that was asked and said no.
    fn refused(status: u16, message: String) -> Self {
        Self {
            error: AdapterError::Network(message),
            outcome: LegOutcome::Refused { status },
        }
    }

    /// A route that answered and had nothing to give.
    fn no_location(error: AdapterError) -> Self {
        Self {
            outcome: LegOutcome::NoLocation {
                detail: error.to_string(),
            },
            error,
        }
    }

    /// A failure that says nothing about access either way.
    fn inconclusive(error: AdapterError) -> Self {
        Self {
            outcome: LegOutcome::Inconclusive {
                detail: error.to_string(),
            },
            error,
        }
    }

    /// A route that existed and was refused before it could be tried.
    fn gated(error: AdapterError) -> Self {
        Self {
            outcome: LegOutcome::Gated {
                why: error.to_string(),
            },
            error,
        }
    }

    /// No route applied — recorded for the preprint leg's absent transform,
    /// and named rather than left as a bare `true`.
    fn skipped(why: &'static str) -> Self {
        Self {
            error: AdapterError::NotFound(why.to_string()),
            outcome: LegOutcome::Skipped {
                why: why.to_string(),
            },
        }
    }

    /// The error the walk has always returned for this failure.
    fn into_adapter(self) -> AdapterError {
        self.error
    }

    /// The error and the outcome, without cloning either — used by the
    /// last-resort leg, whose failure is both recorded and returned.
    fn into_parts(self) -> (AdapterError, LegOutcome) {
        (self.error, self.outcome)
    }
}

impl std::fmt::Display for LegError {
    /// Renders exactly what the walk has always logged for this failure.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}

/// What the walk tried, and what each leg concluded.
///
/// The input to the `acquisition_state.status` this walk records, and the
/// reason #260's coverage claim could not be honest without it. The status is
/// derived here — as a pure function of the outcomes, so it is testable
/// without a database or a network — and the reason enumerates every leg in
/// walk order, because "which routes were attempted" is the other half of the
/// acceptance criterion.
///
/// ## The derivation, and why these three values
///
/// From ADR-007 §2's table, without a migration:
///
/// - **`pending`** ("never tried", picked up again by `acquire`) when nothing
///   was put on the wire for this work, *or* when a route existed and was
///   refused at the gate. This is the "no route tried" case of the issue's
///   third item, and it is the value the walk used to be unable to
///   distinguish from a wall. A gate is `pending` rather than `error` because
///   a malformed DOI is not a transient failure: retrying it unchanged
///   achieves nothing, and this row's action group is where a human who fixes
///   the DOI belongs.
/// - **`unavailable`** only when at least one leg was attempted and *every*
///   attempted leg was refused by the place itself. That is the one case in
///   which "there is no such full text here" is supported by evidence rather
///   than by exhaustion.
/// - **`error`** ("transient failure", retry with backoff) when legs were
///   attempted but at least one of them could not give a yes or a no — a
///   transport failure, a 5xx, a pacing refusal, or an index that named no
///   location. Deliberately **not** `unavailable`: an incomplete index is the
///   documented failure of #260, so it must not be able to produce that
///   verdict. Deliberately not `needs_ill` either — that is the ADR's "no
///   route exists", and this ladder evaluates only six of the routes, so
///   claiming it would be #261's overclaim in a new place.
///   Deliberately not `rate_limited`, which is a statement about a *bucket*,
///   while this row is a statement about a work.
#[derive(Debug, Default)]
struct Ladder {
    attempts: Vec<(Leg, LegOutcome)>,
}

impl Ladder {
    /// Record a leg that failed, with the classification its error carries.
    fn record(&mut self, leg: Leg, outcome: LegOutcome) {
        self.attempts.push((leg, outcome));
    }

    /// Record a leg that did not apply to this work at all.
    fn skip(&mut self, leg: Leg, why: &str) {
        self.record(
            leg,
            LegOutcome::Skipped {
                why: why.to_string(),
            },
        );
    }

    /// Record a route that was refused before it could be tried.
    fn gate(&mut self, leg: Leg, why: String) {
        self.record(leg, LegOutcome::Gated { why });
    }

    /// The `acquisition_state.status` these outcomes support.
    fn status(&self) -> &'static str {
        let attempted: Vec<&LegOutcome> = self
            .attempts
            .iter()
            .map(|(_, outcome)| outcome)
            .filter(|outcome| outcome.was_attempted())
            .collect();

        if attempted.is_empty() || self.any_gated() {
            // Nothing was put on the wire for this work, or a route that applied
            // to it was refused before it could be tried. Both are "never
            // tried" in ADR-007 §2's sense — including the second, because a
            // malformed DOI is not transient: retrying it unchanged achieves
            // nothing, and `pending`'s action group ("picked up again by
            // `acquire`") is where a human who fixes the DOI belongs.
            STATUS_PENDING
        } else if attempted.iter().all(|outcome| outcome.was_refused()) {
            // Every route we could evaluate was asked, and every one refused.
            STATUS_UNAVAILABLE
        } else {
            // We asked, and at least one answer was not a no. Retryable, and
            // emphatically not a verdict about the work.
            STATUS_ERROR
        }
    }

    /// Was any applicable route refused before it could be tried?
    fn any_gated(&self) -> bool {
        self.attempts
            .iter()
            .any(|(_, outcome)| matches!(outcome, LegOutcome::Gated { .. }))
    }

    /// Which routes were attempted, and what each one said — the other half
    /// of the acceptance criterion, and the column a human reads to decide
    /// whether to go and look for the paper themselves.
    fn reason(&self) -> String {
        if self.attempts.is_empty() {
            return "no full text obtained; no route applied to this work".to_string();
        }
        let clauses: Vec<String> = self
            .attempts
            .iter()
            .map(|(leg, outcome)| format!("{} {}", leg.name(), outcome.clause()))
            .collect();
        format!(
            "no full text obtained; the walk considered {} route(s). {}",
            self.attempts.len(),
            clauses.join("; ")
        )
    }
}

/// `acquisition_state.status`: the work is wanted and has not been got, and
/// nothing has established that it cannot be.
const STATUS_PENDING: &str = "pending";
/// `acquisition_state.status`: every route that could be evaluated refused.
const STATUS_UNAVAILABLE: &str = "unavailable";
/// `acquisition_state.status`: a route was tried and could not be resolved.
const STATUS_ERROR: &str = "error";

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
///
/// `pub(crate)` because [`crate::acquire`] fills the same two fields on an
/// `acquisition_state` row: the two halves of `PublisherVerdict` must have one
/// implementation per crate, or #261 is one edited function away from coming
/// back.
pub(crate) fn publisher_columns(doi: Option<&str>) -> (Option<String>, Option<String>) {
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

/// Does this response look like a PDF?
///
/// One rule for the two legs that fetch a URL whose extension cannot always
/// be trusted: the arXiv DOI transform (`/pdf/2301.00001`, no extension) and
/// the OpenAlex-resolved URL (whatever an index pointed at). The
/// `fulltext_kind` derived from the extension is what makes the row's `kind`
/// honest, so getting this wrong files an HTML stub as a PDF — and it is
/// shared rather than re-derived per leg for exactly that reason.
fn looks_like_pdf(url: &str, content_type: Option<&str>) -> bool {
    url.to_lowercase().ends_with(".pdf")
        || content_type.is_some_and(|ct| ct.contains("application/pdf"))
}

/// Map a paced fetch failure to the message this chain has always logged, and
/// to the outcome the ladder records against it (#260).
///
/// The `Status` / `RateLimited` arms carry the status code and **nothing
/// else**. `FetchError`'s `Display` includes the request URL, and the
/// OpenAlex leg's URL carries `api_key` as a query parameter — forwarding
/// it verbatim would turn "the key is in the request line" (#276, which
/// tracks the real fix) into "the key is in every log line". The transport
/// arm keeps the URL, which is where `reqwest` has always put it and where
/// it earns its keep: an unreachable host.
///
/// The classification is done **here**, while the typed error is in hand, and
/// not later by re-reading the message: a 404 and a connection failure produce
/// nearly the same sentence, and the difference between "this server will not
/// serve it" and "we could not find out" is the entire content of the status
/// this walk records.
fn fetch_failure(what: &str, error: FetchError) -> LegError {
    match error {
        FetchError::Status { code, .. } if is_refusal(code) => {
            LegError::refused(code, format!("{what} returned status {code}"))
        }
        FetchError::Status { code, .. } | FetchError::RateLimited { code, .. } => {
            LegError::inconclusive(AdapterError::Network(format!(
                "{what} returned status {code}"
            )))
        }
        // A login redirect says a route exists that needs a human's session,
        // not that the work is unobtainable, so it is inconclusive here: the
        // tier-4 leg has not been taken. Same for a pacing refusal, which is
        // our own budget rather than the publisher's answer.
        other => LegError::inconclusive(AdapterError::Network(format!("{what}: {other}"))),
    }
}

/// Is this HTTP status a refusal — "this server will not serve this to us"?
///
/// 4xx is the client's answer and is decisive about *that* place: 403 is an
/// access decision, 404 and 410 are a statement that it is not there. The two
/// exceptions are 408 (the server gave up waiting, which says nothing about
/// access) and 429 (the publisher asked us to slow down, which is the opposite
/// of a verdict). Everything 5xx is a failure on the server's side, and
/// nothing there is evidence about whether a human could read the paper.
fn is_refusal(code: u16) -> bool {
    (400..500).contains(&code) && !matches!(code, 408 | 429)
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
                    biorxiv_content: format!("{base}/content"),
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

        /// Only the requests whose URL contains `needle` — how a test says
        /// "this leg was never called" without counting unrelated traffic.
        async fn requests_to(&self, needle: &str) -> Vec<String> {
            self.requests()
                .await
                .into_iter()
                .filter(|url| url.contains(needle))
                .collect()
        }

        /// Mount a PDF the way a real server serves one: with the
        /// `Content-Type` header that decides `format` when the URL itself
        /// cannot (the arXiv transform has no file extension).
        async fn serve_pdf(&self, route: &str) {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "application/pdf")
                        .set_body_bytes(PDF_BYTES.to_vec()),
                )
                .mount(&self.server)
                .await;
        }

        /// Mount a 404 for a path, so a leg misses without the walk stopping.
        async fn miss(&self, route: &str) {
            Mock::given(method("GET"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(404))
                .mount(&self.server)
                .await;
        }

        /// The `acquisition_state` rows for a work, as
        /// `(status, reason, hint_url, drop_path)`.
        fn states(&self, paper_id: &str) -> Vec<StateRow> {
            let conn = self.db.conn().expect("conn");
            let mut stmt = conn
                .prepare(
                    "SELECT kind, locator, wanted_version, status, reason, hint_url, drop_path
                     FROM acquisition_state WHERE paper_id = ?1
                     ORDER BY kind, locator",
                )
                .expect("prepare");
            stmt.query_map([paper_id], |r| {
                Ok(StateRow {
                    kind: r.get(0)?,
                    locator: r.get(1)?,
                    wanted_version: r.get(2)?,
                    status: r.get(3)?,
                    reason: r.get(4)?,
                    hint_url: r.get(5)?,
                    drop_path: r.get(6)?,
                })
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
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

    /// One `acquisition_state` row as the ladder wrote it (#260).
    #[derive(Debug)]
    struct StateRow {
        kind: String,
        locator: String,
        wanted_version: String,
        status: String,
        reason: Option<String>,
        hint_url: Option<String>,
        drop_path: Option<String>,
    }

    fn unpaywall_json(pdf_url: &str) -> String {
        format!(r#"{{"best_oa_location":{{"url_for_pdf":"{pdf_url}"}}}}"#)
    }

    /// An index that answered and named no OA location — the response shape
    /// that made #260 report three free preprints as unreachable.
    fn openalex_json_without_a_location() -> String {
        r#"{"id":"W1","best_oa_location":null,"open_access":{"is_oa":false}}"#.to_string()
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

    // =====================================================================
    // #260: the preprint leg, and an honest verdict when it misses.
    //
    // The corpus that opened the issue reported three bioRxiv preprints as
    // unreachable. Each was free, each was served by `biorxiv.org`, and the
    // only reason the ladder said otherwise was that it had no preprint route
    // and asked an index instead.
    // =====================================================================

    /// The acceptance criterion, verbatim: a preprint DOI is obtained **with no
    /// network index lookup**. The work carries an `openalex_id` precisely so
    /// that "no OpenAlex request was made" is a statement about the *order* of
    /// the ladder and not about the record happening to lack the identifier.
    ///
    /// It also pins the two things that make the transform worth having: the
    /// versioned bioRxiv URL is the one that was tried first, and the route
    /// recorded on the artefact is the server that served it — with the
    /// `preprint` version and `oa_license` basis the route implies, which the
    /// database derives rather than this code guessing.
    #[tokio::test]
    async fn a_preprint_doi_is_fetched_before_the_openalex_leg() {
        let fx = Fixture::new().await;
        fx.serve_pdf("/content/10.1101/2025.06.14.659707v1.full.pdf")
            .await;
        // Mounted, and must never be called.
        fx.serve(
            "/works/W123",
            r#"{"best_oa_location":{"pdf_url":"http://example.invalid/never.pdf"}}"#,
        )
        .await;
        fx.serve("/v2/10.1101/2025.06.14.659707", unpaywall_json("{}"))
            .await;
        let paper = save(
            &fx,
            "p-preprint",
            Some("10.1101/2025.06.14.659707"),
            None,
            Some("W123"),
            None,
        );

        let result = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect("the preprint transform should serve the PDF");

        assert_eq!(result.route, RouteId::Biorxiv);
        assert_eq!(result.format, DownloadFormat::Pdf);
        assert_eq!(result.access, AccessStatus::FullText);
        assert!(
            fx.requests_to("/works/W123").await.is_empty(),
            "the preprint leg needs no index, so OpenAlex must not be asked: {:?}",
            fx.requests().await
        );
        assert!(
            fx.requests_to("/v2/10.1101").await.is_empty(),
            "nor Unpaywall: {:?}",
            fx.requests().await
        );
        assert_eq!(
            fx.requests().await.len(),
            1,
            "one request, one hop: {:?}",
            fx.requests().await
        );

        let row = fx.artefact("p-preprint").expect("an artefact row");
        assert_eq!(
            row.route, "biorxiv",
            "the serving route is recorded, not guessed"
        );
        assert_eq!(
            row.version, "preprint",
            "a bioRxiv PDF is a preprint by construction, and the route says so"
        );
        assert_eq!(
            row.access_basis, "oa_license",
            "a preprint server serves material free to read by construction"
        );
        assert_eq!(row.kind, "fulltext_pdf");
        assert_eq!(row.publisher.as_deref(), Some("biorxiv"));
        assert_eq!(
            row.source_url.as_deref(),
            Some(
                format!(
                    "{}/content/10.1101/2025.06.14.659707v1.full.pdf",
                    fx.server.uri()
                )
                .as_str()
            ),
            "the bytes came from the verified transform URL"
        );
        assert_eq!(fx.legacy("p-preprint").1.as_deref(), Some("downloaded"));
        assert!(
            fx.states("p-preprint").is_empty(),
            "a work that was obtained records no gap: {:?}",
            fx.states("p-preprint")
        );
        // One Request permit, at the OA tier: a preprint server is an OA host.
        let requests = fx.pacer.spending(Cost::Request);
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(requests[0].tier, PaceTier::Oa);
    }

    /// A DOI the preprint leg cannot serve must cost the walk a fall-through,
    /// not the work a verdict. Both verified bioRxiv candidates 404 here, and
    /// the Unpaywall leg then finds the PDF — which is the shape of a preprint
    /// whose `v1` path has moved on.
    ///
    /// The discriminating assertions are the two 404s (both candidates were
    /// really tried, in order) and the absence of any `unavailable` row: a
    /// ladder that treated a preprint miss as a wall would stop here.
    #[tokio::test]
    async fn an_unavailable_preprint_falls_through_rather_than_being_reported_unreachable() {
        let fx = Fixture::new().await;
        fx.miss("/content/10.1101/2025.06.14.659707v1.full.pdf")
            .await;
        fx.miss("/content/10.1101/2025.06.14.659707.full.pdf").await;
        fx.serve_unpaywall_pdf("10.1101/2025.06.14.659707").await;
        let paper = save(
            &fx,
            "p-preprint-fallthrough",
            Some("10.1101/2025.06.14.659707"),
            None,
            None,
            None,
        );

        let result = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect("the Unpaywall leg serves it after the preprint leg misses");

        assert_eq!(
            result.route,
            RouteId::Unpaywall,
            "a preprint miss is a fall-through, not a terminal verdict"
        );
        assert_eq!(
            fx.artefact("p-preprint-fallthrough")
                .expect("an artefact row")
                .route,
            "unpaywall"
        );

        // Both candidates were tried, and in the order the module documents.
        let requests = fx.requests().await;
        let versioned = requests
            .iter()
            .position(|u| u.contains("2025.06.14.659707v1.full.pdf"))
            .expect("the versioned candidate was tried");
        let unversioned = requests
            .iter()
            .position(|u| u.contains("2025.06.14.659707.full.pdf"))
            .expect("the un-versioned candidate was tried");
        let unpaywall = requests
            .iter()
            .position(|u| u.contains("/v2/10.1101"))
            .expect("the index leg ran after the preprint leg");
        assert!(
            versioned < unversioned && unversioned < unpaywall,
            "expected v1, then the un-versioned URL, then the index: {requests:?}"
        );

        let states = fx.states("p-preprint-fallthrough");
        assert!(
            !states.iter().any(|s| s.status == "unavailable"),
            "a preprint that missed one route is not an unobtainable work: {states:?}"
        );
        assert!(
            states.is_empty(),
            "a work that was obtained records no gap at all: {states:?}"
        );
    }

    /// **"Unreachable" must never be reachable-by-download**, part one: a work
    /// whose only route was refused at the gate has not been looked for at
    /// all, and says so.
    ///
    /// `pending` is ADR-007 §2's own word for "never tried" and its action-list
    /// group is empty, so a gated work is picked up again by `acquire` instead
    /// of being handed to a human as an unobtainable one. The reason has to
    /// name the routes that were *not* tried and why — the other half of the
    /// acceptance criterion.
    #[tokio::test]
    async fn a_work_whose_doi_was_gated_is_pending_with_the_reason_recorded() {
        let fx = Fixture::new().await;
        // The exact shape from #262's audit: an Elsevier compact DOI truncated
        // at its parenthesis.
        let paper = save(
            &fx,
            "p-gated",
            Some("10.1016/0003-2670(93"),
            None,
            None,
            None,
        );

        let err = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect_err("a DOI with nothing else to try cannot succeed");
        assert!(
            matches!(err, AdapterError::NotFound(_)),
            "the walk's own error is unchanged by this slice: {err}"
        );
        assert_eq!(
            fx.requests().await,
            Vec::<String>::new(),
            "a rejected DOI is never fetched"
        );

        let states = fx.states("p-gated");
        assert_eq!(states.len(), 1, "one want, one row: {states:?}");
        let state = &states[0];
        assert_eq!(
            state.status, "pending",
            "nothing was tried, so nothing may be called unavailable"
        );
        assert_eq!(state.kind, "fulltext");
        assert_eq!(state.locator, "");
        assert_eq!(state.wanted_version, "vor");
        assert_eq!(
            state.drop_path, None,
            "there is no file for a human to drop: the bytes belong to a server"
        );
        let reason = state.reason.as_deref().expect("a reason is recorded");
        assert!(
            reason.contains("refused before it could be tried"),
            "the reason names the gate: {reason}"
        );
        assert!(
            reason.contains("unbalanced bracket"),
            "the gate's own reason is carried through, not summarised away: {reason}"
        );
        for leg in ["preprint transform", "unpaywall", "doi.org publisher page"] {
            assert!(
                reason.contains(leg),
                "the reason must name every route considered, including {leg}: {reason}"
            );
        }
        assert_eq!(
            state.hint_url.as_deref(),
            None,
            "a rejected DOI has no canonical page to point a human at"
        );
    }

    /// **"Unreachable" must never be reachable-by-download**, part two: the
    /// index-empty case, which is the documented failure of #260.
    ///
    /// The preprint leg 404s (a DOI that is not on bioRxiv after all), OpenAlex
    /// answers 200 with `best_oa_location: null`, Unpaywall answers 200 with
    /// no `url_for_pdf`, and doi.org 404s. Two of those four legs *named no
    /// location*, which is a fact about two indexes and about nothing else —
    /// so the verdict cannot be `unavailable`, however complete the list of
    /// legs looks. It is `error`: retryable, and honest about having asked.
    #[tokio::test]
    async fn an_index_that_named_no_location_is_never_reported_as_no_access() {
        let fx = Fixture::new().await;
        fx.miss("/content/10.1101/2024.11.19.624167v1.full.pdf")
            .await;
        fx.miss("/content/10.1101/2024.11.19.624167.full.pdf").await;
        fx.serve("/works/W123", openalex_json_without_a_location())
            .await;
        fx.serve("/v2/10.1101/2024.11.19.624167", "{}").await;
        fx.miss("/doi/10.1101/2024.11.19.624167").await;
        let paper = save(
            &fx,
            "p-no-location",
            Some("10.1101/2024.11.19.624167"),
            None,
            Some("W123"),
            None,
        );

        fx.downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect_err("nothing served the work");

        let states = fx.states("p-no-location");
        assert_eq!(states.len(), 1, "{states:?}");
        let state = &states[0];
        assert_ne!(
            state.status, "unavailable",
            "two indices that named nothing are not evidence about access"
        );
        assert_eq!(
            state.status, "error",
            "routes were tried and could not be resolved — retryable"
        );
        let reason = state.reason.as_deref().expect("a reason is recorded");
        for leg in [
            "preprint transform",
            "openalex",
            "unpaywall",
            "doi.org publisher page",
            "record url",
        ] {
            assert!(reason.contains(leg), "the reason names {leg}: {reason}");
        }
        assert!(
            reason.contains("not evidence that the work is unavailable"),
            "the index's silence must say what it is: {reason}"
        );
        assert!(
            reason.contains("not applicable"),
            "a leg with nothing to try is reported as such, not as a refusal: {reason}"
        );
        assert_eq!(
            state.hint_url.as_deref(),
            Some(format!("{}/doi/10.1101/2024.11.19.624167", fx.server.uri()).as_str()),
            "a human is pointed at the DOI, which is canonical for the work"
        );
    }

    /// `unavailable` is reachable, and only when every route that could be
    /// evaluated refused. A walk where each leg answers 403 or 404 is the one
    /// case in which "there is no such full text here" is evidence rather than
    /// exhaustion — so the status derivation is pinned as a pure function,
    /// where every branch can be read at once.
    #[test]
    fn only_a_walk_whose_every_evaluated_route_refused_is_unavailable() {
        let mut ladder = Ladder::default();
        ladder.record(Leg::Preprint, LegOutcome::Refused { status: 404 });
        ladder.record(Leg::Unpaywall, LegOutcome::Refused { status: 403 });
        ladder.record(Leg::Publisher, LegOutcome::Refused { status: 404 });
        ladder.skip(Leg::ArxivId, "the record carries no arxiv_id");
        ladder.skip(Leg::ManualUrl, "the record carries no url");
        assert_eq!(
            ladder.status(),
            "unavailable",
            "every route with something to try refused, and a leg with nothing to \
             try is not a route that could have served it"
        );

        // One unanswered leg is enough to withhold the verdict.
        for outcome in [
            LegOutcome::NoLocation {
                detail: "no OA location".into(),
            },
            LegOutcome::Inconclusive {
                detail: "connection failed".into(),
            },
        ] {
            let mut ladder = Ladder::default();
            ladder.record(Leg::Preprint, LegOutcome::Refused { status: 404 });
            ladder.record(Leg::Unpaywall, outcome.clone());
            assert_ne!(
                ladder.status(),
                "unavailable",
                "{outcome:?} is not a refusal, so no-access is not established"
            );
        }

        // A gated route exists and is unevaluated, which is also enough to
        // withhold it: a work we declined to look up has not been found absent.
        let mut gated = Ladder::default();
        gated.record(Leg::Preprint, LegOutcome::Refused { status: 404 });
        gated.gate(
            Leg::Publisher,
            "the DOI was rejected before any fetch".to_string(),
        );
        assert_eq!(
            gated.status(),
            "pending",
            "an unevaluated route blocks the no-access verdict, and `pending` is \
             the ADR's word for a route that was never tried"
        );

        // And the empty walk, which is the "no route tried" case in its purest
        // form: nothing was asked of anybody.
        let mut nothing = Ladder::default();
        nothing.skip(Leg::ArxivId, "the record carries no arxiv_id");
        nothing.skip(Leg::Preprint, NO_PREPRINT_ROUTE);
        nothing.skip(Leg::OpenAlex, "the record carries no openalex_id");
        nothing.skip(Leg::Unpaywall, "the record carries no doi");
        nothing.skip(Leg::ManualUrl, "the record carries no url");
        assert_eq!(nothing.status(), "pending");
        let reason = nothing.reason();
        assert!(
            reason.contains("not applicable") && !reason.contains("refused with HTTP"),
            "five routes, none of them applicable, and none of them asked: {reason}"
        );
        // And the degenerate case, where no leg was even considered.
        assert!(
            Ladder::default().reason().contains("no route applied"),
            "an empty walk says so rather than naming nothing"
        );
    }

    /// The arXiv half of the same criterion, and the half where the transform
    /// is subtler: the DOI's suffix is `arXiv.2301.00001` and the server wants
    /// `/pdf/2301.00001`, so a leg that forgot to strip the prefix would send
    /// the 406 URL and fall through. The work carries **no** `arxiv_id`, so the
    /// only route that can serve it here is the DOI transform.
    #[tokio::test]
    async fn an_arxiv_doi_is_fetched_from_the_stripped_url_with_no_index_lookup() {
        let fx = Fixture::new().await;
        // The verified URL: no `.pdf` suffix, no `arxiv.` prefix.
        fx.serve_pdf("/pdf/2301.00001").await;
        let paper = save(
            &fx,
            "p-arxiv-doi",
            Some("10.48550/arXiv.2301.00001"),
            None,
            Some("W123"),
            None,
        );

        let result = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect("the arXiv transform should serve the PDF");

        assert_eq!(result.route, RouteId::Arxiv);
        assert_eq!(result.format, DownloadFormat::Pdf);
        assert_eq!(
            fx.requests().await.len(),
            1,
            "one request, to the verified URL, and nothing else: {:?}",
            fx.requests().await
        );
        // Asserted on the path rather than the whole URL: wiremock records
        // what arrived, so the authority is `localhost` with the default port
        // dropped, while `server.uri()` is the port the client dialled.
        assert_eq!(
            fx.requests_to("/pdf/2301.00001").await.len(),
            1,
            "and it is the stripped URL, not the 406 one: {:?}",
            fx.requests().await
        );
        let row = fx.artefact("p-arxiv-doi").expect("an artefact row");
        assert_eq!(row.route, "arxiv");
        assert_eq!(row.version, "preprint");
        assert_eq!(row.access_basis, "oa_license");
        assert_eq!(row.publisher.as_deref(), Some("arxiv"));
    }

    /// The third branch of the acceptance criterion: a preprint DOI for which
    /// **no verified transform exists** has to *say so*. ChemRxiv is that
    /// branch, and the sentence has to reach the row a human reads — otherwise
    /// the honest answer ("I have no rule for this server") is lost and the
    /// work looks like a publisher problem.
    ///
    /// So: no candidate, the documented reason recorded verbatim, the walk
    /// falling through to the index legs, and a status that is not a verdict
    /// about the work.
    #[tokio::test]
    async fn a_chemrxiv_doi_records_why_it_has_no_preprint_route() {
        let fx = Fixture::new().await;
        fx.serve("/works/W123", openalex_json_without_a_location())
            .await;
        fx.serve("/v2/10.26434/chemrxiv.2024.01.01.123456.v1", "{}")
            .await;
        fx.miss("/doi/10.26434/chemrxiv.2024.01.01.123456.v1").await;
        let doi = "10.26434/chemrxiv.2024.01.01.123456.v1";
        let paper = save(&fx, "p-chemrxiv", Some(doi), None, Some("W123"), None);

        fx.downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect_err("nothing served the work");

        // No URL was proposed for ChemRxiv, so nothing was requested from one.
        // No URL was proposed for ChemRxiv, so nothing was requested from the
        // path shape a preprint transform uses. (The DOI itself appears in the
        // index legs' URLs, so the assertion is about the path, not the DOI.)
        assert!(
            fx.requests_to("/content/").await.is_empty(),
            "an unverified transform must not put a guessed URL on the wire: {:?}",
            fx.requests().await
        );
        let states = fx.states("p-chemrxiv");
        assert_eq!(states.len(), 1, "{states:?}");
        let state = &states[0];
        assert_ne!(
            state.status, "unavailable",
            "a missing rule is a gap in our coverage, not a verdict on the work"
        );
        let reason = state.reason.as_deref().expect("a reason is recorded");
        assert!(
            reason.contains("engage"),
            "the recorded reason is the documented obstacle: {reason}"
        );
        assert!(
            reason.contains("ChemRxiv"),
            "and it names the server: {reason}"
        );
        // The index legs still ran, which is the "or says which it tried" half.
        assert!(!fx.requests_to("/works/W123").await.is_empty());
        assert!(!fx.requests_to("/v2/10.26434").await.is_empty());
    }

    /// The DOI-only entry point (`scitadel download <doi>`) gets the same
    /// transform in the same position. It is a user-facing way to ask for
    /// precisely the DOIs in #260, and it has no `Paper` — so the assertions
    /// are about the request line and the returned route, and the file is the
    /// legacy copy `finish(None, …)` has always written.
    #[tokio::test]
    async fn the_doi_only_path_also_tries_the_preprint_transform_first() {
        let fx = Fixture::new().await;
        fx.serve_pdf("/content/10.1101/2025.06.14.659707v1.full.pdf")
            .await;
        let out_dir = fx.papers_dir();

        let result = fx
            .downloader()
            .download("https://doi.org/10.1101/2025.06.14.659707", &out_dir)
            .await
            .expect("the preprint transform should serve the PDF");

        assert_eq!(result.route, RouteId::Biorxiv);
        assert_eq!(result.format, DownloadFormat::Pdf);
        assert!(
            fx.requests_to("/v2/").await.is_empty(),
            "no index lookup for a DOI that determines its own location: {:?}",
            fx.requests().await
        );
        assert_eq!(fx.requests().await.len(), 1, "{:?}", fx.requests().await);
        assert!(
            result.path.exists(),
            "the DOI-only path still writes its legacy copy: {}",
            result.path.display()
        );
        assert_eq!(
            std::fs::read(&result.path).expect("read back"),
            PDF_BYTES,
            "and it is the bytes the server served"
        );
    }

    /// The other end of the "no route tried" case: a work with **no**
    /// identifiers at all. Every leg is inapplicable, nothing was put on the
    /// wire, and the row has to say so rather than reporting an absence that
    /// was never looked for — while leaving the work `pending`, so `acquire`
    /// picks it up again rather than a human being asked for a document that
    /// nobody has looked for yet.
    ///
    /// The reason enumerating *every* leg is the other half of the acceptance
    /// criterion, and it is asserted leg by leg: a leg that is silently absent
    /// from the record is indistinguishable from one that was tried and
    /// refused, which is the ambiguity this whole section exists to remove.
    #[tokio::test]
    async fn a_work_with_no_identifiers_is_pending_and_names_every_leg() {
        let fx = Fixture::new().await;
        let paper = save(&fx, "p-nothing", None, None, None, None);

        let err = fx
            .downloader()
            .download_paper(&paper, &fx.papers_dir())
            .await
            .expect_err("a work with no identifier cannot be fetched");
        assert!(
            matches!(err, AdapterError::NotFound(_)),
            "the walk's own error is unchanged: {err}"
        );
        assert_eq!(
            fx.requests().await,
            Vec::<String>::new(),
            "nothing to try means nothing is sent"
        );
        assert!(
            fx.pacer.grants().is_empty(),
            "and no permit is spent: {:?}",
            fx.pacer.grants()
        );

        let states = fx.states("p-nothing");
        assert_eq!(states.len(), 1, "{states:?}");
        let state = &states[0];
        assert_eq!(state.status, "pending");
        let reason = state.reason.as_deref().expect("a reason is recorded");
        for leg in [
            "arxiv id",
            "preprint transform",
            "openalex",
            "unpaywall",
            "doi.org publisher page",
            "record url",
        ] {
            assert!(reason.contains(leg), "the reason names {leg}: {reason}");
        }
        assert!(
            !reason.contains("refused with HTTP"),
            "nothing was asked of anybody: {reason}"
        );
        assert_eq!(
            state.hint_url, None,
            "no DOI, so no canonical page to point a human at"
        );
    }

    /// A 408 or a 429 is the publisher asking for patience, not refusing, and
    /// a 5xx is a failure on their side. Neither may be laundered into a
    /// no-access verdict by the shared message they produce.
    #[test]
    fn a_timeout_a_rate_limit_and_a_server_error_are_not_refusals() {
        assert!(is_refusal(403), "403 is an access decision");
        assert!(is_refusal(404), "404 is a statement that it is not there");
        assert!(is_refusal(410));
        for code in [408, 429, 500, 502, 503, 504] {
            assert!(!is_refusal(code), "HTTP {code} is not a refusal");
        }
        assert!(!is_refusal(200));
    }
}
