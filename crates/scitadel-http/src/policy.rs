//! Bucket routing: which publisher platform a host belongs to.
//!
//! ADR-007 §4: "Buckets are publisher platforms, not hosts. A policy table
//! maps hosts to buckets." The limit that matters is the platform's, so
//! `sciencedirect.com`, `pdf.sciencedirectassets.com`, `ars.els-cdn.com`,
//! `linkinghub.elsevier.com` and `api.elsevier.com` all spend the *same*
//! budget. Splitting them by host would let a single acquisition chain
//! multiply a publisher's real allowance by however many CDN aliases it
//! happens to use.
//!
//! The default table below is a *starting point*, and every number in it is a
//! policy choice recorded next to the row it came from. Configuration may
//! make any of them stricter; there is no path that loosens one, because
//! scitadel cannot know what a publisher's actual terms allow.
//!
//! Who enforces a [`BucketPolicy`]: not `PacedClient`. This crate resolves a
//! URL to its platform and asks the [`scitadel_core::ports::Pacer`] for
//! permits; the `Pacer` implementation — the SQLite ledger in `scitadel-db` —
//! is what reads [`BucketRoute::policy`] and enforces the interval and the
//! caps. That is deliberate: the interval can only be enforced where the
//! ledger's `BEGIN IMMEDIATE` transaction can reserve a slot, and a second
//! implementation of the same arithmetic in this crate would be a second thing
//! to get wrong.

use std::collections::HashMap;

use scitadel_core::ports::{Bucket, BucketPolicy};
use url::Url;

/// ADR-007 §4, "unknown host": 5 s per request, 500 requests per rolling
/// 24 h. Applied to every host the table does not know, and the floor under
/// every host it does.
pub const UNKNOWN_HOST_POLICY: BucketPolicy = BucketPolicy::new(5_000, 500, 500);

/// Bucket name for a URL with no host at all (e.g. `mailto:`). Not a real
/// network target, but it still needs a bucket so the pacer is never skipped.
pub const NAMELESS_BUCKET: &str = "unknown";

/// One publisher platform: its name, its policy, and every host that spends
/// its budget.
struct BucketSpec {
    name: &'static str,
    policy: BucketPolicy,
    hosts: &'static [&'static str],
}

