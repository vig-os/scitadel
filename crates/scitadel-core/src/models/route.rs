//! The acquisition vocabulary: which route produced an artefact, what the
//! bytes turned out to be, and what we may therefore keep them for.
//!
//! ADR-007 §3 "Routes and the ladder" enumerates the fetch order, and §1
//! stores the answer on every row as `artefacts.route`. This module is that
//! enumeration as a closed type, so "which route was this?" has exactly one
//! spelling in the database instead of one string built per call site — the
//! same failure mode [`crate::publisher`] exists to fix for publishers
//! (#261), where two hand-maintained prefix tables had already diverged.
//!
//! Two of the three things here are *storage* contracts, because
//! `artefacts.route`, `artefacts.version` and `artefacts.access_basis` are
//! constrained columns in migration 013:
//!
//! - [`RouteId::label`] is the `route` value.
//! - [`RouteId::artefact_version`] is the `version` value.
//! - [`RouteId::access_basis`] is the `access_basis` value.
//!
//! Each accessor has a test below that pins its output to the vocabulary
//! migration 013 declares, because a label outside that vocabulary is not a
//! new route — it is a CHECK constraint violation at acquisition time.
//!
//! ## What S1 wires up
//!
//! Only six routes fetch in S1: [`RouteId::Arxiv`], [`RouteId::Biorxiv`],
//! [`RouteId::OpenAlex`], [`RouteId::Unpaywall`], [`RouteId::Publisher`] and
//! [`RouteId::ManualUrl`] — the four index/landing legs `download_paper` has
//! always tried, the last-resort URL, and #260's preprint leg, which is a
//! transform of the DOI rather than an index lookup. The rest are declared
//! now, before anything uses them, because S2/S3 record `route` on artefacts
//! they fetch and the column has no `CHECK` constraint to fall back on: a
//! route invented at the call site is a route nothing else can match. Their
//! [`Self::fetch_tier`] and [`Self::access_basis`] answers are still real
//! answers, so S2 does not have to re-derive them.
//!
//! ## The four pseudo-routes
//!
//! [`RouteId::Legacy`], [`RouteId::ImportFlat`], [`RouteId::Manual`] and
//! [`RouteId::ManualUrl`] are not ladder steps — migration 013's column
//! comment admits them alongside `RouteId`s because they record *where a file
//! came from* when nothing in the resolver chain fetched it.
//!
//! Three of them genuinely never touch the network
//! ([`Self::never_fetches`]): the legacy backfill, the flat-layout importer
//! and a hand-placed file. [`RouteId::ManualUrl`] is the odd one out and
//! deliberately so: it is the paper record's own `url`, fetched by exactly the
//! same code path as [`RouteId::Publisher`], so it is a real fetch that merely
//! got its bytes from outside the chain. It is grouped with the other three in
//! the column comment because it shares their provenance, and
//! [`Self::fetch_tier`] returns `None` only for the three that truly never go
//! to the network — a tier for a route that cannot fetch would be a lie, and
//! `manual_url` is not one of them.

use crate::models::DownloadStatus;
use crate::ports::PaceTier;

