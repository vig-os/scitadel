//! The DOI set #260's publisher clause is measured against, and where each DOI
//! came from.
//!
//! Ungated on purpose. The live requests live in `tests/contract_oa_set.rs`
//! behind `contract-tests`, but the **table** does not: a feature-gated table
//! can be emptied without the default suite noticing, and an empty table
//! makes a live suite that reports "100% obtained" over nothing. Keeping the
//! table here means `cargo test --workspace` asserts it is non-empty, every
//! DOI validates, and every entry carries a provenance line — with no network.
//!
//! # Why provenance is a field and not a comment
//!
//! #260 names seven DOIs and describes a set of 52 without listing it. A
//! measurement over "the 52" is therefore not reproducible from the issue
//! alone, and the seven that *are* named are all a curated sample rather than a
//! random one. Every DOI below carries where it came from so that a reader can
//! tell a DOI #260 asserted from one a live query produced, and can re-run the
//! query that produced it. A DOI with no provenance would be worse than no
//! DOI: it would look measured.

/// One DOI in the measurement set, and the line that justifies its presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OaProbe {
    /// The DOI, as the resolver will be handed it.
    pub doi: &'static str,
    /// Where this DOI came from: an #260 issue line, or a recorded query.
    pub provenance: Provenance,
    /// What the acceptance criterion this DOI speaks to.
    pub criterion: Criterion,
}

/// How a DOI entered the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Named verbatim in #260's issue body.
    Issue260,
    /// Read out of a live response for a publisher prefix #260 names. The
    /// query is recorded in [`OaProbe::provenance`]'s `Display` so the row is
    /// reproducible rather than asserted.
    LiveQuery,
}

/// Which clause of #260's acceptance a DOI is evidence for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criterion {
    /// "Given a bioRxiv/medRxiv/arXiv/ChemRxiv DOI, `acquire` obtains the PDF
    /// with no network index lookup, or says which of those servers it tried."
    PreprintRoute,
    /// "The 52-DOI OA-publisher set above re-runs with ≥90% obtained, or each
    /// residual is a genuinely paywalled paper."
    OaPublisherSet,
}

