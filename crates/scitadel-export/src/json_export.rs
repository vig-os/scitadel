use scitadel_core::models::Paper;

/// Export papers as a JSON array.
pub fn export_json(papers: &[Paper], indent: usize) -> String {
    if indent > 0 {
        serde_json::to_string_pretty(papers).unwrap_or_else(|_| "[]".to_string())
    } else {
        serde_json::to_string(papers).unwrap_or_else(|_| "[]".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_json_empty() {
        let result = export_json(&[], 2);
        assert_eq!(result, "[]");
    }

    #[test]
    fn test_export_json_round_trip() {
        let paper = Paper::new("Test Paper");
        let json = export_json(&[paper], 2);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0]["title"], "Test Paper");
    }
}
