//! Who is calling: the person the conductor vouches for in the assertion it signs on every
//! request it forwards ([`sa_auth`]), found or made here by its `sub`, or the conductor itself.
//! What a person may do is their `sa:review_archive:*` permissions, as the route's gate checks.

use std::sync::Arc;

use axum::{extract::FromRequestParts, http::request::Parts};
use ev_lib::api::{OperationInput, Refusal};
use review_archive::store::people::{Claim, Seen};
use review_archive_core::PersonId;
use sa_auth::{Gate, Principal, User, Vouch};

use crate::http::{AppState, Code};

/// The person a request is from, as the archive knows them.
#[derive(Clone, Copy, Debug)]
pub struct Caller(pub PersonId);

/// The assertion, then the person it names: claimed in the gate, so no handler's reads include it.
impl<F: Sync> Vouch<F> for AppState {
	type Refusal = Code;

	const REFUSALS: &'static [Code] = &[Code::Unauthenticated, Code::Unclaimable, Code::Internal];

	async fn vouch(&self, _: &Gate<F>, parts: &mut Parts) -> Result<Principal, Refusal<Code>> {
		let user = match self.verifier.principal(parts)? {
			Principal::User(user) => user,
			other => return Ok(other),
		};
		let seen = Seen {
			sub: &user.sub,
			email: &user.email,
			email_verified: user.email_verified,
			name: &user.name,
		};
		match self.archive.person(&seen).await? {
			Claim::Person(person) => {
				parts.extensions.insert(Caller(person));
				Ok(Principal::User(user))
			}
			Claim::Refused(why) => {
				tracing::error!(email = user.email, sub = user.sub, %why, "a sign-in matches an address's rows it may not claim");
				Err(Refusal::new(
					Code::Unclaimable,
					format!("records under {} wait for their owner, and this sign-in cannot claim them: {why}. Ask an admin.", user.email),
				))
			}
		}
	}
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
	type Rejection = std::convert::Infallible;

	async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
		Ok(*parts.extensions.get::<Caller>().expect("the gate's vouch puts the person of the user it admits"))
	}
}

impl OperationInput for Caller {}

/// The conductor's assertion on this request, as it came: what the conductor takes back as the
/// caller's say for the Places buys the request makes (`sa_places::Vouched`).
pub struct Vouched(pub String);

impl<S: Send + Sync> FromRequestParts<S> for Vouched {
	type Rejection = std::convert::Infallible;

	async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
		let token = parts.headers.get(sa_auth::HEADER).expect("the route's gate admitted the request by it");
		Ok(Self(token.to_str().expect("the gate verified it: base64url and dots").to_owned()))
	}
}

impl OperationInput for Vouched {}

impl sa_core::prices::Payer for Caller {
	type Who = PersonId;

	fn payer(&self) -> PersonId {
		self.0
	}
}

/// `serve --dev-member`: a request without an assertion gets one for `user`, signed by a key made
/// here and held by nothing else, so a local dashboard goes through the same gate as the conductor's.
#[derive(Clone)]
pub struct Dev {
	signer: Arc<sa_auth::Signer>,
	user: User,
}

impl Dev {
	pub fn new(user: User) -> Self {
		use base64::{Engine, engine::general_purpose::STANDARD};
		let seed: [u8; 32] = rand::random();
		let signer = format!("dev:{}", STANDARD.encode(seed)).parse().expect("a kid and a 32-byte seed");
		Self { signer: Arc::new(signer), user }
	}

	/// The gate's keys: only this one's.
	pub fn verifier(&self) -> sa_auth::Verifier {
		sa_auth::Verifier::new(self.signer.public().parse().expect("a Signer's public key parses"), sa_auth::Service::ReviewArchive)
	}

	pub(crate) async fn sign(self, mut req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
		if !req.headers().contains_key(sa_auth::HEADER) {
			let now = jiff::Timestamp::now().as_second();
			let assertion = sa_auth::Assertion {
				aud: sa_auth::Service::ReviewArchive,
				principal: Principal::User(self.user.clone()),
				method: req.method().to_string(),
				path: req.uri().path().to_owned(),
				exp: now + sa_auth::TTL,
			};
			let token = self.signer.sign(&assertion).parse().expect("a JWS is base64url and dots");
			req.headers_mut().insert(sa_auth::HEADER, token);
		}
		next.run(req).await
	}
}
