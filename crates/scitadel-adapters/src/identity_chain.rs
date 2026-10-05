//! ADR-007 §3's pre-fetch identity chain, in its own order:
//!
//! > **Pre-fetch**, in S2 for every route: the expected title against the
//! > resolved one (OpenAlex → Crossref → DataCite), fuzzy-matched with
//! > year ±1 and the first author as tie-breakers.
//!
//! # One chain, one matcher, one ladder
//!
//! The decision is [`crate::identity::verify`]'s and nothing else's, and the
//! three registries are consulted here and nowhere else. This module owns
//! **order and fall-through**; it owns no comparison, no threshold and no
//! status vocabulary, which is what keeps a second matcher and a second ladder
//! from being one refactor away.
//!
//! # Why a chain and not "ask the one that knows"
//!
//! The three registries have genuinely disjoint coverage, and a probe of this
//! repo's own evidence (2026-10) is what makes the shape necessary rather than
//! tidy:
//!
//! | DOI | OpenAlex | Crossref | DataCite |
//! |---|---|---|---|
//! | `10.1101/2025.06.14.659707` (bioRxiv, #260) | **200** | **200** | 404 |
//! | `10.18434/m32154` (OSTI/NIST, #260) | **200** | 404 | **200** |
//! | `10.5281/zenodo.23162961` (Zenodo) | 404 | 404 | **200** |
//! | `10.1000/fabricated` (nothing) | 404 | 404 | 404 |
//!
//! Read the third row: a real, registered, publicly readable Zenodo DOI that
//! **no** leg of the chain before DataCite knows anything about. A chain that
//! stopped at the first 404, or that treated "not registered" as "could not
//! read", would leave that work `unverified` for ever — and the fix for that
//! is the third column, not a wider threshold.
//!
//! # The three answers, and which of them ends the chain
//!
//! The distinction the whole module exists to keep is between **"this registry
//! has never heard of this DOI"** and **"we could not find out"**. They look
//! identical if you only look at the status code, they are not, and the wrong
//! reading of either is a bug:
//!
//! - [`HopOutcome::NotRegistered`] — a 404. **The chain continues.** This is
//!   the preprint/repository case the whole slice exists for, and treating it
//!   as a failure would make Crossref's inevitable 404 for a `10.18434` DOI
//!   abort the chain before DataCite is ever asked.
//! - [`HopOutcome::Unreadable`] — a 5xx, a transport failure, a pacing
//!   refusal, or a 200 we cannot parse. **The chain stops.** We do not know
//!   what the later registries would have said, and guessing is worse than
//!   admitting it: a fall-through that skips a hop on a *transient* error
//!   would report "no registry registers this DOI" on a network blip.
//! - [`HopOutcome::Answered`] — a record. **The chain stops** and the remaining
//!   hops are recorded [`HopOutcome::NotConsulted`], because ADR-007 §3's order
//!   is a preference, not a poll: a second registry's title is not a second
//!   opinion worth averaging, it is a different work with the same words.
//!
//! # What happens when nobody answers
//!
//! The honest outcome is [`crate::identity::Verdict::Unverified`], and it is
//! **not** `ok`: three 404s establish that no registry corroborates the work,
//! which is no evidence that it is the work the DOI names. A row is written
//! either way, with both titles — the stored one and none, since there was no
//! resolved one.
//!
//! [`ResolvedWork::source`] is then the **last** hop consulted, which is the
//! only one of the three whose answer made the chain's verdict final, and
//! [`ChainOutcome::why`] spells out the whole trace in the sentence a human
//! reads. Naming the last hop rather than a registry that "answered" is
//! deliberate: there is no `IdentitySource` spelling for "nobody answered", and
//! inventing one would put a value in a column ADR-007 §3 enumerates.

use scitadel_core::config::OpenAlexAuth;
use scitadel_core::models::validate_doi_detailed;
use scitadel_core::ports::PaceTier;
use scitadel_db::sqlite::IdentitySource;
use scitadel_http::{PacedClient, SafeHeaders, WorkScope};

use crate::crossref::{CrossrefAdapter, CrossrefAnswer};
use crate::datacite::{DataCiteAdapter, DataCiteAnswer};
use crate::identity::WorkIdentity;
use crate::openalex;

/// ADR-007 §3's chain, in order. The single authority for it: the walk, the
/// tests and any future caller all read this array rather than re-listing the
/// three, so "the order" has exactly one spelling.
pub const CHAIN: [IdentitySource; 3] = [
    IdentitySource::OpenAlex,
    IdentitySource::Crossref,
    IdentitySource::DataCite,
];

