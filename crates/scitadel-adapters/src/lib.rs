pub mod acquire;
pub mod acquire_ndjson;
pub mod arxiv;
pub mod crossref;
pub mod datacite;
pub mod download;
pub mod epo;
pub mod error;
pub mod europepmc;
pub mod identity;
pub mod identity_chain;
pub mod import_flat;
pub mod inspire;
pub mod lens;
pub mod magic;
pub mod manifest;
pub mod oa_live;
pub mod openalex;
pub mod osti;
pub mod patentsview;
pub mod pmc_oa;
pub mod preprint;
pub mod pubmed;
pub mod registry;
pub mod resolve;
pub mod scan;

use scitadel_core::config::{OpenAlexAuth, PatentsViewAdapterConfig, PatentsViewKeySource};
use scitadel_core::error::CoreError;
use scitadel_core::ports::SourceAdapter;

pub use openalex::SearchField;

/// Build adapter instances from source names.
pub fn build_adapters(
    sources: &[String],
    pubmed_api_key: &str,
    openalex: &OpenAlexAuth,
) -> Result<Vec<Box<dyn SourceAdapter>>, error::AdapterError> {
    build_adapters_full(sources, pubmed_api_key, openalex, "", "", "", "")
}

/// Build adapter instances with all credential options. Keeps the
/// legacy signature: the OpenAlex adapter uses its default relevance
/// mode (`SearchField::Any` — pre-#210 broad `search=`).
///
/// The PatentsView adapter is built UNBOUND here (no host binding
/// check), matching the pre-#268 signature. Callers that want the
/// host binding enforced must use [`build_adapters_with_field`] and
/// pass a [`PatentsViewAdapterConfig`] produced by
/// [`scitadel_core::config::resolve_patentsview_adapter`].
pub fn build_adapters_full(
    sources: &[String],
    pubmed_api_key: &str,
    openalex: &OpenAlexAuth,
    patentsview_key: &str,
    lens_token: &str,
    epo_key: &str,
    epo_secret: &str,
) -> Result<Vec<Box<dyn SourceAdapter>>, error::AdapterError> {
    let patentsview = PatentsViewAdapterConfig {
        api_key: patentsview_key.to_string(),
        base_url: None,
        bound_host: None,
        legacy_unbound: false,
        // The legacy signature does not carry a source hint; the
        // adapter builder here never enforces a binding (bound_host
        // is None), so the remediation hint is unreachable.
        key_source: PatentsViewKeySource::None,
    };
    build_adapters_with_field(
        sources,
        pubmed_api_key,
        openalex,
        &patentsview,
        lens_token,
        epo_key,
        epo_secret,
        SearchField::default(),
    )
}

