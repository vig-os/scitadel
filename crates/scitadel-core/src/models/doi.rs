/// Valid characters in a DOI suffix: alphanumeric plus `-._;()/:`.
/// Covers 99.3% of CrossRef DOIs per their regex analysis.
fn is_valid_suffix_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ';' | '(' | ')' | '/' | ':')
}

/// Normalize a DOI string: trim whitespace, strip common URL prefixes, lowercase.
pub fn normalize_doi(doi: &str) -> String {
    let trimmed = doi.trim();
    let stripped = trimmed
        .strip_prefix("https://doi.org/")
        .or_else(|| trimmed.strip_prefix("http://doi.org/"))
        .or_else(|| trimmed.strip_prefix("https://dx.doi.org/"))
        .or_else(|| trimmed.strip_prefix("http://dx.doi.org/"))
        .unwrap_or(trimmed);
    stripped.to_lowercase()
}

/// Validate a DOI string against the standard format: `10.NNNN…/suffix`.
///
/// - Prefix: `10.` followed by 4–9 digits (registrant code).
/// - Separator: `/`.
/// - Suffix: one or more valid characters (`[a-zA-Z0-9-._;()/:]+`).
///
/// The input is normalized (trimmed, URL-prefix-stripped) before validation.
pub fn validate_doi(doi: &str) -> bool {
    let normalized = normalize_doi(doi);

    // Must start with "10."
    let rest = match normalized.strip_prefix("10.") {
        Some(r) => r,
        None => return false,
    };

    // Extract registrant digits (4–9 digits before the first '/')
    let slash_pos = match rest.find('/') {
        Some(pos) => pos,
        None => return false,
    };

    let registrant = &rest[..slash_pos];
    if registrant.len() < 4 || registrant.len() > 9 {
        return false;
    }
    if !registrant.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    // Suffix must be non-empty and contain only valid characters
    let suffix = &rest[slash_pos + 1..];
    if suffix.is_empty() {
        return false;
    }

    suffix.chars().all(is_valid_suffix_char)
}

/// Sanitize a DOI for use as a filename: replace `/` with `_`, keep only safe chars.
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

    #[test]
    fn valid_dois() {
        assert!(validate_doi("10.1038/s41586-020-2649-2"));
        assert!(validate_doi("10.1371/journal.pone.0000000"));
        assert!(validate_doi("10.1002/anie.200906232"));
        assert!(validate_doi("10.1103/PhysRevLett.116.061102"));
        assert!(validate_doi("10.48550/arXiv.2301.00001"));
    }

    #[test]
    fn valid_with_url_prefix() {
        assert!(validate_doi("https://doi.org/10.1038/s41586-020-2649-2"));
        assert!(validate_doi("http://dx.doi.org/10.1002/anie.200906232"));
    }

    #[test]
    fn valid_with_special_suffix_chars() {
        assert!(validate_doi("10.1000/xyz_(abc)"));
        assert!(validate_doi("10.1000/a:b;c.d_e-f"));
        assert!(validate_doi("10.1234/sub/path/deep"));
    }

    #[test]
    fn invalid_dois() {
        assert!(!validate_doi(""));
        assert!(!validate_doi("not-a-doi"));
        assert!(!validate_doi("10.123/too-short-registrant"));
        assert!(!validate_doi("10.1234/")); // empty suffix
        assert!(!validate_doi("10.1234")); // no slash
        assert!(!validate_doi("11.1234/test")); // wrong prefix
        assert!(!validate_doi("10.12345678901/too-long-registrant")); // >9 digits
        assert!(!validate_doi("10.abcd/test")); // non-digit registrant
    }

    #[test]
    fn normalize_strips_prefix() {
        assert_eq!(
            normalize_doi("https://doi.org/10.1038/TEST"),
            "10.1038/test"
        );
        assert_eq!(
            normalize_doi("http://dx.doi.org/10.1038/TEST"),
            "10.1038/test"
        );
    }

    #[test]
    fn normalize_lowercases() {
        assert_eq!(normalize_doi("10.1038/ABC"), "10.1038/abc");
    }

    #[test]
    fn filename_sanitization() {
        assert_eq!(
            doi_to_filename("10.1038/s41586-020-2649-2"),
            "10.1038_s41586-020-2649-2"
        );
    }
}