/// One registry's answer about a work: what it is called, when, and by whom.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWork {
    pub identity: WorkIdentity,
    /// The registry that answered. Recorded verbatim in
    /// `paper_identity_checks.source`, using the spellings
    /// [`IdentitySource`] already defines.
    pub source: IdentitySource,
}

/// What one hop of the chain established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HopOutcome {
    /// This registry named the work, and the chain stopped here.
    Answered,
    /// A 404: this registry does not register the DOI. The next one was asked.
    NotRegistered,
    /// We could not find out — a 5xx, a transport failure, or a body we could
    /// not read. The chain stopped, because the later registries' answers are
    /// unknown rather than negative.
    Unreadable,
    /// Never asked, because an earlier hop had already answered.
    NotConsulted,
    /// Nothing to ask with: the work has no DOI, so this registry could only
    /// have been reached by an OpenAlex id and there was none to hand.
    NothingToAsk,
}

impl HopOutcome {
    /// The clause this outcome contributes to [`ChainOutcome::why`].
    fn clause(self, source: IdentitySource) -> String {
        match self {
            Self::Answered => format!("{source} named it"),
            Self::NotRegistered => format!("{source} does not register it"),
            Self::Unreadable => format!("{source} could not be asked"),
            Self::NotConsulted => format!("{source} was not consulted"),
            Self::NothingToAsk => format!("{source} had no identifier to ask with"),
        }
    }
}

/// One hop, as the chain recorded it.
///
/// Kept rather than logged and dropped because the order and the
/// "not consulted" half are both claims that have to be checkable:
/// `the_chain_is_ordered_openalex_then_crossref_then_datacite` reads this
/// vector, and so does anyone debugging why a work came back `unverified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainHop {
    pub source: IdentitySource,
    pub outcome: HopOutcome,
}

/// The chain's result.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainOutcome {
    /// The registry's answer, when one came. `None` means **no registry
    /// registers this DOI** (or none could be asked), which the caller records
    /// as [`crate::identity::Verdict::Unverified`].
    pub resolved: Option<ResolvedWork>,
    /// Every hop, in the order they were decided.
    pub hops: Vec<ChainHop>,
}

impl ChainOutcome {
    /// The registry that actually answered, or `None` if none did.
    ///
    /// The strict reading, and the one a caller that wants to know *whether*
    /// anything was established should use. It is [`Self::recorded_source`]'s
    /// question minus the fallback, kept as its own method so "nobody answered"
    /// is a value a caller can match on rather than a coincidence of two
    /// options.
    #[must_use]
    pub fn answered_by(&self) -> Option<IdentitySource> {
        self.resolved.as_ref().map(|resolved| resolved.source)
    }

    /// The registry to record in `paper_identity_checks.source`.
    ///
    /// The one that actually answered when there was one. When there was none,
    /// the **last** hop consulted — the only one whose answer made the chain's
    /// verdict final — so the column always names a registry this walk really
    /// asked, and never one that stayed silent. See the module docs.
    #[must_use]
    pub fn recorded_source(&self) -> Option<IdentitySource> {
        self.answered_by()
            .or_else(|| self.hops.last().map(|hop| hop.source))
    }

    /// One sentence, in the words `scitadel action_list` and the CLI print.
    #[must_use]
    pub fn why(&self) -> String {
        let trace = self
            .hops
            .iter()
            .map(|hop| hop.outcome.clause(hop.source))
            .collect::<Vec<_>>()
            .join("; ");
        match &self.resolved {
            Some(resolved) => format!(
                "{} named the work: {}",
                resolved.source,
                resolved
                    .identity
                    .title
                    .as_deref()
                    .unwrap_or("no title on record")
            ),
            None => format!(
                "no registry in the chain names this work ({trace}) — \
                 `unverified` is not a pass"
            ),
        }
    }

    /// Did the chain actually put anything on the wire?
    ///
    /// The test seam, and the reason [`crate::download`] can tell "there was
    /// nothing to check" from "we checked and learned nothing" — the first
    /// records no pre-fetch row, the second records `unverified`.
    #[must_use]
    pub fn consulted_any(&self) -> bool {
        !matches!(
            self.hops.first().map(|hop| hop.outcome),
            Some(HopOutcome::NothingToAsk)
        ) && self
            .hops
            .iter()
            .any(|hop| !matches!(hop.outcome, HopOutcome::NothingToAsk))
    }
}

