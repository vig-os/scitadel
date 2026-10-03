//! Redirect classification: is the next URL a login page, and where does it
//! point?
//!
//! These are pure functions on a [`Url`] and a header map. ADR-007 §4's
//! login-redirect rule is a *reporting* decision — it sets `needs_login` and
//! backoffs the bucket — so it must be decidable without a network, and
//! testable without one.

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};
use url::Url;

/// Lowercase tokens that identify a login, SSO or authentication endpoint.
///
/// Matched against whole tokens of the host and the path, never against raw
/// substrings: "association" contains `sso` and a paper by *Smith and
/// Associates* would otherwise turn a perfectly good fetch into `needs_login`.
/// Query strings and fragments are not examined at all — DOIs, titles and
/// author lists live there.
const LOGIN_TOKENS: &[&str] = &[
    "auth",
    "authenticate",
    "authentication",
    "authorization",
    "authorize",
    "login",
    "logon",
    "openid",
    "shibboleth",
    "signin",
    "signon",
    "sso",
];

/// Separators inside a host label or path segment.
const TOKEN_SEPARATORS: [char; 4] = ['.', '-', '_', ':'];

/// Does this URL look like a login or SSO page?
///
/// True for `login.ezproxy.example.org/SignIn`, `…/cas/login`,
/// `…/Shibboleth.sso/Login`, `…/sign-in` and `…/auth`; false for a paper
/// whose title happens to contain "authentication".
pub fn is_login_redirect(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    is_login_part(host)
        || url
            .path_segments()
            .is_some_and(|mut segments| segments.any(is_login_part))
}

/// Is this host label or path segment a login endpoint?
///
/// Three ways to be one, all whole-part rather than substring:
///
/// - the part itself is a token, case-insensitively — `/SignIn`, `/login`;
/// - one of its `.`/`-`/`_`/`:`-separated labels is a token — the `login` of
///   `login.ezproxy.example.org`, the `sso` of `Idp/Shibboleth.sso`;
/// - the part with separators squeezed out is a token, so `sign-in`, `log_on`
///   and `open-id` are the same word as `signin`, `logon` and `openid`.
///
/// The host arrives already lowercased from the URL parser but a path segment
/// keeps its case, which is why the comparison ignores it.
fn is_login_part(part: &str) -> bool {
    if is_login_token(part) {
        return true;
    }
    if part.split(TOKEN_SEPARATORS).any(is_login_token) {
        return true;
    }
    let squeezed: String = part
        .chars()
        .filter(|c| !TOKEN_SEPARATORS.contains(c))
        .collect();
    is_login_token(&squeezed)
}

fn is_login_token(part: &str) -> bool {
    LOGIN_TOKENS
        .iter()
        .any(|token| part.eq_ignore_ascii_case(token))
}

/// Is this status one that carries a redirect target?
///
/// The five with a `Location` by definition. 304 and 300 are *redirection*
/// codes but have no target, and treating them as one would turn a plain
/// "not modified" into a parse error.
pub fn is_redirect_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308)
}

/// The `Location` of a redirect response, if there is one.
pub fn redirect_target<'h>(status: StatusCode, headers: &'h HeaderMap) -> Option<&'h HeaderValue> {
    if !is_redirect_status(status) {
        return None;
    }
    headers.get("location")
}

/// 429 and 503 are the two answers that mean "come back later" rather than
/// "this URL is gone" or "you are not allowed".
pub fn is_rate_limited(status: StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 503)
}

/// Epoch milliseconds from a `Retry-After` delta-seconds header.
///
/// The HTTP-date form is deliberately ignored: publishers that send it also
/// send a cap, and guessing a date out of the wall clock buys nothing over
/// falling back to the bucket's own backoff.
pub fn retry_after_ms(headers: &HeaderMap) -> Option<i64> {
    let seconds: i64 = headers
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    epoch_ms().checked_add(seconds.checked_mul(1_000)?)
}

