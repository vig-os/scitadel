//! What a paced fetch can fail with.
//!
//! The variants are a report's vocabulary, not an implementation detail
//! (ADR-007 §3, §4). `needs_login`, `rate_limited` and `needs_ill` are
//! different outcomes with different recoveries, so collapsing them is how
//! "retry tomorrow" turns into "this campaign is broken" — the exact failure
//! `PaceDenied` exists to prevent on the pacer side.

use scitadel_core::ports::PaceDenied;
use thiserror::Error;
use url::Url;

pub type Result<T> = std::result::Result<T, FetchError>;

#[derive(Debug, Error)]
pub enum FetchError {
    /// A redirect landed on a login or SSO page.
    ///
    /// ADR-007 §4: this records `needs_login` and puts the bucket into
    /// backoff. It is *not* a paywall verdict — scitadel cannot tell, from
    /// a URL, whether a human could satisfy this login with the institution's
    /// credentials.
    #[error("redirected to a login page ({url}) — this work needs a human login")]
    LoginRedirect { url: String },

    /// 429 or 503. The publisher asked us to slow down, and the caller can
    /// put the bucket into backoff using [`FetchError::retry_at_ms`].
    #[error("rate limited by {bucket} (HTTP {code}) — the publisher asked us to slow down")]
    RateLimited {
        /// The platform that misbehaved, so the caller backs off the right
        /// bucket rather than the last URL it happened to touch.
        bucket: String,
        code: u16,
        /// From `Retry-After`, when the publisher sent a delta-seconds value.
        /// Epoch milliseconds. `None` means "no hint, use your own backoff".
        retry_after_ms: Option<i64>,
    },

    /// Any other response that is neither a success nor a redirect.
    #[error("HTTP {code} from {url}")]
    Status { code: u16, url: String },

    /// Connection refused, DNS failure, TLS failure, timeout.
    ///
    /// Carries the URL rather than the whole request — but that only helps
    /// for header-based credentials. `reqwest::Error`'s own `Display` embeds
    /// the request URL, so rendering `{source}` would put a query-string
    /// credential straight back in. `summary` is therefore a classified,
    /// URL-free rendering of the same failure, and `source` is kept as
    /// `#[source]` for programmatic inspection but never interpolated.
    #[error("transport error fetching {url}: {source}")]
    Transport {
        url: String,
        /// Wrapped so neither `Display` nor the derived `Debug` can render a
        /// URL. See [`TransportSource`].
        #[source]
        source: TransportSource,
    },

    /// The pacer refused before anything went on the wire: daily cap,
    /// backoff window, a busy ledger, or a wait too long to block on.
    #[error("pacing refused: {0}")]
    Pace(#[from] PaceDenied),

    /// The redirect budget was spent. The start URL is reported because the
    /// chain that ran away is rarely visible from the last hop.
    #[error("gave up after {hops} redirects starting at {url}")]
    TooManyRedirects { hops: usize, url: String },

    /// A `Location` we could not resolve against the current URL.
    #[error("redirect from {url} has an unusable Location header: {detail}")]
    BadRedirect { url: String, detail: String },

    /// A redirect we will not follow at all.
    #[error("refusing to follow the redirect from {from} to {to}: {reason}")]
    RedirectRefused {
        from: String,
        to: String,
        reason: &'static str,
    },

    /// Refused to *build* a header set carrying a credential the caller did
    /// not ask for — a header set declared unauthenticated is a promise, not
    /// a hint.
    #[error("refusing to carry the {header} header: {detail}")]
    CredentialRefused {
        header: &'static str,
        detail: String,
    },

    /// Refused to *send* a credential to a bucket it is not scoped to.
    ///
    /// ADR-007 §5: "a key is attached only to requests in its own publisher's
    /// bucket, and those requests don't follow redirects to other buckets."
    #[error("refusing to send {header} to {target}: the key is scoped to bucket {allowed}")]
    CredentialOutOfScope {
        header: &'static str,
        target: String,
        allowed: String,
    },

    /// The injected `reqwest::Client` could not be built.
    #[error("could not build the HTTP client: {source}")]
    ClientBuild {
        #[source]
        source: reqwest::Error,
    },
}

impl FetchError {
    /// When the caller should not retry before, in epoch milliseconds, if the
    /// publisher or the ledger told us.
    ///
    /// This is the value a caller feeds to `BackoffControl::set_backoff`.
    pub fn retry_at_ms(&self) -> Option<i64> {
        match self {
            Self::RateLimited { retry_after_ms, .. } => *retry_after_ms,
            Self::Pace(PaceDenied::Backoff { until_ms } | PaceDenied::WaitTooLong { until_ms }) => {
                Some(*until_ms)
            }
            _ => None,
        }
    }

