//! What a metadata registry *says a thing is*, in one vocabulary.
//!
//! ADR-007 §3's resolve-then-rank needs two things the identity chain never
//! looked for: whether a record is a preprint, and what licence it carries.
//! Both are **registry-declared facts**, and both are per-registry field
//! shapes — so this module owns the classification and each adapter owns the
//! field it reads.
//!
//! # Why this is its own leaf module
//!
//! Three adapters produce [`RegistryType`] and one consumer ranks it. Putting
//! the type in `resolve.rs` would make `openalex`/`crossref`/`datacite`
//! depend on the ranker to describe their own fields; putting it in any one of
//! them would make the other two depend on a sibling registry's parser. So it
//! sits here, below all of them, and imports nothing but `scitadel-db`.
//!
//! # [`RegistryType::Unrecognised`] is not [`RegistryType::VersionOfRecord`]
//!
//! The distinction is the whole point, and it is the same one
//! `identity_chain.rs` draws between `NotRegistered` and `Unreadable`, and the
//! same one `identity.rs` draws between `Ok` and `Unverified`: **unrecognised
//! is not evidence**. A registry that names a type we have not mapped is not
//! a registry that said "journal article", and a ranker that collapsed the two
//! would rank a dataset above a preprint because it failed to read the
//! word `dataset`.
//!
//! # The three measured shapes
//!
//! | dialect | field | preprint says | VoR says | not-an-article says |
//! |---|---|---|---|---|
//! | Crossref | `message.type` | `posted-content`, `preprint` | `journal-article`, `book`, … | `dataset` |
//! | OpenAlex | `type` | `preprint` | `article` | `dataset` |
//! | DataCite | `types[].resourceTypeGeneral` / `resourceType` | `Preprint` | `Text`, `JournalArticle`, … | `Dataset` |
//!
//! Every list is an **allow-list**, not a deny-list: a dialect word this
//! module has not mapped is [`RegistryType::Unrecognised`], which is a
//! recorded fact about our coverage rather than a guess about the work.
//!
//! # The licence half, and why it is the same module
//!
//! ADR-007 §3 is equally specific about licences: "Crossref `license[]` counts
//! as OA only when it is an allow-listed Creative Commons URL that is in force
//! today (considering `content-version`, `start` and `delay-in-days`). ACS's own
//! policy URLs (`10.15223/policy-*`) don't count." That is a statement about
//! what a registry *says*, made next to the type rules, so it lives here rather
//! than in the ranker: the ranker must not be able to invent a licence, only to
//! compare the ones the registries offered.

use chrono::NaiveDate;
use scitadel_db::sqlite::IdentitySource;

/// A metadata registry the resolve pass consults.
///
/// Deliberately **not** [`IdentitySource`]. That type is the vocabulary
/// ADR-007 §3 enumerates for `paper_identity_checks.source`, and its four
/// registry spellings have no Unpaywall — correctly, because the *identity*
/// chain is "OpenAlex → Crossref → DataCite" and Unpaywall is not in it. This
/// pass reads four registries, so reusing the narrower type would force either
/// a lie (`IdentitySource::OpenAlex` for Unpaywall) or a fifth variant in an
/// enum a database CHECK and a round-trip test already constrain. Two
/// registries with two jobs get two types; [`Registry::identity_source`] is the
/// one honest bridge between them, and it is `None` for Unpaywall because that
/// is the truth rather than a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Registry {
    OpenAlex,
    Crossref,
    DataCite,
    Unpaywall,
    /// Europe PMC's REST search. ADR-007 §3's fifth metadata source
    /// ("Europe PMC (`isOpenAccess`, `inEPMC`)"), added by #254's wave 3.
    EuropePmc,
    /// The `pmc-oa-opendata` bucket's per-version JSON. ADR-007 §3 step 2, and
    /// **not** a registry of records: it keys on a PMCID plus a version number
    /// and answers for one stored file rather than for a work.
    PmcOa,
}

impl Registry {
    /// Every registry, so a vocabulary test can be exhaustive.
    pub const ALL: [Self; 6] = [
        Self::OpenAlex,
        Self::Crossref,
        Self::DataCite,
        Self::Unpaywall,
        Self::EuropePmc,
        Self::PmcOa,
    ];

    /// The lowercase spelling, for a log line or a report.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAlex => "openalex",
            Self::Crossref => "crossref",
            Self::DataCite => "datacite",
            Self::Unpaywall => "unpaywall",
            Self::EuropePmc => "europepmc",
            Self::PmcOa => "pmc_oa",
        }
    }

    /// The `paper_identity_checks.source` spelling, where one exists.
    ///
    /// `None` for everything outside ADR-007 §3's identity chain, and **not**
    /// `Some(OpenAlex)` for any of them:
    ///
    /// - **Unpaywall** — the chain does not include it, and
    ///   [`crate::download`] documents that its `title` is a copy of the
    ///   publisher-supplied title it was handed, so naming it as an identity
    ///   resolver would launder the publisher's own string through a
    ///   verification the verification did not do.
    /// - **Europe PMC** — an aggregator, so its `title` is *another
    ///   aggregator's or a publisher's* title. It would be a fourth laundering
    ///   of the same string, and Europe PMC is in the resolve pass for what it
    ///   says about *versions and licences*, which the three identity registries
    ///   cannot say anything about.
    /// - **The PMC OA dataset** — not a registry of records at all. It has no
    ///   title vocabulary to corroborate: its JSON carries a `title` copied from
    ///   the same deposit Europe PMC indexes, and using it as an identity source
    ///   would make the identity check corroborate a publisher's string with
    ///   that publisher's string.
    #[must_use]
    pub fn identity_source(self) -> Option<IdentitySource> {
        match self {
            Self::OpenAlex => Some(IdentitySource::OpenAlex),
            Self::Crossref => Some(IdentitySource::Crossref),
            Self::DataCite => Some(IdentitySource::DataCite),
            Self::Unpaywall | Self::EuropePmc | Self::PmcOa => None,
        }
    }

    /// The dialect of this registry's type field.
    #[must_use]
    pub fn dialect(self) -> Dialect {
        match self {
            Self::OpenAlex => Dialect::OpenAlex,
            Self::Crossref => Dialect::Crossref,
            Self::DataCite => Dialect::DataCite,
            // The three aggregators and repositories are **not**
            // registration-agency-speaking registries, and these arms are a
            // fallback rather than a statement: none carries a `type` field, so
            // `RegistryType::of` is never called on their behalf and their
            // classification comes from the *licence*, the version flag and the
            // location each names. The arms exist only to keep the match total,
            // and the panics make "someone started reading a type out of
            // Unpaywall / Europe PMC / the PMC OA dataset" loud rather than
            // silent — which matters more for the two PMC sources, because they
            // *do* carry a type-shaped field (`source: MED | PPR`) that is a
            // statement about which database harvested the record, not about
            // the work. Reading that as a work type would rank every `MED`
            // record as a version of record on the strength of a MEDLINE index.
            Self::Unpaywall => unimplemented!("unpaywall carries no type field"),
            Self::EuropePmc => unimplemented!("europe pmc carries no registry type field"),
            Self::PmcOa => unimplemented!("the pmc oa dataset carries no type field"),
        }
    }
}