/// Milliseconds since the Unix epoch, from the system clock.
pub fn epoch_ms() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(delta) => i64::try_from(delta.as_millis()).unwrap_or(i64::MAX),
        // Before 1970. Any publisher expecting a `Retry-After` then is not
        // talking to a real clock.
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("test URL parses")
    }

    /// The login-page shapes an acquisition chain actually meets.
    #[test]
    fn login_urls_are_detected() {
        for candidate in [
            "https://login.ezproxy.example.org/login?redirect=https%3A%2F%2Fpdf",
            "https://linkinghub.elsevier.com/login?redirect=/retrieve/pii/S000",
            "https://idp.example.ac.uk/idp/profile/Login/1",
            "https://shibboleth.its.example.org/idp/profile/SAML2/Redirect/SSO",
            "https://idp.example.edu/Idp/Shibboleth.sso/Login",
            "https://www.example.org/SignIn?returnUrl=%2Fpaper",
            "https://www.example.org/sign-in",
            "https://www.example.org/user/login.html",
            "https://www.example.org/oauth2/authorize",
            "https://www.example.org/auth",
            "https://auth.example.org/",
            "https://www.example.org/openid/auth",
        ] {
            assert!(is_login_redirect(&url(candidate)), "{candidate}");
        }
    }

    /// The reason this is token matching and not substring matching. Each of
    /// these is a URL scitadel legitimately fetches, and every one contains
    /// `sso`, `auth` or `login` as raw text.
    #[test]
    fn real_fetch_urls_are_not_mistaken_for_login_pages() {
        for candidate in [
            "https://www.middle-earth-association.org/tolkien/403",
            "https://www.example.org/papers/authors/assonance",
            "https://www.example.org/doi/10.1000/authorised-replication",
            "https://sciencedirect.com/science/article/pii/S0000000000000000",
            "https://pdf.sciencedirectassets.com/pdfs/x.pdf",
            "https://api.crossref.org/works?query.title=authentication+of+objects",
            "https://api.openalex.org/works?search=login+to+the+game",
            "https://arxiv.org/abs/2401.00001",
            "https://linkinghub.elsevier.com/retrieve/pii/S0000000000000000",
        ] {
            assert!(!is_login_redirect(&url(candidate)), "{candidate}");
        }
    }

    /// The query string is where titles, DOIs and author lists live, so it is
    /// never evidence of a login.
    #[test]
    fn the_query_string_is_not_evidence() {
        assert!(!is_login_redirect(&url(
            "https://api.openalex.org/works?filter=title.search:sso+authentication"
        )));
    }

    #[test]
    fn only_the_five_target_carrying_statuses_are_redirects() {
        for code in [301, 302, 303, 307, 308] {
            assert!(
                is_redirect_status(StatusCode::from_u16(code).unwrap()),
                "{code}"
            );
        }
        for code in [200, 204, 300, 304, 401, 404, 500] {
            assert!(
                !is_redirect_status(StatusCode::from_u16(code).unwrap()),
                "{code}"
            );
        }
    }

    #[test]
    fn redirect_target_needs_both_a_redirect_status_and_a_location() {
        let mut headers = HeaderMap::new();
        headers.insert("location", HeaderValue::from_static("https://b.example/x"));
        assert!(redirect_target(StatusCode::FOUND, &headers).is_some());
        assert!(
            redirect_target(StatusCode::OK, &headers).is_none(),
            "a 200 with a Location header is not a redirect"
        );
        assert!(
            redirect_target(StatusCode::NOT_MODIFIED, &HeaderMap::new()).is_none(),
            "304 has no target and must not be treated as a missing-Location error"
        );
    }

    #[test]
    fn only_429_and_503_mean_slow_down() {
        for code in [429, 503] {
            assert!(
                is_rate_limited(StatusCode::from_u16(code).unwrap()),
                "{code}"
            );
        }
        for code in [200, 403, 404, 500, 502] {
            assert!(
                !is_rate_limited(StatusCode::from_u16(code).unwrap()),
                "{code}"
            );
        }
    }

    #[test]
    fn retry_after_is_read_as_delta_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("30"));
        let parsed = retry_after_ms(&headers).expect("30 seconds parses");
        let delta = parsed - epoch_ms();
        assert!(
            (29_000..=31_000).contains(&delta),
            "expected ~30s from now, got {delta}ms"
        );

        headers.insert(
            "retry-after",
            HeaderValue::from_static("Wed, 21 Oct 2026 07:28:00 GMT"),
        );
        assert_eq!(
            retry_after_ms(&headers),
            None,
            "the HTTP-date form is ignored"
        );

        headers.insert("retry-after", HeaderValue::from_static("soon"));
        assert_eq!(retry_after_ms(&headers), None);

        assert_eq!(retry_after_ms(&HeaderMap::new()), None);
    }
}
