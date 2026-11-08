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
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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

/// The expected POST body, written out as a literal — the SUT builds its
/// own `request_body(..)`, so matching with `body_json(request_body(..))`
/// would be testing serde_json's determinism against itself and would
/// silently pass if the shape drifted on both sides together. Each field
/// is spelled out here to catch a change in the request grammar.
fn expected_request_body(query: &str, size: usize) -> serde_json::Value {
    serde_json::json!({
        "q": { "_text_any": { "patent_abstract": query } },
        "f": [
            "patent_id",
            "patent_title",
            "patent_abstract",
            "patent_date",
            "patent_type",
            "wipo_kind",
            "assignees.assignee_organization",
            "inventors.inventor_name_first",
            "inventors.inventor_name_last",
        ],
        "o": { "size": size },
        "s": [{ "patent_date": "desc" }],
    })
}

#[tokio::test]
async fn search_posts_the_body_with_the_key_and_parses_the_page() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/patent/"))
        .and(header("X-Api-Key", "pr-key-123"))
        // Literal expected body: contains wipo_kind, lacks patent_kind,
        // keeps the grammar's q/f/s/o shape. This is the assertion the
        // previous `body_json(request_body(..))` matcher failed to make.
        .and(body_json(expected_request_body("quantum error", 10)))
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

/// Even without the literal matcher, the body must contain `wipo_kind`
/// and must NOT contain `patent_kind` (the field name a strict server
/// rejects with 400 ERR_Q — the reason this bug exists).
#[tokio::test]
async fn the_posted_body_contains_wipo_kind_and_lacks_patent_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/patent/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(recorded_page()))
        .expect(1)
        .mount(&server)
        .await;

    let _ = PatentsViewAdapter::new("k".into(), 5.0)
        .with_base_url(format!("{}/api/v1/patent/", server.uri()))
        .search("x", 10)
        .await
        .expect("search should succeed");

    let reqs: Vec<Request> = server.received_requests().await.unwrap_or_default();
    assert_eq!(reqs.len(), 1, "exactly one POST");
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).expect("body must be JSON");
    let f = body["f"].as_array().expect("f is a JSON array");
    let names: Vec<&str> = f.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        names.contains(&"wipo_kind"),
        "body.f lacks wipo_kind: {names:?}"
    );
    assert!(
        !names.contains(&"patent_kind"),
        "body.f must not include patent_kind (strict servers reject it): {names:?}"
    );
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

/// reqwest's default redirect policy keeps custom headers across 3xx
/// hops, and `X-Api-Key` is one of ours — a redirect to an attacker
/// controlled host would leak the key. The adapter must refuse to
/// follow, and the error must name the Location host so the operator
/// can trace what happened (#268).
#[tokio::test]
async fn a_cross_host_redirect_is_refused_with_the_location_host_named() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "https://attacker.example.com/steal/"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let err = PatentsViewAdapter::new("pr-key-123".into(), 5.0)
        .with_base_url(server.uri())
        .search("x", 1)
        .await
        .expect_err("a 3xx must be an adapter error, not a silent follow");
    let msg = err.to_string();
    assert!(
        msg.contains("attacker.example.com"),
        "error must name the Location host: {msg}"
    );
    assert!(msg.contains("302"), "error must name the status: {msg}");
    assert!(
        msg.contains("X-Api-Key") || msg.contains("redirect"),
        "error should explain why the hop was refused: {msg}"
    );

    // The adapter must have sent exactly ONE request — no redirect follow.
    let reqs = server.received_requests().await.unwrap_or_default();
    assert_eq!(reqs.len(), 1, "no redirect follow allowed");
}
