/// A DOI that was rejected, with the reason.
///
/// #262: every consumer of a DOI list re-implemented shape checking and each
/// one got a different subset wrong, silently — a rejected candidate either
/// became a corrupt key or was never reported. Returning a reason rather than
/// a bare `false` is what lets a caller say *why* a candidate was dropped, so
/// a human can overturn the judgement.
///
/// The variants are ordered roughly from "obviously not a DOI" to "looks like
/// one but is a truncation", because the latter is the expensive mistake: it
/// produces a key that resolves to nothing and reaches a fetch queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoiRejection {
    /// Empty or whitespace only.
    Empty,
    /// Does not start with the `10.` prefix — not a DOI at all.
    NotDoi,
    /// The registrant code is not 4–9 digits (e.g. `10.123/x`, `10.abcdef/x`).
    MalformedRegistrant(String),
    /// No `/` separating registrant from suffix.
    NoSlash,
    /// Nothing after the `/`.
    EmptySuffix,
    /// The suffix contains a character that cannot appear in a DOI.
    InvalidSuffixCharacter(char),
    /// Brackets or parentheses are unbalanced — almost always a DOI that was
    /// truncated mid-token rather than a malformed one.
    ///
    /// Elsevier's compact DOIs embed a balanced year group
    /// (`10.1016/0003-2670(93)90142-7`), so a parser that treats the closing
    /// bracket as punctuation cuts them in half. The truncated half still
    /// starts with `10.` and still passes a prefix check, which is why this
    /// needs its own case: rejecting it is the whole point of validating.
    UnbalancedBracket,
    /// The suffix is an ellipsis or truncation placeholder (`...`, `…`, `[…]`).
    EllipsisPlaceholder,
    /// A syntactically valid DOI belonging to a data repository rather than a
    /// publisher, carrying a *record URL* in the suffix instead of an
    /// identifier — e.g. a ChEMBL activity URL,
    /// `10.6019/CHEMBL/ACTIVITY/27697493`.
    ///
    /// This one is only detectable with the prefix→publisher table in
    /// [`crate::publisher`]: the shape is indistinguishable from a legitimate
    /// segmented identifier (`10.1093/nar/gkv1075`) on its own.
    RepositoryUrlNotDoi,
}

impl std::fmt::Display for DoiRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty"),
            Self::NotDoi => write!(f, "does not start with the DOI prefix '10.'"),
            Self::MalformedRegistrant(r) => write!(
                f,
                "registrant code {r:?} is not 4-9 digits (a truncated or non-DOI token)"
            ),
            Self::NoSlash => write!(f, "no '/' between registrant code and suffix"),
            Self::EmptySuffix => write!(f, "nothing after the '/'"),
            Self::InvalidSuffixCharacter(c) => {
                write!(f, "suffix contains {c:?}, which cannot appear in a DOI")
            }
            Self::UnbalancedBracket => write!(
                f,
                "unbalanced bracket — the DOI was most likely truncated mid-token \
                 (Elsevier compact DOIs embed a balanced '(YY)' group, so cutting \
                 at the bracket leaves a half that still starts with '10.')"
            ),
            Self::EllipsisPlaceholder => {
                write!(f, "suffix is an ellipsis placeholder, not an identifier")
            }
            Self::RepositoryUrlNotDoi => write!(
                f,
                "suffix is a repository record URL rather than a DOI (this prefix is a \
                 data repository, not a publisher)"
            ),
        }
    }
}

impl std::error::Error for DoiRejection {}

/// Valid characters in a DOI suffix: alphanumeric plus `-._;()/:`.
///
/// Covers 99.3% of CrossRef DOIs per their regex analysis.
fn is_valid_suffix_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ';' | '(' | ')' | '/' | ':')
}

/// Normalize a DOI string: trim whitespace, strip common URL prefixes, lowercase.
///
/// Handles both the resolver-URL form (`https://doi.org/…`,
/// `http://dx.doi.org/…`) and the CURIE form (`doi:…`) that `.bib` files
/// carry. Both used to be handled by two different functions in two crates,
/// with non-overlapping prefix sets, so a DOI keyed off one would not match
/// the same DOI keyed off the other. There is one normalizer now (#261).
pub fn normalize_doi(doi: &str) -> String {
    let trimmed = doi.trim();
    let stripped = trimmed
        .strip_prefix("https://doi.org/")
        .or_else(|| trimmed.strip_prefix("http://doi.org/"))
        .or_else(|| trimmed.strip_prefix("https://dx.doi.org/"))
        .or_else(|| trimmed.strip_prefix("http://dx.doi.org/"))
        .or_else(|| trimmed.strip_prefix("https://www.doi.org/"))
        .or_else(|| trimmed.strip_prefix("http://www.doi.org/"))
        .or_else(|| trimmed.strip_prefix("doi:"))
        .or_else(|| trimmed.strip_prefix("DOI:"))
        .unwrap_or(trimmed);
    stripped.to_lowercase()
}