/// Which route produced an artefact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RouteId {
    // ---- Open access, free to read by construction (ADR-007 §3 steps 1-3) ----
    /// Europe PMC: `fullTextXML` (JATS) and `supplementaryFiles`.
    EuropePmc,
    /// The `pmc-oa-opendata` dataset. Anonymous HTTPS; `oa.fcgi` was retired
    /// on 2026-08-25.
    PmcOa,
    /// An OA repository location named by OpenAlex's `locations[]`.
    OpenAlex,
    /// An OA repository location named by Unpaywall's `best_oa_location`.
    Unpaywall,
    /// The direct arXiv PDF. Free, no API call.
    Arxiv,
    /// bioRxiv/medRxiv, reached by the DOI transform in
    /// `scitadel_adapters::preprint` rather than through an index (#260).
    ///
    /// One route for both servers because the `10.1101` prefix covers both
    /// and the DOI does not say which one a preprint is on — and because
    /// `medrxiv.org` refuses the `/content/<doi>` path outright, so there is
    /// nothing for a second variant to distinguish. The label matches the
    /// `biorxiv` bucket in `scitadel_http`'s policy table, which likewise
    /// spends one budget for `biorxiv.org` and `medrxiv.org` together
    /// (spelled without a link: this crate does not depend on that one).
    Biorxiv,
    /// DOE OSTI, for national-lab reports that have no DOI.
    Osti,

    // ---- Supplementary-material discovery (ADR-007 §3 step 4) ----
    /// DataCite `IsSupplementTo`, which covers Taylor & Francis supplements
    /// on Figshare plus Zenodo and Dryad.
    DataCite,
    /// The cheap Crossref `is-supplemented-by` check.
    Crossref,

    // ---- Tier 2: sanctioned TDM (ADR-007 §3 step 5) ----
    /// Elsevier's Article Retrieval and Object APIs.
    ElsevierTdm,
    /// Wiley's TDM endpoint.
    WileyTdm,
    /// Springer Nature's platform download.
    SpringerTdm,

    // ---- Tier 3 and 4 ----
    /// The publisher's own landing page. Never ScienceDirect (ADR-007 §3).
    Publisher,
    /// A human's browser session, started by a person and never through MCP
    /// (ADR-007 §5).
    BrowserSession,

    // ---- Provenance only; never fetched ----
    /// A file that predates the `artefacts` table, carried across by the
    /// S1 backfill.
    Legacy,
    /// A consumer-written layout read by `scitadel import-flat` (raid).
    ImportFlat,
    /// A hand-placed file, reconciled only by `scitadel scan` or `attach`.
    Manual,
    /// A URL supplied from outside the resolver chain — the paper record's
    /// own `url`, or whatever a person typed.
    ManualUrl,
}

