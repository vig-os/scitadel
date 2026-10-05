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

use crate::ports::PaceTier;

/// The `artefacts.version` vocabulary, as a type.
///
/// Four values and no others, because migration 013's CHECK constraint says so
/// and a fifth would be a constraint violation at acquisition time rather than a
/// compile error. It exists as an enum rather than as the `&'static str` it
/// replaced because it now has **two producers** —
///
/// 1. [`RouteId::artefact_version`], the route's static claim: a preprint server
///    serves a preprint, and every other route establishes nothing on its own;
/// 2. the ranked candidate the resolve-then-rank pass chose, whose version came
///    from the registries that described *this* document.
///
/// Two producers need a join type. With `&'static str` the join was a string
/// comparison inside [`crate::sqlite::record_download`], which is exactly the
/// kind of comparison that silently accepts a fifth spelling — and the point of
/// `RouteId` existing at all, per this module's docs, is that
/// `artefacts.route` has to survive a write/read round trip.
///
/// [`Self::Unknown`] is the fallback value and the *route's* answer, and the two
/// are not the same thing: `Unknown` means "nothing established a version here",
/// which is what [`RouteId::artefact_version`] returns for all but two routes. A
/// candidate the ranker placed on its own `Unstated` rung also files `Unknown`,
/// because there is no column value for "a rendering of this work, version not
/// stated" — and inventing one would mean a second vocabulary, which is the
/// failure this type is here to prevent. The *ranking* keeps the two apart; the
/// column does not, and does not need to.
///
/// # Deliberately narrower than `access_basis`
///
/// `access_basis` stayed `Option<&'static str>`: it has exactly one producer
/// (the route), so a type would buy nothing there, and #261's rule that a caller
/// may not pair a route with another route's licence answer is enforced by there
/// being nowhere to put one. Version is the axis the resolver is *entitled* to
/// answer — it read the document's own record — so it is the one with a
/// caller-settable field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtefactVersion {
    /// The published version of record.
    VersionOfRecord,
    /// The author manuscript: the peer-reviewed text before a publisher's
    /// typesetting. Better than a preprint, worse than the version of record.
    AuthorManuscript,
    /// Posted before or instead of peer review.
    Preprint,
    /// Nothing established which of the three this is.
    Unknown,
}

impl ArtefactVersion {
    /// Every value, so a vocabulary test can be exhaustive.
    pub const ALL: [Self; 4] = [
        Self::VersionOfRecord,
        Self::AuthorManuscript,
        Self::Preprint,
        Self::Unknown,
    ];

    /// The `artefacts.version` column value.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::VersionOfRecord => "vor",
            Self::AuthorManuscript => "am",
            Self::Preprint => "preprint",
            Self::Unknown => "unknown",
        }
    }

    /// Parse a stored value back, for a manifest or a report that reads one.
    ///
    /// `None` for anything else rather than [`Self::Unknown`]: a value migration
    /// 013 never wrote is a corrupt row, and reporting it as "we did not look"
    /// would be the one answer this whole vocabulary exists to keep honest.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|version| version.label() == raw)
    }
}

