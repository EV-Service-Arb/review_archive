//! The HTTP API: everything the CLI does, for other services, and `/me` for members; at `/`,
//! the dashboard's page. The operations are one table ([`routes`]): the router is built from it
//! and its OpenAPI document ([`document`]) derived from it. Every operation wants a caller (see
//! [`crate::auth`]); the conductor's own calls (`/api/conductor/*`, `/dashboards`) are outside the
//! table, and `/openapi.json` is the conductor's or anyone's signed in.
//! JSON in and out — errors included, the ones axum's extractors raise too — with the
//! DTOs of `review_archive_core::dto`. Every handler only translates: what to do, and
//! what the caller got wrong, is the facade's.

use std::{io::Seek, sync::Arc, time::Duration};

use axum::{
	Json, Router,
	body::Bytes,
	extract::{
		FromRequest, FromRequestParts, State,
		rejection::{JsonRejection, PathRejection, QueryRejection},
	},
	http::{Method, StatusCode, header, request::Parts},
	middleware,
	response::{IntoResponse, NoContent, Response},
	routing::get,
};
use ev_lib::api::{
	Created, GenContext, Guard, JsonSchema, OpenApi, OperationInput, OperationOutput, Route,
	openapi::{MediaType, Operation, Response as Answer, StatusCode as Status},
};
use review_archive::{Archive, Rejected, store::export::Destination};
use review_archive_core::{
	ReviewId, TargetId,
	dto::{
		Board, CaptureRequest, ChargePage, DayStats, ErrorBody, ExportQuery, FeedQuery, GmailOverview, JobAccepted, JobDto, LayoutDto, Me, MemberDto, MyGmbs, NewLayout, NewTarget,
		NewWebhook, ReinstatementDto, ReviewDetail, ReviewDto, ReviewsQuery, RunDto, RunsQuery, Settings, SettingsPatch, Snapshot, StatsQuery, TargetDetail, TargetDto, TargetPatch,
		TrackPatch, UsagePage, WaitQuery, WebhookDto, stats_csv,
	},
};
use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_http::{services::ServeDir, timeout::TimeoutLayer};
use v_utils::{Timeframe, macros::SettingsNested};

use crate::{
	auth::{Auth, Caller, Member, Section, authenticate, conductor, conductor_or_user, require},
	worker::Signals,
};

/// How long requests may take, and how many are served at once.
#[derive(Clone, Debug, Deserialize, Serialize, SettingsNested, SmartDefault, schemars::JsonSchema)]
#[settings(prefix = "http")]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
	/// The longest `POST /captures?wait=` holds a request.
	#[default(Timeframe::from("2m"))]
	pub max_wait: Timeframe,
	/// Past this a request is answered 408: `max_wait` and then some. Exports are exempt.
	#[default(Timeframe::from("3m"))]
	pub request_timeout: Timeframe,
	/// API requests served at once; more wait their turn. `/health` is not counted.
	#[default(64)]
	pub max_concurrent: usize,
}

#[derive(Clone)]
pub struct AppState {
	pub archive: Archive,
	pub auth: Arc<Auth>,
	pub signals: Arc<Signals>,
	pub cfg: Arc<HttpConfig>,
}

impl AppState {
	pub fn new(archive: Archive, auth: Auth, signals: Arc<Signals>, cfg: HttpConfig) -> Self {
		Self {
			archive,
			auth: Arc::new(auth),
			signals,
			cfg: Arc::new(cfg),
		}
	}
}

/// Who passes an operation's gate.
#[derive(Clone, Copy, Debug)]
pub struct Gate {
	/// `None`: everyone signed in.
	section: Option<Section>,
	/// Within `http.request_timeout`: all but an export, which streams as long as it takes.
	timed: bool,
}

impl Gate {
	const USER: Self = Self { section: None, timed: true };

	const fn section(section: Section) -> Self {
		Self {
			section: Some(section),
			..Self::USER
		}
	}

	const fn untimed(self) -> Self {
		Self { timed: false, ..self }
	}
}

impl Guard for Gate {
	fn permission(&self) -> Option<String> {
		self.section.map(|s| s.permission().to_owned())
	}
}

/// One API operation: what a person reaches through the conductor's forward.
type ApiRoute = Route<AppState, Gate>;

