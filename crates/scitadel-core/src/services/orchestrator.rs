use std::time::Instant;

use tracing::{info, warn};

use crate::error::CoreError;
use crate::models::{CandidatePaper, Search, SourceOutcome, SourceStatus};
use crate::ports::SourceAdapter;

/// Message recorded on a failed `SourceOutcome`.
///
/// `CoreError::Adapter` renders as `adapter error: <source> — <msg>`, but
/// the outcome already carries the source name, so the prefix is dropped:
/// what lands in the search record (and in `[!] openalex: …`) is the bare
/// `HTTP 429 Too Many Requests — Insufficient budget…` (#212).
fn outcome_error(e: &CoreError) -> String {
    match e {
        CoreError::Adapter(_, msg) => msg.clone(),
        other => other.to_string(),
    }
}

/// Run a single adapter with retry logic. Never panics.
async fn run_adapter(
    adapter: &dyn SourceAdapter,
    query: &str,
    max_results: usize,
    max_retries: u32,
) -> (Vec<CandidatePaper>, SourceOutcome) {
    let start = Instant::now();
    let mut last_error = String::new();

    for attempt in 0..max_retries {
        match adapter.search(query, max_results).await {
            Ok(candidates) => {
                let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                let count = candidates.len() as i32;
                info!(
                    source = adapter.name(),
                    count, elapsed_ms, "Adapter returned results"
                );
                return (
                    candidates,
                    SourceOutcome {
                        source: adapter.name().to_string(),
                        status: SourceStatus::Success,
                        result_count: count,
                        latency_ms: elapsed_ms,
                        error: None,
                    },
                );
            }
            Err(e) => {
                last_error = outcome_error(&e);
                warn!(
                    source = adapter.name(),
                    attempt = attempt + 1,
                    max_retries,
                    error = %last_error,
                    "Adapter attempt failed"
                );
                if attempt < max_retries - 1 {
                    let delay = 2f64.powi(attempt as i32) * rand_jitter();
                    tokio::time::sleep(std::time::Duration::from_secs_f64(delay)).await;
                }
            }
        }
    }

    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    (
        Vec::new(),
        SourceOutcome {
            source: adapter.name().to_string(),
            status: SourceStatus::Failed,
            result_count: 0,
            latency_ms: elapsed_ms,
            error: Some(last_error),
        },
    )
}

fn rand_jitter() -> f64 {
    // Simple jitter: 0.5 to 1.5
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    0.5 + (f64::from(nanos) / f64::from(u32::MAX))
}