/// Validate a DOI and, on failure, say why (#262).
///
/// This is the canonical shape check. It is deliberately *permissive about
/// suffix structure*, because the real identifier families disagree with each
/// other and a naive rule rejects valid DOIs:
///
/// - **Segmented identifiers** put a `/` inside the suffix — OUP
///   `10.1093/nar/gkv1075`, IOP `10.1088/1742-6596/…`, ChemRxiv
///   `10.26434/chemrxiv.…/v1`, De Gruyter `10.17308/<journal>.<vol>/<page>`.
///   So there is deliberately **no** "suffix contains no slash" rule.
/// - **Balanced parentheses** are legitimate and load-bearing: Elsevier's
///   compact form is `10.1016/0003-2670(93)90142-7`.
/// - **A colon in the suffix** is legitimate: ACS's pre-2008 form is
///   `10.1023/a:1006721613253` and `10.1023/b:josl.0000026645.41309.d3`. Any
///   consumer that truncates a token at the first colon destroys real DOIs.
///
/// What it *does* reject is the malformities that are unambiguous: unbalanced
/// brackets, ellipsis placeholders, illegal characters, and — with the
/// prefix table's help — repository record URLs masquerading as DOIs.
///
/// Note the input is normalized first, so callers that would otherwise
/// re-derive a key from a raw string should pass the raw string and use the
/// returned form.
pub fn validate_doi_detailed(doi: &str) -> Result<String, DoiRejection> {
    let normalized = normalize_doi(doi);

    if normalized.is_empty() {
        return Err(DoiRejection::Empty);
    }

    // Must start with "10." — the DOI prefix.
    let rest = normalized.strip_prefix("10.").ok_or(DoiRejection::NotDoi)?;

    // Registrant digits (4–9 digits before the first '/').
    let slash_pos = rest.find('/').ok_or(DoiRejection::NoSlash)?;
    let registrant = &rest[..slash_pos];

    if !(4..=9).contains(&registrant.len()) || !registrant.chars().all(|c| c.is_ascii_digit()) {
        return Err(DoiRejection::MalformedRegistrant(registrant.to_string()));
    }

    let suffix = &rest[slash_pos + 1..];
    if suffix.is_empty() {
        return Err(DoiRejection::EmptySuffix);
    }

    // Placeholders first: an ellipsis is not merely an invalid character,
    // it is a specific human error worth naming.
    if is_ellipsis(suffix) {
        return Err(DoiRejection::EllipsisPlaceholder);
    }

    if let Some(c) = suffix.chars().find(|&c| !is_valid_suffix_char(c)) {
        return Err(DoiRejection::InvalidSuffixCharacter(c));
    }

    // Unbalanced brackets: the truncation trap. Checked before anything
    // prefix-specific so the reason names the real problem.
    if has_unbalanced_bracket(suffix) {
        return Err(DoiRejection::UnbalancedBracket);
    }

    // A repository record URL is syntactically a valid segmented identifier,
    // so only the prefix table can tell it apart from a real one.
    if crate::publisher::suffix_is_repository_url(registrant, suffix) {
        return Err(DoiRejection::RepositoryUrlNotDoi);
    }

    Ok(normalized)
}

/// Do the suffix's brackets balance?
///
/// Both bracket flavours are checked because a truncation can land inside
/// either, and an unbalanced *pair* of different kinds (`(]`) is malformed
/// either way.
fn has_unbalanced_bracket(suffix: &str) -> bool {
    let mut parens = 0i32;
    let mut brackets = 0i32;
    for c in suffix.chars() {
        match c {
            '(' => parens += 1,
            ')' => parens -= 1,
            '[' => brackets += 1,
            ']' => brackets -= 1,
            _ => {}
        }
        if parens < 0 || brackets < 0 {
            // Closing before opening — unbalanced.
            return true;
        }
    }
    parens != 0 || brackets != 0
}

