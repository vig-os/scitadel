pub mod arxiv;
pub mod download;
pub mod epo;
pub mod error;
pub mod inspire;
pub mod lens;
pub mod openalex;
pub mod patentsview;
pub mod pubmed;

use scitadel_core::config::OpenAlexAuth;
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
pub fn build_adapters_full(
    sources: &[String],
    pubmed_api_key: &str,
    openalex: &OpenAlexAuth,
    patentsview_key: &str,
    lens_token: &str,
    epo_key: &str,
    epo_secret: &str,
) -> Result<Vec<Box<dyn SourceAdapter>>, error::AdapterError> {
    build_adapters_with_field(
        sources,
        pubmed_api_key,
        openalex,
        patentsview_key,
        lens_token,
        epo_key,
        epo_secret,
        SearchField::default(),
    )
}

/// Same as `build_adapters_full` but lets the caller pick the OpenAlex
/// relevance mode (#210). Non-OpenAlex adapters are unaffected — their
/// search shapes don't map cleanly onto a single "field" knob.
#[allow(clippy::too_many_arguments)]
pub fn build_adapters_with_field(
    sources: &[String],
    pubmed_api_key: &str,
    openalex: &OpenAlexAuth,
    patentsview_key: &str,
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