/// The measured set: #260's seven named DOIs, plus DOIs read out of live
/// queries against the publisher prefixes #260 names.
///
/// The extras exist because seven DOIs cannot support a percentage, and
/// because #260's own examples are a *curated* sample — the four publisher
/// DOIs it names are each a flagship case, which is the opposite of a random
/// draw. The extras widen each prefix to several works so the number means
/// something.
///
/// **The extras are not a substitute for the 52.** #260 never enumerates them,
/// so no run of this table can settle the clause as written; see the report's
/// "what this cannot measure".
pub const OA_260_PROBES: &[OaProbe] = &[
    // ---- The seven #260 names verbatim ----
    OaProbe {
        doi: "10.1101/2025.06.14.659707",
        provenance: Provenance::Issue260,
        criterion: Criterion::PreprintRoute,
    },
    OaProbe {
        doi: "10.1101/2024.11.19.624167",
        provenance: Provenance::Issue260,
        criterion: Criterion::PreprintRoute,
    },
    OaProbe {
        doi: "10.1101/2024.10.10.615955",
        provenance: Provenance::Issue260,
        criterion: Criterion::PreprintRoute,
    },
    OaProbe {
        doi: "10.3390/pharmaceutics14051098",
        provenance: Provenance::Issue260,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.18434/m32154",
        provenance: Provenance::Issue260,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.1073/pnas.2415658122",
        provenance: Provenance::Issue260,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.1093/nar/gkv1075",
        provenance: Provenance::Issue260,
        criterion: Criterion::OaPublisherSet,
    },
    // ---- Live queries, one per publisher prefix #260 names ----
    //
    // `https://api.crossref.org/works?filter=prefix:<p>,has-license:true&rows=3&select=DOI,license,publisher`,
    // 2026-10-07. `has-license:true` rather than a CC-BY filter because
    // Crossref's `/works` route rejects `filter=license-url` outright (it
    // exists only on `/prefixes/<p>/works`), and the point of these rows is
    // publisher coverage rather than licence filtering.
    OaProbe {
        doi: "10.3390/molecules24152793",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.1073/pnas.2310916120",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.1073/pnas.2424991122",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    // The two `10.1093` rows Crossref returned carry OUP's CHORUS terms rather
    // than a Creative Commons grant, so they are evidence about the *residual*
    // half of the clause — whether a non-CC route at a named OA publisher is
    // genuinely restricted — and not about the CC half.
    OaProbe {
        doi: "10.1093/clinchem/39.3.467",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.1093/neuonc/noz175.233",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    // RSC's Crossref licence is `rsc.li/journals-terms-of-use` for all three
    // rows — not an open grant. Kept precisely so the RSC column in the report
    // is a measured residual rather than an assumption that RSC is paywalled.
    OaProbe {
        doi: "10.1039/d5qo01181g",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.1039/d5cc03932k",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    // `https://api.datacite.org/dois?prefix=10.18434&page[size]=3`, 2026-10-07.
    // DataCite rather than Crossref because a `10.18434` DOI is registered
    // there and, for most of the prefix, nowhere else — which is the shape of
    // residual this table exists to find.
    OaProbe {
        doi: "10.18434/mds2-2400",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
    OaProbe {
        doi: "10.18434/mds2-3129",
        provenance: Provenance::LiveQuery,
        criterion: Criterion::OaPublisherSet,
    },
];

/// The query that produced every [`Provenance::LiveQuery`] row, for the report.
pub const LIVE_QUERY_PROVENANCE: &str = concat!(
    "Crossref: GET https://api.crossref.org/works",
    "?filter=prefix:<10.3390|10.1073|10.1093|10.1039>,has-license:true",
    "&rows=3&select=DOI,license,publisher  (2026-10-07); ",
    "DataCite: GET https://api.datacite.org/dois?prefix=10.18434&page[size]=3 (2026-10-07)"
);

/// The probes #260 names itself, which the extras must never displace.
pub const ISSUE_260_DOIS: [&str; 7] = [
    "10.1101/2025.06.14.659707",
    "10.1101/2024.11.19.624167",
    "10.1101/2024.10.10.615955",
    "10.3390/pharmaceutics14051098",
    "10.18434/m32154",
    "10.1073/pnas.2415658122",
    "10.1093/nar/gkv1075",
];

/// The probes addressing the preprint clause rather than the publisher clause.
pub fn preprint_probes() -> impl Iterator<Item = &'static OaProbe> {
    OA_260_PROBES
        .iter()
        .filter(|probe| probe.criterion == Criterion::PreprintRoute)
}

/// The probes addressing the publisher clause.
pub fn publisher_probes() -> impl Iterator<Item = &'static OaProbe> {
    OA_260_PROBES
        .iter()
        .filter(|probe| probe.criterion == Criterion::OaPublisherSet)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use scitadel_core::models::validate_doi_detailed;

    use super::*;

    /// An empty table would make every downstream percentage a division of
    /// zero, and — worse — would make a live suite that reports "100% obtained"
    /// look like a pass. Asserted here, ungated, so the failure cannot require
    /// the very network access the gate exists to protect.
    #[test]
    fn the_probe_table_is_not_empty() {
        assert!(
            OA_260_PROBES.len() >= 16,
            "the measurement set shrank to {} rows; #260 names seven and the \
             live queries contributed nine",
            OA_260_PROBES.len()
        );
    }

    /// A DOI the resolver will refuse is a row that measures nothing, and it
    /// fails *silently*: `resolve` treats an invalid DOI as "no DOI" and the
    /// live row then reports a registry-clean result for a work nobody asked
    /// about.
    #[test]
    fn every_probe_doi_is_one_the_resolver_accepts() {
        for probe in OA_260_PROBES {
            assert!(
                validate_doi_detailed(probe.doi).is_ok(),
                "{} does not validate, so resolve() would treat the work as \
                 DOI-less and the live row would measure nothing",
                probe.doi
            );
        }
    }

    /// Duplicates would inflate the denominator and make a set look better
    /// covered than it is.
    #[test]
    fn the_probe_table_has_no_duplicates() {
        let mut seen = HashSet::new();
        for probe in OA_260_PROBES {
            assert!(seen.insert(probe.doi), "{} appears twice", probe.doi);
        }
        assert_eq!(seen.len(), OA_260_PROBES.len());
    }

    /// The seven #260 names are the only DOIs in the table with an issue-line
    /// provenance, and every one of them must still be here. A table that grew
    /// by query sampling alone would quietly stop being a measurement *of
    /// #260*, which is the failure this whole exercise exists to avoid.
    #[test]
    fn every_doi_named_by_issue_260_is_present_and_marked_as_such() {
        for doi in ISSUE_260_DOIS {
            let probe = OA_260_PROBES
                .iter()
                .find(|probe| probe.doi == doi)
                .unwrap_or_else(|| panic!("{doi} is named in #260 but absent from the table"));
            assert_eq!(
                probe.provenance,
                Provenance::Issue260,
                "{doi} is in ISSUE_260_DOIS, so its provenance must be the issue"
            );
        }
        let marked = OA_260_PROBES
            .iter()
            .filter(|probe| probe.provenance == Provenance::Issue260)
            .count();
        assert_eq!(
            marked,
            ISSUE_260_DOIS.len(),
            "a row claims issue provenance that ISSUE_260_DOIS does not list"
        );
    }

    /// Both clauses need rows, or the harness would report on one and the
    /// report would read as if it had settled both.
    #[test]
    fn both_acceptance_clauses_have_probes() {
        assert_eq!(preprint_probes().count(), 3, "the three bioRxiv DOIs");
        assert!(
            publisher_probes().count() >= 13,
            "the four named publisher DOIs plus the query-sampled ones"
        );
    }

    /// **The gate is wired to a real test body.**
    ///
    /// #268's acceptance is that the feature-gated suite runs strictly more
    /// tests than the default one, and its own diagnosis of the rot is that
    /// nobody could tell. A count assertion cannot see across two `cargo test`
    /// invocations, so this reads the gated file's source instead: if the file
    /// is emptied, or its `#![cfg(feature = "contract-tests")]` gate is removed,
    /// or the network is reachable from the default suite, this fails here —
    /// ungated, hermetic, and on every `cargo test --workspace`.
    #[test]
    fn the_contract_gate_covers_a_real_live_test() {
        let gated = include_str!("../tests/contract_oa_set.rs");
        assert!(
            gated.starts_with("#![cfg(feature = \"contract-tests\")]"),
            "tests/contract_oa_set.rs must be gated at the crate root, or its \
             live requests run in the default suite"
        );
        let live_tests = gated.matches("#[tokio::test]").count();
        assert!(
            live_tests >= 2,
            "the gated file declares {live_tests} live tests; the gate must \
             cover real work, and an emptied gate is what #268 is about"
        );
        assert!(
            gated.contains("OA_260_PROBES"),
            "the gated file must drive off the ungated probe table rather than \
             keeping its own list, or the two can disagree"
        );
    }
}
