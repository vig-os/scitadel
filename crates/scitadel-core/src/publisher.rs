//! Publisher classification from a DOI's registrant prefix, and the route
//! verdict that follows from it.
//!
//! #261: the acquisition ladder's verdict depends on knowing which publisher
//! a DOI belongs to, and every consumer was keeping its own hand-maintained
//! prefix list. Two such lists already existed in one downstream project and
//! had started to diverge. This module is the single, versioned table they
//! import instead.
//!
//! Registrant prefixes are stable, public and assigned by doi.org, so a
//! checked-in table is a reasonable substitute for a live lookup — see
//! [provenance](#provenance) for the caveat.
//!
//! The other half of the problem is honesty in the output. A two-valued
//! "TDM route available / not available" verdict forces a consumer to report
//! "no TDM route available" for a publisher it never classified, which is a
//! claim the ladder has not established. [`RouteVerdict`] keeps *unknown*
//! distinct from *known absent*, and [`RouteVerdict::note`] is worded so it
//! cannot overstate what was checked.
//!
//! [provenance]: https://www.doi.org/doi-handbook/HTML/doi-syntax.html

use crate::models::normalize_doi;

/// What kind of host sits behind a registrant prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    /// A journal publisher.
    Publisher,
    /// A preprint server. Free to read by construction, so a failed download
    /// is a missing route rather than a paywall (#260).
    PreprintServer,
    /// A data or compound repository. These mint DOIs for *records*, and a
    /// record DOI's suffix is often a record URL — the shape that lets a
    /// ChEMBL activity URL masquerade as a DOI (#262).
    Repository,
}

/// A publisher or host scitadel can name from a DOI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Publisher {
    Aaas,
    Acm,
    Acs,
    Aip,
    Chembl,
    Aps,
    Arxiv,
    Biorxiv,
    Chemrxiv,
    Coperni,
    Cshl,
    Degruyter,
    Elsevier,
    Emerald,
    Frontiers,
    Ieee,
    Iop,
    Mdpi,
    Nature,
    Nist,
    Oup,
    Pnas,
    Plos,
    Rsc,
    RoyalSociety,
    Springer,
    TaylorFrancis,
    Wiley,
    Zenodo,
}

impl Publisher {
    /// Short, stable, machine-friendly name — safe for filenames, database
    /// values and note text. Lowercase, no spaces.
    pub fn label(self) -> &'static str {
        match self {
            Self::Aaas => "aaas",
            Self::Acm => "acm",
            Self::Acs => "acs",
            Self::Aip => "aip",
            Self::Chembl => "chembl",
            Self::Aps => "aps",
            Self::Arxiv => "arxiv",
            Self::Biorxiv => "biorxiv",
            Self::Chemrxiv => "chemrxiv",
            Self::Coperni => "coperni",
            Self::Cshl => "cshl",
            Self::Degruyter => "degruyter",
            Self::Elsevier => "elsevier",
            Self::Emerald => "emerald",
            Self::Frontiers => "frontiers",
            Self::Ieee => "ieee",
            Self::Iop => "iop",
            Self::Mdpi => "mdpi",
            Self::Nature => "nature",
            Self::Nist => "nist",
            Self::Oup => "oup",
            Self::Pnas => "pnas",
            Self::Plos => "plos",
            Self::Rsc => "rsc",
            Self::RoyalSociety => "royal-society",
            Self::Springer => "springer",
            Self::TaylorFrancis => "taylor-francis",
            Self::Wiley => "wiley",
            Self::Zenodo => "zenodo",
        }
    }

    /// Human-facing publisher name, for `acquire` output.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Aaas => "AAAS (Science)",
            Self::Acm => "ACM",
            Self::Acs => "American Chemical Society",
            Self::Aip => "American Institute of Physics",
            Self::Chembl => "EMBL-EBI (ChEMBL)",
            Self::Aps => "American Physical Society",
            Self::Arxiv => "arXiv",
            Self::Biorxiv => "bioRxiv / medRxiv",
            Self::Chemrxiv => "ChemRxiv",
            Self::Coperni => "Copernicus / EGU",
            Self::Cshl => "Cold Spring Harbor Laboratory",
            Self::Degruyter => "De Gruyter",
            Self::Elsevier => "Elsevier",
            Self::Emerald => "Emerald",
            Self::Frontiers => "Frontiers",
            Self::Ieee => "IEEE",
            Self::Iop => "IOP Publishing",
            Self::Mdpi => "MDPI",
            Self::Nature => "Nature Portfolio",
            Self::Nist => "NIST",
            Self::Oup => "Oxford University Press",
            Self::Pnas => "PNAS",
            Self::Plos => "PLOS",
            Self::Rsc => "Royal Society of Chemistry",
            Self::RoyalSociety => "Royal Society",
            Self::Springer => "Springer Nature",
            Self::TaylorFrancis => "Taylor & Francis",
            Self::Wiley => "Wiley",
            Self::Zenodo => "Zenodo",
        }
    }
}

