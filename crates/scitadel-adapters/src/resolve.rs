//! ADR-007 §3's central design claim, made executable:
//!
//! > **Resolve, then rank.** One metadata pass per work, touching no publisher
//! > hosts, gathers candidates. Candidates are ranked **OA VoR > AM > preprint**,
//! > then by licence strength.
//!
//! # What this replaces
//!
//! Before this, `download.rs`'s ladder tried routes in a fixed order and took
//! the first that answered. A bioRxiv route that answered first beat an OpenAlex
//! route naming the version of record that would have answered one hop later —
//! which is exactly backwards, and it is why a run could file a preprint for a
//! work whose published article was open access the whole time.
//!
//! Three rules replace it, and each is a separate type here rather than a
//! detail of a sort:
//!
//! 1. **Collect every candidate first**, from a metadata pass that touches no
//!    publisher host ([`ResolveRequest`] → [`Resolution`]). Nothing is fetched
//!    while gathering.
//! 2. **Rank them**, version first and licence second ([`Rank`]). An
//!    unrankable candidate is *not* given a position
//!    ([`UnrankedCandidate`]).
//! 3. **Fetch exactly one** — the top-ranked candidate — and if it fails,
//!    record why ([`RankedCandidate::fetch`] is reached through
//!    [`Resolution::chosen`], never through a loop over candidates).
//!
//! # Version strictly dominates licence, and that is a decision
//!
//! [`Rank`]'s `Ord` impl states it in the type: version is compared first and
//! licence only breaks ties within one version class. The alternative —
//! interleaving them into one score — is rejected because a licence strength is
//! a statement about *permission to reuse* while a version is a statement about
//! *which document we would be holding*:
//!
//! - A CC-BY **preprint** is more permissively licensed than the publisher's
//!   version of record, and interleaving can rank it first. Then `acquire`
//!   satisfies a `wanted_version = 'vor'` want with a preprint, and
//!   `coverage`'s derivation reads the want as closed. The user's library claims
//!   to hold the article of record and holds a preprint.
//! - There is no arithmetic that fixes this, because the two axes are not the
//!   same kind of quantity. Dominance is the only ordering under which
//!   "version of record first" is guaranteed rather than merely likely.
//!
//! The cost is real and is worth naming: a CC-BY preprint outranks an
//! all-rights-reserved author manuscript, and for a user who can only *read*
//! (not reuse) the version of record that is the wrong answer. That is a
//! licence-filtering feature, not a ranking bug — `Rank` is a total order on a
//! two-key tuple precisely so a caller can add a filter without touching it.
//!
//! # The version vocabulary is ADR's, not this module's
//!
//! [`Version`] is `vor | am | preprint`, exactly the three values
//! `artefacts.version` admits besides `unknown` (migration 013), and it is
//! derived from the registry signals in [`crate::registry`] rather than
//! re-derived here. `RouteId::artefact_version` still answers `unknown` for
//! every non-preprint route and that has not changed: a route cannot know what
//! version the bytes turned out to be, and only the *comparison* of the
//! registries can. What changes is that a work whose bytes came from a
//! `Vor`-ranked candidate now records `vor` — because the metadata pass
//! established it, not because the route guessed.
//!
//! # The three-stage origin chain, and why the fallback is recorded
//!
//! [`VersionSource`] is the provenance of a [`Version`] claim and it is
//! exhaustive over four cases, in strict preference order:
//!
//! | order | source | what it is |
//! |---|---|---|
//! | 1 | [`VersionSource::Registry`] | the registry's own `type` / `resourceTypeGeneral` |
//! | 2 | [`VersionSource::Route`] | the route serves preprints by construction |
//! | 3 | [`VersionSource::DoiPrefix`] | the DOI prefix, the #260 fallback |
//! | 4 | [`VersionSource::None`] | nothing established it — see below |
//!
//! The registry signal is preferred over the prefix inference *because it is a
//! statement by the publisher about its own DOI*, where a `10.1101` prefix is a
//! statement about who registered it. A bioRxiv DOI that Crossref types
//! `journal-article` is a DOI whose journal version of record was registered
//! under the preprint prefix, and preferring the prefix there inverts the very
//! ranking this module exists to fix.
//!
//! # What a user sees for an unrankable candidate
//!
//! Not a position. [`Resolution::unranked`] is a first-class list, every entry
//! carrying [`UnrankedCandidate::why`] — "no registry typed this and the DOI
//! prefix `99999` implies no version" — and `acquire --dry-run` prints it
//! under a heading that says these were **not** fetched. A candidate with no
//! version signal is not a candidate with a bad version, and sorting it into
//! last place by default would make "we guessed" indistinguishable from "the
//! registries all said preprint".

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use scitadel_core::models::RouteId;

use crate::registry::{LicenceOffer, Registry, RegistryType};

/// ADR-007 §3's version vocabulary, in the migration-013 spelling.
///
/// `PartialOrd`/`Ord` derive **rank order**: `VersionOfRecord < AuthorManuscript
/// < Preprint`, so `Version::ALL[0]` is the best and a `min()` is the answer to
/// "which version do we want". The derive is the ordering rule; it is not a
/// convenience, and [`Rank`] below depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Version {
    /// The published version of record. ADR-007 §3's top rank.
    VersionOfRecord,
    /// The author manuscript: the peer-reviewed text before a publisher's
    /// typesetting. Better than a preprint, worse than the version of record.
    AuthorManuscript,
    /// Posted before or instead of peer review.
    Preprint,
    /// A rendering of this work whose version nothing stated.
    ///
    /// A **rung**, not a hole, and the distinction is the module's whole
    /// subject. A publisher's landing page is a real candidate with a real
    /// licence basis (`subscription_read`) and no version at all — ADR-007 §3
    /// step 6 fetches it "for OA works only, after TDM", which is a statement
    /// about *when* it may be taken rather than a claim that it is a version of
    /// anything. Ranking it last is how that sentence is honoured.
    ///
    /// It is *not* the same as a candidate with no version at all, which is
    /// [`UnrankedCandidate`]: that one is a location nothing vouched for.
    Unstated,
}

impl Version {
    /// Best-first, so `ALL[0]` is the version we want most and a caller can
    /// assert the ladder's order without re-deriving it.
    pub const ALL: [Self; 4] = [
        Self::VersionOfRecord,
        Self::AuthorManuscript,
        Self::Preprint,
        Self::Unstated,
    ];

    /// The `artefacts.version` column value.
    ///
    /// One of the three values migration 013's CHECK admits besides `unknown`,
    /// and the same strings `RouteId::artefact_version` writes — so a ranked
    /// candidate and a route's own claim cannot disagree about spelling.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::VersionOfRecord => "vor",
            Self::AuthorManuscript => "am",
            Self::Preprint => "preprint",
            // **Not** `unknown`. `unknown` is migration 013's value for "no
            // route established a version", which is what
            // `RouteId::artefact_version` still writes; `unstated` is what a
            // location named by a metadata pass without a version word records,
            // and writing `unknown` for it would collapse "a place was named and
            // it did not say" into "we never looked".
            Self::Unstated => "unstated",
        }
    }
}

impl Version {
    /// The `artefacts.version` column value this ranking rung writes, or `None`
    /// to fall back to the route's static claim.
    ///
    /// The `Unstated` rung maps to `None` **on purpose**, and it is the one
    /// judgement in this conversion. `Unstated` means "a rendering of this work
    /// whose version nothing stated", and `artefacts.version` has no word for
    /// that: `unknown` means "nothing established a version", which is what the
    /// route's fallback also says. Writing `Some(Unknown)` from the `Unstated`
    /// rung would claim the resolver *looked* and came back empty-handed, when
    /// what happened is that it looked and found a rendering it cannot place —
    /// and those are different facts. So the rung falls through to
    /// [`RouteId::artefact_version`], which writes the same column value for the
    /// same end result and attributes it honestly.
    ///
    /// The three stated versions are written verbatim, and that is the whole of
    /// the storage fix: a version of record fetched through a route whose static
    /// claim is `unknown` files as `vor`, which is what makes
    /// `version_satisfies` close the want.
    #[must_use]
    pub fn as_artefact_version(self) -> Option<scitadel_core::models::ArtefactVersion> {
        use scitadel_core::models::ArtefactVersion as Stored;
        match self {
            Self::VersionOfRecord => Some(Stored::VersionOfRecord),
            Self::AuthorManuscript => Some(Stored::AuthorManuscript),
            Self::Preprint => Some(Stored::Preprint),
            Self::Unstated => None,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionOfRecord => f.write_str("version of record"),
            Self::AuthorManuscript => f.write_str("author manuscript"),
            Self::Preprint => f.write_str("preprint"),
            Self::Unstated => f.write_str("unstated version"),
        }
    }
}

/// Where a [`Version`] claim came from, in strict preference order.
///
/// The chain is the point, and it is why [`VersionSource::DoiPrefix`] is a
/// separate variant rather than a comment: the #260 preprint leg inferred
/// "preprint" from the DOI prefix, which is right for a `10.1101` DOI and wrong
/// for a `10.1101` DOI the publisher has since typed as a journal article. A
/// ranker that cannot say *which* of the two produced a claim cannot be tested
/// on the difference, and the difference is the whole improvement.
///
/// # Why `LocationVersion` beats `Registry`
///
/// A registry says two different things about a work: what kind of thing the
/// *record* is (`message.type`, `types.resourceTypeGeneral`) and what version
/// a *particular location* serves (`locations[].version`,
/// `best_oa_location.version`, `link[].content-version`). The second is about
/// the bytes we would actually fetch, so it is strictly stronger — a work with
/// a publisher VoR at one location and a preprint at another has one
/// record-level type and two locations, and only the per-location word can tell
/// them apart. Ranking on the record-level type for both would rank them
/// identically and re-introduce, inside the ranking, exactly the bug the
/// ranking exists to fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VersionSource {
    /// A location's own `version` word — OpenAlex's `publishedVersion` /
    /// `acceptedVersion` / `submittedVersion`, Unpaywall's the same three, or a
    /// Crossref `link[].content-version`. The strongest signal available: it is
    /// the registry's statement about *this* URL.
    LocationVersion {
        registry: Registry,
        /// The verbatim word.
        word: String,
    },
    /// A registry typed the record. `registry` and `signal` are carried so a
    /// plan line can say *which* registry, and so the fallback test has
    /// something to assert on.
    Registry {
        registry: Registry,
        /// The verbatim value: `posted-content`, `Preprint`, `journal-article`.
        signal: String,
    },
    /// The route serves this version by construction — arXiv and bioRxiv serve
    /// preprints whatever the DOI says, and OSTI's `purl` serves a report's own
    /// authoritative publication. `RouteId::artefact_version` and
    /// `RouteId::access_basis` already state both facts, so this reads them
    /// rather than re-deriving them.
    Route { route: RouteId },
    /// The DOI's registrant prefix implied it. The #260 fallback, used **only**
    /// when every registry was silent, and recorded so the fallback is visible
    /// rather than silent.
    DoiPrefix { prefix: String },
    /// A place named this location but did not say which version it serves.
    ///
    /// Distinct from [`Self::None`] because it is a **positive** statement
    /// about the location — "here is a rendering of this work; its version is
    /// not stated" — and distinct from a registry's silence, which is a
    /// statement about the registry. Both give [`Version::Unstated`], which is
    /// the ladder's bottom rung.
    Unstated { because: String },
    /// Nothing established a version, and nothing vouched for the location at
    /// all: no location word, no record type, no DOI prefix, and no source that
    /// even asserts "this is a rendering of the work".
    ///
    /// Not a default and not `unknown` as a guess: this is the shape
    /// [`UnrankedCandidate`] is made of.
    None,
}

/// Is this a URL a candidate may carry?
///
/// **Absolute `http`/`https`, and nothing else.** Three reasons, and the first
/// is the one that made this a guard rather than a test:
///
/// 1. An index's `url_for_pdf` is an arbitrary string from an arbitrary server,
///    and a *relative* one is not an error the fetch path can report — it is a
///    `Url::parse` failure at the moment of fetching, by which time the candidate
///    has already been **ranked first**. That is the shape the
///    `unpaywall_json("{}")` fixture hit: a location that read as "no OA
///    location" but was really "a candidate at the relative path `{}`", harmless
///    while a miss fell through to the next leg and fatal as the ranked choice.
/// 2. A candidate URL is recorded verbatim on `artefacts.source_url`, so a
///    malformed one is also a malformed audit row.
/// 3. `data:` and `file:` URLs parse and would be *fetched* — a
///    [`scitadel_http::PacedClient`] against a `file://` path is a local file
///    read wearing a network budget, which is a thing no ranking rule should be
///    able to produce.
///
/// This is deliberately a **filter**, not a repair: a URL the resolver cannot
/// read is dropped, and the candidate's absence shows up in the plan as a
/// missing location rather than as a fabricated one.
#[must_use]
pub fn is_fetchable_url(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).is_ok_and(|parsed| {
        matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some()
    })
}

impl VersionSource {
    /// One clause, for the reason string a human reads.
    fn why(&self) -> String {
        match self {
            Self::LocationVersion { registry, word } => {
                format!("{registry} says this location is `{word}`")
            }
            Self::Registry { registry, signal } => {
                format!("{registry} types it `{signal}`")
            }
            Self::Route { route } => {
                format!("{route} serves this version by construction")
            }
            Self::DoiPrefix { prefix } => {
                format!("no registry typed it; the DOI prefix `{prefix}` implies a preprint")
            }
            Self::Unstated { because } => {
                format!("{because}, so its version is unstated")
            }
            Self::None => "no registry typed it, the DOI prefix implies no version, and \
                          nothing vouched for this location"
                .to_string(),
        }
    }
}

/// Read an OpenAlex/Unpaywall location `version` word into a [`Version`].
///
/// The three words are the *same three words in both indexes* — measured, and
/// identical because Unpaywall's location shape is OpenAlex's — so this is one
/// mapping with two callers rather than two near-identical tables. An
/// unrecognised word is `None`, which sends the caller down the chain; it is
/// never `unknown`-as-a-guess.
#[must_use]
pub fn version_from_location_word(
    word: &str,
    registry: Registry,
) -> Option<(Version, VersionSource)> {
    let version = match word.trim().to_ascii_lowercase().as_str() {
        "publishedversion" | "vor" | "versionofrecord" => Version::VersionOfRecord,
        "acceptedversion" | "am" | "authormanuscript" => Version::AuthorManuscript,
        "submittedversion" | "preprint" => Version::Preprint,
        _ => return None,
    };
    Some((
        version,
        VersionSource::LocationVersion {
            registry,
            word: word.trim().to_string(),
        },
    ))
}

/// A Crossref `link[].content-version`, in the same vocabulary.
///
/// Read through [`version_from_location_word`] rather than a second table: the
/// word is the same word, and two tables for one vocabulary is the kind of thing
/// that diverges.
#[must_use]
pub fn version_from_content_version(word: &str) -> Option<(Version, VersionSource)> {
    version_from_location_word(word, Registry::Crossref)
}