fn routes() -> Vec<ApiRoute> {
	use Method as M;
	let user = Gate::USER;
	let operator = Gate::section(Section::Operate);
	vec![
		ApiRoute::new(M::GET, "/targets", operator, list_targets),
		ApiRoute::new(M::POST, "/targets", operator, add_target),
		ApiRoute::new(M::GET, "/targets/{id}", operator, get_target),
		ApiRoute::new(M::PATCH, "/targets/{id}", operator, patch_target),
		ApiRoute::new(M::DELETE, "/targets/{id}", operator, disable_target),
		ApiRoute::new(M::GET, "/targets/{id}/reviews", operator, list_reviews),
		ApiRoute::new(M::GET, "/targets/{id}/runs", operator, list_runs),
		ApiRoute::new(M::POST, "/targets/{id}/scan", operator, scan_target),
		ApiRoute::new(M::GET, "/targets/{id}/export.zip", operator.untimed(), export_zip),
		ApiRoute::new(M::POST, "/captures", operator, capture_place),
		ApiRoute::new(M::GET, "/captures/{file}", user, capture_webp),
		ApiRoute::new(M::GET, "/jobs/{id}", operator, get_job),
		ApiRoute::new(M::GET, "/reviews/{id}", operator, get_review),
		ApiRoute::new(M::GET, "/stats", operator, stats),
		ApiRoute::new(M::GET, "/webhooks", operator, list_webhooks),
		ApiRoute::new(M::POST, "/webhooks", operator, add_webhook),
		ApiRoute::new(M::DELETE, "/webhooks/{id}", operator, delete_webhook),
		ApiRoute::new(M::GET, "/layouts/{dock}", user, list_layouts),
		ApiRoute::new(M::PUT, "/layouts/{dock}/{band}", operator, publish_layout),
		ApiRoute::new(M::GET, "/members", Gate::section(Section::ActAs), list_members),
		ApiRoute::new(M::GET, "/me", user, me),
		ApiRoute::new(M::GET, "/me/overview", user, my_overview),
		ApiRoute::new(M::GET, "/me/gmbs", user, my_gmbs),
		ApiRoute::new(M::GET, "/me/settings", user, my_settings),
		ApiRoute::new(M::PATCH, "/me/settings", user, patch_my_settings),
		ApiRoute::new(M::DELETE, "/me/settings", user, reset_my_settings),
		ApiRoute::new(M::PATCH, "/me/gmbs/{gmb}/tracks/{target}", user, patch_track),
		ApiRoute::new(M::POST, "/me/gmbs/{gmb}/tracks/{target}/relink", user, relink_track),
		ApiRoute::new(M::POST, "/me/gmbs/{gmb}/tracks/{target}/scan", user, scan_track).spends(),
		ApiRoute::new(M::GET, "/me/gmbs/{gmb}/tracks/{target}/board", user, track_board),
		ApiRoute::new(M::PUT, "/me/gmbs/{gmb}/reinstatements/{review}", user, reinstate),
		ApiRoute::new(M::DELETE, "/me/gmbs/{gmb}/reinstatements/{review}", user, withdraw_reinstatement),
	]
}

/// The operations as an OpenAPI document, served under `/`: the conductor puts it under its forward.
pub fn document() -> OpenApi {
	let mut doc = ev_lib::api::document("review_archive", env!("CARGO_PKG_VERSION"), "/", &routes(), ev_lib::api_docs!());
	doc.info.description = Some("Archive of public place reviews: a WebP screenshot of every review as it first appears, plus data for statistics.".into());
	doc
}