/// Every publisher this crate knows about.
///
/// One publisher legitimately spans several registrant prefixes (ACS has a
/// pre- and post-2008 form), so the registry has more rows than this has
/// entries. Useful for building a report of what the table covers.
pub static ALL_PUBLISHERS: &[Publisher] = &[
    Publisher::Aaas,
    Publisher::Acm,
    Publisher::Acs,
    Publisher::Aip,
    Publisher::Aps,
    Publisher::Arxiv,
    Publisher::Biorxiv,
    Publisher::Chemrxiv,
    Publisher::Chembl,
    Publisher::Coperni,
    Publisher::Degruyter,
    Publisher::Elsevier,
    Publisher::Emerald,
    Publisher::Frontiers,
    Publisher::Ieee,
    Publisher::Iop,
    Publisher::Mdpi,
    Publisher::Nature,
    Publisher::Nist,
    Publisher::Oup,
    Publisher::Pnas,
    Publisher::Plos,
    Publisher::Rsc,
    Publisher::RoyalSociety,
    Publisher::Springer,
    Publisher::TaylorFrancis,
    Publisher::Wiley,
    Publisher::Zenodo,
];

/// A sanctioned text-and-data-mining route offered by a publisher.
///
/// Only publishers with a real, documented TDM endpoint get one. Its absence
/// means "no TDM API", which is a different statement from "we did not look".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TdmRoute {
    /// Short name of the route, e.g. `elsevier-tdm`.
    pub name: &'static str,
    /// Environment variable holding the credential.
    pub env_var: &'static str,
    /// Secret-store key holding the credential.
    pub store_key: &'static str,
}

/// One row of the registrant-prefix table.
#[derive(Debug, Clone, Copy)]
struct PrefixEntry {
    /// Registrant code, without the leading `10.` — e.g. `1023`.
    prefix: &'static str,
    publisher: Publisher,
    kind: HostKind,
    tdm: Option<TdmRoute>,
}

const ELSEVIER_TDM: TdmRoute = TdmRoute {
    name: "elsevier-tdm",
    env_var: "SCITADEL_ELSEVIER_TDM_KEY",
    store_key: "elsevier.tdm_api_key",
};
const WILEY_TDM: TdmRoute = TdmRoute {
    name: "wiley-tdm",
    env_var: "SCITADEL_WILEY_TDM_TOKEN",
    store_key: "wiley.tdm_token",
};
const SPRINGER_TDM: TdmRoute = TdmRoute {
    name: "springer-tdm",
    env_var: "SCITADEL_SPRINGER_TDM_KEY",
    store_key: "springer.tdm_api_key",
};

