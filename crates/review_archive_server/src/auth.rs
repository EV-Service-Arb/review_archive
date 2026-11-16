//! Who is calling: the person the Service-Arb panel vouches for, in the assertion it signs on
//! every request it forwards ([`sa_auth`]), checked with its public keys
//! (`PANEL_ASSERTION_KEYS`). What they may do is its `sa:review_archive:*` permissions; who
//! they are here is a person found or made by its `sub`. The conductor's assertion names no one,
//! and opens only its own reads.

use axum::{
	Json,
	extract::{FromRequestParts, Request, State},
	http::{StatusCode, request::Parts},
	middleware::Next,
	response::{IntoResponse, Response},
};
use ev_lib::api::{
	GenContext, OperationInput,
	openapi::{HeaderStyle, Operation, Parameter, ParameterData, ParameterSchemaOrContent, ReferenceOr, SchemaObject},
	schemars::json_schema,
};
use review_archive::store::people::{Claim, Seen};
use review_archive_core::{
	PersonId,
	dto::{ErrorBody, MEMBER_HEADER},
};
use sa_auth::{PermissionSet, Service};

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

fn refuse(status: StatusCode, why: &str) -> Response {
	(status, Json(ErrorBody { error: why.into() })).into_response()
}

fn unauthorized() -> Response {
	refuse(StatusCode::UNAUTHORIZED, "unauthorized")
}

/// The panel's assertion on `req`, checked; a refusal is answered 401.
fn asserted(keys: &sa_auth::Keys, req: &Request) -> Result<sa_auth::Assertion, Response> {
	let Some(token) = req.headers().get(sa_auth::HEADER).and_then(|v| v.to_str().ok()) else {
		return Err(unauthorized());
	};
	let now = jiff::Timestamp::now().as_second();
	sa_auth::verify(keys, token, Service::ReviewArchive, req.method().as_str(), req.uri().path(), None, now).map_err(|e| {
		tracing::warn!(%e, "an assertion refused");
		unauthorized()
	})
}

/// Every operation: puts the [`Caller`] in the request.
pub async fn authenticate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
	let seen = match &*state.auth {
		Auth::Dev { person, email, permissions } => {
			req.extensions_mut().insert(Caller {
				person: *person,
				email: email.clone(),
				permissions: permissions.clone(),
			});
			return next.run(req).await;
		}
		Auth::Panel(keys) => match asserted(keys, &req) {
			Ok(sa_auth::Assertion {
				principal: sa_auth::Principal::User(user),
				..
			}) => user,
			Ok(a) => {
				tracing::warn!(principal = ?a.principal, "an assertion names no user");
				return unauthorized();
			}
			Err(refused) => return refused,
		},
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

/// What an operation's caller holds beyond a sign-in.
#[derive(Clone, Copy, Debug)]
pub enum Section {
	/// The archive's own: targets, scans, captures, hooks, stats, export.
	Operate,
	/// The members' list: whom one may act as.
	ActAs,
}

impl Section {
	pub fn permission(self) -> &'static str {
		match self {
			Self::Operate => sa_auth::Archive::Operate.as_str(),
			Self::ActAs => sa_auth::Members::ActAs.as_str(),
		}
	}
}

/// Layered under [`authenticate`]: the caller holds the section's permission.
pub async fn require(State(section): State<Section>, req: Request, next: Next) -> Response {
	let p = section.permission();
	let held = req.extensions().get::<Caller>().expect("layered under authenticate").permissions.iter().any(|held| held == p);
	match held {
		true => next.run(req).await,
		false => refuse(StatusCode::FORBIDDEN, &format!("this route needs {p}")),
	}
}

/// Who the assertion is from, when only `admits` may call.
async fn principal(state: &AppState, admits: fn(&sa_auth::Principal) -> bool, req: Request, next: Next) -> Response {
	let Auth::Panel(keys) = &*state.auth else {
		return unauthorized(); // `--dev-member` has no conductor to hear from
	};
	match asserted(keys, &req) {
		Ok(a) if admits(&a.principal) => next.run(req).await,
		Ok(a) => {
			tracing::warn!(principal = ?a.principal, path = req.uri().path(), "a route asked by someone it does not admit");
			unauthorized()
		}
		Err(refused) => refused,
	}
}

/// `/api/conductor/*` and `/dashboards`: the conductor's own assertion, and nobody else's.
pub async fn conductor(State(state): State<AppState>, req: Request, next: Next) -> Response {
	principal(&state, |p| matches!(p, sa_auth::Principal::Conductor), req, next).await
}

/// `/openapi.json`: the conductor's pull, or anyone signed in.
pub async fn conductor_or_user(State(state): State<AppState>, req: Request, next: Next) -> Response {
	if let Auth::Dev { .. } = &*state.auth {
		return next.run(req).await;
	}
	principal(&state, |p| matches!(p, sa_auth::Principal::Conductor | sa_auth::Principal::User(_)), req, next).await
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

impl sa_core::prices::Payer for Member {
	type Who = PersonId;

	fn payer(&self) -> PersonId {
		self.0
	}
}

impl OperationInput for Member {
	fn operation_input(_: &mut GenContext, op: &mut Operation) {
		op.parameters.push(ReferenceOr::Item(Parameter::Header {
			parameter_data: ParameterData {
				name: MEMBER_HEADER.into(),
				description: Some("The person id of the member to act as; needs sa:review_archive:members:act_as unless it is the caller's own.".into()),
				required: false,
				deprecated: None,
				format: ParameterSchemaOrContent::Schema(SchemaObject {
					json_schema: json_schema!({ "type": "integer" }),
					example: None,
					external_docs: None,
				}),
				example: None,
				examples: Default::default(),
				explode: None,
				extensions: Default::default(),
			},
			style: HeaderStyle::Simple,
		}));
	}
}

impl OperationInput for Caller {}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
	type Rejection = std::convert::Infallible;

	async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
		Ok(parts.extensions.get::<Caller>().expect("layered under authenticate").clone())
	}
}