/// `mfe`: the built dashboard bundle, served under `/mfe/`. `sign_in`: where the page sends a
/// browser on a 401; with both, `/` is the dashboard's page.
pub fn router(state: AppState, mfe: Option<&std::path::Path>, sign_in: Option<&str>) -> Router {
	let timeout = TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, state.cfg.request_timeout.duration());
	let api = routes()
		.into_iter()
		.fold(Router::new(), |api, r| {
			let route = Router::new().route(r.path, r.handler());
			let route = match r.guard.section {
				Some(section) => route.route_layer(middleware::from_fn_with_state(section, require)),
				None => route,
			};
			api.merge(match r.guard.timed {
				true => route.layer(timeout.clone()),
				false => route,
			})
		})
		.layer(GlobalConcurrencyLimitLayer::new(state.cfg.max_concurrent))
		.route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
	let openapi = Bytes::from(serde_json::to_vec(&document()).expect("a document serializes"));
	// the probes answer whatever load the API is under
	let mut app = Router::new()
		.route("/health", get(|| async { "ok" }))
		.route("/openapi.json", tagged(openapi).route_layer(middleware::from_fn_with_state(state.clone(), conductor_or_user)))
		.route(
			"/dashboards",
			tagged(Bytes::from(format!("[{}]", DASHBOARDS.join(",")))).route_layer(middleware::from_fn_with_state(state.clone(), conductor)),
		)
		.merge(api)
		.merge(
			// the conductor's own calls: never a person, so outside `authenticate`
			Router::new()
				.route("/api/conductor/gmbs", axum::routing::put(apply_snapshot))
				.route("/api/conductor/charges", get(charges))
				.route("/api/conductor/usage", get(usage))
				.route("/api/conductor/notifications", get(notifications))
				.route("/api/conductor/captures/{file}", get(capture_png))
				.layer(timeout)
				.route_layer(middleware::from_fn_with_state(state.clone(), conductor)),
		);
	if let Some(dir) = mfe {
		let bundle = Router::new()
			.fallback_service(ServeDir::new(dir))
			.layer(middleware::from_fn_with_state(bundle_etag(dir), revalidated));
		app = app.nest_service("/mfe", bundle);
		if let Some(sign_in) = sign_in {
			let page = include_str!("../../review_archive_web/index.html").replace("{sign_in}", sign_in);
			let page = move || {
				let page = page.clone();
				async move { axum::response::Html(page) }
			};
			// the dashboard's own paths: a link to one of its views loads the page, which routes there
			app = app
				.route("/", get(page.clone()))
				.route("/settings", get(page.clone()))
				.route("/act-as", get(page.clone()))
				.route("/gmbs", get(page.clone()))
				.route("/gmbs/{*view}", get(page.clone()))
				.route("/gmails/{*view}", get(page.clone()))
				.route("/members/{member}/{*view}", get(page.clone()))
				.route("/members/{member}/", get(page.clone()))
				.route("/members/{member}", get(page));
		}
	}
	app.with_state(state)
}

/// The Grafana dashboards of `grafana/dashboards/`, for the conductor's Grafana: every one in an array.
const DASHBOARDS: [&str; 2] = [
	include_str!("../../../grafana/dashboards/ra-location.json"),
	include_str!("../../../grafana/dashboards/ra-overview.json"),
];

/// A JSON `body` that never changes while the process lives, tagged by its digest.
fn tagged(body: Bytes) -> axum::routing::MethodRouter<AppState> {
	use sha2::{Digest, Sha256};
	let digest: String = Sha256::digest(&body)[..16].iter().map(|b| format!("{b:02x}")).collect();
	let etag = header::HeaderValue::try_from(format!("\"{digest}\"")).expect("hex is a header value");
	get(move || async move { ([(header::CONTENT_TYPE, "application/json")], body) }).layer(middleware::from_fn_with_state(etag, revalidated))
}

/// The whole bundle's digest, for every file in it: the nix store dates each file 1970, so a
/// date tells one build from another for none of them.
fn bundle_etag(dir: &std::path::Path) -> header::HeaderValue {
	use sha2::{Digest, Sha256};
	fn walk(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
		for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading the bundle at {}: {e}", dir.display())) {
			let path = e.expect("listing the bundle").path();
			match path.is_dir() {
				true => walk(&path, files),
				false => files.push(path),
			}
		}
	}
	let mut files = vec![];
	walk(dir, &mut files);
	files.sort();
	let mut h = Sha256::new();
	for f in &files {
		h.update(f.strip_prefix(dir).expect("walked from it").as_os_str().as_encoded_bytes());
		h.update([0]);
		h.update(std::fs::read(f).unwrap_or_else(|e| panic!("reading {}: {e}", f.display())));
	}
	let digest: String = h.finalize()[..16].iter().map(|b| format!("{b:02x}")).collect();
	header::HeaderValue::try_from(format!("\"{digest}\"")).expect("hex is a header value")
}

/// Asked about again on each load, and the same only while its tag is.
async fn revalidated(State(etag): State<header::HeaderValue>, mut req: axum::extract::Request, next: middleware::Next) -> Response {
	// an edge that compresses weakens the tag it passes on
	if req
		.headers()
		.get(header::IF_NONE_MATCH)
		.is_some_and(|v| v.as_bytes().strip_prefix(b"W/").unwrap_or(v.as_bytes()) == etag.as_bytes())
	{
		return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
	}
	req.headers_mut().remove(header::IF_MODIFIED_SINCE);
	let mut res = next.run(req).await;
	let h = res.headers_mut();
	h.remove(header::LAST_MODIFIED);
	h.insert(header::ETAG, etag);
	h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
	res
}

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
	fn into_response(self) -> Response {
		(self.0, Json(ErrorBody { error: self.1 })).into_response()
	}
}

impl OperationOutput for ApiError {
	type Inner = ErrorBody;