/// The chain, and the three adapters it walks.
///
/// A value rather than three fields consulted at three call sites, so the order
/// lives in one loop and the "stop at the first answer" rule cannot be applied
/// to one hop and forgotten for the next.
#[derive(Debug, Clone)]
pub struct IdentityChain {
    openalex_base: String,
    openalex: OpenAlexAuth,
    crossref: CrossrefAdapter,
    datacite: DataCiteAdapter,
}

impl IdentityChain {
    /// The production chain.
    ///
    /// `mailto` is the polite-pool address both registries need, and it is the
    /// same one OpenAlex takes: one operator, one address, one place to
    /// configure it.
    #[must_use]
    pub fn new(openalex_base: impl Into<String>, openalex: OpenAlexAuth, mailto: &str) -> Self {
        Self {
            openalex_base: openalex_base.into(),
            openalex,
            crossref: CrossrefAdapter::new(mailto),
            datacite: DataCiteAdapter::new(mailto),
        }
    }

    /// The chain, for a test's `wiremock` server. Production uses
    /// [`Self::new`].
    #[must_use]
    pub fn with_bases(
        openalex_base: impl Into<String>,
        crossref_base: impl Into<String>,
        datacite_base: impl Into<String>,
        openalex: OpenAlexAuth,
        mailto: &str,
    ) -> Self {
        Self {
            openalex_base: openalex_base.into(),
            openalex,
            crossref: CrossrefAdapter::new(mailto).with_base_url(crossref_base),
            datacite: DataCiteAdapter::new(mailto).with_base_url(datacite_base),
        }
    }

    /// Walk the chain, in [`CHAIN`]'s order, and stop at the first answer.
    ///
    /// `seeded` is an answer a ladder leg already fetched — today only the
    /// OpenAlex leg produces one, and it produces it by id rather than by DOI.
    /// Passing it here means OpenAlex is **not asked again**: a leg that has
    /// OpenAlex's work in hand has already spent that hop's budget, and a
    /// second request for a record already in memory is the sort of waste
    /// #275's `a_second_run_makes_no_network_calls_for_held_artefacts` exists
    /// to forbid. It is accepted as a *pre-answered first hop* rather than
    /// checked against [`CHAIN`][0]'s position, so a future leg that resolves
    /// through Crossref composes the same way.
    ///
    /// `doi` is validated once, up front, by the same authority
    /// [`crate::download`] gates the ladder on (#262). A DOI that does not
    /// validate is not a hop: there is nothing to ask any registry with.
    pub async fn resolve(
        &self,
        client: &PacedClient,
        work: &WorkScope,
        doi: Option<&str>,
        seeded: Option<ResolvedWork>,
    ) -> ChainOutcome {
        let doi = doi
            .map(str::trim)
            .filter(|doi| !doi.is_empty())
            .and_then(|doi| validate_doi_detailed(doi).ok());

        let mut hops: Vec<ChainHop> = Vec::with_capacity(CHAIN.len());
        let mut resolved: Option<ResolvedWork> = None;

        for source in CHAIN {
            if resolved.is_some() {
                hops.push(ChainHop {
                    source,
                    outcome: HopOutcome::NotConsulted,
                });
                continue;
            }
            let hop = match source {
                IdentitySource::OpenAlex => match seeded.clone() {
                    Some(seed) => Hop::Answered(seed),
                    None => self.ask_openalex(client, work, doi.as_deref()).await,
                },
                IdentitySource::Crossref => match doi.as_deref() {
                    None => Hop::NothingToAsk,
                    Some(doi) => match self.crossref.lookup(client, work, doi).await {
                        Ok(CrossrefAnswer::Found(identity)) => {
                            Hop::Answered(ResolvedWork { identity, source })
                        }
                        Ok(CrossrefAnswer::NotRegistered) => Hop::NotRegistered,
                        // An `Err` here is a base URL that will not parse or a
                        // DOI that failed the adapter's own gate — both are
                        // "we could not find out", and both stop the chain.
                        Ok(CrossrefAnswer::Unreadable(_)) | Err(_) => Hop::Unreadable,
                    },
                },
                IdentitySource::DataCite => match doi.as_deref() {
                    None => Hop::NothingToAsk,
                    Some(doi) => match self.datacite.lookup(client, work, doi).await {
                        Ok(DataCiteAnswer::Found(identity)) => {
                            Hop::Answered(ResolvedWork { identity, source })
                        }
                        Ok(DataCiteAnswer::NotRegistered) => Hop::NotRegistered,
                        Ok(DataCiteAnswer::Unreadable(_)) | Err(_) => Hop::Unreadable,
                    },
                },
                // The two spellings that name no registry on this path. A hop
                // source is a metadata registry by construction, so this arm
                // exists to make adding a non-registry to `CHAIN` a no-op
                // rather than a silent fall-through.
                IdentitySource::ServedPage | IdentitySource::Human => Hop::NothingToAsk,
            };

            let outcome = match hop {
                Hop::Answered(answer) => {
                    resolved = Some(answer);
                    HopOutcome::Answered
                }
                Hop::NotRegistered => HopOutcome::NotRegistered,
                Hop::Unreadable => HopOutcome::Unreadable,
                Hop::NothingToAsk => HopOutcome::NothingToAsk,
            };
            hops.push(ChainHop { source, outcome });
            if matches!(outcome, HopOutcome::Unreadable) {
                // Stop, and mark the rest honestly rather than asking a
                // registry whose answer we can no longer weigh.
                for later in CHAIN.into_iter().skip_while(|s| *s != source).skip(1) {
                    hops.push(ChainHop {
                        source: later,
                        outcome: HopOutcome::NotConsulted,
                    });
                }
                break;
            }
        }

        ChainOutcome { resolved, hops }
    }

