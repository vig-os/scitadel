use std::path::{Path, PathBuf};

use scitadel_core::models::{Paper, doi_to_filename, validate_doi};

use crate::error::AdapterError;

/// The format of a downloaded paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadFormat {
    Pdf,
    Html,
}

impl std::fmt::Display for DownloadFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pdf => write!(f, "PDF"),
            Self::Html => write!(f, "HTML"),
        }
    }
}

/// Access level inferred from the downloaded content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessStatus {
    /// Full article content visible (OA PDF, or licensed-IP HTML).
    FullText,
    /// Abstract visible, body paywalled.
    Abstract,
    /// Hard paywall — only access options / login visible.
    Paywall,
    /// Heuristic couldn't classify.
    Unknown,
}

impl std::fmt::Display for AccessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FullText => write!(f, "full text"),
            Self::Abstract => write!(f, "abstract only"),
            Self::Paywall => write!(f, "paywall"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Result of a successful paper download.
#[derive(Debug)]
pub struct DownloadResult {
    pub doi: String,
    pub path: PathBuf,
    pub format: DownloadFormat,
    /// Where the file was sourced from (e.g. "unpaywall", "publisher").
    pub source: String,
    pub bytes: usize,
    pub access: AccessStatus,
}

/// Heuristic: classify publisher HTML as full-text / abstract / paywall.
///
/// Works for the common English-language publisher templates (Elsevier, Springer,
/// Wiley, ACM, IEEE, Nature, Science). Not every publisher normalizes the same way,
/// so when in doubt we return `Unknown` rather than misreporting.
pub fn detect_access_status(html: &str) -> AccessStatus {
    let lower = html.to_lowercase();

    let paywall_phrases = [
        "purchase access",
        "buy this article",
        "buy article",
        "subscribe to access",
        "subscribe to read",
        "subscribe to download",
        "get access to this article",
        "access this article",
        "obtain full access",
        "access options",
        "institutional sign in",
        "institutional login",
        "sign in to view full",
        "sign in to download",
        "subscribe for unlimited",
        "unlock this article",
    ];
    let paywall_hits = paywall_phrases
        .iter()
        .filter(|p| lower.contains(*p))
        .count();

    let full_text_markers = [
        "id=\"references\"",
        "id=\"bibliography\"",
        "id=\"acknowledgments\"",
        "id=\"acknowledgements\"",
        "class=\"references",
        "class=\"bibliography",
        ">references</h",
        ">acknowledgments</h",
        ">acknowledgements</h",
        ">supplementary information<",
        "<article",
    ];
    let full_text_hits = full_text_markers
        .iter()
        .filter(|p| lower.contains(*p))
        .count();

    let long_body = html.len() > 50_000;

    if paywall_hits >= 2 && full_text_hits == 0 {
        AccessStatus::Paywall
    } else if full_text_hits >= 2 && long_body {
        AccessStatus::FullText
    } else if paywall_hits > 0 {
        AccessStatus::Abstract
    } else if full_text_hits > 0 && long_body {
        AccessStatus::FullText
    } else {
        AccessStatus::Unknown
    }
}

/// Downloads papers by DOI using Unpaywall (OA PDFs) with publisher HTML fallback.
pub struct PaperDownloader {
    client: reqwest::Client,
    email: String,
}

impl PaperDownloader {
    pub fn new(email: String, timeout_secs: f64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs_f64(timeout_secs))
            .user_agent("scitadel/0.1 (paper downloader)")
            .build()
            .expect("failed to build HTTP client");
        Self { client, email }
    }

    /// Download by DOI only — kept for the CLI `download <doi>` path.
    /// Tries Unpaywall first, then publisher HTML fallback.
    pub async fn download(
        &self,
        doi: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        if !validate_doi(doi) {
            return Err(AdapterError::Validation(format!("invalid DOI: {doi}")));
        }

        let normalized = scitadel_core::models::normalize_doi(doi);

        tokio::fs::create_dir_all(output_dir).await.map_err(|e| {
            AdapterError::Io(format!(
                "failed to create output dir {}: {e}",
                output_dir.display()
            ))
        })?;

        match self.try_unpaywall(&normalized, output_dir).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::info!(doi = %normalized, error = %e, "Unpaywall lookup failed, falling back to publisher");
            }
        }

        self.download_publisher_html(&normalized, output_dir).await
    }

    /// Download a paper using every identifier available on the `Paper` record.
    ///
    /// Priority:
    /// 1. `arxiv_id` → direct arXiv PDF (no API call, always OA)
    /// 2. `openalex_id` → OpenAlex `/works` API for `best_oa_location.pdf_url`
    /// 3. `doi` → Unpaywall (existing path)
    /// 4. `url` → download the landing page as HTML (last resort)
    pub async fn download_paper(
        &self,
        paper: &Paper,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        tokio::fs::create_dir_all(output_dir).await.map_err(|e| {
            AdapterError::Io(format!(
                "failed to create output dir {}: {e}",
                output_dir.display()
            ))
        })?;

        let stem = file_stem_for(paper);

        if let Some(id) = paper.arxiv_id.as_deref().filter(|s| !s.is_empty()) {
            match self.try_arxiv(id, &stem, output_dir).await {
                Ok(r) => return Ok(r),
                Err(e) => tracing::info!(arxiv_id = %id, error = %e, "arxiv fallback failed"),
            }
        }

        if let Some(id) = paper.openalex_id.as_deref().filter(|s| !s.is_empty()) {
            match self.try_openalex(id, &stem, output_dir).await {
                Ok(r) => return Ok(r),
                Err(e) => tracing::info!(openalex_id = %id, error = %e, "openalex fallback failed"),
            }
        }

        if let Some(doi) = paper.doi.as_deref().filter(|s| validate_doi(s)) {
            let normalized = scitadel_core::models::normalize_doi(doi);
            match self.try_unpaywall(&normalized, output_dir).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    tracing::info!(doi = %normalized, error = %e, "unpaywall fallback failed")
                }
            }
            match self.download_publisher_html(&normalized, output_dir).await {
                Ok(r) => return Ok(r),
                Err(e) => {
                    tracing::info!(doi = %normalized, error = %e, "publisher html fallback failed")
                }
            }
        }

        if let Some(url) = paper.url.as_deref().filter(|s| !s.is_empty()) {
            return self.download_url_as_html(url, &stem, output_dir).await;
        }

        Err(AdapterError::NotFound(
            "no arxiv_id, openalex_id, doi, or url to try".into(),
        ))
    }

    /// Query Unpaywall for an open-access PDF URL and download it.
    async fn try_unpaywall(
        &self,
        doi: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        let url = format!("https://api.unpaywall.org/v2/{}?email={}", doi, self.email);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| AdapterError::Network(format!("Unpaywall API request failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AdapterError::Network(format!(
                "Unpaywall returned status {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AdapterError::Parse(format!("Unpaywall JSON parse failed: {e}")))?;

        // Extract the best OA PDF URL
        let pdf_url = body
            .get("best_oa_location")
            .and_then(|loc| loc.get("url_for_pdf"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                AdapterError::NotFound("no open-access PDF found via Unpaywall".into())
            })?;

        tracing::info!(doi = %doi, pdf_url = %pdf_url, "found OA PDF via Unpaywall");

        // Download the PDF
        let pdf_resp = self
            .client
            .get(pdf_url)
            .send()
            .await
            .map_err(|e| AdapterError::Network(format!("PDF download failed: {e}")))?;

        if !pdf_resp.status().is_success() {
            return Err(AdapterError::Network(format!(
                "PDF download returned status {}",
                pdf_resp.status()
            )));
        }

        let bytes = pdf_resp
            .bytes()
            .await
            .map_err(|e| AdapterError::Network(format!("failed to read PDF bytes: {e}")))?;

        let filename = format!("{}.pdf", doi_to_filename(doi));
        let path = output_dir.join(&filename);
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| AdapterError::Io(format!("failed to write {}: {e}", path.display())))?;

        Ok(DownloadResult {
            doi: doi.to_string(),
            path,
            format: DownloadFormat::Pdf,
            source: "unpaywall".into(),
            bytes: bytes.len(),
            access: AccessStatus::FullText,
        })
    }

    /// Resolve a DOI to its publisher page and download the HTML.
    async fn download_publisher_html(
        &self,
        doi: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        let doi_url = format!("https://doi.org/{doi}");

        let resp = self
            .client
            .get(&doi_url)
            .send()
            .await
            .map_err(|e| AdapterError::Network(format!("DOI resolution failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AdapterError::Network(format!(
                "publisher page returned status {}",
                resp.status()
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AdapterError::Network(format!("failed to read HTML: {e}")))?;

        let access = std::str::from_utf8(&bytes)
            .map(detect_access_status)
            .unwrap_or(AccessStatus::Unknown);

        let filename = format!("{}.html", doi_to_filename(doi));
        let path = output_dir.join(&filename);
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| AdapterError::Io(format!("failed to write {}: {e}", path.display())))?;

        Ok(DownloadResult {
            doi: doi.to_string(),
            path,
            format: DownloadFormat::Html,
            source: "publisher".into(),
            bytes: bytes.len(),
            access,
        })
    }

    /// Try the direct arXiv PDF URL. Free, no API call, always full-text.
    async fn try_arxiv(
        &self,
        arxiv_id: &str,
        stem: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        let pdf_url = arxiv_pdf_url(arxiv_id);
        let resp = self
            .client
            .get(&pdf_url)
            .send()
            .await
            .map_err(|e| AdapterError::Network(format!("arXiv fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AdapterError::Network(format!(
                "arXiv returned status {}",
                resp.status()
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AdapterError::Network(format!("failed to read arXiv PDF: {e}")))?;

        let path = output_dir.join(format!("{stem}.pdf"));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| AdapterError::Io(format!("failed to write {}: {e}", path.display())))?;

        Ok(DownloadResult {
            doi: arxiv_id.to_string(),
            path,
            format: DownloadFormat::Pdf,
            source: "arxiv".into(),
            bytes: bytes.len(),
            access: AccessStatus::FullText,
        })
    }

    /// Query OpenAlex `/works/{id}` and download its `best_oa_location.pdf_url`.
    async fn try_openalex(
        &self,
        openalex_id: &str,
        stem: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        let id = openalex_id.trim_start_matches("https://openalex.org/");
        let api_url = format!("https://api.openalex.org/works/{id}");

        let resp = self
            .client
            .get(&api_url)
            .send()
            .await
            .map_err(|e| AdapterError::Network(format!("OpenAlex API failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AdapterError::Network(format!(
                "OpenAlex returned status {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AdapterError::Parse(format!("OpenAlex JSON parse failed: {e}")))?;

        let pdf_url = body
            .get("best_oa_location")
            .and_then(|loc| loc.get("pdf_url"))
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                body.get("open_access")
                    .and_then(|oa| oa.get("oa_url"))
                    .and_then(serde_json::Value::as_str)
            })
            .ok_or_else(|| AdapterError::NotFound("OpenAlex reports no OA location".into()))?
            .to_string();

        let file_resp = self
            .client
            .get(&pdf_url)
            .send()
            .await
            .map_err(|e| AdapterError::Network(format!("OA URL fetch failed: {e}")))?;

        if !file_resp.status().is_success() {
            return Err(AdapterError::Network(format!(
                "OA URL returned status {}",
                file_resp.status()
            )));
        }

        let is_pdf = pdf_url.to_lowercase().ends_with(".pdf")
            || file_resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| ct.contains("application/pdf"));

        let bytes = file_resp
            .bytes()
            .await
            .map_err(|e| AdapterError::Network(format!("failed to read OA bytes: {e}")))?;

        let (ext, format, access) = if is_pdf {
            ("pdf", DownloadFormat::Pdf, AccessStatus::FullText)
        } else {
            let access = std::str::from_utf8(&bytes)
                .map(detect_access_status)
                .unwrap_or(AccessStatus::Unknown);
            ("html", DownloadFormat::Html, access)
        };

        let path = output_dir.join(format!("{stem}.{ext}"));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| AdapterError::Io(format!("failed to write {}: {e}", path.display())))?;

        Ok(DownloadResult {
            doi: openalex_id.to_string(),
            path,
            format,
            source: "openalex".into(),
            bytes: bytes.len(),
            access,
        })
    }

    /// Last resort: fetch whatever URL the paper has, save as HTML.
    async fn download_url_as_html(
        &self,
        url: &str,
        stem: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| AdapterError::Network(format!("URL fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AdapterError::Network(format!(
                "URL returned status {}",
                resp.status()
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AdapterError::Network(format!("failed to read bytes: {e}")))?;

        let access = std::str::from_utf8(&bytes)
            .map(detect_access_status)
            .unwrap_or(AccessStatus::Unknown);

        let path = output_dir.join(format!("{stem}.html"));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| AdapterError::Io(format!("failed to write {}: {e}", path.display())))?;

        Ok(DownloadResult {
            doi: url.to_string(),
            path,
            format: DownloadFormat::Html,
            source: "url".into(),
            bytes: bytes.len(),
            access,
        })
    }
}

/// Build the direct arXiv PDF URL. Handles `2005.07866`, `2005.07866v1`, full URLs.
fn arxiv_pdf_url(id_or_url: &str) -> String {
    let id = id_or_url
        .trim_start_matches("https://arxiv.org/abs/")
        .trim_start_matches("http://arxiv.org/abs/")
        .trim_start_matches("https://arxiv.org/pdf/")
        .trim_start_matches("http://arxiv.org/pdf/")
        .trim_end_matches(".pdf");
    format!("https://arxiv.org/pdf/{id}.pdf")
}

/// Locate an already-downloaded file for this paper. Returns the path if the
/// expected `.pdf` or `.html` exists under `papers_dir`.
pub fn find_cached_file(paper: &Paper, papers_dir: &Path) -> Option<PathBuf> {
    let stem = file_stem_for(paper);
    for ext in ["pdf", "html"] {
        let path = papers_dir.join(format!("{stem}.{ext}"));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Pick a safe filename stem preferring DOI, then arxiv, then openalex, then the paper's UUID.
pub fn file_stem_for(paper: &Paper) -> String {
    if let Some(doi) = paper.doi.as_deref().filter(|s| validate_doi(s)) {
        return doi_to_filename(&scitadel_core::models::normalize_doi(doi));
    }
    if let Some(id) = paper.arxiv_id.as_deref().filter(|s| !s.is_empty()) {
        return sanitize_filename(&format!("arxiv_{id}"));
    }
    if let Some(id) = paper.openalex_id.as_deref().filter(|s| !s.is_empty()) {
        return sanitize_filename(&format!("openalex_{id}"));
    }
    sanitize_filename(paper.id.as_str())
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_obvious_paywall() {
        let html = r#"<html><body>
            <h1>Access options</h1>
            <button>Purchase access</button>
            <button>Institutional sign in</button>
            </body></html>"#;
        assert_eq!(detect_access_status(html), AccessStatus::Paywall);
    }

    #[test]
    fn classifies_full_text_with_references() {
        let body = "<p>body</p>".repeat(5000);
        let html = format!(
            r#"<html><body>
            <article>{body}</article>
            <section id="references"><h2>References</h2></section>
            <section id="acknowledgments"><h2>Acknowledgments</h2></section>
            </body></html>"#
        );
        assert_eq!(detect_access_status(&html), AccessStatus::FullText);
    }

    #[test]
    fn abstract_only_when_paywall_with_some_content() {
        let html = r#"<html><body>
            <h1>Abstract</h1>
            <p>Some abstract text.</p>
            <div class="paywall">
              <button>Access this article</button>
            </div>
            </body></html>"#;
        assert_eq!(detect_access_status(html), AccessStatus::Abstract);
    }

    #[test]
    fn unknown_when_no_markers() {
        let html = "<html><body><p>hello world</p></body></html>";
        assert_eq!(detect_access_status(html), AccessStatus::Unknown);
    }

    #[test]
    fn arxiv_pdf_url_from_bare_id() {
        assert_eq!(
            arxiv_pdf_url("2005.07866"),
            "https://arxiv.org/pdf/2005.07866.pdf"
        );
    }

    #[test]
    fn arxiv_pdf_url_with_version() {
        assert_eq!(
            arxiv_pdf_url("2005.07866v1"),
            "https://arxiv.org/pdf/2005.07866v1.pdf"
        );
    }

    #[test]
    fn arxiv_pdf_url_from_abs_url() {
        assert_eq!(
            arxiv_pdf_url("http://arxiv.org/abs/2005.07866v1"),
            "https://arxiv.org/pdf/2005.07866v1.pdf"
        );
    }

    #[test]
    fn arxiv_pdf_url_idempotent_on_pdf_url() {
        assert_eq!(
            arxiv_pdf_url("https://arxiv.org/pdf/2005.07866v1.pdf"),
            "https://arxiv.org/pdf/2005.07866v1.pdf"
        );
    }
}
