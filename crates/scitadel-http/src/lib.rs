//! Paced HTTP acquisition — ADR-007 §4, slice S1.
//!
//! This crate owns one job: getting bytes out of a publisher **without
//! getting an institution suspended**. Three pieces:
//!
//! - [`policy::BucketPolicyTable`] maps a host to a *publisher platform*
//!   bucket, because the limit that matters is the platform's. The five
//!   Elsevier hosts are one budget, not five.
//! - [`client::PacedClient`] takes **one `Request` permit per redirect hop**
//!   from its [`scitadel_core::ports::Pacer`], keyed to that hop's bucket,
//!   plus one `Work` permit on the first hop into each bucket for the work.
//! - [`headers::SafeHeaders`] makes ADR-007 §5's credential rule a property
//!   of a value: an anonymous request cannot hold a credential, a credential
//!   cannot leave its publisher's bucket, and `Debug` prints no values.
//!
//! The pacer itself is a port — [`scitadel_core::ports::Pacer`] — and the
//! SQLite ledger that implements it lives in `scitadel-db`. Nothing here
//! talks to a database.
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use scitadel_core::ports::{PaceTier, Pacer};
//! use scitadel_http::{BucketPolicyTable, FetchError, PacedClient, SafeHeaders};
//! use url::Url;
//!
//! # async fn run(pacer: Arc<dyn Pacer>) -> Result<(), FetchError> {
//! let client = PacedClient::new_default(pacer, BucketPolicyTable::new())?;
//! let response = client
//!     .get(
//!         Url::parse("https://pdf.sciencedirectassets.com/paper.pdf")
//!             .expect("a valid URL"),
//!         PaceTier::Tdm,
//!         SafeHeaders::unauthenticated(),
//!     )
//!     .await?;
//! println!("{} bytes from {}", response.content_length().unwrap_or(0), response.bucket);
//! # Ok(())
//! # }
//! ```

pub mod client;
pub mod error;
pub mod headers;
pub mod policy;
pub mod redirect;

pub use client::{MAX_REDIRECT_HOPS, PacedClient, PacedResponse, WorkScope};
pub use error::{FetchError, Result};
pub use headers::SafeHeaders;
pub use policy::{BucketPolicyTable, BucketRoute, UNKNOWN_HOST_POLICY};
pub use redirect::is_login_redirect;