	fn operation_response(ctx: &mut GenContext, op: &mut Operation) -> Option<Answer> {
		Json::<ErrorBody>::operation_response(ctx, op)
	}

	fn inferred_responses(ctx: &mut GenContext, op: &mut Operation) -> Vec<(Option<Status>, Answer)> {
		Self::operation_response(ctx, op).map(|r| (None, r)).into_iter().collect()
	}
}

impl From<eyre::Report> for ApiError {
	/// The caller's mistakes are 404 / 400 / 429 / 409 with the reason. Anything else is a 5xx:
	/// reported, its details kept in the log rather than the response.
	fn from(e: eyre::Report) -> Self {
		match e.downcast_ref::<Rejected>() {
			Some(Rejected::NotFound(m)) => Self(StatusCode::NOT_FOUND, m.clone()),
			Some(Rejected::Invalid(m)) => Self(StatusCode::BAD_REQUEST, m.clone()),
			Some(Rejected::Busy(m)) => Self(StatusCode::TOO_MANY_REQUESTS, m.clone()),
			Some(Rejected::Conflict(m)) => Self(StatusCode::CONFLICT, m.clone()),
			None => {
				crate::report(&e, "request failed");
				Self(StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
			}
		}
	}
}

/// What axum's extractors refuse is the caller's mistake too, and answered the same way.
macro_rules! rejections {
	($($rejection:ty),*) => {$(
		impl From<$rejection> for ApiError {
			fn from(r: $rejection) -> Self {
				Self(r.status(), r.body_text())
			}
		}
	)*};
}
rejections!(JsonRejection, QueryRejection, PathRejection);

/// A JSON body, refused as an [`ErrorBody`].
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
struct JsonBody<T>(T);

/// A query string, refused as an [`ErrorBody`].
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
struct Query<T>(T);

/// A path's parameters, by their names in it, refused as an [`ErrorBody`].
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
struct Path<T>(T);

impl<T: JsonSchema> OperationInput for JsonBody<T> {
	fn operation_input(ctx: &mut GenContext, op: &mut Operation) {
		Json::<T>::operation_input(ctx, op);
	}
}

impl<T: JsonSchema> OperationInput for Query<T> {
	fn operation_input(ctx: &mut GenContext, op: &mut Operation) {
		axum::extract::Query::<T>::operation_input(ctx, op);
	}
}

impl<T: JsonSchema> OperationInput for Path<T> {
	fn operation_input(ctx: &mut GenContext, op: &mut Operation) {
		axum::extract::Path::<T>::operation_input(ctx, op);
	}
}

#[derive(Deserialize, JsonSchema)]
struct Id {
	id: i64,
}

#[derive(Deserialize, JsonSchema)]
struct Track {
	/// The conductor's id of the member's GMB.
	gmb: i64,
	/// The target the GMB's track is on.
	target: i64,
}

#[derive(Deserialize, JsonSchema)]
struct Appeal {
	/// The conductor's id of the member's GMB.
	gmb: i64,
	/// The removed review.
	review: i64,
}

#[derive(Deserialize, JsonSchema)]
struct Dock {
	dock: String,
}

#[derive(Deserialize, JsonSchema)]
struct Slot {
	dock: String,
	/// `sm`, `md` or `xl`.
	band: String,
}

#[derive(Deserialize, JsonSchema)]
struct CaptureFile {
	/// `<sha256>.webp`.
	file: String,
}

/// Whether the caller asked for CSV (`Accept: text/csv`); the answer's media types say so.
struct WantsCsv(bool);

impl<S: Send + Sync> FromRequestParts<S> for WantsCsv {
	type Rejection = std::convert::Infallible;

	async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
		let accept = parts.headers.get(header::ACCEPT).and_then(|v| v.to_str().ok());
		Ok(Self(accept.is_some_and(|a| a.split(',').any(|m| m.trim().starts_with("text/csv")))))
	}
}

impl OperationInput for WantsCsv {}

/// `202` and what was queued.
struct Accepted<T>(T);

impl<T: Serialize> IntoResponse for Accepted<T> {
	fn into_response(self) -> Response {
		(StatusCode::ACCEPTED, Json(self.0)).into_response()
	}
}

impl<T: JsonSchema> OperationOutput for Accepted<T> {
	type Inner = T;

	fn operation_response(ctx: &mut GenContext, op: &mut Operation) -> Option<Answer> {
		Json::<T>::operation_response(ctx, op)
	}

