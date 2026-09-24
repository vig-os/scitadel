//! HTTP-level behaviour of the OpenAlex adapter (#212).
//!
//! Two things the unit tests can't cover: that the credentials actually
//! reach the wire on every endpoint, and that a non-2xx response becomes
//! an `Err` rather than an empty result set.

use scitadel_adapters::openalex::OpenAlexAdapter;
use scitadel_core::config::OpenAlexAuth;
use scitadel_core::ports::SourceAdapter;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn auth() -> OpenAlexAuth {
    OpenAlexAuth {
        email: "me@example.org".into(),
        api_key: "oa-key-123".into(),
    }
}

fn adapter(server: &MockServer) -> OpenAlexAdapter {
    OpenAlexAdapter::new(auth(), 5.0).with_base_url(format!("{}/works", server.uri()))
}

/// The 429 body OpenAlex actually returns once the shared per-IP budget
/// for keyless requests is spent.
const BUDGET_EXHAUSTED: &str = r#"{"error":"Insufficient budget","message":"Insufficient budget. This request has no API key, so it was billed to a shared pool that is now empty."}"#;

fn works_page(titles: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "results": titles
            .iter()
            .enumerate()
            .map(|(i, t)| serde_json::json!({
                "id": format!("https://openalex.org/W{}", 100 + i),
                "title": t,
                "publication_year": 2024,
            }))
            .collect::<Vec<_>>()
    })
}

#[tokio::test]
async fn search_sends_both_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("api_key", "oa-key-123"))
        .and(query_param("mailto", "me@example.org"))
        .and(query_param("search", "DOTA lutetium"))
        .and(query_param("per_page", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&["A", "B"])))
        .expect(1)
        .mount(&server)
        .await;

    let results = adapter(&server)
        .search("DOTA lutetium", 5)
        .await
        .expect("search should succeed");
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn search_surfaces_http_429_instead_of_zero_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_raw(BUDGET_EXHAUSTED, "application/json")
                .insert_header("content-type", "application/json"),
        )
        .mount(&server)
        .await;

    let err = adapter(&server)
        .search("DOTA lutetium", 5)
        .await
        .expect_err("a 429 must not read as an empty result set");

    let msg = err.to_string();
    assert!(msg.contains("429"), "error should name the status: {msg}");
    assert!(
        msg.contains("Insufficient budget"),
        "error should carry the upstream message: {msg}"
    );
    assert!(
        !msg.contains("oa-key-123"),
        "the api_key must never leak into an error string: {msg}"
    );
}

#[tokio::test]
async fn a_500_with_an_html_body_still_produces_a_readable_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string("<html><body>bad gateway</body></html>"),
        )
        .mount(&server)
        .await;

    let err = adapter(&server)
        .search("q", 5)
        .await
        .expect_err("a 500 must be an error");
    let msg = err.to_string();
    assert!(msg.contains("500"), "{msg}");
    assert!(msg.contains("bad gateway"), "{msg}");
}

#[tokio::test]
async fn fetch_work_by_id_sends_the_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works/W2741809807"))
        .and(query_param("api_key", "oa-key-123"))
        .and(query_param("mailto", "me@example.org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "https://openalex.org/W2741809807",
            "title": "Foundational paper",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let work = adapter(&server)
        .fetch_work_by_id("W2741809807")
        .await
        .expect("fetch should succeed");
    assert_eq!(work["title"], "Foundational paper");
}

#[tokio::test]
async fn cited_by_and_batch_fetch_send_the_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("api_key", "oa-key-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&["Citing paper"])))
        .expect(2)
        .mount(&server)
        .await;

    let adapter = adapter(&server);

    // cited_by — the snowball "who cites this" direction.
    let citing = adapter.fetch_cited_by("W1", 25).await.unwrap();
    assert_eq!(citing.len(), 1);

    // batch fetch by ids — the references / snowball materialisation leg.
    let batch = adapter
        .fetch_works_by_ids(&["W1".to_string(), "W2".to_string()])
        .await
        .unwrap();
    assert_eq!(batch.len(), 1);
}

#[tokio::test]
async fn cited_by_propagates_a_429() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/works.*"))
        .respond_with(ResponseTemplate::new(429).set_body_raw(BUDGET_EXHAUSTED, "application/json"))
        .mount(&server)
        .await;

    let adapter = adapter(&server);
    assert!(adapter.fetch_cited_by("W1", 25).await.is_err());
    assert!(adapter.fetch_work_by_id("W1").await.is_err());
    assert!(
        adapter
            .fetch_works_by_ids(&["W1".to_string()])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_keyless_adapter_sends_only_the_mailto() {
    let server = MockServer::start().await;
    let auth = OpenAlexAuth {
        email: "me@example.org".into(),
        api_key: String::new(),
    };
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("mailto", "me@example.org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&["A"])))
        .expect(1)
        .mount(&server)
        .await;

    let adapter = OpenAlexAdapter::new(auth, 5.0).with_base_url(format!("{}/works", server.uri()));
    assert_eq!(adapter.search("q", 5).await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_orchestrator_records_a_429_as_a_failed_source() {
    // End-to-end for defect 3: adapter error → SourceOutcome::Failed with
    // the message attached, while a healthy sibling source keeps its hits.
    use scitadel_core::models::{CandidatePaper, SourceStatus};

    struct Healthy;

    #[async_trait::async_trait]
    impl scitadel_core::ports::SourceAdapter for Healthy {
        fn name(&self) -> &str {
            "arxiv"
        }
        async fn search(
            &self,
            _q: &str,
            _n: usize,
        ) -> Result<Vec<CandidatePaper>, scitadel_core::error::CoreError> {
            Ok(vec![CandidatePaper::new("arxiv", "1", "Survivor")])
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .respond_with(ResponseTemplate::new(429).set_body_raw(BUDGET_EXHAUSTED, "application/json"))
        .mount(&server)
        .await;

    let adapters: Vec<Box<dyn SourceAdapter>> = vec![Box::new(adapter(&server)), Box::new(Healthy)];
    let (search, candidates) =
        scitadel_core::services::orchestrator::run_search("q", &adapters, 5, 1).await;

    assert_eq!(candidates.len(), 1, "arxiv results must survive");

    let oa = search
        .source_outcomes
        .iter()
        .find(|o| o.source == "openalex")
        .expect("openalex outcome recorded");
    assert_eq!(oa.status, SourceStatus::Failed);
    assert_eq!(oa.result_count, 0);
    let err = oa.error.as_deref().unwrap_or_default();
    assert!(err.starts_with("HTTP 429"), "{err}");
    assert!(err.contains("Insufficient budget"), "{err}");

    // And the whole record round-trips through serde, so history and
    // JSON exports carry the failure too.
    let json = serde_json::to_value(&search).unwrap();
    let outcomes = json["source_outcomes"].as_array().unwrap();
    let oa_json = outcomes.iter().find(|o| o["source"] == "openalex").unwrap();
    assert_eq!(oa_json["status"], "failed");
    assert!(
        oa_json["error"]
            .as_str()
            .unwrap()
            .contains("Insufficient budget")
    );
}