    /// Ask OpenAlex for a DOI it may not index, through [`PacedClient`].
    ///
    /// On its own `PacedClient` and no bare `reqwest`, for the reason ADR-007
    /// §3 states: one `Request` permit per hop, out of the `openalex` bucket,
    /// so consulting this registry costs OpenAlex's allowance and not a shared
    /// budget no platform agreed to.
    ///
    /// `Hop::NothingToAsk` when there is no DOI: OpenAlex is the one registry
    /// on this chain that can be reached by an id rather than a DOI, and the
    /// chain has no id to hand when no leg resolved one.
    async fn ask_openalex(&self, client: &PacedClient, work: &WorkScope, doi: Option<&str>) -> Hop {
        let Some(doi) = doi else {
            return Hop::NothingToAsk;
        };
        let Ok(mut url) = openalex::doi_works_url(&self.openalex_base, doi) else {
            return Hop::Unreadable;
        };
        {
            // Both credentials, on every request, for the reasons
            // [`crate::openalex`] documents: `mailto` for the polite pool and
            // `api_key` for the metered quota (#212). Empty ones are omitted
            // rather than sent blank.
            let mut query = url.query_pairs_mut();
            if !self.openalex.email.is_empty() {
                query.append_pair("mailto", &self.openalex.email);
            }
            if !self.openalex.api_key.is_empty() {
                query.append_pair("api_key", &self.openalex.api_key);
            }
        }
        let response = match client
            .get_in_work(work, url, PaceTier::Meta, SafeHeaders::unauthenticated())
            .await
        {
            Ok(response) => response,
            // OpenAlex's measured 404 is a 207-byte **HTML** error page, so it
            // cannot be mistaken for a record even if it were read — and it is
            // not read, because the status settles it first.
            Err(scitadel_http::FetchError::Status { code: 404, .. }) => {
                return Hop::NotRegistered;
            }
            Err(_) => return Hop::Unreadable,
        };
        let text = match response.text().await {
            Ok(text) => text,
            Err(_) => return Hop::Unreadable,
        };
        let body: serde_json::Value = match serde_json::from_str(&text) {
            Ok(body) => body,
            Err(_) => return Hop::Unreadable,
        };
        // OpenAlex's single-record envelope is the bare work object — no
        // `message`/`data` wrapper, unlike both other registries. `id` is the
        // one field every work object carries, so its absence is the honest
        // test for "this 200 was not a work"; a proxy's JSON, a search
        // envelope or an empty object all fail it.
        if body.get("id").is_none() {
            return Hop::Unreadable;
        }
        // A work with no title is still **found**: OpenAlex registered the DOI.
        // The chain stops here, and the matcher reaches its own honest
        // `unverified` on the missing title — which is the right answer,
        // because a second registry's title for the same work would be a
        // different work with the same words.
        Hop::Answered(ResolvedWork {
            identity: openalex::work_identity(&body),
            source: IdentitySource::OpenAlex,
        })
    }
}

