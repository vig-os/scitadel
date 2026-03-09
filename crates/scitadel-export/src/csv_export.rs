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
        wtr.write_record([
            p.id.as_str(),
            &p.title,
            &p.authors.join("; "),
            &p.year.map(|y| y.to_string()).unwrap_or_default(),
            p.journal.as_deref().unwrap_or(""),
            p.doi.as_deref().unwrap_or(""),
            p.arxiv_id.as_deref().unwrap_or(""),
            p.pubmed_id.as_deref().unwrap_or(""),
            p.inspire_id.as_deref().unwrap_or(""),
            p.openalex_id.as_deref().unwrap_or(""),
            &p.r#abstract,
            p.url.as_deref().unwrap_or(""),
        ])
        .ok();
    }

    String::from_utf8(wtr.into_inner().unwrap_or_default()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_csv_header() {
        let result = export_csv(&[]);
        assert!(result.starts_with("id,title,authors,year"));
    }

    #[test]
    fn test_export_csv_with_paper() {
        let mut paper = Paper::new("Test Paper");
        paper.authors = vec!["Alice Smith".into(), "Bob Jones".into()];
        paper.year = Some(2024);

        let result = export_csv(&[paper]);
        assert!(result.contains("Alice Smith; Bob Jones"));
        assert!(result.contains("2024"));
    }
}