    /// Is this the "come back later" family?
    ///
    /// `PaceDenied::LedgerBusy` is deliberately *not* in it: the ledger being
    /// unavailable is an infrastructure failure, and telling the campaign to
    /// retry later would hide it behind a rate-limit story.
    pub fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Pace(
                    PaceDenied::DailyCap
                        | PaceDenied::Backoff { .. }
                        | PaceDenied::WaitTooLong { .. }
                )
        )
    }

    /// Does this work need a human login (ADR-007 §3's `needs_login`)?
    pub fn needs_login(&self) -> bool {
        matches!(self, Self::LoginRedirect { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every failure has to say something a report can act on. An empty or
    /// unrecognisable message is how a retry loop becomes a silent campaign.
    #[test]
    fn every_variant_explains_itself() {
        let all = [
            FetchError::LoginRedirect {
                url: "https://login.example.org/SignIn".into(),
            },
            FetchError::RateLimited {
                bucket: "elsevier".into(),
                code: 429,
                retry_after_ms: Some(1_700_000_000_000),
            },
            FetchError::Status {
                code: 404,
                url: "https://example.org/pdf".into(),
            },
            FetchError::TooManyRedirects {
                hops: 5,
                url: "https://example.org/".into(),
            },
            FetchError::BadRedirect {
                url: "https://example.org/".into(),
                detail: "not ASCII".into(),
            },
            FetchError::RedirectRefused {
                from: "https://example.org/".into(),
                to: "file:///etc/passwd".into(),
                reason: "only http and https are followed",
            },
            FetchError::CredentialRefused {
                header: "Authorization",
                detail: "the caller asked for an unauthenticated request".into(),
            },
            FetchError::CredentialOutOfScope {
                header: "Cookie",
                target: "springer".into(),
                allowed: "elsevier".into(),
            },
            FetchError::Pace(PaceDenied::DailyCap),
        ];
        for e in all {
            let msg = e.to_string();
            assert!(!msg.trim().is_empty(), "{e:?} has no message");
            assert!(
                msg.len() > 12,
                "{e:?} message is not worth showing a user: {msg}"
            );
        }
    }

    #[test]
    fn rate_limited_and_pacing_denials_are_try_again_later() {
        let rl = FetchError::RateLimited {
            bucket: "elsevier".into(),
            code: 429,
            retry_after_ms: Some(42),
        };
        assert!(rl.is_rate_limited());
        assert_eq!(rl.retry_at_ms(), Some(42));

        for denied in [
            PaceDenied::DailyCap,
            PaceDenied::Backoff { until_ms: 7 },
            PaceDenied::WaitTooLong { until_ms: 9 },
        ] {
            let e = FetchError::Pace(denied);
            assert!(e.is_rate_limited(), "{denied} means try later");
        }
        assert_eq!(
            FetchError::Pace(PaceDenied::Backoff { until_ms: 7 }).retry_at_ms(),
            Some(7)
        );
    }

    /// A busy ledger is an infrastructure failure, not a publisher's answer.
    #[test]
    fn a_busy_ledger_is_not_reported_as_rate_limiting() {
        let e = FetchError::Pace(PaceDenied::LedgerBusy);
        assert!(!e.is_rate_limited(), "LedgerBusy must stay visible");
        assert_eq!(e.retry_at_ms(), None);
    }

    #[test]
    fn needs_login_is_only_the_login_redirect() {
        assert!(
            FetchError::LoginRedirect {
                url: "https://sso.example.org/auth".into()
            }
            .needs_login()
        );
        assert!(
            !FetchError::Status {
                code: 403,
                url: "https://example.org/".into()
            }
            .needs_login()
        );
    }

    #[test]
    fn the_source_chain_reaches_the_pacing_denial() {
        use std::error::Error as _;
        let e = FetchError::Pace(PaceDenied::WaitTooLong { until_ms: 3 });
        assert_eq!(
            e.source().map(ToString::to_string).as_deref(),
            Some("wait would exceed the 10s ceiling (until 3)")
        );
    }
}

/// Strip anything credential-shaped from a URL, for storage in an error.
///
/// A URL in an error message reaches further than the request did: it is
/// rendered in the TUI's task panel, returned verbatim to an MCP agent that
/// may log it or paste it into a transcript, and captured by `tracing`. The
/// existing reasoning — "carry the URL rather than the request, so a
/// credential can never reach an error string" — holds for header-based
/// credentials and **fails for query-parameter ones**, which is exactly how
/// OpenAlex takes its key (`?api_key=…`). So the URL is reduced at
/// construction rather than at display: `Debug` is derived, so redacting in
/// `Display` alone would leave the secret in the struct.
///
/// Everything else about the URL is kept. A bare host and path is what makes
/// an error actionable ("which request failed?"), and the parameter *names*
/// are not secret.
pub fn redact_url(url: &str) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        // Unparseable input is not echoed: it may not be a URL at all, and
        // the safe answer is to say nothing about its contents.
        return "<unparseable url>".to_string();
    };

    // Credentials embedded in the authority (`https://user:pass@host`).
    if parsed.username() != "" || parsed.password().is_some() {
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
    }

    let pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if pairs.iter().any(|(k, _)| is_credential_param(k)) {
        parsed.query_pairs_mut().clear();
        for (key, value) in pairs {
            if is_credential_param(&key) {
                parsed.query_pairs_mut().append_pair(&key, REDACTED);
            } else {
                parsed.query_pairs_mut().append_pair(&key, &value);
            }
        }
    }

    // A fragment is never transmitted, so it cannot have leaked on the wire —
    // but a caller can still hand us an OAuth callback URL whose fragment
    // holds a token, and this function's contract is "safe to put in an
    // error", not merely "safe to send".
    let fragment = parsed.fragment().unwrap_or("").to_string();
    if fragment_pairs_need_redaction(&fragment) {
        let kept: Vec<String> = fragment
            .split('&')
            .filter(|pair| !is_credential_param(pair.split('=').next().unwrap_or("")))
            .map(str::to_string)
            .collect();
        let rebuilt = kept.join("&");
        parsed.set_fragment(if rebuilt.is_empty() {
            None
        } else {
            Some(&rebuilt)
        });
    }

    parsed.to_string()
}