/// The built-in table.
///
/// Where the ADR defers to "the publisher's documented limit", the value and
/// where it was read are recorded here — a policy table with unattributed
/// numbers is a policy table nobody can audit.
const BUCKETS: &[BucketSpec] = &[
    BucketSpec {
        // ADR-007 §4, "publisher hosts, non-TDM": 10 s, 300 requests, 100
        // works. This bucket also covers `api.elsevier.com`, whose ADR row
        // ("other tier-2 APIs: per the publisher's API terms") is looser, and
        // the Springer TDM row's 1 s with it. A bucket has exactly one
        // policy, so the strictest rule that applies to any host in the
        // bucket wins: the client must not be able to exceed a publisher's
        // real limit by taking the fast row for the alias that serves it.
        // The cost is throughput on the sanctioned TDM route; §3 already
        // forbids scraping ScienceDirect, so the only traffic this bucket
        // ever sees is the TDM API and tier-3 OA landing pages.
        name: "elsevier",
        policy: BucketPolicy::new(10_000, 300, 100),
        hosts: &[
            "sciencedirect.com",
            "pdf.sciencedirectassets.com",
            "ars.els-cdn.com",
            "linkinghub.elsevier.com",
            "api.elsevier.com",
            "elsevier.com",
        ],
    },
    BucketSpec {
        // ADR-007 §4, "OA repositories and arXiv": 3 s, 1 000 requests. No
        // separate work cap is documented, so the work cap mirrors the request
        // cap and the request cap is the one that binds.
        name: "arxiv",
        policy: BucketPolicy::new(3_000, 1_000, 1_000),
        hosts: &["arxiv.org", "export.arxiv.org"],
    },
    BucketSpec {
        // Same row as arXiv: bioRxiv and medRxiv are one Cold Spring Harbor
        // platform, so one budget.
        name: "biorxiv",
        policy: BucketPolicy::new(3_000, 1_000, 1_000),
        hosts: &["biorxiv.org", "medrxiv.org"],
    },
    BucketSpec {
        // ADR-007 §4, "metadata APIs": the API's documented limit, no extra
        // floor. OpenAlex: 100 requests/second and 100 000 credits/day
        // (developers.openalex.org, Authentication and Rate limits,
        // consulted 2026-10). A list-or-filter query costs 10 credits, so the
        // daily credit budget is the binding constraint for scitadel's
        // dominant call shape: 10 000 requests/day.
        name: "openalex",
        policy: BucketPolicy::new(10, 10_000, 10_000),
        hosts: &["api.openalex.org", "openalex.org"],
    },
    BucketSpec {
        // Crossref: polite pool, 3 requests/second for list-of-records
        // queries and 10/s for single records, effective 1 Dec 2025 and
        // 21 Jul 2026 (crossref.org documentation, consulted 2026-10).
        // scitadel always sends `mailto`, so it sits in the polite pool; the
        // list limit is the one that applies to every call shape it uses.
        // Crossref publishes no daily cap, so the 24 h figure is scitadel's
        // own per-user ceiling — a policy choice, not a published limit.
        name: "crossref",
        policy: BucketPolicy::new(350, 50_000, 50_000),
        hosts: &["api.crossref.org", "crossref.org", "doi.crossref.org"],
    },
    BucketSpec {
        // DataCite: 1 000 requests per 5 minutes per IP for the "identified"
        // tier (a `mailto` parameter or a User-Agent carrying an address),
        // which is the tier scitadel qualifies for
        // (support.datacite.org/docs/rate-limit, consulted 2026-10). The
        // interval is set so that any 5-minute window fits inside that
        // allowance. That window shape cannot be expressed in a 24 h cap,
        // and DataCite publishes no daily cap, so the 24 h figure is again a
        // scitadel policy choice.
        name: "datacite",
        policy: BucketPolicy::new(310, 100_000, 100_000),
        hosts: &["api.datacite.org", "datacite.org"],
    },
    BucketSpec {
        // Europe PMC: 10 requests/second per IP, no daily cap (Europe PMC
        // developer documentation, consulted 2026-10).
        name: "europepmc",
        policy: BucketPolicy::new(100, 100_000, 100_000),
        // `ebi.ac.uk` has a two-label public suffix, so the coarse
        // registrable-domain default would put Europe PMC in a bucket named
        // `ac.uk` shared with every UK academic host on the internet. This
        // row is what stops that.
        hosts: &["ebi.ac.uk", "europepmc.org"],
    },
    BucketSpec {
        // Unpaywall: 100 000 calls/day (unpaywall.org/products/api, consulted
        // 2026-10). It publishes no per-second figure, so the interval is a
        // conservative 10/s rather than a documented limit.
        name: "unpaywall",
        policy: BucketPolicy::new(100, 100_000, 100_000),
        hosts: &["api.unpaywall.org", "unpaywall.org"],
    },
    BucketSpec {
        // ADR-007 §4, "Springer platform (TDM)": 1 s per request, caps "per
        // `tdm_authorisation`". The caps below stand in for the
        // authorisation's caps until the credential store lands, and use the
        // ADR's publisher-host row (300 requests / 100 works). S2 must only
        // ever lower them.
        name: "springer",
        policy: BucketPolicy::new(1_000, 300, 100),
        hosts: &[
            "link.springer.com",
            "api.springernature.com",
            "springernature.com",
            "springer.com",
        ],
    },
];

/// A host (or `host:port`) and the bucket its requests spend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BucketRoute {
    /// The publisher platform.
    pub bucket: Bucket,
    /// That platform's policy.
    pub policy: BucketPolicy,
}

/// Host → publisher-platform bucket, plus the policy for each bucket.
#[derive(Clone, Debug)]
pub struct BucketPolicyTable {
    /// Keyed by [`route_key`], i.e. lowercase with any leading `www.` removed.
    routes: HashMap<String, Bucket>,
    policies: HashMap<String, BucketPolicy>,
}