/// Is the suffix an ellipsis / elision placeholder rather than an identifier?
fn is_ellipsis(suffix: &str) -> bool {
    let stripped: String = suffix
        .chars()
        .filter(|c| !matches!(c, '.' | '…' | '-'))
        .collect();
    // `...`, `…`, `-----` and friends carry no identifier characters.
    stripped.is_empty()
}

/// Validate a DOI string against the standard format: `10.NNNN…/suffix`.
///
/// Boolean wrapper over [`validate_doi_detailed`] for the many call sites that
/// only need the verdict. New code that has to report *why* a candidate was
/// dropped should use the detailed form.
pub fn validate_doi(doi: &str) -> bool {
    validate_doi_detailed(doi).is_ok()
}

/// Sanitize a DOI for use as a filename: replace `/` with `_`, keep only safe chars.
///
/// Not a security boundary — the result is a filename component, never a path,
/// and cannot contain a separator. Use [`validate_doi_detailed`] first if the
/// input is untrusted.
pub fn doi_to_filename(doi: &str) -> String {
    let normalized = normalize_doi(doi);
    normalized
        .chars()
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

    /// #262: the real forms the validator has to accept. Each of these is a
    /// family that a plausible-looking naive rule breaks.
    #[test]
    fn accepts_the_real_identifier_families() {
        let accepted = [
            // Elsevier compact: balanced (YY) group.
            "10.1016/0003-2670(93)90142-7",
            "10.1016/0022-1902(65)80271-8",
            "10.1016/s0960-894x(03)00027-3",
            // ACS pre-2008: colon inside the suffix.
            "10.1023/a:1006721613253",
            "10.1023/b:josl.0000026645.41309.d3",
            // Segmented identifiers: a slash inside the suffix.
            "10.1093/nar/gkv1075",
            "10.1093/nar/26mcq123",
            "10.1088/1742-6596/2026/01/01/12345678",
            "10.26434/chemrxiv.2024.01.01.123456.v1",
            "10.17308/swj.2024.1.2.345",
            // Plain forms, for the baseline.
            "10.1038/s41586-020-2649-2",
            "10.1101/2025.06.14.659707",
            "10.1371/journal.pone.0000000",
            "10.1000/xyz_(abc)",
            "10.1234/sub/path/deep",
        ];
        for doi in accepted {
            assert!(
                validate_doi_detailed(doi).is_ok(),
                "{doi} should be accepted, got {:?}",
                validate_doi_detailed(doi).unwrap_err()
            );
        }
    }

    /// #262: the nine truncated Elsevier DOIs observed reaching a fetch queue
    /// in the wild, plus the RSC identifier concatenated with a date. Every
    /// one must be rejected, and the reason must name the unbalanced bracket.
    #[test]
    fn rejects_the_truncated_dois_from_the_raid_run() {
        let truncated = [
            "10.1016/0003-2670(93",
            "10.1016/0016-7037(84",
            "10.1016/s0969-8051(97",
            "10.1016/0003-2670(77",
            "10.1016/0022-1902(69",
            "10.1016/s0016-7037(97",
            "10.1006/2952(73",
            "10.1016/0022-1902(65",
            "10.1016/s0960-894x(03",
            "10.1016/0016-7037(70",
        ];
        for doi in truncated {
            assert_eq!(
                validate_doi_detailed(doi),
                Err(DoiRejection::UnbalancedBracket),
                "{doi} is a half-parenthesised Elsevier DOI and must be rejected as such"
            );
        }

        // The mirror-image bug: a valid DOI truncated at a real boundary is
        // still a shape-valid half in some cases, which is why callers need
        // the reason rather than a boolean.
        assert!(validate_doi_detailed("10.1039/qr9581200265").is_ok());
    }

    /// A closing bracket with no opener is the same class of damage.
    #[test]
    fn rejects_a_stray_closing_bracket() {
        assert_eq!(
            validate_doi_detailed("10.1016/0003-2670)90142-7"),
            Err(DoiRejection::UnbalancedBracket)
        );
    }

    /// #262: 718 shipped records stored a ChEMBL *activity URL* in a DOI
    /// field. It scrapes into the `10.` prefix and passes a naive check.
    #[test]
    fn rejects_a_repository_record_url() {
        assert_eq!(
            validate_doi_detailed("10.6019/CHEMBL/ACTIVITY/27697493"),
            Err(DoiRejection::RepositoryUrlNotDoi),
            "the exact string from the shipped records; normalisation lowercases it"
        );
        assert_eq!(
            validate_doi_detailed("10.6019/chembl/compound/CHEMBL25"),
            Err(DoiRejection::RepositoryUrlNotDoi),
            "the same trap in another record type"
        );
        // A genuine repository identifier is single-segment and must survive.
        assert!(
            validate_doi_detailed("10.6019/chembl12345").is_ok(),
            "a real ChEMBL compound DOI must not be caught by the record-URL rule"
        );
    }

    #[test]
    fn rejects_elision_placeholders() {
        for bad in ["10.1234/...", "10.1234/…", "10.1234/---", "10.1234/.."] {
            assert_eq!(
                validate_doi_detailed(bad),
                Err(DoiRejection::EllipsisPlaceholder),
                "{bad} is a placeholder"
            );
        }
    }

    #[test]
    fn reports_a_reason_for_every_rejection_class() {
        let cases: [(&str, DoiRejection); 6] = [
            ("", DoiRejection::Empty),
            ("   ", DoiRejection::Empty),
            ("not-a-doi", DoiRejection::NotDoi),
            ("10.123/x", DoiRejection::MalformedRegistrant("123".into())),
            ("10.1234", DoiRejection::NoSlash),
            ("10.1234/", DoiRejection::EmptySuffix),
        ];
        for (input, expected) in cases {
            assert_eq!(validate_doi_detailed(input), Err(expected.clone()));
            // The reason must be human-readable, since a caller reports it
            // so a human can overturn the judgement.
            assert!(
                !expected.to_string().is_empty(),
                "{expected:?} needs a message"
            );
        }

        assert_eq!(
            validate_doi_detailed("10.1234/has space"),
            Err(DoiRejection::InvalidSuffixCharacter(' '))
        );
        assert_eq!(
            validate_doi_detailed("10.12345678901/too-long"),
            Err(DoiRejection::MalformedRegistrant("12345678901".into()))
        );
        assert_eq!(
            validate_doi_detailed("10.abcd/test"),
            Err(DoiRejection::MalformedRegistrant("abcd".into()))
        );
    }

    /// #261: the two normalizers used to live in two crates with
    /// non-overlapping prefix sets, so the same DOI keyed off a resolver URL
    /// and the same DOI keyed off a `doi:` CURIE did not converge.
    #[test]
    fn normalize_handles_every_prefix_form() {
        for raw in [
            "https://doi.org/10.1038/TEST",
            "http://doi.org/10.1038/TEST",
            "https://dx.doi.org/10.1038/TEST",
            "http://dx.doi.org/10.1038/TEST",
            "https://www.doi.org/10.1038/TEST",
            "doi:10.1038/TEST",
            "DOI:10.1038/TEST",
            "  10.1038/TEST  ",
            "10.1038/TEST",
        ] {
            assert_eq!(normalize_doi(raw), "10.1038/test", "failed on {raw:?}");
        }
    }

    /// The normalized, canonical form comes back from the detailed validator,
    /// so callers cannot accidentally key on the raw string.
    #[test]
    fn detailed_validation_returns_the_canonical_form() {
        assert_eq!(
            validate_doi_detailed("https://doi.org/10.1038/S41586-020-2649-2").unwrap(),
            "10.1038/s41586-020-2649-2"
        );
    }

    /// The boolean wrapper must not drift from the detailed form.
    #[test]
    fn boolean_wrapper_agrees_with_detailed_validation() {
        for doi in [
            "10.1038/s41586-020-2649-2",
            "10.1016/0003-2670(93",
            "10.1023/b:josl.0000026645.41309.d3",
            "not-a-doi",
            "",
            "10.6019/CHEMBL/ACTIVITY/27697493",
        ] {
            assert_eq!(
                validate_doi(doi),
                validate_doi_detailed(doi).is_ok(),
                "wrapper disagreed for {doi:?}"
            );
        }
    }

    #[test]
    fn filename_sanitization_cannot_produce_a_path() {
        assert_eq!(
            doi_to_filename("10.1038/s41586-020-2649-2"),
            "10.1038_s41586-020-2649-2"
        );
        assert_eq!(
            doi_to_filename("10.1093/nar/gkv1075"),
            "10.1093_nar_gkv1075"
        );
        // A traversal attempt cannot survive as a separator.
        assert!(!doi_to_filename("../../etc/passwd").contains('/'));
    }
}