	fn inferred_responses(ctx: &mut GenContext, op: &mut Operation) -> Vec<(Option<Status>, Answer)> {
		Self::operation_response(ctx, op).map(|r| (Some(Status::Code(202)), r)).into_iter().collect()
	}
}

/// An ad-hoc capture's job: `200` finished within `wait`, `202` queued or still running.
enum Captured {
	Finished(JobDto),
	Pending(JobAccepted),
}

impl IntoResponse for Captured {
	fn into_response(self) -> Response {
		match self {
			Self::Finished(job) => Json(job).into_response(),
			Self::Pending(queued) => Accepted(queued).into_response(),
		}
	}
}

impl OperationOutput for Captured {
	type Inner = JobDto;

	fn inferred_responses(ctx: &mut GenContext, op: &mut Operation) -> Vec<(Option<Status>, Answer)> {
		[Json::<JobDto>::inferred_responses(ctx, op), Accepted::<JobAccepted>::inferred_responses(ctx, op)].concat()
	}
}

/// A `200` of `mime`, whose bytes have no schema.
fn file_answer(mime: &str, description: &str) -> Vec<(Option<Status>, Answer)> {
	let answer = Answer {
		description: description.into(),
		content: [(mime.to_owned(), MediaType::default())].into_iter().collect(),
		..Answer::default()
	};
	vec![(Some(Status::Code(200)), answer)]
}

/// A capture's WebP: behind auth, so no shared cache may keep it; named by its hash, so it never changes.
struct Webp(Vec<u8>);

impl IntoResponse for Webp {
	fn into_response(self) -> Response {
		([(header::CONTENT_TYPE, "image/webp"), (header::CACHE_CONTROL, "private, max-age=31536000, immutable")], self.0).into_response()
	}
}

impl OperationOutput for Webp {
	type Inner = Vec<u8>;

	fn inferred_responses(_: &mut GenContext, _: &mut Operation) -> Vec<(Option<Status>, Answer)> {
		file_answer("image/webp", "the capture, its provenance in Exif")
	}
}

/// A target's export, streamed from a file.
struct Zip {
	target: i64,
	body: axum::body::Body,
}

impl IntoResponse for Zip {
	fn into_response(self) -> Response {
		let disposition = format!("attachment; filename=\"target-{}.zip\"", self.target);
		([(header::CONTENT_TYPE, "application/zip".to_owned()), (header::CONTENT_DISPOSITION, disposition)], self.body).into_response()
	}
}

impl OperationOutput for Zip {
	type Inner = Vec<u8>;

	fn inferred_responses(_: &mut GenContext, _: &mut Operation) -> Vec<(Option<Status>, Answer)> {
		file_answer("application/zip", "`manifest.json` and the first capture of each review")
	}
}

/// Day stats as JSON, or as CSV to who asked for it.
enum Stats {
	Json(Vec<DayStats>),
	Csv(String),
}

impl IntoResponse for Stats {
	fn into_response(self) -> Response {
		match self {
			Self::Json(rows) => Json(rows).into_response(),
			Self::Csv(csv) => ([(header::CONTENT_TYPE, "text/csv; charset=utf-8")], csv).into_response(),
		}
	}
}

impl OperationOutput for Stats {
	type Inner = Vec<DayStats>;

	fn inferred_responses(ctx: &mut GenContext, op: &mut Operation) -> Vec<(Option<Status>, Answer)> {
		let mut answers = Json::<Vec<DayStats>>::inferred_responses(ctx, op);
		for (_, a) in &mut answers {
			a.content.insert("text/csv".into(), MediaType::default());
		}
		answers
	}
}

type ApiResult<T> = Result<T, ApiError>;

/// Every watched place.
async fn list_targets(State(s): State<AppState>) -> ApiResult<Json<Vec<TargetDto>>> {
	Ok(Json(s.archive.targets().await?.into_iter().map(TargetDto::from).collect()))
}

/// Watches a place: a place id, or a Maps URL, as `target add` resolves them.
async fn add_target(State(s): State<AppState>, JsonBody(req): JsonBody<NewTarget>) -> ApiResult<Created<TargetDto>> {
	Ok(Created(s.archive.add_target(&req).await?.target.into()))
}

/// A target with its last run, what is archived, and the next scheduled scan.
async fn get_target(State(s): State<AppState>, Path(Id { id }): Path<Id>) -> ApiResult<Json<TargetDetail>> {
	Ok(Json(s.archive.target_detail(TargetId(id)).await?))
}