/// Execute a federated search across all adapters in parallel.
///
/// Source failures do not abort the whole search.
pub async fn run_search(
    query: &str,
    adapters: &[Box<dyn SourceAdapter>],
    max_results: usize,
    max_retries: u32,
) -> (Search, Vec<CandidatePaper>) {
    let mut search = Search::new(query);
    search.sources = adapters.iter().map(|a| a.name().to_string()).collect();
    search.parameters = serde_json::json!({ "max_results": max_results });

    info!(
        search_id = search.id.short(),
        query,
        sources = ?search.sources,
        "Starting federated search"
    );

    // Run all adapters concurrently using join_all (no spawn needed)
    let futures: Vec<_> = adapters
        .iter()
        .map(|adapter| run_adapter(adapter.as_ref(), query, max_results, max_retries))
        .collect();

    let results = futures::future::join_all(futures).await;

    let mut all_candidates = Vec::new();
    let mut outcomes = Vec::new();

    for (candidates, outcome) in results {
        all_candidates.extend(candidates);
        outcomes.push(outcome);
    }

    search.source_outcomes = outcomes;
    search.total_candidates = all_candidates.len() as i32;

    info!(
        search_id = search.id.short(),
        total_candidates = all_candidates.len(),
        "Search complete"
    );

    (search, all_candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MockAdapter {
        name: String,
        results: Vec<CandidatePaper>,
        should_fail: bool,
        error: String,
    }

    #[async_trait]
    impl SourceAdapter for MockAdapter {
        fn name(&self) -> &str {
            &self.name
        }

        async fn search(
            &self,
            _query: &str,
            _max_results: usize,
        ) -> Result<Vec<CandidatePaper>, CoreError> {
            if self.should_fail {
                Err(CoreError::Adapter(self.name.clone(), self.error.clone()))
            } else {
                Ok(self.results.clone())
            }
        }
    }

    #[tokio::test]
    async fn test_run_search_success() {
        let adapters: Vec<Box<dyn SourceAdapter>> = vec![Box::new(MockAdapter {
            name: "mock".into(),
            results: vec![CandidatePaper::new("mock", "1", "Test Paper")],
            should_fail: false,
            error: String::new(),
        })];

        let (search, candidates) = run_search("test query", &adapters, 50, 1).await;
        assert_eq!(candidates.len(), 1);
        assert_eq!(search.source_outcomes.len(), 1);
        assert_eq!(search.source_outcomes[0].status, SourceStatus::Success);
    }

    #[tokio::test]
    async fn failure_outcome_carries_the_adapter_message_verbatim() {
        // #212: a 429 must survive into the search record as an error,
        // not be flattened into a zero-result success.
        let adapters: Vec<Box<dyn SourceAdapter>> = vec![Box::new(MockAdapter {
            name: "openalex".into(),
            results: vec![],
            should_fail: true,
            error: "HTTP 429 Too Many Requests — Insufficient budget.".into(),
        })];

        let (search, candidates) = run_search("test", &adapters, 50, 1).await;
        assert!(candidates.is_empty());

        let outcome = &search.source_outcomes[0];
        assert_eq!(outcome.status, SourceStatus::Failed);
        assert_eq!(outcome.result_count, 0);
        assert_eq!(
            outcome.error.as_deref(),
            Some("HTTP 429 Too Many Requests — Insufficient budget."),
            "the source prefix belongs in `outcome.source`, not the message"
        );
    }

    #[tokio::test]
    async fn a_failing_source_does_not_drop_the_others_results() {
        let adapters: Vec<Box<dyn SourceAdapter>> = vec![
            Box::new(MockAdapter {
                name: "openalex".into(),
                results: vec![],
                should_fail: true,
                error: "HTTP 429 Too Many Requests".into(),
            }),
            Box::new(MockAdapter {
                name: "arxiv".into(),
                results: vec![
                    CandidatePaper::new("arxiv", "1", "Paper A"),
                    CandidatePaper::new("arxiv", "2", "Paper B"),
                ],
                should_fail: false,
                error: String::new(),
            }),
        ];

        let (search, candidates) = run_search("test", &adapters, 50, 1).await;
        assert_eq!(candidates.len(), 2, "arxiv results must survive the 429");
        assert_eq!(search.total_candidates, 2);

        let failed: Vec<&str> = search
            .source_outcomes
            .iter()
            .filter(|o| o.status == SourceStatus::Failed)
            .map(|o| o.source.as_str())
            .collect();
        assert_eq!(failed, vec!["openalex"]);
    }

    #[tokio::test]
    async fn test_run_search_partial_failure() {
        let adapters: Vec<Box<dyn SourceAdapter>> = vec![
            Box::new(MockAdapter {
                name: "good".into(),
                results: vec![CandidatePaper::new("good", "1", "Paper A")],
                should_fail: false,
                error: String::new(),
            }),
            Box::new(MockAdapter {
                name: "bad".into(),
                results: vec![],
                should_fail: true,
                error: "mock failure".into(),
            }),
        ];

        let (search, candidates) = run_search("test", &adapters, 50, 1).await;
        assert_eq!(candidates.len(), 1);
        assert_eq!(search.source_outcomes.len(), 2);

        let good = search
            .source_outcomes
            .iter()
            .find(|o| o.source == "good")
            .unwrap();
        let bad = search
            .source_outcomes
            .iter()
            .find(|o| o.source == "bad")
            .unwrap();
        assert_eq!(good.status, SourceStatus::Success);
        assert_eq!(bad.status, SourceStatus::Failed);
    }
}