/// Same as `build_adapters_full` but lets the caller pick the OpenAlex
/// relevance mode (#210). Non-OpenAlex adapters are unaffected — their
/// search shapes don't map cleanly onto a single "field" knob.
///
/// PatentsView takes its fully-resolved [`PatentsViewAdapterConfig`]
/// (#268) so the host-binding check runs here: if the key is bound to
/// a host and the effective base URL's host does not match, no
/// adapter is built and no request is ever made.
#[allow(clippy::too_many_arguments)]
pub fn build_adapters_with_field(
    sources: &[String],
    pubmed_api_key: &str,
    openalex: &OpenAlexAuth,
    patentsview: &PatentsViewAdapterConfig,
    lens_token: &str,
    epo_key: &str,
    epo_secret: &str,
    openalex_field: SearchField,
) -> Result<Vec<Box<dyn SourceAdapter>>, error::AdapterError> {
    let mut adapters: Vec<Box<dyn SourceAdapter>> = Vec::new();

    for source in sources {
        match source.as_str() {
            "pubmed" => {
                adapters.push(Box::new(pubmed::PubMedAdapter::new(
                    pubmed_api_key.to_string(),
                    30.0,
                )));
            }
            "arxiv" => {
                adapters.push(Box::new(arxiv::ArxivAdapter::new(30.0)));
            }
            "openalex" => {
                adapters.push(Box::new(
                    openalex::OpenAlexAdapter::new(openalex.clone(), 30.0)
                        .with_default_field(openalex_field),
                ));
            }
            "inspire" => {
                adapters.push(Box::new(inspire::InspireAdapter::new(30.0)));
            }
            "patentsview" => {
                // The patentsview adapter posts the keychain-stored key as
                // `X-Api-Key`; a `base_url` from `.scitadel/config.toml`
                // (committable at the repo top level) or
                // `SCITADEL_PATENTSVIEW_URL` reaches here via `patentsview`.
                // Validate it once (so http-non-loopback and malformed
                // URLs never see the key), then enforce the host binding
                // (so a bound key never reaches a different host) — both
                // before any adapter is built (#268).
                let validated = validate_patentsview_base_url(patentsview.base_url.as_deref())?;
                let effective_url = validated
                    .as_deref()
                    .unwrap_or(patentsview::PATENTSVIEW_API_URL);
                enforce_patentsview_host_binding(patentsview, effective_url)?;

                let mut adapter =
                    patentsview::PatentsViewAdapter::new(patentsview.api_key.clone(), 30.0);
                if let Some(url) = validated {
                    adapter = adapter.with_base_url(url);
                }
                adapters.push(Box::new(adapter));
            }
            "lens" => {
                adapters.push(Box::new(lens::LensAdapter::new(
                    lens_token.to_string(),
                    30.0,
                )));
            }
            "epo" => {
                adapters.push(Box::new(epo::EpoOpsAdapter::new(
                    epo_key.to_string(),
                    epo_secret.to_string(),
                    30.0,
                )));
            }
            other => {
                return Err(error::AdapterError::UnknownSource(other.to_string()));
            }
        }
    }

    Ok(adapters)
}

/// Validate a configured `base_url` for the PatentsView adapter (#268).
///
/// The adapter posts the keychain-stored API key as `X-Api-Key`, so a
/// user-supplied endpoint has to be guarded: plaintext http would expose
/// the key on the wire, a non-https scheme makes no sense for this
/// grammar, and a malformed URL would surface as a confusing reqwest
/// error at search time instead of a config error at startup.
///
/// Rules:
/// - `None`, or a value that is empty or whitespace-only after trimming,
///   is normalised to `None` so an empty `base_url = ""` in
///   `.scitadel/config.toml` behaves like an unset
///   `SCITADEL_PATENTSVIEW_URL` and the adapter's built-in default wins.
/// - `https://` is accepted.
/// - `http://` is accepted only for loopback hosts (`127.0.0.1`, `::1`,
///   `localhost`), which the wiremock-backed tests need.
/// - Anything else is a [`CoreError::Config`] with a message that names
///   what was wrong.
///
/// Error messages echo the configured URL as its origin (scheme + host +
/// port) only. `userinfo` (`user:pass@`) is stripped, and the path and
/// query are dropped — the grammar's path is a fixed `/api/v1/patent/`
/// and bearer tokens routinely ride in query strings (e.g. `?key=…`), so
/// echoing either would push them into logs.
pub fn validate_patentsview_base_url(raw: Option<&str>) -> Result<Option<String>, CoreError> {
    let Some(trimmed) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };

    let parsed = url::Url::parse(trimmed).map_err(|e| {
        // The input is not a URL, so there is nothing to parse userinfo
        // out of; echo it as given. Still quote it so the shape of the
        // typo is visible.
        CoreError::Config(format!(
            "patentsview base_url is not a valid URL ({e}): {trimmed:?}"
        ))
    })?;

    match parsed.scheme() {
        "https" => Ok(Some(trimmed.to_string())),
        "http" => {
            let is_loopback = match parsed.host() {
                Some(url::Host::Domain(d)) => d == "localhost",
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            };
            if is_loopback {
                Ok(Some(trimmed.to_string()))
            } else {
                let safe = url_origin_for_display(&parsed);
                Err(CoreError::Config(format!(
                    "patentsview base_url must use https; plaintext http is only allowed \
                     for loopback hosts (127.0.0.1, ::1, localhost). Got {safe}. \
                     The adapter posts the stored API key as X-Api-Key, so the key would \
                     leave the machine in cleartext."
                )))
            }
        }
        other => {
            let safe = url_origin_for_display(&parsed);
            Err(CoreError::Config(format!(
                "patentsview base_url must use https (got scheme {other:?}): {safe}"
            )))
        }
    }
}