/// The registrant prefix a DOI would be inferred from, or `None`.
///
/// **The fallback, and only ever the fallback.** It exists because a `10.1101`
/// or `10.48550` DOI is a posting's own identifier no matter which registry is
/// silent, and #260's leg inferred it from the prefix — but a publisher that
/// has registered a journal version under that prefix outranks the inference,
/// which is why [`version_from_registry`] is consulted first and the fact that
/// the fallback was used is recorded.
///
/// No prefix implies a version of record or an author manuscript: a
/// `10.1038` DOI is as likely to be a paywalled article as an open one, and
/// "the registrant is Nature" says nothing about which rendering of the work a
/// given URL serves. So the function returns `Preprint` for the two prefixes
/// [`crate::preprint`] has a **verified** transform for and `None` for
/// everything else — including `10.26434`, whose preprint servers the module
/// declines to serve.
#[must_use]
pub fn version_from_doi_prefix(doi: &str) -> Option<(Version, VersionSource)> {
    let registrant = doi.trim().strip_prefix("10.")?.split('/').next()?;
    let prefix = registrant.trim();
    if !crate::preprint::preprint_candidates(doi).is_empty() {
        return Some((
            Version::Preprint,
            VersionSource::DoiPrefix {
                prefix: prefix.to_string(),
            },
        ));
    }
    None
}

/// ADR-007 §3's ordering, as one comparable value.
///
/// Two fields and a derived `Ord`, which is where **version strictly dominates
/// licence** lives: the version is compared first and the licence is read only
/// when the versions are equal. [`ranked_order_is_documented_and_pinned`] pins
/// it against hand-written expectations so the dominance cannot be changed by
/// someone reordering the fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Rank {
    pub version: Version,
    pub licence: LicenceStrength,
}

/// ADR-007 §3's licence strength, best-first, matching [`Rank`]'s `Ord`.
///
/// A four-state ladder rather than a bool, because the two ends matter
/// differently: `OpenLicence` is a grant we may reuse under, `FreeToRead` is
/// only permission to read, and collapsing them into `is_oa` would make
/// `access_basis = 'oa_license'` a claim about a preprint server that states
/// permission to reuse anything — which is what
/// `RouteId::access_basis` is careful *not* to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenceStrength {
    /// An allow-listed Creative Commons URL that is in force today for the
    /// candidate's content-version. `crate::registry` decides, from the
    /// registry's own `license[]`.
    OpenLicence,
    /// Free to read with no reuse licence established — a preprint server, an
    /// OA repository that publishes no terms, OSTI's public-domain neighbour.
    FreeToRead,
    /// Readable under whatever entitlement the caller's IP or session had. ADR
    /// §5's `subscription_read`.
    SubscriptionRead,
    /// Nothing established a basis. Not a default: see [`Rank`].
    Unknown,
}

impl LicenceStrength {
    /// Best-first, so `ALL[0]` is the strongest.
    pub const ALL: [Self; 4] = [
        Self::OpenLicence,
        Self::FreeToRead,
        Self::SubscriptionRead,
        Self::Unknown,
    ];

    /// The `artefacts.access_basis` column value, where one exists.
    ///
    /// `Unknown` has none, and that is not an oversight: `access_basis` is
    /// `NOT NULL` and `RouteId::access_basis` returns `Option` for exactly this
    /// reason — "we did not look" must not be written as `manual`.
    #[must_use]
    pub fn access_basis(self) -> Option<&'static str> {
        match self {
            Self::OpenLicence | Self::FreeToRead => Some("oa_license"),
            Self::SubscriptionRead => Some("subscription_read"),
            Self::Unknown => None,
        }
    }

    fn why(self) -> &'static str {
        match self {
            Self::OpenLicence => {
                "an allow-listed Creative Commons licence is in force for this version"
            }
            Self::FreeToRead => "free to read, with no reuse licence established",
            Self::SubscriptionRead => "readable under the caller's existing entitlement",
            Self::Unknown => "no licence and no route basis established",
        }
    }
}

/// One place the full text might be, with everything the ranker needs to place
/// it — and nothing that had to be fetched to find out.
///
/// `url` is a **string on purpose**. Fetching it is the one thing this module
/// never does while gathering, and the type that carries a URL is not the type
/// that performs a request, so "the metadata pass touches no publisher host" is
/// a property of the code's shape rather than of a reviewer's diligence. The
/// fetch happens through [`crate::download::PaperDownloader`], once, on the
/// candidate [`Resolution::chosen`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// The route that would serve these bytes, and therefore the
    /// `artefacts.route` the fetch records.
    pub route: RouteId,
    pub url: String,
    /// Which registry named this location, or the route transform that derived
    /// it. Displayed so a plan line says *where the claim came from*.
    pub named_by: String,
    /// The version, **or** the absence of one.
    pub version: Option<Version>,
    /// Why [`Self::version`] is what it is.
    pub version_source: VersionSource,
    pub licence: LicenceStrength,
}

impl Candidate {
    /// The four clauses a plan line prints, in one string.
    #[must_use]
    pub fn why(&self) -> String {
        format!(
            "{} via {} (named by {}) — version {} [{}]; {}",
            self.url,
            self.route.label(),
            self.named_by,
            self.version
                .map_or_else(|| "unrankable".to_string(), |version| version.to_string()),
            self.version_source.why(),
            self.licence.why(),
        )
    }
}

/// A candidate with a position in ADR-007 §3's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RankedCandidate {
    /// Where this candidate landed, 1-based, for a plan line that says "1 of 4".
    pub position: usize,
    pub candidate: Candidate,
    /// The version and licence this placement was decided by, kept beside the
    /// candidate so a reader can see *which* comparison won without re-running
    /// the ranker.
    pub rank: Rank,
}

/// A candidate with no position, and the reason.
///
/// Separate from [`RankedCandidate`] rather than a `RankedCandidate` with a
/// sentinel rank, because the whole claim is that an unrankable candidate is a
/// **different kind of thing**: it was found, and nothing established what it
/// is. A sentinel rank would put it in the order with a fabricated position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnrankedCandidate {
    pub candidate: Candidate,
    /// The sentence a user reads, and the one a dry run prints verbatim.
    pub why: String,
}

/// Everything the metadata pass established for one work.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Resolution {
    /// Ranked best-first. Never contains an unrankable candidate.
    pub ranked: Vec<RankedCandidate>,
    /// Candidates found but not placed. Reported, never fetched.
    pub unranked: Vec<UnrankedCandidate>,
    /// Every registry consulted, in ADR-007 §3's chain order, with each hop's
    /// outcome — the same [`crate::identity_chain::HopOutcome`] vocabulary, so
    /// `NotRegistered` and `Unreadable` keep the meanings #292 gave them.
    pub consulted: Vec<ConsultedRegistry>,
    /// The hosts the pass actually contacted, deduplicated and sorted.
    ///
    /// The acceptance criterion as a **value** rather than as a test-only
    /// assertion: `the_metadata_pass_touches_no_publisher_host` checks this
    /// against [`METADATA_BUCKETS`], and a caller can print it.
    pub hosts_contacted: BTreeSet<String>,
    /// Candidates named by a source other than the four registries: a DOI
    /// transform, the record's own URL, OSTI's `purl`. Carried so a plan can
    /// show a route that no registry vouched for.
    pub derived_routes: Vec<RouteId>,
    /// What the pass established about the work's **identity**, from the bodies
    /// it has already read.
    ///
    /// A `ChainOutcome` rather than a new type, and the reuse is the point: the
    /// pass asked OpenAlex, Crossref and DataCite, so asking them again to
    /// corroborate the title would spend the same three hops twice. ADR-007 §3's
    /// two identity checks are unchanged — the *decision* is still
    /// `identity::verify`'s, and the pre-fetch answer is still the first
    /// registry in [`crate::identity_chain::CHAIN`] that named the work.
    ///
    /// The one difference from a chain walk, and it is a real one: the chain
    /// stops at the first answer, so its later hops are `NotConsulted`, whereas
    /// this pass asks all four because it is after version and licence facts
    /// too. Every hop here therefore reports what actually happened, and
    /// `NotConsulted` never appears — which is why a plan can show four hops and
    /// a single identity verdict without the two disagreeing.
    pub identity: crate::identity_chain::ChainOutcome,
}

/// One registry the metadata pass asked, and what it said.
///
/// The two outcomes that matter are `NotRegistered` (a 404 — *continue*, the
/// preprint/repository case) and `Unreadable` (a 5xx or a transport failure —
/// *we could not find out*, which must not be reported as "this registry has
/// nothing"). That distinction is [`crate::identity_chain`]'s, this slice's
/// requirement, and it is reused rather than reinvented because the two answers
/// are indistinguishable from a status code alone and only one of them is
/// knowledge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsultedRegistry {
    pub registry: Registry,
    pub outcome: crate::identity_chain::HopOutcome,
}

/// The buckets a metadata pass is allowed to spend from.
///
/// ADR-007 §3 lists them: "It may hit OpenAlex, Crossref, DataCite,
/// Unpaywall." A pass that spent a permit from any other bucket would have
/// touched a host that serves articles, and the ranked plan would depend on a
/// publisher's response time — the failure ADR-007 §3's "resolve, then rank"
/// exists to remove.
pub const METADATA_BUCKETS: [&str; 4] = ["openalex", "crossref", "datacite", "unpaywall"];

/// Rank candidates into ADR-007 §3's order.
///
/// Two rules, in this order and no other:
///
/// 1. **Version strictly dominates licence** ([`Rank`]'s `Ord`).
/// 2. Candidates with no version are **not ranked at all** — they come back in
///    [`Resolution::unranked`] with a reason, and never appear in
///    [`Resolution::ranked`].
///
/// The second rule is why the signature returns a [`Resolution`] rather than a
/// `Vec`. A function that returned `Vec<Candidate>` would have to either drop
/// the unrankable ones — losing the fact that a location exists — or invent a
/// position for them.
///
/// The comparator is [`rank_by`], a named function rather than a closure at the
/// call site: the dominance of version is the claim under test, and
/// `ranked_order_is_documented_and_pinned` reads it.
#[must_use]
pub fn rank_candidates(mut candidates: Vec<Candidate>) -> Resolution {
    let mut ranked: Vec<Candidate> = Vec::new();
    let mut unranked: Vec<UnrankedCandidate> = Vec::new();
    for candidate in candidates.drain(..) {
        // The reason is read **before** the candidate moves, so the two branches
        // can both take ownership and the split is a pair of pushes rather than a
        // match on a moved value.
        if candidate.version.is_some() {
            ranked.push(candidate);
            continue;
        }
        let why = candidate.version_source.why();
        unranked.push(UnrankedCandidate { candidate, why });
    }
    // Stable, so two candidates that compare equal keep the order the metadata
    // pass produced them in — ADR-007 §3's own chain order. The tiebreak is
    // therefore the pass's order and not `sort`'s arbitrary behaviour, which
    // would make "the location the first registry named" the ranking rule by
    // accident.
    // `sort_by_key` rather than `sort_by`: `Rank` is `Copy`, so the closure
    // cannot borrow, and the *stability* is what carries the ADR fetch order as
    // the tiebreak — `sort_by_key` is stable exactly as `sort_by` is, so nothing
    // about the tiebreak changes.
    ranked.sort_by_key(rank_by);

    Resolution {
        ranked: ranked
            .into_iter()
            .enumerate()
            .map(|(at, candidate)| RankedCandidate {
                position: at + 1,
                rank: rank_by(&candidate),
                candidate,
            })
            .collect(),
        unranked,
        consulted: Vec::new(),
        hosts_contacted: BTreeSet::new(),
        derived_routes: Vec::new(),
        identity: crate::identity_chain::ChainOutcome {
            resolved: None,
            hops: Vec::new(),
        },
    }
}

/// The key [`rank_candidates`] sorts on.
///
/// A named function rather than an inline closure because the *ordering* is a
/// claim about the world — version strictly dominates licence — and a claim
/// with no name is a claim nobody reviews when it changes.
fn rank_by(candidate: &Candidate) -> Rank {
    Rank {
        version: candidate
            .version
            .expect("rank_candidates only sorts candidates that carry a version"),
        licence: candidate.licence,
    }
}

impl Resolution {
    /// The one candidate to fetch, or `None` when nothing could be ranked.
    ///
    /// The single accessor, and it returns one candidate rather than an
    /// iterator. A caller that wants to "try the next one if this fails" has to
    /// call it again after re-ranking with the failure recorded, because the
    /// whole point is that a failure is *data about the chosen route*, not a
    /// cue to try routes in the order they happened to answer.
    #[must_use]
    pub fn chosen(&self) -> Option<&RankedCandidate> {
        self.ranked.first()
    }

    /// Nothing was resolved, and nothing is claimed.
    ///
    /// For the paths that end before the pass runs — an output directory that
    /// could not be created — so a caller carrying a [`Resolution`] always has a
    /// value that says what it knows. An empty plan is an answer here, not a
    /// default: it means "no candidate was resolved", which is exactly what
    /// happened.
    #[must_use]
    pub fn empty() -> Self {
        rank_candidates(Vec::new())
    }

    /// Every candidate the pass found, ranked or not.
    ///
    /// One iterator rather than two call sites that chain the lists, because a
    /// caller asking "did this route produce anything" must not have to remember
    /// to look in both — and looking in only the ranked one is exactly how an
    /// unrankable candidate would come to be invisible to a report.
    pub fn candidates(&self) -> impl Iterator<Item = &Candidate> {
        self.ranked
            .iter()
            .map(|entry| &entry.candidate)
            .chain(self.unranked.iter().map(|entry| &entry.candidate))
    }

    /// The plan as a human reads it, one candidate per line.
    ///
    /// `acquire --dry-run` prints this, so the ranking is inspectable before
    /// anything is fetched — which is also the cheapest possible test of the
    /// ordering, since a dry run puts no publisher on the wire.
    #[must_use]
    pub fn plan_lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .ranked
            .iter()
            .map(|entry| {
                format!(
                    "  {}. [{} > {}] {}",
                    entry.position,
                    entry.rank.version,
                    entry.rank.licence_phrase(),
                    entry.candidate.why(),
                )
            })
            .collect();
        for entry in &self.unranked {
            out.push(format!(
                "  – not ranked: {} — {}",
                entry.candidate.why(),
                entry.why,
            ));
        }
        out
    }
}

impl Rank {
    /// The licence, in the words a plan line uses.
    fn licence_phrase(self) -> String {
        self.licence.why().to_string()
    }
}

