pub mod error;
pub mod pubmed;
pub mod arxiv;
pub mod openalex;
pub mod inspire;
pub mod patentsview;
pub mod lens;
pub mod epo;
pub mod download;

use scitadel_core::ports::SourceAdapter;

/// Build adapter instances from source names.
pub fn build_adapters(
    sources: &[String],
    pubmed_api_key: &str,
    openalex_email: &str,
) -> Result<Vec<Box<dyn SourceAdapter>>, error::AdapterError> {
    build_adapters_full(sources, pubmed_api_key, openalex_email, "", "", "", "")
}

/// Build adapter instances with all credential options.
pub fn build_adapters_full(
    sources: &[String],
    pubmed_api_key: &str,
    openalex_email: &str,
    patentsview_key: &str,
    lens_token: &str,
    epo_key: &str,
    epo_secret: &str,
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
                adapters.push(Box::new(openalex::OpenAlexAdapter::new(
                    openalex_email.to_string(),
                    30.0,
                )));
            }
            "inspire" => {
                adapters.push(Box::new(inspire::InspireAdapter::new(30.0)));
            }
            "patentsview" => {
                adapters.push(Box::new(patentsview::PatentsViewAdapter::new(
                    patentsview_key.to_string(),
                    30.0,
                )));
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
