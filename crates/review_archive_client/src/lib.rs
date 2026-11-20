//! A typed async client for a running review_archive, reached through the Service-Arb panel,
//! which vouches for the caller. Requests and responses are the DTOs of
//! `review_archive_core::dto`, the same types the server serializes.
//!
//! ```no_run
//! # async fn demo() -> Result<(), review_archive_client::Error> {
//! use review_archive_client::{CapturePlace, Client, dto::{CaptureLimits, CaptureRequest}};
//!
//! let client = Client::ambient(reqwest::Client::new(), "https://sa.evinvest.ltd/api/review_archive")?;
//! let req = CaptureRequest {
//!     place: Some("ChIJLU7jZClu5kcR4PcOOO6p3I0".into()),
//!     limits: CaptureLimits { max_reviews: Some(10), ..Default::default() },
//!     ..Default::default()
//! };
//! match client.capture_place(Some(60), &req).await? {
//!     CapturePlace::Ok(job) => {
//!         for r in job.reviews.unwrap_or_default() {
//!             println!("{} {:?}", r.author, r.capture_url);
//!         }
//!     }
//!     CapturePlace::Accepted(queued) => println!("still running: GET /jobs/{}", queued.job_id),
//! }
//! # Ok(()) }
//! ```

#![warn(missing_docs)]

use std::fmt;

use reqwest::{Method, RequestBuilder, StatusCode, Url};
pub use review_archive_core::dto;

/// What went wrong with a call.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	/// The request did not complete, or its answer did not decode.
	#[error("review_archive request failed: {0}")]
	Http(#[from] reqwest::Error),
	/// The base URL is unusable, or a path leads off it.
	#[error("review_archive URL: {0}")]
	Url(String),
	/// The archive refused the request.
	#[error("review_archive answered {status} {code}: {message}")]
	Api {
		/// The status.
		status: StatusCode,
		/// The refusal's word (`not_found`, `busy`, …).
		code: String,
		/// Its `error`, for a person.
		message: String,
	},
}

impl Error {
	/// The status, for an answer from the archive.
	pub fn status(&self) -> Option<StatusCode> {
		match self {
			Self::Api { status, .. } => Some(*status),
			Self::Http(e) => e.status(),
			Self::Url(_) => None,
		}
	}
}

/// A running archive.
#[derive(Clone)]
pub struct Client {
	http: reqwest::Client,
	base: Url,
}
impl Client {
	/// A client whose HTTP client carries what authenticates: in a browser, the panel's
	/// session cookie and CSRF header.
	pub fn ambient(http: reqwest::Client, base: &str) -> Result<Self, Error> {
		let mut base: Url = base.parse().map_err(|e| Error::Url(format!("{base:?}: {e}")))?;
		if !base.path().ends_with('/') {
			let path = format!("{}/", base.path());
			base.set_path(&path);
		}
		Ok(Self { http, base })
	}

	/// `path` the document's, its params segment-encoded: under the base.
	fn request(&self, method: Method, path: &str) -> Result<RequestBuilder, Error> {
		let url = self.base.join(path.trim_start_matches('/')).map_err(|e| Error::Url(format!("{path:?}: {e}")))?;
		assert_eq!(url.origin(), self.base.origin(), "{path}: an operation's path stays on the base");
		Ok(self.http.request(method, url))
	}

	async fn send(req: RequestBuilder) -> Result<reqwest::Response, Error> {
		let resp = req.send().await?;
		let status = resp.status();
		if status.is_success() {
			return Ok(resp);
		}
		#[derive(serde::Deserialize)]
		struct Refused {
			code: String,
			error: String,
		}
		let Refused { code, error } = resp.json().await?;
		Err(Error::Api { status, code, message: error })
	}
}

impl fmt::Debug for Client {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Client").field("base", &self.base.as_str()).finish_non_exhaustive()
	}
}

include!("generated.rs");