/// What one hop produced, before it becomes a [`ChainHop`].
///
/// Carries the answer rather than only the verdict, so the loop has one place
/// that can stop the chain and no way to record a hop as answered without
/// having something to record.
#[derive(Debug, Clone, PartialEq)]
enum Hop {
    Answered(ResolvedWork),
    NotRegistered,
    Unreadable,
    NothingToAsk,
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::ports::{Bucket, Cost, PaceDenied, Pacer, Permit};
    use std::sync::Arc;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The real, measured coverage split. Each row is a DOI this repo's own
    /// evidence names, with what each registry actually answered on 2026-10 —
    /// which is the whole justification for the chain's shape.
    const MEASURED: [(&str, bool, bool, bool); 4] = [
        // DOI                                OpenAlex Crossref DataCite
        ("10.1101/2025.06.14.659707", true, true, false), // bioRxiv, #260
        ("10.18434/m32154", true, false, true),           // OSTI/NIST, #260
        ("10.5281/zenodo.23162961", false, false, true),  // Zenodo
        ("10.1000/fabricated", false, false, false),      // nothing
    ];

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

    /// A policy table that routes three `wiremock` servers — one per registry —
    /// to three named buckets.
    ///
    /// Three servers rather than one, and that is the point: the policy table
    /// keys on **host**, so a single test server cannot host two platforms, and
    /// a path-prefix "route" would test a table the production code never
    /// builds. Production separates these by host too — `api.openalex.org`,
    /// `api.crossref.org`, `api.datacite.org` — so this reproduces the real
    /// separation rather than simulating it.
    fn table_routing(servers: &[MockServer; 3]) -> scitadel_http::BucketPolicyTable {
        let mut table = scitadel_http::BucketPolicyTable::new();
        for (server, bucket) in servers.iter().zip(["openalex", "crossref", "datacite"]) {
            table.route(&format!("127.0.0.1:{}", server.address().port()), bucket);
        }
        table
    }

    fn client() -> PacedClient {
        PacedClient::new(
            PacedClient::default_transport().expect("transport builds"),
            Arc::new(GrantingPacer),
            scitadel_http::BucketPolicyTable::new(),
        )
    }

    /// A `wiremock` registry that answers `404` for everything, which is what a
    /// registry that has never heard of a DOI does.
    async fn silent(server: &MockServer, prefix: &str) {
        Mock::given(method("GET"))
            .and(path_regex(format!("^{prefix}/.*$")))
            .respond_with(ResponseTemplate::new(404).set_body_string("Resource not found.\n"))
            .mount(server)
            .await;
    }

    /// A `wiremock` registry that names a work, in **its own** envelope.
    ///
    /// Two bodies rather than one shared fixture, because the two registries
    /// genuinely disagree on shape and a test that served one to both would
    /// pass for the wrong reason — the adapter that cannot read the other
    /// registry's envelope is exactly what
    /// `the_two_registries_shapes_are_not_interchangeable` pins.
    async fn naming(server: &MockServer, prefix: &str) {
        Mock::given(method("GET"))
            .and(path_regex(format!("^{prefix}/.*$")))
            .respond_with(ResponseTemplate::new(200).set_body_string(match prefix {
                "/oa" => OPENALEX_WORK,
                "/cr" => CROSSREF_WORK,
                "/dc" => DATACITE_WORK,
                other => panic!("no fixture for the {other} route"),
            }))
            .mount(server)
            .await;
    }

