use std::path::{Path, PathBuf};

use scitadel_core::models::{validate_doi, doi_to_filename};

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

/// Result of a successful paper download.
#[derive(Debug)]
pub struct DownloadResult {
    pub doi: String,
    pub path: PathBuf,
    pub format: DownloadFormat,
    /// Where the file was sourced from (e.g. "unpaywall", "publisher").
    pub source: String,
    pub bytes: usize,
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

    /// Download a paper by DOI into `output_dir`.
    ///
    /// Tries Unpaywall for an open-access PDF first, then falls back to
    /// fetching the publisher landing page HTML via DOI resolution.
    pub async fn download(
        &self,
        doi: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        if !validate_doi(doi) {
            return Err(AdapterError::Validation(format!("invalid DOI: {doi}")));
        }

        let normalized = scitadel_core::models::normalize_doi(doi);

        // Ensure output directory exists
        tokio::fs::create_dir_all(output_dir).await.map_err(|e| {
            AdapterError::Io(format!("failed to create output dir {}: {e}", output_dir.display()))
        })?;

        // Try Unpaywall first for OA PDF
        match self.try_unpaywall(&normalized, output_dir).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::info!(doi = %normalized, error = %e, "Unpaywall lookup failed, falling back to publisher");
            }
        }

        // Fallback: resolve DOI to publisher page and download HTML
        self.download_publisher_html(&normalized, output_dir).await
    }

    /// Query Unpaywall for an open-access PDF URL and download it.
    async fn try_unpaywall(
        &self,
        doi: &str,
        output_dir: &Path,
    ) -> Result<DownloadResult, AdapterError> {
        let url = format!(
            "https://api.unpaywall.org/v2/{}?email={}",
            doi, self.email
        );

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
        tokio::fs::write(&path, &bytes).await.map_err(|e| {
            AdapterError::Io(format!("failed to write {}: {e}", path.display()))
        })?;

        Ok(DownloadResult {
            doi: doi.to_string(),
            path,
            format: DownloadFormat::Pdf,
            source: "unpaywall".into(),
            bytes: bytes.len(),
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

        let filename = format!("{}.html", doi_to_filename(doi));
        let path = output_dir.join(&filename);
        tokio::fs::write(&path, &bytes).await.map_err(|e| {
            AdapterError::Io(format!("failed to write {}: {e}", path.display()))
        })?;

        Ok(DownloadResult {
            doi: doi.to_string(),
            path,
            format: DownloadFormat::Html,
            source: "publisher".into(),
            bytes: bytes.len(),
        })
    }
}
