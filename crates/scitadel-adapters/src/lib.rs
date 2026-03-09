pub mod error;
pub mod pubmed;
pub mod arxiv;
pub mod openalex;
pub mod inspire;

use scitadel_core::ports::SourceAdapter;

/// Build adapter instances from source names.
pub fn build_adapters(
    sources: &[String],
    pubmed_api_key: &str,
    openalex_email: &str,
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
            other => {
                return Err(error::AdapterError::UnknownSource(other.to_string()));
            }
        }
    }

    Ok(adapters)
}
