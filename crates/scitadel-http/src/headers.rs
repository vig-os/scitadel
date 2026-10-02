//! `SafeHeaders`: the only way a caller gets headers onto the wire.
//!
//! ADR-007 §5 is unusually specific about credentials — "a key is attached
//! only to requests in its own publisher's bucket, and those requests don't
//! follow redirects to other buckets" — and its canary test is that a planted
//! secret never reaches an error path, a log or a render buffer. A
//! `HeaderMap` passed around freely satisfies none of that, so
//! [`SafeHeaders`] makes it a property of the value rather than a rule
//! somebody has to remember:
//!
//! - a header set built as *unauthenticated* refuses to hold a credential at
//!   all;
//! - a header set built for a publisher carries its credentials scoped to that
//!   publisher's bucket, and every hop is checked against it before the
//!   request is built;
//! - `Debug` prints header names, never values.

use std::fmt;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use scitadel_core::ports::Bucket;

use crate::error::FetchError;

/// Header names that carry a credential, with the spelling used in messages.
const CREDENTIAL_HEADERS: [(&str, &str); 3] = [
    ("authorization", "Authorization"),
    ("proxy-authorization", "Proxy-Authorization"),
    ("cookie", "Cookie"),
];

/// A request's headers, with the credential rules of ADR-007 §5 baked in.
#[derive(Clone, Default)]
pub struct SafeHeaders {
    map: HeaderMap,
    /// `Some(bucket)` when the caller is authenticated for that publisher's
    /// bucket; `None` when the request must be anonymous.
    scope: Option<Bucket>,
}

impl SafeHeaders {
    /// Headers for a request that must not carry a credential.
    pub fn unauthenticated() -> Self {
        Self::default()
    }

    /// Headers that may carry a credential, scoped to one publisher's bucket.
    ///
    /// `PacedClient` re-checks every hop against this bucket and refuses to
    /// send the credential to any other, which is what stops a redirect to a
    /// CDN or an aggregating proxy from handing a TDM key to a third party.
    pub fn authenticated(bucket: impl Into<String>) -> Self {
        Self {
            map: HeaderMap::new(),
            scope: Some(Bucket(bucket.into())),
        }
    }

    /// May these headers carry a credential at all?
    pub fn is_authenticated(&self) -> bool {
        self.scope.is_some()
    }

    /// The bucket this credential is scoped to, if any.
    pub fn scope(&self) -> Option<&Bucket> {
        self.scope.as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn get(&self, name: &HeaderName) -> Option<&HeaderValue> {
        self.map.get(name)
    }

    /// Add a header.
    ///
    /// A credential header on an unauthenticated set is refused, not dropped:
    /// a silently ignored `Authorization` would look like an authentication
    /// bug on the publisher's side, which is far harder to diagnose than an
    /// error here.
    pub fn insert(
        &mut self,
        name: HeaderName,
        value: HeaderValue,
    ) -> Result<Option<HeaderValue>, FetchError> {
        let refusal = credential_name(&name).and_then(|header| {
            self.scope.is_none().then(|| FetchError::CredentialRefused {
                header,
                detail: "the caller asked for an unauthenticated request".to_string(),
            })
        });
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        Ok(self.map.insert(name, value))
    }

    /// Set `Authorization`, e.g. `Bearer …`.
    pub fn set_authorization(&mut self, value: &str) -> Result<(), FetchError> {
        let value = credential_value("Authorization", value)?;
        self.insert(HeaderName::from_static("authorization"), value)?;
        Ok(())
    }

    /// Set `Cookie`, e.g. `session=…`.
    pub fn set_cookie(&mut self, value: &str) -> Result<(), FetchError> {
        let value = credential_value("Cookie", value)?;
        self.insert(HeaderName::from_static("cookie"), value)?;
        Ok(())
    }

    /// Does this set actually hold a credential? Scope alone is not enough —
    /// an authenticated set with no credential must not restrict the hop.
    pub fn has_credentials(&self) -> bool {
        self.map.keys().any(|name| credential_name(name).is_some())
    }

    /// Refuse a hop that would carry a credential outside its scope.
    ///
    /// Called once per hop by `PacedClient`, before the request is built and
    /// after the permit is known but before anything goes on the wire.
    pub(crate) fn check_scope(&self, bucket: &Bucket) -> Result<(), FetchError> {
        if !self.has_credentials() {
            return Ok(());
        }
        let Some(header) = self.map.keys().find_map(credential_name) else {
            return Ok(());
        };
        match &self.scope {
            None => Err(FetchError::CredentialRefused {
                header,
                detail: "an unauthenticated header set must not carry a credential".to_string(),
            }),
            Some(allowed) if allowed == bucket => Ok(()),
            Some(allowed) => Err(FetchError::CredentialOutOfScope {
                header,
                target: bucket.to_string(),
                allowed: allowed.to_string(),
            }),
        }
    }

    pub(crate) fn map(&self) -> &HeaderMap {
        &self.map
    }
}

/// `Debug` prints names and scope, never values (ADR-007 §5's canary test).
impl fmt::Debug for SafeHeaders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut names: Vec<&str> = self.map.keys().map(HeaderName::as_str).collect();
        names.sort_unstable();
        f.debug_struct("SafeHeaders")
            .field("scope", &self.scope)
            .field("header_names", &names)
            .field("values", &"<redacted>")
            .finish()
    }
}

impl PartialEq for SafeHeaders {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map && self.scope == other.scope
    }
}