/// Render a URL as `scheme://host[:port]` for safe echoing in errors.
///
/// Drops `userinfo` (`user:pass@`), the path and the query string. The
/// path on this adapter is a fixed `/api/v1/patent/`, so the operator
/// already knows what it is; bearer tokens, however, do sometimes
/// appear in query strings (`?api_key=…`) by mistake, and echoing them
/// would push the secret into any shell or CI log that captured the
/// error.
pub(crate) fn url_origin_for_display(u: &url::Url) -> String {
    let scheme = u.scheme();
    match u.host_str() {
        Some(host) => {
            // Ipv6 hosts need brackets in URL syntax.
            let host_display = match u.host() {
                Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
                _ => host.to_string(),
            };
            match u.port() {
                Some(p) => format!("{scheme}://{host_display}:{p}"),
                None => format!("{scheme}://{host_display}"),
            }
        }
        None => format!("{scheme}://<no host>"),
    }
}

/// Enforce the host binding of a PatentsView key (#268).
///
/// If the key has a `bound_host`, the effective URL's host must match
/// it exactly. Otherwise the adapter is not built and no request is
/// ever made.
///
/// Comparison is on the *host only, any port*: scitadel never stores
/// a `:port` in the binding, and the request URL's port is ignored.
/// The host read from the request URL is pushed through the same
/// [`scitadel_core::credentials::normalize_patentsview_host`] normaliser
/// that `scitadel auth login --host <X>` applied at store time, so
/// `Patentref.IO`, `patentref.io.` and `patentref.io` are the same
/// host for this check.
///
/// `effective_url` is what [`build_adapters_with_field`] computed — the
/// validated `base_url`, or the built-in default. It is assumed to be
/// a valid URL; a malformed one is treated as "no host", which
/// refuses as a mismatch.
///
/// The error message is **source-aware**: the remediation hint is
/// different for a stored key (rebind with `auth login patentsview
/// --host <h>`) and for an env-set key (set `SCITADEL_PATENTSVIEW_URL`
/// or clear the file-set `base_url`) — telling an env-key user to run
/// `auth login` was a review nit (the key doesn't live there).
pub(crate) fn enforce_patentsview_host_binding(
    cfg: &PatentsViewAdapterConfig,
    effective_url: &str,
) -> Result<(), CoreError> {
    let Some(bound_host) = cfg.bound_host.as_deref() else {
        return Ok(());
    };

    let parsed = url::Url::parse(effective_url).ok();
    let request_host_raw = parsed.as_ref().and_then(|u| u.host_str());
    // Apply the same normaliser used at store time so two spellings
    // of the same host never cross-refuse. `url::Url::parse` already
    // lowercases ASCII domains, but the normaliser is cheap and keeps
    // the two call sites symmetric.
    let request_host_norm = request_host_raw
        .and_then(|h| scitadel_core::credentials::normalize_patentsview_host(h).ok());
    let request_host_for_msg = request_host_raw.unwrap_or("<no host>");
    let origin = parsed
        .as_ref()
        .map_or_else(|| format!("{effective_url:?}"), url_origin_for_display);

    if request_host_norm.as_deref() == Some(bound_host) {
        return Ok(());
    }

    let msg = if cfg.legacy_unbound {
        format!(
            "the stored patentsview key has no host binding and is treated as a USPTO \
             PatentSearch key (search.patentsview.org), which USPTO retired on \
             2026-03-20. The configured endpoint is {origin}, so the key is NOT being \
             sent. Request a PatentRef key at https://patentref.io/docs/migration.html \
             and run `scitadel auth login patentsview` to replace it; or, if you trust \
             the configured endpoint, run `scitadel auth login patentsview --host \
             {request_host_for_msg}` with a key that mirror issues.",
        )
    } else {
        match cfg.key_source {
            PatentsViewKeySource::Env | PatentsViewKeySource::None => format!(
                "the env-set patentsview key (SCITADEL_PATENTSVIEW_KEY) is held to the \
                 default host {bound_host} because `[patentsview] base_url` in \
                 `.scitadel/config.toml` is not authority for sending it elsewhere; \
                 the configured endpoint is {origin}. Either unset `[patentsview] \
                 base_url` in `.scitadel/config.toml`, or set \
                 SCITADEL_PATENTSVIEW_URL=<trusted URL> so the operator's choice \
                 is explicit this invocation.",
            ),
            PatentsViewKeySource::Stored => format!(
                "the stored patentsview key is bound to {bound_host}; the configured \
                 endpoint is {origin}. Run `scitadel auth login patentsview --host \
                 {request_host_for_msg}` to bind a key to that host if you trust it. \
                 Binding is host-only (any port).",
            ),
            PatentsViewKeySource::File => format!(
                "the patentsview key in `.scitadel/config.toml` is bound to \
                 {bound_host}; the configured endpoint is {origin}. Change \
                 `[patentsview] base_url` in the same file, or move the key \
                 to `scitadel auth login patentsview --host \
                 {request_host_for_msg}`.",
            ),
        }
    };
    Err(CoreError::Config(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_base_url_is_accepted() {
        let got = validate_patentsview_base_url(Some("https://mirror.example.org/api/v1/patent/"))
            .expect("https must be accepted");
        assert_eq!(
            got.as_deref(),
            Some("https://mirror.example.org/api/v1/patent/")
        );
    }

    #[test]
    fn http_to_a_non_loopback_host_is_rejected() {
        let err = validate_patentsview_base_url(Some("http://mirror.example.org/api/v1/patent/"))
            .expect_err("plaintext http to a non-loopback host must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("https"), "{msg}");
        assert!(msg.contains("loopback"), "{msg}");
        assert!(msg.contains("X-Api-Key"), "{msg}");
    }

    #[test]
    fn http_to_a_loopback_host_is_accepted_for_wiremock() {
        for url in [
            "http://127.0.0.1:1234/api/v1/patent/",
            "http://localhost:4567/api/v1/patent/",
            "http://[::1]:1/api/v1/patent/",
        ] {
            let got = validate_patentsview_base_url(Some(url))
                .unwrap_or_else(|e| panic!("loopback http must be accepted for {url}: {e}"));
            assert_eq!(got.as_deref(), Some(url));
        }
    }

    #[test]
    fn an_empty_or_whitespace_base_url_falls_back_to_the_default() {
        for raw in [None, Some(""), Some("   "), Some("\t\n")] {
            let got = validate_patentsview_base_url(raw)
                .unwrap_or_else(|e| panic!("empty should be None (got {raw:?}): {e}"));
            assert!(got.is_none(), "{raw:?} -> {got:?}");
        }
    }

    #[test]
    fn a_non_http_scheme_is_rejected() {
        let err = validate_patentsview_base_url(Some("ftp://mirror.example.org/patents"))
            .expect_err("ftp must be rejected");
        assert!(err.to_string().contains("https"), "{err}");
    }

    #[test]
    fn a_malformed_url_is_rejected_with_a_clear_message() {
        let err = validate_patentsview_base_url(Some("not-a-url"))
            .expect_err("a non-URL must be rejected");
        assert!(err.to_string().contains("not a valid URL"), "{err}");
    }

    #[test]
    fn the_default_host_constant_matches_the_default_url_host() {
        // The CLI's `auth login patentsview` (no `--host`) binds the
        // stored key to `PATENTSVIEW_DEFAULT_HOST`; the adapter builder
        // compares against the host of `PATENTSVIEW_API_URL`. If the
        // two drift, the no-`--host` default silently refuses every
        // request. This test pins them together.
        let url = url::Url::parse(patentsview::PATENTSVIEW_API_URL)
            .expect("the default URL constant must be a valid URL");
        assert_eq!(
            url.host_str(),
            Some(scitadel_core::credentials::PATENTSVIEW_DEFAULT_HOST),
            "PATENTSVIEW_DEFAULT_HOST ({:?}) must equal the host of \
             PATENTSVIEW_API_URL ({})",
            scitadel_core::credentials::PATENTSVIEW_DEFAULT_HOST,
            patentsview::PATENTSVIEW_API_URL,
        );
    }

    // ---------------------------------------------------------------
    // enforce_patentsview_host_binding — host normalisation and
    // source-aware refusal messages (#268).
    // ---------------------------------------------------------------

    fn pv_cfg(
        bound_host: Option<&str>,
        key_source: PatentsViewKeySource,
        legacy_unbound: bool,
    ) -> PatentsViewAdapterConfig {
        PatentsViewAdapterConfig {
            api_key: "k".into(),
            base_url: None,
            bound_host: bound_host.map(str::to_string),
            legacy_unbound,
            key_source,
        }
    }

    #[test]
    fn a_matching_host_is_accepted_regardless_of_port_or_case() {
        // Host-only binding: a `:1234` port on the request URL does
        // not disagree with a bare-host binding, and case does not
        // matter because both sides normalise.
        let cfg = pv_cfg(Some("patentref.io"), PatentsViewKeySource::Stored, false);
        enforce_patentsview_host_binding(&cfg, "https://patentref.io:8443/api/v1/patent/")
            .expect("a port must not disagree with a host-only binding");
        enforce_patentsview_host_binding(&cfg, "https://PatentRef.IO/api/v1/patent/")
            .expect("case must not disagree");
    }

    #[test]
    fn a_stored_key_refusal_points_at_auth_login_host_rebind() {
        let cfg = pv_cfg(Some("patentref.io"), PatentsViewKeySource::Stored, false);
        let err =
            enforce_patentsview_host_binding(&cfg, "https://mirror.example.org/api/v1/patent/")
                .expect_err("cross-host must refuse");
        let msg = err.to_string();
        assert!(msg.contains("stored patentsview key"), "{msg}");
        assert!(
            msg.contains("scitadel auth login patentsview --host"),
            "a stored-key error must point at the rebind command: {msg}"
        );
        assert!(
            !msg.contains("SCITADEL_PATENTSVIEW_URL"),
            "a stored-key error must NOT mention SCITADEL_PATENTSVIEW_URL: {msg}"
        );
    }

    #[test]
    fn an_env_key_refusal_points_at_scitadel_patentsview_url_not_auth_login() {
        // The nit: telling an env-key user to run `auth login` is
        // wrong — their key doesn't live there. The remediation is
        // env-side (SCITADEL_PATENTSVIEW_URL) or TOML-side (clear the
        // file's base_url).
        let cfg = pv_cfg(Some("patentref.io"), PatentsViewKeySource::Env, false);
        let err = enforce_patentsview_host_binding(&cfg, "https://attacker.example/api/v1/patent/")
            .expect_err("cross-host must refuse");
        let msg = err.to_string();
        assert!(msg.contains("env-set patentsview key"), "{msg}");
        assert!(
            msg.contains("SCITADEL_PATENTSVIEW_URL"),
            "an env-key error must point at SCITADEL_PATENTSVIEW_URL: {msg}"
        );
        assert!(
            msg.contains("`[patentsview] base_url`") || msg.contains("patentsview] base_url"),
            "an env-key error must name the file-set base_url: {msg}"
        );
        assert!(
            !msg.contains("auth login patentsview --host"),
            "an env-key error must NOT say `auth login --host` (that only writes a stored key): {msg}"
        );
    }
}
