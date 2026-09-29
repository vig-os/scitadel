//! HTTP-level behaviour of the OpenAlex adapter (#212).
//!
//! Two things the unit tests can't cover: that the credentials actually
//! reach the wire on every endpoint, and that a non-2xx response becomes
//! an `Err` rather than an empty result set.

use scitadel_adapters::openalex::{OpenAlexAdapter, SearchField};
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

// ---------- #210: title-aware search + DOI lookup ----------

/// Live probe evidence (recorded 2026-09 while implementing #210):
///
/// - `GET /works?search=Estimating+the+Dimension+of+a+Model` — 4.07M
///   hits, target #1 today but historically drowned by fulltext noise.
/// - `GET /works?filter=title.search:Estimating+the+Dimension+of+a+Model` —
///   474 hits, target #1, all top results are the Schwarz paper.
/// - `GET /works?filter=display_name.search:…` is aliased server-side
///   to `title.search`, so we standardise on the latter.
///
/// The wiremock tests below encode both call shapes and the auto path's
/// "title first, broad fills the tail" merge.
#[tokio::test]
async fn title_search_hits_the_title_filter_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("api_key", "oa-key-123"))
        .and(query_param(
            "filter",
            "title.search:Estimating the Dimension of a Model",
        ))
        .and(query_param("per_page", "5"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(works_page(&["Estimating the Dimension of a Model"])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let results = adapter(&server)
        .search_field("Estimating the Dimension of a Model", 5, SearchField::Title)
        .await
        .expect("title search should succeed");
    assert_eq!(results.len(), 1);
    assert!(results[0].title.contains("Estimating the Dimension"));
}

#[tokio::test]
async fn auto_search_prefers_title_hits_and_pads_from_broad() {
    let server = MockServer::start().await;
    // Title leg returns two hits, share one work id (W100) with the broad
    // leg so the dedup path is exercised.
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("filter", "title.search:BIC"))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&["Title A", "Title B"])))
        .expect(1)
        .mount(&server)
        .await;
    // Broad leg returns three hits; W100 overlaps title, W102/W103 fresh.
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("search", "BIC"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [
                {"id": "https://openalex.org/W100", "title": "Title A"},
                {"id": "https://openalex.org/W102", "title": "Broad C"},
                {"id": "https://openalex.org/W103", "title": "Broad D"},
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let results = adapter(&server)
        .search_field("BIC", 5, SearchField::Auto)
        .await
        .expect("auto search should succeed");

    // Title hits come first, dedup drops W100 from broad, tail padded.
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].title, "Title A");
    assert_eq!(results[1].title, "Title B");
    assert_eq!(results[2].title, "Broad C");
    assert_eq!(results[3].title, "Broad D");
    // Rank is re-numbered across the merged list.
    assert_eq!(results[0].rank, Some(1));
    assert_eq!(results[3].rank, Some(4));
}

#[tokio::test]
async fn auto_search_skips_the_broad_leg_when_title_already_full() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("filter", "title.search:BIC"))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&["A", "B", "C"])))
        .expect(1)
        .mount(&server)
        .await;
    // `search=` path must NOT fire when title returned max_results.
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("search", "BIC"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let results = adapter(&server)
        .search_field("BIC", 3, SearchField::Auto)
        .await
        .expect("auto search should succeed");
    assert_eq!(results.len(), 3);
}

