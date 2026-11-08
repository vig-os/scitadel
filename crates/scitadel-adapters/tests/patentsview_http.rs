//! Wire contract of the PatentsView adapter against the PatentSearch v1
//! grammar (#268).
//!
//! USPTO's PatentSearch host is gone, so these run offline: the request
//! body is checked against the grammar's own field list, and a recorded
//! PatentSearch-shaped response is replayed through a mock server.

use std::collections::HashSet;

use scitadel_adapters::patentsview::{
    PATENT_FIELDS, PATENTSVIEW_API_URL, PatentsViewAdapter, request_body,
};
use scitadel_core::ports::SourceAdapter;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// `components.schemas.PatentFields` of the PatentSearch grammar, from
/// PatentRef's OpenAPI document (API 1.21.0, fetched 2026-11-08).
const GRAMMAR_FIELDS: &str = include_str!("fixtures/patentsearch_patent_fields.json");

fn grammar_fields() -> HashSet<String> {
    serde_json::from_str(GRAMMAR_FIELDS).expect("fixture is a JSON list of names")
}

/// One page as the `patent` endpoint returns it (`PatentResponse`).
fn recorded_page() -> serde_json::Value {
    serde_json::json!({
        "error": false,
        "count": 2,
        "total_hits": 2,
        "patents": [
            {
                "patent_id": "11234567",
                "patent_title": "Method for quantum error correction",
                "patent_abstract": "A method for correcting errors in quantum computing systems.",
                "patent_date": "2023-01-31",
                "patent_type": "utility",
                "wipo_kind": "B2",
                "assignees": [{"assignee_organization": "Quantum Corp"}],
                "inventors": [
                    {"inventor_name_first": "Alice", "inventor_name_last": "Smith"}
                ]
            },
            {
                "patent_id": "RE49999",
                "patent_title": "Reissued detector",
                "patent_abstract": null,
                "patent_date": "2022-06-07",
                "patent_type": "reissue",
                "wipo_kind": "E",
                "assignees": null,
                "inventors": null
            }
        ]
    })
}

#[test]
fn every_requested_field_exists_in_the_grammar() {
    let known = grammar_fields();
    let unknown: Vec<&str> = PATENT_FIELDS
        .iter()
        .copied()
        .filter(|f| !known.contains(*f))
        .collect();
    assert!(
        unknown.is_empty(),
        "fields the grammar rejects with 400 ERR_Q: {unknown:?}"
    );
}

#[test]
fn request_body_asks_for_wipo_kind_not_patent_kind() {
    let body = request_body("quantum", 25);
    let f: Vec<&str> = body["f"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(f.contains(&"wipo_kind"));
    assert!(!f.contains(&"patent_kind"));
    assert_eq!(body["o"]["size"], 25);
}

#[test]
fn request_body_caps_size_at_the_grammar_maximum() {
    assert_eq!(request_body("x", 5000)["o"]["size"], 1000);
}

#[test]
fn default_endpoint_is_not_the_retired_uspto_host() {
    assert!(!PATENTSVIEW_API_URL.contains("search.patentsview.org"));
    assert!(PATENTSVIEW_API_URL.ends_with("/api/v1/patent/"));
}

#[tokio::test]
async fn search_posts_the_body_with_the_key_and_parses_the_page() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/patent/"))
        .and(header("X-Api-Key", "pr-key-123"))
        .and(body_json(request_body("quantum error", 10)))
        .respond_with(ResponseTemplate::new(200).set_body_json(recorded_page()))
        .expect(1)
        .mount(&server)
        .await;

    let results = PatentsViewAdapter::new("pr-key-123".into(), 5.0)
        .with_base_url(format!("{}/api/v1/patent/", server.uri()))
        .search("quantum error", 10)
        .await
        .expect("search should succeed");

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].source_id, "11234567");
    assert_eq!(results[0].authors, vec!["Alice Smith"]);
    assert_eq!(
        results[0].journal.as_deref(),
        Some("Quantum Corp (utility B2)")
    );
    assert_eq!(results[1].source_id, "RE49999");
    assert_eq!(results[1].year, Some(2022));
    assert!(results[1].authors.is_empty());
}

#[tokio::test]
async fn a_rejected_query_surfaces_the_status_reason_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400)
                .insert_header("X-Status-Reason", "Invalid field: patent_kind")
                .insert_header("X-Status-Reason-Code", "ERR_Q")
                .set_body_json(serde_json::json!({"error": true})),
        )
        .mount(&server)
        .await;

    let err = PatentsViewAdapter::new("k".into(), 5.0)
        .with_base_url(server.uri())
        .search("x", 1)
        .await
        .expect_err("a 400 must be an error, not an empty result");
    let msg = err.to_string();
    assert!(msg.contains("400"), "{msg}");
    assert!(msg.contains("Invalid field: patent_kind"), "{msg}");
}

#[tokio::test]
async fn a_missing_key_points_at_where_to_get_one() {
    let err = PatentsViewAdapter::new(String::new(), 5.0)
        .search("x", 1)
        .await
        .expect_err("no key, no request");
    let msg = err.to_string();
    assert!(msg.contains("patentref.io"), "{msg}");
    assert!(!msg.contains("patentsview.org/apis"), "{msg}");
}
