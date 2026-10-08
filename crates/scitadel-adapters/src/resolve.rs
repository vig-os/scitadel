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
//! [`VersionSource`] is the provenance of a [`Version`] claim, in strict
//! preference order:
//!
//! | order | source | what it is |
//! |---|---|---|
//! | 1 | [`VersionSource::Repository`] | Europe PMC's manuscript flags, or the PMC OA dataset's per-version `is_manuscript` |
//! | 2 | [`VersionSource::LocationVersion`] | a location's own `version` word — OpenAlex, Unpaywall, Crossref |
//! | 3 | [`VersionSource::Registry`] | the registry's own `type` / `resourceTypeGeneral` |
//! | 4 | [`VersionSource::Route`] | the route serves preprints by construction |
//! | 5 | [`VersionSource::DoiPrefix`] | the DOI prefix, the #260 fallback |
//! | — | [`VersionSource::Unstated`] | a source named a copy and declined to characterise it |
//! | — | [`VersionSource::None`] | nothing established it — see below |
//!
//! **The repository row is first, and it is a different kind of claim.** Rows 2–5
//! are all statements about a *work* or about a *URL an index catalogued*; row 1
//! is a statement about **the file we would fetch** — `is_manuscript: false` is
//! PMC saying this object is the typeset article, which no DOI registry can say
//! and no DOI prefix can imply. It outranks them because it answers the question
//! the ranker is asking; see [`version_from_repository`] for why it is a
//! separate variant rather than a widened [`version_from_location_word`].
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
    /// A biomedical repository said which **copy it holds** — Europe PMC's
    /// `*AuthMan` flags, or the PMC OA dataset's per-version `is_manuscript`.
    ///
    /// A separate variant rather than a widened [`Self::LocationVersion`], and
    /// the reason is that the two are different kinds of claim.
    /// `LocationVersion` is a *word from a shared three-word vocabulary*, which
    /// OpenAlex and Unpaywall spell identically because Unpaywall's location
    /// shape is OpenAlex's. A repository flag is not a fourth word in that
    /// vocabulary: it is a boolean about one stored file, from a service with a
    /// different field name, and it answers a **stronger** question — not "how
    /// does this index classify that URL" but "is this file the typeset
    /// article". `VersionSource` exists to say which of those produced a
    /// claim, so the two cannot share a variant.
    ///
    /// `signal` carries the field and its value verbatim (`is_manuscript=false`,
    /// `source=PPR`, `no manuscript flag is set`) so a plan line names what was
    /// read, which is what makes the claim checkable against PMC rather than
    /// against us.
    Repository { registry: Registry, signal: String },
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
            Self::Repository { registry, signal } => {
                format!("{registry} says `{signal}`")
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

/// A biomedical repository's statement about the copy it will serve.
///
/// **This is the (a) branch of #254's wave-3 choice**, and it is (a)
/// because of what the alternative would mean. Option (b) was to extend
/// [`version_from_location_word`] to recognise a manuscript signal — and
/// that function's whole contract, stated at its definition, is that its
/// input is "the same three words in both indexes, measured". Europe PMC's
/// `epmcAuthMan: Y` and PMC-OA's `is_manuscript: false` are not a fourth
/// word in that vocabulary; they are **booleans about a copy**, in two
/// different vocabularies, from two services that do not share a field
/// name. Widening the word table to hold them would make the table a
/// mixture of words and flags and would leave the *comparator* looking at
/// three-sources-agree spelling for a claim one source made.
///
/// So the chain gains a **variant** instead, and it is a variant rather
/// than a reuse of [`Self::LocationVersion`] for the same reason: the
/// provenance differs in kind, not in wording. `LocationVersion` is an
/// index saying `publishedVersion` about a URL it indexed — a word from a
/// shared vocabulary. A repository saying `is_manuscript: false` is saying
/// something strictly stronger: *this file, in this archive, is the
/// typeset article*. Keeping it in its own variant means the `why()` a plan
/// line prints names the field that was read, so a reader can check PMC's
/// answer rather than ours.
///
/// The [`Rank`] comparator is untouched, and that is the property option (a)
/// buys: [`Version`] and [`LicenceStrength`] keep their meaning and their
/// `Ord`, so `ranked_order_is_documented_and_pinned` still pins the same
/// claim it pinned before PMC-OA existed.
///
/// # The `Unstated` case is the one that matters
///
/// A Europe PMC `MED` record with every manuscript flag `N` maps to
/// [`RepositoryVersion::Unstated`], and this function turns that into
/// `(Version::Unstated, …)` — **a rung, not a hole**. It is *not* `None`:
/// Europe PMC named a real, fetchable copy and merely did not say which
/// version it is, which is a positive statement about a location. Mapping
/// that silence to `Version::VersionOfRecord` would file whatever came back
/// as the article of record on nobody's word, and returning `None` would say
/// Europe PMC vouched for nothing at all, which is false — it vouched for
/// the copy and declined to characterise it. Both errors are overclaims, in
/// opposite directions, and #261 is about both.
#[must_use]
pub fn version_from_repository(
    registry: Registry,
    stated: crate::registry::RepositoryVersion,
) -> (Version, VersionSource) {
    let version = stated.as_version();
    let source = VersionSource::Repository {
        registry,
        signal: match stated {
            crate::registry::RepositoryVersion::VersionOfRecord => "is_manuscript=false",
            crate::registry::RepositoryVersion::AuthorManuscript => "is_manuscript=true",
            crate::registry::RepositoryVersion::Preprint => "source=PPR",
            crate::registry::RepositoryVersion::Unstated => "no manuscript flag is set",
        }
        .to_string(),
    };
    (version, source)
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

/// ADR-007 §3's fetch order, as one comparable position on a ladder.
///
/// **This is the ADR's own ordering, which the ranker was not implementing.**
/// ADR-007 §3 says, under "Fetch order within the ranked candidates":
///
/// 1. Europe PMC: `fullTextXML` (JATS) and `supplementaryFiles` …
/// 2. The PMC OA dataset (`pmc-oa-opendata` S3, anonymous HTTPS) …
/// 3. OA repository locations from OpenAlex and Unpaywall, arXiv for
///    preprints, and DOE OSTI for national-lab reports.
/// 4. SI discovery …
/// 5. Tier 2 (sanctioned TDM) …
/// 6. the publisher landing page ("for OA works only")
/// 7. the browser session (last resort)
///
/// Two candidates that tie on version *and* licence are the same document of the
/// same strength, and the ADR has already decided between them. The ranker had no
/// notion of the order at all, so the winner was whichever registry the pass
/// happened to ask first — and on #260's probe table that was the wrong one every
/// time: a PMC-OA copy sat below the publisher's own copy of the same VoR, and
/// the publisher's copy was a bot wall.
///
/// Derived `Ord` follows **declaration order**, which is why the variants are
/// declared in §3's numbering. [`every_route_id_is_placed_on_the_adr_fetch_order`]
/// pins that against the ADR's own digits rather than against this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchStep {
    /// §3 step 1.
    EuropePmc,
    /// §3 step 2.
    PmcOa,
    /// §3 step 3.
    RepositoryLocation,
    /// §3 step 4.
    SupplementDiscovery,
    /// §3 step 5.
    SanctionedTdm,
    /// §3 step 6.
    PublisherLandingPage,
    /// §3 step 7, the last resort.
    BrowserSession,
    /// **Not a rung on §3's ladder** — deliberately the weakest position, and
    /// the only one with no ADR step number.
    ///
    /// It holds a route that records where a file came from rather than where to
    /// fetch it: `RouteId::Legacy`, `RouteId::ImportFlat` and `RouteId::Manual`,
    /// which [`RouteId::never_fetches`] is the existing name for. There is no
    /// honest middle rung for "this is not a route at all", so these land last —
    /// and the alternative, inventing a rung between two numbered steps, would be
    /// claiming an ADR position the ADR does not take.
    ///
    /// §3 step 8 is the ladder running out ("otherwise the work lands in
    /// `needs_ill`…"), which is an outcome rather than a route, so it is not a
    /// variant here either.
    NotAFetchRoute,
}

impl FetchStep {
    /// Every rung, in §3's order, strongest first.
    pub const ALL: [Self; 8] = [
        Self::EuropePmc,
        Self::PmcOa,
        Self::RepositoryLocation,
        Self::SupplementDiscovery,
        Self::SanctionedTdm,
        Self::PublisherLandingPage,
        Self::BrowserSession,
        Self::NotAFetchRoute,
    ];

    /// ADR-007 §3's own step number, or `0` for the rung that is not on the
    /// ladder. The `0` is what keeps [`Self::NotAFetchRoute`] from being
    /// presented as something the ADR numbered.
    #[must_use]
    pub fn adr_step(self) -> u8 {
        match self {
            Self::EuropePmc => 1,
            Self::PmcOa => 2,
            Self::RepositoryLocation => 3,
            Self::SupplementDiscovery => 4,
            Self::SanctionedTdm => 5,
            Self::PublisherLandingPage => 6,
            Self::BrowserSession => 7,
            Self::NotAFetchRoute => 0,
        }
    }

    /// The phrase a plan line uses, so a reader can see which rung decided a
    /// placement without reading the ADR.
    #[must_use]
    pub fn why(self) -> String {
        match self {
            Self::EuropePmc => String::from(
                "ADR-007 §3 step 1: Europe PMC, which serves the full text and its \
                 supplementary files itself",
            ),
            Self::PmcOa => String::from(
                "ADR-007 §3 step 2: the PMC OA dataset, anonymous HTTPS with no \
                 publisher in the path",
            ),
            Self::RepositoryLocation => {
                String::from("ADR-007 §3 step 3: an OA repository's own copy of the work")
            }
            Self::SupplementDiscovery => String::from(
                "ADR-007 §3 step 4: supplementary material, which travels separately \
                 from the article",
            ),
            Self::SanctionedTdm => String::from(
                "ADR-007 §3 step 5: a publisher's sanctioned text-and-data-mining \
                 endpoint",
            ),
            Self::PublisherLandingPage => String::from(
                "ADR-007 §3 step 6: the publisher's own landing page, which is a \
                 document of last resort",
            ),
            Self::BrowserSession => String::from(
                "ADR-007 §3 step 7: a human's browser session, the last resort of \
                 the last resort",
            ),
            Self::NotAFetchRoute => String::from(
                "not on ADR-007 §3's fetch order: this route records where a file \
                 came from rather than where to fetch it",
            ),
        }
    }
}

/// Where a candidate sits on [`FetchStep`], from its route and its URL.
///
/// **Route-primary, URL as the override, and only for the three routes that are
/// identity sources.** `RouteId::Crossref`, `RouteId::DataCite` and
/// `RouteId::OpenAlex` answer "what does this DOI refer to"; a candidate they
/// name can be a repository location (step 3), the PMC-OA dataset (step 2) or a
/// publisher's page (step 6), and the route says nothing about which. So for
/// those three the URL decides — but only for hosts the ADR itself names, because
/// a host list of "repositories we recognise" would be a reliability heuristic
/// wearing a typing hat, and #260 is not a licence to invent one.
///
/// [`the_step_is_derived_from_the_url_where_the_route_is_only_an_identity_source`]
/// pins every branch, including the one that is a **known limitation**: an
/// index-named URL on a publisher's own host stays step 3, because nothing in the
/// URL says it is a publisher.
#[must_use]
pub fn fetch_step_for(route: RouteId, url: &str) -> FetchStep {
    use RouteId as R;

    // The three identity sources first, and only for hosts ADR-007 §3 names.
    if matches!(
        route,
        R::Crossref | R::DataCite | R::OpenAlex | R::Unpaywall
    ) {
        // §3 step 2 names this bucket outright, so a route that names it is
        // step 2 whatever route named it.
        if url_has_host(url, &["pmc-oa-opendata.s3.amazonaws.com"]) {
            return FetchStep::PmcOa;
        }
        // §3 step 1's service, whichever index catalogued it.
        if url_has_host(url, &["europepmc.org"]) {
            return FetchStep::EuropePmc;
        }
        // The DOI resolver's landing page **is** §3 step 6's document: "the
        // publisher landing page". Recognising the resolver rather than a
        // publisher is what makes this rule provable rather than a guess.
        if url_has_host(url, &["doi.org", "dx.doi.org"]) {
            return FetchStep::PublisherLandingPage;
        }
    }

    match route {
        // Route-named, so the ADR names the step: step 1 names Europe PMC and
        // step 2 names the dataset.
        R::EuropePmc => FetchStep::EuropePmc,
        R::PmcOa => FetchStep::PmcOa,
        // Step 5, and the mapping is the ADR's resolve bullet rather than a
        // judgement: after `link[intended-application=text-mining]`, §3 step 5 is
        // what such a link is *for*. `RouteId::Crossref` also names step 4's
        // `is-supplemented-by` check, but that path builds no candidate today —
        // see the comment on the `SupplementDiscovery` arm.
        R::Crossref | R::ElsevierTdm | R::WileyTdm | R::SpringerTdm => FetchStep::SanctionedTdm,
        // Step 3, by ADR: "OA repository locations from OpenAlex and Unpaywall,
        // arXiv for preprints, and DOE OSTI for national-lab reports".
        //
        // `RouteId::Biorxiv` is **not named in §3 step 3**, which lists arXiv and
        // OSTI and no other preprint server. It is placed here rather than left
        // unplaced because step 3 is the only rung in the ADR that means "a
        // preprint server's own copy of the work", and leaving the bioRxiv route
        // below the publisher landing page would invert #260's preprint clause.
        // The placement is a reading of §3 and it is on the record as one.
        //
        // `RouteId::DataCite` is here too, and that is a **gap in the ADR**: §3
        // names DataCite for step 4's `IsSupplementTo` and for nothing else, while
        // this resolver also builds a `RouteId::DataCite` candidate from
        // `attributes.url` — the work's **own** landing page, which is not on
        // §3's ladder at all. Measured on `10.18434/*`, `attributes.url` is
        // `data.nist.gov/od/id/mds2-…`, a repository landing page for a
        // national-lab deposit, so step 3 is the honest placement. The gap is
        // reported rather than papered over with a default.
        R::OpenAlex | R::Unpaywall | R::Arxiv | R::Biorxiv | R::Osti | R::DataCite => {
            FetchStep::RepositoryLocation
        }
        // Step 6. `RouteId::ManualUrl` is here too: a URL a person or a reference
        // manager supplied is the publisher's own page in the ordinary case, and
        // the resolver already emits `RouteId::Publisher` first for the same
        // document — so the two tie and the pass's order keeps them apart. The
        // existing code calls ManualUrl "the last resort"; §3's own last resort is
        // step 7, and claiming that would be claiming a browser session for a
        // plain HTTP GET.
        R::Publisher | R::ManualUrl => FetchStep::PublisherLandingPage,
        // Step 7, verbatim.
        R::BrowserSession => FetchStep::BrowserSession,
        // Not on the ladder at all: these three record where a file came from
        // rather than where to fetch it, which is [`RouteId::never_fetches`]'s
        // existing name for the property.
        //
        // **No arm returns [`FetchStep::SupplementDiscovery`]**, and that is the
        // honest state of §3 step 4: the ADR numbers it, but nothing in this
        // resolver builds a candidate from `IsSupplementTo` or
        // `is-supplemented-by` — `RouteId::Crossref` and `RouteId::DataCite` are
        // used for `link[]` and `attributes.url` instead. The rung is kept
        // because a supplement is fetched differently from the article it
        // accompanies, and an empty rung is a gap in the implementation rather
        // than a fabricated position. `the_step_4_rung_is_numbered_and_empty`
        // pins that it is empty, so a future arm cannot be added silently.
        R::Legacy | R::ImportFlat | R::Manual => FetchStep::NotAFetchRoute,
    }
}

/// Whether a URL's host is one of `hosts`, exactly or as a subdomain of one.
///
/// A `bool` rather than a returned host so nothing has to outlive a parsed
/// [`reqwest::Url`]. `parse` rather than a string scan: a malformed candidate URL
/// is exactly the case where guessing at the host would be the wrong answer.
fn url_has_host(url: &str, hosts: &[&str]) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    // The trailing dot is a legal absolute name and `doi.org.` resolves, so it is
    // normalised away rather than treated as a different host.
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    hosts.iter().any(|wanted| {
        host == *wanted
            || (host.len() > wanted.len()
                && host.ends_with(wanted)
                && host.as_bytes()[host.len() - wanted.len() - 1] == b'.')
    })
}