/// Which rendering of a work a biomedical repository says it holds.
///
/// **Not** a variant of [`RegistryType`], and the difference is the point:
/// `RegistryType` is what a record *is* (`posted-content`, `journal-article`),
/// while this is what a repository says about **the copy it will serve you**.
/// A work has one record-level type and several copies, so the second question
/// is the one the ranker actually needs — and only a repository can answer it.
///
/// Four states, and the fourth is the load-bearing one:
///
/// | state | what said it |
/// |---|---|
/// | [`Self::VersionOfRecord`] | PMC-OA's `is_manuscript: false` |
/// | [`Self::AuthorManuscript`] | any of Europe PMC's three `*AuthMan` flags, or `is_manuscript: true` |
/// | [`Self::Preprint`] | a Europe PMC `source: PPR` record with no manuscript flag |
/// | [`Self::Unstated`] | a `MED` record with every flag `N` — nothing said |
///
/// The last row is why this is a type and not a bool. Europe PMC answering
/// "I hold full text, and none of my manuscript flags is set" is **not** a
/// statement that its copy is the typeset article, and mapping that silence to
/// a version of record would file whatever bytes we fetched as the article of
/// record on no one's word. It maps to [`crate::resolve::Version::Unstated`],
/// which is the ladder's bottom rung — ranked, fetched only if nothing better
/// exists, and recorded as unstated rather than guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryVersion {
    /// The published, typeset article.
    VersionOfRecord,
    /// The peer-reviewed text before a publisher's typesetting.
    AuthorManuscript,
    /// Posted before or instead of peer review.
    Preprint,
    /// A repository named the copy and did not say which version it is.
    Unstated,
}

impl RepositoryVersion {
    /// The `repository_version` this state maps to in [`crate::resolve`].
    ///
    /// The one-way mapping [`crate::resolve::Version::as_artefact_version`]
    /// needs: `Unstated` has no rung of its own and falls through to the route's
    /// fallback, for the reason that function documents.
    #[must_use]
    pub fn as_version(self) -> crate::resolve::Version {
        use crate::resolve::Version;
        match self {
            Self::VersionOfRecord => Version::VersionOfRecord,
            Self::AuthorManuscript => Version::AuthorManuscript,
            Self::Preprint => Version::Preprint,
            Self::Unstated => Version::Unstated,
        }
    }
}

impl From<RepositoryVersion> for crate::resolve::Version {
    fn from(value: RepositoryVersion) -> Self {
        value.as_version()
    }
}

impl std::fmt::Display for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Which registry's vocabulary a type string is written in.
///
/// The three shapes genuinely disagree — `posted-content` (Crossref),
/// `preprint` (OpenAlex), `Preprint` (DataCite) — and none of the three is a
/// superset of another, so a single case-insensitive match would silently
/// mis-read one registry through another's rules. An explicit tag is cheaper
/// than the bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Crossref `message.type`.
    Crossref,
    /// OpenAlex `type` / `type_crossref`.
    OpenAlex,
    /// DataCite `types[].resourceTypeGeneral` / `resourceType`.
    DataCite,
}

/// What a registry's `type` field says the record is.
///
/// Four states, not two, for the reason the module docs give: `Unrecognised`
/// is a fact about *our* allow-lists and `NotAnArticle` is a fact about the
/// *work*, and a ranker that merged them would rank a dataset as a version of
/// record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryType {
    /// The registry calls it a preprint.
    Preprint,
    /// The registry calls it a published article of some kind: the version of
    /// record for our purposes.
    VersionOfRecord,
    /// The registry calls it something that is not a full-text article at all —
    /// a dataset, a piece of software, a collection. Such a record cannot
    /// answer "where is this paper", and saying so is better than ranking it.
    NotAnArticle,
    /// The registry named a type this module has not mapped.
    ///
    /// A recorded gap in our vocabulary, not evidence about the work. The
    /// caller falls back — to another registry, or to the DOI-prefix inference —
    /// and records which it used.
    Unrecognised,
}