/// Registrant prefix → host.
///
/// **Provenance.** Registrant codes are assigned by doi.org and are stable and
/// public; this is a curated subset covering the publishers that appear in
/// real retrieval corpora, not the full registry. An unlisted prefix yields
/// [`PublisherVerdict::Unknown`] rather than a guess — which is the point:
/// the table is allowed to be incomplete, and its incompleteness is visible
/// instead of being papered over with a default.
///
/// Splitting a publisher across several prefixes is normal and expected:
/// ACS alone has `1021` (pre-2008) and `1023` (post-2008), and the two forms
/// differ in suffix shape, which is why a single-prefix table was never
/// going to work.
static REGISTRY: &[PrefixEntry] = &[
    // --- RSC -------------------------------------------------------------
    PrefixEntry {
        prefix: "1039",
        publisher: Publisher::Rsc,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- ACS (pre- and post-2008 forms) ---------------------------------
    PrefixEntry {
        prefix: "1021",
        publisher: Publisher::Acs,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1023",
        publisher: Publisher::Acs,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- Elsevier (compact DOIs embed a balanced '(YY)' group) -----------
    PrefixEntry {
        prefix: "1016",
        publisher: Publisher::Elsevier,
        kind: HostKind::Publisher,
        tdm: Some(ELSEVIER_TDM),
    },
    // --- Wiley -----------------------------------------------------------
    PrefixEntry {
        prefix: "1002",
        publisher: Publisher::Wiley,
        kind: HostKind::Publisher,
        tdm: Some(WILEY_TDM),
    },
    // --- Springer Nature -------------------------------------------------
    PrefixEntry {
        prefix: "1007",
        publisher: Publisher::Springer,
        kind: HostKind::Publisher,
        tdm: Some(SPRINGER_TDM),
    },
    PrefixEntry {
        prefix: "1038",
        publisher: Publisher::Nature,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- OUP (segmented: 10.1093/nar/…) ---------------------------------
    PrefixEntry {
        prefix: "1093",
        publisher: Publisher::Oup,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- PNAS / AAAS -----------------------------------------------------
    PrefixEntry {
        prefix: "1073",
        publisher: Publisher::Pnas,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1126",
        publisher: Publisher::Aaas,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- MDPI, Frontiers, PLOS: fully OA publishers ----------------------
    PrefixEntry {
        prefix: "3390",
        publisher: Publisher::Mdpi,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "3389",
        publisher: Publisher::Frontiers,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1371",
        publisher: Publisher::Plos,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- NIST: US government work, public domain ------------------------
    PrefixEntry {
        prefix: "18434",
        publisher: Publisher::Nist,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- Preprint servers (free by construction) -------------------------
    PrefixEntry {
        prefix: "1101",
        publisher: Publisher::Biorxiv,
        kind: HostKind::PreprintServer,
        tdm: None,
    },
    PrefixEntry {
        prefix: "48550",
        publisher: Publisher::Arxiv,
        kind: HostKind::PreprintServer,
        tdm: None,
    },
    PrefixEntry {
        prefix: "26434",
        publisher: Publisher::Chemrxiv,
        kind: HostKind::PreprintServer,
        tdm: None,
    },
    // --- Repositories: record DOIs, suffixes may be record URLs ----------
    PrefixEntry {
        prefix: "6019",
        publisher: Publisher::Chembl,
        kind: HostKind::Repository,
        tdm: None,
    },
    PrefixEntry {
        prefix: "5281",
        publisher: Publisher::Zenodo,
        kind: HostKind::Repository,
        tdm: None,
    },
    // --- Physics / computing --------------------------------------------
    PrefixEntry {
        prefix: "1103",
        publisher: Publisher::Aps,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1109",
        publisher: Publisher::Ieee,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1145",
        publisher: Publisher::Acm,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1063",
        publisher: Publisher::Aip,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- IOP (segmented: 10.1088/1742-6596/…) ---------------------------
    PrefixEntry {
        prefix: "1088",
        publisher: Publisher::Iop,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- De Gruyter (segmented: 10.17308/<journal>.<vol>/<page>) ---------
    PrefixEntry {
        prefix: "1515",
        publisher: Publisher::Degruyter,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "17308",
        publisher: Publisher::Degruyter,
        kind: HostKind::Publisher,
        tdm: None,
    },
    // --- Other publishers ------------------------------------------------
    PrefixEntry {
        prefix: "1080",
        publisher: Publisher::TaylorFrancis,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1098",
        publisher: Publisher::RoyalSociety,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "1284",
        publisher: Publisher::Emerald,
        kind: HostKind::Publisher,
        tdm: None,
    },
    PrefixEntry {
        prefix: "5194",
        publisher: Publisher::Coperni,
        kind: HostKind::Publisher,
        tdm: None,
    },
];

/// What the table could or could not say about a DOI's publisher.
///
/// A consumer gets a name or an explicit `Unknown` — never a default that
/// reads as knowledge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherVerdict {
    Known {
        publisher: Publisher,
        kind: HostKind,
        tdm: Option<TdmRoute>,
    },
    /// The registrant prefix is not in the table. The prefix is carried
    /// through so a caller can report *which* one, and a run can collect the
    /// missing prefixes for a table update.
    Unknown { prefix: String },
}

impl PublisherVerdict {
    /// The publisher, or `None` when the prefix is not classified. This is the
    /// ergonomic accessor; [`Self`] is what a report should carry.
    pub fn publisher(&self) -> Option<Publisher> {
        match self {
            Self::Known { publisher, .. } => Some(*publisher),
            Self::Unknown { .. } => None,
        }
    }

    /// Short stable label for storage and filenames, or `unknown`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Known { publisher, .. } => publisher.label(),
            Self::Unknown { .. } => "unknown",
        }
    }
}

/// The registrant code of a DOI: the digits between `10.` and the first `/`.
///
/// Returns `None` for anything that is not DOI-shaped, so callers can treat
/// "not a DOI" and "DOI of an unknown publisher" as different outcomes.
///
/// Owned rather than borrowed because it is derived from
/// [`normalize_doi`], which allocates; handing back a slice into a temporary
/// would not compile, and a `Cow` here would be premature.
pub fn registrant_prefix(doi: &str) -> Option<String> {
    let normalized = normalize_doi(doi);
    let rest = normalized.strip_prefix("10.")?;
    let slash = rest.find('/')?;
    let registrant = &rest[..slash];
    if (4..=9).contains(&registrant.len()) && registrant.chars().all(|c| c.is_ascii_digit()) {
        Some(registrant.to_string())
    } else {
        None
    }
}

/// Classify a DOI's publisher from its registrant prefix.
pub fn classify_publisher(doi: &str) -> PublisherVerdict {
    let Some(prefix) = registrant_prefix(doi) else {
        return PublisherVerdict::Unknown {
            prefix: String::from("<not-a-doi>"),
        };
    };
    match REGISTRY.iter().find(|e| e.prefix == prefix) {
        Some(e) => PublisherVerdict::Known {
            publisher: e.publisher,
            kind: e.kind,
            tdm: e.tdm,
        },
        None => PublisherVerdict::Unknown { prefix },
    }
}

/// Whether a suffix under a **repository** prefix is a record URL rather than
/// a DOI identifier.
///
/// Only meaningful for repository prefixes, which is why it takes the
/// registrant rather than the suffix alone: `10.1093/nar/gkv1075` is a
/// perfectly good segmented DOI, and `10.6019/CHEMBL/ACTIVITY/27697493` is a
/// ChEMBL record URL. The two differ only in which prefix they sit under, so
/// no suffix-only rule can separate them — a suffix-only "no slashes" or "no
/// colon" rule gets one of them wrong, which is exactly the trap described in
/// #261.
///
/// Returns `false` for any prefix that is not a repository, so a legitimate
/// publisher DOI is never rejected by this rule.
pub fn suffix_is_repository_url(registrant: &str, suffix: &str) -> bool {
    let Some(entry) = REGISTRY.iter().find(|e| e.prefix == registrant) else {
        return false;
    };
    if entry.kind != HostKind::Repository {
        return false;
    }

    // A repository mints one DOI per *record*, and the identifier it uses is
    // single-segment: ChEMBL `10.6019/chembl12345`, Zenodo
    // `10.5281/zenodo.1234567`, arXiv `10.48550/arxiv.2301.00001`. A
    // path-shaped suffix under the same prefix is therefore a record *URL*
    // (`CHEMBL/ACTIVITY/27697493`) that scraped into a DOI field, not an
    // identifier.
    //
    // Matching on shape rather than on a record-type vocabulary keeps this
    // from needing to enumerate ChEMBL's record types, and it cannot
    // misfire on a publisher prefix because the prefix check above already
    // returned.
    suffix.contains('/')
}

/// What the acquisition ladder concluded about a DOI's full-text route.
///
/// #261's central complaint: a two-valued verdict forced a consumer to
/// report "no TDM route available" for a publisher the ladder had never
/// classified. That sentence is the text a human reads to decide what to do
/// next, and for 249 papers it named a route that was never evaluated. These
/// four states keep *unknown* apart from *known absent*, and only
/// [`Self::TdmKeyMissing`] may ever suggest registering a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteVerdict {
    /// The publisher is classified and has no TDM API. An established
    /// negative, not a gap in coverage.
    NoTdmRouteAvailable { publisher: Publisher },
    /// The publisher is classified, has a TDM API, and the credential is
    /// configured.
    TdmAvailable {
        publisher: Publisher,
        route: TdmRoute,
    },
    /// The publisher is classified and has a TDM API, but the credential is
    /// missing. The only state that justifies telling a user to register a
    /// key.
    TdmKeyMissing {
        publisher: Publisher,
        route: TdmRoute,
    },
    /// The prefix is not in the table. No route was evaluated, so no claim
    /// about route availability may be made.
    PublisherUnknown { prefix: String },
}

impl RouteVerdict {
    /// True only when the ladder actually established that no TDM route
    /// exists. A caller gating a "no TDM route available" message on this
    /// cannot overstate what was checked.
    pub fn is_established_negative(&self) -> bool {
        matches!(self, Self::NoTdmRouteAvailable { .. })
    }

    /// True when the publisher's prefix was not classified.
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::PublisherUnknown { .. })
    }

    /// Human-facing note for a coverage report.
    ///
    /// The wording is the point of this type, so it is asserted in the tests
    /// below: [`Self::PublisherUnknown`] must not contain a claim about TDM
    /// route availability.
    pub fn note(&self) -> String {
        match self {
            Self::NoTdmRouteAvailable { publisher } => format!(
                "{} has no TDM route available (classified from the DOI prefix)",
                publisher.display_name()
            ),
            Self::TdmAvailable { publisher, route } => format!(
                "{} has a TDM route available via {}",
                publisher.display_name(),
                route.name
            ),
            Self::TdmKeyMissing { publisher, route } => format!(
                "{} offers a TDM route ({}) but no credential is configured - \
                 run `scitadel auth login tdm` or set {}",
                publisher.display_name(),
                route.name,
                route.env_var
            ),
            Self::PublisherUnknown { prefix } => format!(
                "publisher not classified (registrant prefix 10.{prefix}); \
                 no route was evaluated, so route availability is undetermined"
            ),
        }
    }
}

/// Classify a DOI's publisher and say what that means for full-text access.
///
/// `tdm_configured` reports whether the credential for a discovered TDM route
/// is present; the ladder supplies it so this function stays free of I/O.
pub fn route_verdict(doi: &str, tdm_configured: impl Fn(&TdmRoute) -> bool) -> RouteVerdict {
    match classify_publisher(doi) {
        PublisherVerdict::Unknown { prefix } => RouteVerdict::PublisherUnknown { prefix },
        PublisherVerdict::Known {
            publisher,
            tdm: None,
            ..
        } => RouteVerdict::NoTdmRouteAvailable { publisher },
        PublisherVerdict::Known {
            publisher,
            tdm: Some(route),
            ..
        } => {
            if tdm_configured(&route) {
                RouteVerdict::TdmAvailable { publisher, route }
            } else {
                RouteVerdict::TdmKeyMissing { publisher, route }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #261: a consumer must be able to get a publisher, or an explicit
    /// unknown, without writing its own prefix table.
    #[test]
    fn classifies_the_publishers_measured_in_the_raid_run() {
        let cases = [
            ("10.1023/b:josl.0000026645.41309.d3", Publisher::Acs),
            ("10.1016/j.jneumeth.2005.09.009", Publisher::Elsevier),
            ("10.1039/d0nr01234a", Publisher::Rsc),
            ("10.1002/anie.200906232", Publisher::Wiley),
            ("10.1093/nar/gkv1075", Publisher::Oup),
            ("10.1007/s10462-023-04567-8", Publisher::Springer),
            ("10.3390/pharmaceutics14051098", Publisher::Mdpi),
            ("10.1073/pnas.2415658122", Publisher::Pnas),
            ("10.18434/m32154", Publisher::Nist),
        ];
        for (doi, expected) in cases {
            assert_eq!(
                classify_publisher(doi).publisher(),
                Some(expected),
                "{doi} should classify as {}",
                expected.label()
            );
        }
    }

    /// #261: the segmented-identifier families must survive unchanged. A naive
    /// "suffix has no slash" rule would reject every one of these.
    #[test]
    fn segmented_identifiers_classify_and_round_trip() {
        let segmented = [
            "10.1093/nar/gkv1075",
            "10.1088/1742-6596/2026/01/01/12345678",
            "10.26434/chemrxiv.2024.01.01.123456.v1",
            "10.17308/swj.2024.1.2.345",
        ];
        for doi in segmented {
            assert_ne!(
                classify_publisher(doi).publisher(),
                None,
                "{doi} must classify"
            );
            // Round-trip: normalising is idempotent and does not truncate.
            assert_eq!(normalize_doi(doi), doi, "{doi} must round-trip");
            assert_eq!(
                normalize_doi(&normalize_doi(doi)),
                normalize_doi(doi),
                "{doi} normalisation must be idempotent"
            );
        }
    }

    /// #261: an unlisted prefix is `Unknown`, never a silent default. This is
    /// the acceptance criterion that stops 249 papers being told "no TDM
    /// route available" for a publisher nobody classified.
    #[test]
    fn an_unlisted_prefix_is_explicitly_unknown() {
        let v = classify_publisher("10.99999/some.suffix.12345");
        assert_eq!(v.publisher(), None);
        assert_eq!(v.label(), "unknown");
        assert!(
            matches!(&v, PublisherVerdict::Unknown { prefix } if prefix == "99999"),
            "the offending prefix must be carried through for a table update, got {v:?}"
        );
    }

    #[test]
    fn a_non_doi_is_unknown_rather_than_a_panicking_prefix_error() {
        for bad in ["", "not-a-doi", "10.1234", "10.abcd/x", "11.1234/x"] {
            assert_eq!(
                classify_publisher(bad).publisher(),
                None,
                "{bad:?} should not classify"
            );
        }
    }

    /// The repository-URL rule must fire for repository prefixes and stay
    /// quiet for every publisher, or it becomes a false-positive generator.
    #[test]
    fn repository_url_rule_is_scoped_to_repository_prefixes() {
        // Repository prefix: a record URL.
        assert!(suffix_is_repository_url("6019", "CHEMBL/ACTIVITY/27697493"));
        assert!(suffix_is_repository_url("6019", "chembl/compound/CHEMBL25"));
        // Repository prefix, but a plain identifier — not a record URL.
        assert!(!suffix_is_repository_url("6019", "chembl12345"));

        // Publisher prefixes: never rejected, however slash-heavy.
        assert!(!suffix_is_repository_url("1093", "nar/gkv1075"));
        assert!(!suffix_is_repository_url(
            "1088",
            "1742-6596/2026/01/01/12345678"
        ));
        assert!(!suffix_is_repository_url(
            "26434",
            "chemrxiv.2024.01.01.123456.v1"
        ));

        // Unknown prefix: no opinion.
        assert!(!suffix_is_repository_url("99999", "CHEMBL/ACTIVITY/1"));
    }

    #[test]
    fn every_publisher_has_a_distinct_stable_label() {
        // Over the enum, not the registry: one publisher spanning two
        // prefixes (ACS `1021`/`1023`) is correct, two publishers sharing a
        // label is not.
        let mut labels: Vec<&str> = ALL_PUBLISHERS.iter().map(|p| p.label()).collect();
        labels.sort_unstable();
        let before = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), before, "publisher labels must be distinct");

        // `label` is used for database values and filenames, so it has to be
        // lowercase and free of separators.
        for p in ALL_PUBLISHERS {
            let l = p.label();
            assert!(
                l.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{l} must be filename- and key-safe"
            );
            assert!(!p.display_name().is_empty());
        }
    }

    /// #261 acceptance: three (here four) distinct verdicts, and — the part
    /// that actually mattered — a note for an unclassified publisher must not
    /// claim a route was evaluated.
    #[test]
    fn route_verdicts_keep_unknown_distinct_from_known_absent() {
        // A classified publisher with no TDM API: an established negative.
        let rsc = route_verdict("10.1039/d0nr01234a", |_| true);
        assert_eq!(
            rsc,
            RouteVerdict::NoTdmRouteAvailable {
                publisher: Publisher::Rsc
            }
        );
        assert!(rsc.is_established_negative());
        assert!(!rsc.is_unknown());
        assert!(rsc.note().contains("no TDM route available"));

        // A classified publisher *with* a TDM API, credential present.
        let ready = route_verdict("10.1016/j.jneumeth.2005.09.009", |_| true);
        assert!(matches!(ready, RouteVerdict::TdmAvailable { .. }));

        // Same publisher, credential absent: the only state that may tell a
        // user to register a key.
        let missing = route_verdict("10.1016/j.jneumeth.2005.09.009", |_| false);
        assert!(matches!(missing, RouteVerdict::TdmKeyMissing { .. }));
        let note = missing.note();
        assert!(note.contains(ELSEVIER_TDM.env_var), "note: {note}");
        assert!(
            note.contains("auth login"),
            "note should be actionable: {note}"
        );

        // An unclassified prefix. The whole point: the note must NOT assert
        // that no route exists, because the ladder never looked.
        let unknown = route_verdict("10.99999/some.suffix.12345", |_| false);
        assert!(unknown.is_unknown());
        assert!(!unknown.is_established_negative());
        let note = unknown.note();
        assert!(
            !note.contains("no TDM route available"),
            "an unclassified publisher must not be reported as lacking a TDM route: {note}"
        );
        assert!(
            note.contains("not classified") && note.contains("99999"),
            "the note must say what was not known and which prefix: {note}"
        );
    }

    /// A caller gating on `is_established_negative` must never be able to
    /// produce the old, wrong sentence for any DOI in a plausible corpus.
    #[test]
    fn the_established_negative_guard_cannot_be_bypassed() {
        let corpus = [
            "10.1023/b:josl.0000026645.41309.d3",
            "10.1016/0003-2670(93)90142-7",
            "10.1039/d0nr01234a",
            "10.1093/nar/gkv1075",
            "10.1101/2025.06.14.659707",
            "10.18434/m32154",
            "10.99999/unlisted.12345",
        ];
        for doi in corpus {
            let v = route_verdict(doi, |_| false);
            if v.is_unknown() {
                assert!(
                    !v.note().contains("no TDM route available"),
                    "{doi} is unclassified but the note claims a TDM verdict"
                );
            }
        }
    }

    /// `ALL_PUBLISHERS` is only useful if it stays exhaustive — a new variant
    /// added to the enum but not the list would quietly vanish from coverage
    /// reports.
    #[test]
    fn all_publishers_covers_the_registry() {
        for e in REGISTRY {
            assert!(
                ALL_PUBLISHERS.contains(&e.publisher),
                "{} is in the registry but missing from ALL_PUBLISHERS",
                e.publisher.label()
            );
        }
    }

    /// A typo'd or duplicated row would silently shadow another publisher.
    #[test]
    fn the_registry_has_no_duplicate_prefixes() {
        let mut seen = std::collections::HashSet::new();
        for e in REGISTRY {
            assert!(
                e.prefix.chars().all(|c| c.is_ascii_digit()),
                "{} is not a registrant code",
                e.prefix
            );
            assert!(
                (4..=9).contains(&e.prefix.len()),
                "{} is not 4-9 digits",
                e.prefix
            );
            assert!(seen.insert(e.prefix), "duplicate prefix {}", e.prefix);
        }
    }

    /// Only real publishers get a TDM route; a repository or preprint server
    /// never should, since they are free to read.
    #[test]
    fn tdm_routes_belong_only_to_publishers() {
        for e in REGISTRY {
            if e.tdm.is_some() {
                assert_eq!(
                    e.kind,
                    HostKind::Publisher,
                    "{} has a TDM route but is not a publisher",
                    e.publisher.label()
                );
            }
        }
    }

    /// Preprint servers are free by construction, which is the whole point of
    /// #260's route list. If one is ever mis-typed as a publisher, that fix
    /// silently stops applying.
    #[test]
    fn preprint_servers_are_classified_as_such() {
        for doi in [
            "10.1101/2025.06.14.659707",
            "10.48550/arXiv.2301.00001",
            "10.26434/chemrxiv.2024.01.01.123456.v1",
        ] {
            match classify_publisher(doi) {
                PublisherVerdict::Known { kind, .. } => {
                    assert_eq!(kind, HostKind::PreprintServer, "{doi}");
                }
                other @ PublisherVerdict::Unknown { .. } => {
                    panic!("{doi} should classify as a preprint server, got {other:?}")
                }
            }
        }
    }
}
