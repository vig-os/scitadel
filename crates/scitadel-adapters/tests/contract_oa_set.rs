#![cfg(feature = "contract-tests")]
//! Live measurement of #260's acceptance criterion, over the real resolve pass.
//!
//! Gated at the crate root rather than per-`#[test]`, so the default
//! `cargo test --workspace` cannot reach the network even by accident: with the
//! feature off this file compiles to an empty crate.
//!
//! ## What "obtained" means here
//!
//! Bytes. The resolve pass *names* candidates; it never fetches them, and a
//! plan line naming a URL at rank 1 is a claim about the metadata, not about
//! the publisher. So this harness does what the acceptance clause asks and
//! fetches: for each DOI it runs [`MetadataPass::resolve`] over the live
//! six-source pass, takes [`Resolution::chosen`], and issues a real ranged GET
//! for that URL through the same [`PacedClient`] and the same bucket policies
//! the production downloader uses. A ranked candidate behind a 403 or a 404 is
//! reported as **not obtained**, with the status.
//!
//! ## The three outcomes this file must keep apart
//!
//! #261's overclaim rule, and the reason this is a measurement rather than a
//! test that asserts a percentage:
//!
//! - **obtained** — a 2xx and at least one byte back;
//! - **not obtained** — we asked and it was not there. A status, always;
//! - **network-error** — we could not ask. A transport failure, a DNS error, a
//!   TLS failure, a timeout, or a pacer refusal.
//!
//! Collapsing the third into the second is how "unreachable" came to describe 52
//! free papers in #260 in the first place.
//!
//! ## Read-only
//!
//! No database and no blob is written. The pacer is backed by an in-memory
//! SQLite ledger purely to hold the permits it grants; nothing under test reads
//! it back, and the campaign never touches a `papers` or `artefacts` row. The
//! candidate bodies are read into memory and discarded.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use scitadel_adapters::oa_live::{Criterion, OA_260_PROBES, OaProbe};
use scitadel_adapters::resolve::{MetadataPass, RankedCandidate, Resolution, WorkRefs};
use scitadel_core::config::OpenAlexAuth;
use scitadel_core::ports::{Bucket, Cost, PaceDenied, PaceTier, Pacer, Permit};
use scitadel_http::{BucketPolicyTable, FetchError, PacedClient, SafeHeaders, WorkScope};

/// One DOI's measured row.
#[derive(Debug)]
struct Row {
    doi: &'static str,
    criterion: Criterion,
    /// The six sources the pass asked, and what each said.
    consulted: Vec<String>,
    hosts: BTreeSet<String>,
    /// The candidate the ranker chose, if it chose one.
    chosen: Option<ChosenPick>,
    /// What happened when we actually fetched it.
    outcome: Outcome,
    /// Every candidate the pass found, ranked or not.
    plan: Vec<String>,
}

#[derive(Debug)]
struct ChosenPick {
    position: usize,
    route: String,
    url: String,
    version: String,
    /// The candidate's own one-line justification, which carries the version
    /// source, the licence strength and who named the location — the three
    /// facts `Resolution::plan_lines` prints, and the ones a reader of the
    /// report needs to judge whether the pick was justified.
    why: String,
}

/// The three outcomes, kept apart on purpose — see the module docs.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// A 2xx and a byte count.
    Obtained {
        status: u16,
        bytes: usize,
        /// The served `Content-Type`, when the response carried one.
        ///
        /// **Recorded because "obtained" is not "obtained the article."** A
        /// ranged GET against a PubMed abstract page answers `203` with several
        /// thousand bytes of HTML, and a byte count cannot tell that from a PDF.
        /// #260's clause is "obtains the PDF", so the report needs to be able to
        /// separate the two, and a harness that cannot is one whose number can
        /// rise for the wrong reason without anybody noticing.
        content_type: Option<String>,
        /// What the bytes we actually received **are**, from
        /// [`scitadel_adapters::magic::sniff`].
        ///
        /// Sniffed rather than read off the `Content-Type`, and the reason is
        /// `binary/octet-stream`: S3 serves the PMC-OA dataset's PDFs with no
        /// content type at all, so a declared-type rule reports the only genuine
        /// PDFs in this table as *not* articles. The bytes were in hand and were
        /// being discarded.
        sniffed: scitadel_adapters::magic::Magic,
    },
    /// We asked; the status says no.
    NotObtained { status: u16, note: String },
    /// We could not ask. Never reported as unreachable.
    NetworkError { note: String },
    /// The resolve pass named nothing fetchable at all.
    NoCandidate { note: String },
}