/// Does this fragment hold a credential-shaped parameter at all? Checked
/// before rewriting so a fragment with no credentials is left byte-identical.
fn fragment_pairs_need_redaction(fragment: &str) -> bool {
    !fragment.is_empty()
        && fragment
            .split('&')
            .any(|pair| is_credential_param(pair.split('=').next().unwrap_or("")))
}

/// A `reqwest::Error` that cannot print itself.
///
/// `reqwest::Error`'s `Debug` *and* `Display` both embed the request URL, so
/// merely redacting `FetchError::url` is not enough — and `FetchError`
/// derives `Debug`, which means a `{:?}` in a log line or a test failure
/// message leaks the same credential the `Display` fix was protecting.
///
/// The typed error is still reachable through [`Self::error`] for anything
/// programmatic (`is_timeout`, `is_connect`, `status`). Only the *rendering*
/// is replaced, with a classification that carries no URL.
pub struct TransportSource(reqwest::Error);

impl TransportSource {
    /// Wrap a transport failure so it cannot render a URL.
    pub fn new(source: reqwest::Error) -> Self {
        Self(source)
    }

    /// The underlying error, for programmatic inspection.
    pub fn error(&self) -> &reqwest::Error {
        &self.0
    }
}

impl std::fmt::Debug for TransportSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&transport_summary(&self.0))
    }
}

impl std::fmt::Display for TransportSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&transport_summary(&self.0))
    }
}

impl std::error::Error for TransportSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// Classify a transport failure without echoing the URL.
///
/// `reqwest::Error`'s `Display` includes the request URL, which is how a
/// `?api_key=…` credential survives an otherwise careful redaction. The
/// caller still has the typed `source` for anything programmatic; this is
/// only what a human or an agent reads.
pub fn transport_summary(source: &reqwest::Error) -> String {
    if source.is_timeout() {
        "timed out".to_string()
    } else if source.is_connect() {
        "connection failed".to_string()
    } else if source.is_redirect() {
        "too many redirects".to_string()
    } else if source.is_body() || source.is_decode() {
        "response body could not be read".to_string()
    } else if let Some(status) = source.status() {
        format!("unexpected status {status}")
    } else {
        // `is_request()` and anything unclassified: no detail, because the
        // detail `reqwest` would give is the URL.
        "request failed".to_string()
    }
}