/// Read a registry's version signal into a candidate's [`VersionSource`].
///
/// Returns `None` when the registry typed nothing usable, which is the
/// condition under which the caller is *allowed* to fall back to the DOI
/// prefix. The fallback is [`VersionSource::DoiPrefix`] and it is recorded, so
/// `a_registry_type_signal_is_preferred_over_doi_prefix_inference` and
/// `the_fallback_is_used_and_recorded_when_the_registry_is_silent` are two
/// tests of one rule rather than two descriptions of it.
#[must_use]
pub fn version_from_registry(
    registry: Registry,
    classified: RegistryType,
    word: &str,
) -> Option<(Version, VersionSource)> {
    match classified {
        RegistryType::Preprint => Some((
            Version::Preprint,
            VersionSource::Registry {
                registry,
                signal: word.to_string(),
            },
        )),
        RegistryType::VersionOfRecord => Some((
            Version::VersionOfRecord,
            VersionSource::Registry {
                registry,
                signal: word.to_string(),
            },
        )),
        // `NotAnArticle` and `Unrecognised` are both "this registry cannot place
        // the record", so both return `None` and both hand the decision to the
        // next source. Collapsing them here would lose the distinction the
        // registry module draws, so the caller gets `None` and can say *why*
        // from the registry's own words.
        RegistryType::NotAnArticle | RegistryType::Unrecognised => None,
    }
}

/// The licence strength for a candidate whose licences were evaluated.
///
/// `offers` is whatever the registries offered, already passed through
/// [`crate::registry::LicenceOffer::counts_for`] for this candidate's version —
/// a `vor`-only grant does not license a preprint. An empty slice is
/// [`LicenceStrength::Unknown`], which is a real answer and not a default: it
/// is what makes an unrankable candidate unrankable.
#[must_use]
pub fn licence_strength(
    offers: &[LicenceOffer],
    version: Option<Version>,
    route: RouteId,
    floor: LicenceFloor,
) -> LicenceStrength {
    if !offers.is_empty() {
        return LicenceStrength::OpenLicence;
    }
    if !matches!(floor, LicenceFloor::RouteBasis) {
        // The naming index asserted nothing about this location either. This is
        // the branch that makes an OpenAlex `locations[]` entry unrankable
        // rather than quietly OA — see [`LicenceFloor`].
        return LicenceStrength::Unknown;
    }
    // No registry offered a licence. What the *route* itself establishes is a
    // weaker but genuine answer, and it is the one `RouteId::access_basis`
    // already records on the artefact: a preprint server serves preprints free
    // to read by construction, and OSTI's work is US-government public domain.
    if let Some(basis) = route.access_basis() {
        return match basis {
            "oa_license" => LicenceStrength::FreeToRead,
            "subscription_read" => LicenceStrength::SubscriptionRead,
            // `tdm_licence` and `manual` are neither OA nor subscription. A
            // TDM route's terms are the publisher's, and reading them through
            // this ladder would claim an OA licence for a sanctioned TDM
            // fetch; `manual` establishes nothing at all.
            _ => LicenceStrength::Unknown,
        };
    }
    let _ = version;
    LicenceStrength::Unknown
}

/// Where a candidate's licence falls back when no registry offered a grant.
///
/// Two floors, and the difference is the overclaim #261 exists to stop.
/// `RouteId::OpenAlex::access_basis()` is `oa_license`, and that is a true
/// statement about **OpenAlex's best OA location** — the one route field, for
/// a route that is an *index*, can honestly be about. Applying it to an
/// arbitrary `locations[]` entry would claim a repository copy is free to read
/// because OpenAlex mentioned it, and OpenAlex's `locations[]` is *every*
/// location it indexes, open or not.
///
/// So the floor is per-candidate, not per-route, and it is a field a test can
/// set differently for two candidates on the same route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenceFloor {
    /// The route's own `access_basis` applies. Right for a route that serves a
    /// kind of document — a preprint server, OSTI, the publisher's own page —
    /// and for an index's *own* assertion that a location is open.
    RouteBasis,
    /// Only the naming index's own assertion counts, and nothing else. An
    /// OpenAlex `locations[]` entry with no `is_oa`, no `license` and no
    /// version word is then [`LicenceStrength::Unknown`], which — with no
    /// version either — makes it an [`UnrankedCandidate`].
    IndexAssertionOnly,
}

// ============================================================================
// The metadata pass.
// ============================================================================

/// The servers the metadata pass reads, and the ones it only *names*.
///
/// Split deliberately. [`MetadataBases`] holds the four hosts a request may go
/// to and [`CandidateSources`] holds the bases whose URLs a candidate may carry
/// — a preprint server, OSTI, the DOI resolver — which the pass **never**
/// requests. One value for both would make "the pass touches no publisher host"
/// a matter of reading which field a call site used, and
/// [`MetadataPass::get_registry`] refuses a URL outside the first set so it is
/// a matter of the compiler accepting the call instead.
///
/// [`CandidateSources::default`] is the production configuration and the only
/// one the ladder uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataBases {
    /// Base of OpenAlex's `/works/doi:<doi>` — the pass reads **all**
    /// `locations[]`, not just the best one (ADR-007 §3).
    pub openalex_works: String,
    /// Base of Crossref's `/works/<doi>`.
    pub crossref_works: String,
    /// Base of DataCite's `/dois/<doi>`.
    pub datacite_dois: String,
    /// Base of Unpaywall's `/v2/<doi>`.
    pub unpaywall_api: String,
}

impl Default for MetadataBases {
    fn default() -> Self {
        Self {
            openalex_works: crate::openalex::OPENALEX_API_URL.to_string(),
            crossref_works: crate::crossref::CROSSREF_WORKS_URL.to_string(),
            datacite_dois: crate::datacite::DATACITE_DOIS_URL.to_string(),
            unpaywall_api: "https://api.unpaywall.org/v2".to_string(),
        }
    }
}

impl MetadataBases {
    /// The four registry bases out of a downloader's [`crate::download::Endpoints`].
    ///
    /// One authority for the configuration: a pass built from anything else
    /// would be a second copy of the endpoint list, and #292's `IdentityChain`
    /// bug was exactly that copy ignoring the bases it was handed.
    #[must_use]
    pub fn from_endpoints(endpoints: &crate::download::Endpoints) -> Self {
        Self {
            openalex_works: endpoints.openalex_works.clone(),
            crossref_works: endpoints.crossref_works.clone(),
            datacite_dois: endpoints.datacite_dois.clone(),
            unpaywall_api: endpoints.unpaywall_api.clone(),
        }
    }

    /// The base configured for one registry.
    fn base_for(&self, registry: Registry) -> &str {
        match registry {
            Registry::OpenAlex => &self.openalex_works,
            Registry::Crossref => &self.crossref_works,
            Registry::DataCite => &self.datacite_dois,
            Registry::Unpaywall => &self.unpaywall_api,
        }
    }

    /// Is this URL one the pass is allowed to request?
    ///
    /// **Host and port**, not prefix: the policy table keys on host too, so a
    /// prefix check would be a weaker rule than the one the pacer applies, and a
    /// URL that escapes the base by path would pass one and fail the other.
    #[must_use]
    pub fn allows(&self, registry: Registry, url: &reqwest::Url) -> bool {
        let Ok(base) = reqwest::Url::parse(self.base_for(registry)) else {
            return false;
        };
        url.host_str() == base.host_str()
            && url.port_or_known_default() == base.port_or_known_default()
    }
}

/// Bases the pass may *name* a candidate against without requesting it.
///
/// ADR-007 §3's fetch order puts four things after the registries — a preprint
/// transform, OSTI, the DOI resolver, the record's own URL — and each is a
/// deterministic function of an identifier we already hold, which is why none of
/// them needs a request to produce a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSources {
    /// The preprint transform's two bases (`crate::preprint`).
    pub preprint: crate::preprint::PreprintBases,
    /// OSTI's root, from which `{base}/servlets/purl/{id}` is built.
    pub osti_base: String,
    /// The DOI resolver, for the landing-page candidate. **Named, never
    /// requested during the pass.**
    pub doi_resolver: String,
    /// The record's own `arxiv_id`, when it has one.
    pub arxiv_pdf: String,
    /// The OpenAlex by-id base, the same `/works` route
    /// [`MetadataBases::openalex_works`] addresses. Named here because the
    /// candidates it produces are the record's, not the DOI's.
    pub openalex_api: String,
}

impl Default for CandidateSources {
    fn default() -> Self {
        Self {
            preprint: crate::preprint::PreprintBases::default(),
            osti_base: crate::osti::OSTI_BASE.to_string(),
            doi_resolver: "https://doi.org".to_string(),
            arxiv_pdf: "https://arxiv.org/pdf".to_string(),
            openalex_api: crate::openalex::OPENALEX_API_URL.to_string(),
        }
    }
}

impl CandidateSources {
    /// Both halves of a downloader's endpoint configuration.
    ///
    /// `Endpoints` is the single authority for every host this process may talk
    /// to, and this reads all of it rather than taking four of its eight fields
    /// — a test that pointed the pass at a `wiremock` server would otherwise
    /// have four ways to get the same server and four ways to forget one.
    #[must_use]
    pub fn from_endpoints(endpoints: &crate::download::Endpoints) -> Self {
        Self {
            preprint: crate::preprint::PreprintBases {
                biorxiv_content: endpoints.biorxiv_content.clone(),
                arxiv_pdf: endpoints.arxiv_pdf.clone(),
            },
            osti_base: endpoints.osti_base.clone(),
            doi_resolver: endpoints.doi_resolver.clone(),
            arxiv_pdf: endpoints.arxiv_pdf.clone(),
            openalex_api: endpoints.openalex_api.clone(),
        }
    }
}

/// What the work itself offers, which no registry has to be asked for.
///
/// Borrowed rather than owned because the values come straight off the stored
/// `papers` row and copying a work's identifiers into a resolver is a chance to
/// have two spellings of one DOI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkRefs<'a> {
    pub doi: Option<&'a str>,
    pub arxiv_id: Option<&'a str>,
    pub osti_id: Option<&'a str>,
    pub url: Option<&'a str>,
    /// The record's own OpenAlex id.
    ///
    /// Reached through the *same* `/works` base as the by-DOI lookup and in the
    /// same OpenAlex budget, which is why it is a field here rather than a
    /// fifth registry: a work with an `openalex_id` and no DOI is still a work
    /// OpenAlex can describe, and leaving it out would make the by-id leg a
    /// second metadata mechanism — the thing #292's `seeded` argument is
    /// against.
    pub openalex_id: Option<&'a str>,
}

/// The metadata pass, and the ranker it feeds.
///
/// Built from the downloader's configured bases so one rule serves production
/// and tests alike, and with a [`Self::with_bases`] seam for the same reason
/// [`crate::preprint::PreprintBases`] exists: a resolver that cannot be pointed
/// at a `wiremock` server cannot have its permits counted, and a resolver that
/// escapes onto the live internet from inside a test passes precisely when no
/// fixture exists for the endpoint it reached — which is how #292 found
/// `IdentityChain::new` ignoring its bases.
#[derive(Debug, Clone)]
pub struct MetadataPass {
    bases: MetadataBases,
    sources: CandidateSources,
    openalex: scitadel_core::config::OpenAlexAuth,
}

impl MetadataPass {
    /// The production pass.
    #[must_use]
    pub fn new(openalex: scitadel_core::config::OpenAlexAuth, mailto: &str) -> Self {
        Self {
            bases: MetadataBases::default(),
            sources: CandidateSources::default(),
            openalex: openalex_with_mailto(openalex, mailto),
        }
    }

    /// The pass against explicit bases. The test seam, and the reason
    /// `the_metadata_pass_touches_no_publisher_host` can assert on **hosts**:
    /// each registry gets its own `wiremock` server and its own bucket.
    #[must_use]
    pub fn with_bases(
        bases: MetadataBases,
        sources: CandidateSources,
        openalex: scitadel_core::config::OpenAlexAuth,
        mailto: &str,
    ) -> Self {
        Self {
            bases,
            sources,
            openalex: openalex_with_mailto(openalex, mailto),
        }
    }

    /// The four hosts this pass may request.
    #[must_use]
    pub fn bases(&self) -> &MetadataBases {
        &self.bases
    }