impl RegistryType {
    /// Classify one of a registry's own type strings.
    ///
    /// Case-insensitive, because DataCite spells its values in
    /// `CapitalisedCase` and Crossref and OpenAlex in `kebab-case`, and the
    /// three sets overlap (`Text` / `text`) — so the comparison is on a
    /// normalised string and each dialect contributes its own list.
    ///
    /// Every arm is an **allow-list**. An unmapped word is
    /// [`Self::Unrecognised`], never a guess, and `the_three_dialects_are_not_interchangeable`
    /// pins that a word only one dialect uses is read by that dialect alone.
    #[must_use]
    pub fn of(raw: &str, dialect: Dialect) -> Self {
        let word = raw.trim().to_ascii_lowercase();
        match dialect {
            Dialect::Crossref => match word.as_str() {
                // Crossref's own preprint type, and the newer `preprint`
                // spelling its members are migrating to. Both measured on
                // `10.26434/chemrxiv.15000484` and `10.1101/…`.
                "posted-content" | "preprint" => Self::Preprint,
                "journal-article"
                | "proceedings-article"
                | "book"
                | "book-chapter"
                | "book-section"
                | "monograph"
                | "dissertation"
                | "reference-book"
                | "report"
                | "standard"
                | "reference-entry" => Self::VersionOfRecord,
                "dataset" | "software" | "component" | "peer-review" => Self::NotAnArticle,
                _ => Self::Unrecognised,
            },
            Dialect::OpenAlex => match word.as_str() {
                "preprint" => Self::Preprint,
                // OpenAlex's `type` for a peer-reviewed article is `article`;
                // `journal-article` appears in `type_crossref`, which is the
                // Crossref word arriving by the back door, and is mapped here
                // for the same reason.
                "article" | "journal-article" | "book" | "book-chapter" | "book-section"
                | "dissertation" | "report" | "review" => Self::VersionOfRecord,
                "dataset" | "parity" | "software" => Self::NotAnArticle,
                _ => Self::Unrecognised,
            },
            Dialect::DataCite => match word.as_str() {
                "preprint" => Self::Preprint,
                // `Text` is DataCite's general type for journal articles and is
                // by far the most common value on the repository DOIs this
                // slice exists for.
                "text" | "textresource" | "book" | "bookchapter" | "report" | "standard"
                | "dissertation" | "journalarticle" | "journal" | "publication" => {
                    Self::VersionOfRecord
                }
                "dataset"
                | "software"
                | "collection"
                | "image"
                | "audiovisual"
                | "model"
                | "physicalobject"
                | "interactiveresource" => Self::NotAnArticle,
                _ => Self::Unrecognised,
            },
        }
    }

    /// Is this a fact about the work rather than about our vocabulary?
    ///
    /// The three the ranker may act on. [`Self::Unrecognised`] is excluded
    /// deliberately: acting on it is the defaulting this slice exists to stop.
    #[must_use]
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Preprint | Self::VersionOfRecord)
    }

    /// The word a human reads in a plan line, or in the reason a fallback was
    /// recorded.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Preprint => "preprint",
            Self::VersionOfRecord => "version of record",
            Self::NotAnArticle => "not an article",
            Self::Unrecognised => "unrecognised type",
        }
    }
}

/// The host that publishes the licence terms themselves.
///
/// Matched on the host alone rather than on a list of full URLs: Creative
/// Commons has published new licence versions since this table was written, and
/// an allow-list of exact URLs would quietly start rejecting `by/4.0` when
/// `by/5.0` appears. The *shape* is what identifies a CC licence, and anything
/// not on this host is not one.
const CREATIVE_COMMONS_HOST: &str = "creativecommons.org";

/// Path prefixes that identify a Creative Commons licence grant.
///
/// `licenses/<code>/` covers BY, BY-SA, BY-NC, BY-NC-SA and BY-NC-ND at every
/// published version; `publicdomain/` covers CC0 and the Public Domain Mark,
/// which grant the same freedom and are the terms an institutional repository
/// most often deposits.
const CREATIVE_COMMONS_PATHS: [&str; 2] = ["/licenses/", "/publicdomain/"];

/// A DOI prefix whose `license[]` entries are **publisher policy URLs**, not
/// licences.
///
/// ADR-007 §3 names it explicitly: "ACS's own policy URLs (`10.15223/policy-*`)
/// don't count." A policy URL states *where the publisher's terms live*, not
/// what we may do with the bytes, so counting one as a licence would make
/// `access_basis = 'oa_license'` a claim about permission that nobody made.
/// Named as a prefix rather than a full URL because ACS registers under several
/// and the shape is what matters.
const PUBLISHER_POLICY_PREFIX: &str = "10.15223/policy-";

/// Why an offered licence does not count, or `None` when it does.
///
/// The three refusals are kept apart for the same reason the registry type's
/// states are: they are different facts about different things, and a ranker
/// that collapsed them into "not a licence" could not say *why* in the plan a
/// candidate's licence is unknown, which is the one sentence a person reading a
/// dry run actually needs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "refused_because", rename_all = "snake_case")]
pub enum LicenceRefusal {
    /// Not a Creative Commons URL at all, and not on an allow-listed host.
    NotCreativeCommons(String),
    /// A publisher policy URL (`10.15223/policy-*`), which states terms without
    /// granting them.
    PublisherPolicy(String),
    /// `start` is in the future: the licence is announced but not yet in force.
    NotYetInForce(String),
    /// `delay-in-days` has not elapsed since `start`.
    InForceLater { start: String, delay_in_days: i64 },
    /// `content-version` names a version of the work this licence does not
    /// cover — a `vor`-only licence does not license the preprint we would fetch.
    WrongContentVersion { content_version: String },
    /// A date we could not read, so "in force today" is unknown.
    UnreadableDate(String),
}

/// One `license[]` entry as a registry offered it, plus the verdict on it.
///
/// Crossref's `license[]` and DataCite's `rightsList[]` are the same shape of
/// fact under two names, and neither is a licence until three separate things
/// hold: the URL is an allow-listed CC grant, the grant is in force today, and
/// it covers the version of the work we would actually fetch.
///
/// `Serialize` because the whole offer travels with the ranked candidate that
/// won on it: a plan line that says `OpenLicence` without the URL that
/// established it cannot be checked, and the manifest mirror's four licence
/// columns are this struct verbatim.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LicenceOffer {
    /// The URL the registry gave, verbatim.
    pub url: String,
    /// `content-version`, when the registry declares one.
    pub content_version: Option<String>,
    /// `start`, when the registry declares one.
    pub start: Option<String>,
    /// `delay-in-days`, when the registry declares one.
    pub delay_in_days: Option<i64>,
    /// Which registry offered it.
    pub registry: Registry,
}