/// Changes what the patch names; the rest stays.
async fn patch_target(State(s): State<AppState>, Path(Id { id }): Path<Id>, JsonBody(patch): JsonBody<TargetPatch>) -> ApiResult<Json<TargetDto>> {
	Ok(Json(s.archive.update_target(TargetId(id), &patch).await?.into()))
}

/// Disables the target. Nothing archived is deleted.
async fn disable_target(State(s): State<AppState>, Path(Id { id }): Path<Id>) -> ApiResult<Json<TargetDto>> {
	let disable = TargetPatch {
		enabled: Some(false),
		..Default::default()
	};
	Ok(Json(s.archive.update_target(TargetId(id), &disable).await?.into()))
}

/// The target's reviews as archived.
async fn list_reviews(State(s): State<AppState>, Path(Id { id }): Path<Id>, Query(q): Query<ReviewsQuery>) -> ApiResult<Json<Vec<ReviewDto>>> {
	Ok(Json(s.archive.reviews(TargetId(id), &q).await?))
}

/// The target's runs, newest first.
async fn list_runs(State(s): State<AppState>, Path(Id { id }): Path<Id>, Query(q): Query<RunsQuery>) -> ApiResult<Json<Vec<RunDto>>> {
	Ok(Json(s.archive.runs(TargetId(id), &q).await?))
}

/// Scans the target now, ahead of the scheduled scans.
///
/// The same scan already queued is not queued twice: its id comes back. Past the queue's
/// `max_queued_jobs`, 429.
async fn scan_target(State(s): State<AppState>, Path(Id { id }): Path<Id>) -> ApiResult<Accepted<JobAccepted>> {
	let job_id = s.archive.enqueue_scan(TargetId(id)).await?;
	s.signals.job_queued.notify_one();
	Ok(Accepted(JobAccepted { job_id }))
}

/// Captures a place without registering it.
///
/// Its results are kept under an implicit, disabled target (the place's existing one, if any),
/// so nothing is lost. `max_reviews` and the number of `review_ids` are at most
/// `max_reviews_initial`. With `wait`, answers the finished job if it finishes by then; else
/// the job's id, to poll. Past the queue's `max_queued_jobs`, 429.
async fn capture_place(State(s): State<AppState>, Query(q): Query<WaitQuery>, JsonBody(req): JsonBody<CaptureRequest>) -> ApiResult<Captured> {
	// subscribed before queueing, so the job cannot end unseen in between
	let mut finished = s.signals.job_finished.subscribe();
	let job_id = s.archive.enqueue_capture(&req).await?;
	s.signals.job_queued.notify_one();
	let deadline = tokio::time::Instant::now() + Duration::from_secs(q.wait.unwrap_or(0)).min(s.cfg.max_wait.duration());
	loop {
		let job = s.archive.job(job_id).await?;
		if job.status.is_finished() {
			return Ok(Captured::Finished(job));
		}
		match tokio::time::timeout_at(deadline, finished.changed()).await {
			Ok(Ok(())) => {}
			// out of time, or the worker is gone: the job id is still good for polling
			Ok(Err(_)) | Err(_) => return Ok(Captured::Pending(JobAccepted { job_id })),
		}
	}
}

/// A job: `queued`, `running`, `done` or `failed`; on `done`, the reviews its run listed with
/// their capture URLs (for a capture of `review_ids`, those of them it found).
async fn get_job(State(s): State<AppState>, Path(Id { id }): Path<Id>) -> ApiResult<Json<JobDto>> {
	Ok(Json(s.archive.job(id).await?))
}

/// A review with every version and capture.
async fn get_review(State(s): State<AppState>, Path(Id { id }): Path<Id>) -> ApiResult<Json<ReviewDetail>> {
	Ok(Json(s.archive.review(ReviewId(id)).await?))
}

/// A capture's WebP, provenance in its Exif.
///
/// Only what the archive recorded; for a member, only what shows a review of a place they track.
async fn capture_webp(State(s): State<AppState>, caller: Caller, Path(CaptureFile { file }): Path<CaptureFile>) -> ApiResult<Webp> {
	let sha = file.strip_suffix(".webp").ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such capture".into()))?;
	Ok(Webp(match caller.permissions.may(sa_auth::Archive::Operate) {
		true => s.archive.capture_webp(sha).await?,
		false => s.archive.member_capture_webp(caller.person, sha).await?,
	}))
}

