use scitadel_core::models::Paper;

/// Export papers as BibTeX entries.
pub fn export_bibtex(papers: &[Paper]) -> String {
    let entries: Vec<String> = papers.iter().map(paper_to_bibtex).collect();
    if entries.is_empty() {
        String::new()
    } else {
        entries.join("\n\n") + "\n"
    }
}

fn paper_to_bibtex(paper: &Paper) -> String {
    let key = generate_bibtex_key(paper);
    let mut fields = Vec::new();

    fields.push(format!("  title = {{{}}}", paper.title));
    if !paper.authors.is_empty() {
        fields.push(format!("  author = {{{}}}", paper.authors.join(" and ")));
    }
    if let Some(year) = paper.year {
        fields.push(format!("  year = {{{year}}}"));
    }
    if let Some(ref journal) = paper.journal {
        fields.push(format!("  journal = {{{journal}}}"));
    }
    if let Some(ref doi) = paper.doi {
        fields.push(format!("  doi = {{{doi}}}"));
    }
    if let Some(ref url) = paper.url {
        fields.push(format!("  url = {{{url}}}"));
    }
    if let Some(ref arxiv_id) = paper.arxiv_id {
        fields.push(format!("  eprint = {{{arxiv_id}}}"));
        fields.push("  archiveprefix = {arXiv}".to_string());
    }
    if !paper.r#abstract.is_empty() {
        fields.push(format!("  abstract = {{{}}}", paper.r#abstract));
    }

    format!("@article{{{key},\n{}\n}}", fields.join(",\n"))
}

fn generate_bibtex_key(paper: &Paper) -> String {
    let author_part = paper
        .authors
        .first()
        .map(|a| {
            let name = a.split(',').next().unwrap_or(a);
            let last = name.split_whitespace().last().unwrap_or("").to_lowercase();
            last.chars().filter(|c| c.is_alphanumeric()).collect::<String>()
        })
        .unwrap_or_default();

    let year_part = paper.year.map(|y| y.to_string()).unwrap_or_default();

    let title_part = paper
        .title
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();

    let key = format!("{author_part}{year_part}{title_part}");
    if key.is_empty() {
        paper.id.short().to_string()
    } else {
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_bibtex_key() {
        let mut paper = Paper::new("Machine Learning for Science");
        paper.authors = vec!["Smith, John".into()];
        paper.year = Some(2024);

        let key = generate_bibtex_key(&paper);
        assert_eq!(key, "smith2024machine");
    }

    #[test]
    fn test_export_bibtex_basic() {
        let mut paper = Paper::new("Test Paper");
        paper.authors = vec!["Alice Smith".into()];
        paper.year = Some(2024);
        paper.doi = Some("10.1234/test".into());

        let result = export_bibtex(&[paper]);
        assert!(result.contains("@article{"));
        assert!(result.contains("title = {Test Paper}"));
        assert!(result.contains("doi = {10.1234/test}"));
    }

    #[test]
    fn test_export_bibtex_empty() {
        assert_eq!(export_bibtex(&[]), "");
    }
}