impl LicenceOffer {
    /// Read a Crossref `license[]` array.
    ///
    /// Entries are `{ "URL": …, "content-version": …, "start": …,
    /// "delay-in-days": … }`; any entry that is not an object with a `URL`
    /// string contributes nothing rather than a placeholder, which is the same
    /// rule `parse_work` follows for identity facts.
    #[must_use]
    pub fn from_crossref(message: &serde_json::Value) -> Vec<Self> {
        message
            .get("license")
            .and_then(serde_json::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| Self::from_entry(entry, Registry::Crossref))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Read a DataCite `rightsList[]` array.
    ///
    /// `rights` rather than `rightsUri` is what carries the CC grant on a
    /// DataCite record, but a record sometimes carries only the URI, so both
    /// are read and the URI is preferred when both are present — the URL is
    /// what the allow-list can actually check.
    #[must_use]
    pub fn from_datacite(attributes: &serde_json::Value) -> Vec<Self> {
        attributes
            .get("rightsList")
            .and_then(serde_json::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        let url = entry
                            .get("rightsUri")
                            .and_then(serde_json::Value::as_str)
                            .or_else(|| entry.get("rights").and_then(serde_json::Value::as_str))?;
                        let mut offer = Self::from_entry(
                            &serde_json::json!({ "URL": url }),
                            Registry::DataCite,
                        )
                        .unwrap_or_else(|| Self {
                            url: url.to_string(),
                            content_version: None,
                            start: None,
                            delay_in_days: None,
                            registry: Registry::DataCite,
                        });
                        // DataCite carries the date window under
                        // `dates`, not on the rights entry.
                        if let Some(dates) = entry.get("dates").and_then(|d| d.get("date")) {
                            offer.start = Some(dates.as_str().unwrap_or_default().to_string());
                        }
                        Some(offer)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Read a Unpaywall location's `license` string.
    ///
    /// A bare string rather than an object, which is why this is its own
    /// constructor rather than a reuse of [`Self::from_crossref`] with a
    /// reshaped value: the three registries genuinely disagree on shape and
    /// `datacite.rs`/`crossref.rs` both document that disagreement rather than
    /// smoothing it into a fictional common type.
    #[must_use]
    pub fn from_unpaywall(location: &serde_json::Value) -> Option<Self> {
        location
            .get("license")
            .and_then(serde_json::Value::as_str)
            .filter(|url| !url.trim().is_empty())
            .map(|url| Self {
                url: url.trim().to_string(),
                content_version: location
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                start: None,
                delay_in_days: None,
                registry: Registry::Unpaywall,
            })
    }

    /// Read Europe PMC's `license` short code, or the PMC OA dataset's
    /// `license_code`.
    ///
    /// **A code, not a URL**, and that is the whole reason this is its own
    /// constructor rather than a reuse of [`Self::from_entry`]: Europe PMC
    /// writes `cc by-nc-nd` where Crossref writes
    /// `https://creativecommons.org/licenses/by-nc-nd/4.0/`. Measured values
    /// across a `MED` record, a `PPR` record and two PMC-OA versions:
    ///
    /// | source | codes seen |
    /// |---|---|
    /// | Europe PMC `license` | `cc by`, `cc by-nc`, `cc by-nc-sa`, `cc by-nd`, `cc by-sa`, `cc0` |
    /// | PMC OA `license_code` | `CC BY`, `CC BY-NC-ND`, `CC0`, `TDM` |
    ///
    /// The code names a **licence family**, not a version, so the URL this
    /// synthesises carries the version the Creative Commons licence was
    /// published at rather than one the repository chose. That is a deliberate
    /// narrowing: the allow-list checks host plus path *shape* precisely so a
    /// new CC version appearing cannot start being refused, so the version in
    /// the URL cannot change any verdict, and inventing one is cheaper than
    /// refusing a grant a repository plainly stated.
    ///
    /// **`TDM` is not a licence and must not become one.** The bucket's own
    /// README defines it: author manuscripts "where the full text is available
    /// for text mining, and where the full text may also be used consistent
    /// with the principles of fair use". That is a text-mining permission, and
    /// reading it as a reuse grant would put `access_basis = 'oa_license'` on
    /// bytes nobody granted reuse of — #261's bug, reached through a code that
    /// looks like the others. It returns `None`, which is the honest answer and
    /// is what puts the candidate on `FreeToRead`.
    #[must_use]
    pub fn from_pmc_code(code: &str, registry: Registry) -> Option<Self> {
        let normalised = code
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_', ' '], "");
        // `cc0` normalises to `cc0` and `CC0` to `cc0`; the `TDM` case is the
        // one that must fall through, and it does so by not being in the table
        // rather than by a special arm — a special arm would be one more place
        // to forget the rule.
        let path = match normalised.as_str() {
            "cc0" | "cczero" => "publicdomain/zero/1.0",
            "ccby" => "licenses/by/4.0",
            "ccby-sa" | "ccbysa" => "licenses/by-sa/4.0",
            "ccby-nd" | "ccbynd" => "licenses/by-nd/4.0",
            "ccby-nc" | "ccbync" => "licenses/by-nc/4.0",
            "ccby-nc-sa" | "ccbyncsa" => "licenses/by-nc-sa/4.0",
            "ccby-nc-nd" | "ccbyncnd" => "licenses/by-nc-nd/4.0",
            _ => return None,
        };
        Some(Self {
            url: format!("https://creativecommons.org/{path}/"),
            content_version: None,
            start: None,
            delay_in_days: None,
            registry,
        })
    }

    fn from_entry(entry: &serde_json::Value, registry: Registry) -> Option<Self> {
        let url = entry.get("URL").and_then(serde_json::Value::as_str)?;
        Some(Self {
            url: url.trim().to_string(),
            content_version: entry
                .get("content-version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            start: entry
                .get("start")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            delay_in_days: entry
                .get("delay-in-days")
                .and_then(serde_json::Value::as_i64),
            registry,
        })
    }

    /// Does this offer license `version` of the work, as of `today`?
    ///
    /// `None` means it counts, and every `Some` carries the reason it does not —
    /// the plan prints those reasons, because "no licence found" and "the
    /// publisher's policy URL does not count" are different sentences and only
    /// one of them is actionable.
    ///
    /// The three checks are ADR-007 §3's, in the order it states them:
    ///
    /// 1. an **allow-listed CC URL**. The host must be
    ///    `creativecommons.org` and the path must be a licence or public-domain
    ///    grant; and a `10.15223/policy-*` URL is refused by name even though it
    ///    sits on a CC-looking path, because it is a link to a publisher's terms
    ///    rather than a grant.
    /// 2. **in force today**, considering `start` and `delay-in-days`.
    /// 3. covering **this content-version**. A `vor`-only licence does not
    ///    license the preprint or the author manuscript we would fetch, and
    ///    applying it anyway is the overclaim #261 exists to stop.
    pub fn counts_for(&self, version: &str, today: NaiveDate) -> Option<LicenceRefusal> {
        if let Some(refusal) = self.url_refusal() {
            return Some(refusal);
        }
        if let Some(declared) = self
            .content_version
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && !same_content_version(declared, version)
        {
            return Some(LicenceRefusal::WrongContentVersion {
                content_version: declared.to_string(),
            });
        }
        // A delay with no `start` to count from is a window we cannot
        // evaluate, so it is refused rather than assumed elapsed — the same
        // refusal as one that has demonstrably not elapsed yet, because in both
        // cases the honest answer is "we cannot tell".
        let Some(start) = self
            .start
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return self.delay_in_days.filter(|delay| *delay > 0).map(|delay| {
                LicenceRefusal::InForceLater {
                    start: String::from("a start date the registry did not give"),
                    delay_in_days: delay,
                }
            });
        };
        let Some(from) = read_date(start) else {
            return Some(LicenceRefusal::UnreadableDate(start.to_string()));
        };
        if from > today {
            return Some(LicenceRefusal::NotYetInForce(start.to_string()));
        }
        if let Some(delay) = self.delay_in_days.filter(|delay| *delay > 0) {
            // A delay is a count of days, and a negative or absurd one is a
            // field we cannot evaluate rather than a licence we may apply.
            let elapsed = u64::try_from(delay)
                .ok()
                .map(|days| from + chrono::Duration::days(i64::try_from(days).unwrap_or(i64::MAX)));
            if elapsed.is_none_or(|in_force| in_force > today) {
                return Some(LicenceRefusal::InForceLater {
                    start: start.to_string(),
                    delay_in_days: delay,
                });
            }
        }
        None
    }

    /// The allow-list check alone, with no dates and no version.
    ///
    /// Separated because it is also what the *candidate collection* asks: a
    /// location carrying only an unusable licence string still has to be
    /// reported, with the reason.
    #[must_use]
    pub fn url_refusal(&self) -> Option<LicenceRefusal> {
        let url = self.url.trim();
        if url.to_ascii_lowercase().contains(PUBLISHER_POLICY_PREFIX) {
            return Some(LicenceRefusal::PublisherPolicy(url.to_string()));
        }
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return Some(LicenceRefusal::NotCreativeCommons(url.to_string()));
        };
        let host_ok = parsed
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(CREATIVE_COMMONS_HOST));
        // The prefix alone is not enough: `creativecommons.org/licenses/` is
        // an index page, not a grant, so something must follow it.
        let path = parsed.path().to_ascii_lowercase();
        let path_ok = CREATIVE_COMMONS_PATHS.iter().any(|prefix| {
            path.strip_prefix(prefix)
                .is_some_and(|rest| !rest.trim_matches('/').is_empty())
        });
        if host_ok && path_ok {
            None
        } else {
            Some(LicenceRefusal::NotCreativeCommons(url.to_string()))
        }
    }
}

/// Does a Crossref `content-version` grant cover `version` of the work?
///
/// Crossref's values are `vor` (version of record), `am` (author manuscript) and
/// `vor_am`, and DataCite's finer `resourceType` uses the long spellings. The
/// comparison is therefore on normalised synonyms rather than string equality —
/// but **only** for the values this module knows, so a value neither side
/// recognises does not accidentally match the other by accident of casing.
fn same_content_version(declared: &str, version: &str) -> bool {
    const VOR: [&str; 5] = [
        "vor",
        "versionofrecord",
        "final",
        "published",
        "version of record",
    ];
    const AM: [&str; 5] = [
        "am",
        "authormanuscript",
        "acceptedmanuscript",
        "am-only",
        "author manuscript",
    ];
    const VOR_AM: [&str; 4] = ["vor_am", "voram", "vor+am", "vor and am"];
    let normalise = |value: &str| {
        value
            .trim()
            .to_ascii_lowercase()
            .replace(['_', '+', ' '], "")
    };
    let declared = normalise(declared);
    let version = normalise(version);
    let group = |value: &str| {
        if VOR.contains(&value) {
            "vor"
        } else if AM.contains(&value) {
            "am"
        } else if VOR_AM.contains(&value) {
            "vor_am"
        } else {
            ""
        }
    };
    let left = group(&declared);
    // An unknown grant name covers nothing we can reason about, so it is
    // refused rather than treated as a wildcard.
    let wanted = group(&version);
    !left.is_empty()
        && (left == wanted
            // `vor_am` is Crossref's grant for *both* the version of record and
            // the author manuscript behind it, so it covers either — and
            // neither of the other two, because a preprint is a third content
            // version the registry did not speak for.
            || (left == "vor_am" && matches!(wanted, "vor" | "am")))
}

/// Read a registry's date field as a calendar date.
///
/// Crossref writes `start` as `2025-01-02` or `2025-01-02T00:00:00Z` and
/// DataCite's `dates.date` as the same, so the leading date is taken and the
/// time part — which never decides whether a licence is in force today — is
/// ignored. A date we cannot read is [`LicenceRefusal::UnreadableDate`] and
/// never a licence.
fn read_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.fZ"))
        .or_else(|_| NaiveDate::parse_from_str(raw, "%Y-%m-%dT%H:%M:%SZ"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 1).expect("a date")
    }