#[tokio::test]
async fn fetch_paper_by_doi_returns_some_on_hit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works/doi:10.1214/aos/1176344136"))
        .and(query_param("api_key", "oa-key-123"))
        .and(query_param("mailto", "me@example.org"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "https://openalex.org/W2146855196",
            "title": "Estimating the Dimension of a Model",
            "publication_year": 1978,
            "doi": "https://doi.org/10.1214/aos/1176344136",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let paper = adapter(&server)
        .fetch_paper_by_doi("10.1214/aos/1176344136")
        .await
        .expect("DOI lookup should not error")
        .expect("known DOI should resolve");
    assert_eq!(paper.title, "Estimating the Dimension of a Model");
    assert_eq!(paper.year, Some(1978));
    assert_eq!(paper.doi.as_deref(), Some("10.1214/aos/1176344136"));
    assert_eq!(paper.openalex_id.as_deref(), Some("W2146855196"));
    // Canonical paper id = short OpenAlex id, same as work_to_paper.
    assert_eq!(paper.id.as_str(), "W2146855196");
}

#[tokio::test]
async fn fetch_paper_by_doi_normalises_url_prefixed_input() {
    let server = MockServer::start().await;
    // Regardless of "https://doi.org/…" or bare form, the path segment
    // MUST be the lowercase canonical DOI.
    Mock::given(method("GET"))
        .and(path("/works/doi:10.1038/test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "https://openalex.org/W555",
            "title": "Ok",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let paper = adapter(&server)
        .fetch_paper_by_doi("https://doi.org/10.1038/TEST")
        .await
        .expect("DOI lookup should succeed")
        .expect("known DOI should resolve");
    assert_eq!(paper.title, "Ok");
}

#[tokio::test]
async fn fetch_paper_by_doi_returns_none_on_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/works/doi:.*"))
        .respond_with(ResponseTemplate::new(404).set_body_string("<html>Not Found</html>"))
        .expect(1)
        .mount(&server)
        .await;

    let result = adapter(&server)
        .fetch_paper_by_doi("10.9999/xxx-nothing")
        .await
        .expect("404 must become Ok(None), not Err");
    assert!(result.is_none(), "404 for a well-formed DOI = not found");
}

#[tokio::test]
async fn fetch_paper_by_doi_rejects_a_malformed_doi_before_the_wire() {
    // No mocks mounted — the adapter MUST fail fast without an HTTP call.
    let server = MockServer::start().await;
    let err = adapter(&server)
        .fetch_paper_by_doi("not-a-doi")
        .await
        .expect_err("malformed DOI must be rejected");
    let msg = err.to_string();
    assert!(msg.contains("invalid DOI"), "{msg}");
    assert!(msg.contains("not-a-doi"), "{msg}");
}

#[tokio::test]
async fn title_search_sanitises_openalex_filter_metachars_before_the_wire() {
    // Regression for the #210 review blocker: OpenAlex's `filter=` syntax
    // treats `,` as filter separator (HTTP 400 unescaped), `|` as OR,
    // `!` as NOT and `"` as phrase boundary. Any of those in a title
    // must reach the wire as a space so the value stays a searchable
    // string. Live probe (2026-09): raw comma → HTTP 400
    // "A filter value contains an unescaped comma"; sanitised value →
    // HTTP 200 with Efron 1979 at rank 1.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        // The value the mock asserts is the SANITISED one — no comma,
        // no pipe, no bang, no quote.
        .and(query_param(
            "filter",
            "title.search:Bootstrap methods another look at the jackknife",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&[
            "Bootstrap Methods: Another Look at the Jackknife",
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let results = adapter(&server)
        .search_field(
            "Bootstrap methods, another look at the jackknife",
            5,
            SearchField::Title,
        )
        .await
        .expect("comma-bearing title must not blow up");
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn title_search_sanitises_pipe_bang_and_quote_before_the_wire() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("filter", "title.search:foo bar baz qux"))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&["Ok"])))
        .expect(1)
        .mount(&server)
        .await;

    let results = adapter(&server)
        .search_field(r#"foo|bar !baz "qux""#, 5, SearchField::Title)
        .await
        .expect("metachar-only query must still hit the wire cleanly");
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn auto_search_falls_through_to_broad_when_title_leg_errors() {
    // Review requirement (#210): "cannot lose" — a title-leg failure
    // must not blank a valid federated search. This models the 400 the
    // wire returned before the sanitiser landed, and any future
    // API-side syntax quirk we can't anticipate.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param(
            "filter",
            "title.search:Bootstrap methods another look at the jackknife",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":"Invalid request","message":"synthetic upstream failure for test"}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param(
            "search",
            "Bootstrap methods, another look at the jackknife",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(works_page(&["Broad hit"])))
        .expect(1)
        .mount(&server)
        .await;

    let results = adapter(&server)
        .search_field(
            "Bootstrap methods, another look at the jackknife",
            5,
            SearchField::Auto,
        )
        .await
        .expect("Auto must survive a title-leg error");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Broad hit");
}

#[tokio::test]
async fn auto_search_propagates_error_when_both_legs_fail() {
    // The other side of the fault-tolerance coin: if title AND broad
    // both fail, the search-run record must carry a real error rather
    // than a silent Ok(vec![]).
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("filter", "title.search:q"))
        .respond_with(ResponseTemplate::new(400).set_body_string("title fail"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works"))
        .and(query_param("search", "q"))
        .respond_with(ResponseTemplate::new(429).set_body_raw(BUDGET_EXHAUSTED, "application/json"))
        .expect(1)
        .mount(&server)
        .await;

    let err = adapter(&server)
        .search_field("q", 5, SearchField::Auto)
        .await
        .expect_err("both legs failing must surface, not silently pass");
    assert!(err.to_string().contains("429"), "{err}");
}

#[tokio::test]
async fn fetch_paper_by_doi_surfaces_a_500_as_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/works/doi:.*"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;
    let err = adapter(&server)
        .fetch_paper_by_doi("10.1214/aos/1176344136")
        .await
        .expect_err("a 500 must not read as not-found");
    assert!(err.to_string().contains("500"), "{err}");
}