impl Outcome {
    fn obtained(&self) -> bool {
        matches!(self, Self::Obtained { .. })
    }

    /// Whether the bytes are the **article** rather than a page about it.
    ///
    /// A `Markup` or `Json` body is a landing page, an abstract page or a dataset
    /// manifest, and #260's "obtains the PDF" is not satisfied by any of them
    /// however many bytes arrived.
    fn obtained_article(&self) -> bool {
        match self {
            Self::Obtained { sniffed, .. } => {
                matches!(sniffed, scitadel_adapters::magic::Magic::Pdf)
            }
            _ => false,
        }
    }

    fn render(&self) -> String {
        match self {
            Self::Obtained {
                status,
                bytes,
                content_type,
                sniffed,
            } => format!(
                "OBTAINED http {status}, {bytes} bytes, {}{} — the bytes are {}",
                content_type.as_deref().unwrap_or("(no content-type)"),
                if self.obtained_article() {
                    " [ARTICLE]"
                } else {
                    ""
                },
                sniffed.describe(),
            ),
            Self::NotObtained { status, note } => format!("NOT-OBTAINED http {status} ({note})"),
            Self::NetworkError { note } => format!("NETWORK-ERROR ({note})"),
            Self::NoCandidate { note } => format!("NO-CANDIDATE ({note})"),
        }
    }
}

/// A pacer that spends the **real** bucket policies with no ledger behind it.
///
/// The point of reusing [`PacedClient`] is that the harness spends the same
/// budgets production spends; a stub pacer that grants instantly would let the
/// measurement hammer a publisher at a rate production would never use, which
/// is both rude and a different measurement. This one reads
/// [`BucketPolicyTable`] — so the intervals and caps are the shipped ones — and
/// holds the rolling-window accounting in memory.
///
/// In-memory because the ledger's purpose is to be *shared across processes*
/// (ADR-007 §4), and a single-process measurement has no second process to
/// share with. The SQLite ledger would be a second copy of the same arithmetic
/// with a database attached and nothing to gain.
struct PolicyPacer {
    policies: Arc<BucketPolicyTable>,
    /// `(bucket, tier, unit) -> (spent, next allowed)`.
    state: std::sync::Mutex<std::collections::HashMap<(String, u8, u8), (i64, std::time::Instant)>>,
}