impl RouteId {
    /// The stable database value for `artefacts.route`.
    ///
    /// Lowercase and `_`-separated, matching the four pseudo-route spellings
    /// migration 013 already documents. A published crate cannot rename one of
    /// these without a data migration.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::EuropePmc => "europepmc",
            Self::PmcOa => "pmc_oa",
            Self::OpenAlex => "openalex",
            Self::Unpaywall => "unpaywall",
            Self::Arxiv => "arxiv",
            Self::Biorxiv => "biorxiv",
            Self::Osti => "osti",
            Self::DataCite => "datacite",
            Self::Crossref => "crossref",
            Self::ElsevierTdm => "elsevier_tdm",
            Self::WileyTdm => "wiley_tdm",
            Self::SpringerTdm => "springer_tdm",
            Self::Publisher => "publisher",
            Self::BrowserSession => "browser_session",
            Self::Legacy => "legacy",
            Self::ImportFlat => "import_flat",
            Self::Manual => "manual",
            Self::ManualUrl => "manual_url",
        }
    }

    /// Every route, so a coverage report or a test cannot miss one.
    pub const ALL: [Self; 18] = [
        Self::EuropePmc,
        Self::PmcOa,
        Self::OpenAlex,
        Self::Unpaywall,
        Self::Arxiv,
        Self::Biorxiv,
        Self::Osti,
        Self::DataCite,
        Self::Crossref,
        Self::ElsevierTdm,
        Self::WileyTdm,
        Self::SpringerTdm,
        Self::Publisher,
        Self::BrowserSession,
        Self::Legacy,
        Self::ImportFlat,
        Self::Manual,
        Self::ManualUrl,
    ];

    /// The routes that record where a file came from rather than fetching it:
    /// the legacy backfill, the flat-layout importer, and a hand-placed file.
    ///
    /// These are the three of migration 013's four documented pseudo-routes
    /// that can never spend a permit. [`Self::ManualUrl`] is the fourth and is
    /// *not* one of them — it fetches.
    #[must_use]
    pub fn never_fetches(self) -> bool {
        matches!(self, Self::Legacy | Self::ImportFlat | Self::Manual)
    }

    /// The `PaceTier` this route's requests are charged at, or `None` for a
    /// pseudo-route.
    ///
    /// ADR-007 §4's four tiers, with one judgement call worth naming: Europe
    /// PMC sits in the §4 "metadata APIs" row alongside OpenAlex, Crossref,
    /// DataCite and Unpaywall, so it charges `Meta` even though the bytes it
    /// serves are full text. Its bucket policy is 100 ms / 100 000 requests —
    /// the loosest of the API rows — so the mis-attribution cannot cost us
    /// anything, and putting it in `Oa` would spend a *repositories* budget on
    /// a metadata API.
    ///
    /// `Publisher` charges `Oa` rather than inventing a fifth tier: §3 step 6
    /// fetches landing-page HTML "for OA works only, after TDM", and the
    /// interval that actually throttles it comes from the bucket's policy, not
    /// from this label.
    #[must_use]
    pub fn fetch_tier(self) -> Option<PaceTier> {
        match self {
            Self::EuropePmc
            | Self::OpenAlex
            | Self::Unpaywall
            | Self::DataCite
            | Self::Crossref => Some(PaceTier::Meta),
            Self::PmcOa
            | Self::Arxiv
            | Self::Biorxiv
            | Self::Osti
            | Self::Publisher
            | Self::ManualUrl => Some(PaceTier::Oa),
            Self::ElsevierTdm | Self::WileyTdm | Self::SpringerTdm => Some(PaceTier::Tdm),
            Self::BrowserSession => Some(PaceTier::Session),
            Self::Legacy | Self::ImportFlat | Self::Manual => None,
        }
    }

    /// The `artefacts.version` value a fetch on this route can support, or
    /// `None` for a pseudo-route.
    ///
    /// Only the two preprint-by-construction routes get to claim
    /// `preprint`: a PDF served from arXiv or from bioRxiv/medRxiv *is* a
    /// preprint, whatever the DOI says about its eventual journal version,
    /// and the ADR's own "have" rule trusts `unknown` on
    /// `route IN ('legacy','import_flat')` rows precisely because nothing
    /// else should carry it. Every other route is `unknown` here and becomes
    /// `vor`/`am` when S3's resolve-then-rank compares OpenAlex, Crossref and
    /// DataCite against each other. Claiming `vor` from a single Unpaywall
    /// location is the overclaiming #261 exists to stop.
    #[must_use]
    pub fn artefact_version(self) -> Option<&'static str> {
        if self.never_fetches() {
            return None;
        }
        Some(if matches!(self, Self::Arxiv | Self::Biorxiv) {
            "preprint"
        } else {
            "unknown"
        })
    }

    /// The `artefacts.access_basis` value this route supports, or `None` when
    /// the route establishes no basis at all.
    ///
    /// `access_basis` is `NOT NULL`, so "we did not look" has no spelling —
    /// which is why the answer here can be `None`, and why a caller must then
    /// decide for itself rather than inherit a guess.
    ///
    /// - `oa_license`: routes that only ever serve material free to read by
    ///   construction. (OSTI's work is US-government public domain, which the
    ///   six-value vocabulary has no term for; `oa_license` is the honest
    ///   neighbour, since nothing restricts it.)
    /// - `tdm_licence`: the sanctioned TDM platforms, whose licences the
    ///   publisher actually published.
    /// - `subscription_read`: bytes read under whatever entitlement the
    ///   caller's IP or session had. §2's `needs_authorisation` and
    ///   `not_entitled` are how a *failure* to hold that entitlement is
    ///   recorded, so this value is the honest reading of "it worked".
    /// - `manual`: no machine vouched for these bytes.
    /// - `None`: the two SI-discovery routes. They answer "where do the bytes
    ///   live", not "may we keep them", so the basis belongs to whatever route
    ///   serves them — and S5 is the slice that resolves it.
    #[must_use]
    pub fn access_basis(self) -> Option<&'static str> {
        match self {
            Self::EuropePmc
            | Self::PmcOa
            | Self::OpenAlex
            | Self::Unpaywall
            | Self::Arxiv
            | Self::Biorxiv
            | Self::Osti => Some("oa_license"),
            Self::ElsevierTdm | Self::WileyTdm | Self::SpringerTdm => Some("tdm_licence"),
            Self::Publisher | Self::BrowserSession | Self::ManualUrl => Some("subscription_read"),
            Self::Legacy | Self::ImportFlat | Self::Manual => Some("manual"),
            Self::DataCite | Self::Crossref => None,
        }
    }
}