/// The same archive as `export`: `manifest.json` and the first capture of each review.
async fn export_zip(State(s): State<AppState>, Path(Id { id }): Path<Id>, Query(q): Query<ExportQuery>) -> ApiResult<Zip> {
	// An archive of thousands of captures does not belong in memory: it is written to a temp file
	// with no name, which goes away with its last handle whatever happens to the request.
	let mut file = tempfile::tempfile().map_err(eyre::Report::new)?;
	s.archive.export(TargetId(id), &q, Destination::Zip(file.try_clone().map_err(eyre::Report::new)?)).await?;
	file.rewind().map_err(eyre::Report::new)?;
	Ok(Zip {
		target: id,
		body: axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(file))),
	})
}

/// Per target and UTC day: new, changed, gone, mean rating, histogram.
///
/// `gone` counts what the runs of that day marked gone, whether or not it came back later.
/// `Accept: text/csv` for CSV, the histogram as `stars_1` … `stars_5`.
async fn stats(State(s): State<AppState>, WantsCsv(csv): WantsCsv, Query(q): Query<StatsQuery>) -> ApiResult<Stats> {
	let rows = s.archive.stats(&q).await?;
	Ok(match csv {
		true => Stats::Csv(stats_csv(&rows)?),
		false => Stats::Json(rows),
	})
}

/// Subscribes to events.
///
/// Deliveries are POSTed with `X-Signature: sha256=<hex HMAC-SHA256 of the body under the
/// secret>`, `X-Event` and `X-Delivery-Id`; a non-2xx answer is retried with backoff for about a
/// day. The body is an `EventPayload`: `event`, `occurred_at`, `target_id`, and the `review` or
/// `run`. The URL is `http(s)`, and may not point at a local or private address unless the
/// archive's `webhooks.allowed_hosts` names its host; a non-empty `allowed_hosts` is the whole
/// list of hosts hooks may go to. Redirects are not followed.
async fn add_webhook(State(s): State<AppState>, JsonBody(hook): JsonBody<NewWebhook>) -> ApiResult<Created<WebhookDto>> {
	Ok(Created(s.archive.add_webhook(&hook).await?))
}

/// Every hook, never its secret.
async fn list_webhooks(State(s): State<AppState>) -> ApiResult<Json<Vec<WebhookDto>>> {
	Ok(Json(s.archive.webhooks().await?))
}

/// Removes the hook and whatever it was still owed.
async fn delete_webhook(State(s): State<AppState>, Path(Id { id }): Path<Id>) -> ApiResult<NoContent> {
	s.archive.delete_webhook(id).await?;
	Ok(NoContent)
}

/// Every band's published layout of a dock: what a browser without one of its own starts from.
async fn list_layouts(State(s): State<AppState>, Path(Dock { dock }): Path<Dock>) -> ApiResult<Json<Vec<LayoutDto>>> {
	Ok(Json(s.archive.layouts(&dock).await?))
}

/// Publishes a dock's layout at a band for every browser without one of its own.
async fn publish_layout(State(s): State<AppState>, Path(Slot { dock, band }): Path<Slot>, JsonBody(layout): JsonBody<NewLayout>) -> ApiResult<NoContent> {
	s.archive.publish_layout(&dock, &band, &layout).await?;
	Ok(NoContent)
}

/// Who is signed in.
async fn me(State(s): State<AppState>, caller: Caller) -> ApiResult<Json<Me>> {
	let mine = s.archive.store()?.person_by_id(caller.person).await?.expect("authenticate found or made them");
	Ok(Json(Me {
		id: caller.person.0,
		email: mine.email,
		name: mine.name,
		permissions: caller.permissions.iter().map(str::to_owned).collect(),
	}))
}

/// Everyone who signed in here, and the addresses from before people had ids, with what they hold.
async fn list_members(State(s): State<AppState>) -> ApiResult<Json<Vec<MemberDto>>> {
	Ok(Json(s.archive.members().await?))
}

/// The member's gmails, each with its GMBs and their places as tracked, by screenshots over the last 7 days.
async fn my_overview(State(s): State<AppState>, Member(m): Member) -> ApiResult<Json<Vec<GmailOverview>>> {
	Ok(Json(s.archive.overview(m).await?))
}

/// The archive's tag on each of the member's GMBs, for the conductor's page.
async fn my_gmbs(State(s): State<AppState>, Member(m): Member) -> ApiResult<Json<MyGmbs>> {
	Ok(Json(s.archive.my_gmbs(m).await?))
}

/// What the member chose, each beside its default.
async fn my_settings(State(s): State<AppState>, Member(m): Member) -> ApiResult<Json<Settings>> {
	Ok(Json(s.archive.settings(m).await?))
}

