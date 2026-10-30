//! Who is calling: the person the Service-Arb panel vouches for, in the assertion it signs on
//! every request it forwards ([`sa_auth`]), checked with its public keys
//! (`PANEL_ASSERTION_KEYS`). What they may do is its `sa:review_archive:*` permissions; who
//! they are here is a person found or made by its `sub`.

use std::{
	collections::HashMap,
	sync::Mutex,
	time::{Duration, Instant},
};

use axum::{
	Json,
	extract::{FromRequestParts, Request, State},
	http::{StatusCode, request::Parts},
	middleware::Next,
	response::{IntoResponse, Response},
};
use review_archive::store::people::{Claim, Seen};
use review_archive_core::{
	PersonId,
	dto::{ErrorBody, MEMBER_HEADER},
};
use sa_auth::{Permission, PermissionSet, Service};

use crate::http::AppState;

/// Who a request is from.
#[derive(Clone, Debug)]
pub struct Caller {
	pub person: PersonId,
	pub email: String,
	pub permissions: PermissionSet,
}

/// Whose word a caller is taken on.
pub enum Auth {
	/// The panel's.
	Panel(sa_auth::Keys),
	/// `serve --dev-member`: every request is this one person, for a local dashboard.
	Dev { person: PersonId, email: String, permissions: PermissionSet },
}

/// Test-posts to Telegram, one per person per [`TEST_GAP`].
pub struct TestPosts(Mutex<HashMap<PersonId, Instant>>);

const TEST_GAP: Duration = Duration::from_secs(60);

impl TestPosts {
	pub fn new() -> Self {
		Self(Mutex::new(HashMap::new()))
	}

	/// Whether `person` may test-post now; if so, the next waits [`TEST_GAP`].
	pub fn take(&self, person: PersonId) -> bool {
		let mut last = self.0.lock().expect("held for a lookup only, never across a panic");
		let now = Instant::now();
		last.retain(|_, at| now.duration_since(*at) < TEST_GAP);
		match last.contains_key(&person) {
			true => false,
			false => {
				last.insert(person, now);
				true
			}
		}
	}
}

/// Carries a viewer's token on Grafana's fetches to the API.
pub const VIEWER_HEADER: &str = "x-ra-viewer";

/// Grafana holds no identity: each query a viewer's browser sends it has a token written into
/// it, standing for that viewer on GETs, until Grafana answers the query.
pub(crate) struct Viewers(Mutex<HashMap<String, Caller>>);

/// A [`Viewers`] token, forgotten when dropped.
pub(crate) struct Viewer<'a> {
	viewers: &'a Viewers,
	pub(crate) token: String,
}

impl Viewers {
	pub(crate) fn new() -> Self {
		Self(Mutex::new(HashMap::new()))
	}

	pub(crate) fn mint(&self, caller: Caller) -> Viewer<'_> {
		let token: String = rand::random::<[u8; 32]>().iter().map(|b| format!("{b:02x}")).collect();
		let fresh = self.0.lock().expect("held for a lookup only, never across a panic").insert(token.clone(), caller).is_none();
		assert!(fresh, "256 random bits do not repeat");
		Viewer { viewers: self, token }
	}

	fn get(&self, token: &str) -> Option<Caller> {
		self.0.lock().expect("held for a lookup only, never across a panic").get(token).cloned()
	}
}

impl Drop for Viewer<'_> {
	fn drop(&mut self) {
		let minted = self.viewers.0.lock().expect("held for a lookup only, never across a panic").remove(&self.token);
		assert!(minted.is_some(), "only its Viewer removes a token");
	}
}

fn refuse(status: StatusCode, why: &str) -> Response {
	(status, Json(ErrorBody { error: why.into() })).into_response()
}

fn unauthorized() -> Response {
	refuse(StatusCode::UNAUTHORIZED, "unauthorized")
}

