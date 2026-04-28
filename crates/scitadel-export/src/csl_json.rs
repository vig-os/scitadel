//! CSL-JSON 1.0.2 export (#135 sub-feature A).
//!
//! Mirrors [`crate::bibtex`]'s shape so the `bib snapshot` / `bib verify`
//! plumbing can route on the sidecar's `format` discriminant. We target
//! plain canonical CSL 1.0.2 (NOT citeproc-js extended, NOT pandoc
//! flavored) — Typst's own Rust libs (`citationberg`, `hayagriva`) read
//! canonical, and a `--pandoc` escape hatch can bolt on later.
//!
//! **Determinism invariants (checked by tests below):**
//! - Entries sorted alphabetically by `id`
//! - JSON object keys emitted in a fixed canonical order (id, type, title,
//!   author, issued, container-title, DOI, URL, abstract, keyword)
//! - Author order is preserved (citation order is meaningful)
//! - LF line endings; pretty-printed with 2-space indent
//! - No timestamps anywhere in the output
//!
//! **Field omission rule:** when a paper field is `None` / empty, the
//! corresponding CSL field is omitted entirely. We never emit `null`,
//! empty string, or empty array — that would just create JSON noise the
//! canonical schema considers ambiguous.

use scitadel_core::bibtex_key::generate_key;
use scitadel_core::models::Paper;
use serde_json::{Map, Value};

/// Default CSL `type` when the internal data model carries no
/// `paper_type`. Today `Paper` has no such field, so every entry gets
/// this. When/if the model grows a paper_type, only values present in
/// [`CSL_TYPES`] should be emitted (canonical schema is strict).
pub const DEFAULT_CSL_TYPE: &str = "article-journal";

/// Canonical CSL 1.0.2 `type` enum. Sourced from the schema's
/// `csl-data.json`. Used so a future paper_type-aware mapper can fall
/// back to `article-journal` for anything outside the set rather than
/// emitting JSON the canonical schema rejects.
pub const CSL_TYPES: &[&str] = &[
    "article",
    "article-journal",
    "article-magazine",
    "article-newspaper",
    "bill",
    "book",
    "broadcast",
    "chapter",
    "classic",
    "collection",
    "dataset",
    "document",
    "entry",
    "entry-dictionary",
    "entry-encyclopedia",
    "event",
    "figure",
    "graphic",
    "hearing",
    "interview",
    "legal_case",
    "legislation",
    "manuscript",
    "map",
    "motion_picture",
    "musical_score",
    "pamphlet",
    "paper-conference",
    "patent",
    "performance",
    "periodical",
    "personal_communication",
    "post",
    "post-weblog",
    "regulation",
    "report",
    "review",
    "review-book",
    "software",
    "song",
    "speech",
    "standard",
    "thesis",
    "treaty",
    "webpage",
];

/// Export papers as a deterministic CSL-JSON 1.0.2 document. Returns the
/// pretty-printed JSON array string (with trailing newline) for parity
/// with [`crate::export_bibtex`].
#[must_use]
pub fn export_csl_json(papers: &[Paper]) -> String {
    export_csl_json_with_tags(papers, |_| Vec::new())
}

/// Same as [`export_csl_json`] but threads paper-tags into a `keyword`
/// field per the CSL 1.0.2 spec (single comma-separated string, NOT an
/// array — schema mandates `"type": "string"` for `keyword`).
pub fn export_csl_json_with_tags<F>(papers: &[Paper], mut tags_for: F) -> String
where
    F: FnMut(&str) -> Vec<String>,
{
    // Sort by id (the field that ends up in the JSON, mirroring how
    // BibTeX sorts by citekey). Stable so ties keep insertion order.
    let mut entries: Vec<(String, &Paper)> = papers.iter().map(|p| (csl_id_for(p), p)).collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut arr: Vec<Value> = Vec::with_capacity(entries.len());
    for (id, paper) in &entries {
        let tags = tags_for(paper.id.as_str());
        arr.push(paper_to_csl(id, paper, &tags));
    }

    // serde_json's pretty printer emits 2-space indent + LF. We rely on
    // BTreeMap-ordered insertion + our manual canonical-order emit (see
    // `paper_to_csl`) for stable output.
    let mut s = serde_json::to_string_pretty(&Value::Array(arr))
        .expect("Map<String, Value> always serializes");
    s.push('\n');
    s
}