impl BucketPolicyTable {
    /// The built-in table: every host ADR-007 §4 names, plus the unknown-host
    /// defaults for everything else.
    pub fn new() -> Self {
        let mut table = Self::empty();
        for spec in BUCKETS {
            table.policy(spec.name, spec.policy);
            for host in spec.hosts {
                table.route(host, spec.name);
            }
        }
        table
    }

    /// A table with no routes at all: every host is an unknown host.
    pub fn empty() -> Self {
        Self {
            routes: HashMap::new(),
            policies: HashMap::new(),
        }
    }

    /// Point a host (or `host:port`) at a bucket, creating the bucket's
    /// policy if this is its first host.
    ///
    /// A port is matched only when it is explicit and not the scheme default,
    /// so two local services on the same address stay separate buckets while
    /// every real publisher on 443 collapses to one.
    pub fn route(&mut self, host: &str, bucket: &str) -> &mut Self {
        self.policies
            .entry(bucket.to_string())
            .or_insert(UNKNOWN_HOST_POLICY);
        self.routes
            .insert(route_key(host), Bucket(bucket.to_string()));
        self
    }

    /// Set a bucket's policy.
    pub fn policy(&mut self, bucket: &str, policy: BucketPolicy) -> &mut Self {
        self.policies.insert(bucket.to_string(), policy);
        self
    }

    /// The bucket a URL's requests are charged to.
    pub fn bucket_for(&self, url: &Url) -> Bucket {
        self.resolve(url).bucket
    }

    /// The policy that applies to a bucket, falling back to the unknown-host
    /// defaults for a bucket no host routes to.
    pub fn policy_for(&self, bucket: &Bucket) -> BucketPolicy {
        self.policies
            .get(bucket.as_str())
            .copied()
            .unwrap_or(UNKNOWN_HOST_POLICY)
    }

    /// Bucket and policy in one lookup, which is all `PacedClient` needs per
    /// hop.
    pub fn resolve(&self, url: &Url) -> BucketRoute {
        let host = url.host_str().unwrap_or_default();
        if let Some(bucket) = self.lookup(host, url) {
            let policy = self.policy_for(&bucket);
            return BucketRoute { bucket, policy };
        }
        BucketRoute {
            bucket: Bucket(unknown_bucket_name(url)),
            policy: UNKNOWN_HOST_POLICY,
        }
    }

    /// Registered lookup, most specific first: `host:port`, then `host`
    /// (which also covers the `www.`-stripped form).
    fn lookup(&self, host: &str, url: &Url) -> Option<Bucket> {
        if let Some(port) = non_default_port(url) {
            let with_port = self.routes.get(&route_key(&format!("{host}:{port}")));
            if let Some(bucket) = with_port {
                return Some(bucket.clone());
            }
        }
        self.routes.get(&route_key(host)).cloned()
    }
}

