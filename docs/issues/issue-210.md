---
type: issue
state: closed
created: 2026-08-26T11:45:55Z
updated: 2026-09-29T00:22:38Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/210
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:39:26.487Z
---

# [Issue 210]: [openalex search cannot find papers by exact title; no DOI→metadata lookup](https://github.com/vig-os/scitadel/issues/210)

## Summary

`search` against `openalex` cannot find papers by their exact title, and there is no way to resolve a DOI to metadata. Both bit hard during a citation-verification workload: four fabricated or mis-attributed references survived a scitadel-based check and had to be caught with `curl` against CrossRef and the OpenAlex REST API.

## Repro 1 — exact title of a famous paper returns nothing relevant

```
search(query="Estimating the Dimension of a Model", sources="openalex", max_results=5)
```

Returns, in order:

1. A Proportional Hazards Model for the Subdistribution of a Competing Risk (1999)
2. The Graph Neural Network Model (2008)
3. [20] Processing of X-ray diffraction data collected in oscillation mode (1997)
4. Fitting Linear Mixed-Effects Models Using lme4 (2015)
5. mgm: Estimating Time-Varying Mixed Graphical Models... (2020)

The target — Schwarz 1978, *Estimating the Dimension of a Model*, Ann. Statist. 6(2):461–464, the BIC paper — is absent.

**It is not a coverage gap.** OpenAlex has the record, and its own title filter ranks it second:

```
GET https://api.openalex.org/works?filter=title.search:estimating%20the%20dimension%20of%20a%20model
  -> 2020  mgm: Estimating Time-Varying Mixed Graphical Models
  -> 1978  Estimating the Dimension of a Model      10.1214/aos/1176344136   <-- target
  -> 2001  Estimates of anthropogenic carbon uptake...
```

This suggests scitadel passes the query to OpenAlex's broad `search=` (full-text/abstract relevance) rather than `filter=title.search:`. Exact titles get drowned by topically-adjacent noise.

Same failure across many queries in one session, all for papers OpenAlex holds:

| query | what came back |
|---|---|
| `Provencher CONTIN constrained regularization program` | hemp protein concentrates; gene delivery; amyloid fibrils |
| `Improving FISTA faster smarter greedier Liang Schonlieb` | inertial viscosity algorithms; heart-disease detection |
| `On Learning Mixture Models with Sparse Parameters` | learning mixtures of low-rank models; dermatological disease classification |
| `Bootstrap methods another look at the jackknife Efron` | parsimony jackknifing; UFBoot2 |
| `positronium formation lifetime pH dependence aqueous solution` | MOFs; polymer packaging; LDH composites |

The pattern is consistent: distinctive *topic* words work, exact *titles* do not, and pre-1995 works fare worst.

## Repro 2 — no DOI → metadata lookup

There is no tool to ask "what does this DOI resolve to?". `download_paper` accepts a `doi` as a fallback for fetching a file, but nothing returns metadata for a DOI.

OpenAlex supports this directly:

```
GET https://api.openalex.org/works/https://doi.org/10.1214/aos/1176344136
  -> "Estimating the Dimension of a Model" (1978)
```

## Why it matters

The workload was verifying ~90 references before they went into a manuscript. The failure mode that actually occurred four times was **a plausible-looking DOI attached to wrong metadata**:

- a fabricated title behind a real DOI that resolved to an unrelated KEK B-factory paper
- an article number off by one, resolving to a different paper in the same journal
- a fabricated author list on two real papers
- an entry mixing the title of one paper with the journal/volume/pages of another

Catching these requires exactly the two things missing: resolve the DOI and compare, or search the exact title and compare. Both had to be done outside scitadel with `curl`. A `get_paper_by_doi` would have caught all four in one pass.

## Suggestions

1. **Exact-title path.** Route queries to `filter=title.search:` when the query looks like a title, or add an explicit `match: title|topic|auto` argument. Alternatively support quoted phrases in `query` and map them onto the title filter.
2. **`get_paper_by_doi(doi)`** — resolve DOI → full metadata, via OpenAlex `/works/doi:...` with a CrossRef fallback. Highest value per line of code for verification workloads.
3. **Field-scoped query syntax** — `author:`, `year:`, `doi:`, `title:` — so a query can be narrowed rather than reformulated by guesswork.
4. **CrossRef as a source.** It beats OpenAlex for older and non-preprint literature and is the authority for DOI registration. Several targets here were only resolvable there.
5. **Search-quality regression test.** A fixture of ~20 exact titles of well-known papers with expected top-3 containment would have caught this. Suggested seeds: Schwarz 1978 (BIC), Efron 1979 (bootstrap), Kuhn 1955 (Hungarian), Provencher 1982 (CONTIN), Beck & Teboulle 2009 (FISTA).

Point 5 matters most long-term: the current behaviour fails silently. It returns five confident-looking, plausibly-adjacent results, so an agent that does not already know the answer cannot tell a search failure from a genuine absence — and will conclude "not found" for a paper with tens of thousands of citations.

---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 12:22 AM_

Fixed in #238:
- `search` has a `--field any|title|auto` option (CLI and MCP; the default stays `any`). `title` uses OpenAlex's `title.search` filter, with filter-syntax characters such as `,` removed from the query.
- New `resolve-doi` CLI command and `resolve_doi` MCP tool.
- Follow-ups not included: a CrossRef fallback for DOI lookup, and an opt-in live-network regression test over a fixture of exact titles.

