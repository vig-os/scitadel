pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(3);
    let mut out: String = s.chars().take(keep).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn keeps_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncates_ascii() {
        assert_eq!(truncate("abcdefghij", 6), "abc...");
    }

    #[test]
    fn multi_byte_char_boundary_is_respected() {
        // Curly apostrophe U+2019 is 3 bytes; byte-slice of 27 would land mid-char.
        let s = "Isaac B. Hilton, Anthony D\u{2019}Ippolito et al.";
        let out = truncate(s, 30);
        assert!(out.ends_with("..."));
        assert!(out.chars().count() <= 30);
    }

    #[test]
    fn handles_zero_max() {
        assert_eq!(truncate("anything", 0), "...");
    }
}
