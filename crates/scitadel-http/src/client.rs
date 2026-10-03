//! `PacedClient`: the only way scitadel fetches a publisher's bytes.
//!
//! ADR-007 §4. Three rules make this a `PacedClient` and not a `reqwest`
//! wrapper:
//!
//! 1. **Redirects are followed here, not by the transport.** The transport
//!    runs with `redirect::Policy::none()` because reqwest would otherwise
//!    follow a chain without ever consulting the pacer — a five-hop redirect
//!    would spend one budget of one platform's allowance and hit four others
//!    for free. Every hop therefore takes **one `Request` permit, keyed to
//!    that hop's own bucket**.
//! 2. **A permit is spent when it is granted**, not when the fetch succeeds,
//!    and the sleep until `not_before` happens outside any ledger
//!    transaction.
//! 3. **Buckets are platforms, not hosts** — see [`crate::policy`].

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use scitadel_core::ports::{Bucket, Cost, PaceTier, Pacer};
use url::Url;

use crate::error::{FetchError, Result};
use crate::headers::SafeHeaders;
use crate::policy::BucketPolicyTable;
use crate::redirect::{is_login_redirect, is_rate_limited, redirect_target, retry_after_ms};

/// Redirect hops one `get` may follow before it gives up.
///
/// A legitimate acquisition chain is one or two hops (DOI → landing page →
/// PDF). Five leaves room for a resolver and an access gateway without leaving
/// room for a loop, which is the whole failure mode a hop limit exists for.
pub const MAX_REDIRECT_HOPS: usize = 5;

/// An HTTP client that cannot spend a permit it did not ask for.
pub struct PacedClient {
    /// Built with `redirect::Policy::none()` — see [`Self::default_transport`].
    transport: reqwest::Client,
    pacer: Arc<dyn Pacer>,
    policy: BucketPolicyTable,
}

impl PacedClient {
    /// Wrap a transport, a pacer and a bucket policy table.
    ///
    /// `transport` **must** have been built with
    /// `reqwest::redirect::Policy::none()`; [`Self::default_transport`] does
    /// that. reqwest exposes no getter for a client's redirect policy, so
    /// this cannot be checked at runtime — it is a construction contract, and
    /// a client that follows redirects behind this one's back silently
    /// defeats the one `Request` permit per hop rule.
    pub fn new(
        transport: reqwest::Client,
        pacer: Arc<dyn Pacer>,
        policy: BucketPolicyTable,
    ) -> Self {
        Self {
            transport,
            pacer,
            policy,
        }
    }

    /// A `PacedClient` on a transport built the way this type requires.
    pub fn new_default(pacer: Arc<dyn Pacer>, policy: BucketPolicyTable) -> Result<Self> {
        Ok(Self::new(Self::default_transport()?, pacer, policy))
    }

    /// [`Self::new_default`], with a per-request timeout.
    ///
    /// Without one, a publisher that accepts a connection and then stops
    /// writing holds the fetch open indefinitely — which, for an acquisition
    /// chain that walks four routes per work, is the difference between a
    /// stuck campaign and a slow one. The timeout is on the transport because
    /// that is the only place reqwest can enforce it, and this crate is the
    /// only crate allowed to name `reqwest::Client` (ADR-007 §3), so a caller
    /// that needs one must ask for it here rather than build a bare client
    /// beside this one.
    pub fn with_timeout(
        timeout: Duration,
        pacer: Arc<dyn Pacer>,
        policy: BucketPolicyTable,
    ) -> Result<Self> {
        Ok(Self::new(Self::transport(Some(timeout))?, pacer, policy))
    }

    /// The transport `PacedClient` expects: redirects disabled, because
    /// following them is this type's job.
    pub fn default_transport() -> Result<reqwest::Client> {
        Self::transport(None)
    }