/// Chooses what the patch names; the rest stays.
async fn patch_my_settings(State(s): State<AppState>, Member(m): Member, JsonBody(req): JsonBody<SettingsPatch>) -> ApiResult<Json<Settings>> {
	Ok(Json(s.archive.patch_settings(m, &req).await?))
}

/// Every setting back to its default.
async fn reset_my_settings(State(s): State<AppState>, Member(m): Member) -> ApiResult<Json<Settings>> {
	Ok(Json(s.archive.reset_settings(m).await?))
}

/// Switches the GMB's track of a place on or off, or marks it a favorite; the place's target stays as it is.
async fn patch_track(State(s): State<AppState>, Member(m): Member, Path(Track { gmb, target }): Path<Track>, JsonBody(req): JsonBody<TrackPatch>) -> ApiResult<NoContent> {
	s.archive.patch_track(m, gmb, TargetId(target), &req).await?;
	Ok(NoContent)
}

/// Asks Google about a tracked place again: its id, name and address become what Google gives now.
async fn relink_track(State(s): State<AppState>, Member(m): Member, Path(Track { gmb, target }): Path<Track>) -> ApiResult<Json<TargetDto>> {
	Ok(Json(s.archive.relink(m, gmb, TargetId(target)).await?))
}

/// Queues a scan of a tracked place, on top of its schedule; the member who asks pays for it in tokens.
///
/// 429 when the GMB is switched off or not verified, Maps is paused, the member is out of
/// tokens, or the queue is full.
async fn scan_track(State(s): State<AppState>, Member(m): Member, Path(Track { gmb, target }): Path<Track>) -> ApiResult<Accepted<JobAccepted>> {
	let job_id = s.archive.scan_tracked(m, gmb, TargetId(target)).await?;
	s.signals.job_queued.notify_one();
	Ok(Accepted(JobAccepted { job_id }))
}

/// A tracked place's reviews in four columns: snapshotted, suspended, removed, reinstating.
async fn track_board(State(s): State<AppState>, Member(m): Member, Path(Track { gmb, target }): Path<Track>) -> ApiResult<Json<Board>> {
	Ok(Json(s.archive.board(m, gmb, TargetId(target)).await?))
}

/// Records that the removed review's reinstatement was asked of Google, now.
///
/// Asking again while one is open answers the open one.
async fn reinstate(State(s): State<AppState>, Member(m): Member, Path(Appeal { gmb, review }): Path<Appeal>) -> ApiResult<Json<ReinstatementDto>> {
	Ok(Json(s.archive.reinstate(m, gmb, ReviewId(review)).await?))
}

/// Withdraws the open request; it stays on record as withdrawn.
async fn withdraw_reinstatement(State(s): State<AppState>, Member(m): Member, Path(Appeal { gmb, review }): Path<Appeal>) -> ApiResult<NoContent> {
	s.archive.withdraw_reinstatement(m, gmb, ReviewId(review)).await?;
	Ok(NoContent)
}

/// The conductor's GMBs and balances, taken whole or not at all.
async fn apply_snapshot(State(s): State<AppState>, JsonBody(snap): JsonBody<Snapshot>) -> ApiResult<StatusCode> {
	s.archive.apply_snapshot(&snap).await?;
	Ok(StatusCode::NO_CONTENT)
}

/// What walks cost, for the conductor to charge: the feed past `after`.
async fn charges(State(s): State<AppState>, Query(q): Query<FeedQuery>) -> ApiResult<Json<ChargePage>> {
	Ok(Json(s.archive.charges(q.after, q.limit).await?))
}

/// What walks did, whoever paid, for the conductor's costs: the feed past `after`.
#[utoipa::path(get, path = "/api/conductor/usage", tag = "conductor", params(FeedQuery),
	responses((status = 200, body = UsagePage), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody)))]
async fn usage(State(s): State<AppState>, Query(q): Query<FeedQuery>) -> ApiResult<Json<UsagePage>> {
	Ok(Json(s.archive.usage(q.after, q.limit).await?))
}

/// The Alerts feed past `after`: notes on the places live tracks name, each with their GMBs.
async fn notifications(State(s): State<AppState>, Query(q): Query<FeedQuery>) -> ApiResult<Json<sa_alerts::NotePage>> {
	Ok(Json(s.archive.notifications(q.after, q.limit).await?))
}

/// A capture as PNG, the photo of a `review_gone` note.
async fn capture_png(State(s): State<AppState>, Path(file): Path<String>) -> ApiResult<Response> {
	let sha = file.strip_suffix(".png").ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such capture".into()))?;
	Ok((
		[(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=31536000, immutable")],
		s.archive.capture_png(sha).await?,
	)
		.into_response())
}