    /// The whole point of the four-state type is that the two "no" states stay
    /// apart, so they are asserted apart, in both directions.
    #[test]
    fn unrecognised_is_not_an_article_and_neither_is_a_preprint() {
        assert_ne!(RegistryType::Unrecognised, RegistryType::NotAnArticle);
        assert_ne!(RegistryType::Unrecognised, RegistryType::VersionOfRecord);
        assert!(!RegistryType::Unrecognised.is_usable());
        assert!(!RegistryType::NotAnArticle.is_usable());
        assert!(RegistryType::Preprint.is_usable());
        assert!(RegistryType::VersionOfRecord.is_usable());
    }

    /// The measured preprint spellings, each in its own registry's dialect.
    /// A record carrying one of these words is a preprint because the registry
    /// **says so** — not because its DOI prefix looked like a preprint server.
    #[test]
    fn each_registrys_own_preprint_spelling_is_recognised() {
        assert_eq!(
            RegistryType::of("posted-content", Dialect::Crossref),
            RegistryType::Preprint
        );
        assert_eq!(
            RegistryType::of("preprint", Dialect::OpenAlex),
            RegistryType::Preprint
        );
        assert_eq!(
            RegistryType::of("Preprint", Dialect::DataCite),
            RegistryType::Preprint
        );
        // And case is normalised, because DataCite capitalises and the others
        // do not.
        assert_eq!(
            RegistryType::of("POSTED-CONTENT", Dialect::Crossref),
            RegistryType::Preprint
        );
        assert_eq!(
            RegistryType::of("preprint", Dialect::DataCite),
            RegistryType::Preprint
        );
    }