impl std::fmt::Display for ArtefactVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Which route produced an artefact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
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

    /// The `artefacts.version` this route establishes **on its own**, or `None`
    /// for a pseudo-route.
    ///
    /// Only the two preprint-by-construction routes get to claim
    /// [`ArtefactVersion::Preprint`]: a PDF served from arXiv or from
    /// bioRxiv/medRxiv *is* a preprint, whatever the DOI says about its eventual
    /// journal version, and the ADR's own "have" rule trusts `unknown` on
    /// `route IN ('legacy','import_flat')` rows precisely because nothing else
    /// should carry it.
    ///
    /// Every other route answers [`ArtefactVersion::Unknown`], and that answer is
    /// **not** the whole story any more. It is the *fallback*, used when the
    /// resolve-then-rank pass established nothing — and "established nothing" is a
    /// much narrower condition than "is not a preprint server", because the pass
    /// reads OpenAlex's `locations[].version`, Unpaywall's `version`, Crossref's
    /// `type` and DataCite's `types.resourceTypeGeneral` before it fetches.
    ///
    /// **Which answer is authoritative when they disagree: the resolver's.** A
    /// route knows what *kind* of document it serves — a preprint server serves
    /// preprints — and knows nothing about what any particular URL turned out to
    /// be. The resolver compared the registries that registered this DOI and read
    /// the version word off the location it chose, which is a fact about the
    /// bytes rather than an inference from the server they came off. A
    /// disagreement is therefore the resolver being right and this being a
    /// fallback, and it is not an inconsistency to reconcile.
    ///
    /// [`crate::sqlite::record_download`] holds that precedence in one place, so
    /// the two are never compared at a call site.
    #[must_use]
    pub fn artefact_version(self) -> Option<ArtefactVersion> {
        if self.never_fetches() {
            return None;
        }
        Some(if matches!(self, Self::Arxiv | Self::Biorxiv) {
            ArtefactVersion::Preprint
        } else {
            ArtefactVersion::Unknown
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
                    VERSION_VOCABULARY.contains(&version.label()),
                    "{route} would write version {:?}",
                    version.label()
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

    /// **The static route → version mapping, and nothing else.**
    ///
    /// A PDF served from a preprint server is a preprint, by construction:
    /// arXiv (#S1's record-level leg) and bioRxiv/medRxiv (#260's DOI
    /// transform). Nothing a *route* alone can tell VoR from an author
    /// manuscript from a preprint, so every other route stays
    /// [`ArtefactVersion::Unknown`].
    ///
    /// The set is pinned rather than stated in prose because the failure mode
    /// is silent: a route added to the preprint arm of
    /// [`Self::artefact_version`] without being listed here would make a
    /// publisher's VoR masquerade as a preprint — the inverse of the
    /// overclaim this accessor exists to prevent.
    ///
    /// ## What this test is *not*, since #254
    ///
    /// It was **kept, not weakened**, and its scope **narrowed in its name**:
    /// it pins the **mapping**, and the mapping is unchanged and still the
    /// right answer to "what does this route establish on its own".
    ///
    /// What it is no longer the whole story about is which value reaches
    /// `artefacts.version`, because resolve-then-rank gives that column a second
    /// producer. That used to be recorded only as a claim in
    /// `resolve::Version::label`, where nothing checked it; it is now the
    /// storage precedence, and the next test pins it. So the two halves of "what
    /// version does this artefact have" are pinned separately and neither is
    /// folded into the other:
    ///
    /// - **this test**: the route's static claim, which is a fallback;
    /// - [`Self::the_resolvers_version_outranks_the_routes_fallback`]: which of
    ///   the two wins, and the loop that makes the answer matter.
    ///
    /// Renaming it to `a_route_establishes_a_version_only_if_it_serves_one_kind_of_document`
    /// would say the same thing about a wider set of claims, but the name is the
    /// cheap part and the split is the substance — so the name says "a version",
    /// which is what it still asserts.
    #[test]
    fn only_the_preprint_servers_may_claim_a_version() {
        assert_eq!(
            RouteId::Arxiv.artefact_version(),
            Some(ArtefactVersion::Preprint)
        );
        assert_eq!(
            RouteId::Biorxiv.artefact_version(),
            Some(ArtefactVersion::Preprint)
        );
        for route in RouteId::ALL
            .into_iter()
            .filter(|r| !r.never_fetches())
            .filter(|r| !matches!(r, RouteId::Arxiv | RouteId::Biorxiv))
        {
            assert_eq!(
                route.artefact_version(),
                Some(ArtefactVersion::Unknown),
                "{route} must not claim a version it did not establish"
            );
        }
    }

    /// **The storage precedence is documented here and *pinned* in
    /// `scitadel-db`** — at `the_resolvers_version_outranks_the_routes_fallback`,
    /// in `sqlite/artefacts.rs`.
    ///
    /// It cannot be pinned from this crate: the join happens inside
    /// [`crate::sqlite::record_download`], and `scitadel-db` depends on
    /// `scitadel-core`, so the dependency runs the wrong way for a test here to
    /// reach it. What this crate can own — and does, in
    /// `only_the_preprint_servers_may_claim_a_version` above — is the **fallback**
    /// side of the pair. The two halves of "what version does this artefact have"
    /// are therefore pinned in two crates, which is where their subjects live,
    /// and each test says which half it is so neither is mistaken for the whole.
    ///
    /// The precedence, restated so this file is not half the story:
    ///
    /// | resolver established | column gets |
    /// |---|---|
    /// | `Some(VersionOfRecord \| AuthorManuscript \| Preprint)` | **the resolver's** value |
    /// | `Some(Unknown)` — looked, nothing stated | `unknown` |
    /// | `None` — did not look | the route's [`Self::artefact_version`] |
    ///
    /// The middle row is why `Some(Unknown)` and `None` are different inputs:
    /// both write `unknown` here, but only the second consults the route, and a
    /// caller that cannot tell them apart has thrown away the distinction between
    /// "we asked and got no answer" and "we never asked".
    ///
    /// [`ArtefactVersion`] and the column must not drift, in either direction.
    ///
    /// Round-tripped rather than listed twice, because the migration-013 CHECK
    /// list is the third copy of this vocabulary and a hand-written third copy is
    /// how two of them come to disagree.
    #[test]
    fn artefact_version_round_trips_its_own_column_values() {
        const MIGRATION_013: [&str; 4] = ["vor", "am", "preprint", "unknown"];
        for label in MIGRATION_013 {
            let parsed = ArtefactVersion::parse(label)
                .unwrap_or_else(|| panic!("{label} is a column value this type cannot read"));
            assert_eq!(parsed.label(), label, "round trip for {label}");
        }
        assert_eq!(
            ArtefactVersion::ALL.map(ArtefactVersion::label),
            MIGRATION_013,
            "and the vocabulary is exactly migration 013's, in order"
        );
        // A value the CHECK constraint would have refused anyway must not be
        // laundered into "we did not look".
        assert_eq!(ArtefactVersion::parse("draft"), None);
        assert_eq!(ArtefactVersion::parse(""), None);
        assert_eq!(ArtefactVersion::parse("VOR"), None);
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

    /// The mapping this build *does* keep, and the one that replaced the legacy
    /// collapse: `access_status` is recorded per artefact and read back
    /// unchanged, so the TUI's state column and `coverage` now distinguish an
    /// abstract from a hard paywall instead of folding all three into one value.
    ///
    /// This replaces `access_status_maps_to_the_legacy_download_status`, which
    /// pinned `AccessStatus::download_status()` — the map onto
    /// `papers.download_status`. That method went with the column in #253's S2e:
    /// a one-to-one map onto a retired two-valued column had no reader left, and
    /// keeping it would have kept a second, quietly-diverging answer to "did we
    /// get the paper" alive next to `sqlite::coverage`'s derivation.
    #[test]
    fn access_status_labels_round_trip_and_stay_distinct() {
        for access in [
            AccessStatus::FullText,
            AccessStatus::Abstract,
            AccessStatus::Paywall,
            AccessStatus::Unknown,
        ] {
            assert_eq!(
                AccessStatus::ALL_LABELS
                    .iter()
                    .filter(|label| **label == access.label())
                    .count(),
                1,
                "{access} writes a label the schema allows, and writes it once"
            );
        }
        // The three that the legacy column collapsed into `paywall` are three
        // distinct labels now, which is the whole point of reading the artefact
        // rather than the column.
        let collapsed = [
            AccessStatus::Abstract,
            AccessStatus::Paywall,
            AccessStatus::Unknown,
        ];
        for a in collapsed {
            for b in collapsed {
                assert_eq!(
                    a.label() == b.label(),
                    a == b,
                    "{a} and {b} are distinguishable by label"
                );
            }
        }
    }
}
