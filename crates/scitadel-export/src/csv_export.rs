use scitadel_core::models::Paper;

/// Export papers as CSV.
pub fn export_csv(papers: &[Paper]) -> String {
    let mut wtr = csv::Writer::from_writer(Vec::new());

    wtr.write_record([
        "id",
        "title",
        "authors",
        "year",
        "journal",
        "doi",
        "arxiv_id",
        "pubmed_id",
        "inspire_id",
        "openalex_id",
        "abstract",
        "url",
    ])
    .ok();

    for p in papers {
        // `as_str`, not `rendered`: a CSV row is a data record, and the
        // 200-character cap would truncate the title in it — and the abstract,
        // which in a CSV is a whole column, not a preview.
        let authors = p
            .authors
            .iter()
            .map(|author| author.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        wtr.write_record([
            p.id.as_str(),
            p.title.as_str(),
            &authors,
            &p.year.map(|y| y.to_string()).unwrap_or_default(),
            p.journal.as_deref().unwrap_or(""),
            p.doi.as_deref().unwrap_or(""),
            p.arxiv_id.as_deref().unwrap_or(""),
            p.pubmed_id.as_deref().unwrap_or(""),
            p.inspire_id.as_deref().unwrap_or(""),
            p.openalex_id.as_deref().unwrap_or(""),
            p.r#abstract.as_str(),
            p.url.as_deref().unwrap_or(""),
        ])
        .ok();
    }

    String::from_utf8(wtr.into_inner().unwrap_or_default()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::untrusted::{UntrustedBody, UntrustedText};

    #[test]
    fn test_export_csv_header() {
        let result = export_csv(&[]);
        assert!(result.starts_with("id,title,authors,year"));
    }

    #[test]
    fn test_export_csv_with_paper() {
        let mut paper = Paper::new("Test Paper");
        paper.authors = vec![
            UntrustedText::ours("Alice Smith"),
            UntrustedText::ours("Bob Jones"),
        ];
        paper.year = Some(2024);

        let result = export_csv(&[paper]);
        assert!(result.contains("Alice Smith; Bob Jones"));
        assert!(result.contains("2024"));
    }

    /// An abstract long enough that a 200-character cap would cut it, with
    /// paragraph breaks a whitespace collapse would destroy and an escape a
    /// render would strip — the three things a data path must not do.
    fn long_abstract() -> String {
        format!(
            "First paragraph.\n\nSecond paragraph: {}.\n\u{1b}[2J\n\nThird paragraph: {}.",
            "lorem ipsum dolor sit amet ".repeat(10),
            "consectetur adipiscing elit ".repeat(10),
        )
    }

    /// #287's data-path half: a CSV row is a record, not a display, so the
    /// abstract goes out as stored — whole, newlines and all.
    ///
    /// The contrast is the point. `UntrustedText`'s 200-character cap is right
    /// for the title column and would truncate this abstract to a fragment, and
    /// a fragment in a reference manager's CSV is a corrupt record with nothing
    /// in it to say so. Asserted against both failure modes, because either one
    /// is what a careless wiring produces.
    #[test]
    fn test_export_csv_keeps_the_abstract_whole() {
        let mut paper = Paper::new("Test Paper");
        let raw = long_abstract();
        assert!(
            raw.chars().count() > 200,
            "the fixture really is longer than the cap"
        );
        paper.r#abstract = UntrustedBody::publisher_supplied(raw.as_str());

        let result = export_csv(&[paper]);

        assert!(
            result.contains(&raw),
            "the abstract column is the stored record, byte for byte: {result}"
        );
        assert!(
            result.chars().count() > 300,
            "and it is not capped at 200: {} characters",
            result.chars().count()
        );
    }
}