/// Choose the CSL `id` for a paper: persisted `bibtex_key` if present,
/// else regenerate via the same algorithm BibTeX export uses. Falls
/// back to `paper.id` if even that fails. Always a non-empty string.
fn csl_id_for(paper: &Paper) -> String {
    if let Some(k) = paper.bibtex_key.as_ref().filter(|s| !s.is_empty()) {
        return k.clone();
    }
    let generated = generate_key(paper);
    if generated.is_empty() {
        paper.id.as_str().to_string()
    } else {
        generated
    }
}

/// Build a single CSL-JSON object for one paper. Field order matches
/// the spec's typical ordering and is fixed so the output is byte-stable
/// across runs. Empty / `None` fields are omitted entirely.
fn paper_to_csl(id: &str, paper: &Paper, tags: &[String]) -> Value {
    // Use a Map (not BTreeMap-via-derive) so we control insertion
    // order. serde_json's Map preserves insertion order with the
    // `preserve_order` feature; without it, fields are alphabetized
    // anyway, which is also deterministic. Either way, byte-stable.
    let mut m = Map::new();

    m.insert("id".into(), Value::String(id.to_string()));
    m.insert("type".into(), Value::String(DEFAULT_CSL_TYPE.into()));
    if !paper.title.is_empty() {
        m.insert("title".into(), Value::String(paper.title.clone()));
    }
    if !paper.authors.is_empty() {
        let authors: Vec<Value> = paper.authors.iter().map(|a| author_to_csl(a)).collect();
        m.insert("author".into(), Value::Array(authors));
    }
    if let Some(year) = paper.year {
        // Canonical schema: `issued.date-parts` is `[[YYYY]]` or
        // `[[YYYY, MM, DD]]`. Numbers, not strings.
        let inner = Value::Array(vec![Value::Number(year.into())]);
        let mut date = Map::new();
        date.insert("date-parts".into(), Value::Array(vec![inner]));
        m.insert("issued".into(), Value::Object(date));
    }
    if let Some(j) = paper.journal.as_ref().filter(|s| !s.is_empty()) {
        // CSL canonical name is `container-title`, NOT `journal`.
        m.insert("container-title".into(), Value::String(j.clone()));
    }
    if let Some(d) = paper.doi.as_ref().filter(|s| !s.is_empty()) {
        // Canonical capitalization is `DOI` (uppercase) per spec.
        m.insert("DOI".into(), Value::String(d.clone()));
    }
    if let Some(u) = paper.url.as_ref().filter(|s| !s.is_empty()) {
        m.insert("URL".into(), Value::String(u.clone()));
    }
    if !paper.r#abstract.is_empty() {
        m.insert("abstract".into(), Value::String(paper.r#abstract.clone()));
    }
    if !tags.is_empty() {
        // Spec: `keyword` is a single string ("comma-separated keywords"
        // per common-practice convention). Joining with ", " matches
        // citeproc-js behavior and Zotero's emitter.
        m.insert("keyword".into(), Value::String(tags.join(", ")));
    }
    Value::Object(m)
}

/// Parse a single author string into the canonical
/// `{ family, given }` shape. Convention used across BibTeX, Zotero,
/// CrossRef: `"Last, First"`. If no comma is present, treat the entire
/// string as `family` (matches CSL's `literal` fallback in spirit
/// without requiring the `literal` field, which the canonical schema
/// allows but most processors don't render the same).
fn author_to_csl(author: &str) -> Value {
    let trimmed = author.trim();
    if let Some(idx) = trimmed.find(',') {
        let family = trimmed[..idx].trim().to_string();
        let given = trimmed[idx + 1..].trim().to_string();
        let mut m = Map::new();
        m.insert("family".into(), Value::String(family));
        if !given.is_empty() {
            m.insert("given".into(), Value::String(given));
        }
        Value::Object(m)
    } else {
        let mut m = Map::new();
        m.insert("family".into(), Value::String(trimmed.to_string()));
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scitadel_core::models::PaperId;
    use serde_json::json;

    fn paper(title: &str, authors: &[&str], year: Option<i32>) -> Paper {
        let mut p = Paper::new(title);
        p.authors = authors.iter().map(|s| (*s).to_string()).collect();
        p.year = year;
        p
    }

    fn parse(out: &str) -> Vec<Value> {
        let v: Value = serde_json::from_str(out).expect("valid JSON");
        v.as_array().expect("top-level array").clone()
    }

    #[test]
    fn export_emits_top_level_array() {
        let mut p = paper("X", &["Smith"], Some(2024));
        p.bibtex_key = Some("smith2024x".into());
        let out = export_csl_json(&[p]);
        let arr = parse(&out);
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn export_is_deterministic_across_runs() {
        let mut p1 = paper("Zebra", &["Zed"], Some(2024));
        p1.bibtex_key = Some("zed2024zebra".into());
        let mut p2 = paper("Apple", &["Aaron"], Some(2024));
        p2.bibtex_key = Some("aaron2024apple".into());
        let a = export_csl_json(&[p1.clone(), p2.clone()]);
        let b = export_csl_json(&[p2, p1]);
        assert_eq!(a, b, "input order must not affect output");
    }

    #[test]
    fn export_sorts_by_id() {
        let mut p1 = paper("Zebra", &["Zed"], Some(2024));
        p1.bibtex_key = Some("zed2024zebra".into());
        let mut p2 = paper("Apple", &["Aaron"], Some(2024));
        p2.bibtex_key = Some("aaron2024apple".into());
        let out = export_csl_json(&[p1, p2]);
        let aaron = out.find("aaron2024apple").unwrap();
        let zed = out.find("zed2024zebra").unwrap();
        assert!(aaron < zed, "alphabetical by id");
    }

    #[test]
    fn id_uses_bibtex_key_when_present() {
        let mut p = paper("X", &["Smith"], Some(2024));
        p.bibtex_key = Some("custom-key".into());
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["id"], json!("custom-key"));
    }

    #[test]
    fn id_falls_back_to_generated_key_when_bibtex_key_missing() {
        let p = paper("Machine Learning", &["Smith, John"], Some(2024));
        let arr = parse(&export_csl_json(&[p]));
        // generate_key is deterministic; "smith2024machine" per ADR-006.
        assert_eq!(arr[0]["id"], json!("smith2024machine"));
    }

    #[test]
    fn type_defaults_to_article_journal() {
        let p = paper("X", &["Smith"], Some(2024));
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["type"], json!("article-journal"));
        assert!(CSL_TYPES.contains(&"article-journal"));
    }

    #[test]
    fn journal_maps_to_container_title() {
        let mut p = paper("X", &["Smith"], Some(2024));
        p.journal = Some("Nature".into());
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["container-title"], json!("Nature"));
        assert!(arr[0].get("journal").is_none(), "must NOT emit `journal`");
    }

    #[test]
    fn year_maps_to_issued_date_parts() {
        let p = paper("X", &["Smith"], Some(2024));
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["issued"]["date-parts"][0][0], json!(2024));
    }

    #[test]
    fn missing_year_omits_issued() {
        let p = paper("X", &["Smith"], None);
        let arr = parse(&export_csl_json(&[p]));
        assert!(arr[0].get("issued").is_none());
    }

    #[test]
    fn authors_parse_last_first_with_comma() {
        let p = paper("X", &["Smith, John"], Some(2024));
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["author"][0]["family"], json!("Smith"));
        assert_eq!(arr[0]["author"][0]["given"], json!("John"));
    }

    #[test]
    fn authors_without_comma_become_family_only() {
        let p = paper("X", &["Madonna"], Some(2024));
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["author"][0]["family"], json!("Madonna"));
        assert!(arr[0]["author"][0].get("given").is_none());
    }

    #[test]
    fn authors_preserve_input_order() {
        let p = paper("X", &["Zed, Z.", "Aaron, A."], Some(2024));
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["author"][0]["family"], json!("Zed"));
        assert_eq!(arr[0]["author"][1]["family"], json!("Aaron"));
    }

    #[test]
    fn doi_uses_uppercase_canonical_field() {
        let mut p = paper("X", &["Smith"], Some(2024));
        p.doi = Some("10.1/abc".into());
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["DOI"], json!("10.1/abc"));
        assert!(arr[0].get("doi").is_none(), "canonical is uppercase DOI");
    }

    #[test]
    fn url_uses_uppercase_canonical_field() {
        let mut p = paper("X", &["Smith"], Some(2024));
        p.url = Some("https://example.org/x".into());
        let arr = parse(&export_csl_json(&[p]));
        assert_eq!(arr[0]["URL"], json!("https://example.org/x"));
    }

    #[test]
    fn keyword_is_comma_joined_string_not_array() {
        let p = paper("X", &["Smith"], Some(2024));
        let out = export_csl_json_with_tags(&[p], |_| {
            vec!["alpha".into(), "beta".into(), "gamma".into()]
        });
        let arr = parse(&out);
        assert_eq!(arr[0]["keyword"], json!("alpha, beta, gamma"));
        assert!(arr[0]["keyword"].is_string(), "spec: keyword is a string");
    }

    #[test]
    fn no_tags_omits_keyword_field() {
        let p = paper("X", &["Smith"], Some(2024));
        let arr = parse(&export_csl_json_with_tags(&[p], |_| Vec::new()));
        assert!(arr[0].get("keyword").is_none());
    }

    #[test]
    fn empty_papers_produces_empty_array() {
        let out = export_csl_json(&[]);
        assert_eq!(parse(&out).len(), 0);
        assert!(out.starts_with('['));
    }

    #[test]
    fn missing_optional_fields_are_omitted_not_nulled() {
        let mut p = Paper::new("Bare title");
        p.id = PaperId::from("p-bare");
        p.bibtex_key = Some("bare-key".into());
        // No authors, no year, no journal, no doi, no url, no abstract.
        let arr = parse(&export_csl_json(&[p]));
        let entry = &arr[0];
        for key in [
            "author",
            "issued",
            "container-title",
            "DOI",
            "URL",
            "abstract",
            "keyword",
        ] {
            assert!(
                entry.get(key).is_none(),
                "{key} must be omitted, not null/empty"
            );
        }
        // Required fields still present.
        assert_eq!(entry["id"], json!("bare-key"));
        assert_eq!(entry["type"], json!("article-journal"));
        assert_eq!(entry["title"], json!("Bare title"));
    }

    #[test]
    fn output_uses_lf_not_crlf() {
        let mut p = paper("X", &["Smith"], Some(2024));
        p.bibtex_key = Some("smith2024x".into());
        let out = export_csl_json(&[p]);
        assert!(!out.contains('\r'), "force LF; no CR anywhere");
    }

    #[test]
    fn preserves_unicode_literally() {
        let mut p = paper("Über quantum", &["Müller, Hans"], Some(2024));
        p.bibtex_key = Some("muller2024uber".into());
        let out = export_csl_json(&[p]);
        assert!(out.contains("Über quantum"));
        assert!(out.contains("Müller"));
    }

    // ---------- Schema validation ----------
    //
    // A full JSON Schema validator is overkill for the canonical CSL
    // schema's needs here (we control the emitter; the failure modes
    // are mismatched field names, missing required fields, and wrong
    // types). Hand-rolled struct validation against the canonical
    // 1.0.2 schema's invariants — pulled from
    // https://github.com/citation-style-language/schema/blob/master/
    //   schemas/input/csl-data.json — covers what we actually need.

    fn validate_csl_entry(entry: &Value) -> Result<(), String> {
        let obj = entry
            .as_object()
            .ok_or_else(|| "entry must be an object".to_string())?;

        // Required: id (string OR number per spec; we always emit string).
        match obj.get("id") {
            Some(v) if v.is_string() || v.is_number() => {}
            Some(_) => return Err("id must be string or number".into()),
            None => return Err("id is required".into()),
        }
        // Required: type (string from canonical enum).
        match obj.get("type") {
            Some(Value::String(s)) => {
                if !CSL_TYPES.contains(&s.as_str()) {
                    return Err(format!("type '{s}' is not in canonical CSL enum"));
                }
            }
            Some(_) => return Err("type must be string".into()),
            None => return Err("type is required".into()),
        }
        // Optional but typed: title (string), DOI (string), URL (string),
        // container-title (string), abstract (string), keyword (string).
        for key in [
            "title",
            "DOI",
            "URL",
            "container-title",
            "abstract",
            "keyword",
        ] {
            if let Some(v) = obj.get(key)
                && !v.is_string()
            {
                return Err(format!("{key} must be string"));
            }
        }
        // Optional: author (array of name objects with `family` or `literal`).
        if let Some(authors) = obj.get("author") {
            let arr = authors.as_array().ok_or("author must be array")?;
            for a in arr {
                let ao = a.as_object().ok_or("author entry must be object")?;
                let has_name = ao.contains_key("family") || ao.contains_key("literal");
                if !has_name {
                    return Err("author entry needs `family` or `literal`".into());
                }
                for nk in [
                    "family",
                    "given",
                    "literal",
                    "suffix",
                    "non-dropping-particle",
                ] {
                    if let Some(v) = ao.get(nk)
                        && !v.is_string()
                    {
                        return Err(format!("author.{nk} must be string"));
                    }
                }
            }
        }
        // Optional: issued (date object with `date-parts` or `literal`/`raw`).
        if let Some(d) = obj.get("issued") {
            let dobj = d.as_object().ok_or("issued must be object")?;
            if let Some(dp) = dobj.get("date-parts") {
                let outer = dp.as_array().ok_or("date-parts must be array")?;
                for inner in outer {
                    let inner_arr = inner.as_array().ok_or("date-parts[i] must be array")?;
                    for n in inner_arr {
                        if !n.is_number() && !n.is_string() {
                            return Err("date-parts[i][j] must be number or string".into());
                        }
                    }
                }
            }
        }

        // Reject any *known wrong-name* fields that we'd hate to ship by
        // mistake (defends against future regressions).
        for forbidden in ["journal", "doi", "url", "keywords"] {
            if obj.contains_key(forbidden) {
                return Err(format!("forbidden non-canonical field: {forbidden}"));
            }
        }
        Ok(())
    }

    fn validate_csl_array(out: &str) -> Result<(), String> {
        let arr = parse(out);
        for (i, e) in arr.iter().enumerate() {
            validate_csl_entry(e).map_err(|err| format!("entry {i}: {err}"))?;
        }
        Ok(())
    }

    #[test]
    fn schema_full_metadata_validates() {
        let mut p = paper(
            "Attention Is All You Need",
            &["Vaswani, A.", "Shazeer, N.", "Parmar, N."],
            Some(2017),
        );
        p.bibtex_key = Some("vaswani2017attention".into());
        p.doi = Some("10.48550/arXiv.1706.03762".into());
        p.url = Some("https://arxiv.org/abs/1706.03762".into());
        p.journal = Some("NeurIPS".into());
        p.r#abstract = "We propose…".into();
        let out =
            export_csl_json_with_tags(&[p], |_| vec!["transformer".into(), "attention".into()]);
        validate_csl_array(&out).expect("full-metadata entry must validate");
    }

    #[test]
    fn schema_missing_optional_fields_validates() {
        let mut p = Paper::new("Bare");
        p.id = PaperId::from("p-bare");
        p.bibtex_key = Some("bare2020".into());
        let out = export_csl_json(&[p]);
        validate_csl_array(&out).expect("bare entry must still validate");
    }

    #[test]
    fn schema_multi_author_validates() {
        let mut p = paper(
            "X",
            &[
                "Smith, John",
                "Doe, Jane",
                "Müller, Hans",
                "Madonna", // no comma — family-only fallback
            ],
            Some(2024),
        );
        p.bibtex_key = Some("smith2024x".into());
        let out = export_csl_json(&[p]);
        validate_csl_array(&out).expect("multi-author entry must validate");
    }
}