    /// The transport contract, with an optional per-request timeout.
    fn transport(timeout: Option<Duration>) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("scitadel-http/", env!("CARGO_PKG_VERSION")));
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        builder
            .build()
            .map_err(|source| FetchError::ClientBuild { source })
    }

    /// The bucket policy table in force.
    pub fn policy(&self) -> &BucketPolicyTable {
        &self.policy
    }

    /// The bucket a URL would be charged to, without fetching it.
    pub fn bucket_for(&self, url: &Url) -> Bucket {
        self.policy.bucket_for(url)
    }

    /// Fetch `url`, following redirects under the pacer.
    ///
    /// Spends one `Request` permit per hop, plus one `Work` permit on the
    /// first hop into each bucket. A `Work` permit is scoped to this call,
    /// which is one resource of one work; fetching several resources of the
    /// same work should share a [`WorkScope`] so the platform is charged once
    /// — see [`Self::get_in_work`].
    pub async fn get(
        &self,
        url: Url,
        tier: PaceTier,
        headers: SafeHeaders,
    ) -> Result<PacedResponse> {
        self.get_in_work(&WorkScope::new(), url, tier, headers)
            .await
    }

    /// [`Self::get`], with the work-scope the caller keeps for one work.
    ///
    /// Landing page and PDF of the same paper are two `get` calls for one
    /// work. Passing the same `WorkScope` to both charges the `Work` permit
    /// once per bucket across the two, which is what ADR-007 §4 means by "one
    /// `Work` grant per work per bucket"; passing two fresh scopes would
    /// charge it twice.
    pub async fn get_in_work(
        &self,
        work: &WorkScope,
        url: Url,
        tier: PaceTier,
        headers: SafeHeaders,
    ) -> Result<PacedResponse> {
        let requested_url = url.clone();
        let mut current = url;
        let mut hops: usize = 0;
        let mut visited: Vec<Bucket> = Vec::new();

        loop {
            // Decided before anything is spent: a chain that is already on a
            // login page must not buy a permit to find that out, and must not
            // spend the login host's budget either.
            if is_login_redirect(&current) {
                return Err(FetchError::LoginRedirect {
                    url: crate::error::redact_url(current.as_ref()),
                });
            }

            let route = self.policy.resolve(&current);
            let bucket = route.bucket;
            visited.push(bucket.clone());

            // ADR-007 §5: a key goes only to its own publisher's bucket. This
            // runs per hop, so a redirect off-platform is refused rather than
            // answered with the credential.
            headers.check_scope(&bucket)?;

            // One Request permit per hop, keyed to this hop's bucket. A
            // redirect chain A -> 302 -> B spends two of them, one from A's
            // bucket and one from B's.
            let permit = self.pacer.acquire(&bucket, tier, Cost::Request).await?;
            sleep_until(permit.not_before).await;

            // One Work permit on the first hop into this bucket for this work.
            if work.charge(&bucket) {
                let work_permit = self.pacer.acquire(&bucket, tier, Cost::Work).await?;
                sleep_until(work_permit.not_before).await;
            }

            let response = self
                .transport
                .get(current.clone())
                .headers(headers.map().clone())
                .send()
                .await
                .map_err(|source| FetchError::Transport {
                    url: crate::error::redact_url(current.as_ref()),
                    source: crate::error::TransportSource::new(source),
                })?;

            let status = response.status();

            if let Some(location) = redirect_target(status, response.headers()) {
                hops += 1;
                if hops > MAX_REDIRECT_HOPS {
                    return Err(FetchError::TooManyRedirects {
                        hops: hops - 1,
                        url: crate::error::redact_url(requested_url.as_ref()),
                    });
                }
                let next = resolve_redirect(&current, location)?;
                if !matches!(next.scheme(), "http" | "https") {
                    return Err(FetchError::RedirectRefused {
                        from: current.to_string(),
                        to: next.to_string(),
                        reason: "only http and https are followed",
                    });
                }
                current = next;
                continue;
            }

            if is_rate_limited(status) {
                return Err(FetchError::RateLimited {
                    bucket: bucket.to_string(),
                    code: status.as_u16(),
                    retry_after_ms: retry_after_ms(response.headers()),
                });
            }
            if !status.is_success() {
                return Err(FetchError::Status {
                    code: status.as_u16(),
                    url: crate::error::redact_url(current.as_ref()),
                });
            }

            return Ok(PacedResponse {
                status,
                url: current,
                requested_url,
                bucket,
                hops,
                buckets: visited,
                headers: response.headers().clone(),
                response,
            });
        }
    }
}

