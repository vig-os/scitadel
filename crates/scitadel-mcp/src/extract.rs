//! PDF text extraction with a layout-aware fallback (#145).
//!
//! Why two tiers: `pdf-extract` is pure-Rust and ships in every build,
//! but it mangles two-column scientific layouts (left/right interleave),
//! drops ligatures, and breaks formula-heavy passages. Poppler's
//! `pdftotext -layout` produces materially better text on those exact
//! papers — and most macOS / Linux scientific workstations already have
//! it via `brew install poppler` / the distro `poppler-utils` package.
//!
//! We probe `pdftotext` once per process and cache the result. If
//! present, we shell out to it; if not, we fall back to `pdf-extract`.
//! No new mandatory dependency, transparent improvement when available.

use std::path::Path;
use std::sync::OnceLock;

/// Identifies which extractor produced the text. Surfaced in the MCP
/// `read_paper` response so a downstream agent can tell whether it's
/// reading layout-preserved poppler output or naive flow text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfExtractor {
    /// `pdftotext -layout` from poppler-utils. Preserves columns +
    /// reading order; recommended for 2-column scientific PDFs.
    Pdftotext,
    /// Pure-Rust `pdf-extract` crate. Always available; lossier on
    /// multi-column layouts.
    PdfExtractCrate,
}

impl PdfExtractor {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdftotext => "pdftotext",
            Self::PdfExtractCrate => "pdf-extract",
        }
    }
}

/// Extract text from a PDF, preferring `pdftotext -layout` when
/// available. Returns the text and which extractor produced it. Runs
/// in `spawn_blocking` to keep the async runtime free.
pub async fn extract_pdf_text(path: &Path) -> Result<(String, PdfExtractor), String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || extract_pdf_text_sync(&path))
        .await
        .map_err(|e| format!("pdf extract task failed: {e}"))?
}

/// Synchronous extraction core. Tier 1: `pdftotext -layout -nopgbrk
/// <pdf> -`. Tier 2: `pdf_extract::extract_text`. Never panics — every
/// failure returns a string so the MCP envelope can surface it.
fn extract_pdf_text_sync(path: &Path) -> Result<(String, PdfExtractor), String> {
    if pdftotext_available() {
        match run_pdftotext(path) {
            Ok(text) => return Ok((text, PdfExtractor::Pdftotext)),
            Err(e) => {
                // Don't fail the whole call — a malformed PDF that
                // breaks pdftotext might still parse with pdf-extract,
                // and either way a degraded extract beats no text.
                tracing::warn!(
                    error = %e,
                    "pdftotext failed; falling back to pdf-extract"
                );
            }
        }
    }
    let text = pdf_extract::extract_text(path).map_err(|e| format!("pdf extract failed: {e}"))?;
    Ok((text, PdfExtractor::PdfExtractCrate))
}

/// Cached PATH probe for `pdftotext`. The answer doesn't change over
/// the process lifetime, so we walk `$PATH` once and cache the result.
fn pdftotext_available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("pdftotext").is_file())
        })
    })
}

/// Shell out to `pdftotext -layout -nopgbrk <pdf> -`. `-layout`
/// preserves column geometry (the whole point of using poppler);
/// `-nopgbrk` strips form-feed page separators that confuse downstream
/// readers. `-` writes to stdout so we never touch a temp file.
fn run_pdftotext(path: &Path) -> Result<String, String> {
    let output = std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg("-nopgbrk")
        .arg(path)
        .arg("-")
        .output()
        .map_err(|e| format!("pdftotext spawn failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "pdftotext exited {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("pdftotext stdout was not UTF-8: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractor_names_are_stable() {
        // The strings end up in API responses and logs — pinning them
        // means renaming the enum variant doesn't silently change the
        // wire format.
        assert_eq!(PdfExtractor::Pdftotext.as_str(), "pdftotext");
        assert_eq!(PdfExtractor::PdfExtractCrate.as_str(), "pdf-extract");
    }

    #[test]
    fn pdftotext_probe_is_cached_and_idempotent() {
        // Two calls should agree (and cheaply — the cache is the point).
        let a = pdftotext_available();
        let b = pdftotext_available();
        assert_eq!(a, b);
    }
}