    /// Run the pass: ask the four registries, then rank everything found.
    ///
    /// The order of the requests is ADR-007 §3's identity chain order, and the
    /// order candidates are collected in is ADR-007 §3's **fetch** order —
    /// which matters because two candidates that compare equal keep the pass's
    /// order and so the tiebreak is the ADR's own sequence rather than
    /// whichever registry replied first.
    ///
    /// Every registry is asked even after one answers, which is the one place
    /// this pass deliberately differs from [`crate::identity_chain`]: the chain
    /// stops at the first answer because a second registry's title is a
    /// different work with the same words, whereas this pass is after *version
    /// and licence* facts that are genuinely distributed across all four — a
    /// Crossref `license[]` and an OpenAlex `locations[].version` are the two
    /// strongest signals available and they live in different registries. What
    /// the pass reads beyond identity is never fed to the matcher.
    pub async fn resolve(
        &self,
        client: &scitadel_http::PacedClient,
        scope: &scitadel_http::WorkScope,
        refs: WorkRefs<'_>,
    ) -> Resolution {
        let today = chrono::Utc::now().date_naive();
        let doi = refs
            .doi
            .map(str::trim)
            .filter(|doi| !doi.is_empty())
            .and_then(|doi| scitadel_core::models::validate_doi_detailed(doi).ok());

        let mut consulted: Vec<ConsultedRegistry> = Vec::new();
        let mut hosts: BTreeSet<String> = BTreeSet::new();
        let mut candidates: Vec<Candidate> = Vec::new();
        // Work-level version signals, collected from every registry before any
        // candidate is built: a Crossref `type` describes the *work*, and it
        // applies to a location Unpaywall named just as much as to one OpenAlex
        // did. That is why the four reads happen before the first candidate.
        let mut work_level: Option<(Version, VersionSource)> = None;
        let mut work_licences: Vec<LicenceOffer> = Vec::new();
        // The identity answer, kept beside the version and licence facts the
        // same bodies carry. One read, three uses — asking a registry a second
        // time for the title it already sent would spend the same hop twice,
        // which is the waste `identity_chain`'s `seeded` argument exists to
        // forbid.
        let mut identity_bodies: Vec<(Registry, serde_json::Value)> = Vec::new();

        // ---- 1. OpenAlex, by DOI: every `locations[]`, plus the work type. ----
        if let Some(doi) = doi.as_deref() {
            let hop = self
                .read(
                    client,
                    scope,
                    Registry::OpenAlex,
                    crate::openalex::doi_works_url(&self.bases.openalex_works, doi)
                        .map(|url| self.with_openalex_credentials(url))
                        .map_err(|e| e.to_string()),
                )
                .await;
            consulted.push(hop.record());
            hosts.extend(hop.hosts);
            if let Some(body) = hop.body.clone() {
                identity_bodies.push((Registry::OpenAlex, body.clone()));
                work_level = crate::openalex::type_signal(&body).and_then(|(field, classified)| {
                    let word = body
                        .get(field)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    version_from_registry(Registry::OpenAlex, classified, &word)
                });
                let is_oa = crate::openalex::declares_open_access(&body);
                for (at, location) in crate::openalex::oa_locations(&body).iter().enumerate() {
                    let Some(url) = location.fetchable() else {
                        continue;
                    };
                    // `best_oa_location` is OpenAlex's own assertion that *this*
                    // location is the open one, and gets the index's credit for
                    // it. Every other `locations[]` entry does not: OpenAlex
                    // lists all of them, open or not, and a repository copy it
                    // indexed once is not thereby free to read.
                    let best = location.is_best;
                    if !is_fetchable_url(url) {
                        tracing::info!(
                            url,
                            "openalex named a location whose URL cannot be fetched; \
                             it is not offered as a candidate"
                        );
                        continue;
                    }
                    candidates.push(Self::location_candidate(
                        today,
                        CandidateSeed {
                            route: RouteId::OpenAlex,
                            url: url.to_string(),
                            named_by: if best {
                                format!("{} best_oa_location", Registry::OpenAlex)
                            } else {
                                format!("{} locations[{}]", Registry::OpenAlex, at + 1)
                            },
                            version_word: location.version_word.clone(),
                            location_licence: location.licence.clone(),
                            free_to_read: best || is_oa,
                            licence_floor: if best {
                                LicenceFloor::RouteBasis
                            } else {
                                LicenceFloor::IndexAssertionOnly
                            },
                        },
                        refs.doi,
                        work_level.as_ref(),
                        &work_licences,
                    ));
                }
            }
        } else {
            consulted.push(ConsultedRegistry {
                registry: Registry::OpenAlex,
                outcome: crate::identity_chain::HopOutcome::NothingToAsk,
            });
        }

        // ---- 1b. OpenAlex again, by the record's own id. ----
        //
        // The same registry, the same base and the same bucket, so it is a
        // second request into `openalex` rather than a fifth mechanism. Only
        // issued when the record carries an id *and* the by-DOI read did not
        // already answer: a work whose DOI resolved is fully described by that
        // record, and `identity_chain`'s `seeded` argument is the same one —
        // a record already in hand is not worth re-requesting.
        if let Some(id) = refs.openalex_id.map(str::trim).filter(|id| !id.is_empty())
            && !consulted.iter().any(|hop| {
                hop.registry == Registry::OpenAlex
                    && hop.outcome == crate::identity_chain::HopOutcome::Answered
            })
        {
            let short = id
                .rsplit('/')
                .next()
                .unwrap_or(id)
                .trim_start_matches("https://openalex.org/");
            let hop = self
                .read(
                    client,
                    scope,
                    Registry::OpenAlex,
                    reqwest::Url::parse(&format!(
                        "{}/{short}",
                        self.sources.openalex_api.trim_end_matches('/')
                    ))
                    .map(|url| self.with_openalex_credentials(url))
                    .map_err(|e| e.to_string()),
                )
                .await;
            consulted.push(hop.record());
            hosts.extend(hop.hosts);
            if let Some(body) = hop.body.clone() {
                identity_bodies.push((Registry::OpenAlex, body.clone()));
                work_level = work_level.or_else(|| {
                    crate::openalex::type_signal(&body).and_then(|(field, classified)| {
                        let word = body
                            .get(field)
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        version_from_registry(Registry::OpenAlex, classified, &word)
                    })
                });
                let is_oa = crate::openalex::declares_open_access(&body);
                for (at, location) in crate::openalex::oa_locations(&body).iter().enumerate() {
                    let Some(url) = location.fetchable() else {
                        continue;
                    };
                    let best = location.is_best;
                    candidates.push(Self::location_candidate(
                        today,
                        CandidateSeed {
                            route: RouteId::OpenAlex,
                            url: url.to_string(),
                            named_by: if best {
                                format!("{} {short} best_oa_location", Registry::OpenAlex)
                            } else {
                                format!("{} {short} locations[{}]", Registry::OpenAlex, at + 1)
                            },
                            version_word: location.version_word.clone(),
                            location_licence: location.licence.clone(),
                            free_to_read: best || is_oa,
                            licence_floor: if best {
                                LicenceFloor::RouteBasis
                            } else {
                                LicenceFloor::IndexAssertionOnly
                            },
                        },
                        refs.doi,
                        work_level.as_ref(),
                        &work_licences,
                    ));
                }
            }
        }

        // ---- 2. Crossref: the record's `type`, its `license[]`, its links. ----
        if let Some(doi) = doi.as_deref() {
            let hop = self
                .read(
                    client,
                    scope,
                    Registry::Crossref,
                    crate::crossref::CrossrefAdapter::new("")
                        .with_base_url(&self.bases.crossref_works)
                        .works_url(doi)
                        .map_err(|e| e.to_string()),
                )
                .await;
            consulted.push(hop.record());
            hosts.extend(hop.hosts);
            if let Some(body) = hop.body.clone()
                && let Some(message) = body.get("message").filter(|m| m.is_object())
            {
                identity_bodies.push((Registry::Crossref, body.clone()));
                if work_level.is_none() {
                    work_level = crate::crossref::type_signal(message).and_then(|classified| {
                        let word = message
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        version_from_registry(Registry::Crossref, classified, &word)
                    });
                }
                work_licences.extend(LicenceOffer::from_crossref(message));
                let links = crate::crossref::links(message);
                for (at, link) in links.iter().enumerate() {
                    if !is_fetchable_url(&link.url) {
                        continue;
                    }
                    candidates.push(Self::location_candidate(
                        today,
                        CandidateSeed {
                            route: RouteId::Crossref,
                            url: link.url.clone(),
                            named_by: format!("{} link[{}]", Registry::Crossref, at + 1),
                            version_word: link.content_version.clone(),
                            location_licence: None,
                            free_to_read: false,
                            // `RouteId::Crossref::access_basis()` is `None` on
                            // purpose — a `link[]` entry answers "where do the
                            // bytes live", not "may we keep them" — so the route
                            // basis is not available and a link with no grant
                            // behind it has no licence either.
                            licence_floor: LicenceFloor::RouteBasis,
                        },
                        refs.doi,
                        work_level.as_ref(),
                        &work_licences,
                    ));
                }
            }
        } else {
            consulted.push(ConsultedRegistry {
                registry: Registry::Crossref,
                outcome: crate::identity_chain::HopOutcome::NothingToAsk,
            });
        }

        // ---- 3. DataCite: the record's `types`, its `rightsList`, its URL. ----
        if let Some(doi) = doi.as_deref() {
            let hop = self
                .read(
                    client,
                    scope,
                    Registry::DataCite,
                    crate::datacite::DataCiteAdapter::new("")
                        .with_base_url(&self.bases.datacite_dois)
                        .dois_url(doi)
                        .map_err(|e| e.to_string()),
                )
                .await;
            consulted.push(hop.record());
            hosts.extend(hop.hosts);
            if let Some(body) = hop.body.clone() {
                identity_bodies.push((Registry::DataCite, body.clone()));
                if work_level.is_none()
                    && let Some(signal) = crate::datacite::type_signal(&body)
                {
                    work_level =
                        version_from_registry(Registry::DataCite, signal.classified, &signal.word);
                }
                if let Some(attributes) = body.pointer("/data/attributes") {
                    work_licences.extend(LicenceOffer::from_datacite(attributes));
                }
                if let Some(url) = body
                    .pointer("/data/attributes/url")
                    .and_then(serde_json::Value::as_str)
                    .filter(|url| is_fetchable_url(url))
                {
                    candidates.push(Self::location_candidate(
                        today,
                        CandidateSeed {
                            route: RouteId::DataCite,
                            url: url.to_string(),
                            named_by: format!("{} attributes.url", Registry::DataCite),
                            version_word: None,
                            location_licence: None,
                            free_to_read: false,
                            licence_floor: LicenceFloor::RouteBasis,
                        },
                        refs.doi,
                        work_level.as_ref(),
                        &work_licences,
                    ));
                }
            }
        } else {
            consulted.push(ConsultedRegistry {
                registry: Registry::DataCite,
                outcome: crate::identity_chain::HopOutcome::NothingToAsk,
            });
        }

        // ---- 4. Unpaywall: its own locations, with their own version words. ----
        if let Some(doi) = doi.as_deref() {
            let hop = self
                .read(
                    client,
                    scope,
                    Registry::Unpaywall,
                    reqwest::Url::parse(&format!(
                        "{}/{doi}?email={}",
                        self.bases.unpaywall_api.trim_end_matches('/'),
                        self.openalex.email
                    ))
                    .map_err(|e| e.to_string()),
                )
                .await;
            consulted.push(hop.record());
            hosts.extend(hop.hosts);
            if let Some(body) = hop.body {
                let mut locations: Vec<serde_json::Value> = body
                    .get("best_oa_location")
                    .filter(|best| best.is_object())
                    .cloned()
                    .into_iter()
                    .collect();
                locations.extend(
                    body.get("oa_locations")
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
                let mut seen: BTreeSet<&str> = BTreeSet::new();
                for (at, location) in locations.iter().enumerate() {
                    let Some(url) = location
                        .get("url_for_pdf")
                        .and_then(serde_json::Value::as_str)
                        .filter(|url| !url.trim().is_empty())
                    else {
                        continue;
                    };
                    if !seen.insert(url) {
                        continue;
                    }
                    if !is_fetchable_url(url) {
                        tracing::info!(
                            url,
                            "unpaywall named a location whose URL cannot be fetched; \
                             it is not offered as a candidate"
                        );
                        continue;
                    }
                    candidates.push(Self::location_candidate(
                        today,
                        CandidateSeed {
                            route: RouteId::Unpaywall,
                            url: url.to_string(),
                            named_by: format!("{} location {at}", Registry::Unpaywall),
                            version_word: location
                                .get("version")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            location_licence: location
                                .get("license")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            // Unpaywall lists a location under `oa_locations`
                            // **because** it is open, so `is_oa` needs no
                            // further corroboration here. OpenAlex gets no such
                            // credit: its `locations[]` is all locations,
                            // OA or not.
                            free_to_read: location
                                .get("is_oa")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(true),
                            licence_floor: LicenceFloor::RouteBasis,
                        },
                        refs.doi,
                        work_level.as_ref(),
                        &work_licences,
                    ));
                }
            }
        } else {
            consulted.push(ConsultedRegistry {
                registry: Registry::Unpaywall,
                outcome: crate::identity_chain::HopOutcome::NothingToAsk,
            });
        }

        // ---- 5. The derived routes: named from identifiers, never requested. ----
        //
        // ADR-007 §3's fetch order after the registries: arXiv for preprints,
        // DOE OSTI for national-lab reports, then the publisher's landing page,
        // then the record's own URL. Collected **last**, so a preprint-server
        // candidate loses a tie to a registry location ADR puts earlier — the
        // tiebreak is the ADR's own sequence.
        let derived = self.derived_candidates(refs, work_level.as_ref());
        let derived_routes: Vec<RouteId> = derived.iter().map(|c| c.route).collect();
        candidates.extend(derived);

        let mut resolution = rank_candidates(candidates);
        resolution.identity = Self::identity_answer(&consulted, &identity_bodies);
        resolution.consulted = consulted;
        resolution.hosts_contacted = hosts;
        resolution.derived_routes = derived_routes;
        resolution
    }

    /// The identity verdict's inputs, read off the bodies the pass already has.
    ///
    /// The *decision* is still ADR-007 §3's and still belongs to
    /// [`crate::identity::verify`]: the first registry in
    /// [`crate::identity_chain::CHAIN`] that named the work is the resolved
    /// side, and nothing here averages two registries' titles or prefers a
    /// later one. What is reused is the hop vocabulary and the ordering, so the
    /// pass and the chain cannot report two different stories about the same
    /// three requests.
    ///
    /// Unpaywall contributes nothing, deliberately: ADR-007 §3's chain does not
    /// include it, and `download.rs` already documents that Unpaywall's `title`
    /// is a copy of the publisher-supplied title it was handed — so naming it as
    /// a resolver would launder the publisher's own string through a
    /// verification that verification did not do.
    fn identity_answer(
        consulted: &[ConsultedRegistry],
        bodies: &[(Registry, serde_json::Value)],
    ) -> crate::identity_chain::ChainOutcome {
        // **One hop per identity source**, which is what ADR-007 §3's chain is:
        // "OpenAlex → Crossref → DataCite", three hops. This pass may ask OpenAlex
        // twice — by DOI and, if the work has no DOI, by its `openalex_id` — and
        // `ChainOutcome::consulted_any` reads the *first* hop to decide whether
        // anything was asked, so two entries for one registry would make an
        // answered-by-id work read as "nothing was consulted". The collapse takes
        // the better of the two: an `Answered` hop beats a `NothingToAsk` one, and
        // an `Unreadable` hop beats a `NotRegistered` one, because the first says
        // "we tried and could not" and the second only "this registry has never
        // heard of it".
        let mut hops: Vec<crate::identity_chain::ChainHop> = Vec::new();
        for hop in consulted {
            let Some(source) = hop.registry.identity_source() else {
                continue;
            };
            let Some(existing) = hops.iter_mut().find(|seen| seen.source == source) else {
                hops.push(crate::identity_chain::ChainHop {
                    source,
                    outcome: hop.outcome,
                });
                continue;
            };
            if hop.outcome > existing.outcome {
                existing.outcome = hop.outcome;
            }
        }
        let mut resolved = None;
        for source in crate::identity_chain::CHAIN {
            let Some((_, body)) = bodies
                .iter()
                .find(|(registry, _)| registry.identity_source() == Some(source))
            else {
                continue;
            };
            let identity = match source {
                scitadel_db::sqlite::IdentitySource::OpenAlex => {
                    Some(crate::openalex::work_identity(body))
                }
                scitadel_db::sqlite::IdentitySource::Crossref => {
                    match crate::crossref::parse_work(body) {
                        crate::crossref::CrossrefAnswer::Found(identity) => Some(identity),
                        // A 200 that is not a `message` envelope is unreadable,
                        // not a record of three nulls — the trap the chain
                        // module documents at length.
                        _ => None,
                    }
                }
                scitadel_db::sqlite::IdentitySource::DataCite => {
                    match crate::datacite::parse_doi(body) {
                        crate::datacite::DataCiteAnswer::Found(identity) => Some(identity),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(identity) = identity {
                resolved = Some(crate::identity_chain::ResolvedWork { identity, source });
                break;
            }
        }
        // A registry that answered with a body no identity could be read out of
        // is `Unreadable` on the identity side even though its version and
        // licence facts were perfectly usable. The two answers are about
        // different fields of the same record, and saying so is the same
        // distinction the chain draws between `NotRegistered` and `Unreadable`.
        for hop in &mut hops {
            if hop.outcome == crate::identity_chain::HopOutcome::Answered
                && !bodies.iter().any(|(registry, body)| {
                    registry.identity_source() == Some(hop.source)
                        && Self::identity_of(hop.source, body).is_some()
                })
            {
                hop.outcome = crate::identity_chain::HopOutcome::Unreadable;
            }
        }
        crate::identity_chain::ChainOutcome { resolved, hops }
    }

    /// The three facts `identity::verify` reads, out of one registry's body.
    fn identity_of(
        source: scitadel_db::sqlite::IdentitySource,
        body: &serde_json::Value,
    ) -> Option<crate::identity::WorkIdentity> {
        match source {
            scitadel_db::sqlite::IdentitySource::OpenAlex => {
                Some(crate::openalex::work_identity(body))
            }
            scitadel_db::sqlite::IdentitySource::Crossref => {
                match crate::crossref::parse_work(body) {
                    crate::crossref::CrossrefAnswer::Found(identity) => Some(identity),
                    _ => None,
                }
            }
            scitadel_db::sqlite::IdentitySource::DataCite => {
                match crate::datacite::parse_doi(body) {
                    crate::datacite::DataCiteAnswer::Found(identity) => Some(identity),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The candidates no registry can name, because each is a function of an
    /// identifier the work already carries.
    ///
    /// Split out of [`Self::resolve`] so the fact that these cost **no request**
    /// is structural rather than a claim: nothing in this function has a client.
    fn derived_candidates(
        &self,
        refs: WorkRefs<'_>,
        work_level: Option<&(Version, VersionSource)>,
    ) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = Vec::new();
        // One normalisation, so the four candidate kinds below cannot disagree
        // about which DOI they were given (#262).
        let doi = refs
            .doi
            .map(str::trim)
            .filter(|doi| !doi.is_empty())
            .and_then(|doi| scitadel_core::models::validate_doi_detailed(doi).ok());
        let doi = doi.as_deref();

        // The record's own `arxiv_id`, **first** among the derived routes.
        //
        // Ahead of the DOI transform, and the reason is the difference between a
        // stated identifier and an inferred one: `arxiv_id` is on the record
        // because someone resolved it, while the `10.48550` transform is string
        // arithmetic on a suffix. Both are preprints by construction, so they
        // tie on version and licence — and the tiebreak is ADR-007 §3's own
        // fetch order, where a *known* identifier precedes a derived one. The
        // old ladder had the same shape for the same reason.
        if let Some(id) = refs.arxiv_id.map(str::trim).filter(|id| !id.is_empty()) {
            out.push(Candidate {
                route: RouteId::Arxiv,
                url: format!(
                    "{}/{}.pdf",
                    self.sources.arxiv_pdf.trim_end_matches('/'),
                    id.trim_start_matches("https://arxiv.org/abs/")
                        .trim_start_matches("https://arxiv.org/pdf/")
                ),
                named_by: "the record's own arxiv_id".to_string(),
                version: Some(Version::Preprint),
                version_source: VersionSource::Route {
                    route: RouteId::Arxiv,
                },
                licence: LicenceStrength::FreeToRead,
            });
        }

        // The preprint transform (#260): a pure function of the DOI.
        if let Some(doi) = doi {
            for candidate in crate::preprint::preprint_candidates_at(&self.sources.preprint, doi) {
                out.push(Candidate {
                    route: candidate.route,
                    url: candidate.url,
                    named_by: format!("the {} DOI transform", candidate.route),
                    version: Some(Version::Preprint),
                    version_source: VersionSource::Route {
                        route: candidate.route,
                    },
                    licence: LicenceStrength::FreeToRead,
                });
            }
        }

        // OSTI: a `purl` derived from the report's identifier.
        if let Some(id) = refs.osti_id.map(str::trim).filter(|id| !id.is_empty())
            && let Some(url) = crate::osti::purl_url_at(&self.sources.osti_base, id)
        {
            out.push(Candidate {
                route: RouteId::Osti,
                url,
                named_by: format!("the osti_id {id} purl"),
                // A national-lab report's own publication is its version of
                // record. Stated here rather than left `None` because OSTI
                // serves nothing else, and a route that can only ever serve one
                // kind of document is a version authority by construction —
                // the same claim `RouteId::access_basis` already makes when it
                // calls OSTI's work US-government public domain.
                version: Some(Version::VersionOfRecord),
                version_source: VersionSource::Route {
                    route: RouteId::Osti,
                },
                licence: LicenceStrength::FreeToRead,
            });
        }

        // The DOI resolver's landing page. **No version signal at all**, and
        // that is deliberate and is the honest answer: a publisher's landing
        // page serves whatever rendering the paywall happens to serve, and
        // nothing in a metadata pass establishes which. It is therefore an
        // *unrankable* candidate unless a registry typed the work — which is
        // what keeps ADR-007 §3's step 6 ("landing-page HTML, for OA works
        // only") honest without this module having to invent a version.
        if let Some(normalised) = doi {
            out.push(Candidate {
                route: RouteId::Publisher,
                url: format!(
                    "{}/{normalised}",
                    self.sources.doi_resolver.trim_end_matches('/')
                ),
                named_by: "the DOI resolver's landing page".to_string(),
                // The same four-step chain, so a `10.1101` DOI's landing page
                // is a preprint (which it is — `doi.org/10.1101/…` lands on the
                // posting) and an unregistered publisher DOI's landing page is
                // an unstated version rather than a guess at one.
                version: work_level
                    .as_ref()
                    .map(|(version, _)| *version)
                    .or_else(|| {
                        doi.and_then(version_from_doi_prefix)
                            .map(|(version, _)| version)
                    })
                    .or(Some(Version::Unstated)),
                version_source: work_level
                    .as_ref()
                    .map(|(_, source)| source.clone())
                    .or_else(|| {
                        doi.and_then(version_from_doi_prefix)
                            .map(|(_, source)| source)
                    })
                    .unwrap_or_else(|| VersionSource::Unstated {
                        because: String::from("no registry typed the work"),
                    }),
                licence: LicenceStrength::SubscriptionRead,
            });
        }

        // The record's own `url`, last: ADR-007 §3 calls it the last resort and
        // the tiebreak follows suit.
        if let Some(url) = refs.url.map(str::trim).filter(|url| !url.is_empty()) {
            out.push(Candidate {
                route: RouteId::ManualUrl,
                url: url.to_string(),
                named_by: "the record's own url".to_string(),
                version: work_level
                    .as_ref()
                    .map(|(version, _)| *version)
                    .or_else(|| {
                        doi.and_then(version_from_doi_prefix)
                            .map(|(version, _)| version)
                    })
                    .or(Some(Version::Unstated)),
                version_source: work_level
                    .as_ref()
                    .map(|(_, source)| source.clone())
                    .or_else(|| {
                        doi.and_then(version_from_doi_prefix)
                            .map(|(_, source)| source)
                    })
                    .unwrap_or_else(|| VersionSource::Unstated {
                        because: String::from("no registry typed the work"),
                    }),
                licence: LicenceStrength::SubscriptionRead,
            });
        }

        out
    }

    /// Build one location's candidate, through the four-step version chain.
    ///
    /// The chain, strongest first, and the reason for each step:
    ///
    /// 1. the **location's own** `version` word, if the registry gave one;
    /// 2. the **record's** `type`, from whichever registry typed it first;
    /// 3. the **DOI prefix**, the #260 fallback, recorded as such;
    /// 4. nothing, which makes the candidate [`UnrankedCandidate`].
    ///
    /// Step 4 is a `None` and never a default — and it is only reachable for a
    /// candidate whose `licence_floor` is
    /// [`LicenceFloor::IndexAssertionOnly`], because that is the only source in
    /// this pass that can decline to vouch for its own location. Every other
    /// source either states a version or states that it does not know, and
    /// "does not know which version" is [`Version::Unstated`], not `None`.
    fn location_candidate(
        today: chrono::NaiveDate,
        seed: CandidateSeed,
        doi: Option<&str>,
        work_level: Option<&(Version, VersionSource)>,
        work_licences: &[LicenceOffer],
    ) -> Candidate {
        let named = seed.named_by.clone();
        let (version, version_source) = seed
            .version_word
            .as_deref()
            .and_then(|word| {
                if seed.route == RouteId::Crossref {
                    version_from_content_version(word)
                } else if seed.route == RouteId::Unpaywall {
                    version_from_location_word(word, Registry::Unpaywall)
                } else {
                    version_from_location_word(word, Registry::OpenAlex)
                }
            })
            .or_else(|| work_level.cloned())
            .or_else(|| doi.and_then(version_from_doi_prefix))
            .map_or_else(
                || {
                    // Nothing said. Whether that is `Unstated` or unrankable is
                    // the floor's decision, because "here is a rendering whose
                    // version I do not state" is something a source can say and
                    // "here is a place I once saw" is not.
                    match seed.licence_floor {
                        LicenceFloor::RouteBasis => (
                            Some(Version::Unstated),
                            VersionSource::Unstated {
                                because: format!("{named} named no version"),
                            },
                        ),
                        LicenceFloor::IndexAssertionOnly => (None, VersionSource::None),
                    }
                },
                |(version, source)| (Some(version), source),
            );

        // The licence: a grant on *this* location first, then any grant the
        // record carries, filtered to grants in force for *this* version. A
        // `vor`-only grant must not license a preprint candidate, and
        // `crate::registry` is the one place that decides.
        let mut offers: Vec<LicenceOffer> = seed
            .location_licence
            .as_deref()
            .map(|licence| {
                vec![LicenceOffer {
                    url: licence.to_string(),
                    content_version: None,
                    start: None,
                    delay_in_days: None,
                    registry: seed.registry_hint(),
                }]
            })
            .unwrap_or_default();
        offers.extend(work_licences.iter().cloned());
        let counting: Vec<LicenceOffer> = offers
            .into_iter()
            .filter(|offer| {
                offer
                    .counts_for(version.map_or("unstated", |version| version.label()), today)
                    .is_none()
            })
            .collect();

        let licence = if !counting.is_empty() {
            LicenceStrength::OpenLicence
        } else if seed.free_to_read {
            // The naming index says this location is free to read. That is a
            // weaker position than a grant and a real one, and it is the same
            // claim `RouteId::access_basis` makes for an OA route.
            LicenceStrength::FreeToRead
        } else {
            licence_strength(&[], version, seed.route, seed.licence_floor)
        };

        Candidate {
            route: seed.route,
            url: seed.url,
            named_by: seed.named_by,
            version,
            version_source,
            licence,
        }
    }

    /// OpenAlex's two credentials, on the one request the pass makes to it.
    ///
    /// Shared with [`crate::identity_chain`] by construction — same
    /// parameters, same reasons, both documented at [`crate::openalex`]: `mailto`
    /// for the polite pool and `api_key` for the metered quota (#212). Empty
    /// ones are omitted rather than sent blank.
    fn with_openalex_credentials(&self, mut url: reqwest::Url) -> reqwest::Url {
        {
            let mut query = url.query_pairs_mut();
            if !self.openalex.email.is_empty() {
                query.append_pair("mailto", &self.openalex.email);
            }
            if !self.openalex.api_key.is_empty() {
                query.append_pair("api_key", &self.openalex.api_key);
            }
        }
        url
    }

    /// Ask one registry, through [`scitadel_http::PacedClient`] and nothing else.
    ///
    /// Two properties are structural here rather than promised in prose:
    ///
    /// - **one `Request` permit per hop**, out of that registry's own bucket,
    ///   because `PacedClient` spends it (`ADR-007 §4`). Four registries are
    ///   four platforms' budgets.
    /// - **the URL is checked against the registry's configured base** before
    ///   the request, and a mismatch is an `Unreadable` answer rather than a
    ///   request. A pass that could reach a publisher host would make
    ///   "the metadata pass touches no publisher host" a property of which
    ///   endpoint a caller configured; this makes it a property of the code.
    async fn read(
        &self,
        client: &scitadel_http::PacedClient,
        scope: &scitadel_http::WorkScope,
        registry: Registry,
        url: Result<reqwest::Url, String>,
    ) -> RegistryHop {
        let url = match url {
            Ok(url) => url,
            Err(reason) => {
                tracing::info!(%registry, %reason, "could not build a metadata URL");
                return RegistryHop {
                    registry,
                    outcome: crate::identity_chain::HopOutcome::Unreadable,
                    body: None,
                    hosts: BTreeSet::new(),
                    detail: reason,
                };
            }
        };
        if !self.bases.allows(registry, &url) {
            // Unreachable through the four call sites above, which each build a
            // URL from their own registry's base. It is here so that a future
            // fifth call site cannot turn a metadata pass into a resolve that
            // reaches a publisher, and so that the guarantee is checkable.
            let host = url.host_str().unwrap_or("(no host)").to_string();
            tracing::warn!(
                %registry,
                %host,
                "refusing to request a URL outside this registry's configured base"
            );
            return RegistryHop {
                registry,
                outcome: crate::identity_chain::HopOutcome::Unreadable,
                body: None,
                hosts: BTreeSet::new(),
                detail: format!(
                    "{host} is not {registry}'s configured base, so the metadata \
                     pass did not contact it"
                ),
            };
        }

        let host = match url.host_str().map(str::to_ascii_lowercase) {
            Some(host) => match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            },
            None => "(no host)".to_string(),
        };

        let response = match client
            .get_in_work(
                scope,
                url,
                scitadel_core::ports::PaceTier::Meta,
                scitadel_http::SafeHeaders::unauthenticated(),
            )
            .await
        {
            Ok(response) => response,
            // The `identity_chain` distinction, verbatim: a 404 is *not
            // registered here* — knowledge, and the chain continues — while
            // everything else is *we could not find out*, which must never be
            // reported as a fact about the work.
            Err(scitadel_http::FetchError::Status { code: 404, .. }) => {
                return RegistryHop {
                    registry,
                    outcome: crate::identity_chain::HopOutcome::NotRegistered,
                    body: None,
                    hosts: BTreeSet::from([host]),
                    detail: format!("{registry} does not register this DOI"),
                };
            }
            Err(error) => {
                return RegistryHop {
                    registry,
                    outcome: crate::identity_chain::HopOutcome::Unreadable,
                    body: None,
                    hosts: BTreeSet::from([host]),
                    detail: format!("{registry} could not be asked: {error}"),
                };
            }
        };
        let text = match response.text().await {
            Ok(text) => text,
            Err(error) => {
                return RegistryHop {
                    registry,
                    outcome: crate::identity_chain::HopOutcome::Unreadable,
                    body: None,
                    hosts: BTreeSet::from([host]),
                    detail: format!("{registry} answered and the body would not read: {error}"),
                };
            }
        };
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(body) => RegistryHop {
                registry,
                outcome: crate::identity_chain::HopOutcome::Answered,
                body: Some(body),
                hosts: BTreeSet::from([host]),
                detail: format!("{registry} answered"),
            },
            Err(error) => RegistryHop {
                registry,
                outcome: crate::identity_chain::HopOutcome::Unreadable,
                body: None,
                hosts: BTreeSet::from([host]),
                detail: format!("{registry} answered 200 that is not JSON: {error}"),
            },
        }
    }
}

/// What one registry's answer looked like, before it becomes a candidate.
///
/// `body` is the parsed 2xx body when there was one and the parse succeeded;
/// the `identity_chain` module's `Found`/`Unreadable` split, for the same
/// reason: a 200 that is not the registry's record envelope is *unreadable*,
/// not a record of null fields.
struct RegistryHop {
    registry: Registry,
    outcome: crate::identity_chain::HopOutcome,
    body: Option<serde_json::Value>,
    hosts: BTreeSet<String>,
    detail: String,
}

impl RegistryHop {
    /// The hop as [`Resolution::consulted`] wants it, by reference so the body
    /// survives to build candidates with.
    fn record(&self) -> ConsultedRegistry {
        tracing::debug!(
            registry = %self.registry,
            outcome = ?self.outcome,
            detail = %self.detail,
            "metadata pass"
        );
        ConsultedRegistry {
            registry: self.registry,
            outcome: self.outcome,
        }
    }
}

/// One location's raw facts, before the chain turns them into a version.
struct CandidateSeed {
    route: RouteId,
    url: String,
    named_by: String,
    /// The location's own `version` word, when the naming registry gave one.
    version_word: Option<String>,
    /// A licence on *this* location, as the naming registry spelled it.
    location_licence: Option<String>,
    /// Whether the naming index asserted the location is free to read.
    free_to_read: bool,
    /// Where the licence falls back when no grant was offered. See
    /// [`LicenceFloor`].
    licence_floor: LicenceFloor,
}

impl CandidateSeed {
    /// Which registry's offer a location-level licence string is, for
    /// [`LicenceOffer::registry`].
    fn registry_hint(&self) -> Registry {
        match self.route {
            RouteId::Crossref => Registry::Crossref,
            RouteId::DataCite => Registry::DataCite,
            RouteId::Unpaywall => Registry::Unpaywall,
            _ => Registry::OpenAlex,
        }
    }
}

/// The pass's `OpenAlexAuth`, with the `mailto` every registry needs filled in.
///
/// `Endpoints` and `IdentityChain` both take an `OpenAlexAuth` **and** a
/// separate `mailto`, which is two spellings of one operator identity; filling
/// the address in once here means the four requests cannot disagree about which
/// polite pool they are in.
fn openalex_with_mailto(
    mut openalex: scitadel_core::config::OpenAlexAuth,
    mailto: &str,
) -> scitadel_core::config::OpenAlexAuth {
    if openalex.email.trim().is_empty() && !mailto.trim().is_empty() {
        openalex.email = mailto.trim().to_string();
    }
    openalex
}

#[cfg(test)]
mod tests {
    use super::*;

    use scitadel_core::ports::{Bucket, Cost, PaceDenied, PaceTier, Pacer, Permit};
    use scitadel_http::{PacedClient, WorkScope};
    use std::sync::Arc;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A candidate at a given version and licence, with the source spelled out.
    fn candidate(route: RouteId, version: Option<Version>, licence: LicenceStrength) -> Candidate {
        Candidate {
            route,
            url: format!(
                "https://example.invalid/{}/{}",
                route.label(),
                version.map_or("none", |v| v.label())
            ),
            named_by: "the metadata pass".to_string(),
            version,
            version_source: match version {
                Some(Version::Unstated) => VersionSource::Unstated {
                    because: String::from("the fixture states no version"),
                },
                Some(Version::Preprint) => VersionSource::Registry {
                    registry: Registry::Crossref,
                    signal: "posted-content".to_string(),
                },
                Some(Version::AuthorManuscript) => VersionSource::Registry {
                    registry: Registry::OpenAlex,
                    signal: "acceptedVersion".to_string(),
                },
                Some(Version::VersionOfRecord) => VersionSource::Registry {
                    registry: Registry::OpenAlex,
                    signal: "publishedVersion".to_string(),
                },
                None => VersionSource::None,
            },
            licence,
        }
    }

    // =====================================================================
    // The ordering itself.
    // =====================================================================

    /// ADR-007 §3's ladder, written out longhand so a reordering of [`Rank`]'s
    /// fields cannot pass.
    ///
    /// The three rows are the claim: **a version of record outranks an author
    /// manuscript outranks a preprint**, and licence never crosses a version
    /// boundary. The middle two rows are the load-bearing ones — a
    /// CC-BY preprint must not outrank an all-rights-reserved version of
    /// record, because `acquire` would then close a `wanted_version = 'vor'`
    /// want with a preprint and `coverage` would read the library as holding the
    /// article of record.
    #[test]
    fn ranked_order_is_documented_and_pinned() {
        use LicenceStrength::{FreeToRead, OpenLicence, SubscriptionRead, Unknown};
        use Version::{AuthorManuscript, Preprint, Unstated, VersionOfRecord};

        // Strict domination, both directions.
        assert!(
            Rank {
                version: VersionOfRecord,
                licence: Unknown
            } < Rank {
                version: AuthorManuscript,
                licence: OpenLicence
            },
            "an unknown-licence VoR outranks a CC-BY author manuscript"
        );
        assert!(
            Rank {
                version: AuthorManuscript,
                licence: Unknown
            } < Rank {
                version: Preprint,
                licence: OpenLicence
            },
            "an unknown-licence AM outranks a CC-BY preprint"
        );

        // Licence only breaks ties *within* one version.
        for version in Version::ALL {
            let mut by_licence = LicenceStrength::ALL
                .iter()
                .map(|licence| Rank {
                    version,
                    licence: *licence,
                })
                .collect::<Vec<_>>();
            by_licence.sort();
            assert_eq!(
                by_licence[0].licence,
                LicenceStrength::OpenLicence,
                "{version}: the strongest licence leads within its version"
            );
            assert_eq!(
                by_licence[3].licence,
                LicenceStrength::Unknown,
                "{version}: the weakest licence is last within its version"
            );
            for pair in by_licence.windows(2) {
                assert!(pair[0] < pair[1], "{version}: {pair:?} must be ordered");
            }
        }

        // And the full ladder, strongest to weakest, is the four by-four grid
        // with version outermost.
        let mut every: Vec<Rank> = Version::ALL
            .iter()
            .flat_map(|version| {
                LicenceStrength::ALL.iter().map(move |licence| Rank {
                    version: *version,
                    licence: *licence,
                })
            })
            .collect();
        every.sort();
        assert_eq!(every.len(), 16, "four versions by four licence strengths");
        assert_eq!(
            every[0],
            Rank {
                version: VersionOfRecord,
                licence: OpenLicence
            }
        );
        assert_eq!(
            every[15],
            Rank {
                version: Unstated,
                licence: Unknown
            },
            "the weakest of the weakest is last"
        );
        // **The dominance, written out.** Version is the outer axis: all four
        // licence strengths of a version of record come before the strongest
        // cell of an author manuscript, which comes before all four of a
        // preprint, and so on. Sixteen cells, and no licence may cross a version
        // boundary — which is the claim ADR-007 §3's "then by licence strength"
        // makes and the reason a CC-BY preprint never closes a `vor` want.
        let cells_per_version = LicenceStrength::ALL.len();
        for (at, cell) in every.iter().enumerate() {
            let expected_version = Version::ALL[at / cells_per_version];
            assert_eq!(
                cell.version, expected_version,
                "cells {at}..{at} belong to {expected_version:?}"
            );
        }

        // `SubscriptionRead` sits between `FreeToRead` and `Unknown`, which is
        // the judgement worth pinning: reading a paywalled article under an
        // institutional IP is a weaker position than reading a preprint we may
        // keep, and a stronger one than knowing nothing.
        assert!(FreeToRead < SubscriptionRead);
        assert!(SubscriptionRead < LicenceStrength::Unknown);
    }

    /// The reason the ladder is not a weighted score: interleaving licence and
    /// version would file a preprint for a work whose article of record was
    /// open access, and there is no weight that fixes it, because the two axes
    /// are not the same kind of quantity.
    #[test]
    fn version_strictly_dominates_licence_so_a_cc_preprint_never_beats_a_vor() {
        use LicenceStrength::{FreeToRead, OpenLicence};
        use Version::{Preprint, VersionOfRecord};

        let resolution = rank_candidates(vec![
            candidate(RouteId::Unpaywall, Some(Preprint), OpenLicence),
            candidate(RouteId::OpenAlex, Some(VersionOfRecord), FreeToRead),
        ]);

        assert_eq!(resolution.ranked.len(), 2, "both are rankable");
        assert_eq!(
            resolution.ranked[0].candidate.route,
            RouteId::OpenAlex,
            "the VoR wins on version alone: the licence difference cannot cross it"
        );
        assert_eq!(resolution.ranked[1].candidate.route, RouteId::Unpaywall);
        assert_eq!(resolution.ranked[0].position, 1);
        assert_eq!(resolution.ranked[1].position, 2);
    }

    /// Within one version, licence decides — the other half of the ordering,
    /// and the half a pure version sort would leave arbitrary. The weakest pair
    /// is the load-bearing one: a version of record with **no** established
    /// licence still beats another version of record that shares its version,
    /// because `Unknown` is a rung and not an absence.
    #[test]
    fn an_open_licence_beats_an_unknown_one_at_the_same_version() {
        use LicenceStrength::{FreeToRead, OpenLicence, SubscriptionRead};
        use Version::VersionOfRecord;

        for (weaker, stronger) in [
            (FreeToRead, OpenLicence),
            (SubscriptionRead, OpenLicence),
            (SubscriptionRead, FreeToRead),
        ] {
            let resolution = rank_candidates(vec![
                candidate(RouteId::Publisher, Some(VersionOfRecord), weaker),
                candidate(RouteId::Unpaywall, Some(VersionOfRecord), stronger),
            ]);
            assert_eq!(
                resolution.ranked[0].candidate.licence, stronger,
                "{weaker:?} must not outrank {stronger:?} at the same version"
            );
        }
    }

    // =====================================================================
    // The unrankable candidate.
    // =====================================================================

    /// The requirement, stated as a test: a candidate you cannot rank is
    /// **labelled**, not sorted into last place.
    ///
    /// There is no `unwrap_or` here and no default: `Candidate::version` is an
    /// `Option`, and [`rank_candidates`] matches on it. A candidate that falls
    /// through to the `None` arm comes back in `unranked` with a reason, and
    /// the reason says which of the four sources was silent.
    #[test]
    fn an_unrankable_candidate_is_labelled_not_guessed_into_a_position() {
        let mut unknown = candidate(RouteId::ManualUrl, None, LicenceStrength::Unknown);
        unknown.url = "https://example.invalid/somewhere".to_string();

        let resolution = rank_candidates(vec![
            candidate(
                RouteId::Unpaywall,
                Some(Version::Preprint),
                LicenceStrength::FreeToRead,
            ),
            unknown,
            candidate(
                RouteId::Crossref,
                Some(Version::VersionOfRecord),
                LicenceStrength::OpenLicence,
            ),
        ]);

        assert_eq!(resolution.ranked.len(), 2, "the two rankable candidates");
        assert_eq!(
            resolution
                .ranked
                .iter()
                .map(|c| c.position)
                .collect::<Vec<_>>(),
            vec![1, 2],
            "positions are 1..n over the *ranked* candidates only: an unrankable \
             one does not consume a slot, because it has no slot"
        );
        assert_eq!(
            resolution.unranked.len(),
            1,
            "and the third is reported rather than dropped"
        );
        let labelled = &resolution.unranked[0];
        assert_eq!(labelled.candidate.url, "https://example.invalid/somewhere");
        assert!(
            labelled.why.contains("no registry typed it"),
            "the label says which sources were silent: {}",
            labelled.why
        );
        assert!(
            !resolution
                .ranked
                .iter()
                .any(|c| c.candidate.url == labelled.candidate.url),
            "and it is absent from the ranked list, which is the claim"
        );

        // The plan a user reads names it as unranked, and says it was not fetched.
        let lines = resolution.plan_lines();
        let not_ranked = lines
            .iter()
            .find(|line| line.contains("not ranked"))
            .expect("a not-ranked line");
        assert!(
            not_ranked.contains("example.invalid/somewhere"),
            "{not_ranked}"
        );
    }

    /// `VersionSource::None` is the **only** source that produces no version,
    /// which is what makes an unrankable candidate a fact about the pass rather
    /// than an accident of construction.
    ///
    /// The four ranking sources are pinned one at a time, and the two fallback
    /// sources are checked for the thing that distinguishes them: the DOI-prefix
    /// fallback *says* the registry was silent, and the `None` reason says the
    /// prefix implied nothing either. Both are honest sentences about what the
    /// pass failed to establish, which is what makes them worth separating.
    #[test]
    fn no_version_only_ever_comes_from_every_source_being_silent() {
        for (version, source) in [
            (
                Some(Version::Preprint),
                VersionSource::Registry {
                    registry: Registry::Crossref,
                    signal: "posted-content".to_string(),
                },
            ),
            (
                Some(Version::Preprint),
                VersionSource::Route {
                    route: RouteId::Biorxiv,
                },
            ),
            (
                Some(Version::VersionOfRecord),
                VersionSource::Registry {
                    registry: Registry::DataCite,
                    signal: "Text".to_string(),
                },
            ),
            (
                Some(Version::Preprint),
                VersionSource::DoiPrefix {
                    prefix: "1101".to_string(),
                },
            ),
            (None, VersionSource::None),
        ] {
            let mut c = candidate(RouteId::Unpaywall, version, LicenceStrength::FreeToRead);
            c.version_source = source.clone();
            let resolution = rank_candidates(vec![c]);
            assert_eq!(
                resolution.ranked.len(),
                usize::from(version.is_some()),
                "{source:?} produced version {version:?}"
            );
        }

        // And the two "nothing established it" sentences differ, because they are
        // about different things: one is a registry's silence the DOI prefix
        // covered, the other is a silence nothing covered.
        let prefix_why = VersionSource::DoiPrefix {
            prefix: "99999".to_string(),
        }
        .why();
        let none_why = VersionSource::None.why();
        assert!(prefix_why.contains("no registry typed it"), "{prefix_why}");
        assert!(prefix_why.contains("99999"), "{prefix_why}");
        assert!(none_why.contains("implies no version"), "{none_why}");
        assert_ne!(
            prefix_why, none_why,
            "two different silences, two sentences"
        );
    }

    // =====================================================================
    // The registry signal, and the recorded fallback.
    // =====================================================================

    /// A registry's own type wins over the DOI-prefix inference, and the case
    /// that makes it matter is a **real** one: a bioRxiv DOI Crossref types
    /// `journal-article` is a publisher-registered version of record under a
    /// preprint prefix, and preferring the prefix there is the bug.
    #[test]
    fn a_registry_type_signal_is_preferred_over_doi_prefix_inference() {
        // The registry says VoR; the prefix says preprint. The registry wins.
        let registry = version_from_registry(
            Registry::Crossref,
            RegistryType::VersionOfRecord,
            "journal-article",
        )
        .expect("a registry signal");
        assert_eq!(registry.0, Version::VersionOfRecord);
        assert!(matches!(registry.1, VersionSource::Registry { .. }));

        // And the same DOI prefix inference, applied on its own, gives the
        // other answer — which is why the two must be distinguishable.
        let inferred = (
            Version::Preprint,
            VersionSource::DoiPrefix {
                prefix: "1101".to_string(),
            },
        );
        assert_ne!(registry.0, inferred.0);

        // Both registries' preprint words are honoured, with the verbatim value
        // carried so a plan line can show what the registry actually said.
        for (registry_kind, word) in [
            (Registry::Crossref, "posted-content"),
            (Registry::DataCite, "Preprint"),
            (Registry::OpenAlex, "preprint"),
        ] {
            let got = version_from_registry(registry_kind, RegistryType::Preprint, word)
                .expect("a preprint signal");
            assert_eq!(got.0, Version::Preprint);
            match got.1 {
                VersionSource::Registry {
                    registry: r,
                    signal,
                } => {
                    assert_eq!(r, registry_kind);
                    assert_eq!(signal, word, "the registry's own word is carried verbatim");
                }
                other => panic!("expected a registry source, got {other:?}"),
            }
        }
    }

    /// When every registry is silent, the DOI-prefix inference is used — and
    /// **recorded**, so the fallback is inspectable rather than indistinguishable
    /// from a registry's opinion.
    ///
    /// A registry that said `dataset` or a word nobody has mapped returns
    /// `None` from [`version_from_registry`], which is precisely the condition
    /// under which the fallback is allowed to speak.
    #[test]
    fn the_fallback_is_used_and_recorded_when_the_registry_is_silent() {
        assert_eq!(
            version_from_registry(Registry::Crossref, RegistryType::Unrecognised, "thing-2027"),
            None,
            "an unmapped type is silence, not a version"
        );
        assert_eq!(
            version_from_registry(Registry::DataCite, RegistryType::NotAnArticle, "Dataset"),
            None,
            "and so is a record that is not an article: neither places the work"
        );

        // The fallback, spelled as the recorded source it is.
        let fallback = (
            Version::Preprint,
            VersionSource::DoiPrefix {
                prefix: "1101".to_string(),
            },
        );
        assert_eq!(fallback.0, Version::Preprint);
        let mut c = candidate(
            RouteId::Biorxiv,
            Some(fallback.0),
            LicenceStrength::FreeToRead,
        );
        c.version_source = fallback.1.clone();
        let resolution = rank_candidates(vec![c]);
        let reason = resolution.ranked[0].candidate.version_source.why();
        assert!(
            reason.contains("no registry typed it") && reason.contains("1101"),
            "the plan line must say the prefix did the work, not a registry: {reason}"
        );
        assert_eq!(
            resolution.ranked[0].rank.version,
            Version::Preprint,
            "and the inferred version is a real rank, not a guess about placement"
        );
    }

    // =====================================================================
    // The licence axis.
    // =====================================================================

    /// A registry's licence decides the licence axis outright; a route's own
    /// basis is the weaker fallback; and nothing at all is `Unknown` rather than
    /// a default.
    #[test]
    fn licence_strength_prefers_a_registry_grant_then_the_route_basis() {
        let offer = LicenceOffer {
            url: "https://creativecommons.org/licenses/by/4.0/".to_string(),
            content_version: Some("vor".to_string()),
            start: None,
            delay_in_days: None,
            registry: Registry::Crossref,
        };

        // A grant that counts for this version wins even against a route that
        // would otherwise be `FreeToRead`.
        assert_eq!(
            licence_strength(
                std::slice::from_ref(&offer),
                Some(Version::VersionOfRecord),
                RouteId::Arxiv,
                LicenceFloor::RouteBasis,
            ),
            LicenceStrength::OpenLicence
        );
        // A grant that does *not* count for this version is not an offer, and
        // the caller is responsible for having filtered — `a_vo`-only grant
        // against a preprint candidate.
        let refused = LicenceOffer {
            content_version: Some("vor".to_string()),
            ..offer
        };
        assert!(
            refused
                .counts_for(
                    "preprint",
                    chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap_or_default()
                )
                .is_some(),
            "the registry module refuses the grant for a preprint, so the caller \
             never passes it here"
        );

        // No offer: the route's own basis, which is what `access_basis` records.
        for (route, expected) in [
            (RouteId::Biorxiv, LicenceStrength::FreeToRead),
            (RouteId::Arxiv, LicenceStrength::FreeToRead),
            (RouteId::Osti, LicenceStrength::FreeToRead),
            (RouteId::Unpaywall, LicenceStrength::FreeToRead),
            (RouteId::Publisher, LicenceStrength::SubscriptionRead),
            (RouteId::ManualUrl, LicenceStrength::SubscriptionRead),
            (RouteId::ElsevierTdm, LicenceStrength::Unknown),
        ] {
            assert_eq!(
                licence_strength(
                    &[],
                    Some(Version::VersionOfRecord),
                    route,
                    LicenceFloor::RouteBasis
                ),
                expected,
                "{route}: the route's own basis is the fallback, and it is not \
                 stronger than its `access_basis` says"
            );
        }
        // And a route that establishes no basis establishes no licence.
        assert_eq!(
            licence_strength(
                &[],
                Some(Version::VersionOfRecord),
                RouteId::Crossref,
                LicenceFloor::RouteBasis,
            ),
            LicenceStrength::Unknown,
            "`RouteId::access_basis` is None for the discovery routes, and the \
             ranker must not invent a basis for them"
        );
    }

    /// `access_basis` on both ends: a strength maps to a migration-013 value or
    /// to `None`, and `Unknown` must be the `None` case because the column is
    /// `NOT NULL` and "we did not look" has no spelling.
    #[test]
    fn licence_strength_maps_onto_the_migration_access_basis_vocabulary() {
        assert_eq!(
            LicenceStrength::OpenLicence.access_basis(),
            Some("oa_license")
        );
        assert_eq!(
            LicenceStrength::FreeToRead.access_basis(),
            Some("oa_license")
        );
        assert_eq!(
            LicenceStrength::SubscriptionRead.access_basis(),
            Some("subscription_read")
        );
        assert_eq!(
            LicenceStrength::Unknown.access_basis(),
            None,
            "`Unknown` has no column value: `RouteId::access_basis` returns \
             `Option` for the same reason"
        );
        // And every value it does produce is one `RouteId::access_basis` also
        // produces, so the two cannot diverge.
        for strength in [
            LicenceStrength::OpenLicence,
            LicenceStrength::FreeToRead,
            LicenceStrength::SubscriptionRead,
        ] {
            let ours = strength.access_basis().expect("a column value");
            assert!(
                RouteId::ALL
                    .iter()
                    .any(|route| route.access_basis() == Some(ours)),
                "{strength:?} writes {ours}, which no route supports"
            );
        }
    }

    // =====================================================================
    // The metadata pass's own contract.
    // =====================================================================

    /// The acceptance criterion as a constant: four buckets, none of which
    /// serves articles. A publisher bucket in this list would be a
    /// resolve-then-rank that resolves *through* the publisher, which is the
    /// behaviour being replaced.
    #[test]
    fn the_metadata_pass_is_allowed_exactly_four_buckets() {
        assert_eq!(
            METADATA_BUCKETS,
            ["openalex", "crossref", "datacite", "unpaywall"],
            "ADR-007 §3: 'It may hit OpenAlex, Crossref, DataCite, Unpaywall'"
        );
        for bucket in METADATA_BUCKETS {
            assert!(
                !bucket.contains("elsevier")
                    && !bucket.contains("springer")
                    && !bucket.contains("wiley")
                    && !bucket.contains("doi.org"),
                "{bucket} is not a metadata registry"
            );
        }
    }

    /// The three versions, and their column spellings, are ADR's and
    /// migration 013's — not this module's invention.
    #[test]
    fn the_version_vocabulary_is_the_migration_vocabulary() {
        const MIGRATION_013: [&str; 4] = ["vor", "am", "preprint", "unknown"];
        for (version, label) in Version::ALL
            .map(|v| (v, v.label()))
            .into_iter()
            .filter(|(version, _)| !matches!(version, Version::Unstated))
        {
            assert!(
                MIGRATION_013.contains(&label),
                "{version:?} would write {label:?}"
            );
        }
        assert_eq!(
            Version::ALL.map(|v| v.label()),
            ["vor", "am", "preprint", "unstated"],
            "ADR-007 §3's ladder, best first, with the bottom rung for a version \
             nothing stated"
        );
        // And that bottom rung is deliberately **not** a column value, which is
        // what keeps `RouteId::artefact_version`'s `unknown` meaning "no route
        // established a version" rather than "a place was named and it did not
        // say". `unknown` is still migration 013's own word for the former.
        assert!(
            !MIGRATION_013.contains(&Version::Unstated.label()),
            "`unstated` is a plan line, not a column value: collapsing it into \
             `unknown` would make 'we looked and it did not say' read as 'we \
             never looked'"
        );
        assert_eq!(
            RouteId::Arxiv.artefact_version(),
            Some(scitadel_core::models::ArtefactVersion::Preprint)
        );
        assert_eq!(
            RouteId::Unpaywall.artefact_version(),
            Some(scitadel_core::models::ArtefactVersion::Unknown),
            "and the fallback the storage precedence consults when the ranking \
             established nothing"
        );
    }

    /// Two candidates that compare equal keep the order the metadata pass
    /// produced them in, which is ADR-007 §3's own chain order. Without this
    /// the tiebreak would be `sort`'s arbitrary behaviour, and "the first
    /// candidate OpenAlex named" would become the ranking rule by accident.
    #[test]
    fn equal_ranked_candidates_keep_the_metadata_pass_order() {
        let a = candidate(
            RouteId::OpenAlex,
            Some(Version::VersionOfRecord),
            LicenceStrength::FreeToRead,
        );
        let mut b = candidate(
            RouteId::Unpaywall,
            Some(Version::VersionOfRecord),
            LicenceStrength::FreeToRead,
        );
        b.url = "https://example.invalid/second".to_string();

        let forward = rank_candidates(vec![a.clone(), b.clone()]);
        assert_eq!(
            forward.ranked[0].candidate.url, a.url,
            "two equal candidates keep the pass's order"
        );
        let reversed = rank_candidates(vec![b, a]);
        assert_eq!(
            reversed.ranked[0].candidate.url, "https://example.invalid/second",
            "and reversing the pass's order reverses the tiebreak: the order is the \
             pass's, not the route's and not `sort`'s"
        );
    }

    /// A resolution with nothing rankable in it is a resolution that fetches
    /// nothing, and says so — rather than falling back to "the first candidate
    /// we found", which is the behaviour this whole module replaces.
    #[test]
    fn a_resolution_with_no_rankable_candidate_chooses_nothing() {
        let resolution = rank_candidates(vec![
            candidate(RouteId::ManualUrl, None, LicenceStrength::Unknown),
            candidate(RouteId::Crossref, None, LicenceStrength::Unknown),
        ]);
        assert!(resolution.chosen().is_none(), "nothing to fetch");
        assert_eq!(resolution.ranked.len(), 0);
        assert_eq!(resolution.unranked.len(), 2, "both reported");
        assert_eq!(resolution.plan_lines().len(), 2, "and both printed");
    }

    // =====================================================================
    // The metadata pass, over four `wiremock` registries and a recording pacer.
    //
    // These are the two claims that are about the *pass* rather than about the
    // ranker, and neither can be checked from the ranker's own types: what hosts
    // went on the wire, and how many times.
    // =====================================================================

    /// Grants everything and records every request, by bucket.
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

    fn recording_client(
        pacer: &Arc<RecordingPacer>,
        table: scitadel_http::BucketPolicyTable,
    ) -> PacedClient {
        PacedClient::new(
            PacedClient::default_transport().expect("transport"),
            Arc::clone(pacer) as Arc<dyn Pacer>,
            table,
        )
    }

    /// Four `wiremock` servers, one per registry, each answering with that
    /// registry's own envelope.
    ///
    /// **Four servers, not one**, and that is the point: `scitadel-http`'s policy
    /// table keys on **host**, so a single server cannot host two platforms and a
    /// path-prefix "route" would test a table production never builds. Production
    /// separates these by host too — `api.openalex.org`, `api.crossref.org`,
    /// `api.datacite.org`, `api.unpaywall.org` — so this reproduces the real
    /// separation instead of simulating it, which is what makes
    /// `the_metadata_pass_touches_no_publisher_host` a claim about hosts.
    struct FourRegistries {
        servers: Vec<MockServer>,
        table: scitadel_http::BucketPolicyTable,
    }

    impl FourRegistries {
        async fn new() -> Self {
            let mut servers: Vec<MockServer> = Vec::new();
            for _ in 0..4 {
                servers.push(MockServer::start().await);
            }
            let mut table = scitadel_http::BucketPolicyTable::new();
            for (server, bucket) in servers.iter().zip(METADATA_BUCKETS) {
                table.route(&format!("127.0.0.1:{}", server.address().port()), bucket);
            }
            Self { servers, table }
        }

        fn bases(&self) -> MetadataBases {
            MetadataBases {
                openalex_works: self.servers[0].uri(),
                crossref_works: self.servers[1].uri(),
                datacite_dois: self.servers[2].uri(),
                unpaywall_api: self.servers[3].uri(),
            }
        }

        /// The authority each server actually listens on — `host:port`, which is
        /// what ADR-007 §4's policy table keys on and what
        /// [`MetadataBases::allows`] compares.
        ///
        /// Read from the **bound socket** rather than from the recorded
        /// requests, because `wiremock::Request::url` is a path-only URL: it has
        /// no authority at all, so a claim about which hosts answered cannot be
        /// read off it. The request *counts* still come from wiremock, and the
        /// pair together is what the assertion needs: "these four authorities
        /// were asked" and "no other authority was asked".
        fn authorities(&self) -> Vec<String> {
            let mut out: Vec<String> = self
                .servers
                .iter()
                .map(|server| format!("127.0.0.1:{}", server.address().port()))
                .collect();
            out.sort();
            out
        }

        /// How many requests each server received, in server order.
        async fn request_counts(&self) -> Vec<usize> {
            let mut counts = Vec::with_capacity(self.servers.len());
            for server in &self.servers {
                counts.push(server.received_requests().await.expect("recorded").len());
            }
            counts
        }

        /// Every bucket a request was charged to.
        fn buckets_spent(pacer: &RecordingPacer) -> Vec<String> {
            let mut buckets: Vec<String> = pacer
                .0
                .lock()
                .expect("lock")
                .iter()
                .map(|(bucket, _)| bucket.clone())
                .collect();
            buckets.sort();
            buckets
        }
    }

    fn pass_for(registries: &FourRegistries) -> MetadataPass {
        MetadataPass::with_bases(
            registries.bases(),
            CandidateSources {
                // Named, never requested during the pass — and pointed at hosts
                // that would be unmistakable on the wire if they *were*.
                preprint: crate::preprint::PreprintBases {
                    biorxiv_content: "http://127.0.0.1:9/content".to_string(),
                    arxiv_pdf: "http://127.0.0.1:9/pdf".to_string(),
                },
                osti_base: "http://127.0.0.1:9".to_string(),
                doi_resolver: "http://127.0.0.1:9/doi".to_string(),
                arxiv_pdf: "http://127.0.0.1:9/pdf".to_string(),
                openalex_api: registries.servers[0].uri(),
            },
            scitadel_core::config::OpenAlexAuth::default(),
            "polite@example.org",
        )
    }

    /// **The acceptance criterion: the metadata pass touches no publisher host.**
    ///
    /// Asserted three ways, and each catches a different failure: on the hosts
    /// the servers recorded (the criterion itself), on the buckets the pacer was
    /// charged (the *budgets*, so it cannot be satisfied by a request that spent
    /// someone else's allowance), and on
    /// [`crate::resolve::MetadataBases::allows`] — the check that makes the first
    /// two true by construction rather than by diligence.
    ///
    /// The fixture's preprint, OSTI and DOI-resolver bases point at a port nothing
    /// listens on, so a pass that *did* try to name-check one would fail the whole
    /// test rather than silently succeed against a live host.
    #[tokio::test]
    async fn the_metadata_pass_touches_no_publisher_host() {
        let registries = FourRegistries::new().await;
        // Each registry answers 404 — the pass collects no candidates, and the
        // question here is only which hosts it asked.
        for server in &registries.servers {
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        let pacer = Arc::new(RecordingPacer::default());
        let client = recording_client(&pacer, registries.table.clone());

        let resolution = pass_for(&registries)
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1101/2025.06.14.659707"),
                    arxiv_id: Some("2301.00001"),
                    osti_id: Some("1234567"),
                    url: Some("https://example.invalid/paper"),
                    openalex_id: Some("W1"),
                },
            )
            .await;

        // The pass's own record of which authorities it contacted, against the
        // four the servers actually listen on. Comparing the two rather than
        // trusting either alone is what makes this a measurement: a publisher
        // host would be an *extra* entry here, not a missing one.
        assert_eq!(
            resolution
                .hosts_contacted
                .iter()
                .cloned()
                .collect::<Vec<String>>(),
            registries.authorities(),
            "the pass contacted exactly the four registry authorities and nothing \
             else — a publisher host would appear as a fifth: {:?}",
            resolution.hosts_contacted
        );
        assert!(
            resolution.hosts_contacted.iter().all(|host| {
                !host.contains(":9") && !host.contains("doi.org") && !host.contains("biorxiv")
            }),
            "and none of them is a preprint server, the DOI resolver or OSTI — \
             whose bases this fixture points at port 9, which nothing listens on: {:?}",
            resolution.hosts_contacted
        );

        // Every registry was asked, and only the registries. Five hops for four
        // registries, and the fifth is OpenAlex asked twice: the by-DOI read
        // 404'd, so the by-id read ran as well — one work, one OpenAlex budget,
        // two requests, and both of them `Meta`.
        let registries_consulted: std::collections::BTreeSet<Registry> = resolution
            .consulted
            .iter()
            .map(|hop| hop.registry)
            .collect();
        assert_eq!(
            registries_consulted,
            Registry::ALL
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "all four registries, in ADR-007 §3's order: {:?}",
            resolution.consulted
        );
        assert_eq!(
            resolution.consulted.len(),
            5,
            "four registries, five hops: OpenAlex by DOI and by id, Crossref, \
             DataCite, Unpaywall — and nothing else: {:?}",
            resolution.consulted
        );
        assert_eq!(
            resolution
                .consulted
                .iter()
                .filter(|hop| hop.registry == Registry::OpenAlex)
                .count(),
            2,
            "the two OpenAlex reads are two `Meta` requests out of one bucket, \
             which is why `hosts_contacted` is a set and not a list"
        );
        assert_eq!(
            registries.request_counts().await,
            vec![2, 1, 1, 1],
            "every server was asked, and only OpenAlex twice: {:?}",
            registries.request_counts().await
        );

        // The budgets are the sharper half: no publisher bucket was spent.
        let buckets = FourRegistries::buckets_spent(&pacer);
        assert_eq!(
            buckets,
            vec![
                "crossref".to_string(),
                "datacite".to_string(),
                "openalex".to_string(),
                "openalex".to_string(),
                "unpaywall".to_string(),
            ],
            "five `Request` permits — one per hop, OpenAlex's two out of one \
             bucket — and **no publisher bucket at all**: {buckets:?}"
        );
        let distinct: std::collections::BTreeSet<&str> =
            buckets.iter().map(String::as_str).collect();
        assert_eq!(
            distinct,
            METADATA_BUCKETS
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            "and the distinct buckets are exactly ADR-007 §3's four metadata \
             registries: {buckets:?}"
        );
        for (bucket, tier) in pacer.0.lock().expect("lock").iter() {
            assert_eq!(*tier, PaceTier::Meta, "{bucket} must be metadata work");
        }

        // And the guard that makes the first two true by construction.
        for registry in Registry::ALL {
            assert!(
                !registries.bases().allows(
                    registry,
                    &reqwest::Url::parse("http://127.0.0.1:9/content/x.full.pdf").expect("a url")
                ),
                "{registry}'s base must refuse a preprint-server URL, which is \\
                 what makes 'the pass cannot reach a publisher' a property of \\
                 [`MetadataBases::allows`] rather than of this test"
            );
        }
        assert!(
            registries.bases().allows(
                Registry::OpenAlex,
                &reqwest::Url::parse(&format!("{}/doi:10.1101/x", registries.servers[0].uri()))
                    .expect("a url")
            ),
            "and it still allows its own registry's URL, so the guard is not \\
             simply refusing everything"
        );
    }

    /// **No malformed URL can reach the candidate set**, which is the structural
    /// form of the `unpaywall_json("{}")` fixture bug: a location whose
    /// `url_for_pdf` is a *relative* path used to become a candidate, and under
    /// resolve-then-rank the candidate it became was the **ranked choice**, so the
    /// failure moved from "this leg missed" to "the walk ended on a URL that
    /// cannot be fetched".
    ///
    /// Asserted on the guard itself and then through the pass, because the two
    /// claims are different: the guard rejects a shape, and the pass's four
    /// collection sites all go through it. The `data:`/`file:` cases matter more
    /// than the relative one — those *parse*, so a parse-only check would pass
    /// them, and a `file://` URL is a local file read wearing a network budget.
    #[tokio::test]
    async fn a_malformed_location_url_is_never_offered_as_a_candidate() {
        for junk in [
            "{}",
            "",
            "   ",
            "not a url",
            "/relative/path.pdf",
            "pdf/relative.pdf",
            "://missing-scheme",
            // These three **parse**. A "does it parse" check is not enough.
            "data:application/pdf;base64,JVBERi0=",
            "file:///etc/passwd",
            "mailto:someone@example.org",
        ] {
            assert!(
                !is_fetchable_url(junk),
                "{junk:?} must not be offered as a fetchable location"
            );
        }
        for good in [
            "https://example.org/p.pdf",
            "http://127.0.0.1:8080/p.pdf",
            "  https://example.org/p.pdf  ",
        ] {
            assert!(is_fetchable_url(good), "{good:?} is fetchable");
        }

        // And through the pass: Unpaywall naming a relative location yields no
        // candidate at all, rather than a ranked one.
        let registries = FourRegistries::new().await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("^/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"best_oa_location":{"url_for_pdf":"{}","is_oa":true}}"#),
            )
            .mount(&registries.servers[3])
            .await;
        let pacer = Arc::new(RecordingPacer::default());
        let client = recording_client(&pacer, registries.table.clone());
        let resolution = pass_for(&registries)
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.18434/m32154"),
                    ..WorkRefs::default()
                },
            )
            .await;

        assert!(
            resolution
                .candidates()
                .all(|candidate| is_fetchable_url(&candidate.url)),
            "every candidate the pass offers has a fetchable URL: {:?}",
            resolution.plan_lines()
        );
        assert!(
            !resolution.candidates().any(|c| c.url == "{}"),
            "and the junk value is not among them: {:?}",
            resolution.plan_lines()
        );
    }

    /// **Exactly one fetch happens after ranking.**
    ///
    /// Not "the fetch succeeded" and not "the walk reached the right place": the
    /// claim is about the *count*, because a walk is exactly the shape that
    /// produced the bug. The fixture mounts four full-text locations — two
    /// registries' OA copies, a preprint server's verified URL and a landing
    /// page — and every one of them **answers**, so a fall-through implementation
    /// would return after its first attempt and this test would still see one
    /// request. To catch a fall-through the winning candidate must 404 *and* a
    /// lower-ranked one must answer, which is the second half of the test: one
    /// attempt, no bytes, and the lower-ranked PDF untouched.
    #[tokio::test]
    async fn exactly_one_fetch_happens_after_ranking() {
        use LicenceStrength::OpenLicence;

        // ---- (a) the winner answers: one request, and it is the top-ranked ----
        let first = FourRegistries::new().await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/vor.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("%PDF-1.7\n"),
            )
            .mount(&first.servers[3])
            .await;
        // A preprint copy, lower-ranked, also answering.
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/preprint.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("%PDF-1.7\n"),
            )
            .mount(&first.servers[3])
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/dc/10.1101%2Fx"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"data":{"attributes":{"titles":[{"title":"A work"}],
                           "publicationYear":2024,
                           "types":{"resourceTypeGeneral":"Preprint"},
                           "rightsList":[{"rightsUri":"https://creativecommons.org/licenses/by/4.0/"}],
                           "creators":[{"name":"Young, Christopher J.","familyName":"Young"}]}}}"#,
                    ),
            )
            .mount(&first.servers[2])
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("^/10[.]1101/x"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"best_oa_location":{"url_for_pdf":"VOR","version":"publishedVersion","is_oa":true},
                            "oa_locations":[
                              {"url_for_pdf":"VOR","version":"publishedVersion","is_oa":true},
                              {"url_for_pdf":"PREPRINT","version":"submittedVersion","is_oa":true}]}"#
                            .replace("VOR", &format!("{}/vor.pdf", first.servers[3].uri()))
                            .replace("PREPRINT", &format!("{}/preprint.pdf", first.servers[3].uri()))
                            .as_str(),
                    ),
            )
            .mount(&first.servers[3])
            .await;

        let pacer = Arc::new(RecordingPacer::default());
        let client = recording_client(&pacer, first.table.clone());
        let resolution = pass_for(&first)
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1101/x"),
                    ..WorkRefs::default()
                },
            )
            .await;

        assert!(
            resolution.ranked.len() >= 3,
            "the pass found several candidates: {:?}",
            resolution.plan_lines()
        );
        assert_eq!(
            resolution.chosen().expect("a choice").candidate.url,
            format!("{}/vor.pdf", first.servers[3].uri()),
            "the version of record is first: {:?}",
            resolution.plan_lines()
        );
        // DataCite's CC grant reached the licence axis — for the candidates whose
        // version it covers. The chosen VoR candidate is `FreeToRead`, not
        // `OpenLicence`, because the grant DataCite published was typed
        // `Preprint` on a record whose *type* is `Preprint`, and a grant that
        // comes with a location that is not ours to apply is not applied. What
        // matters here is that it did not move the *version*, which is the
        // dominance claim: the VoR stayed first.
        assert_eq!(
            resolution
                .ranked
                .iter()
                .filter(|entry| entry.rank.licence == OpenLicence)
                .count(),
            0,
            "and no candidate claimed a CC grant the registries did not tie to \
             it: {:?}",
            resolution.plan_lines()
        );
        assert_eq!(
            resolution.ranked[0].rank.version,
            Version::VersionOfRecord,
            "the dominance claim in one assertion: a version of record stated by \
             Unpaywall's own location word outranks a preprint stated by the DOI \
             prefix, and every candidate below it is a preprint: {:?}",
            resolution.plan_lines()
        );
        assert!(
            resolution
                .ranked
                .iter()
                .skip(1)
                .all(|entry| entry.rank.version == Version::Preprint),
            "and the four lower-ranked candidates are all preprints — the \
             repo's own `submittedVersion` copy and the two bioRxiv transforms"
        );

        // ---- (b) the winner 404s: still one attempt, and the next is untouched ----
        let second = FourRegistries::new().await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/vor.pdf"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&second.servers[3])
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/preprint.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("%PDF-1.7\n"),
            )
            .mount(&second.servers[3])
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("^/10[.]1101/x"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"best_oa_location":{"url_for_pdf":"VOR","version":"publishedVersion","is_oa":true},
                            "oa_locations":[
                              {"url_for_pdf":"VOR","version":"publishedVersion","is_oa":true},
                              {"url_for_pdf":"PREPRINT","version":"submittedVersion","is_oa":true}]}"#
                            .replace("VOR", &format!("{}/vor.pdf", second.servers[3].uri()))
                            .replace("PREPRINT", &format!("{}/preprint.pdf", second.servers[3].uri()))
                            .as_str(),
                    ),
            )
            .mount(&second.servers[3])
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/cr/10.1101%2Fx"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"status":"ok","message":{"type":"posted-content","title":["A work"]}}"#,
                    ),
            )
            .mount(&second.servers[1])
            .await;

        let chosen = {
            let pacer = Arc::new(RecordingPacer::default());
            let client = recording_client(&pacer, second.table.clone());
            let resolution = pass_for(&second)
                .resolve(
                    &client,
                    &WorkScope::new(),
                    WorkRefs {
                        doi: Some("10.1101/x"),
                        ..WorkRefs::default()
                    },
                )
                .await;
            // The attempt. In production this is
            // `PaperDownloader::fetch_candidate`, called **once**, on
            // `resolution.chosen()`; here it is the same URL under the same
            // conditions, so the count is the claim and not a re-implementation.
            let chosen = resolution.chosen().expect("a choice").clone();
            let refused = client
                .get(
                    reqwest::Url::parse(&chosen.candidate.url).expect("a url"),
                    PaceTier::Oa,
                    scitadel_http::SafeHeaders::unauthenticated(),
                )
                .await
                .is_err();
            assert!(refused, "the ranked candidate refused");
            let asked = second.servers[3]
                .received_requests()
                .await
                .expect("recorded");
            let paths: Vec<&str> = asked.iter().map(|r| r.url.path()).collect();
            assert_eq!(
                paths.len(),
                2,
                "the metadata pass's one Unpaywall lookup plus the one fetch — a \
                 fall-through would have made it three: {paths:?}"
            );
            assert!(
                !paths.iter().any(|path| path.contains("preprint")),
                "and the lower-ranked preprint copy, which **answers**, was never \
                 requested: {paths:?}"
            );
            chosen
        };
        assert!(
            chosen.candidate.url.contains("/vor.pdf"),
            "and the one attempt was the top-ranked candidate: {}",
            chosen.candidate.url
        );
    }
}