fn credential_name(name: &HeaderName) -> Option<&'static str> {
    let lower = name.as_str();
    CREDENTIAL_HEADERS
        .iter()
        .find(|(needle, _)| *needle == lower)
        .map(|(_, spelled)| *spelled)
}

fn credential_value(header: &'static str, value: &str) -> Result<HeaderValue, FetchError> {
    HeaderValue::from_str(value).map_err(|e| FetchError::CredentialRefused {
        header,
        detail: format!("value is not a legal HTTP header value: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ELSEVIER: &str = "elsevier";

    fn elsevier() -> Bucket {
        Bucket(ELSEVIER.into())
    }

    /// The headline rule: asking for an anonymous request and then smuggling a
    /// credential in is a bug the type must catch.
    #[test]
    fn an_unauthenticated_set_refuses_to_carry_a_credential() {
        for (static_name, spelled) in CREDENTIAL_HEADERS {
            let mut headers = SafeHeaders::unauthenticated();
            let err = headers
                .insert(
                    HeaderName::from_static(static_name),
                    HeaderValue::from_static("Bearer hunter2"),
                )
                .expect_err("a credential must not fit in an anonymous set");
            assert!(matches!(err, FetchError::CredentialRefused { .. }), "{err}");
            assert!(err.to_string().contains(spelled), "{err}");
            assert!(headers.is_empty(), "{spelled} was refused but still stored");
        }

        let mut headers = SafeHeaders::unauthenticated();
        assert!(headers.set_authorization("Bearer tdm-key").is_err());
        assert!(headers.set_cookie("session=abc").is_err());
        assert!(!headers.has_credentials());
    }

    #[test]
    fn an_authenticated_set_accepts_a_credential_scoped_to_its_bucket() {
        let mut headers = SafeHeaders::authenticated(ELSEVIER);
        headers
            .set_authorization("Bearer tdm-key")
            .expect("scoped credential");
        headers
            .insert(
                HeaderName::from_static("accept"),
                HeaderValue::from_static("application/pdf"),
            )
            .expect("ordinary header");
        assert!(headers.is_authenticated());
        assert_eq!(headers.len(), 2);
        assert!(headers.check_scope(&elsevier()).is_ok());
    }

    /// ADR-007 §5: the key never leaves its publisher's bucket. This is the
    /// check that makes a redirect to a CDN or a proxy safe.
    #[test]
    fn a_credential_will_not_cross_into_another_bucket() {
        let mut headers = SafeHeaders::authenticated(ELSEVIER);
        headers
            .set_cookie("session=abc")
            .expect("scoped credential");
        let err = headers
            .check_scope(&Bucket("unknown-publisher.test".into()))
            .expect_err("the key must not go to another platform");
        assert!(
            matches!(err, FetchError::CredentialOutOfScope { .. }),
            "{err}"
        );
        let msg = err.to_string();
        assert!(msg.contains("Cookie"), "{msg}");
        assert!(msg.contains("elsevier"), "{msg}");
    }

    /// An authenticated set holding no credential must not restrict the hop —
    /// otherwise every hop to a second host would be refused for no reason.
    #[test]
    fn scope_only_restricts_when_a_credential_is_actually_present() {
        let headers = SafeHeaders::authenticated(ELSEVIER);
        assert!(headers.is_authenticated());
        assert!(!headers.has_credentials());
        assert!(headers.check_scope(&Bucket("arxiv".into())).is_ok());
    }

    /// An anonymous request may of course reach any bucket.
    #[test]
    fn anonymous_requests_are_unrestricted() {
        let headers = SafeHeaders::unauthenticated();
        assert!(!headers.is_authenticated());
        assert!(headers.check_scope(&Bucket("anything".into())).is_ok());
    }

    /// ADR-007 §5's canary test, at this layer: a planted secret must not
    /// reach `Debug`, which is what every log line and TUI render buffer ends
    /// up printing.
    #[test]
    fn debug_redacts_every_value() {
        const SECRET: &str = "tdm-secret-9f3a-do-not-print";
        let mut headers = SafeHeaders::authenticated(ELSEVIER);
        headers
            .set_authorization(&format!("Bearer {SECRET}"))
            .expect("scoped credential");
        headers.set_cookie(SECRET).expect("scoped credential");
        let rendered = format!("{headers:?}");
        assert!(!rendered.contains(SECRET), "the secret leaked: {rendered}");
        assert!(rendered.contains("authorization"), "{rendered}");
        assert!(rendered.contains("cookie"), "{rendered}");
        assert!(rendered.contains("redacted"), "{rendered}");
    }

    #[test]
    fn an_illegal_header_value_is_reported_not_smuggled() {
        let mut headers = SafeHeaders::authenticated(ELSEVIER);
        let err = headers
            .set_authorization("Bearer bad\nvalue")
            .expect_err("a newline cannot go in a header");
        assert!(err.to_string().contains("Authorization"), "{err}");
    }

    #[test]
    fn equality_compares_scope_as_well_as_headers() {
        let mut a = SafeHeaders::authenticated(ELSEVIER);
        a.set_authorization("Bearer k").expect("scoped");
        let mut b = SafeHeaders::authenticated(ELSEVIER);
        b.set_authorization("Bearer k").expect("scoped");
        let mut c = SafeHeaders::authenticated("springer");
        c.set_authorization("Bearer k").expect("scoped");
        assert_eq!(a, b);
        assert_ne!(a, c, "a different bucket is a different scope");
    }
}