/// Every authenticated route: puts the [`Caller`] in the request.
pub async fn authenticate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
	if let Some(token) = req.headers().get(VIEWER_HEADER) {
		let caller = token.to_str().ok().and_then(|t| state.viewers.get(t));
		return match (req.method() == axum::http::Method::GET, caller) {
			(true, Some(caller)) => {
				req.extensions_mut().insert(caller);
				next.run(req).await
			}
			_ => unauthorized(),
		};
	}
	let seen = match &*state.auth {
		Auth::Dev { person, email, permissions } => {
			req.extensions_mut().insert(Caller {
				person: *person,
				email: email.clone(),
				permissions: permissions.clone(),
			});
			return next.run(req).await;
		}
		Auth::Panel(keys) => {
			let Some(token) = req.headers().get(sa_auth::HEADER).and_then(|v| v.to_str().ok()) else {
				return unauthorized();
			};
			let now = jiff::Timestamp::now().as_second();
			match sa_auth::verify(keys, token, Service::ReviewArchive, req.method().as_str(), req.uri().path(), now) {
				Ok(a) => a,
				Err(e) => {
					tracing::warn!(%e, "an assertion refused");
					return unauthorized();
				}
			}
		}
	};
	let claim = state
		.archive
		.person(&Seen {
			sub: &seen.sub,
			email: &seen.email,
			email_verified: seen.email_verified,
			name: &seen.name,
		})
		.await;
	let person = match claim {
		Ok(Claim::Person(p)) => p,
		Ok(Claim::Refused(why)) => {
			tracing::error!(email = seen.email, sub = seen.sub, %why, "a sign-in matches an address's rows it may not claim");
			return refuse(
				StatusCode::FORBIDDEN,
				&format!("records under {} wait for their owner, and this sign-in cannot claim them: {why}. Ask an admin.", seen.email),
			);
		}
		Err(e) => return crate::http::ApiError::from(e).into_response(),
	};
	req.extensions_mut().insert(Caller {
		person,
		email: seen.email,
		permissions: seen.permissions,
	});
	next.run(req).await
}

async fn require(p: impl Permission, req: Request, next: Next) -> Response {
	match req.extensions().get::<Caller>().expect("layered under authenticate").permissions.may(p) {
		true => next.run(req).await,
		false => refuse(StatusCode::FORBIDDEN, &format!("this route needs {}", p.as_str())),
	}
}

/// The archive's own routes: targets, scans, captures, hooks, stats, export.
pub async fn operates_archive(req: Request, next: Next) -> Response {
	require(sa_auth::Archive::Operate, req, next).await
}

/// The members' list: whom one may act as.
pub async fn acts_as(req: Request, next: Next) -> Response {
	require(sa_auth::Members::ActAs, req, next).await
}

/// A member's balance change.
pub async fn grants_tokens(req: Request, next: Next) -> Response {
	require(sa_auth::Tokens::Grant, req, next).await
}

/// Whose `/me` this is: the caller, or the person `X-Member` names: the caller again, or anyone
/// for one who may act as others.
pub struct Member(pub PersonId);

impl FromRequestParts<AppState> for Member {
	type Rejection = Response;

	async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
		let caller = parts.extensions.get::<Caller>().expect("layered under authenticate");
		let Some(named) = parts.headers.get(MEMBER_HEADER) else {
			return Ok(Self(caller.person));
		};
		let id = named
			.to_str()
			.ok()
			.and_then(|v| v.trim().parse::<i64>().ok())
			.map(PersonId)
			.ok_or_else(|| refuse(StatusCode::BAD_REQUEST, "X-Member is not a person id"))?;
		if id == caller.person {
			return Ok(Self(id));
		}
		if !caller.permissions.may(sa_auth::Members::ActAs) {
			return Err(refuse(StatusCode::FORBIDDEN, "acting as another member needs sa:review_archive:members:act_as"));
		}
		state.archive.check_person(id).await.map_err(|e| crate::http::ApiError::from(e).into_response())?;
		Ok(Self(id))
	}
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
	type Rejection = std::convert::Infallible;

	async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
		Ok(parts.extensions.get::<Caller>().expect("layered under authenticate").clone())
	}
}