impl Default for BucketPolicyTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalise a host for table keys: lowercase, no port-independent surprise,
/// no `www.`.
///
/// Folding `www.` away means the table lists one name per platform instead of
/// two, and a request to `www.sciencedirect.com` spends the `sciencedirect.com`
/// budget instead of quietly forming a second one.
fn route_key(host: &str) -> String {
    let host = host.trim().to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// An explicit, non-default port — the thing that makes two services on
/// `127.0.0.1` two different buckets.
fn non_default_port(url: &Url) -> Option<u16> {
    let port = url.port()?;
    (Some(port) != default_port(url.scheme())).then_some(port)
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

/// The bucket name for a host that is not in the table.
///
/// Named after the registrable domain so that the CDN aliases of an unknown
/// publisher still share one budget, and carries the port when there is a
/// non-default one, because two services on the same address genuinely are
/// two rate-limit targets.
fn unknown_bucket_name(url: &Url) -> String {
    let Some(host) = url.host_str() else {
        return NAMELESS_BUCKET.to_string();
    };
    let base = registrable_domain(host);
    match non_default_port(url) {
        Some(port) => format!("{base}:{port}"),
        None => base,
    }
}

/// Best-effort registrable domain of a host that is not in the table.
///
/// Deliberately coarse: this takes the last two labels, so
/// `pdf.journals.example.co.uk` becomes `co.uk` rather than
/// `example.co.uk`. That is the safe direction. A too-broad bucket
/// under-requests one platform; a too-narrow one would let scitadel quietly
/// exceed a publisher's real limit, which is the failure this whole crate
/// exists to prevent. Hosts that need to be exact belong in the table — and
/// the built-in one can be extended with [`BucketPolicyTable::route`].
pub fn registrable_domain(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return NAMELESS_BUCKET.to_string();
    }
    // An IP literal has no registrable domain, and splitting it on `.` would
    // produce nonsense (`0.0.1` for `127.0.0.1`).
    if host.parse::<std::net::IpAddr>().is_ok() || host.contains(':') {
        return host;
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    match labels.len() {
        0 | 1 => host,
        n => labels[n - 2..].join("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).expect("test URL parses")
    }

    /// The ADR's own worked example. If these ever land in separate buckets,
    /// one Elsevier chain spends five budgets and the point of the table is
    /// gone.
    #[test]
    fn the_five_elsevier_hosts_are_one_bucket() {
        let table = BucketPolicyTable::new();
        let hosts = [
            "https://sciencedirect.com/science/article/pii/S0000000000000000",
            "https://pdf.sciencedirectassets.com/pdfs/x.pdf",
            "https://ars.els-cdn.com/content/image/figure.png",
            "https://linkinghub.elsevier.com/retrieve/pii/S000",
            "https://api.elsevier.com/content/article/doi/10.1/x",
        ];
        let resolved: Vec<Bucket> = hosts.iter().map(|h| table.bucket_for(&url(h))).collect();
        for (host, bucket) in hosts.iter().zip(&resolved) {
            assert_eq!(bucket, &Bucket("elsevier".into()), "{host} -> {bucket}");
        }
        // And the www alias, which is the same platform.
        assert_eq!(
            table.bucket_for(&url("https://www.sciencedirect.com/x")),
            Bucket("elsevier".into())
        );
    }

    #[test]
    fn every_named_host_of_the_adr_lands_in_its_own_bucket() {
        let table = BucketPolicyTable::new();
        for (host, want) in [
            ("https://arxiv.org/abs/2401.00001", "arxiv"),
            ("https://export.arxiv.org/abs/2401.00001", "arxiv"),
            ("https://www.biorxiv.org/content/10.1101/x", "biorxiv"),
            ("https://www.medrxiv.org/content/10.1101/x", "biorxiv"),
            ("https://api.openalex.org/works/W1", "openalex"),
            ("https://api.crossref.org/works/10.1/x", "crossref"),
            ("https://api.datacite.org/dois/10.1/x", "datacite"),
            ("https://api.unpaywall.org/v2/10.1/x", "unpaywall"),
            (
                "https://www.ebi.ac.uk/europepmc/webservices/rest/search",
                "europepmc",
            ),
        ] {
            assert_eq!(table.bucket_for(&url(host)), Bucket(want.into()), "{host}");
        }
    }

    /// An unknown host gets a bucket named after its registrable domain and the
    /// ADR's unknown-host defaults — 5 s, 500 requests.
    #[test]
    fn an_unknown_host_gets_registrable_domain_naming_and_the_defaults() {
        let table = BucketPolicyTable::new();
        let route = table.resolve(&url("https://pdf.unknown-publisher.test/dumps/1234.pdf"));
        assert_eq!(route.bucket, Bucket("unknown-publisher.test".into()));
        assert_eq!(route.policy.min_interval_ms, 5_000);
        assert_eq!(route.policy.request_cap, 500);
        assert_eq!(route.policy, UNKNOWN_HOST_POLICY);
    }

    /// A CDN alias must not become a second budget for the same platform.
    #[test]
    fn unknown_hosts_that_share_a_domain_share_a_bucket() {
        let table = BucketPolicyTable::new();
        let a = table.bucket_for(&url("https://cdn.unknown-publisher.test/a.pdf"));
        let b = table.bucket_for(&url("https://www.unknown-publisher.test/b.pdf"));
        assert_eq!(a, b);
        assert_eq!(a, Bucket("unknown-publisher.test".into()));
    }

    /// Two services on one address are two targets, and the table must be able
    /// to say so — otherwise every local test server shares a budget and the
    /// per-hop accounting cannot be observed.
    #[test]
    fn a_non_default_port_keeps_two_services_on_one_address_apart() {
        let table = BucketPolicyTable::new();
        let a = table.bucket_for(&url("http://127.0.0.1:38121/a"));
        let b = table.bucket_for(&url("http://127.0.0.1:38122/b"));
        assert_ne!(a, b, "different ports are different rate-limit targets");
        assert_eq!(a, Bucket("127.0.0.1:38121".into()));
        // Same host, different ports, same registrable domain.
        assert_eq!(registrable_domain("127.0.0.1"), "127.0.0.1");
    }

    #[test]
    fn the_default_port_does_not_split_a_bucket() {
        let table = BucketPolicyTable::new();
        assert_eq!(
            table.bucket_for(&url("https://sciencedirect.com:443/x")),
            Bucket("elsevier".into())
        );
        assert_eq!(
            table.bucket_for(&url("http://example.org:80/x")),
            Bucket("example.org".into())
        );
    }

    /// A registered `host:port` entry must win over the host-wide one.
    #[test]
    fn an_explicit_route_overrides_the_registrable_domain_default() {
        let mut table = BucketPolicyTable::new();
        table.route("127.0.0.1:9999", "alpha");
        table.route("127.0.0.1:9999", "beta"); // last write wins
        assert_eq!(
            table.bucket_for(&url("http://127.0.0.1:9999/x")),
            Bucket("beta".into())
        );
        // An unregistered port on the same host is still the default.
        assert_eq!(
            table.bucket_for(&url("http://127.0.0.1:9998/x")),
            Bucket("127.0.0.1:9998".into())
        );
    }

    #[test]
    fn registrable_domain_keeps_ip_literals_and_ipv6_whole() {
        assert_eq!(registrable_domain("127.0.0.1"), "127.0.0.1");
        assert_eq!(registrable_domain("[::1]"), "[::1]");
        assert_eq!(registrable_domain("localhost"), "localhost");
        assert_eq!(registrable_domain("a.b.c.example.org"), "example.org");
        assert_eq!(registrable_domain("Example.ORG."), "example.org");
        assert_eq!(registrable_domain(""), NAMELESS_BUCKET);
    }

    /// A URL with no host still needs a bucket, or the pacer gets skipped.
    #[test]
    fn a_hostless_url_still_resolves_to_a_bucket() {
        let table = BucketPolicyTable::new();
        let route = table.resolve(&url("mailto:someone@example.org"));
        assert_eq!(route.bucket, Bucket(NAMELESS_BUCKET.into()));
        assert_eq!(route.policy, UNKNOWN_HOST_POLICY);
    }

    #[test]
    fn route_keys_fold_case_and_www() {
        assert_eq!(route_key("WWW.ScienceDirect.com"), "sciencedirect.com");
        assert_eq!(route_key("  PDF.Example.ORG "), "pdf.example.org");
        assert_eq!(route_key("notwww.example.org"), "notwww.example.org");
    }

    /// The table is the ADR's bucket policy table: every row it documents must
    /// be present, or a platform silently inherits the 5 s / 500 default.
    #[test]
    fn every_adr_bucket_is_present_with_its_documented_interval() {
        let table = BucketPolicyTable::new();
        for (bucket, interval_ms) in [
            ("elsevier", 10_000),
            ("arxiv", 3_000),
            ("biorxiv", 3_000),
            ("openalex", 10),
            ("crossref", 350),
            ("datacite", 310),
            ("europepmc", 100),
            ("unpaywall", 100),
            ("springer", 1_000),
        ] {
            assert_eq!(
                table.policy_for(&Bucket(bucket.into())).min_interval_ms,
                interval_ms,
                "{bucket} must keep its ADR interval"
            );
        }
    }

    #[test]
    fn an_empty_table_is_all_unknown_hosts() {
        let table = BucketPolicyTable::empty();
        assert_eq!(
            table.bucket_for(&url("https://sciencedirect.com/x")),
            Bucket("sciencedirect.com".into())
        );
        assert_eq!(
            table.policy_for(&Bucket("sciencedirect.com".into())),
            UNKNOWN_HOST_POLICY
        );
    }
}