/// Substrings that mark a query parameter as credential-shaped.
const CREDENTIAL_PARAM_NEEDLES: [&str; 6] = ["key", "token", "secret", "password", "passwd", "sig"];

/// Is this query-parameter name one that carries a credential?
///
/// Matched by substring rather than by an exact list, because the spelling
/// varies by publisher (`api_key`, `apikey`, `api-key`, `x-api-key`,
/// `access_token`, `insttoken`). Over-redacting an odd parameter name costs a
/// little debuggability; under-redacting publishes the key.
fn is_credential_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    CREDENTIAL_PARAM_NEEDLES
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Placeholder written in place of a credential value.
///
/// Deliberately not the empty string, so "this parameter was present" stays
/// visible, and deliberately URL-safe: a bracketed `<redacted>` comes back
/// out of `append_pair` as `%3Credacted%3E`, which is unreadable in exactly
/// the log line this exists to keep clean.
const REDACTED: &str = "REDACTED";

#[cfg(test)]
mod redaction_tests {
    use super::*;

    /// The exact leak from #276: the OpenAlex key is a query parameter, so it
    /// rides along in any error that carries the URL.
    #[test]
    fn an_openalex_api_key_never_appears_in_a_redacted_url() {
        let redacted = redact_url(
            "https://api.openalex.org/works/doi:10.1038/s41586-020-2649-2?api_key=sk-live-SUPERSECRET&mailto=me@example.org",
        );
        assert!(
            !redacted.contains("SUPERSECRET"),
            "the key survived redaction: {redacted}"
        );
        assert!(redacted.contains("api_key=REDACTED"), "{redacted}");
        // The non-credential parameters are untouched — they are what makes
        // the error actionable.
        assert!(redacted.contains("mailto=me%40example.org"), "{redacted}");
        // And the request itself is still identifiable.
        assert!(redacted.contains("api.openalex.org/works/"), "{redacted}");
    }

    #[test]
    fn debug_cannot_leak_either() {
        // `FetchError` derives `Debug`, so redacting only in `Display` would
        // leave the key in the struct.
        let secret = "sk-live-SUPERSECRET";
        let err = FetchError::Status {
            code: 500,
            url: redact_url(&format!("https://api.openalex.org/works?api_key={secret}")),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");
        assert!(!debug.contains(secret), "Debug leaked: {debug}");
        assert!(!display.contains(secret), "Display leaked: {display}");
        assert!(
            display.contains("500"),
            "the status must survive: {display}"
        );
    }

    #[test]
    fn credential_shaped_parameters_are_caught_whatever_the_spelling() {
        for name in [
            "api_key",
            "apikey",
            "api-key",
            "x-api-key",
            "access_token",
            "insttoken",
            "client_secret",
            "password",
            "signature",
        ] {
            let url = format!("https://example.org/p?{name}=LEAKME");
            assert!(
                !redact_url(&url).contains("LEAKME"),
                "{name} was not treated as a credential"
            );
        }
    }

    #[test]
    fn authority_credentials_are_stripped() {
        let redacted = redact_url("https://alice:hunter2@api.example.org/works");
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(redacted.contains("api.example.org"), "{redacted}");
    }

    #[test]
    fn a_url_with_no_credentials_is_returned_unchanged() {
        let url = "https://biorxiv.org/content/10.1101/2025.06.14.659707v1.full.pdf";
        assert_eq!(redact_url(url), url);
        let with_query = "https://example.org/works?filter=open_access:true&per-page=25";
        assert_eq!(redact_url(with_query), with_query);
    }

    #[test]
    fn unparseable_input_is_not_echoed() {
        // It may not be a URL at all; the conservative answer is to say
        // nothing about its contents.
        let out = redact_url("not a url at all, possibly a secret: sk-live-XYZ");
        assert!(!out.contains("sk-live"), "{out}");
        assert_eq!(out, "<unparseable url>");
    }

    #[test]
    fn a_fragment_can_also_carry_a_token() {
        let redacted = redact_url("https://example.org/callback#access_token=LEAKME&state=1");
        assert!(!redacted.contains("LEAKME"), "{redacted}");
    }
}