    /// The VoR half, which is what makes a `journal-article` record rank above
    /// a preprint rather than merely not-rank-below-it.
    #[test]
    fn each_registrys_own_article_spelling_is_a_version_of_record() {
        assert_eq!(
            RegistryType::of("journal-article", Dialect::Crossref),
            RegistryType::VersionOfRecord
        );
        assert_eq!(
            RegistryType::of("article", Dialect::OpenAlex),
            RegistryType::VersionOfRecord
        );
        assert_eq!(
            RegistryType::of("Text", Dialect::DataCite),
            RegistryType::VersionOfRecord
        );
    }

    /// A word one dialect uses must not be read through another's rules. This
    /// is the difference between three adapters with genuinely disjoint field
    /// vocabularies and one case-insensitive `match` that guesses.
    #[test]
    fn the_three_dialects_are_not_interchangeable() {
        // `dataset` is mapped in all three, deliberately, so the *not an
        // article* verdict travels. These are the words that must not.
        assert_eq!(
            RegistryType::of("posted-content", Dialect::OpenAlex),
            RegistryType::Unrecognised,
            "Crossref's preprint word is not OpenAlex's"
        );
        assert_eq!(
            RegistryType::of("journal-article", Dialect::DataCite),
            RegistryType::Unrecognised,
            "Crossref's article word is not DataCite's"
        );
        assert_eq!(
            RegistryType::of("Text", Dialect::OpenAlex),
            RegistryType::Unrecognised,
            "DataCite's word is not OpenAlex's"
        );
        // And a word nobody maps stays unmapped rather than becoming a VoR.
        for dialect in [Dialect::Crossref, Dialect::OpenAlex, Dialect::DataCite] {
            assert_eq!(
                RegistryType::of("something-new-2027", dialect),
                RegistryType::Unrecognised,
                "{dialect:?} must not guess at a type it has not mapped"
            );
            assert_eq!(
                RegistryType::of("", dialect),
                RegistryType::Unrecognised,
                "{dialect:?}: an absent type is not an article"
            );
        }
    }

    /// `Registry` and `IdentitySource` overlap on exactly the three registries
    /// ADR-007 §3's identity chain names, and the three that are *not* in that
    /// chain must be refused rather than laundered onto another's spelling.
    #[test]
    fn the_two_registry_vocabularies_overlap_exactly_where_the_identity_chain_does() {
        let labels: HashSet<&str> = Registry::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(
            labels,
            HashSet::from([
                "openalex",
                "crossref",
                "datacite",
                "unpaywall",
                "europepmc",
                "pmc_oa"
            ]),
            "six sources, and no two of them share a spelling"
        );
        for registry in Registry::ALL {
            match registry.identity_source() {
                Some(source) => assert_eq!(source.label(), registry.label(), "{registry}"),
                None => assert!(
                    matches!(
                        registry,
                        Registry::Unpaywall | Registry::EuropePmc | Registry::PmcOa
                    ),
                    "only the three sources ADR-007 §3's identity chain does not \
                     name may borrow no spelling, and they must not borrow a \
                     registry's: {registry}"
                ),
            }
        }
    }

    /// The two PMC sources are in the resolve pass for what they say about
    /// **versions and licences**, and neither is an identity source — so their
    /// `dialect()` is a loud panic rather than a guess.
    ///
    /// Europe PMC carries a `source: MED | PPR | PMC` field that looks like a
    /// type and is not one: it names the database the record was harvested from,
    /// so reading it as a work type would rank every `MED` record as a version of
    /// record on the strength of a MEDLINE index entry.
    #[test]
    #[should_panic(expected = "europe pmc carries no registry type field")]
    fn europe_pmc_carries_no_registry_type_field() {
        let _ = Registry::EuropePmc.dialect();
    }

    #[test]
    #[should_panic(expected = "the pmc oa dataset carries no type field")]
    fn the_pmc_oa_dataset_carries_no_type_field() {
        let _ = Registry::PmcOa.dialect();
    }

    /// `is_manuscript: false` is the **only** thing in the whole metadata pass
    /// that says a specific file is the version of record, and it is a statement
    /// about a copy rather than about a record.
    ///
    /// Asserted as a mapping rather than through the ranker because the ranker's
    /// contribution is `Version` itself, which the resolve module owns; what is
    /// new here is the four-state vocabulary and the fact that silence is its
    /// own state rather than a version of record.
    #[test]
    fn a_repository_states_which_copy_it_holds_in_four_states() {
        use RepositoryVersion::*;
        assert_eq!(
            VersionOfRecord.as_version(),
            crate::resolve::Version::VersionOfRecord
        );
        assert_eq!(
            AuthorManuscript.as_version(),
            crate::resolve::Version::AuthorManuscript
        );
        assert_eq!(Preprint.as_version(), crate::resolve::Version::Preprint);
        // And the fourth state is the load-bearing one: a repository naming a
        // copy and not characterising it is `Unstated`, **not** a version of
        // record.
        assert_eq!(Unstated.as_version(), crate::resolve::Version::Unstated);
        assert_ne!(
            Unstated.as_version(),
            crate::resolve::Version::VersionOfRecord,
            "mapping 'the repository did not say' onto `vor` is the overclaim \
             #261 is about: it files whatever bytes came back as the article of \
             record on nobody's word"
        );
        assert_eq!(
            Unstated.as_version().as_artefact_version(),
            None,
            "and `Unstated` writes no column value, falling through to the \
             route's own claim — the same precedence the resolve ladder uses"
        );
    }

