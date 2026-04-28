//! Kebab-slug helper for question-derived filenames.
//!
//! Used by the TUI export keybind (#135 sub-feature B) to pre-fill the
//! path prompt with a human-readable default like
//! `what-is-the-role-of-attention.bib`. Lives in `scitadel-export` so
//! the CLI can adopt the same default in a future iteration without a
//! second copy of the rules.
//!
//! Rules (locked here so behavior is testable):
//! 1. Map common Latin diacritics down to their ASCII base (`Ü` → `u`).
//! 2. Lowercase the result.
//! 3. Replace any non-`[a-z0-9]` rune with `-`.
//! 4. Collapse repeated `-` runs into a single `-`.
//! 5. Trim leading and trailing `-`.
//! 6. Cap at [`MAX_SLUG_LEN`] characters; trim trailing `-` again after
//!    the cap so we never emit `foo-bar-`.
//! 7. If the result is empty (e.g. all punctuation, all CJK), return
//!    [`FALLBACK_SLUG`].

/// Maximum slug length. 60 leaves headroom under POSIX 255-char filename
/// limits even after the user appends a long parent path + an extension.
pub const MAX_SLUG_LEN: usize = 60;

/// Slug returned when the input has no ASCII-alphanumeric characters.
/// `paper.bib` is the same default `bib snapshot` falls back to when
/// `--output` is omitted, so the two surfaces stay aligned.
pub const FALLBACK_SLUG: &str = "untitled";

/// Slugify a question (or any title) into a kebab-cased ASCII filename
/// stem. See module docs for the locked rules.
#[must_use]
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_dash = false;
    for ch in input.chars() {
        let mapped = ascii_fold_lower(ch);
        for c in mapped.chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c);
                last_was_dash = false;
            } else if !last_was_dash {
                out.push('-');
                last_was_dash = true;
            }
        }
    }

    // Trim trailing `-` (leading was prevented by `last_was_dash` start).
    while out.starts_with('-') {
        out.remove(0);
    }
    while out.ends_with('-') {
        out.pop();
    }

    if out.len() > MAX_SLUG_LEN {
        out.truncate(MAX_SLUG_LEN);
        while out.ends_with('-') {
            out.pop();
        }
    }

    if out.is_empty() {
        FALLBACK_SLUG.to_string()
    } else {
        out
    }
}

/// Map a single char to its ASCII-lower form when the rune has an
/// obvious Latin equivalent; otherwise lowercase whatever ASCII it
/// already is. Non-ASCII non-mapped runes pass through and will be
/// replaced with `-` by the caller.
fn ascii_fold_lower(ch: char) -> String {
    match ch {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => {
            "a".into()
        }
        'Æ' | 'æ' => "ae".into(),
        'Ç' | 'ç' => "c".into(),
        'È' | 'É' | 'Ê' | 'Ë' | 'è' | 'é' | 'ê' | 'ë' => "e".into(),
        'Ì' | 'Í' | 'Î' | 'Ï' | 'ì' | 'í' | 'î' | 'ï' => "i".into(),
        'Ñ' | 'ñ' => "n".into(),
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => {
            "o".into()
        }
        'Œ' | 'œ' => "oe".into(),
        'ß' => "ss".into(),
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'ù' | 'ú' | 'û' | 'ü' => "u".into(),
        'Ý' | 'ý' | 'ÿ' => "y".into(),
        c if c.is_ascii() => c.to_ascii_lowercase().to_string(),
        // Non-ASCII rune we don't know — caller will dash-fold it.
        c => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_question_slugifies_lowercase_kebab() {
        assert_eq!(
            slugify("What is the role of attention?"),
            "what-is-the-role-of-attention"
        );
    }

    #[test]
    fn punctuation_is_dashed_and_collapsed() {
        // Ampersand + comma + multi-space all collapse to a single `-`.
        assert_eq!(
            slugify("Learning &  reasoning, in LLMs!"),
            "learning-reasoning-in-llms"
        );
    }

    #[test]
    fn diacritics_fold_to_ascii() {
        assert_eq!(slugify("Über résumé"), "uber-resume");
        assert_eq!(slugify("Crème brûlée"), "creme-brulee");
        assert_eq!(slugify("naïve façade"), "naive-facade");
    }

    #[test]
    fn all_non_alphanumeric_returns_fallback() {
        assert_eq!(slugify("???"), FALLBACK_SLUG);
        assert_eq!(slugify("   "), FALLBACK_SLUG);
        assert_eq!(slugify("--- ---"), FALLBACK_SLUG);
    }

    #[test]
    fn empty_string_returns_fallback() {
        assert_eq!(slugify(""), FALLBACK_SLUG);
    }

    #[test]
    fn length_is_capped_without_trailing_dash() {
        let long = "a ".repeat(80); // 80 alpha chars separated by spaces
        let out = slugify(&long);
        assert!(out.len() <= MAX_SLUG_LEN, "len = {}", out.len());
        assert!(!out.ends_with('-'), "got: {out}");
    }

    #[test]
    fn cjk_only_returns_fallback() {
        // Pure non-Latin runes have no ASCII fold; everything dashes,
        // then trims to empty → fallback.
        assert_eq!(slugify("日本語"), FALLBACK_SLUG);
    }

    #[test]
    fn leading_and_trailing_punctuation_trimmed() {
        assert_eq!(slugify("--hello world!!"), "hello-world");
    }

    #[test]
    fn numbers_are_preserved() {
        assert_eq!(slugify("GPT-4 vs Claude 3.5"), "gpt-4-vs-claude-3-5");
    }
}