impl PolicyPacer {
    fn new(policies: Arc<BucketPolicyTable>) -> Self {
        Self {
            policies,
            state: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

#[async_trait]
impl Pacer for PolicyPacer {
    /// Mirrors [`scitadel_db::sqlite::pacer`]'s arithmetic against the shipped
    /// policies, without the database. `SQLITE_BUSY`'s "fail closed" has no
    /// analogue here because there is no ledger to be busy, so the only
    /// refusals are the cap and the wait ceiling — both of which the real
    /// ledger would also produce for this workload.
    async fn acquire(
        &self,
        bucket: &Bucket,
        tier: PaceTier,
        cost: Cost,
    ) -> Result<Permit, PaceDenied> {
        let policy = self.policies.policy_for(bucket);
        let key = (bucket.0.clone(), tier as u8, cost as u8);
        let (cap, interval_ms) = match cost {
            Cost::Request => (policy.request_cap, policy.min_interval_ms),
            // The `Work` grant is a per-bucket reservation rather than a hop, so
            // it is paced by the cap and not by the request interval.
            Cost::Work => (policy.work_cap, 0),
        };
        let interval = Duration::from_millis(interval_ms.max(0) as u64);
        let mut state = self.state.lock().expect("the pacing state is not poisoned");
        let slot = state.entry(key).or_insert((0, std::time::Instant::now()));
        if slot.0 >= cap {
            return Err(PaceDenied::DailyCap);
        }
        let now = std::time::Instant::now();
        let not_before = slot.1.max(now);
        // The 10 s ceiling, from ADR-007 §4. A harness that waited out a long
        // interval would look like a hung suite; a harness that ignored the cap
        // would exceed the publisher's allowance. Refusing is the honest third
        // option, and it is recorded as a network-error rather than as
        // "unreachable" precisely because nothing was asked.
        if not_before.duration_since(now) > Duration::from_secs(10) {
            return Err(PaceDenied::WaitTooLong { until_ms: 0 });
        }
        slot.0 += 1;
        slot.1 = not_before + interval;
        Ok(Permit {
            bucket: bucket.clone(),
            tier,
            not_before,
        })
    }
}

fn mailto() -> String {
    std::env::var("SCITADEL_CONTRACT_MAILTO")
        .unwrap_or_else(|_| "scitadel-contract-tests@example.org".to_string())
}

/// One client and one pass for the whole run, so the permits are shared across
/// DOIs the way a campaign's would be.
fn live() -> (PacedClient, MetadataPass) {
    let table = Arc::new(BucketPolicyTable::new());
    let pacer: Arc<dyn Pacer> = Arc::new(PolicyPacer::new(Arc::clone(&table)));
    let client = PacedClient::with_timeout(Duration::from_secs(45), pacer, (*table).clone())
        .expect("the transport builds");
    let auth = OpenAlexAuth {
        email: mailto(),
        api_key: std::env::var("SCITADEL_OPENALEX_API_KEY").unwrap_or_default(),
    };
    let pass = MetadataPass::new(auth, &mailto());
    // The default bases are the live ones, which is the point; assert it rather
    // than trust it, because a test that resolved against wiremock would report
    // a perfectly green measurement of nothing.
    assert_eq!(
        pass.bases().openalex_works,
        scitadel_adapters::openalex::OPENALEX_API_URL,
        "the live pass must resolve against the live OpenAlex base"
    );
    (client, pass)
}

/// Fetch the chosen candidate for real, and classify what came back.
///
/// A ranged GET rather than a full body: the question is whether bytes are
/// obtainable, and `Range: bytes=0-1023` answers it for 1 KB instead of a whole
/// PDF. The servers that ignore `Range` send the whole body, which is recorded
/// by the length rather than truncated here.
async fn fetch_chosen(
    client: &PacedClient,
    scope: &WorkScope,
    chosen: &RankedCandidate,
) -> Outcome {
    let Ok(url) = reqwest::Url::parse(chosen.candidate.url.trim()) else {
        return Outcome::NotObtained {
            status: 0,
            note: format!("{} is not a parseable URL", chosen.candidate.url),
        };
    };
    let mut headers = SafeHeaders::unauthenticated();
    // `Range` is not a credential, so `SafeHeaders` accepts it; the refusal it
    // does enforce is on `Authorization`/`Cookie`, which is the property worth
    // having on the wire here.
    let _ = headers.insert(
        reqwest::header::RANGE,
        reqwest::header::HeaderValue::from_static("bytes=0-1023"),
    );
    match client.get_in_work(scope, url, PaceTier::Oa, headers).await {
        Ok(response) => {
            let status = response.status.as_u16();
            // Read before the body, because `bytes()` consumes the response.
            let content_type = response.content_type().unwrap_or("(none)").to_string();
            let body = response.bytes().await.unwrap_or_default();
            let bytes = body.len();
            if bytes == 0 {
                Outcome::NotObtained {
                    status,
                    note: "a 2xx with an empty body is not an article".to_string(),
                }
            } else {
                Outcome::Obtained {
                    status,
                    bytes,
                    content_type: Some(content_type),
                    // Sniff the bytes we were going to throw away. A ranged GET
                    // gives 1 KB, which is more than enough for every signature
                    // in `magic` — `%PDF-` is five.
                    sniffed: scitadel_adapters::magic::sniff(&body),
                }
            }
        }
        Err(FetchError::Status { code, .. }) => Outcome::NotObtained {
            status: code,
            note: "the ranked location is not served".to_string(),
        },
        Err(error @ FetchError::Transport { .. }) => Outcome::NetworkError {
            note: format!("transport: {error}"),
        },
        Err(error @ FetchError::Pace(_)) => Outcome::NetworkError {
            note: format!("the pacer refused, so nothing was asked: {error}"),
        },
        Err(error @ (FetchError::LoginRedirect { .. } | FetchError::TooManyRedirects { .. })) => {
            Outcome::NetworkError {
                note: format!("the redirect chain did not terminate: {error}"),
            }
        }
        Err(FetchError::RateLimited { code, .. }) => Outcome::NotObtained {
            status: code,
            note: "the publisher asked us to stop".to_string(),
        },
        Err(other) => Outcome::NetworkError {
            note: format!("{other}"),
        },
    }
}

async fn measure(pass: &MetadataPass, client: &PacedClient, probe: &'static OaProbe) -> Row {
    let resolution: Resolution = pass
        .resolve(
            client,
            &WorkScope::new(),
            WorkRefs {
                doi: Some(probe.doi),
                arxiv_id: None,
                osti_id: None,
                url: None,
                openalex_id: None,
                pmcid: None,
            },
        )
        .await;

    let consulted = resolution
        .consulted
        .iter()
        .map(|hop| format!("{}={:?}", hop.registry.label(), hop.outcome))
        .collect();
    let plan = resolution.plan_lines();

    // One scope for the fetch too, so the resolve pass and the fetch are one
    // work to every platform they touch (ADR-007 §4) — the same pairing
    // `download_paper_ranked` makes.
    let scope = WorkScope::new();
    let outcome = match resolution.chosen() {
        Some(chosen) => {
            // Re-resolving under the fetch scope rather than reusing the first
            // call would double the metadata spend; the second scope only ever
            // holds the fetch's own permits.
            fetch_chosen(client, &scope, chosen).await
        }
        None => Outcome::NoCandidate {
            note: format!(
                "the ranker placed nothing: {} candidates found, {} of them ranked",
                resolution.candidates().count(),
                resolution.ranked.len()
            ),
        },
    };

    let chosen = resolution.chosen().map(|chosen| ChosenPick {
        position: chosen.position,
        route: chosen.candidate.route.to_string(),
        url: chosen.candidate.url.clone(),
        version: chosen
            .candidate
            .version
            .map_or_else(|| "unrankable".to_string(), |version| version.to_string()),
        why: chosen.candidate.why(),
    });

    Row {
        doi: probe.doi,
        criterion: probe.criterion,
        consulted,
        hosts: resolution.hosts_contacted,
        chosen,
        outcome,
        plan,
    }
}

/// The report's table, plus the plan and host lines underneath it.
///
/// Wide on purpose: a measurement whose ranked pick and its fetch result are on
/// separate lines invites reading the first and not the second, which is how a
/// "ranked #1" came to be reported as obtained.
fn render_table(rows: &[Row]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str(
        "\nDOI | criterion | consulted | ranked pick (route/version/licence) | fetch result | obtained\n",
    );
    for row in rows {
        let pick = row.chosen.as_ref().map_or_else(
            || "-".to_string(),
            |pick| {
                format!(
                    "{} #{} version {} | {}",
                    pick.route, pick.position, pick.version, pick.why
                )
            },
        );
        writeln!(
            out,
            "{} | {:?} | {} | {} | {} | {}",
            row.doi,
            row.criterion,
            row.consulted.join(" "),
            pick,
            row.outcome.render(),
            if row.outcome.obtained() { "YES" } else { "no" }
        )
        .expect("writing to a String cannot fail");
    }
    for row in rows {
        writeln!(out, "\n--- {} ---", row.doi).expect("a String write cannot fail");
        if let Some(pick) = &row.chosen {
            writeln!(out, "fetched: {}", pick.url).expect("a String write cannot fail");
        }
        writeln!(out, "hosts: {:?}", row.hosts).expect("a String write cannot fail");
        for line in &row.plan {
            writeln!(out, "plan:{line}").expect("a String write cannot fail");
        }
    }
    out
}

/// The measurement itself. Asserts nothing about the *result*, because a
/// percentage over seven curated DOIs is the report's claim to make and not a
/// regression this suite should enforce — a paywall appearing at OUP is not a
/// code defect. What it does assert is the harness's own honesty: every probe
/// produced a row, and every row's outcome is one of the three.
#[tokio::test]
async fn every_probe_produces_a_classified_row() {
    let (client, pass) = live();
    let mut rows = Vec::new();
    for probe in OA_260_PROBES {
        rows.push(measure(&pass, &client, probe).await);
    }
    assert_eq!(rows.len(), OA_260_PROBES.len());
    println!("{}", render_table(&rows));

    for row in &rows {
        assert!(
            matches!(
                row.outcome,
                Outcome::Obtained { .. }
                    | Outcome::NotObtained { .. }
                    | Outcome::NetworkError { .. }
                    | Outcome::NoCandidate { .. }
            ),
            "{} produced an unclassified outcome",
            row.doi
        );
        // A `NotObtained` with no status would be the overclaim #261 is about:
        // "we could not obtain it" without saying what we were told.
        if let Outcome::NotObtained { status, .. } = &row.outcome {
            assert_ne!(
                *status, 0,
                "{} is reported as not obtained with no HTTP status, which \
                 reads as unreachable rather than as a question we did not get \
                 to ask",
                row.doi
            );
        }
    }
}

/// The floors the two ranker changes were measured against, and why they are
/// floors.
///
/// **Recorded prior measurements, not expectations.** Recorded 2026-10-07 on this
/// file's own harness, at `807d0ff` — before the Crossref
/// `link[intended-application]` filter and before ADR-007 §3's fetch order became
/// the tiebreak:
///
/// | | bytes obtained | of which the bytes are a PDF |
/// |---|---|---|
/// | before | 4 / 16 | 1 / 16 |
/// | after | 15 / 16 | 6 / 16 |
///
/// Two numbers, not one, because on this table the two ranker changes moved both
/// and only one of them is what #260 is about. The eleven new byte-wins include
/// nine that are **not** the article: PubMed and NCBI landing pages, and two
/// `doi.org` resolutions that end at a DataCite JSON manifest. They are real
/// 2xx responses and the pre-change harness scored them, which is precisely why
/// a byte count alone is not this file's claim. `obtained_article` and the second
/// floor exist so the number cannot be banked twice.
///
/// The assertions are **strictly greater than**, which is the only stateless way
/// to say "went up". Deliberately not `assert_eq!`: publisher bot walls move, OUP
/// changes its CDN, and a contract test that turns red because a publisher
/// changed something is a test nobody trusts — which is how this file gets
/// deleted.
///
/// The corollary is that a large *fall* is tolerated, because these floors are low
/// and MDPI alone owns three rows. What they catch is the regression that is a
/// code defect: the ranker picking a candidate that cannot be served at all, or
/// picking a page when the article was sitting at rank 2. That is the regression
/// worth failing on, and it is the one these changes removed.
const OBTAINED_BYTES_FLOOR: usize = 4;
const OBTAINED_PDF_FLOOR: usize = 1;

#[tokio::test]
async fn the_obtained_count_went_up_from_what_the_ranker_used_to_choose() {
    let (client, pass) = live();
    let mut rows = Vec::new();
    for probe in OA_260_PROBES {
        rows.push(measure(&pass, &client, probe).await);
    }

    let obtained = rows.iter().filter(|row| row.outcome.obtained()).count();
    let articles = rows
        .iter()
        .filter(|row| row.outcome.obtained_article())
        .count();
    println!("{}", render_table(&rows));
    println!(
        "\nobtained bytes {obtained}/{} (floor {OBTAINED_BYTES_FLOOR}); \
         obtained **as a PDF** {articles}/{} (floor {OBTAINED_PDF_FLOOR})",
        OA_260_PROBES.len(),
        OA_260_PROBES.len(),
    );
    for row in &rows {
        if row.outcome.obtained() && !row.outcome.obtained_article() {
            println!(
                "  ! {} — {} bytes, but not the article: {}",
                row.doi,
                row.outcome.render(),
                row.chosen.as_ref().map_or("-", |pick| pick.url.as_str())
            );
        }
    }

    assert!(
        obtained > OBTAINED_BYTES_FLOOR,
        "only {obtained} of {} probes obtained bytes, which is not more than the \
         {OBTAINED_BYTES_FLOOR} this harness measured before ADR-007 §3's fetch \
         order became the tiebreak and Crossref stopped offering its own \
         plagiarism-detection endpoint as a candidate. Either the ranker is \
         choosing a location that cannot be served again, or something upstream \
         stopped answering — both are defects, and neither is a publisher having \
         a bad day.\n\n{}",
        OA_260_PROBES.len(),
        render_table(&rows),
    );

    assert!(
        articles > OBTAINED_PDF_FLOOR,
        "only {articles} of {} probes served a PDF, which is not more than the \
         {OBTAINED_PDF_FLOOR} measured before the two ranker changes. This is \
         the assertion that matters: #260's clause is 'obtains the PDF', and the \
         byte count can be satisfied by a landing page.\n\n{}",
        OA_260_PROBES.len(),
        render_table(&rows),
    );
}

/// The two ranker changes, asserted on the **plan** rather than on a fetch.
///
/// The measurement above is about bytes, and bytes are a moving target. This is
/// about the thing the changes actually are, and it is stable:
///
/// - a `link[]` Crossref offers as `similarity-checking` is **not** a candidate,
///   so no plan line may name `syndication.highwire.org` or `harvest.aps.org`;
/// - on an exact version-and-licence tie, the candidate on the earlier ADR-007
///   §3 step is the one `Resolution::chosen` returns.
///
/// Both are asserted over the live pass, so they fail the moment the metadata
/// changes shape rather than the moment a publisher does.
#[tokio::test]
async fn no_similarity_checking_endpoint_is_ever_a_candidate() {
    let (client, pass) = live();
    let mut offenders = Vec::new();
    for probe in OA_260_PROBES {
        let resolution = pass
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some(probe.doi),
                    arxiv_id: None,
                    osti_id: None,
                    url: None,
                    openalex_id: None,
                    pmcid: None,
                },
            )
            .await;
        for entry in resolution.ranked {
            if entry.candidate.route != scitadel_core::models::RouteId::Crossref {
                continue;
            }
            // A `RouteId::Crossref` candidate can only exist now if it came from a
            // `text-mining` link, so its URL is the check.
            offenders.push(format!(
                "{} #{} {}",
                probe.doi, entry.position, entry.candidate.url
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "a Crossref-named candidate survived for: {}. ADR-007 §3's resolve bullet \
         admits `link[intended-application=text-mining]` only; \
         `syndication.highwire.org` and `harvest.aps.org` are Crossref's own \
         plagiarism-detection endpoints and 403 an anonymous client.",
        offenders.join("; ")
    );
}

/// **Diagnostic, not the measurement.** For every probe whose rank-1 fetch
/// failed, classify *why* — a 403 on a publisher's own host is not a paywall
/// verdict.
///
/// #261's rule is that "unreachable" must mean "no access". A 403 does not
/// mean that: MDPI, PNAS and RSC all serve a freely-readable article to a
/// browser and 403 an anonymous ranged GET from a named HTTP client. Classifying
/// the body through the downloader's own [`detect_access_status`] separates
/// "this is the paywall template" from "this is a bot wall" from "this is the
/// article" — and the three need three different fixes, none of which is a
/// metadata change.
#[tokio::test]
async fn every_not_obtained_pick_says_whether_the_body_is_a_paywall() {
    let (client, pass) = live();
    let mut rows = Vec::new();
    for probe in OA_260_PROBES {
        rows.push(measure(&pass, &client, probe).await);
    }

    println!("\n=== paywall-body diagnostic ===");
    for row in &rows {
        if row.outcome.obtained() {
            continue;
        }
        let Some(pick) = &row.chosen else {
            println!("{} — no candidate at all", row.doi);
            continue;
        };
        // The landing page, not the candidate URL: a PDF that 403s has no body
        // to classify, and the landing page is what a human would be shown.
        let landing =
            reqwest::Url::parse(&format!("https://doi.org/{}", row.doi)).expect("a DOI URL parses");
        let verdict = match client
            .get_in_work(
                &WorkScope::new(),
                landing,
                PaceTier::Oa,
                SafeHeaders::unauthenticated(),
            )
            .await
        {
            Ok(response) => {
                let status = response.status.as_u16();
                let content_type = response.content_type().unwrap_or("(none)").to_string();
                let body = response.text().await.unwrap_or_default();
                let access = scitadel_adapters::download::detect_access_status(&body);
                format!(
                    "landing {status} {content_type} {} bytes -> {access}",
                    body.len()
                )
            }
            Err(error) => format!("landing fetch failed: {error}"),
        };
        println!(
            "{}\n  rank 1 was {} ({})\n  {verdict}",
            row.doi,
            pick.url,
            row.outcome.render()
        );
    }
}

/// **Diagnostic, not the measurement.** For every probe whose rank-1 fetch
/// failed, walk the remaining ranked candidates in order and report which one
/// *would* have served bytes.
///
/// This is not what `acquire` does — [`Resolution::chosen`] returns one
/// candidate and a failure is data about that route, not a cue to try the next
/// — so nothing here may be read as a pass rate. It exists to answer the
/// question the table above raises: when rank 1 is a 403, is the *work*
/// unobtainable or is the *route* wrong? Those need opposite fixes.
///
/// The walk deliberately stops at the first candidate that serves bytes. It
/// costs one request per hop against a real publisher, and "some later
/// candidate would have worked" is all the answer needs.
#[tokio::test]
async fn every_failed_pick_reports_which_ranked_candidate_would_serve() {
    let (client, pass) = live();
    let mut rows = Vec::new();
    for probe in OA_260_PROBES {
        rows.push(measure(&pass, &client, probe).await);
    }

    println!("\n=== fallback diagnostic ===");
    for (probe, row) in OA_260_PROBES.iter().zip(&rows) {
        if row.outcome.obtained() {
            continue;
        }
        let resolution = pass
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some(probe.doi),
                    arxiv_id: None,
                    osti_id: None,
                    url: None,
                    openalex_id: None,
                    pmcid: None,
                },
            )
            .await;
        let first_url = row.chosen.as_ref().map(|pick| pick.url.clone());
        println!(
            "\n{} — rank 1 was {}",
            probe.doi,
            first_url.as_deref().unwrap_or("-")
        );
        for entry in &resolution.ranked {
            if Some(&entry.candidate.url) == first_url.as_ref() {
                continue;
            }
            let outcome = fetch_chosen(&client, &WorkScope::new(), entry).await;
            println!(
                "  #{} {} -> {}",
                entry.position,
                entry.candidate.url,
                outcome.render()
            );
            if outcome.obtained() {
                break;
            }
        }
    }
}

/// The preprint clause: a `10.1101` DOI must resolve to a candidate **without**
/// a registry naming one, because the transform is a function of the DOI.
///
/// The assertion is about the *source* of the candidate, not about whether the
/// fetch succeeded — access at bioRxiv is not this clause's question, and a
/// transient 503 from Cold Spring Harbor should not turn this file red. What it
/// does assert is that the route exists and is reachable, which is the part
/// #260 says was missing.
#[tokio::test]
async fn every_preprint_doi_names_a_preprint_route() {
    let (client, pass) = live();
    let probes: Vec<&'static OaProbe> = scitadel_adapters::oa_live::preprint_probes().collect();
    assert_eq!(probes.len(), 3, "the three bioRxiv DOIs #260 names");

    for probe in probes {
        let resolution = pass
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some(probe.doi),
                    arxiv_id: None,
                    osti_id: None,
                    url: None,
                    openalex_id: None,
                    pmcid: None,
                },
            )
            .await;
        let transform = resolution
            .candidates()
            .find(|candidate| candidate.route == scitadel_core::models::RouteId::Biorxiv);
        let transform = transform.unwrap_or_else(|| {
            panic!(
                "{} produced no bioRxiv candidate; the DOI transform is the \
                 whole of #260's preprint route and its absence is the bug. \
                 Plan was:\n{}",
                probe.doi,
                render_table(&[])
            )
        });
        assert!(
            transform.url.contains(probe.doi),
            "the transform named {}, which does not carry the DOI",
            transform.url
        );
        println!(
            "{} -> {} (named by {})",
            probe.doi, transform.url, transform.named_by
        );
    }
}