    const OPENALEX_WORK: &str = r#"{
      "id": "https://openalex.org/W1",
      "title": "A work",
      "publication_year": 2020,
      "authorships": [{"author": {"display_name": "Young, Christopher J."}}]
    }"#;

    const CROSSREF_WORK: &str = r#"{
      "status": "ok",
      "message": {
        "title": ["A work"],
        "published": {"date-parts": [[2020]]},
        "author": [{"given": "Christopher J.", "family": "Young"}]
      }
    }"#;

    const DATACITE_WORK: &str = r#"{
      "data": {"attributes": {
        "titles": [{"title": "A work"}],
        "publicationYear": 2020,
        "creators": [{"name": "Young, Christopher J.", "familyName": "Young"}]
      }}
    }"#;

    /// The chain, pointed at one server with three route prefixes.
    fn chain_for(server: &MockServer) -> IdentityChain {
        let base = server.uri();
        IdentityChain::with_bases(
            format!("{base}/oa"),
            format!("{base}/cr"),
            format!("{base}/dc"),
            OpenAlexAuth::default(),
            "polite@example.org",
        )
    }

    fn sources(outcome: &ChainOutcome) -> Vec<IdentitySource> {
        outcome.hops.iter().map(|hop| hop.source).collect()
    }

    /// The production constructor's three hosts and the exact request line each
    /// produces.
    ///
    /// Asserted here because every *other* test in this file goes through
    /// [`IdentityChain::with_bases`], so a typo in a production base URL would
    /// otherwise be invisible to the entire suite. This is the one place the
    /// test seam and production share a claim, which is exactly why it needs
    /// saying rather than assuming.
    #[test]
    fn the_production_constructor_targets_the_three_measured_hosts() {
        let chain = IdentityChain::new(
            crate::openalex::OPENALEX_API_URL,
            OpenAlexAuth::default(),
            "polite@example.org",
        );
        assert_eq!(chain.openalex_base, "https://api.openalex.org/works");
        assert_eq!(
            chain
                .crossref
                .works_url("10.18434/m32154")
                .map(|url| url.to_string())
                .ok(),
            Some(
                "https://api.crossref.org/works/10.18434%2Fm32154?mailto=polite%40example.org"
                    .to_string()
            ),
            "the measured Crossref line: DOI in the path, `mailto` in the query"
        );
        assert_eq!(
            chain
                .datacite
                .dois_url("10.18434/m32154")
                .map(|url| url.to_string())
                .ok(),
            Some(
                "https://api.datacite.org/dois/10.18434%2Fm32154?mailto=polite%40example.org"
                    .to_string()
            ),
            "and the measured DataCite one, which differs only in the host and \
             the `/dois` path"
        );
    }

    fn outcomes(outcome: &ChainOutcome) -> Vec<HopOutcome> {
        outcome.hops.iter().map(|hop| hop.outcome).collect()
    }

    // =====================================================================
    // The order, and the stop-at-the-first-answer rule.
    // =====================================================================

    /// ADR-007 §3's order, read off the hops rather than off a comment.
    ///
    /// And the second half, which is the rule that makes the order meaningful:
    /// once a hop answers, the later registries are **not consulted** — the
    /// chain stops, and the hops say so. A second registry's title is not a
    /// second opinion to reconcile; it is a different work with the same words.
    #[tokio::test]
    async fn the_chain_is_ordered_openalex_then_crossref_then_datacite() {
        let server = MockServer::start().await;
        // OpenAlex answers, so nothing after it may be asked.
        Mock::given(method("GET"))
            .and(path("/oa/doi:10.1038%2Fnature.2024.001"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"id":"https://openalex.org/W1","title":"A nature paper","publication_year":2024,
                             "authorships":[{"author":{"display_name":"Harris, James M."}}]}"#,
                    ),
            )
            .mount(&server)
            .await;

        let outcome = chain_for(&server)
            .resolve(
                &client(),
                &WorkScope::new(),
                Some("10.1038/nature.2024.001"),
                None,
            )
            .await;

        assert_eq!(
            sources(&outcome),
            vec![
                IdentitySource::OpenAlex,
                IdentitySource::Crossref,
                IdentitySource::DataCite
            ],
            "ADR-007 §3's order, in the hops themselves"
        );
        assert_eq!(
            outcomes(&outcome),
            vec![
                HopOutcome::Answered,
                HopOutcome::NotConsulted,
                HopOutcome::NotConsulted
            ],
            "a later source must not be consulted once one has answered"
        );
        assert_eq!(outcome.recorded_source(), Some(IdentitySource::OpenAlex));

        // And nothing was requested of the two that were not consulted.
        let requested: Vec<String> = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .map(|r| r.url.path().to_string())
            .collect();
        assert_eq!(requested, vec!["/oa/doi:10.1038%2Fnature.2024.001"]);
    }

    /// The same rule for a *seeded* first hop: a ladder leg that already holds
    /// OpenAlex's work must not cause a second OpenAlex request. Asserted
    /// through a request count, because "the chain asked OpenAlex again" and
    /// "the chain asked OpenAlex again and got the same answer" look identical
    /// from the answer.
    #[tokio::test]
    async fn a_seeded_first_hop_is_not_re_requested() {
        let server = MockServer::start().await;
        silent(&server, "/oa").await;
        silent(&server, "/cr").await;
        silent(&server, "/dc").await;

        let outcome = chain_for(&server)
            .resolve(
                &client(),
                &WorkScope::new(),
                Some("10.1101/2025.06.14.659707"),
                Some(ResolvedWork {
                    identity: WorkIdentity::full(
                        "A preprint",
                        Some(2025),
                        Some("Passaro, Saro".into()),
                    ),
                    source: IdentitySource::OpenAlex,
                }),
            )
            .await;

        assert_eq!(
            outcomes(&outcome),
            vec![
                HopOutcome::Answered,
                HopOutcome::NotConsulted,
                HopOutcome::NotConsulted
            ]
        );
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty(),
            "a leg that already holds the record must not spend the hop again"
        );
    }

    // =====================================================================
    // The fall-through, which is the reason this module exists.
    // =====================================================================

    /// A 404 is a fact and the chain walks past it.
    ///
    /// Asserted on every one of the four measured coverage rows, because the
    /// rows disagree with each other and a rule that worked for one would not
    /// work for the others.
    #[tokio::test]
    async fn a_404_falls_through_rather_than_ending_the_chain() {
        for (doi, openalex, crossref, datacite) in MEASURED {
            let server = MockServer::start().await;
            for (prefix, hit) in [("/oa", openalex), ("/cr", crossref), ("/dc", datacite)] {
                if hit {
                    naming(&server, prefix).await;
                } else {
                    silent(&server, prefix).await;
                }
            }

            let outcome = chain_for(&server)
                .resolve(&client(), &WorkScope::new(), Some(doi), None)
                .await;

            let expected_source = match (openalex, crossref, datacite) {
                (true, _, _) => Some(IdentitySource::OpenAlex),
                (false, true, _) => Some(IdentitySource::Crossref),
                (false, false, true) => Some(IdentitySource::DataCite),
                (false, false, false) => None,
            };
            assert_eq!(
                outcome.answered_by(),
                expected_source,
                "{doi}: measured coverage (oa={openalex}, cr={crossref}, dc={datacite}) \
                 must produce this source"
            );
            // Whatever answered, the hops before it were 404s and the hops
            // after it were not consulted — never the reverse.
            let answered = outcome
                .hops
                .iter()
                .position(|hop| hop.outcome == HopOutcome::Answered);
            if let Some(at) = answered {
                assert!(
                    outcome.hops[..at]
                        .iter()
                        .all(|hop| hop.outcome == HopOutcome::NotRegistered),
                    "{doi}: every hop before the answer was a 404: {:?}",
                    outcome.hops
                );
                assert!(
                    outcome.hops[at + 1..]
                        .iter()
                        .all(|hop| hop.outcome == HopOutcome::NotConsulted),
                    "{doi}: every hop after the answer was not consulted: {:?}",
                    outcome.hops
                );
            }
        }
    }

    /// The hop order is fixed by [`CHAIN`] and every hop is recorded, so a walk
    /// that consulted all three reads as three `NotRegistered` hops and nothing
    /// else. This is the "unknown to both" shape, asserted at the chain level;
    /// the end-to-end `unverified` row is asserted through `acquire`.
    #[tokio::test]
    async fn a_doi_no_registry_knows_consults_all_three_and_records_each() {
        let server = MockServer::start().await;
        silent(&server, "/oa").await;
        silent(&server, "/cr").await;
        silent(&server, "/dc").await;

        let outcome = chain_for(&server)
            .resolve(
                &client(),
                &WorkScope::new(),
                Some("10.1000/fabricated"),
                None,
            )
            .await;

        assert_eq!(outcome.resolved, None, "nobody named it");
        assert_eq!(
            outcomes(&outcome),
            vec![
                HopOutcome::NotRegistered,
                HopOutcome::NotRegistered,
                HopOutcome::NotRegistered
            ],
            "a 404 must not end the chain — that is the preprint case"
        );
        assert_eq!(
            outcome.recorded_source(),
            Some(IdentitySource::DataCite),
            "the last hop, whose answer made the verdict final"
        );
        assert!(outcome.consulted_any());
        let why = outcome.why();
        for source in ["openalex", "crossref", "datacite"] {
            assert!(why.contains(source), "{why} must name {source}");
        }
    }

    /// The opposite of a fall-through: a hop we **could not** read stops the
    /// chain, and the later hops say they were not consulted.
    ///
    /// The distinction is the module's whole reason for existing, and it is
    /// invisible if you only look at "the chain did not get an answer": a
    /// transient 500 must not be reported as "no registry registers this DOI",
    /// because that claim is about the work and the 500 is about us.
    #[tokio::test]
    async fn an_unreadable_hop_stops_the_chain_rather_than_skipping_past_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex("^/oa/.*$"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        // A later hop that *would* have answered, so a chain that wrongly
        // skipped past the failure would produce an `ok` here.
        Mock::given(method("GET"))
            .and(path_regex("^/dc/.*$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"data":{"attributes":{"titles":[{"title":"A work"}],
                             "publicationYear":2020,
                             "creators":[{"name":"Young, C."}]}}}"#,
                    ),
            )
            .mount(&server)
            .await;

        let outcome = chain_for(&server)
            .resolve(&client(), &WorkScope::new(), Some("10.5281/zenodo.1"), None)
            .await;

        assert_eq!(
            outcomes(&outcome),
            vec![
                HopOutcome::Unreadable,
                HopOutcome::NotConsulted,
                HopOutcome::NotConsulted
            ],
            "a hop we could not read is not a hop that said no"
        );
        assert_eq!(outcome.resolved, None);
        assert!(outcome.why().contains("unverified"), "{}", outcome.why());
    }

    /// A work with no DOI has nothing to ask Crossref or DataCite with, and the
    /// chain must say so rather than reporting a coverage miss — OpenAlex *can*
    /// be asked by id, so the first hop is `NothingToAsk` too when no leg
    /// resolved one.
    #[tokio::test]
    async fn a_work_with_no_doi_asks_nobody() {
        let server = MockServer::start().await;
        let outcome = chain_for(&server)
            .resolve(&client(), &WorkScope::new(), None, None)
            .await;
        assert_eq!(
            outcomes(&outcome),
            vec![
                HopOutcome::NothingToAsk,
                HopOutcome::NothingToAsk,
                HopOutcome::NothingToAsk
            ]
        );
        assert_eq!(outcome.resolved, None);
        assert!(
            !outcome.consulted_any(),
            "nothing was put on the wire, so the caller records no pre-fetch row"
        );
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty(),
            "and nothing was requested"
        );
    }

    /// A DOI the validator rejects is not a hop, for #262's reason: the same
    /// authority that gates the ladder gates this, so there is no second, more
    /// permissive opinion of what a DOI is.
    #[tokio::test]
    async fn an_invalid_doi_asks_nobody() {
        let server = MockServer::start().await;
        let outcome = chain_for(&server)
            .resolve(&client(), &WorkScope::new(), Some("not a doi"), None)
            .await;
        assert!(!outcome.consulted_any());
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty()
        );
    }

    /// A 200 from OpenAlex that is not a work object is "we could not read it",
    /// not "OpenAlex does not register it" — its 404 is an HTML page, so the
    /// `id` check is what a proxy's JSON has to survive.
    #[tokio::test]
    async fn an_openalex_200_without_a_work_is_unreadable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex("^/oa/.*$"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"meta":{"count":0}}"#),
            )
            .mount(&server)
            .await;

        let outcome = chain_for(&server)
            .resolve(
                &client(),
                &WorkScope::new(),
                Some("10.1000/fabricated"),
                None,
            )
            .await;
        assert_eq!(outcomes(&outcome)[0], HopOutcome::Unreadable);
        assert_eq!(outcome.resolved, None);
    }

    /// One `Request` permit per hop it actually makes, out of **three different
    /// platforms'** budgets — one per registry, which is the whole of ADR-007
    /// §4's "buckets are platforms, not hosts".
    ///
    /// Three servers, because a policy table keys on **host**: one server would
    /// put all three hops in one bucket, and the assertion would then be about
    /// the test rig rather than about the chain. Production separates these by
    /// host too — `api.openalex.org`, `api.crossref.org`, `api.datacite.org` —
    /// so this reproduces the real separation rather than simulating it.
    #[tokio::test]
    async fn each_hop_spends_its_own_platforms_bucket() {
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

        // One server per registry, each answering 404 for everything, so all
        // three hops really happen and in ADR-007 §3's order.
        let servers = [
            MockServer::start().await,
            MockServer::start().await,
            MockServer::start().await,
        ];
        for server in &servers {
            silent(server, "").await;
        }

        let chain = IdentityChain::with_bases(
            servers[0].uri(),
            servers[1].uri(),
            servers[2].uri(),
            OpenAlexAuth::default(),
            "polite@example.org",
        );
        let pacer = Arc::new(RecordingPacer::default());
        let client = PacedClient::new(
            PacedClient::default_transport().expect("transport"),
            Arc::clone(&pacer) as Arc<dyn Pacer>,
            table_routing(&servers),
        );
        let outcome = chain
            .resolve(&client, &WorkScope::new(), Some("10.1000/fabricated"), None)
            .await;
        assert_eq!(outcome.resolved, None, "all three registries answered 404");

        assert_eq!(
            pacer.0.lock().expect("lock").clone(),
            vec![
                ("openalex".to_string(), PaceTier::Meta),
                ("crossref".to_string(), PaceTier::Meta),
                ("datacite".to_string(), PaceTier::Meta),
            ],
            "three hops, three platforms, three `Request` permits, all at the \
             `Meta` tier, in ADR-007 §3's order"
        );
    }
}