/// ADR-007 §3's ordering, as one comparable value.
///
/// Three fields and a derived `Ord`, which is where **version strictly dominates
/// licence** lives: the version is compared first and the licence is read only
/// when the versions are equal. [`ranked_order_is_documented_and_pinned`] pins
/// it against hand-written expectations so the dominance cannot be changed by
/// someone reordering the fields.
///
/// [`FetchStep`] is third, so it is read **only on an exact version *and* licence
/// tie** — the property
/// [`the_fetch_order_never_reorders_across_a_version_or_licence_difference`]
/// pins. It cannot reach across a version boundary or a licence boundary, because
/// a derived `Ord` returns at the first field that differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Rank {
    pub version: Version,
    pub licence: LicenceStrength,
    /// ADR-007 §3's position for this candidate's route. The tiebreak.
    pub fetch_step: FetchStep,
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
    /// Candidates named by a source other than the six registries: a DOI
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
    /// this pass asks all of them because it is after version and licence facts
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
/// ADR-007 §3 names five sources for the resolve pass: OpenAlex, Unpaywall,
/// Crossref, DataCite and **Europe PMC** (`isOpenAccess`, `inEPMC`), plus the
/// `pmc-oa-opendata` dataset in step 2. A pass that spent a permit from any
/// other bucket would have touched a host that serves articles, and the ranked
/// plan would depend on a publisher's response time — the failure ADR-007 §3's
/// "resolve, then rank" exists to remove.
///
/// **The PMC-OA bucket is the one that needed arguing for.** It is not a
/// registry, it is an anonymous S3 bucket, and a listing of eight million
/// objects is not what a "metadata API" sounds like. It is in this list for two
/// reasons that both matter: it is where the *only* per-version
/// `is_manuscript` in the whole pass lives, and it is reached **without**
/// credentials and without any redirect — so it is as far from a publisher as
/// anything this pass touches. The alternative, spending the `europepmc`
/// bucket for it, would merge a public dataset into EBI's metadata budget and
/// make one service's outage report as another's.
pub const METADATA_BUCKETS: [&str; 6] = [
    "openalex",
    "crossref",
    "datacite",
    "unpaywall",
    "europepmc",
    "pmc_oa",
];

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
/// claim about the world — version strictly dominates licence, and ADR-007 §3's
/// fetch order breaks an exact tie between them — and a claim with no name is a
/// claim nobody reviews when it changes.
fn rank_by(candidate: &Candidate) -> Rank {
    Rank {
        version: candidate
            .version
            .expect("rank_candidates only sorts candidates that carry a version"),
        licence: candidate.licence,
        fetch_step: fetch_step_for(candidate.route, &candidate.url),
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
    ///
    /// Each line carries its ADR-007 §3 step, because that step is now a third
    /// sort key and a reader asking "why did the PMC-OA copy win?" cannot answer
    /// it from a version and a licence alone — those two said the candidates were
    /// equal.
    #[must_use]
    pub fn plan_lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .ranked
            .iter()
            .map(|entry| {
                format!(
                    "  {}. [step {} of ADR-007 §3: {} > {}] {}",
                    entry.position,
                    entry.rank.fetch_step.adr_step(),
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
    /// Base of Europe PMC's REST service, `/search` below it. A **base and not
    /// the full URL**, because the pass builds three shapes below it — the
    /// search, and (in a later slice) `fullTextXML` and `supplementaryFiles` —
    /// and a base is what stops a test pointing one at a server and another at
    /// the live service.
    pub europepmc_rest: String,
    /// Base of the `pmc-oa-opendata` bucket's HTTPS endpoint. Both the
    /// version listing and the per-version JSON are built from it, and the
    /// guard that refuses a URL outside it is what keeps the bucket's own
    /// `pdf_url` — an *absolute* URL in a third party's JSON — from becoming a
    /// request this pass makes.
    pub pmc_oa_bucket: String,
}

impl Default for MetadataBases {
    fn default() -> Self {
        Self {
            openalex_works: crate::openalex::OPENALEX_API_URL.to_string(),
            crossref_works: crate::crossref::CROSSREF_WORKS_URL.to_string(),
            datacite_dois: crate::datacite::DATACITE_DOIS_URL.to_string(),
            unpaywall_api: "https://api.unpaywall.org/v2".to_string(),
            europepmc_rest: crate::europepmc::EUROPE_PMC_REST_URL.to_string(),
            pmc_oa_bucket: crate::pmc_oa::PMC_OA_BUCKET_URL.to_string(),
        }
    }
}

impl MetadataBases {
    /// The six registry bases out of a downloader's [`crate::download::Endpoints`].
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
            europepmc_rest: endpoints.europepmc_rest.clone(),
            pmc_oa_bucket: endpoints.pmc_oa_bucket.clone(),
        }
    }

    /// The base configured for one registry.
    fn base_for(&self, registry: Registry) -> &str {
        match registry {
            Registry::OpenAlex => &self.openalex_works,
            Registry::Crossref => &self.crossref_works,
            Registry::DataCite => &self.datacite_dois,
            Registry::Unpaywall => &self.unpaywall_api,
            Registry::EuropePmc => &self.europepmc_rest,
            Registry::PmcOa => &self.pmc_oa_bucket,
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
    /// The record's own `papers.pmcid`, read by a targeted query rather than
    /// carried on [`scitadel_core::models::Paper`].
    ///
    /// Read separately for the reason
    /// [`scitadel_db::sqlite::identity::read_pmcid`] documents: `Paper` is
    /// written back wholesale, so a model field would be nulled on the next
    /// save for a work that *has* a PMCID. It is also the **only** handle the
    /// PMC OA dataset accepts — the bucket is keyed on a PMCID, not a DOI — so
    /// without it that leg is unreachable for every work Europe PMC does not
    /// answer for.
    pub pmcid: Option<&'a str>,
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
    /// The PMC-OA URL builder, held beside [`Self::bases`] rather than inside
    /// it so that the base and the shapes built from it cannot be configured
    /// apart — the #292 bug, where `IdentityChain::new` ignored the bases it was
    /// handed.
    pmc_oa: crate::pmc_oa::PmcOaAdapter,
}

impl MetadataPass {
    /// The production pass.
    #[must_use]
    pub fn new(openalex: scitadel_core::config::OpenAlexAuth, mailto: &str) -> Self {
        Self {
            bases: MetadataBases::default(),
            sources: CandidateSources::default(),
            openalex: openalex_with_mailto(openalex, mailto),
            pmc_oa: crate::pmc_oa::PmcOaAdapter::new()
                .with_base_url(&MetadataBases::default().pmc_oa_bucket),
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
            pmc_oa: crate::pmc_oa::PmcOaAdapter::new().with_base_url(&bases.pmc_oa_bucket),
            bases,
            sources,
            openalex: openalex_with_mailto(openalex, mailto),
        }
    }

    /// The six hosts this pass may request.
    #[must_use]
    pub fn bases(&self) -> &MetadataBases {
        &self.bases
    }

    /// The four hosts this pass may **name** a candidate against without
    /// requesting it.
    #[must_use]
    pub fn sources(&self) -> &CandidateSources {
        &self.sources
    }

    /// Run the pass: ask the six sources, then rank everything found.
    ///
    /// The order of the requests is ADR-007 §3's identity chain order, and the
    /// order candidates are collected in is ADR-007 §3's **fetch** order —
    /// which matters because two candidates that compare equal keep the pass's
    /// order and so the tiebreak is the ADR's own sequence rather than
    /// whichever registry replied first.
    ///
    /// Every source is asked even after one answers, which is the one place
    /// this pass deliberately differs from [`crate::identity_chain`]: the chain
    /// stops at the first answer because a second registry's title is a
    /// different work with the same words, whereas this pass is after *version
    /// and licence* facts that are genuinely distributed across all six — and
    /// the strongest of them is not a registration registry's at all.
    /// [`VersionSource::Repository`], from the `is_manuscript` flag in the
    /// `pmc-oa-opendata` dataset and Europe PMC's `*AuthMan` flags, is a
    /// boolean about one stored file rather than a classification word, so it
    /// outranks every `locations[].version` and `license[]` reading in the
    /// chain below it. The five registration registries answer the questions
    /// that remain — identity, and the licences no repository states. What the
    /// pass reads beyond identity is never fed to the matcher.
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
        // Whether **any** source has asserted the work is open access. Not the
        // same as "some candidate carries an OpenLicence" — it is the work-level
        // assertion ADR-007 §3 step 6 gates the publisher landing page on, and it
        // is what stops that page from being a blanket last resort. `false` means
        // "nobody said", which is why the gate is `false`-closed.
        let mut work_is_open_access = false;
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
                work_is_open_access |= is_oa;
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
                            version_flag: None,
                            location_licence: location.licence.clone(),
                            location_pmc_code: None,
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
                work_is_open_access |= is_oa;
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
                            version_flag: None,
                            location_licence: location.licence.clone(),
                            location_pmc_code: None,
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
                    // ADR-007 §3's resolve bullet reads
                    // `link[intended-application=text-mining]` — a filter the ADR
                    // already specifies and this pass was not applying. The other
                    // values are Crossref's own services: `similarity-checking` is
                    // plagiarism detection, `syndication` is content
                    // redistribution, and `unspecified` records that the publisher
                    // declined to say. None is somewhere a person or a library
                    // reads from, and the `similarity-checking` case is worse than
                    // useless: it is stamped `content-version: vor` on a record
                    // whose own `type` is `posted-content`, so it outranks the
                    // preprint every registry agrees on and then 403s.
                    if !link.is_reading_route() {
                        tracing::debug!(
                            url = link.url,
                            intended_application =
                                link.intended_application.as_deref().unwrap_or("(absent)"),
                            "crossref named a link that is not a reading route; \
                             ADR-007 §3 admits only text-mining"
                        );
                        continue;
                    }
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
                            version_flag: None,
                            location_licence: None,
                            location_pmc_code: None,
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
                            version_flag: None,
                            location_licence: None,
                            location_pmc_code: None,
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
                // Unpaywall's own record-level `is_oa`, which is the assertion
                // step 6 is gated on. Read before the locations because the
                // locations are the consequence of it.
                work_is_open_access |= body
                    .get("is_oa")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
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
                            version_flag: None,
                            location_licence: location
                                .get("license")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_string),
                            location_pmc_code: None,
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

        // ---- 5. Europe PMC: `isOpenAccess`, `license`, and the manuscript flags. ----
        //
        // Asked **after** the four registration registries and **before** the
        // PMC-OA lookup below, and the order is load-bearing in both
        // directions. Europe PMC answers "is this work open access" — which is
        // what gates the PMC-OA read entirely — and the DOI prefix fallback has
        // already had its chance, so a `10.1101` preprint that Europe PMC says
        // is a manuscript is placed by the repository rather than by string
        // arithmetic.
        let mut pmcids: BTreeSet<String> = refs
            .pmcid
            .and_then(crate::pmc_oa::normalise_pmcid)
            .into_iter()
            .collect();
        if let Some(doi) = doi.as_deref() {
            let hop = self
                .read(
                    client,
                    scope,
                    Registry::EuropePmc,
                    crate::europepmc::EuropePmcAdapter::new(&self.openalex.email)
                        .with_base_url(&self.bases.europepmc_rest)
                        .search_url(doi)
                        .map_err(|e| e.to_string()),
                )
                .await;
            consulted.push(hop.record());
            hosts.extend(hop.hosts);
            if let Some(body) = hop.body.clone()
                && let crate::europepmc::EuropePmcAnswer::Found(records) =
                    crate::europepmc::parse_search(&body)
            {
                for record in &records {
                    work_is_open_access |= record.is_open_access;
                    if let Some(pmcid) = &record.pmcid {
                        pmcids.insert(pmcid.clone());
                    }
                    for (at, location) in record.full_text_urls.iter().enumerate() {
                        if !location.is_repository_document() {
                            continue;
                        }
                        if !is_fetchable_url(&location.url) {
                            tracing::info!(
                                url = %location.url,
                                "europe pmc named a full-text URL that cannot be \
                                 fetched; it is not offered as a candidate"
                            );
                            continue;
                        }
                        candidates.push(Self::location_candidate(
                            today,
                            CandidateSeed {
                                route: RouteId::EuropePmc,
                                url: location.url.clone(),
                                named_by: format!(
                                    "{} record {} fullTextUrl[{}]",
                                    Registry::EuropePmc,
                                    record.id,
                                    at + 1
                                ),
                                version_word: None,
                                version_flag: Some(version_from_repository(
                                    Registry::EuropePmc,
                                    record.version,
                                )),
                                location_licence: None,
                                location_pmc_code: record.licence.clone(),
                                free_to_read: record.is_open_access,
                                licence_floor: LicenceFloor::RouteBasis,
                            },
                            refs.doi,
                            work_level.as_ref(),
                            &work_licences,
                        ));
                    }
                }
            }
        } else {
            consulted.push(ConsultedRegistry {
                registry: Registry::EuropePmc,
                outcome: crate::identity_chain::HopOutcome::NothingToAsk,
            });
        }

        // ---- 6. The PMC OA dataset: one candidate per stored version. ----
        //
        // The most expensive read in the pass — a listing plus one JSON per
        // version — and it runs **only** for a PMCID, which comes either from
        // `papers.pmcid` or from the Europe PMC record above. A work with
        // neither has nothing to ask this bucket: it is keyed on a PMCID, not a
        // DOI, and the id converter that would turn one into the other is a
        // fifth mechanism this pass deliberately does not have.
        for pmcid in &pmcids {
            let listed = self
                .read_text(
                    client,
                    scope,
                    Registry::PmcOa,
                    self.pmc_oa
                        .versions_url(pmcid)
                        .ok_or_else(|| format!("{pmcid} is not a PMCID")),
                )
                .await;
            consulted.push(listed.record());
            hosts.extend(listed.hosts);
            // `read_text` parks a non-JSON body as a `Value::String`, which is
            // the one place a raw body survives into this module — the listing is
            // XML and re-serialising it through `serde_json` would be a
            // round-trip that could only lose information.
            let Some(serde_json::Value::String(body)) = listed.body else {
                continue;
            };
            // Every stored version, in the bucket's order. Not the last and not
            // the first: the bucket's README is explicit that the version number
            // says nothing about which is the author manuscript, which is why
            // the JSON below is read at all.
            for key in crate::pmc_oa::parse_versions(&body) {
                let version = key.version;
                let hop = self
                    .read(
                        client,
                        scope,
                        Registry::PmcOa,
                        self.pmc_oa
                            .metadata_url(&key.pmcid, version)
                            .ok_or_else(|| format!("{key} is not a version")),
                    )
                    .await;
                consulted.push(hop.record());
                hosts.extend(hop.hosts);
                let Some(version_body) = hop.body else {
                    continue;
                };
                let Some(record) =
                    crate::pmc_oa::parse_version(&self.pmc_oa, &key.pmcid, version, &version_body)
                else {
                    continue;
                };
                // The PDF is the artefact, and its absence is measured rather
                // than hypothetical (PMC8238499.1 licenses the XML and not the
                // PDF), so a version with no `pdf_url` contributes nothing and
                // says so.
                let Some(url) = record.pdf_url.clone() else {
                    tracing::info!(
                        pmcid = %record.pmcid,
                        version,
                        "pmc oa version has no pdf_url; it licenses the XML and \
                         not the PDF, so it is not offered as a full-text candidate"
                    );
                    continue;
                };
                work_is_open_access |= record.is_pmc_openaccess;
                candidates.push(Self::location_candidate(
                    today,
                    CandidateSeed {
                        route: RouteId::PmcOa,
                        url,
                        named_by: format!(
                            "{} {pmcid}.{version} ({})",
                            Registry::PmcOa,
                            if record.is_manuscript {
                                "author manuscript"
                            } else {
                                "not a manuscript"
                            }
                        ),
                        version_word: None,
                        version_flag: Some(version_from_repository(
                            Registry::PmcOa,
                            crate::pmc_oa::version_of(&record),
                        )),
                        location_licence: None,
                        location_pmc_code: record.license_code.clone(),
                        // `is_pmc_openaccess` is the dataset's own statement
                        // that the version is in the OA subset. An author
                        // manuscript can be in the dataset and *not* be (the
                        // `TDM` case), so the two are read separately.
                        free_to_read: record.is_pmc_openaccess,
                        licence_floor: LicenceFloor::RouteBasis,
                    },
                    refs.doi,
                    work_level.as_ref(),
                    &work_licences,
                ));
            }
        }
        if pmcids.is_empty() {
            consulted.push(ConsultedRegistry {
                registry: Registry::PmcOa,
                outcome: crate::identity_chain::HopOutcome::NothingToAsk,
            });
        }

        // ---- 7. The derived routes: named from identifiers, never requested. ----
        //
        // ADR-007 §3's fetch order after the registries: arXiv for preprints,
        // DOE OSTI for national-lab reports, then the publisher's landing page,
        // then the record's own URL. Collected **last**, so a preprint-server
        // candidate loses a tie to a registry location ADR puts earlier — the
        // tiebreak is the ADR's own sequence.
        let derived = self.derived_candidates(refs, work_level.as_ref(), work_is_open_access);
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
        work_is_open_access: bool,
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

        // The DOI resolver's landing page, **only for an open-access work** —
        // ADR-007 §3 step 6: "Landing-page HTML, for OA works only, after TDM."
        //
        // The gate is the load-bearing part of this branch, and it is what stops
        // the landing page from becoming a blanket last resort. Without it, every
        // work with a DOI produced a `RouteId::Publisher` candidate, and the
        // ranker's bottom rung made it *reachable*: a work with no OA copy
        // anywhere would fetch a publisher's page, which is a publisher host on
        // the wire for a document that a human could not have read for free
        // either. `is_oa` from Unpaywall, `open_access.is_oa` from OpenAlex,
        // Europe PMC's `isOpenAccess` and the dataset's `is_pmc_openaccess` are
        // the four sources that can open the gate; none of them opening it means
        // nobody asserted the work is open access, and a landing page is then not
        // a candidate at all.
        //
        // **No version signal from the page itself**, which remains deliberate and
        // honest: a publisher's landing page serves whatever rendering the
        // paywall happens to serve, and nothing in a metadata pass establishes
        // which. It is therefore an *unrankable* candidate unless a registry typed
        // the work — which is what keeps step 6 honest without this module having
        // to invent a version.
        if let Some(normalised) = doi.filter(|_| work_is_open_access) {
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
        //
        // **Not** gated on open access, and deliberately so where the landing page
        // is: this URL came from the work's own record — a person, an import or an
        // adapter put it there — and dropping it for a work nobody has declared OA
        // would silently discard the one thing a human supplied. It ranks on
        // `Unstated` and `SubscriptionRead`, so it only wins when nothing else
        // exists, which is where a last resort belongs.
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
        // Taken by value first: the chain below moves the two licence fields out
        // of `seed`, and reading `seed.route` after that would be a use of a
        // moved value — so the two decisions that depend on the route are taken
        // here, while `seed` is whole.
        let route = seed.route;
        let registry_hint = seed.registry_hint();
        let (version, version_source) = seed
            .version_flag
            .or_else(|| {
                seed.version_word.as_deref().and_then(|word| {
                    if route == RouteId::Crossref {
                        version_from_content_version(word)
                    } else if route == RouteId::Unpaywall {
                        version_from_location_word(word, Registry::Unpaywall)
                    } else {
                        version_from_location_word(word, Registry::OpenAlex)
                    }
                })
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
                    registry: registry_hint,
                }]
            })
            .unwrap_or_default();
        // A code is only an offer if it names a licence we recognise. `TDM` and
        // an unmapped code both land here as `None`, which is the honest answer
        // and puts the candidate on `FreeToRead` — free to read, with no reuse
        // grant established.
        if let Some(code) = seed.location_pmc_code.as_deref()
            && let Some(offer) = LicenceOffer::from_pmc_code(code, registry_hint)
        {
            offers.push(offer);
        }
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
        let text = self.fetch_text(client, scope, registry, url).await;
        let (outcome, body, detail, hosts) = match text {
            Err((outcome, detail, hosts)) => (outcome, None, detail, hosts),
            Ok((raw, host)) => match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(body) => (
                    crate::identity_chain::HopOutcome::Answered,
                    Some(body),
                    format!("{registry} answered"),
                    BTreeSet::from([host]),
                ),
                Err(error) => (
                    crate::identity_chain::HopOutcome::Unreadable,
                    None,
                    format!("{registry} answered 200 that is not JSON: {error}"),
                    BTreeSet::from([host]),
                ),
            },
        };
        RegistryHop {
            registry,
            outcome,
            body,
            hosts,
            detail,
        }
    }

    /// [`Self::read`] for a body that is **not** JSON.
    ///
    /// Exists for the PMC-OA version listing, which is an S3
    /// `<ListBucketResult>` XML document. Split rather than generalised because
    /// the failure modes differ in a way the caller has to act on: a non-JSON
    /// body from a registry is an *unreadable answer*, while the same body from
    /// the bucket is the shape the bucket serves, and folding them together
    /// would mean either a second content-type branch inside `read` or a
    /// "parsed as JSON or gave up" rule that swallows the distinction.
    async fn read_text(
        &self,
        client: &scitadel_http::PacedClient,
        scope: &scitadel_http::WorkScope,
        registry: Registry,
        url: Result<reqwest::Url, String>,
    ) -> RegistryHop {
        match self.fetch_text(client, scope, registry, url).await {
            Err((outcome, detail, hosts)) => RegistryHop {
                registry,
                outcome,
                body: None,
                hosts,
                detail,
            },
            Ok((raw, host)) => RegistryHop {
                registry,
                outcome: crate::identity_chain::HopOutcome::Answered,
                body: Some(serde_json::Value::String(raw)),
                hosts: BTreeSet::from([host]),
                detail: format!("{registry} answered"),
            },
        }
    }

    /// One paced GET, guarded by the registry's configured base.
    ///
    /// Two properties are structural here rather than promised in prose:
    ///
    /// - **one `Request` permit per hop**, out of that registry's own bucket,
    ///   because `PacedClient` spends it (`ADR-007 §4`). Six sources are six
    ///   platforms' budgets.
    /// - **the URL is checked against the registry's configured base** before
    ///   the request, and a mismatch is an `Unreadable` answer rather than a
    ///   request. A pass that could reach a publisher host would make "the
    ///   metadata pass touches no publisher host" a property of which endpoint a
    ///   caller configured; this makes it a property of the code.
    ///
    /// The base check is load-bearing for the PMC-OA leg in a way it is not for
    /// the other five: that bucket's `pdf_url` values are **absolute `s3://`
    /// URIs in a third party's JSON**, so without the guard a JSON field would
    /// be the thing that decides which host this pass contacts. The `pdf_url` is
    /// rewritten to the bucket's own HTTPS base before it becomes a candidate,
    /// and the guard is what proves it.
    ///
    /// The error arm carries `(outcome, detail, hosts)` rather than a
    /// [`RegistryHop`] so both readers above build the hop themselves; a caller
    /// that forgets to record the hosts would then not compile, and a hop with
    /// no hosts is what makes `hosts_contacted` quietly wrong.
    async fn fetch_text(
        &self,
        client: &scitadel_http::PacedClient,
        scope: &scitadel_http::WorkScope,
        registry: Registry,
        url: Result<reqwest::Url, String>,
    ) -> Result<(String, String), (crate::identity_chain::HopOutcome, String, BTreeSet<String>)>
    {
        let url = match url {
            Ok(url) => url,
            Err(reason) => {
                tracing::info!(%registry, %reason, "could not build a metadata URL");
                return Err((
                    crate::identity_chain::HopOutcome::Unreadable,
                    reason,
                    BTreeSet::new(),
                ));
            }
        };
        if !self.bases.allows(registry, &url) {
            let host = url.host_str().unwrap_or("(no host)").to_string();
            tracing::warn!(
                %registry,
                %host,
                "refusing to request a URL outside this registry's configured base"
            );
            return Err((
                crate::identity_chain::HopOutcome::Unreadable,
                format!(
                    "{host} is not {registry}'s configured base, so the metadata \
                     pass did not contact it"
                ),
                BTreeSet::new(),
            ));
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
                return Err((
                    crate::identity_chain::HopOutcome::NotRegistered,
                    format!("{registry} has no record here"),
                    BTreeSet::from([host]),
                ));
            }
            Err(error) => {
                return Err((
                    crate::identity_chain::HopOutcome::Unreadable,
                    format!("{registry} could not be asked: {error}"),
                    BTreeSet::from([host]),
                ));
            }
        };
        match response.text().await {
            Ok(text) => Ok((text, host)),
            Err(error) => Err((
                crate::identity_chain::HopOutcome::Unreadable,
                format!("{registry} answered and the body would not read: {error}"),
                BTreeSet::from([host]),
            )),
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
    /// A version a repository stated **as a flag about this copy**, already
    /// resolved. Consulted **before** `version_word` and before the work-level
    /// chain, because it is the strongest signal in the pass: it is a statement
    /// about the file we would fetch, where a word from an index is a statement
    /// about a URL the index catalogued and a work-level type is a statement
    /// about the work.
    version_flag: Option<(Version, VersionSource)>,
    /// A licence on *this* location, as the naming registry spelled it — a
    /// **URL**, for Crossref `link[]`, DataCite and Unpaywall.
    location_licence: Option<String>,
    /// A licence on *this* location, as Europe PMC and the PMC OA dataset
    /// spell it: a **short code** (`cc by-nc-nd`, `CC0`, `TDM`).
    ///
    /// A separate field rather than a flag on `location_licence` because the
    /// two are not the same thing and the difference decides a verdict: a URL
    /// goes to [`LicenceOffer::from_entry`] and is checked against the
    /// allow-list by host and path, while a code goes to
    /// [`LicenceOffer::from_pmc_code`], which has to recognise a family name
    /// and must refuse `TDM`. Squeezing both into one `Option<String>` would
    /// mean deciding which reader to use at the far end, from the shape of a
    /// string — which is exactly the guess `LicenceOffer`'s three separate
    /// constructors exist to avoid.
    location_pmc_code: Option<String>,
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
            RouteId::EuropePmc => Registry::EuropePmc,
            RouteId::PmcOa => Registry::PmcOa,
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
        use FetchStep::RepositoryLocation;
        use LicenceStrength::{FreeToRead, OpenLicence, SubscriptionRead, Unknown};
        use Version::{AuthorManuscript, Preprint, Unstated, VersionOfRecord};

        // Every `Rank` here pins the fetch step, because this test is about the
        // **first two** keys only. Letting the third vary would not weaken the
        // claim below, it would change it — so the two are separated, and
        // `version_still_dominates_licence_with_the_fetch_step_free_to_vary`
        // proves the same dominance with the tiebreak free to move.
        //
        // Strict domination, both directions.
        assert!(
            Rank {
                version: VersionOfRecord,
                licence: Unknown,
                fetch_step: RepositoryLocation
            } < Rank {
                version: AuthorManuscript,
                licence: OpenLicence,
                fetch_step: RepositoryLocation
            },
            "an unknown-licence VoR outranks a CC-BY author manuscript"
        );
        assert!(
            Rank {
                version: AuthorManuscript,
                licence: Unknown,
                fetch_step: RepositoryLocation
            } < Rank {
                version: Preprint,
                licence: OpenLicence,
                fetch_step: RepositoryLocation
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
                    fetch_step: RepositoryLocation,
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
                    fetch_step: RepositoryLocation,
                })
            })
            .collect();
        every.sort();
        assert_eq!(every.len(), 16, "four versions by four licence strengths");
        assert_eq!(
            every[0],
            Rank {
                version: VersionOfRecord,
                licence: OpenLicence,
                fetch_step: RepositoryLocation
            }
        );
        assert_eq!(
            every[15],
            Rank {
                version: Unstated,
                licence: Unknown,
                fetch_step: RepositoryLocation
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

    /// The **companion** to `ranked_order_is_documented_and_pinned`, and the one
    /// that a third sort key actually threatens.
    ///
    /// Adding [`FetchStep`] to [`Rank`] cannot reorder across a version or a
    /// licence — a derived `Ord` returns at the first field that differs — but
    /// "cannot" is a claim about `Ord`'s implementation, so it is tested rather
    /// than asserted. The grid is the full four-by-four-by-eight, all 128 cells,
    /// and version must still be the outer axis: every cell of a version of record
    /// sorts ahead of every cell of an author manuscript, whatever the fetch step
    /// in either.
    #[test]
    fn version_still_dominates_licence_with_the_fetch_step_free_to_vary() {
        const LICENCES: [LicenceStrength; 4] = LicenceStrength::ALL;
        const VERSIONS: [Version; 4] = Version::ALL;

        let mut every: Vec<Rank> = VERSIONS
            .iter()
            .flat_map(|version| {
                LICENCES.iter().flat_map(move |licence| {
                    FetchStep::ALL
                        .iter()
                        .map(move |fetch_step| Rank {
                            version: *version,
                            licence: *licence,
                            fetch_step: *fetch_step,
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        every.sort();

        let expected = VERSIONS.len() * LICENCES.len() * FetchStep::ALL.len();
        assert_eq!(every.len(), expected, "four by four by eight");
        for (at, cell) in every.iter().enumerate() {
            assert_eq!(
                cell.version,
                VERSIONS[at / (LICENCES.len() * FetchStep::ALL.len())],
                "cell {at} must still belong to the version block it sorted into: \
                 the fetch step is the *last* key, so it cannot reorder the outer \
                 two"
            );
        }
        // And the extremes, so the assertion is not vacuous: the weakest
        // `Unstated`/`Unknown` cell at the best fetch step still precedes the
        // strongest `VersionOfRecord`/`OpenLicence` cell at the worst one.
        assert_eq!(
            every[0],
            Rank {
                version: Version::VersionOfRecord,
                licence: LicenceStrength::OpenLicence,
                fetch_step: FetchStep::EuropePmc
            }
        );
        assert_eq!(
            every[expected - 1],
            Rank {
                version: Version::Unstated,
                licence: LicenceStrength::Unknown,
                fetch_step: FetchStep::NotAFetchRoute
            }
        );
    }

    /// §3 step 4 is **numbered and currently empty**, and that is a fact about
    /// the implementation rather than a gap papered over with a default.
    ///
    /// Nothing in the resolver builds a candidate from DataCite's
    /// `IsSupplementTo` or Crossref's `is-supplemented-by`, so no route maps to
    /// `FetchStep::SupplementDiscovery`. The rung exists because the ADR numbers
    /// it and because a supplement is fetched differently from the article it
    /// accompanies; asserting it is empty means the day someone wires SI
    /// discovery up, this fails and asks them to say where on the ladder it goes.
    #[test]
    fn the_step_4_rung_is_numbered_and_empty() {
        for route in RouteId::ALL {
            assert_ne!(
                fetch_step_for(route, "https://example.invalid/supp.pdf"),
                FetchStep::SupplementDiscovery,
                "{route} now maps to §3 step 4; say in this test which route it is \
                 and why a supplement ranks there"
            );
        }
        assert_eq!(FetchStep::SupplementDiscovery.adr_step(), 4);
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
    // ADR-007 §3's fetch order, as the tiebreak on an exact version+licence tie.
    // =====================================================================

    /// **The thing #260 exists to change**, in the shape the live probe table
    /// found it in: a PMC-OA copy of the version of record and a publisher's own
    /// copy of the version of record, same version, same `OpenLicence`, so the
    /// two-key `Rank` called them equal and the winner was whichever registry
    /// answered first.
    ///
    /// Both orders are exercised, because "stable sort kept the pass's order"
    /// would make this test pass with the tiebreak absent if the repository
    /// candidate happened to be emitted first.
    #[test]
    fn a_repository_copy_beats_a_publisher_copy_of_the_same_version_and_licence() {
        use LicenceStrength::OpenLicence;
        use Version::VersionOfRecord;

        for repository_first in [true, false] {
            let repository = candidate(RouteId::PmcOa, Some(VersionOfRecord), OpenLicence);
            let publisher = candidate(RouteId::Unpaywall, Some(VersionOfRecord), OpenLicence);
            assert_eq!(
                (repository.version, repository.licence),
                (publisher.version, publisher.licence),
                "the fixture only means anything if the two tie exactly, which is \
                 the only condition under which ADR-007 §3's fetch order applies"
            );

            let resolution = rank_candidates(if repository_first {
                vec![repository, publisher]
            } else {
                vec![publisher, repository]
            });

            assert_eq!(
                resolution.ranked[0].candidate.route,
                RouteId::PmcOa,
                "repository_first={repository_first}: the pmc-oa dataset is \
                 ADR-007 §3 step 2 and the publisher's own copy is a later step, \
                 and on an exact tie the ADR's own ordering decides"
            );
            assert_eq!(resolution.ranked[1].candidate.route, RouteId::Unpaywall);
        }
    }

    /// The **half** of the requirement that is easy to get wrong: the fetch
    /// order must not reach across a version or licence boundary. Wave 2 fixed
    /// version-over-licence dominance (#293) and that is settled; a third sort
    /// key that could reorder *across* either of the first two would undo it
    /// silently, because a fetch-order-first comparator still "dominates
    /// version" on every sample that happens to share a step.
    #[test]
    fn the_fetch_order_never_reorders_across_a_version_or_licence_difference() {
        use LicenceStrength::{FreeToRead, OpenLicence};
        use Version::{Preprint, VersionOfRecord};

        // A publisher copy is a later fetch step than the repository copy, so
        // these are the pairs the tiebreak would reorder if it were allowed to.
        // Each holds **one** axis equal and makes the other decide, which is
        // what "never reorder across a difference" has to mean.
        for (stronger, weaker) in [
            // Across a licence difference: same version, and the repository copy
            // has the stronger licence.
            (
                candidate(RouteId::PmcOa, Some(VersionOfRecord), OpenLicence),
                candidate(RouteId::Unpaywall, Some(VersionOfRecord), FreeToRead),
            ),
            // Across a version difference: same licence, and the repository copy
            // is the version of record.
            (
                candidate(RouteId::PmcOa, Some(VersionOfRecord), FreeToRead),
                candidate(RouteId::Unpaywall, Some(Preprint), FreeToRead),
            ),
        ] {
            assert_ne!(
                stronger.route, weaker.route,
                "the fixture must exercise two different fetch steps"
            );
            assert_eq!(
                fetch_step_for(stronger.route, &stronger.url),
                FetchStep::PmcOa,
                "the fixture's premise: the repository copy is §3 step 2"
            );
            assert!(
                fetch_step_for(weaker.route, &weaker.url) > FetchStep::PmcOa,
                "the fixture's premise: the weaker candidate is on a later rung"
            );

            let resolution = rank_candidates(vec![weaker.clone(), stronger.clone()]);
            assert_eq!(
                resolution.ranked[0].candidate.route, stronger.route,
                "{stronger:?} must beat {weaker:?} on version or licence alone: \
                 §3's order is the *tiebreak*, not the primary key"
            );
            // And the tie-break is what does decide once the two are equal, so
            // the assertion above is not passing because the tie-break is inert.
            assert_ne!(
                (stronger.version, stronger.licence),
                (weaker.version, weaker.licence),
                "each fixture pair must differ on at least one of the first two keys"
            );
        }
    }

    /// Every route that can produce a candidate lands on a rung, and the rungs
    /// are §3's own seven plus the weakest position for a route that is not a
    /// fetch route at all. Exhaustive over [`RouteId::ALL`] so a new variant
    /// cannot be added without being placed — the compiler's exhaustiveness
    /// would catch the match, and this catches the *ordering* of the rungs.
    #[test]
    fn every_route_id_is_placed_on_the_adr_fetch_order() {
        for route in RouteId::ALL {
            let step = crate::resolve::fetch_step_for(route, "https://example.invalid/x.pdf");
            assert!(
                crate::resolve::FetchStep::ALL.contains(&step),
                "{route} mapped to a step outside the ladder"
            );
        }

        // §3's own order, best first. Pinned by hand against the ADR's
        // numbering rather than against the implementation, because the claim
        // under test is that the enum *is* the ADR's list.
        let ordered = [
            (crate::resolve::FetchStep::EuropePmc, 1),
            (crate::resolve::FetchStep::PmcOa, 2),
            (crate::resolve::FetchStep::RepositoryLocation, 3),
            (crate::resolve::FetchStep::SupplementDiscovery, 4),
            (crate::resolve::FetchStep::SanctionedTdm, 5),
            (crate::resolve::FetchStep::PublisherLandingPage, 6),
            (crate::resolve::FetchStep::BrowserSession, 7),
            (crate::resolve::FetchStep::NotAFetchRoute, 0),
        ];
        for (step, adr_step) in ordered {
            assert_eq!(
                step.adr_step(),
                adr_step,
                "{step:?} does not carry ADR-007 §3 step {adr_step}"
            );
        }
        let mut weakest_first = crate::resolve::FetchStep::ALL;
        weakest_first.sort();
        assert_eq!(
            weakest_first,
            crate::resolve::FetchStep::ALL,
            "FetchStep::ALL must already be in §3's order, strongest first"
        );
    }

    /// The never-fetch routes are not a rung *above* the browser session, and
    /// the resolver never offers one as a candidate at all. `leg_of` already
    /// gives them no leg of their own for the same reason; this is the
    /// ranking-side half of that, and it is asserted against the property that
    /// matters rather than against the enum: nothing that cannot be fetched can
    /// be the one candidate `acquire` fetches.
    #[test]
    fn a_route_that_never_fetches_is_never_the_chosen_candidate() {
        for route in [RouteId::Legacy, RouteId::ImportFlat, RouteId::Manual] {
            assert_eq!(
                crate::resolve::fetch_step_for(route, "https://example.invalid/x.pdf"),
                crate::resolve::FetchStep::NotAFetchRoute,
                "{route} records where a file came from rather than where to fetch \
                 it, so it has no place on §3's fetch ladder"
            );
            assert!(
                route.never_fetches(),
                "{route} should be declared provenance-only"
            );
        }
        assert_eq!(
            crate::resolve::fetch_step_for(
                RouteId::BrowserSession,
                "https://example.invalid/x.pdf"
            ),
            crate::resolve::FetchStep::BrowserSession,
            "ADR-007 §3 step 7 is the last resort and is still a fetch route"
        );
    }

    /// **Where the URL, not the route, decides** — the brief's case, and the
    /// only place [`fetch_step_for`] consults the URL at all.
    ///
    /// `RouteId::Crossref` is step 5 because ADR-007 §3's resolve bullet reads
    /// `link[intended-application=text-mining]` and step 5 is what a `text-mining`
    /// link is for. But `RouteId::DataCite` and `RouteId::OpenAlex` are
    /// *identity* sources: the same route can name a repository copy or a
    /// publisher's page, and step 3 and step 6 are different rungs. Naming a
    /// location is not the same as saying what kind of place it is, so for those
    /// three the URL decides.
    #[test]
    fn the_step_is_derived_from_the_url_where_the_route_is_only_an_identity_source() {
        use crate::resolve::FetchStep as Step;
        use crate::resolve::fetch_step_for as step_of;

        // The case in the brief: a Crossref-named URL on the pmc-oa bucket is
        // step 2, not "whatever Crossref is".
        assert_eq!(
            step_of(
                RouteId::Crossref,
                "https://pmc-oa-opendata.s3.amazonaws.com/PMC4702794.1/PMC4702794.1.pdf"
            ),
            Step::PmcOa
        );
        // The DOI resolver's landing page is §3 step 6 by definition, whichever
        // registry named it.
        for route in [RouteId::OpenAlex, RouteId::Unpaywall, RouteId::DataCite] {
            assert_eq!(
                step_of(route, "https://doi.org/10.18434/mds2-2400"),
                Step::PublisherLandingPage,
                "{route} naming doi.org named the landing page, which §3 step 6 is"
            );
        }
        // And Europe PMC named by an index is still §3 step 1.
        assert_eq!(
            step_of(
                RouteId::OpenAlex,
                "https://europepmc.org/articles/PMC4702794"
            ),
            Step::EuropePmc
        );
        // A publisher's own PDF named by an index stays step 3, because §3 step 3
        // is "OA repository locations **from OpenAlex and Unpaywall**" and the
        // resolver cannot tell a repository host from a publisher host without a
        // list. Asserted so the limitation is a decision on the record rather
        // than an oversight; see the report.
        assert_eq!(
            step_of(
                RouteId::Unpaywall,
                "https://www.mdpi.com/1420-3049/24/15/2793/pdf"
            ),
            Step::RepositoryLocation
        );
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

    /// The acceptance criterion as a constant: six buckets, none of which
    /// serves articles. A publisher bucket in this list would be a
    /// resolve-then-rank that resolves *through* the publisher, which is the
    /// behaviour being replaced.
    #[test]
    fn the_metadata_pass_is_allowed_exactly_adrs_five_sources_and_the_dataset() {
        assert_eq!(
            METADATA_BUCKETS,
            [
                "openalex",
                "crossref",
                "datacite",
                "unpaywall",
                "europepmc",
                "pmc_oa"
            ],
            "ADR-007 §3 names OpenAlex, Unpaywall, Crossref, DataCite and \
             Europe PMC for the resolve pass, and the `pmc-oa-opendata` dataset \
             in step 2"
        );
        // The two PMC sources earn their place on two separate grounds, and
        // either alone would not be enough — so they are asserted separately
        // rather than as "two more entries".
        assert!(
            METADATA_BUCKETS.contains(&"europepmc"),
            "Europe PMC is the only source that reports `isOpenAccess`, \
             `license` and the manuscript flags for a work"
        );
        assert!(
            METADATA_BUCKETS.contains(&"pmc_oa"),
            "and the dataset is the only source that reports `is_manuscript` per \
             *stored version*, which is the strongest version signal available"
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

    /// One `wiremock` server per [`METADATA_BUCKETS`] entry, each answering
    /// with that source's own envelope.
    ///
    /// **Six servers, not one**, and that is the point: `scitadel-http`'s policy
    /// table keys on **host**, so a single server cannot host two platforms and a
    /// path-prefix "route" would test a table production never builds. Production
    /// separates these by host too — `api.openalex.org`, `api.crossref.org`,
    /// `api.datacite.org`, `api.unpaywall.org`, `www.ebi.ac.uk`,
    /// `pmc-oa-opendata.s3.amazonaws.com` — so this reproduces the real
    /// separation instead of simulating it, which is what makes
    /// `the_metadata_pass_touches_no_publisher_host` a claim about hosts.
    struct MetadataSources {
        servers: Vec<MockServer>,
        table: scitadel_http::BucketPolicyTable,
    }

    impl MetadataSources {
        /// One server per [`METADATA_BUCKETS`] entry, each routed to that
        /// bucket's name.
        ///
        /// The count comes from the constant rather than a literal, so adding a
        /// metadata source cannot leave this fixture serving one fewer host than
        /// the pass asks — which would make `hosts_contacted` compare equal
        /// while a real hop went somewhere unwatched.
        async fn new() -> Self {
            let mut servers: Vec<MockServer> = Vec::new();
            for _ in 0..METADATA_BUCKETS.len() {
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
                europepmc_rest: self.servers[4].uri(),
                pmc_oa_bucket: self.servers[5].uri(),
            }
        }

        /// One server per metadata source, by bucket name.
        fn server(&self, bucket: &str) -> &MockServer {
            let at = METADATA_BUCKETS
                .iter()
                .position(|name| *name == bucket)
                .unwrap_or_else(|| panic!("{bucket} is not a metadata bucket"));
            &self.servers[at]
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

    fn pass_for(registries: &MetadataSources) -> MetadataPass {
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
    /// **Every source is asked, Europe PMC and the PMC-OA dataset included**
    /// (#254 wave 3), and the fixture gives the work a `pmcid` so the bucket leg
    /// is a real read rather than a `NothingToAsk` — a pass that never asked the
    /// bucket would satisfy "touched no publisher host" trivially, which is the
    /// green-test-that-tested-nothing shape.
    ///
    /// The fixture's preprint, OSTI and DOI-resolver bases point at a port nothing
    /// listens on, so a pass that *did* try to name-check one would fail the whole
    /// test rather than silently succeed against a live host.
    #[tokio::test]
    async fn the_metadata_pass_touches_no_publisher_host() {
        let sources = MetadataSources::new().await;
        // Each source answers 404 — the pass collects no candidates, and the
        // question here is only which hosts it asked.
        for server in &sources.servers {
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        let pacer = Arc::new(RecordingPacer::default());
        let client = recording_client(&pacer, sources.table.clone());

        let resolution = pass_for(&sources)
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1101/2025.06.14.659707"),
                    arxiv_id: Some("2301.00001"),
                    osti_id: Some("1234567"),
                    url: Some("https://example.invalid/paper"),
                    openalex_id: Some("W1"),
                    pmcid: Some("PMC7759461"),
                },
            )
            .await;

        // The pass's own record of which authorities it contacted, against the
        // six the servers actually listen on. Comparing the two rather than
        // trusting either alone is what makes this a measurement: a publisher
        // host would be an *extra* entry here, not a missing one.
        assert_eq!(
            resolution
                .hosts_contacted
                .iter()
                .cloned()
                .collect::<Vec<String>>(),
            sources.authorities(),
            "the pass contacted exactly the six metadata authorities and nothing \
             else — a publisher host would appear as a seventh: {:?}",
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

        // Every source was asked, and only the sources. Seven hops for six
        // sources, and the extra one is OpenAlex asked twice: the by-DOI read
        // 404'd, so the by-id read ran as well — one work, one OpenAlex budget,
        // two requests, and both of them `Meta`.
        let sources_consulted: std::collections::BTreeSet<Registry> = resolution
            .consulted
            .iter()
            .map(|hop| hop.registry)
            .collect();
        assert_eq!(
            sources_consulted,
            Registry::ALL
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "all six sources, in ADR-007 §3's order: {:?}",
            resolution.consulted
        );
        assert_eq!(
            resolution.consulted.len(),
            7,
            "six sources, seven hops: OpenAlex by DOI and by id, Crossref, \
             DataCite, Unpaywall, Europe PMC, and the PMC-OA version listing — \
             and nothing else: {:?}",
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
            sources.request_counts().await,
            vec![2, 1, 1, 1, 1, 1],
            "every server was asked, and only OpenAlex twice: {:?}",
            sources.request_counts().await
        );

        // The budgets are the sharper half: no publisher bucket was spent.
        let buckets = MetadataSources::buckets_spent(&pacer);
        assert_eq!(
            buckets,
            vec![
                "crossref".to_string(),
                "datacite".to_string(),
                "europepmc".to_string(),
                "openalex".to_string(),
                "openalex".to_string(),
                "pmc_oa".to_string(),
                "unpaywall".to_string(),
            ],
            "seven `Request` permits — one per hop, OpenAlex's two out of one \
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
            "and the distinct buckets are exactly ADR-007 §3's metadata sources: {buckets:?}"
        );
        for (bucket, tier) in pacer.0.lock().expect("lock").iter() {
            assert_eq!(*tier, PaceTier::Meta, "{bucket} must be metadata work");
        }

        // And the guard that makes the first two true by construction.
        for registry in Registry::ALL {
            assert!(
                !sources.bases().allows(
                    registry,
                    &reqwest::Url::parse("http://127.0.0.1:9/content/x.full.pdf").expect("a url")
                ),
                "{registry}'s base must refuse a preprint-server URL, which is \
                 what makes 'the pass cannot reach a publisher' a property of \
                 [`MetadataBases::allows`] rather than of this test"
            );
        }
        assert!(
            sources.bases().allows(
                Registry::OpenAlex,
                &reqwest::Url::parse(&format!("{}/doi:10.1101/x", sources.servers[0].uri()))
                    .expect("a url")
            ),
            "and it still allows its own registry's URL, so the guard is not \
             simply refusing everything"
        );
    }

    /// **Every Europe PMC location the record names becomes a candidate**, and
    /// each carries the repository's own version claim rather than an inferred
    /// one — the shape #254's wave 3 exists to produce.
    ///
    /// The fixture is the measured NumPy envelope. Every assertion below is about
    /// what the *plan* says, because that is what a user reads and what a future
    /// change to the comparator would be caught by:
    ///
    /// - both served styles become candidates, and the DOI resolver's page does
    ///   not (it is the publisher route's candidate, not Europe PMC's);
    /// - each candidate's version comes from the repository, and a `MED` record
    ///   with every manuscript flag `N` says `unstated` — **not** a version of
    ///   record, which is the overclaim this slice exists to avoid;
    /// - `license: cc by` becomes an open licence, because Europe PMC stated a
    ///   licence and `inEPMC: Y` alone never would.
    #[tokio::test]
    async fn a_europe_pmc_record_names_a_location_per_served_document() {
        let sources = MetadataSources::new().await;
        for server in &sources.servers {
            if server.address().port() == sources.server("europepmc").address().port() {
                continue;
            }
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(crate::europepmc::tests::NUMPY_ENVELOPE),
            )
            .mount(sources.server("europepmc"))
            .await;
        let pacer = Arc::new(RecordingPacer::default());
        let resolution = pass_for(&sources)
            .resolve(
                &recording_client(&pacer, sources.table.clone()),
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1038/s41586-020-2649-2"),
                    ..WorkRefs::default()
                },
            )
            .await;

        let locations: Vec<&Candidate> = resolution
            .candidates()
            .filter(|candidate| candidate.route == RouteId::EuropePmc)
            .collect();
        assert_eq!(
            locations.len(),
            2,
            "one candidate per served document — the html render and the pdf — \
             and not the `site: DOI` landing page: {:?}",
            resolution.plan_lines()
        );
        for location in &locations {
            assert_eq!(
                location.version,
                Some(Version::Unstated),
                "a `MED` record with every manuscript flag `N` states no \
                 version, and that is the bottom rung rather than a version of \
                 record"
            );
            assert!(
                matches!(location.version_source, VersionSource::Repository { .. }),
                "and the source is the repository's own field, not a DOI-prefix \
                 inference: {:?}",
                location.version_source
            );
            assert_eq!(
                location.licence,
                LicenceStrength::OpenLicence,
                "`license: cc by` is a grant Europe PMC stated, so it counts: {:?}",
                resolution.plan_lines()
            );
            // And the reason a plan line prints says which field was read.
            assert!(
                location.version_source.why().contains("manuscript"),
                "the reason names the field the answer came from: {}",
                location.version_source.why()
            );
        }
        assert!(
            !resolution
                .candidates()
                .any(|candidate| candidate.url.contains("doi.org/10.1038")
                    && candidate.route == RouteId::EuropePmc),
            "the DOI resolver's page is not a Europe PMC location: {:?}",
            resolution.plan_lines()
        );
    }

    /// **The two signals must disagree somewhere, and here is where**: a PMC-OA
    /// version of record and an OpenAlex location typed `acceptedVersion` for the
    /// same work.
    ///
    /// The repository's `is_manuscript: false` wins, and the reason is the chain's
    /// first row rather than a tiebreak: `is_manuscript` is a statement about
    /// **the file at this URL**, while OpenAlex's `acceptedVersion` is a statement
    /// about a URL OpenAlex catalogued — which may be a *different* file, on a
    /// different host. A work with a publisher copy at one location and a
    /// repository manuscript at another has one work-level record and two
    /// locations, so a record-level type cannot separate them and a per-location
    /// word from an index can only be right by accident. Preferring the
    /// repository here is what stops a `coverage` report claiming the library
    /// holds an author manuscript when the bytes are the typeset article.
    ///
    /// Asserted on the **rank**, not on the parse: the parse is the adapter's
    /// test, and what this module has to get right is which claim wins.
    #[tokio::test]
    async fn a_repository_version_of_record_beats_an_index_author_manuscript() {
        let sources = MetadataSources::new().await;
        for server in &sources.servers {
            let is_openalex =
                server.address().port() == sources.server("openalex").address().port();
            let is_bucket = server.address().port() == sources.server("pmc_oa").address().port();
            if is_openalex || is_bucket {
                continue;
            }
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        // OpenAlex types the *record* `article` and names one location with the
        // word `acceptedVersion`.
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("^/doi:10[.]1038"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"id":"W1","type":"article","open_access":{"is_oa":true},
                            "best_oa_location":{"is_oa":true,"version":"acceptedVersion",
                              "pdf_url":"https://example.invalid/manuscript.pdf"},
                            "locations":[{"is_oa":true,"version":"acceptedVersion",
                              "pdf_url":"https://example.invalid/manuscript.pdf"}]}"#,
                    ),
            )
            .mount(sources.server("openalex"))
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/xml")
                    .set_body_string(
                        "<ListBucketResult><CommonPrefixes><Prefix>PMC7759461.1/</Prefix></CommonPrefixes></ListBucketResult>",
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/metadata/PMC7759461.1.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"pmcid":"PMC7759461","version":1,"is_manuscript":false,
                            "is_pmc_openaccess":true,"license_code":"CC BY",
                            "pdf_url":"s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.pdf?md5=ab"}"#,
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        let pacer = Arc::new(RecordingPacer::default());
        let resolution = pass_for(&sources)
            .resolve(
                &recording_client(&pacer, sources.table.clone()),
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1038/s41586-020-2649-2"),
                    pmcid: Some("PMC7759461"),
                    ..WorkRefs::default()
                },
            )
            .await;

        let openalex = resolution
            .candidates()
            .find(|candidate| candidate.route == RouteId::OpenAlex)
            .expect("OpenAlex named a location");
        assert_eq!(
            openalex.version,
            Some(Version::AuthorManuscript),
            "the index's own `acceptedVersion` word, honoured as a claim about \
             the URL it catalogued"
        );
        let dataset = resolution
            .candidates()
            .find(|candidate| candidate.route == RouteId::PmcOa)
            .expect("the stored version is a candidate");
        assert_eq!(dataset.version, Some(Version::VersionOfRecord));

        // The repository's file wins, on version — not on licence, which is the
        // axis the dominance rule exists to keep from crossing a version
        // boundary.
        let ranked: Vec<(&str, Version, LicenceStrength)> = resolution
            .ranked
            .iter()
            .map(|entry| {
                (
                    entry.candidate.route.label(),
                    entry.rank.version,
                    entry.rank.licence,
                )
            })
            .collect();
        assert_eq!(
            ranked.first(),
            Some(&(
                "pmc_oa",
                Version::VersionOfRecord,
                LicenceStrength::OpenLicence
            )),
            "a repository-stated version of record outranks an index-stated \
             author manuscript: {ranked:?}"
        );
        let manuscript_position = ranked
            .iter()
            .position(|(route, ..)| *route == "openalex")
            .expect("the OpenAlex candidate is ranked");
        assert!(
            manuscript_position > 0,
            "and the index's manuscript is below it: {ranked:?}"
        );
    }

    /// **A `TDM` manuscript is free to read and nothing more** — and the
    /// assertion is that it is *not* an open licence.
    ///
    /// The PMC OA dataset's own `README.txt` defines the code: author manuscripts
    /// "where the full text is available for text mining, and where the full text
    /// may also be used consistent with the principles of fair use". That is a
    /// text-mining permission. Reading it as a reuse grant would write
    /// `access_basis = 'oa_license'` on bytes nobody granted reuse of — #261's
    /// bug, reached through a code that looks exactly like the CC codes beside
    /// it.
    #[tokio::test]
    async fn a_tdm_author_manuscript_is_free_to_read_and_not_licensed() {
        let sources = MetadataSources::new().await;
        for server in &sources.servers {
            if server.address().port() == sources.server("pmc_oa").address().port() {
                continue;
            }
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/xml")
                    .set_body_string(
                        "<ListBucketResult><CommonPrefixes><Prefix>PMC7759461.1/</Prefix></CommonPrefixes></ListBucketResult>",
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/metadata/PMC7759461.1.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"pmcid":"PMC7759461","version":1,"is_manuscript":true,
                            "is_pmc_openaccess":false,"is_retracted":false,
                            "license_code":"TDM",
                            "pdf_url":"s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.pdf?md5=ab"}"#,
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        let pacer = Arc::new(RecordingPacer::default());
        let resolution = pass_for(&sources)
            .resolve(
                &recording_client(&pacer, sources.table.clone()),
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1038/s41586-020-2649-2"),
                    pmcid: Some("PMC7759461"),
                    ..WorkRefs::default()
                },
            )
            .await;

        let manuscript = resolution
            .candidates()
            .find(|candidate| candidate.route == RouteId::PmcOa)
            .expect("the stored version is a candidate");
        assert_eq!(
            manuscript.version,
            Some(Version::AuthorManuscript),
            "`is_manuscript: true` is the strongest version evidence in the pass"
        );
        assert_eq!(
            manuscript.licence,
            LicenceStrength::FreeToRead,
            "`TDM` is a text-mining permission, not a reuse grant — so the \
             candidate is free to read and carries no licence we may apply"
        );
        assert_ne!(
            manuscript.licence,
            LicenceStrength::OpenLicence,
            "and above all it is not an open licence, which is the whole claim"
        );
        assert_eq!(
            manuscript.licence.access_basis(),
            Some("oa_license"),
            "…which is `FreeToRead`'s honest column value: free to read, no grant"
        );
    }

    /// **The repository's own claim beats a DOI-prefix inference**, which is the
    /// whole point of #254's wave 3 and the reason the repository row sits first
    /// in the chain.
    ///
    /// The case is real and it is the mirror image of #260's: PMC says this
    /// stored file is an author manuscript, and the DOI's prefix says `10.1101`,
    /// which the fallback would read as "preprint". Both are about this work and
    /// both are true — the manuscript is the preprint's own author-submitted text
    /// — but they name different rungs, and preferring the weaker one would place
    /// a repository-stated author manuscript **below** a preprint. ADR-007 §3's
    /// ladder is `OA VoR > AM > preprint`, so the manuscript must rank first, and
    /// only the flag can put it there.
    ///
    /// Without the repository row this candidate would sit on
    /// [`Version::Preprint`] from [`VersionSource::DoiPrefix`], and the plan line
    /// would name the prefix as the reason.
    #[tokio::test]
    async fn a_manuscript_flag_outranks_the_doi_prefix_fallback() {
        // Pure, and first: the two claims side by side.
        let (manuscript, source) = version_from_repository(
            Registry::PmcOa,
            crate::registry::RepositoryVersion::AuthorManuscript,
        );
        let (preprint, _) = version_from_doi_prefix("10.1101/2020.01.30.927871")
            .expect("a 10.1101 DOI has a verified preprint transform");
        assert_eq!(manuscript, Version::AuthorManuscript);
        assert_eq!(preprint, Version::Preprint);
        // `Version`'s derived `Ord` is best-first, so `am < preprint` here means
        // the manuscript ranks *ahead* — which is ADR-007 §3's `OA VoR > AM >
        // preprint`. Written this way because the comparison's direction is the
        // one thing a reader has to get right, and `>` would read as the claim.
        assert!(
            manuscript < preprint,
            "ADR-007 §3's ladder is `vor > am > preprint` and `Version`'s `Ord` \
             is best-first, so a repository's `is_manuscript: true` must sort \
             **before** the prefix's preprint inference"
        );
        assert!(matches!(source, VersionSource::Repository { .. }));

        // And through the pass, with no registry typing the work and a `10.1101`
        // DOI, so the prefix fallback is the only competing claim.
        let sources = MetadataSources::new().await;
        for server in &sources.servers {
            if server.address().port() == sources.server("pmc_oa").address().port() {
                continue;
            }
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/xml")
                    .set_body_string(
                        "<ListBucketResult><CommonPrefixes><Prefix>PMC7759461.1/</Prefix></CommonPrefixes></ListBucketResult>",
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/metadata/PMC7759461.1.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"pmcid":"PMC7759461","version":1,"is_manuscript":true,
                            "is_pmc_openaccess":false,"license_code":"TDM",
                            "pdf_url":"s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.pdf?md5=ab"}"#,
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        let pacer = Arc::new(RecordingPacer::default());
        let resolution = pass_for(&sources)
            .resolve(
                &recording_client(&pacer, sources.table.clone()),
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1101/2020.01.30.927871"),
                    pmcid: Some("PMC7759461"),
                    ..WorkRefs::default()
                },
            )
            .await;

        let dataset = resolution
            .candidates()
            .find(|candidate| candidate.route == RouteId::PmcOa)
            .expect("the stored version is a candidate");
        assert_eq!(
            dataset.version,
            Some(Version::AuthorManuscript),
            "not the prefix's preprint: {:?}",
            resolution.plan_lines()
        );
        assert!(
            matches!(dataset.version_source, VersionSource::Repository { .. }),
            "and the reason names the repository's field, not the prefix: {}",
            dataset.version_source.why()
        );
        // The derived bioRxiv transform is still a candidate, still a preprint —
        // and now ranks *below* the manuscript, which is the point.
        let bio = resolution
            .candidates()
            .find(|candidate| candidate.route == RouteId::Biorxiv)
            .expect("the 10.1101 transform is still derived");
        assert_eq!(bio.version, Some(Version::Preprint));
        let ranked: Vec<&str> = resolution
            .ranked
            .iter()
            .map(|entry| entry.candidate.route.label())
            .collect();
        assert_eq!(
            ranked.first(),
            Some(&"pmc_oa"),
            "the repository-stated manuscript is ranked first, and the preprint \
             transform below it: {ranked:?}"
        );
    }

    /// **The publisher landing page is offered only for an open-access work**,
    /// and this is the test of the gate rather than of the fetch (ADR-007 §3
    /// step 6: "Landing-page HTML, for OA works only, after TDM").
    ///
    /// The failure this prevents is the one that made the landing page a
    /// **blanket last resort**: with it ungated, every work carrying a DOI
    /// produced a `RouteId::Publisher` candidate, and the ranker's bottom rung —
    /// [`Version::Unstated`] — made it reachable. A work with no OA copy anywhere
    /// then fetched a publisher's page, which is a publisher host on the wire for
    /// a document no reader could have reached for free either. The pass would
    /// report `subscription_read` and `coverage` would read the library as
    /// holding something it does not.
    ///
    /// Three fixtures, three answers, and the middle one is the load-bearing
    /// case: a work no source declared open access gets **no landing page at
    /// all**, not a degraded one.
    #[tokio::test]
    async fn the_publisher_landing_page_is_offered_only_for_an_open_access_work() {
        // ---- (a) Unpaywall says the work is OA and names no location ----
        let open = MetadataSources::new().await;
        // The catch-all 404 goes on **every other** server, and not before the
        // specific mock: `wiremock` resolves to the first registered match, so a
        // blanket mock mounted first would swallow the answer this test is about.
        for server in &open.servers {
            if server.address().port() == open.server("unpaywall").address().port() {
                continue;
            }
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/10.99999/some.suffix.12345"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"doi":"10.99999/some.suffix.12345","is_oa":true}"#),
            )
            .mount(open.server("unpaywall"))
            .await;
        let pacer = Arc::new(RecordingPacer::default());
        let resolution = pass_for(&open)
            .resolve(
                &recording_client(&pacer, open.table.clone()),
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.99999/some.suffix.12345"),
                    ..WorkRefs::default()
                },
            )
            .await;
        assert!(
            resolution
                .candidates()
                .any(|candidate| candidate.route == RouteId::Publisher),
            "an open-access work with no OA copy named still gets its landing \
             page, which is exactly what step 6 permits: {:?}",
            resolution.plan_lines()
        );

        // ---- (b) nothing says the work is open access ----
        let closed = MetadataSources::new().await;
        for server in &closed.servers {
            if server.address().port() == closed.server("unpaywall").address().port() {
                continue;
            }
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        // Unpaywall answers 200 with a **well-formed** record that says `is_oa:
        // false` and names no location — the shape #260 was filed over. A silent
        // 404 would not prove the gate: it would prove nothing was read.
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/10.99999/some.suffix.12345"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"doi":"10.99999/some.suffix.12345","is_oa":false,"best_oa_location":null}"#),
            )
            .mount(closed.server("unpaywall"))
            .await;
        let pacer = Arc::new(RecordingPacer::default());
        let resolution = pass_for(&closed)
            .resolve(
                &recording_client(&pacer, closed.table.clone()),
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.99999/some.suffix.12345"),
                    ..WorkRefs::default()
                },
            )
            .await;
        assert_eq!(
            resolution
                .candidates()
                .filter(|candidate| candidate.route == RouteId::Publisher)
                .count(),
            0,
            "a work nobody declared open access gets **no** landing-page \
             candidate — not a ranked-last one, because reaching a publisher \
             over a free-to-read budget is what step 6 forbids: {:?}",
            resolution.plan_lines()
        );

        // ---- (c) the record's own url is NOT gated, and why ----
        //
        // The same work, but the record carries a URL a person or an importer
        // put there. It survives the gate, because dropping it would silently
        // discard the one thing a human supplied; it ranks on `Unstated` and
        // `SubscriptionRead`, so it only wins when nothing else exists.
        let pacer = Arc::new(RecordingPacer::default());
        let resolution = pass_for(&closed)
            .resolve(
                &recording_client(&pacer, closed.table.clone()),
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.99999/some.suffix.12345"),
                    url: Some("https://example.invalid/paper"),
                    ..WorkRefs::default()
                },
            )
            .await;
        let manual = resolution
            .candidates()
            .find(|candidate| candidate.route == RouteId::ManualUrl)
            .expect("the record's own url is still a candidate");
        assert_eq!(
            manual.version,
            Some(Version::Unstated),
            "and it ranks on the bottom rung, where a last resort belongs: {:?}",
            resolution.plan_lines()
        );
        assert_eq!(manual.licence, LicenceStrength::SubscriptionRead);
    }

    /// **A third party's absolute URL cannot choose this process's network
    /// targets**, which is the new half of "no publisher host" (#254 wave 3).
    ///
    /// The PMC-OA bucket's own JSON hands the pass **absolute `s3://` URLs**, one
    /// per stored version. If the bucket's configured base were not the authority
    /// for what this pass may request, that JSON field would be the thing that
    /// decides which host gets contacted — and it is the *bucket's* JSON, not the
    /// work's record, so it is a stranger's URL picking this process's route. That
    /// is the exact shape ADR-007 §3's resolve-then-rank exists to prevent, arrived
    /// at from a direction nobody was looking.
    ///
    /// Three assertions: the bucket was genuinely asked (a pass that skipped it
    /// would satisfy the criterion trivially), nothing outside it was contacted,
    /// and no candidate carries an `s3://` URL — because the rewritten `pdf_url`
    /// is an absolute URL that lands in `artefacts.source_url`.
    #[tokio::test]
    async fn the_pmc_oa_buckets_own_absolute_urls_cannot_redirect_the_pass() {
        let sources = MetadataSources::new().await;
        // The bucket answers with a listing naming two versions, and each version's
        // JSON carries an `s3://` `pdf_url` for a *different* host — the shape a
        // compromised or rewritten dataset would have.
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/xml")
                    .set_body_string(
                        "<ListBucketResult><CommonPrefixes><Prefix>PMC7759461.1/</Prefix></CommonPrefixes></ListBucketResult>",
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/metadata/PMC7759461.1.json"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(
                        r#"{"pmcid":"PMC7759461","version":1,"is_manuscript":false,
                            "is_pmc_openaccess":true,"license_code":"CC BY",
                            "pdf_url":"s3://pmc-oa-opendata/PMC7759461.1/PMC7759461.1.pdf?md5=4b3fc3a0a63cf2c7c4df3cc1b3d3d25e"}"#,
                    ),
            )
            .mount(sources.server("pmc_oa"))
            .await;
        for server in &sources.servers {
            if server.address().port() == sources.server("pmc_oa").address().port() {
                continue;
            }
            Mock::given(wiremock::matchers::method("GET"))
                .respond_with(ResponseTemplate::new(404).set_body_string("no\n"))
                .mount(server)
                .await;
        }
        let pacer = Arc::new(RecordingPacer::default());
        let client = recording_client(&pacer, sources.table.clone());

        let resolution = pass_for(&sources)
            .resolve(
                &client,
                &WorkScope::new(),
                WorkRefs {
                    doi: Some("10.1038/s41586-020-2649-2"),
                    pmcid: Some("PMC7759461"),
                    ..WorkRefs::default()
                },
            )
            .await;

        // The bucket was asked twice — the listing and the one version — and both
        // hops are recorded as answered, so "no publisher host" was not satisfied
        // by not trying.
        assert_eq!(
            resolution
                .consulted
                .iter()
                .filter(|hop| hop.registry == Registry::PmcOa)
                .count(),
            2,
            "a listing and one per-version read: {:?}",
            resolution.consulted
        );
        let bucket_port = sources.server("pmc_oa").address().port();
        assert_eq!(
            resolution
                .hosts_contacted
                .iter()
                .filter(|host| *host == &format!("127.0.0.1:{bucket_port}"))
                .count(),
            1,
            "one authority, though two requests: {:?}",
            resolution.hosts_contacted
        );

        // And the version's PDF became a candidate — on the bucket's own host.
        let pmc_oa = resolution
            .candidates()
            .find(|candidate| candidate.route == RouteId::PmcOa)
            .expect("the version's PDF is a candidate");
        assert_eq!(
            pmc_oa.url,
            format!(
                "{}/PMC7759461.1/PMC7759461.1.pdf?md5=4b3fc3a0a63cf2c7c4df3cc1b3d3d25e",
                sources.server("pmc_oa").uri()
            ),
            "`s3://` is rewritten to the bucket's configured HTTPS base, which is \
             what makes the candidate fetchable *and* what stops the dataset \
             naming this process's host"
        );
        assert!(
            !pmc_oa.url.starts_with("s3://"),
            "and no candidate ever carries an `s3://` URL: {:?}",
            resolution.plan_lines()
        );
        // The version claim and the licence are the two things this slice exists
        // for, asserted through the ranked plan rather than through the parser.
        assert_eq!(
            pmc_oa.version,
            Some(Version::VersionOfRecord),
            "`is_manuscript: false` is the version of record — PMC's own \
             statement about this file, and the strongest evidence in the pass"
        );
        assert_eq!(pmc_oa.licence, LicenceStrength::OpenLicence);

        // The guard itself, on the URL the bucket's JSON would have handed us.
        for foreign in [
            "http://127.0.0.1:9/PMC7759461.1.pdf",
            "https://pmc-oa-opendata.s3.amazonaws.com.evil.test/x.pdf",
        ] {
            assert!(
                !sources.bases().allows(
                    Registry::PmcOa,
                    &reqwest::Url::parse(foreign).expect("a url")
                ),
                "{foreign} is outside the bucket's configured authority"
            );
        }
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
        let registries = MetadataSources::new().await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("^/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(r#"{"best_oa_location":{"url_for_pdf":"{}","is_oa":true}}"#),
            )
            .mount(registries.server("unpaywall"))
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
        let first = MetadataSources::new().await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/vor.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("%PDF-1.7\n"),
            )
            .mount(first.server("unpaywall"))
            .await;
        // A preprint copy, lower-ranked, also answering.
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/preprint.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("%PDF-1.7\n"),
            )
            .mount(first.server("unpaywall"))
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
                            .replace("VOR", &format!("{}/vor.pdf", first.server("unpaywall").uri()))
                            .replace("PREPRINT", &format!("{}/preprint.pdf", first.server("unpaywall").uri()))
                            .as_str(),
                    ),
            )
            .mount(first.server("unpaywall"))
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
            format!("{}/vor.pdf", first.server("unpaywall").uri()),
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
        let second = MetadataSources::new().await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/vor.pdf"))
            .respond_with(ResponseTemplate::new(404))
            .mount(second.server("unpaywall"))
            .await;
        Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/preprint.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/pdf")
                    .set_body_string("%PDF-1.7\n"),
            )
            .mount(second.server("unpaywall"))
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
                            .replace("VOR", &format!("{}/vor.pdf", second.server("unpaywall").uri()))
                            .replace("PREPRINT", &format!("{}/preprint.pdf", second.server("unpaywall").uri()))
                            .as_str(),
                    ),
            )
            .mount(second.server("unpaywall"))
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
            let asked = second
                .server("unpaywall")
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
