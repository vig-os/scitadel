---
type: issue
state: open
created: 2026-11-08T12:20:27Z
updated: 2026-11-09T04:56:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/scitadel/issues/323
comments: 3
labels: chore, effort:large, phase:design
assignees: none
milestone: none
projects: none
parent: 311
children: none
synced: 2026-11-09T08:50:55.512Z
---

# [Issue 323]: [spike(hub): source triage — coverage, licence, cost and tiering of literature and patent sources](https://github.com/vig-os/scitadel/issues/323)

Part of #311. Design: ADR-008 (Proposed), PR #317. Research dated 2026-11-08.

> **Amended 2026-11-09 (owner decisions 2026-11-08, ADR-008 §15–§16).** The corpus is **walled**: nothing is published on Hugging Face or anywhere else as a dataset. The second licence boundary below is now **"may serve in a paid tier"**, not "may publish on HF". Read every `HF .tsra` cell in the matrices as that boundary, with two tightenings:
> - **CC-BY-NC and CC-BY-ND content is excluded from every paid tier**, even where a cell says "YES as a separate NC product" (e.g. Reliance on Science, PMC `oa_noncomm`, arXiv CC BY-NC-ND). It may be cached and embedded internally for research use only, and it is disabled in phase 1 (#318).
> - **Text and data mining for a commercial tier relies on EU DSM Art. 4, not Art. 3.** Art. 3 covers research organisations only. Art. 4 requires honouring rightsholder opt-outs, so ingest needs an **opt-out pipeline** (H18, #330): robots.txt, TDMRep (`tdm-reservation`) headers and metadata, and publisher terms. It runs per source and per item before anything enters a paid-tier index, and the opt-out evidence is recorded in provenance. The Swiss CopA Art. 24d reading (research) doesn't allow commercial redistribution of mining results either.
>
> The row-level licence column and the source/version provenance stay. They now gate what each tier serves, rather than what a dataset card lists.

## Motivation

H1 (#314, US patents) and H2 (#315, OpenAlex) lock in the two sources the hub builds its first products from. Everything else still has to be chosen against coverage, licence, cost and the access limits of a small lab: enrichment sources, worldwide patents (H8), a medical/preprint corpus for the labs that need one, chemistry, citation graphs. The point of this spike is to make that choice once. It has to come before H4 (#318, artefact store + licence/entitlement gating) must answer "which licence class is this" for every byte the hub holds, and before H5 (#319, cascade) and H6 (#320, base + delta) are wired per source.

Two licence boundaries drive the decisions:
- **what the hub may cache + embed** internally for entitled lab users (ETH), and
- **what the hub may serve in a paid tier** (hosted personal, lab or org tiers; ADR-008 §17).

The second is strict. A paid tier is commercial use, so every item it serves has to carry a licence that permits commercial use and derivatives, and must have cleared the Art. 4 opt-out check. "Trust us, it's probably fine" isn't enough.

## Scope

Every source the hub has reason to touch, sorted into the two domains (literature; patents + enrichment). For each: coverage today, licences for internal cache and for paid-tier serving, cost, access, update cadence, quality, and overlap with the others. Then a tiering decision and a Phase 1 / Phase 2 order.

Out of scope: publisher APIs behind ADR-007 acquisition (the hub does not crawl — clients acquire and offer the artefacts to H4), and any source already decided in a shipping issue (USPTO grant XML and PatentsView PVGPATDIS are in #314; OpenAlex snapshot + API in #315).

## Matrix — literature

| Source | Coverage (2026-11-08) | Licence — cache / embed | Licence — HF `.tsra` | Cost | Access | Cadence | Quality |
|---|---|---|---|---|---|---|---|
| **OpenAlex** | ~510M works + authors/institutions/sources/concepts/topics; abstracts (inverted index), no FT; snapshot ~770 GB Parquet | CC0 | **YES (CC0)** | Free snapshot; API free $1/day key; Member $5k/yr; Member+ $10k/yr unlocks `from_updated_date` + daily snapshot; **$10/1k content downloads** | `s3://openalex/data/` (not requester-pays) + `api.openalex.org` | Snapshot quarterly (free) or daily (Member+); API continuous | Rich structured metadata; abstracts only [[pricing]](https://help.openalex.org/access/pricing/) |
| **Crossref** | ~180M DOI records (2026-03-17 public data file; +12.7M); no FT; publisher-supplied abstracts on some | Public domain-like metadata; abstracts carry publisher restrictions | **CONDITIONAL — YES for metadata graph; strip/flag abstracts** | Public data file free; REST free with polite pool; Metadata Plus paid (price not public) | Academic Torrents + AWS S3; REST `api.crossref.org` (5 req/s public, 10 polite) | Public file annual (March); REST continuous | Clean structured metadata + references; OpenAlex is downstream of it [[2026 announcement]](https://www.crossref.org/blog/2026-public-data-file-now-available/) |
| **PubMed baseline + updatefiles** | ~37M citations, 1946–present; NO full text | Free (NLM terms); attribution to NLM; "some abstracts may be protected by copyright" per NLM | **CONDITIONAL — metadata YES; abstracts grey area (publisher-owned)** | Free | `ftp.ncbi.nlm.nih.gov/pubmed/baseline/` + `/updatefiles/` | Annual baseline (year-end); daily updates | PubMed DTD XML; best MeSH indexing [[README]](https://ftp.ncbi.nlm.nih.gov/pubmed/baseline/README.txt) |
| **PMC OA — `oa_comm`** | ~3.4M OA articles overall; `oa_comm` = CC0 / CC BY / CC BY-SA / CC BY-ND subset (count unverified); JATS XML + PDF + media | Yes under per-article CC licence | **YES per-article** with licence column; publish `oa_comm` only | Free | **S3 `pmc-oa-opendata` us-east-1, no sign-in** (FTP removed week of 2026-08-24) | Continuous | JATS XML (clean); chef's kiss [[PMC Cloud]](https://pmc.ncbi.nlm.nih.gov/tools/cloud/) |
| **PMC OA — `oa_noncomm`** | CC BY-NC* subset of the above | Yes, non-commercial | **NO** — downstream HF consumers inherit NC; embeddings-only is a derived work still arguably restricted | Free | Same S3 bucket, `oa_noncomm/` prefix | Continuous | Same JATS quality |
| **Europe PMC** | ~43M abstracts, ~10M+ FT (unverified current count); extra author manuscripts beyond PMC | Per-article licence (same model as PMC OA) | Same rule as PMC OA — depends per article | Free | REST + OAI-PMH + FTP (`ftp.europepmc.org/pub/databases/pmc/oa/`, returned 403 today — unverified) | Weekly metadata; monthly PMID↔PMCID↔DOI map | JATS XML; EBI-hosted (closer to Zürich than us-east-1) [[downloads]](https://europepmc.org/downloads) |
| **arXiv (bulk S3)** | ~11 TB (Nov 2026), 1991–present; ~9.2 TB (Apr 2025) + ~100 GB/mo | Default **arXiv non-exclusive licence** — distribution-only, does NOT grant us sublicensing. Subset under CC BY / CC BY-NC-ND / CC0 (per-paper, in OAI-PMH metadata, not in the tar) | **NO for default-licensed PDFs/LaTeX**. Only CC BY / CC0 subset may be republished. Abstracts + embeddings arguably yes; **arXiv asks that any index built on the full text links back for downloads** | Requester-pays S3 us-east-1 (`s3://arxiv/pdf/`, `s3://arxiv/src/`). ~$0.09/GB egress → ~$1000 one-time full sync, ~$9/mo delta (unverified 2026 rate) | AWS creds required | Monthly new, occasional revisions | LaTeX source + PDF [[bulk S3]](https://info.arxiv.org/help/bulk_data_s3.html), [[licence]](https://info.arxiv.org/help/license/) |
| **bioRxiv + medRxiv (S3)** | ~300k biorxiv, ~80k medrxiv preprints (unverified) | Per-paper licence; mix of CC BY / CC BY-NC / CC BY-NC-ND / CC0 / "all rights reserved deposit" | **NO** for default/ARR subset; YES for CC BY/CC0 subset per record | Requester-pays S3 us-east-1 (`s3://biorxiv-src-monthly`; `s3://medrxiv-src-monthly` strongly indicated but **not confirmed on the bioRxiv TDM page**) | AWS creds | Monthly redeposit [[TDM]](https://www.biorxiv.org/tdm) | MECA packages with JATS-adjacent full text |
| **Semantic Scholar (S2ORC + Datasets API)** | ~200M S2AG records, ~12M+ parsed FT in S2ORC; monthly releases | **Likely yes for internal non-commercial research** — but current API licence requires non-commercial use + link-back; ask ETH legal | **NO — custom AI2 API licence** forbids transfer / sublicense / redistribute; overrides older ODC-BY references | Free (needs API key) | `api.semanticscholar.org` + Datasets API | Monthly dataset releases | Parsed sections + references — only aggregator in this set with uniform parsed FT [[current licence]](https://api.semanticscholar.org/license/) |
| **CORE** | Tens of millions of OA papers; older advertised ~2.1 TB; 2026 counts not announced | Historically ODC-BY on CORE's layer; currently **under review** per CORE's 2025 notice; underlying works keep their own licences (not tracked per-item) | **NO** — per-item licence not tracked at CORE's level; cannot filter safely | Dataset free under old terms; API tiered (free + paid); current terms opaque | `api.core.ac.uk/v3` + dataset download | "At least once a year" per CORE's flyer; 2026 release not found | Noisy extraction quality; long-tail institutional-repo coverage [[CORE FAQ]](https://core.ac.uk/faq) |
| **Unpaywall** | ~40M OA-status DOIs; no FT | CC0 (unverified at the source page itself; widely reported) | **MOOT — OpenAlex ingests Unpaywall; use the OpenAlex snapshot** | Lookup API free; Data Feed paid; snapshot **discontinued** | `api.unpaywall.org` (per-item); Data Feed changefiles | Snapshot discontinued; Data Feed daily/weekly | Narrow by design: OA status only [[OpenAlex help]](https://help.openalex.org/access/unpaywall/) |
| **DataCite** | ~70M+ DOIs skewed to data, software, preprints, grey lit | CC0 metadata | **YES (CC0)** | Free; monthly file needs identified user | REST + OAI-PMH + S3 monthly + annual | REST continuous; bulk monthly + annual [[public data file]](https://datacite.org/blog/announcing-datacites-first-public-data-file/) | DataCite kernel schema; completeness varies by repo |
| **OSTI (SciTech Connect / DOE PAGES)** | ~3M+ DOE-funded STI records | **Contractor/grantee manuscripts are NOT public domain** — DOE has only a government-use licence; downstream users have no right to reproduce or redistribute. Federal-employee works are PD. Metadata PD | **NO** for FT except federal-employee / explicitly open records; metadata YES | Free | REST `osti.gov/api/v1/records` | Continuous | Niche physics/energy; "public access does not connote public domain" per OSTI [[website policy]](https://www.osti.gov/opennet/website_policy) |
| **INSPIRE-HEP** | ~1.7M literature records (Mar 2025; likely ~1.8–1.9M now, unverified); HEP-focused | Metadata CC0; full-text PDFs = upstream (arXiv/publisher) licence | **YES** for metadata (CC0); NO for FT PDFs | Free | REST `inspirehep.net/api` (15 req/5s/IP); OAI-PMH; bulk dumps unverified | Continuous | Curated HEP bibliography + citation graph — value is the graph, not the FT (which duplicates arXiv) |

## Matrix — patents and enrichment

| Source | Coverage (2026-11-08) | Licence — cache / embed | Licence — HF `.tsra` | Cost | Access | Cadence | Quality |
|---|---|---|---|---|---|---|---|
| **USPTO Grant Full Text XML** (locked in by #314) | ~8M grants 1976–present; ICE DTD; abstract + description + claims + drawing descriptions | PD under 17 USC §105 (US gov publication); honour per-applicant copyright notices | **YES (PD-USGov)** with applicant-notice filter | Free | `bulkdata.uspto.gov` / `data.uspto.gov/bulkdata`; weekly | Weekly (Tue) | Best-in-class; MathML + chemistry; TIFF drawings [[product]](https://developer.uspto.gov/product/patent-grant-full-text-dataxml) |
| **USPTO PGPub Full Text XML** | Pre-grant publications 2001–present (A1/A2/A9); same ICE DTD | Same as grants (PD-USGov) + applicant notices | **YES (PD-USGov)** | Free | Same bulkdata host | Weekly (Thu) | Captures filings that never issue — disjoint from grants |
| **PatentsView PVGPATDIS** (locked in by #314) | 35 TSV tables; disambiguated inventors, assignees, locations, CPC, citations for US grants 1976–present (Q4 2025 release live) | CC BY 4.0 | **YES (CC BY 4.0)** with attribution | Free | ODP Bulk Data Directory `data.uspto.gov/bulkdata`; `patentsview.org` now 301s | Quarterly | Gold standard for US entity disambiguation [[Q4 2025 release]](https://www.uspto.gov/subscription-center/2026/patentsview-releases-q4-2025-data-update) |
| **PatentRef (third-party beta)** | US grants 1976+, weekly from USPTO; abstract + claims via API | Returned data CC BY 4.0 (per `patentref.io/LICENSES.md`); derived lapse signals CC BY 4.0; "Too Good" flag not published; rules citations fair-use excerpts | **NO** — no bulk export; stopgap replacement for the retired PatentSearch API | Free beta, 1,000 req/month, 45 req/min | `patentref.io/api/v1/*` | Weekly | Keyword matching w/o stemming; no OCR; beta exit/pricing unverified |
| **Google Patents Public Datasets (BigQuery)** | 100M+ publications worldwide; US FT; most non-US metadata-only; pre-parsed JSON | CC BY 4.0 per 2017 launch credit line (re-verify) | **YES (CC BY 4.0)** for the CC BY portions with "Google Patents Public Data by IFI CLAIMS Patent Services and Google" + CPC/WIPO attributions; non-US text follows originating office's own copyright | BigQuery compute (1 TB/mo free, ~$6.25/TB after) + GCS/HF egress | `bigquery-public-data.patents`; needs GCP billing acct | Quarterly per IFI | Pre-joined across offices — value-add over raw XML but second-hand. **Warning: `google/patents-public-data` examples repo archived read-only 2026-04-18 — re-verify data licence before relying on it** |
| **USPTO Open Data Portal — Patent File Wrapper API** | Application file-wrapper, transactions, continuity, prosecution status; not FT | PD-USGov | **YES (PD-USGov)** metadata | Free; API key via developer signup; rate limits unverified for 2026 | `data.uspto.gov` | Near-real-time | Net-new signal over bulk (examiner, continuity, prosecution) |
| **EPO DOCDB** | ~130 authorities bibliographic + abstracts; ST.36 XML | EPO T&Cs — subscriber-only; redistribution forbidden by default | **NO** (EPO licence) | 2024 shop listing: €2,700 frontfile + €9,100 backfile (first-time: both); **2026 figure unverified** | Request credentials via `patentdata@epo.org`; EPO Webshop retired 2025-04-01 [[products page]](https://www.epo.org/en/service-support/ordering/patent-information-products) | Weekly frontfile + annual backfile | Authoritative worldwide bibliographic feed |
| **EPO INPADOC** | >500M legal events across ~100 authorities; patent families | EPO T&Cs — subscriber-only | **NO** | 2024 listing: €2,700 + €9,100 backfile; 2026 unverified | Same as DOCDB | Backfile delivered annually | Essential for family dedup + lapsed/active filtering |
| **EPO EP full-text (EBD)** | EP publications 1978+ (A + B docs) | EPO T&Cs — subscriber-only | **NO** (compilation licence gates it even where national PD applies) | Flat annual service fee; 2026 unverified | EPO subscription | Weekly | Three EPO languages for titles/abstracts/claims; descriptions in filing language |
| **EPO OPS (REST)** | Live window over DOCDB/INPADOC/EP full text | EPO T&Cs — live-query only, no bulk caching for redistribution | **NO** | Free tier **4 GB/wk** registered + throttled; above that, flat annual sub (2025: €2,800; 2026 unverified) | `ops.epo.org` [[fair use]](https://www.epo.org/en/service-support/ordering/fair-use) | Continuous | Right vehicle for on-demand lookups, wrong vehicle for a published corpus |
| **WIPO PATENTSCOPE** | ~110M docs (PCT + 70+ national); FT search for PCT | T&Cs (Oct 2025) prohibit bulk acquisition, storage, redistribution, scraping; >10 searches/min from one IP = excessive | **NO** without WIPO written consent | Separate paid PCT data products exist; 2,000 CHF/yr figure is older and unverified | `patentscope.wipo.int` live only [[terms]](https://www.wipo.int/en/web/patentscope/data/terms_patentscope) | Live | Live DB only; sublicensees must pass T&Cs on |
| **Lens.org** | ~155M patents (deduped by family) + 275M scholarly works unified; built on DOCDB + USPTO + CORE | API Terms generally bar redistribution of downloaded Lens data or derivative databases that replace Lens; academic aggregate outputs usually allowed | **NO** for the data; derived embeddings = grey area (needs written clearance) | Scholarly + Commercial API tiers; stable current pricing not public; `user-agreement` URL 404'd today | `docs.api.lens.org`; token from user profile; 429 on exceed | Continuous | Mostly a reshaped DOCDB + USPTO + S2 — not uniquely needed if we have PatentsView + DOCDB + OpenAlex |
| **Reliance on Science (Marx & Fuegi)** | Patent→paper (front-page + in-text) citation links; v64 (2024-06-03) covers citations through 2023 and USPTO-only in-text through 2022; newer version banner — date unverified | CC BY-NC 4.0 (contact for commercial) | **YES as a separate CC BY-NC `.tsra`** with `NonCommercial: true`, attribution, isolated from the CC0/CC BY base | Free | Zenodo `records/11461587/latest` | Point-release | Clean patent_id ↔ (paper WOS/MAG id) with source/context flags [[v64]](https://zenodo.org/records/11461587) |
| **SureChEMBL** | ~55M chemistry-annotated compounds + patent→compound maps + biomedical NER entities | CC BY 4.0 per FAQ (directory-level LICENSE unverified) | **YES (CC BY 4.0)** with attribution to EMBL-EBI | Free | `ftp://ftp.ebi.ac.uk/pub/databases/chembl/SureChEMBL/bulk_data/` (Parquet) [[bulk]](https://chembl.gitbook.io/surechembl/downloads/bulk-data) | Every 2 weeks, full snapshot | SMILES/InChI + biomedical annotations; clean Parquet [[FAQ]](https://chembl.gitbook.io/surechembl/faq) |

## Paid-tier serving: what may NOT be served

*(This section was written against the original HF-publication bar and has been retargeted. Everything listed here also stays out of every paid tier, together with all CC-BY-NC and CC-BY-ND items and any item whose rightsholder has opted out under EU DSM Art. 4.)*

Everything the hub ingests can be **cached and embedded internally** for entitled lab users under H4's licence/entitlement gating (#318). Paid-tier serving is a stricter bar. The following must stay lab-internal, or be served only as a specific derived subset:

- **Semantic Scholar (S2AG + S2ORC)** — the current AI2 Datasets API licence forbids redistribution, resale, sublicensing, transfer. This is the single biggest lane-change from older OSS norms and supersedes the historical ODC-BY labelling.
- **CORE (full dataset)** — licence under review; per-item licence of the underlying OA works is not tracked at CORE's level, so we cannot filter safely for HF.
- **Unpaywall** — moot; OpenAlex replaces it, snapshot was discontinued.
- **EPO DOCDB, INPADOC, EP full-text (bulk), OPS** — subscriber contract forbids redistribution (whether or not ETH subscribes).
- **WIPO PATENTSCOPE** — T&Cs prohibit bulk caching and redistribution.
- **Lens.org** — API Terms forbid redistribution of downloaded Lens data or replacement databases.
- **PMC OA `oa_noncomm` subset** — NC licences don't cleanly travel to HF as a public dataset; cache and embed internally only (or publish as a separate NC product with that restriction tagged).
- **arXiv default-licensed majority** — full text / PDFs / LaTeX source are NOT redistributable under arXiv's non-exclusive licence. Only the CC BY / CC0 subset may be republished; abstracts + embeddings go on HF with link-back to arXiv.
- **bioRxiv/medRxiv default + ARR subset** — same model as arXiv: lab-internal full text; publish only the CC BY / CC0 subset + embeddings.
- **OSTI contractor manuscripts (full text)** — not public domain, only a government-use licence. Metadata is fine.
- **PubMed publisher-supplied abstracts** — grey; NLM itself flags "some abstracts may be protected by copyright". ETH legal to decide; default to metadata-only on HF.
- **Reliance on Science derivatives in the main base** — must live in a separate CC BY-NC `.tsra`, not merged into the CC0/CC BY patent base.

Every `.tsra` product carries a licence column at row level (H5 pipeline; one of H4's licence classes), the source and version per row, the attribution strings, and the Art. 4 opt-out check result. Paid-tier `get` / `search` filter on these columns, and nothing is published as a dataset (walled corpus).

## Tiering

**TIER A — backbone, bulk, open, ingest first.**
- USPTO Grant Full Text XML (#314) — PD-USGov.
- USPTO PGPub Full Text XML — PD-USGov. **Fold into #314** as a second product sharing the H5 cascade; captures filings that never issue.
- PatentsView PVGPATDIS (#314) — CC BY 4.0.
- OpenAlex snapshot subset + free-key sync (#315) — CC0.
- **PMC OA `oa_comm` subset** — the biomedical full-text backbone, JATS XML, per-article CC0/CC BY/CC BY-SA/CC BY-ND. Natural first biomedical corpus because it reuses the H5 JATS path that the patent XML path already needs.
- **SureChEMBL** — CC BY 4.0, bulk Parquet, every 2 weeks; drops straight into the patent corpus as a chemistry enrichment product.
- **DataCite** — CC0 metadata; small, publishable; fills the data/software/preprint DOI gap OpenAlex doesn't always resolve.

**TIER B — fills gaps or adds enrichment.**
- **Europe PMC** — same per-article licence model as PMC OA, extra author manuscripts outside PMC; preferred route if EBI egress to Zürich beats AWS us-east-1.
- **bioRxiv + medRxiv** — preprint coverage; cache internally; publish only CC BY/CC0 + embeddings.
- **arXiv (bulk S3)** — cache internally (one-time ~$1000 egress for ~11 TB plus ~$9/mo); publish only embeddings + abstracts + link-back (arXiv's explicit redistribution rule).
- **Google Patents Public Datasets (BigQuery)** — worldwide text source for H8; CC BY 4.0 portions are HF-publishable, but re-verify the dataset's current licence notice given the examples-repo archival on 2026-04-18.
- **Crossref** — fills metadata OpenAlex hasn't ingested yet, canonical references lists, DOI resolution; publish the bibliographic graph on HF, strip abstracts.
- **PubMed baseline + updatefiles** — metadata backbone for the biomedical side; MeSH indexing is unique. Publish metadata; defer abstract redistribution until ETH legal clears it.
- **Reliance on Science** — patent→paper link enrichment; ships as a separate CC BY-NC `.tsra`.

**TIER C — per-request or live API only.**
- EPO OPS — live lookups within the 4 GB/week free tier; not a source for the published corpus.
- USPTO ODP File Wrapper API — net-new prosecution-history signal; fetch per-record on query.
- PatentRef — third-party beta already in flight (#268 stopgap).
- INSPIRE-HEP — API only; metadata is CC0 and useful for HEP-focused labs; skip the FT (duplicates arXiv).
- OSTI — metadata API only; no FT redistribution path.

**REJECT** (for the published corpus; not even cached without a licence event):
- Semantic Scholar — AI2 licence blocks redistribution; internal use needs ETH legal sign-off.
- CORE — licence under review + untraced per-item licences.
- Unpaywall — subsumed by OpenAlex.
- WIPO PATENTSCOPE — T&Cs forbid bulk.
- Lens.org — redistribution forbidden; no unique data over PatentsView + DOCDB + OpenAlex.
- EPO DOCDB / INPADOC / EP bulk / EP full-text — only reconsider if ETH subscribes **and** we're willing to run as subscriber-only (never on HF). Open question below.

## Ingest order

**Phase 1 (US-first, free/open only).** The two issues already filed drive this — triage adds three siblings and merges one into #314:

1. #314 — USPTO grant XML + PVGPATDIS. **Add PGPub XML as a second product under this issue** (same ICE DTD, disjoint document kinds, same cascade).
2. #315 — OpenAlex snapshot subset + free-key subscriptions.
3. **PMC OA `oa_comm`** — biomedical full-text backbone. New sibling issue (call it H9).
4. **SureChEMBL** — chemistry enrichment over #314's patent base. New sibling issue (H10).
5. **DataCite + Crossref metadata** — small, fast metadata enrichment layer under H4's licence classes. New sibling issue (H11).

**Phase 2 (worldwide, mixed-licence, higher budget).** This is where H8 (worldwide patents, being filed in parallel) draws its sources from, and where the lab-internal + public-subset split is decided per source:

6. **Google Patents Public Datasets (BigQuery)** — worldwide patent text for H8; re-verify CC BY notice at the dataset first.
7. **EPO DOCDB / INPADOC** — only if ETH subscribes; lab-internal only, never on HF.
8. **Europe PMC** — if EBI egress is better than us-east-1, promote to Phase 1 and replace PMC OA with it.
9. **bioRxiv + medRxiv** — biomedical preprint layer; cache FT, publish CC BY/CC0 subset.
10. **arXiv** — budget-approved one-time egress; cache FT, publish embeddings + abstracts + link-back.
11. **Reliance on Science** — separate CC BY-NC `.tsra` for patent→paper links.
12. **PubMed baseline + updatefiles** — citation layer (metadata on HF; abstracts pending legal).
13. **USPTO ODP File Wrapper API + EPO OPS + INSPIRE-HEP** — live per-record fallbacks via H3 (#316), not ingested in bulk.

## Open questions for the owner

1. **OpenAlex membership at ETH.** Is ETH Library already an OpenAlex Member ($5k/yr, $20/day) or Member+ ($10k/yr, $100/day + `from_updated_date` sync + daily snapshot)? #315's design works on the free $1/day key but Member+ removes the lookback-window gap. [[membership blog]](https://blog.openalex.org/?p=3946)
2. **EPO bulk subscription at ETH.** Is ETH a PATLIB centre, or does it hold an EPO DOCDB / INPADOC subscription? If yes, we can internal-cache for entitled users (never HF). If no, is this worth €11.8k + ongoing fees (2024 figure; 2026 unverified) vs. building worldwide from Google Patents BQ (CC BY)?
3. **Semantic Scholar — internal use at ETH.** The AI2 Datasets API licence permits non-commercial internal use but forbids redistribution. Does ETH legal clear "lab hub serves entitled lab users" as "internal non-commercial research"? Drives whether S2ORC's parsed full text is available for Clef/Kev training signals (#246) even without HF publication.
4. **PubMed publisher-supplied abstracts.** NLM flags that some are copyright-restricted. Does ETH legal treat this as safe to redistribute on HF under the NLM terms, or metadata-only?
5. **arXiv budget.** ~$1000 one-time + ~$9/mo egress for a full ~11 TB sync out of us-east-1 (unverified 2026 rate). Co-locating in us-east-1 or via CloudFront changes the number. Approve, defer, or stay abstracts-only?
6. **Google Patents BQ reliability.** `google/patents-public-data` examples repo archived read-only on 2026-04-18. Is this administrative or a signal that the dataset is being deprecated? Needs to be checked before Phase 2 depends on it.
7. **PMC OA NC subset.** Internal caching + embedding of the `oa_noncomm` subset is permitted under NC. ETH legal to confirm that embeddings derived from NC text are themselves OK to publish on HF or not.
8. **Reliance on Science version.** Zenodo shows a newer version than v64; the newer date wasn't visible in today's fetch. Needs a direct check before the Phase 2 sibling lands.

## Acceptance

- [ ] This matrix + tiering is linked from ADR-008 (PR #317) or a subsequent amendment as the current source triage.
- [ ] #314 is extended to include USPTO PGPub XML as a second product under its H1 scope (or a new sub-issue files that).
- [ ] Three new Phase-1 sibling issues exist: H9 (PMC OA `oa_comm`), H10 (SureChEMBL), H11 (DataCite + Crossref metadata).
- [ ] The eight open questions above have a tracked answer (comment on this issue, or a linked issue each).
- [ ] H4 (#318) references this triage as the source of its licence classes; H5 (#319) references the per-source JATS/XML/Parquet mix this triage settles.

## Dependencies

- Informational for H0 (#313), H1 (#314), H2 (#315), H3 (#316); drives per-source scope for H4 (#318), H5 (#319), H6 (#320), H7 (#321) and H8 (worldwide, filed in parallel).
- No code, no infra. Research artefact; sibling issues under #311 do the work.


---

# [Comment #1]() by [gerchowl]()

_Posted on November 8, 2026 at 12:21 PM_

Re-checked unverified item 10 from vm-dev (2026-11-08): `bulkdata.uspto.gov` does not resolve here either (curl: no response), while `data.uspto.gov`, `api.uspto.gov` and `developer.uspto.gov` do. This is not a network quirk on the research side: the legacy bulk host appears to be retired. Full-text grant and pre-grant XML should be pulled from the Open Data Portal's Bulk Data Directory on `data.uspto.gov` (the same place PVGPATDIS moved to). #314 should target the ODP bulk API, not bulkdata.uspto.gov.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 8, 2026 at 12:40 PM_

## Aggregators and additional open sources

Research dated 2026-11-08. Columns match #323's matrix; one row per source. Citations in-line; items I could not verify from a primary doc today are marked **unverified** and listed again at the end.

### Literature + citation / identity aggregators

| Source | Coverage (2026-11-08) | Licence — cache / embed | Licence — HF `.tsra` | Cost | Access | Cadence | Format | Overlap |
|---|---|---|---|---|---|---|---|---|
| **OpenAIRE Graph** | Publications, datasets, software, orgs, projects, relations, communities; whole-graph dump v10.5.0 = 321.3 GB (2025-09-12, Zenodo 17098012); metadata only, no FT | CC BY 4.0 | **YES (CC BY 4.0)** with attribution | Free | Zenodo [`10.5281/zenodo.17098012`](https://zenodo.org/records/17098012) + delta records in the OpenAIRE Research Graph Zenodo community | Point-release ~2×/yr; Scholix link dump CC0 | tar.gz of gzipped JSONL, schema `10.5281/zenodo.14608526` | Downstream of Crossref/DataCite; carries FP7/H2020/HE funding linkage OpenAlex loses |
| **CORE** | Tens of millions of OA papers; 2024 the last confirmed dataset year; 2025/2026 dataset release **unverified** | Older dumps ODC-BY on CORE's layer; newer datasets paid for non-members; per-item underlying licence **not tracked** at CORE's level | **NO** — cannot filter per item | Older ODC-BY dumps free; newer datasets paid; free API for Supporting/Sustaining members | [`core.ac.uk/services/dataset`](https://core.ac.uk/services/dataset) + `api.core.ac.uk/v3` | "At least once a year"; 2025/2026 cadence unverified | CSV, JSON; FT as separate files | Long-tail institutional repos; noisy extraction |
| **OpenCitations Index + Meta** | **2.56B citations** in Index (dump 2026-07-28); Meta = bibliographic metadata for everything in the Index (dump 2026-06-27); no FT | CC0 across all datasets | **YES (CC0)** | Free | [`download.opencitations.net`](https://download.opencitations.net/); REST + SPARQL at `api.opencitations.net`; Zenodo/figshare mirrors | Index dumps 1–6 months; Meta similar; REST continuous | CSV, N-Triples, Scholix, RDF, QLever/Virtuoso DB; **no Parquet** | Citation graph; superset of Crossref refs (adds DataCite Event Data + NIH-OCC) |
| **Retraction Watch (via Crossref)** | Retractions, expressions of concern, corrections; count unverified for 2026 | **Licence not explicitly named** on the GitLab repo or [Crossref's doc page](https://www.crossref.org/documentation/retrieve-metadata/retraction-watch/); Crossref's [2023 announcement](https://retractionwatch.com/2023/09/12/the-retraction-watch-database-becomes-completely-open-and-rw-becomes-far-more-sustainable/) said "completely open and freely available"; Crossref's site-wide notice is CC BY 4.0 for website content | **CONDITIONAL — treat as CC BY 4.0 "data via Crossref / Retraction Watch" until Crossref states otherwise**; file a licence-clarification ticket before HF publication | Free | `gitlab.com/crossref/retraction-watch-data` (git clone); also per-DOI via Crossref REST | Every working day | CSV; multi-value fields semicolon-separated | QC-critical flag layer — nothing else carries it |
| **DOAJ** | ~20k+ OA journal records; article records count unverified for 2026 | **Article + journal metadata CC0** (per [DOAJ terms](https://doaj.org/terms/)) | **YES (CC0)** | Free; Premium Metadata Service (daily) paid | [public-data-dump](https://doaj.org/docs/public-data-dump/) (full JSON, application by email to `dominic@doaj.org`); `doaj.org/api/` REST; journal CSV + OAI-PMH free | Full dumps monthly (public); daily (paid); journal CSV refreshes every 60 min | JSON (dumps); CSV; OAI-PMH XML | Authoritative OA-journal filter over Crossref/OpenAlex |
| **BASE** | ~320–400M records across 10,700+ providers (count unverified — BASE site blocked by Anubis today) | API IP-allowlisted + non-commercial by default; redistribution forbidden | **NO** — bulk harvest / redistribution forbidden | Free for non-commercial after IP allowlist | `api.base-search.net/` (request + use-case) | Continuous | XML via API; OAI-PMH per upstream provider | Union over OpenAIRE + CORE + OAI-PMH endpoints — no uniquely-owned data |
| **DBLP** | CS bibliography (Dagstuhl); record count unverified for 2026 | CC0 1.0 since 2019-11 ([dblp blog](https://blog.dblp.org/tag/license)) | **YES (CC0)** | Free | XML dump + DTD on dblp.org; monthly DOI'd snapshots on DROPS (e.g. [`10.4230/dblp.xml.2026-07-01`](https://drops.dagstuhl.de/entities/artifact/10.4230/dblp.xml.2026-07-01)) | Monthly snapshots; daily dumps | Single XML file + DTD | OpenAlex/Crossref overlap, with sharper author disambiguation for CS |
| **ACL Anthology** | NLP/CL FT archive; per-paper count for 2026 unverified | 2016+: **CC BY 4.0**; pre-2016: CC BY-NC-SA 3.0 ([copyright](https://aclanthology.org/info/contrib/)) | **YES for the 2016+ CC BY subset**; pre-2016 NC-SA cache-internal only | Free | Bulk BibTeX `anthology.bib.gz`; sharded `anthology-N.bib`; per-paper BibTeX/MODS XML; XML schema in `acl-org/acl-anthology` GitHub under `data/xml/`; per-paper PDF URLs | Rolling (per-event) | PDF + XML + BibTeX | arXiv for same preprints; OpenAlex for the records |
| **NASA ADS** | Astrophysics abstracts + metadata; no FT hosting | API terms; redistribution right **unverified** | **NO / DEFER** — no CC0/CC-BY statement, no bulk dump, API terms forbid actions that risk system stability | Free API key; higher limits by `adshelp@cfa.harvard.edu` | API only ([`api.adsabs.harvard.edu`](https://api.adsabs.harvard.edu); [rate limits](https://adsabs.github.io/help/policies/rate-limits), 5000 req/day/token) | Continuous (API) | JSON via API | OpenAlex / arXiv astro-ph / Crossref |
| **RePEc** | Economics federation (IDEAS, EconPapers, EDIRC, LogEc, CitEc) | Public and freely accessible per [IDEAS getdata](https://ideas.repec.org/getdata.html), but **no uniform open licence** at the federation level; each publisher archive sets its own terms | **NO / DEFER** — no federation-level licence | Free | Per-archive ReDIF harvest (`remi`); API discouraged for bulk | Per-publisher continuous | ReDIF (plain-text templates) | OpenAlex/Crossref overlap for econ papers |
| **OAPEN / DOAB** | Directory of OA Books + OA Books Library; counts unverified | Metadata CC0 at both; book files per-title CC licence [[DOAB metadata]](https://www.doabooks.org/en/doab/metadata-harvesting-and-content-dissemination) [[OAPEN metadata]](https://oapen.org/article/metadata) | **YES per-item** for CC0/CC-BY titles; exclude NC/ND; CC-BY-SA cache-internal (SA propagation complicates HF) | Free | DOAB OAI-PMH `directory.doabooks.org/oai/`; OAPEN OAI-PMH `library.oapen.org/oai/` (xoai exposes all fields); bulk ONIX 3.0 / MARCXML / CSV / RIS; KBART | Continuous | ONIX, MARCXML, OAI-DC, CSV, RIS, KBART, JSON; PDF/EPUB per title | Books orthogonal to journals |
| **Software Heritage** | Billions of source files + graph edges across archived projects | Bulk-access terms shift per-component licence responsibility to the user; graph dataset reported CC BY 4.0 third-party ([Zenodo 2583978](https://zenodo.org/record/2583978)) — SWH primary doc **blocked by Anubis** today | **NO** for source code; **DEFER** for the graph dataset pending primary-doc re-verification | Free | ORC files via S3 (AWS Open Data) + public Amazon Athena instance; [graph docs](https://docs.softwareheritage.org/devel/swh-dataset/graph/) ([terms, mirror](https://mirror.softwareheritage.enea.it/legal/access)) | Point-release snapshots (irregular) | Apache ORC (graph); raw source objects via SWH API | Orthogonal to literature + patents |
| **ORCID Public Data File** | Researcher identity + affiliations + works list for public ORCID records; 2024 file on Figshare confirmed; 2025 file referenced on Dimensions BigQuery page but **URL/size unverified** | CC0 — "ORCID does not impose restrictions or conditions on use" ([2024 Figshare](https://orcid.figshare.com/articles/dataset/ORCID_Public_Data_File_2024/27151305)) | **YES (CC0)** | Free | Annual tarball on ORCID's Figshare account; Dimensions BigQuery mirrors (CC0) | Annual (autumn) | XML (2024: summaries + 11 activity archives, `.tar.gz`); 2025 structure unverified | OpenAlex authors/institutions overlap; ORCID is identity-primary |

### Preprint / general repositories

| Source | Coverage (2026-11-08) | Licence — cache / embed | Licence — HF `.tsra` | Cost | Access | Cadence | Format | Overlap |
|---|---|---|---|---|---|---|---|---|
| **ChemRxiv** (Cambridge Open Engage) | ~14,564 FT preprints per [COAR](https://doapr.coar-repositories.org/repositories/chemrxiv/) (stale); 2026 total **unverified** | Per-preprint CC licence (CC BY / CC BY-NC / CC BY-NC-ND) | **YES per-item for CC-BY subset only**; strip NC/ND | Free | Cambridge Open Engage public API ([docs page returned 403 today](https://chemrxiv.org/engage/chemrxiv/public-api/documentation)); COAR directory shows no OAI URL | Continuous | PDF + supplementary; JSON metadata (format unverified) | OpenAlex / Crossref metadata; chemistry FT unique |
| **OSF Preprints** | ~2,097,029 metadata records across PsyArXiv, SocArXiv, EarthArXiv, engrXiv, LawArXiv, MetaArXiv, etc. ([COAR](https://doapr.coar-repositories.org/repositories/osf-preprints/)) | Per-preprint: **CC0 1.0** or **CC BY 4.0** (community services may offer others) | **YES per-item (CC0/CC-BY)** | Free | OSF API v2 base `https://api.osf.io/v2/preprints/` ([developer.osf.io](https://developer.osf.io/)); OAI-PMH endpoint **unverified** | Continuous | PDF, Word; JSON over REST | OpenAlex / DataCite; unique social-sciences FT |
| **HAL** (CCSD/CNRS) | Millions of deposits across all disciplines; 2026 total unverified | Metadata CC0; file-level licence per depositor. From **January 2026** licence field is mandatory on new file deposits ([CCSD 2026-02](https://www.ccsd.cnrs.fr/en/2026/02/no-hal-deposit-without-licence-the-essential-information-you-need-to-know/)); legacy files mostly have no clear licence (>75% per CCSD) | **YES (CC0) for metadata graph**; FT CC-BY/CC0 subset only — legacy no-licence files excluded until the Jan-2026 field propagates | Free | OAI-PMH `https://api.archives-ouvertes.fr/oai/hal/` (portal-scoped under `/oai/<portal>/`), [docs](https://api.archives-ouvertes.fr/docs/oai); search API [docs](https://api.archives-ouvertes.fr/docs); SWORD for deposit | Continuous; SWORD import production release scheduled end of 2026 | TEI (native); OAI-DC and others; PDF for file bodies | OpenAlex / Crossref / arXiv cross-deposits; unique French-research FT |
| **Research Square** (In Review) | ~88,442 FT preprints ([COAR](https://doapr.coar-repositories.org/repositories/research-square/)) | Editorial policy: **every preprint is CC BY 4.0** (In Review refuses incompatible licences; corroborated by [Wikipedia](https://en.wikipedia.org/wiki/Research_Square) and the [request-api page](https://www.researchsquare.com/request-api)) | **YES (CC BY 4.0 across the corpus)** | Free (request API form) | Request JSON API via `https://www.researchsquare.com/request-api`; metadata also via Crossref; OAI-PMH URL blank in COAR; bulk-harvest rate limits **unverified** | Continuous | Author-side docx/RTF/LaTeX; JSON metadata | Crossref / Europe PMC; unique "manuscripts in review" |
| **Preprints.org** (MDPI) | ~21,880 records per COAR (stale); 2026 total unverified | Stated policy: **every preprint is CC BY 4.0** ([about](https://www.preprints.org/about); fetched 403, relying on COAR + search) | **YES (CC BY 4.0) in principle** | Free | Metadata via Crossref; **no documented API, OAI-PMH or bulk download**; TDM terms not published | Continuous | PDF; metadata via Crossref | Crossref / OpenAlex |
| **Zenodo** (CERN) | >6M entries across data, publications, software, posters, presentations ([OERSI 2025-11-29](https://oersi.org/pages/de/blog/2025/11/29/zenodo); not an authoritative Zenodo-hosted count) | **Metadata CC0** per [developer docs](https://developers.zenodo.org/); file licence per depositor | **YES per-record** for CC0/CC-BY subset; metadata YES (CC0) | Free | REST `https://zenodo.org/api/` (30 req/min since [2025-11-25](https://blog.zenodo.org/2025/11/25/2025-11-14-search-api-updates/)); OAI-PMH `https://zenodo.org/oai2d`; **monthly dumps via `/api/exporter`** (most-recent 3 kept); token auth | REST continuous; OAI-PMH incremental; dumps monthly | DataCite XML, JSON, oai_dc, MARC21, DCAT; file bodies as uploaded | DataCite shares DOI backbone; OpenAIRE builds on Zenodo; unique long-tail data/software/grey-lit |

### Swiss institutional repositories

| Source | Coverage (2026-11-08) | Licence — cache / embed | Licence — HF `.tsra` | Cost | Access | Cadence | Format | Overlap |
|---|---|---|---|---|---|---|---|---|
| **EPFL Infoscience** | DSpace-CRIS; OAI earliest datestamp 2004-08-31 (unverified total) | Metadata **CC0 1.0** per [EPFL help](https://help-infoscience.epfl.ch/export-reuse.html); per-record file licence "if specified, such as Creative Commons"; EPFL-community-restricted items excluded from external use ([charter](https://www.epfl.ch/campus/library/services-researchers/infoscience-en/charter-deposit-licence-and-conditions-of-use/)) | **YES for CC0 metadata graph**; FT per-item CC-BY/CC0 subset only | Free; no stated rate limits | OAI-PMH `https://infoscience.epfl.ch/server/oai/openaire4` (verbs + `from`/`until`, verified today); REST `/server/api`; formats `oai_dc`, `marcxml`, `oai_openaire` | Continuous | DC XML, MARCXML, OpenAIRE CERIF XML | OpenAlex / Crossref (DOIs); arXiv for EPFL preprints |
| **ZORA** (UZH) | DSpace 7+, migrated from EPrints Oct 2025 per [apis.io](https://apis.io/providers/university-of-zurich/) (third-party, unverified from UZH) | **Unverified** — direct terms page blocked by Anubis bot-protection today | **Unverified** — needs ETH-network/direct contact | Free (presumed) | OAI-PMH `/oai/request`; REST `/server/api` — both listed by third-party only | Unverified | DSpace DC/METS/OpenAIRE presumed | OpenAlex / Crossref (UZH outputs) |
| **BORIS Portal** (UniBe) | DSpace 7.6.1 + CRIS; portal launched 2024-09-02; migration from legacy BORIS Publications "not yet complete"; OAI earliest datestamp 2026-08-10 (OAI index recently rebuilt — likely does NOT expose the full historical corpus yet) | [UniBe library](https://ub.unibe.ch/services/open_science/boris_portal/index_eng.html) states research data CC + CC0, documentation CC0; publication-record licence not stated | **CONDITIONAL YES** for research-data + CC0-documentation subset only; publications: DEFER per-record | Free | OAI-PMH `https://boris-portal.unibe.ch/server/oai/request` (Identify confirmed live today) | Unverified cadence | DSpace DC/METS/OpenAIRE | OpenAlex / Crossref (UniBe outputs); Zenodo overlap for data deposits |
| **SERVAL** (UNIL) | UNIL IR; 2026 total unverified | SERVAL deposit policy removes infringing items on proof; preservation indefinite; **explicit reuse licence not found** | **DEFER** — per-item terms not surfaced | Free (presumed) | Historical OAI `https://serval.unil.ch/oaiprovider` returned **HTTP 410 Gone** today; `wp.unil.ch/infoserval` "Harvesting of publications (OAI-PMH)" returned 404. Current URL **unverified** | Unverified | Unverified | OpenAlex / Crossref (UNIL outputs) |

**ETH Research Collection** — coordinator-verified facts incorporated by reference (DSpace 7.6; OAI-PMH at `https://www.research-collection.ethz.ch/server/oai/request` and REST at `/server/api` respond; off-network callers hit "Too Many Requests … access temporarily restricted" after ~4 requests, with instructions to use the ETH network or contact `research-collection@library.ethz.ch`). Not re-polled here.

---

## PHASE 1 LICENCE-CLEAN SCOPE

Owner decision 2026-11-08: BUILD FIRST on a strictly licence-clean corpus, then approach ETH Library with a working demo. No licensed/paywalled caches, no grey-area sources, no scraping, in phase 1. Within that envelope phase 1 goes for **maximum depth** (full text, figures, tables, cross-links, embeddings for as much as we can fit), not minimum viable.

**Only sources whose content is CC0, CC-BY or public-domain PER ITEM, with per-item filterability, and whose access terms allow bulk harvest, go in:**

1. **USPTO Grant Full Text XML** — PD-USGov, via ODP Bulk Data Directory on `data.uspto.gov` (per the top comment); weekly. Full text, drawings, MathML; 100% PD.
2. **USPTO PGPub Full Text XML** — PD-USGov; weekly; same path and schema; folded into #314's scope (captures filings that never issue).
3. **PatentsView PVGPATDIS** — CC BY 4.0; quarterly; US disambiguated inventors / assignees / locations / CPC / citations.
4. **OpenAlex quarterly snapshot subset + free-key sync** — CC0; ~770 GB Parquet; abstracts (inverted index), no FT.
5. **PMC OA `oa_comm` packages** — JATS XML + tables (MathML) + original figure files; filtered per article to CC0 / CC BY only (exclude CC BY-SA / CC BY-ND when SA/ND propagation complicates HF; cache-internal otherwise). Biomedical FT backbone.
6. **SureChEMBL** — CC BY 4.0; every 2 weeks; SMILES/InChI + patent→compound maps, biomedical NER.
7. **DataCite** — CC0 metadata; monthly + annual; DOIs for data / software / preprints / grey lit.
8. **Crossref public data file + REST** — public-domain-like metadata graph + references; publish bibliographic graph only, **strip publisher-supplied abstracts**.
9. **OpenAIRE Graph** — CC BY 4.0; Zenodo v10.5.0 (321 GB); EU funding linkage (FP7 / H2020 / HE) + Scholix CC0.
10. **OpenCitations Index + Meta** — CC0; 2.56B citations (2026-07-28); strictly more open-citation coverage than Crossref's own.
11. **DOAJ** — CC0 article + journal metadata; monthly full dump (application) + daily journal CSV.
12. **DBLP** — CC0; monthly DOI'd XML snapshots on DROPS; sharper CS author disambiguation.
13. **ACL Anthology, 2016+ CC BY subset** — CC BY 4.0; metadata XML in `acl-org/acl-anthology` + `anthology.bib.gz` + per-paper PDFs. Pre-2016 NC-SA cache-internal only.
14. **DOAB / OAPEN** — CC0 metadata + per-title CC0/CC-BY book FT subset (exclude NC/ND; SA cache-internal).
15. **ORCID Public Data File** — CC0; annual on Figshare; researcher identity + affiliations + works list.
16. **OSF Preprints, CC0/CC-BY subset** — PsyArXiv / SocArXiv / EarthArXiv / engrXiv / LawArXiv / MetaArXiv full text filtered per preprint.
17. **HAL metadata** — CC0 across the board; TEI; OAI-PMH. **HAL file-level FT subset** — only files declaring CC-BY/CC0 after the Jan-2026 mandatory-licence rollout; legacy no-licence files excluded until licence field propagates.
18. **Research Square** — editorial policy CC BY 4.0 across the corpus; INCLUDE subject to confirming the request-API permits bulk harvest.
19. **Zenodo, CC0/CC-BY per-record subset** — metadata CC0; monthly dumps via `/api/exporter`; OAI-PMH + REST (30 req/min). Lowest-friction phase-1 add after OpenAlex and PMC OA.
20. **ChemRxiv, CC-BY subset only** — INCLUDE subject to re-verifying the public-API endpoint and TDM rate limits (FAQ 403'd today).
21. **arXiv CC-BY/CC0 subset** — LaTeX **source** via bulk S3 for papers whose per-paper licence (in OAI-PMH metadata, not in the tar) is CC-BY/CC0: perfect equations, tables, often-vector figures. Non-redistributable majority stays out of phase 1.
22. **bioRxiv + medRxiv CC-BY/CC0 subset** — MECA packages (JATS + figures), filtered per paper.
23. **Europe PMC CC-BY/CC0 subset** — same per-article licence model as PMC OA; EBI egress likely closer to Zürich than us-east-1 for CH users.
24. **INSPIRE-HEP metadata** — CC0; HEP citation graph (not FT — duplicates arXiv).
25. **Retraction Watch via Crossref** — must-have QC flag layer. Licence footnote: treat as **CC BY 4.0 "data via Crossref / Retraction Watch"** until Crossref states otherwise; file a licence-clarification ticket before HF publication.

**Owner-addendum phase-1 shape (2026-11-08) — folded into the above:**
- **Structured sources first (T0: cheap, best quality):** PMC OA `oa_comm` packages (JATS tables/MathML + original figure files); arXiv LaTeX SOURCE via bulk S3 for CC-BY/CC0-licensed papers only (verify per-paper licence applies to the source, not just the PDF); bioRxiv/medRxiv bulk JATS + figures for CC-BY items; US patent XML (100% PD); ACL / OAPEN / DOAB / Zenodo CC-BY items. The GPU-heavy extraction (Docling / Marker / math OCR) is reserved for PDF-only items.
- **Crossref TDM-link harvester:** filter Crossref metadata to CC-BY/CC0 licence URLs and follow the text-mining full-text links politely through scitadel's paced per-publisher buckets (ADR-007). Main non-biomed lever for ETH fields: MDPI, Frontiers, Copernicus, IOP, APS (PRX etc.), Springer Nature OA, IEEE Access, ACS AuthorChoice CC-BY. Verify Crossref exposes `license` + `link[intended-application=text-mining]` and note any publisher terms.
- **HF as cold store for the licence-clean public corpus:** the full corpus (esp. figure and patent-drawing products) lives on HF and is read by HTTP range (tessera#777); anvil keeps indexes (BM25/ANN/graph), metadata, text blocks and the hot set. Sizing *(estimates)*: ~5–10M licence-clean papers × (text ~8 KB + tables ~5 KB + figures ~200 KB JXL + vectors ~30 KB) ≈ 1–2.5 TB; US patent drawings ≈ 0.5–1.5 TB; this exceeds anvil's 2 TB 990 PRO, hence HF cold + an anvil hot set.
- **Embedding levels:** work, section, chunk, figure (Qwen3-VL), table (serialised + caption → text embedding), independent claim, graph node. GPU budget *(estimates)*: ~250M text chunks ≈ 1–2 days on the 5090; ~100M+ figures/drawings with the 2B VL embedder ≈ days–weeks → staged (hot topics first, the rest at `priority=low`).

### DEFERRED until the ETH Library conversation

- **ETH Research Collection bulk harvest** — the coordinator-verified facts show off-network rate-limiting; harvest needs a Library agreement, not a cold-start API scrape.
- **CORE full text** — licence under review + per-item licence of underlying works not tracked at CORE's level.
- **Semantic Scholar / S2ORC** — AI2 Datasets API licence blocks redistribution; non-commercial internal use needs ETH legal clearance.
- **Unlicensed bronze / green OA full text** — any paper that is free-to-read but carries no explicit CC licence falls into Art. 24d's scope and needs the lab's legal reading before being cached.
- **Paywalled caches** — anything behind a publisher paywall, via any vehicle (library IP, SUSHI, OpenAthens, …) is phase-2 by definition.
- **EPO paid bulk (DOCDB / INPADOC / EP full text)** — subscriber contract forbids redistribution; cache-internal even then.
- **Swiss institutional repositories** (EPFL Infoscience, ZORA, BORIS Portal, SERVAL) — only EPFL's CC0 metadata is phase-1-eligible in principle, but it is subsumed by OpenAlex's EPFL coverage; the others are either bot-blocked (ZORA), migrated/incomplete (BORIS Portal, SERVAL endpoint 410) or need a direct library contact. All four roll up into the ETH-Library conversation as Swiss repository federation.
- **NASA ADS, RePEc, Software Heritage (source code), Preprints.org, BASE** — see table above: each fails on licence, access channel or terms. Reconsider on a per-source basis if a specific need appears in phase 2.

**What phase 2 unlocks, in a single ETH Library conversation (not before):**
- a Research Collection harvest agreement;
- the lab's reading of **CopA Art. 24d** for internal text/data mining of free-to-read works (covers unlicensed bronze/green);
- a publisher **local-hosting addendum** negotiated through the **Consortium of Swiss Academic Libraries** (which touches EPO bulk, publisher paid caches, and Swiss repository federation);
- **COUNTER 5.1 / SUSHI** usage reporting plumbing, designed in #318 but disabled in phase 1.

### Retraction flags — a must-have

Retraction Watch is treated as a QC input, not an optional enrichment: every record the hub holds that has a matching retraction-flag entry (by DOI or PMID) is surfaced with its retraction status and date on `search` and `get` responses, and search result rendering says so. This applies to the phase-1 licence-clean corpus — flags are joined at index-build time, not at query time.

### Unverified facts worth flagging

- Retraction Watch data licence not explicitly named on the GitLab repo or Crossref's Retraction Watch doc page; treat as CC BY 4.0 pending Crossref clarification.
- CORE 2025/2026 dataset release — 2024 remains the last confirmed year; services subpages returned 403.
- OAI-PMH endpoint URLs for ChemRxiv and OSF Preprints — COAR left blank; ChemRxiv's public-API docs page returned 403 today.
- HAL legacy-file licence coverage after Jan 2026 — CCSD says >75% currently have no clear licence; practical CC-BY/CC0 subset size unverified.
- Software Heritage graph-dataset licence — CC BY 4.0 appears only on third-party Zenodo mirrors; SWH primary docs blocked by Anubis.
- ZORA licence + terms — Anubis bot-protection on landing + `/information.html` blocked polite fetches; needs ETH-network/direct contact.
- SERVAL current OAI endpoint — old URL returned 410 Gone; migrated or retired, URL unknown.
- Preprints.org (MDPI) API / OAI-PMH / bulk — none documented in public routes; a direct MDPI enquiry would be needed for any bulk use.
- NASA ADS redistribution right — no explicit statement anywhere; DEFER rather than REJECT.
- ORCID 2025 Public Data File — referenced on Dimensions BigQuery page but URL / size / schema not re-confirmed from ORCID's own Figshare.


---

# [Comment #3]() by [gerchowl]()

_Posted on November 9, 2026 at 04:56 AM_

Amended for the 2026-11-08 owner decisions (walled corpus, nerdmachines repo home, licence-gated phase 1, NC/ND excluded from paid tiers, EU DSM Art. 4 opt-out). Source: triage-matrix Cluster E; ADR-008 batch on #317.