impl std::fmt::Display for RouteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for RouteId {
    type Err = UnknownRoute;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|route| route.label() == s)
            .ok_or_else(|| UnknownRoute(s.to_string()))
    }
}

/// A `RouteId` label that no route produces.
///
/// Carries the offending string, because the caller is about to write it into
/// a column or a report and needs to say which value it could not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownRoute(pub String);

impl std::fmt::Display for UnknownRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} is not a known route", self.0)
    }
}

impl std::error::Error for UnknownRoute {}

/// Access level inferred from downloaded content.
///
/// Migration 013 constrains `artefacts.access_status` to four values, and
/// these four variants are them. [`Self::label`] is the column value;
/// [`std::fmt::Display`] is the human phrasing the TUI and CLI already print,
/// kept byte-identical so no output changes.
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

impl AccessStatus {
    /// The `artefacts.access_status` value.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::FullText => "full_text",
            Self::Abstract => "abstract",
            Self::Paywall => "paywall",
            Self::Unknown => "unknown",
        }
    }

    /// Every value [`Self::label`] can produce. A test below asserts this is
    /// exactly the migration 013 CHECK vocabulary — the list is here so that
    /// assertion can be exhaustive without a second hand-written copy.
    pub const ALL_LABELS: [&'static str; 4] = ["full_text", "abstract", "paywall", "unknown"];

    /// The legacy `papers.download_status` this access level implies.
    ///
    /// One mapping, defined once. The TUI has always collapsed Abstract,
    /// Paywall and Unknown into a single `paywall` value (ADR-007 §1 "Legacy
    /// data" calls this out), and both the TUI and the CLI's post-download
    /// write used to re-derive it independently — which is how two writers
    /// eventually disagree about the same download.
    #[must_use]
    pub fn download_status(self) -> DownloadStatus {
        match self {
            Self::FullText => DownloadStatus::Downloaded,
            Self::Abstract | Self::Paywall | Self::Unknown => DownloadStatus::Paywall,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The `artefacts.access_basis` CHECK vocabulary from migration 013.
    const ACCESS_BASIS_VOCABULARY: [&str; 6] = [
        "oa_license",
        "tdm_licence",
        "statutory_24d",
        "subscription_read",
        "ill",
        "manual",
    ];

    /// The `artefacts.version` CHECK vocabulary from migration 013.
    const VERSION_VOCABULARY: [&str; 4] = ["vor", "am", "preprint", "unknown"];

    /// The four pseudo-route spellings migration 013's column comment names.
    const MIGRATION_PSEUDO_ROUTES: [&str; 4] = ["legacy", "import_flat", "manual", "manual_url"];

    /// `label` is a database value. Two routes sharing one would make
    /// `artefacts.route` unable to say which route produced a file, and a
    /// non-conforming character would make the value awkward to match.
    #[test]
    fn every_route_has_a_distinct_conforming_label() {
        let mut seen = HashSet::new();
        for route in RouteId::ALL {
            let label = route.label();
            assert!(
                seen.insert(label),
                "{label:?} is claimed by more than one route"
            );
            assert!(
                label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{label:?} is not a safe database value"
            );
            assert_eq!(route.to_string(), label, "Display must be the label");
        }
        assert_eq!(seen.len(), RouteId::ALL.len());
    }

    /// A route added to the enum but forgotten in `ALL` vanishes from every
    /// report that iterates routes, with nothing to notice it.
    #[test]
    fn all_covers_every_route() {
        let mut labels: Vec<&str> = RouteId::ALL.iter().map(|r| r.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 18, "the enum has eighteen routes");
        assert!(RouteId::ALL.contains(&RouteId::DataCite));
        assert!(RouteId::ALL.contains(&RouteId::ManualUrl));
        assert!(
            RouteId::ALL.contains(&RouteId::Biorxiv),
            "#260 added a route, and a route missing from ALL vanishes from every \
             report that iterates routes with nothing to notice it"
        );
    }

    /// The pseudo-routes are named in migration 013, not invented here, and
    /// this pins the exact spellings the backfill and the flat importer write.
    /// Three of the four never fetch; `manual_url` does, and the two facts are
    /// asserted separately so neither can quietly absorb the other.
    #[test]
    fn the_migration_s_pseudo_route_spellings_are_all_representable() {
        for label in MIGRATION_PSEUDO_ROUTES {
            let route: RouteId = label.parse().expect("a migration value parses");
            assert_eq!(route.label(), label);
        }
        assert!(RouteId::Legacy.never_fetches());
        assert!(RouteId::ImportFlat.never_fetches());
        assert!(RouteId::Manual.never_fetches());
        assert!(
            !RouteId::ManualUrl.never_fetches(),
            "manual_url fetches the paper record's own URL — it is not a \
             provenance-only value, despite sharing their column spelling"
        );
    }

    /// `Parse` is how a report or a manifest reads a stored value back. If it
    /// disagreed with `label`, a written row would be unreadable.
    #[test]
    fn labels_round_trip_through_parse() {
        for route in RouteId::ALL {
            let parsed: RouteId = route.label().parse().expect("a written label parses");
            assert_eq!(parsed, route);
        }
        let err = "sciencedirect_scraped".parse::<RouteId>().unwrap_err();
        assert!(err.to_string().contains("sciencedirect_scraped"), "{err}");
    }

    /// Only a route that genuinely never touches the network may lack a tier.
    #[test]
    fn only_a_route_that_never_fetches_has_no_tier() {
        for route in RouteId::ALL {
            assert_eq!(
                route.fetch_tier().is_none(),
                route.never_fetches(),
                "{route} tier/never_fetches disagree"
            );
        }
        assert_eq!(RouteId::ManualUrl.fetch_tier(), Some(PaceTier::Oa));
        assert_eq!(RouteId::Arxiv.fetch_tier(), Some(PaceTier::Oa));
        assert_eq!(RouteId::OpenAlex.fetch_tier(), Some(PaceTier::Meta));
        assert_eq!(RouteId::ElsevierTdm.fetch_tier(), Some(PaceTier::Tdm));
        assert_eq!(
            RouteId::BrowserSession.fetch_tier(),
            Some(PaceTier::Session)
        );
        for route in [RouteId::Legacy, RouteId::ImportFlat, RouteId::Manual] {
            assert_eq!(route.fetch_tier(), None, "{route}");
        }
    }

    /// ADR-007 §4's tiers have separate rolling caps, so putting a route in
    /// the wrong one spends the wrong budget. These are the four rows §3
    /// actually names.
    #[test]
    fn fetch_tiers_follow_the_adrs_classification() {
        for route in [
            RouteId::Arxiv,
            RouteId::Biorxiv,
            RouteId::Osti,
            RouteId::PmcOa,
        ] {
            assert_eq!(route.fetch_tier(), Some(PaceTier::Oa), "{route}");
        }
        for route in [
            RouteId::OpenAlex,
            RouteId::Unpaywall,
            RouteId::EuropePmc,
            RouteId::Crossref,
            RouteId::DataCite,
        ] {
            assert_eq!(route.fetch_tier(), Some(PaceTier::Meta), "{route}");
        }
        for route in [
            RouteId::ElsevierTdm,
            RouteId::WileyTdm,
            RouteId::SpringerTdm,
        ] {
            assert_eq!(route.fetch_tier(), Some(PaceTier::Tdm), "{route}");
        }
    }

    /// Every value these two accessors produce must satisfy the CHECK
    /// constraints of migration 013, or the write fails at acquisition time
    /// with a constraint error instead of at compile time.
    #[test]
    fn artefact_values_stay_inside_the_migration_vocabulary() {
        for route in RouteId::ALL {
            let version = route.artefact_version();
            assert_eq!(
                version.is_none(),
                route.never_fetches(),
                "{route}: version presence must match pseudo"
            );
            if let Some(version) = version {
                assert!(
                    VERSION_VOCABULARY.contains(&version),
                    "{route} would write version {version:?}"
                );
            }
            if let Some(basis) = route.access_basis() {
                assert!(
                    ACCESS_BASIS_VOCABULARY.contains(&basis),
                    "{route} would write access_basis {basis:?}"
                );
            }
        }
    }

    /// A PDF served from a preprint server is a preprint, by construction:
    /// arXiv (#S1's record-level leg) and bioRxiv/medRxiv (#260's DOI
    /// transform). Nothing else in S1 can tell VoR from author manuscript, so
    /// everything else stays `unknown`.
    ///
    /// The set is pinned rather than stated in prose because the failure mode
    /// is silent: a route added to the preprint arm of
    /// [`Self::artefact_version`] without being listed here would make a
    /// publisher's VoR masquerade as a preprint — the inverse of the
    /// overclaim this accessor exists to prevent.
    #[test]
    fn only_the_preprint_servers_may_claim_a_version() {
        assert_eq!(RouteId::Arxiv.artefact_version(), Some("preprint"));
        assert_eq!(RouteId::Biorxiv.artefact_version(), Some("preprint"));
        for route in RouteId::ALL
            .into_iter()
            .filter(|r| !r.never_fetches())
            .filter(|r| !matches!(r, RouteId::Arxiv | RouteId::Biorxiv))
        {
            assert_eq!(
                route.artefact_version(),
                Some("unknown"),
                "{route} must not claim a version it did not establish"
            );
        }
    }

    /// The two SI-discovery routes discover where bytes live; they do not
    /// serve them, so they establish no access basis and the caller must say
    /// which route actually did.
    #[test]
    fn discovery_routes_establish_no_access_basis() {
        assert_eq!(RouteId::DataCite.access_basis(), None);
        assert_eq!(RouteId::Crossref.access_basis(), None);
        assert_eq!(RouteId::Arxiv.access_basis(), Some("oa_license"));
        assert_eq!(
            RouteId::Biorxiv.access_basis(),
            Some("oa_license"),
            "a preprint server serves material free to read by construction"
        );
        assert_eq!(RouteId::ElsevierTdm.access_basis(), Some("tdm_licence"));
        assert_eq!(RouteId::Publisher.access_basis(), Some("subscription_read"));
        assert_eq!(RouteId::ManualUrl.access_basis(), Some("subscription_read"));
        assert_eq!(RouteId::Legacy.access_basis(), Some("manual"));
    }

    /// `label` is the `access_status` value, and the set of labels must be
    /// exactly the migration's CHECK list — no more (a typo becomes a
    /// constraint violation), no fewer (an unrepresentable status).
    #[test]
    fn access_status_labels_are_exactly_the_migration_vocabulary() {
        let produced: HashSet<&str> = [
            AccessStatus::FullText,
            AccessStatus::Abstract,
            AccessStatus::Paywall,
            AccessStatus::Unknown,
        ]
        .into_iter()
        .map(AccessStatus::label)
        .collect();
        let expected: HashSet<&str> = AccessStatus::ALL_LABELS.into_iter().collect();
        assert_eq!(produced, expected);
    }

    /// The human-facing phrasing is printed by the TUI task panel and the
    /// CLI, so it is a compatibility surface like any other.
    #[test]
    fn access_status_display_strings_are_unchanged() {
        assert_eq!(AccessStatus::FullText.to_string(), "full text");
        assert_eq!(AccessStatus::Abstract.to_string(), "abstract only");
        assert_eq!(AccessStatus::Paywall.to_string(), "paywall");
        assert_eq!(AccessStatus::Unknown.to_string(), "unknown");
    }

    /// The legacy mapping the TUI and the CLI both used to re-derive: only
    /// real full text counts as `downloaded`, and the other three collapse to
    /// `paywall` (ADR-007 §1 "Legacy data").
    #[test]
    fn access_status_maps_to_the_legacy_download_status() {
        assert_eq!(
            AccessStatus::FullText.download_status(),
            DownloadStatus::Downloaded
        );
        for access in [
            AccessStatus::Abstract,
            AccessStatus::Paywall,
            AccessStatus::Unknown,
        ] {
            assert_eq!(
                access.download_status(),
                DownloadStatus::Paywall,
                "{access} collapses to the legacy paywall value"
            );
        }
    }
}