/// Sleep until a permit's `not_before`.
///
/// ADR-007 §4 step 5: the wait happens *outside* the ledger transaction. A
/// permit already in the future is normal, not an error, and an `Instant` in
/// the past must not turn into a panic.
async fn sleep_until(deadline: Instant) {
    // A deadline already in the past means "no wait", not "a negative sleep".
    let wait = deadline
        .checked_duration_since(Instant::now())
        .unwrap_or_default();
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

/// Resolve a `Location` against the URL that produced it.
fn resolve_redirect(base: &Url, location: &HeaderValue) -> Result<Url> {
    let text = location.to_str().map_err(|_| FetchError::BadRedirect {
        url: base.to_string(),
        detail: "Location is not ASCII text".to_string(),
    })?;
    base.join(text).map_err(|e| FetchError::BadRedirect {
        url: base.to_string(),
        detail: e.to_string(),
    })
}

/// Which buckets one work has already been charged a `Work` permit for.
///
/// Cloning shares the state, so the landing-page request and the PDF request
/// of one paper can be issued concurrently and still be charged once.
#[derive(Clone, Debug, Default)]
pub struct WorkScope {
    charged: Arc<Mutex<Vec<Bucket>>>,
}

impl WorkScope {
    /// A fresh scope: nothing charged yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `bucket` as charged for this work, reporting whether this call
    /// was the one that charged it.
    fn charge(&self, bucket: &Bucket) -> bool {
        let mut charged = self.charged.lock().unwrap_or_else(PoisonError::into_inner);
        if charged.contains(bucket) {
            return false;
        }
        charged.push(bucket.clone());
        true
    }

    /// The buckets charged so far, in first-visit order.
    pub fn charged(&self) -> Vec<Bucket> {
        self.charged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A successful paced fetch.
#[derive(Debug)]
pub struct PacedResponse {
    /// Always a success status; anything else is a [`FetchError::Status`].
    pub status: StatusCode,
    /// The URL that produced this response, after redirects.
    pub url: Url,
    /// The URL the caller asked for.
    pub requested_url: Url,
    /// The bucket the final hop was charged to.
    pub bucket: Bucket,
    /// Redirects followed to get here.
    pub hops: usize,
    /// One entry per hop, in order. More than one entry means the chain left
    /// the platform it started on, or at least crossed a host boundary.
    pub buckets: Vec<Bucket>,
    /// The final response's headers.
    pub headers: HeaderMap,
    response: reqwest::Response,
}

impl PacedResponse {
    /// The response's `Content-Type`, if it had one.
    pub fn content_type(&self) -> Option<&str> {
        self.headers
            .get(reqwest::header::CONTENT_TYPE)?
            .to_str()
            .ok()
    }

    /// The final `Content-Length`, if the publisher declared one.
    pub fn content_length(&self) -> Option<u64> {
        self.headers
            .get(reqwest::header::CONTENT_LENGTH)?
            .to_str()
            .ok()?
            .parse()
            .ok()
    }

    /// Read the body.
    pub async fn bytes(self) -> Result<Vec<u8>> {
        let url = self.url.to_string();
        self.response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|source| FetchError::Transport {
                url: crate::error::redact_url(url.as_ref()),
                source: crate::error::TransportSource::new(source),
            })
    }

    /// Read the body as UTF-8 text.
    pub async fn text(self) -> Result<String> {
        let url = self.url.to_string();
        self.response
            .text()
            .await
            .map_err(|source| FetchError::Transport {
                url: crate::error::redact_url(url.as_ref()),
                source: crate::error::TransportSource::new(source),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use scitadel_core::ports::{BucketPolicy, PaceDenied, Permit};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    // ---------- a hand-written pacer, so the accounting is observable ----------

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Grant {
        bucket: Bucket,
        tier: PaceTier,
        cost: Cost,
    }

    /// Records every acquisition and grants it immediately. No SQLite ledger:
    /// these tests are about which permits `PacedClient` *asks for*, not about
    /// what a ledger does with them.
    #[derive(Debug, Default)]
    struct FakePacer {
        log: Mutex<Vec<Grant>>,
        deny: Option<PaceDenied>,
        delay_ms: u64,
    }

    impl FakePacer {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn denying(denied: PaceDenied) -> Arc<Self> {
            Arc::new(Self {
                deny: Some(denied),
                ..Self::default()
            })
        }

        fn delaying(delay_ms: u64) -> Arc<Self> {
            Arc::new(Self {
                delay_ms,
                ..Self::default()
            })
        }

        fn grants(&self) -> Vec<Grant> {
            self.log.lock().expect("uncontended").clone()
        }

        fn spending(&self, cost: Cost) -> Vec<Grant> {
            self.grants()
                .into_iter()
                .filter(|grant| grant.cost == cost)
                .collect()
        }
    }

    #[async_trait]
    impl Pacer for FakePacer {
        async fn acquire(
            &self,
            bucket: &Bucket,
            tier: PaceTier,
            cost: Cost,
        ) -> std::result::Result<Permit, PaceDenied> {
            self.log.lock().expect("uncontended").push(Grant {
                bucket: bucket.clone(),
                tier,
                cost,
            });
            if let Some(denied) = self.deny {
                return Err(denied);
            }
            Ok(Permit {
                bucket: bucket.clone(),
                tier,
                not_before: Instant::now() + Duration::from_millis(self.delay_ms),
            })
        }
    }

    // ---------- fixtures ----------

    fn url(s: &str) -> Url {
        Url::parse(s).expect("test URL parses")
    }

    /// `host:port` of a mock server, which is how the policy table keys it.
    fn endpoint(server: &MockServer) -> String {
        server.uri().trim_start_matches("http://").to_string()
    }

    fn client(pacer: Arc<dyn Pacer>, policy: BucketPolicyTable) -> PacedClient {
        PacedClient::new_default(pacer, policy).expect("transport builds")
    }

    async fn redirecting(from: &MockServer, to_uri: &str, from_path: &str) {
        Mock::given(method("GET"))
            .and(path(from_path))
            .respond_with(ResponseTemplate::new(302).insert_header("location", to_uri))
            .mount(from)
            .await;
    }

    // ---------- the ADR's headline rule: one Request permit per hop ----------

    /// ADR-007 §4: "PacedClient follows redirects itself and takes one
    /// Request permit per hop, keyed to that hop's bucket."
    ///
    /// This is the test that would fail if the transport followed redirects
    /// itself: the chain A -> 302 -> B would then cost one permit and touch
    /// two platforms.
    #[tokio::test]
    async fn a_redirect_chain_spends_one_request_per_hop_across_two_buckets() {
        let a = MockServer::start().await;
        let b = MockServer::start().await;
        redirecting(&a, &format!("{}/paper.pdf", b.uri()), "/start").await;
        Mock::given(method("GET"))
            .and(path("/paper.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_string("PDF-BYTES"))
            .mount(&b)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let response = client
            .get(
                url(&format!("{}/start", a.uri())),
                PaceTier::Tdm,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect("the chain should resolve");

        // Two hops, two hosts, therefore two buckets on two ports.
        let requests = pacer.spending(Cost::Request);
        assert_eq!(requests.len(), 2, "one Request permit per hop, not per get");
        assert_ne!(
            requests[0].bucket, requests[1].bucket,
            "each hop must be keyed to its own bucket"
        );
        assert_eq!(requests[0].bucket.as_str(), endpoint(&a));
        assert_eq!(requests[1].bucket.as_str(), endpoint(&b));
        assert!(requests.iter().all(|g| g.tier == PaceTier::Tdm));

        // The Work permit is charged once per bucket, so two buckets means two.
        let works = pacer.spending(Cost::Work);
        assert_eq!(works.len(), 2, "one Work permit per work *per bucket*");
        assert_eq!(works[0].bucket.as_str(), endpoint(&a));
        assert_eq!(works[1].bucket.as_str(), endpoint(&b));

        // And the accounting is in order: Request, then Work, then the next hop.
        assert_eq!(
            pacer.grants().iter().map(|g| g.cost).collect::<Vec<_>>(),
            vec![Cost::Request, Cost::Work, Cost::Request, Cost::Work],
            "each hop charges its own bucket: Request, then Work on arrival"
        );

        assert_eq!(response.hops, 1);
        assert_eq!(response.buckets.len(), 2);
        assert_eq!(response.url.as_str(), format!("{}/paper.pdf", b.uri()));
        assert_eq!(
            response.requested_url.as_str(),
            format!("{}/start", a.uri())
        );
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.text().await.expect("body"), "PDF-BYTES");
        assert_eq!(a.received_requests().await.expect("recorded").len(), 1);
    }

    /// Both hops into the *same* platform: still two `Request` permits, but
    /// only one `Work` permit. This is the Elsevier case — a chain inside one
    /// bucket must not buy itself a second work grant.
    #[tokio::test]
    async fn two_hops_into_the_same_platform_bucket_spend_two_requests_and_one_work() {
        let a = MockServer::start().await;
        let b = MockServer::start().await;

        let mut table = BucketPolicyTable::new();
        table
            .route(&endpoint(&a), "elsevier")
            .route(&endpoint(&b), "elsevier");
        assert_eq!(
            table.policy_for(&Bucket("elsevier".into())),
            BucketPolicy::new(10_000, 300, 100),
            "both hosts must spend the ADR's publisher-platform budget"
        );

        redirecting(&a, &format!("{}/landing", b.uri()), "/article").await;
        Mock::given(method("GET"))
            .and(path("/landing"))
            .respond_with(ResponseTemplate::new(200).set_body_string("LANDING"))
            .mount(&b)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), table);
        let response = client
            .get(
                url(&format!("{}/article", a.uri())),
                PaceTier::Tdm,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect("the chain should resolve");

        let requests = pacer.spending(Cost::Request);
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|g| g.bucket == Bucket("elsevier".into()))
        );
        let works = pacer.spending(Cost::Work);
        assert_eq!(works.len(), 1, "the Work grant is per work per bucket");
        assert_eq!(works[0].bucket, Bucket("elsevier".into()));
        assert_eq!(response.buckets, vec![Bucket("elsevier".into()); 2]);
    }

    /// A `WorkScope` shared across two fetches of one work charges the work
    /// once; two scopes charge it twice. Both halves, because either alone
    /// would be a plausible-looking bug.
    #[tokio::test]
    async fn one_work_scope_charges_a_work_permit_once_across_two_fetches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/landing"))
            .respond_with(ResponseTemplate::new(200).set_body_string("LANDING"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/paper.pdf"))
            .respond_with(ResponseTemplate::new(200).set_body_string("PDF"))
            .mount(&server)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());

        let shared = WorkScope::new();
        for path in ["/landing", "/paper.pdf"] {
            client
                .get_in_work(
                    &shared,
                    url(&format!("{}{path}", server.uri())),
                    PaceTier::Tdm,
                    SafeHeaders::unauthenticated(),
                )
                .await
                .expect("fetch should succeed")
                .bytes()
                .await
                .expect("body");
        }
        assert_eq!(pacer.spending(Cost::Work).len(), 1, "shared scope");
        assert_eq!(pacer.spending(Cost::Request).len(), 2, "two hops");
        assert_eq!(shared.charged().len(), 1, "one bucket entered");

        // A fresh scope per fetch is what a caller that forgets to thread the
        // scope ends up doing: the work is charged every time.
        for path in ["/landing", "/paper.pdf"] {
            client
                .get_in_work(
                    &WorkScope::new(),
                    url(&format!("{}{path}", server.uri())),
                    PaceTier::Tdm,
                    SafeHeaders::unauthenticated(),
                )
                .await
                .expect("fetch should succeed")
                .bytes()
                .await
                .expect("body");
        }
        assert_eq!(pacer.spending(Cost::Work).len(), 3, "1 shared + 2 unscoped");
    }

    // ---------- refusal paths ----------

    /// ADR-007 §4: a redirect to a login page becomes `LoginRedirect`, which
    /// records `needs_login`. The point of checking before the next permit is
    /// that the login host's budget is never spent to learn the answer.
    #[tokio::test]
    async fn a_redirect_to_a_login_page_is_refused_without_spending_its_bucket() {
        let a = MockServer::start().await;
        redirecting(
            &a,
            "https://login.ezproxy.example.org/SignIn?redir=/x",
            "/article",
        )
        .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url(&format!("{}/article", a.uri())),
                PaceTier::Oa,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a login page is not a fetchable body");

        assert!(err.needs_login(), "{err}");
        let FetchError::LoginRedirect { url } = &err else {
            panic!("expected LoginRedirect, got {err:?}");
        };
        assert!(url.contains("login.ezproxy.example.org"), "{url}");
        let grants = pacer.grants();
        assert_eq!(
            grants.len(),
            2,
            "only the first hop buys permits (one Request, one Work): {grants:?}"
        );
        assert!(
            grants.iter().all(|g| g.bucket.as_str() == endpoint(&a)),
            "the login host's budget must not be spent: {grants:?}"
        );
    }

    /// A direct fetch of a login URL is refused with nothing spent at all.
    #[tokio::test]
    async fn a_login_url_is_refused_before_the_first_permit() {
        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url("https://idp.example.ac.uk/idp/profile/Login/1"),
                PaceTier::Session,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a login page is not a fetchable body");
        assert!(err.needs_login(), "{err}");
        assert!(pacer.grants().is_empty(), "{:?}", pacer.grants());
    }

    #[tokio::test]
    async fn a_redirect_loop_gives_up_and_reports_where_it_started() {
        let server = MockServer::start().await;
        redirecting(&server, &format!("{}/loop", server.uri()), "/loop").await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url(&format!("{}/loop", server.uri())),
                PaceTier::Oa,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a loop must not run forever");

        match &err {
            FetchError::TooManyRedirects { hops, url } => {
                assert_eq!(*hops, MAX_REDIRECT_HOPS);
                assert!(url.ends_with("/loop"), "{url}");
            }
            other => panic!("expected TooManyRedirects, got {other:?}"),
        }
        assert!(
            !err.is_rate_limited(),
            "a loop is not the publisher's fault"
        );
        assert_eq!(
            server.received_requests().await.expect("recorded").len(),
            MAX_REDIRECT_HOPS + 1,
            "MAX hops are followed, and the refusal happens on the next one"
        );
        assert_eq!(pacer.spending(Cost::Request).len(), MAX_REDIRECT_HOPS + 1);
        assert_eq!(pacer.spending(Cost::Work).len(), 1, "one bucket, one work");
    }

    /// 429 and 503 are the two answers that mean "come back later", and the
    /// caller needs the platform to back off.
    #[tokio::test]
    async fn a_429_is_a_rate_limit_and_carries_the_retry_hint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/busy"))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_body_string("slow down")
                    .insert_header("retry-after", "30"),
            )
            .mount(&server)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url(&format!("{}/busy", server.uri())),
                PaceTier::Meta,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a 429 is not a body");

        assert!(err.is_rate_limited(), "{err}");
        assert!(err.to_string().contains("429"), "{err}");
        assert!(err.to_string().contains("HTTP"), "{err}");
        let hint = err.retry_at_ms().expect("Retry-After should be surfaced");
        let delta = hint - crate::redirect::epoch_ms();
        assert!(
            (29_000..=31_000).contains(&delta),
            "unexpected retry hint {delta}ms"
        );
    }

    #[tokio::test]
    async fn a_503_is_a_rate_limit_too() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/maintenance"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url(&format!("{}/maintenance", server.uri())),
                PaceTier::Meta,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a 503 is not a body");
        assert!(err.is_rate_limited(), "{err}");
        assert!(err.to_string().contains("503"), "{err}");
    }

    /// A 403 is not a rate limit and not a login: it is a status.
    #[tokio::test]
    async fn a_403_is_a_status_not_a_rate_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/denied"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url(&format!("{}/denied", server.uri())),
                PaceTier::Tdm,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a 403 is not a body");
        assert!(!err.is_rate_limited(), "{err}");
        assert!(!err.needs_login(), "{err}");
        assert!(err.to_string().contains("403"), "{err}");
    }

    /// A pacing refusal happens before anything goes on the wire, and it keeps
    /// its own identity: a daily cap is not a transport failure.
    #[tokio::test]
    async fn a_pacing_denial_is_reported_as_a_pacing_denial() {
        let server = MockServer::start().await; // no mocks: any request would 404
        let pacer = FakePacer::denying(PaceDenied::DailyCap);
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url(&format!("{}/anything", server.uri())),
                PaceTier::Meta,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a denied pacing request cannot succeed");

        assert!(
            matches!(err, FetchError::Pace(PaceDenied::DailyCap)),
            "{err:?}"
        );
        assert!(err.is_rate_limited(), "a cap means try later");
        assert_eq!(err.retry_at_ms(), None, "a cap has no retry instant");
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded")
                .is_empty()
        );
        assert_eq!(pacer.grants().len(), 1, "the refusal is one attempt");
    }

    #[tokio::test]
    async fn a_ledger_failure_is_not_disguised_as_rate_limiting() {
        let pacer = FakePacer::denying(PaceDenied::LedgerBusy);
        let client = client(pacer, BucketPolicyTable::new());
        let err = client
            .get(
                url("https://api.openalex.org/works"),
                PaceTier::Meta,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a busy ledger fails closed");
        assert!(
            matches!(err, FetchError::Pace(PaceDenied::LedgerBusy)),
            "{err:?}"
        );
        assert!(!err.is_rate_limited(), "{err}");
        assert!(err.to_string().contains("closed"), "{err}");
    }

    /// ADR-007 §4 step 5: the wait happens outside the transaction, and a
    /// permit in the future really is waited out.
    #[tokio::test]
    async fn the_client_waits_for_a_permit_in_the_future() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_body_string("OK"))
            .mount(&server)
            .await;

        let pacer = FakePacer::delaying(40);
        let client = client(pacer, BucketPolicyTable::new());
        let started = Instant::now();
        client
            .get(
                url(&format!("{}/slow", server.uri())),
                PaceTier::Oa,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect("fetch should succeed");
        let elapsed = started.elapsed();
        // One Request permit plus one Work permit, each 40 ms in the future.
        assert!(
            elapsed >= Duration::from_millis(80),
            "both permits must be waited out, took {elapsed:?}"
        );
    }

    // ---------- credential scoping (ADR-007 §5) ----------

    /// "A key is attached only to requests in its own publisher's bucket, and
    /// those requests don't follow redirects to other buckets."
    #[tokio::test]
    async fn a_credential_reaches_its_own_bucket_and_stops_at_the_next_one() {
        let a = MockServer::start().await;
        let b = MockServer::start().await;

        let mut table = BucketPolicyTable::new();
        table
            .route(&endpoint(&a), "elsevier")
            .route(&endpoint(&b), "elsevier-cdn");
        redirecting(&a, &format!("{}/paper.pdf", b.uri()), "/article").await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), table);
        let mut headers = SafeHeaders::authenticated("elsevier");
        headers
            .set_authorization("Bearer tdm-key-9f3a")
            .expect("scoped credential");

        let err = client
            .get(url(&format!("{}/article", a.uri())), PaceTier::Tdm, headers)
            .await
            .expect_err("the second bucket must not get the key");

        assert!(
            matches!(err, FetchError::CredentialOutOfScope { .. }),
            "{err:?}"
        );
        assert!(err.to_string().contains("elsevier"), "{err}");
        assert!(
            !err.to_string().contains("tdm-key-9f3a"),
            "the key must never reach an error string: {err}"
        );

        // The first hop did carry it.
        let received = a.received_requests().await.expect("recorded");
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0]
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer tdm-key-9f3a"),
            "the key belongs on its own platform's request"
        );

        // The second bucket was never even asked for a permit.
        assert_eq!(
            pacer.spending(Cost::Request).len(),
            1,
            "{:?}",
            pacer.grants()
        );
        assert!(b.received_requests().await.expect("recorded").is_empty());
    }

    /// An anonymous request to another bucket is just a normal fetch.
    #[tokio::test]
    async fn an_anonymous_request_crosses_buckets_freely() {
        let a = MockServer::start().await;
        let b = MockServer::start().await;
        redirecting(&a, &format!("{}/landing", b.uri()), "/article").await;
        Mock::given(method("GET"))
            .and(path("/landing"))
            .respond_with(ResponseTemplate::new(200).set_body_string("LANDING"))
            .mount(&b)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let response = client
            .get(
                url(&format!("{}/article", a.uri())),
                PaceTier::Oa,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect("an anonymous chain should resolve");
        assert_eq!(response.hops, 1);
        assert_eq!(pacer.spending(Cost::Request).len(), 2);
    }

    // ---------- transport hardening ----------

    /// Only http and https are followed. A `Location` pointing at the local
    /// filesystem is a refused redirect, not a fetched file.
    #[tokio::test]
    async fn a_non_http_redirect_is_refused() {
        let server = MockServer::start().await;
        redirecting(&server, "file:///etc/passwd", "/download").await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let err = client
            .get(
                url(&format!("{}/download", server.uri())),
                PaceTier::Tdm,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("file:// must not be followed");
        match err {
            FetchError::RedirectRefused { reason, .. } => {
                assert!(reason.contains("http"), "{reason}");
            }
            other => panic!("expected RedirectRefused, got {other:?}"),
        }
    }

    /// A relative `Location` resolves against the URL that produced it.
    #[tokio::test]
    async fn a_relative_location_resolves_against_the_current_url() {
        let server = MockServer::start().await;
        redirecting(&server, "../../v2/pii/S000?x=1", "/article/pii/S000").await;
        Mock::given(method("GET"))
            .and(path("/v2/pii/S000"))
            .respond_with(ResponseTemplate::new(200).set_body_string("V2"))
            .mount(&server)
            .await;

        let pacer = FakePacer::new();
        let client = client(pacer.clone(), BucketPolicyTable::new());
        let response = client
            .get(
                url(&format!("{}/article/pii/S000", server.uri())),
                PaceTier::Tdm,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect("a relative redirect should resolve");
        assert_eq!(response.hops, 1);
        assert!(
            response.url.as_str().ends_with("/v2/pii/S000?x=1"),
            "{response:?}"
        );
        assert_eq!(response.content_type(), Some("text/plain"));
        assert_eq!(response.content_length(), Some(2), "body is \"V2\"");
    }

    /// A transport-level failure must not look like a publisher verdict.
    #[tokio::test]
    async fn an_unreachable_host_is_a_transport_error() {
        let client = client(FakePacer::new(), BucketPolicyTable::new());
        let err = client
            .get(
                // Port 1 on the loopback interface: nothing listens there.
                url("http://127.0.0.1:1/nothing"),
                PaceTier::Meta,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("a refused connection is not a body");
        assert!(matches!(err, FetchError::Transport { .. }), "{err:?}");
        assert!(err.to_string().contains("127.0.0.1:1"), "{err}");
        assert!(!err.is_rate_limited(), "{err}");
    }

    /// #276 end to end: a credential in the query string must not survive
    /// into a `Transport` error.
    ///
    /// This is the leak that redacting `{url}` alone does not close, because
    /// `reqwest::Error`'s own `Display` embeds the request URL — so an error
    /// that rendered `{source}` would hand the key back. The assertion is on
    /// the rendered string, which is what reaches the TUI's task panel and an
    /// MCP agent's return value.
    #[tokio::test]
    async fn a_credential_in_the_query_never_reaches_a_transport_error() {
        let client = client(FakePacer::new(), BucketPolicyTable::new());
        let secret = "sk-live-SUPERSECRET";

        let err = client
            .get(
                // Port 1 on loopback: nothing listens, so this is a real
                // connection refusal rather than a stubbed error.
                url(&format!(
                    "http://127.0.0.1:1/works?api_key={secret}&mailto=me@example.org"
                )),
                PaceTier::Meta,
                SafeHeaders::unauthenticated(),
            )
            .await
            .expect_err("nothing is listening on port 1");

        assert!(matches!(err, FetchError::Transport { .. }), "{err:?}");
        for rendered in [err.to_string(), format!("{err:?}")] {
            assert!(
                !rendered.contains(secret),
                "the credential reached a rendered error: {rendered}"
            );
            assert!(
                !rendered.contains("api_key=sk-live"),
                "the parameter survived unredacted: {rendered}"
            );
        }
        // Redaction must not cost the information that makes the error useful.
        assert!(err.to_string().contains("api_key=REDACTED"), "{err}");
        assert!(err.to_string().contains("mailto=me%40example.org"), "{err}");
    }

    /// The redirect policy is the load-bearing part of the transport contract.
    /// reqwest exposes no getter, so this asserts the observable consequence
    /// instead: a 302 arrives at the client rather than being swallowed.
    #[tokio::test]
    async fn the_default_transport_does_not_follow_redirects() {
        let server = MockServer::start().await;
        redirecting(&server, &format!("{}/final", server.uri()), "/start").await;
        Mock::given(method("GET"))
            .and(path("/final"))
            .respond_with(ResponseTemplate::new(200).set_body_string("FINAL"))
            .mount(&server)
            .await;

        let raw = PacedClient::default_transport().expect("transport builds");
        let seen = raw
            .get(url(&format!("{}/start", server.uri())))
            .send()
            .await
            .expect("request");
        assert_eq!(
            seen.status(),
            StatusCode::FOUND,
            "the transport must hand 3xx back to PacedClient"
        );
        assert_eq!(server.received_requests().await.expect("recorded").len(), 1);
    }

    /// A publisher that accepts the connection and then goes quiet must not hold
    /// a fetch open forever. `new_default` has no timeout, so this asserts
    /// the *capability* callers need, and that a timeout arrives as a
    /// transport failure rather than as a publisher verdict.
    #[tokio::test]
    async fn a_request_timeout_is_enforced_and_reads_as_a_transport_error() {
        // A listener that accepts and then never writes: the shape of a
        // publisher that stops answering mid-handshake. A wiremock delay
        // would not do — it eventually answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let stalled = format!("http://{}/slow", listener.local_addr().expect("an addr"));
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket); // never written to, never dropped
            }
        });

        let impatient = PacedClient::with_timeout(
            Duration::from_millis(80),
            FakePacer::new(),
            BucketPolicyTable::new(),
        )
        .expect("transport builds");
        let err = impatient
            .get(url(&stalled), PaceTier::Oa, SafeHeaders::unauthenticated())
            .await
            .expect_err("a publisher that never answers must not hang the chain");
        assert!(matches!(err, FetchError::Transport { .. }), "{err:?}");
        assert!(
            !err.is_rate_limited(),
            "our own timeout is not the publisher's"
        );
        assert!(err.to_string().contains("/slow"), "{err}");

        // A generous timeout on the same stalled socket still waits, which is
        // what proves the 80 ms above was the timeout firing rather than a
        // connection that was refused.
        let patient = PacedClient::with_timeout(
            Duration::from_millis(400),
            FakePacer::new(),
            BucketPolicyTable::new(),
        )
        .expect("transport builds");
        let started = Instant::now();
        let err = patient
            .get(url(&stalled), PaceTier::Oa, SafeHeaders::unauthenticated())
            .await
            .expect_err("400ms is not enough for a socket that never writes");
        assert!(started.elapsed() >= Duration::from_millis(350), "{err:?}");
    }

    /// The policy table in force is the one the client was built with.
    #[tokio::test]
    async fn the_client_exposes_its_bucket_table() {
        let client = client(FakePacer::new(), BucketPolicyTable::new());
        assert_eq!(
            client.bucket_for(&url("https://api.crossref.org/works")),
            Bucket("crossref".into())
        );
        assert_eq!(
            client.policy().policy_for(&Bucket("elsevier".into())),
            BucketPolicy::new(10_000, 300, 100)
        );
    }

    /// A campaign fetches many works concurrently, so the future has to be
    /// `Send` and the client has to be `Sync`. Asserted at compile time: this
    /// test fails to *build* if either stops holding, rather than failing at
    /// some later `tokio::spawn` in another crate.
    #[test]
    fn the_fetch_future_is_send() {
        fn assert_send<T: Send>(_: T) {}
        fn assert_sync<T: Sync>(_: &T) {}

        let client = client(FakePacer::new(), BucketPolicyTable::new());
        assert_sync(&client);
        assert_send(client.get(
            url("https://pdf.sciencedirectassets.com/paper.pdf"),
            PaceTier::Tdm,
            SafeHeaders::unauthenticated(),
        ));
        assert_send(client.get_in_work(
            &WorkScope::new(),
            url("https://pdf.sciencedirectassets.com/paper.pdf"),
            PaceTier::Tdm,
            SafeHeaders::unauthenticated(),
        ));
    }
}