/// The six sources are all asked, and the pass still names no publisher host.
///
/// This is the structural claim [`MetadataPass`] makes — "resolve, then rank"
/// means the ranking cannot depend on a publisher's response time — and it is
/// the one property of the measurement that *should* be a test: if the pass
/// started contacting publishers, every number above would be measuring
/// publisher latency rather than metadata quality.
#[tokio::test]
async fn the_live_pass_still_names_no_publisher_host() {
    let (client, pass) = live();
    let probe = OA_260_PROBES
        .first()
        .expect("the probe table is asserted non-empty by oa_live's own tests");
    let resolution = pass
        .resolve(
            &client,
            &WorkScope::new(),
            WorkRefs {
                doi: Some(probe.doi),
                arxiv_id: None,
                osti_id: None,
                url: None,
                openalex_id: None,
                pmcid: None,
            },
        )
        .await;

    // Hosts are hosts; the policy is about buckets, so the check routes each
    // one through the shipped table rather than string-matching a host list.
    // Asserting `METADATA_BUCKETS::contains(host)` would pass for nothing and
    // fail for `api.crossref.org` — which is the mistake this test is here to
    // catch, not to make.
    for host in &resolution.hosts_contacted {
        let url =
            reqwest::Url::parse(&format!("https://{host}/")).expect("a contacted host parses");
        let bucket = client.policy().bucket_for(&url);
        assert!(
            scitadel_adapters::resolve::METADATA_BUCKETS.contains(&bucket.as_str()),
            "{host} spends the {bucket} bucket, which is not one of the six \
             metadata buckets; the live pass contacted a host outside its own \
             policy table"
        );
    }
    println!("hosts contacted: {:?}", resolution.hosts_contacted);
    println!(
        "consulted: {}",
        resolution
            .consulted
            .iter()
            .map(|hop| format!("{}={:?}", hop.registry.label(), hop.outcome))
            .collect::<Vec<_>>()
            .join(" ")
    );
}