    /// The measured Creative Commons short codes, and the one that is **not** a
    /// licence.
    ///
    /// `TDM` is the case that matters: it marks an author manuscript available
    /// for text mining under fair use, and reading it as a reuse grant would put
    /// `access_basis = 'oa_license'` on bytes nobody granted reuse of. It returns
    /// `None` rather than a special-cased refusal, so a code added later is
    /// refused the same way by construction.
    #[test]
    fn a_pmc_licence_code_becomes_a_cc_grant_and_tdm_becomes_nothing() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 1).expect("a date");
        for (code, want) in [
            ("cc by", "licenses/by"),
            ("CC BY", "licenses/by"),
            ("cc by-nc-nd", "licenses/by-nc-nd"),
            ("CC BY-NC-ND", "licenses/by-nc-nd"),
            ("cc by-nc-sa", "licenses/by-nc-sa"),
            ("cc by-sa", "licenses/by-sa"),
            ("cc by-nc", "licenses/by-nc"),
            ("cc by-nd", "licenses/by-nd"),
            ("cc0", "publicdomain/zero"),
            ("CC0", "publicdomain/zero"),
        ] {
            let offer = LicenceOffer::from_pmc_code(code, Registry::EuropePmc)
                .unwrap_or_else(|| panic!("{code} is a measured CC code"));
            assert!(
                offer.url.contains(want),
                "{code} -> {want}, got {}",
                offer.url
            );
            assert!(
                offer.counts_for("vor", today).is_none(),
                "{code} must count as an in-force grant: {:?}",
                offer.counts_for("vor", today)
            );
        }
        // The text-mining marker, and every code nobody has mapped.
        for code in [
            "TDM",
            "tdm",
            "",
            "   ",
            "CC BY-XYZ",
            "10.15223/policy-a",
            "public domain",
        ] {
            assert_eq!(
                LicenceOffer::from_pmc_code(code, Registry::PmcOa),
                None,
                "{code:?} is not an allow-listed reuse grant"
            );
        }
    }

    /// A CC URL on the right host and path counts, whatever its version number
    /// — a new licence version appearing must not silently start being refused,
    /// which is why the check is on host plus path *shape* and not on a list of
    /// exact URLs.
    #[test]
    fn a_creative_commons_grant_counts() {
        for url in [
            "https://creativecommons.org/licenses/by/4.0/",
            "http://creativecommons.org/licenses/by-nc-sa/3.0/de/",
            "https://creativecommons.org/licenses/by/5.0/",
            "https://creativecommons.org/publicdomain/zero/1.0/",
            "https://creativecommons.org/publicdomain/mark/1.0/",
        ] {
            let offer = LicenceOffer {
                url: url.to_string(),
                content_version: None,
                start: None,
                delay_in_days: None,
                registry: Registry::Crossref,
            };
            assert_eq!(
                offer.counts_for("vor", today()),
                None,
                "{url} is a CC grant"
            );
        }
    }

    /// Everything that is *not* a CC grant, including the one the ADR names by
    /// name. The policy-URL case is asserted separately below because it is the
    /// one a naive host check would wave through.
    #[test]
    fn only_a_creative_commons_grant_counts() {
        for url in [
            "https://example.org/open-access",
            "https://www.sciencedirect.com/science/article/pii/x",
            "https://creativecommons.org/",
            "https://creativecommons.org/licenses/",
            "not a url at all",
        ] {
            let offer = LicenceOffer {
                url: url.to_string(),
                content_version: None,
                start: None,
                delay_in_days: None,
                registry: Registry::Crossref,
            };
            assert!(
                matches!(
                    offer.counts_for("vor", today()),
                    Some(LicenceRefusal::NotCreativeCommons(_))
                ),
                "{url} is not a CC grant and must be refused"
            );
        }
    }

    /// ADR-007 §3, verbatim: "ACS's own policy URLs (`10.15223/policy-*`) don't
    /// count." A policy URL *links* to terms; it does not grant anything, and
    /// treating it as a licence would make `access_basis = 'oa_license'` a
    /// permission nobody stated.
    #[test]
    fn acs_policy_urls_are_not_licences() {
        for url in [
            "https://doi.org/10.15223/policy-2015-06-30",
            "https://acs.figshare.com/articles/journal_contribution/10.15223/policy-2018-01-01",
            "http://dx.doi.org/10.15223/policy-2013-04-03",
            // The same prefix under a different case, because a DOI's suffix
            // case is normalised on the way in and this check must not depend
            // on which registrar minted it.
            "https://doi.org/10.15223/POLICY-2020-02-02",
        ] {
            let offer = LicenceOffer {
                url: url.to_string(),
                content_version: None,
                start: None,
                delay_in_days: None,
                registry: Registry::Crossref,
            };
            assert_eq!(
                offer.url_refusal(),
                Some(LicenceRefusal::PublisherPolicy(url.to_string())),
                "{url} states where the terms live; it does not grant them"
            );
        }
    }

    /// `start` and `delay-in-days`, both directions. A licence announced for
    /// the future is not a licence today, and a delay we cannot evaluate
    /// without a `start` is not a licence either.
    #[test]
    fn a_licence_counts_only_when_it_is_in_force_today() {
        let base = LicenceOffer {
            url: "https://creativecommons.org/licenses/by/4.0/".to_string(),
            content_version: None,
            start: None,
            delay_in_days: None,
            registry: Registry::Crossref,
        };
        // Long in force.
        let past = LicenceOffer {
            start: Some("2020-01-01".into()),
            ..base.clone()
        };
        assert_eq!(past.counts_for("vor", today()), None);
        // Announced, not yet in force.
        let future = LicenceOffer {
            start: Some("2027-01-01".into()),
            ..base.clone()
        };
        assert!(matches!(
            future.counts_for("vor", today()),
            Some(LicenceRefusal::NotYetInForce(_))
        ));
        // In force, but delayed past today.
        let delayed = LicenceOffer {
            start: Some("2026-09-25".into()),
            delay_in_days: Some(30),
            ..base.clone()
        };
        assert!(matches!(
            delayed.counts_for("vor", today()),
            Some(LicenceRefusal::InForceLater {
                delay_in_days: 30,
                ..
            })
        ));
        // The same delay with more time elapsed does count.
        let elapsed = LicenceOffer {
            start: Some("2026-01-01".into()),
            delay_in_days: Some(30),
            ..base.clone()
        };
        assert_eq!(elapsed.counts_for("vor", today()), None);
        // A delay with no window to count from is not evaluable.
        let orphan = LicenceOffer {
            delay_in_days: Some(12),
            ..base.clone()
        };
        assert!(matches!(
            orphan.counts_for("vor", today()),
            Some(LicenceRefusal::InForceLater {
                delay_in_days: 12,
                ..
            })
        ));
        // And a date we cannot read is never a licence.
        let unreadable = LicenceOffer {
            start: Some("whenever".into()),
            ..base
        };
        assert!(matches!(
            unreadable.counts_for("vor", today()),
            Some(LicenceRefusal::UnreadableDate(_))
        ));
    }

    /// `content-version` is a real constraint, not decoration: a `vor`-only
    /// grant does not license the preprint or the author manuscript we would
    /// fetch, and applying it anyway is the overclaim #261 exists to stop.
    #[test]
    fn a_content_version_narrower_than_the_candidate_does_not_count() {
        let vor_only = LicenceOffer {
            url: "https://creativecommons.org/licenses/by/4.0/".to_string(),
            content_version: Some("vor".into()),
            start: None,
            delay_in_days: None,
            registry: Registry::Crossref,
        };
        assert_eq!(vor_only.counts_for("vor", today()), None);
        assert!(matches!(
            vor_only.counts_for("preprint", today()),
            Some(LicenceRefusal::WrongContentVersion { .. })
        ));
        assert!(matches!(
            vor_only.counts_for("am", today()),
            Some(LicenceRefusal::WrongContentVersion { .. })
        ));
        // `vor_am` is the grant that covers both the published version and the
        // manuscript behind it.
        let both = LicenceOffer {
            content_version: Some("vor_am".into()),
            ..vor_only
        };
        assert_eq!(both.counts_for("vor", today()), None);
        assert_eq!(both.counts_for("am", today()), None);
        // And a content-version nobody has mapped covers nothing.
        let exotic = LicenceOffer {
            content_version: Some("supplementary".into()),
            ..both
        };
        assert!(matches!(
            exotic.counts_for("vor", today()),
            Some(LicenceRefusal::WrongContentVersion { .. })
        ));
    }

    /// The three registries' three shapes, read from records in each one's own
    /// envelope. A test that served one shape to all three would pass for the
    /// wrong reason, which is why each fixture is that registry's real shape.
    #[test]
    fn each_registrys_licence_shape_is_read_where_it_actually_is() {
        let crossref = serde_json::json!({
            "license": [
                { "URL": "https://creativecommons.org/licenses/by/4.0/", "content-version": "vor" },
                { "URL": "https://example.org/tms" }
            ]
        });
        let offers = LicenceOffer::from_crossref(&crossref);
        assert_eq!(offers.len(), 2, "an unparseable entry contributes nothing");
        assert_eq!(offers[0].registry, Registry::Crossref);
        assert_eq!(offers[0].content_version.as_deref(), Some("vor"));
        assert_eq!(offers[1].content_version, None);
        assert!(
            offers[1].counts_for("vor", today()).is_some(),
            "a non-CC URL in the same array does not count"
        );

        let datacite = serde_json::json!({
            "rightsList": [
                { "rights": "CC BY 4.0", "rightsUri": "https://creativecommons.org/licenses/by/4.0/",
                  "dates": { "date": "2021-05-06" } }
            ]
        });
        let offers = LicenceOffer::from_datacite(&datacite);
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].registry, Registry::DataCite);
        assert_eq!(
            offers[0].url,
            "https://creativecommons.org/licenses/by/4.0/"
        );
        assert_eq!(offers[0].start.as_deref(), Some("2021-05-06"));
        assert_eq!(offers[0].counts_for("vor", today()), None);

        let unpaywall = serde_json::json!({
            "url_for_pdf": "https://example.org/p.pdf",
            "license": "https://creativecommons.org/licenses/by-sa/4.0/",
            "version": "vor"
        });
        let offer = LicenceOffer::from_unpaywall(&unpaywall).expect("a licence string");
        assert_eq!(offer.registry, Registry::Unpaywall);
        assert_eq!(offer.content_version.as_deref(), Some("vor"));
        assert_eq!(offer.counts_for("vor", today()), None);
        assert_eq!(
            LicenceOffer::from_unpaywall(&serde_json::json!({})),
            None,
            "a location with no licence string offers none — and offers `None`, \
             not an empty licence"
        );
    }

    /// An absent licence array is no licence, and is read as an empty list
    /// rather than as a licence with blank fields. This is the same
    /// "absent fact is never agreement" rule `identity.rs` states throughout.
    #[test]
    fn an_absent_licence_offers_nothing() {
        assert!(LicenceOffer::from_crossref(&serde_json::json!({})).is_empty());
        assert!(LicenceOffer::from_datacite(&serde_json::json!({})).is_empty());
        assert!(LicenceOffer::from_crossref(&serde_json::json!({ "license": "by" })).is_empty());
        assert!(
            LicenceOffer::from_crossref(&serde_json::json!({ "license": [{}, 3, null] }))
                .is_empty(),
            "entries that are not licence objects contribute nothing"
        );
    }
}
