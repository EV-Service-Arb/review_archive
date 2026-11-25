//! The HTTP API: the archive's operations, and `/me` for members; at `/`, the dashboard's page.
//! The operations are one table (`routes`): the router is built from it and its OpenAPI
//! document ([`document`]) derived from it. Every row's gate is `sa_auth`'s (see [`crate::auth`]);
//! the conductor's own calls (`/api/conductor/*`, `/dashboards`) are outside the table, and
//! `/openapi.json` is the conductor's or anyone's signed in.
//! JSON in and out, refusals `{code, error}`, with the DTOs of `review_archive_core::dto`. Every
//! handler only translates: what to do, and what the caller got wrong, is the facade's.

mod dashboards;

use std::{
	io::Seek,
	path::{Path, PathBuf},
	sync::Arc,
	time::Duration,
};

use axum::{
	Json, Router,
	extract::{FromRequestParts, State},
	http::{Method, StatusCode, header, request::Parts},
	middleware,
	response::{IntoResponse, NoContent, Response},
	routing::get,
};
use ev_lib::{
	api::{
		Body, Created, ErrorCode, GenContext, Ids, JsonSchema, OpenApi, OperationInput, OperationOutput, Params, Refusal, Refuses, Route,
		openapi::{MediaType, Operation, Response as Answer, StatusCode as Status},
		serve,
	},
	views::Views,
};
use review_archive::{Archive, Rejected, store::export::Destination};
use review_archive_core::{
	ReviewId, TargetId,
	dto::{
		self, Board, CaptureRequest, ChargePage, DayStats, EventsQuery, ExportQuery, FeedQuery, GmailOverview, GmbEvents, JobAccepted, JobDto, LayoutDto, Me, MyGmbs, NewLayout, NewTarget,
		NewWebhook, ReinstatementDto, ReviewDetail, ReviewDto, ReviewsQuery, RunDto, RunsQuery, Settings, SettingsPatch, Snapshot, StatsQuery, StickQuery, StickReport, TargetDetail,
		TargetDto, TargetPatch, TrackPatch, UsagePage, WaitQuery, WebhookDto, stats_csv,
	},
	views::View,
};
use sa_auth::{Conductor, Denied, User, Verifier};
use sa_core::{
	feeds,
	prices::{Spends, WalkStep},
};
use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_http::services::ServeDir;
use v_utils::{Timeframe, macros::SettingsNested};

use crate::{
	auth::{Caller, Dev, Vouched},
	worker::Signals,
};

type Gate = sa_auth::Gate<Timed>;

/// One API operation: what a person reaches through the conductor's forward.
type ApiRoute = Route<AppState, Gate>;

type ApiResult<T> = Result<T, Refusal<Code>>;

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
	/// The conductor's keys; no `FromRef`, so the gate is [`crate::auth`]'s own `Vouch`.
	pub verifier: Verifier,
	pub signals: Arc<Signals>,
	pub cfg: Arc<HttpConfig>,
}

impl AppState {
	pub fn new(archive: Archive, verifier: Verifier, signals: Arc<Signals>, cfg: HttpConfig) -> Self {
		Self {
			archive,
			verifier,
			signals,
			cfg: Arc::new(cfg),
		}
	}
}

/// Within `http.request_timeout`: all but an export, which streams as long as it takes.
#[derive(Clone, Copy, Debug)]
pub struct Timed(bool);

/// The operations as an OpenAPI document, served under `/`: the conductor puts it under its forward.
pub fn document() -> OpenApi {
	let mut doc = ev_lib::api::document("review_archive", env!("CARGO_PKG_VERSION"), "/", &routes(), ev_lib::api_docs!(), Some(View::table()));
	doc.info.description = Some("Archive of public place reviews: a WebP screenshot of every review as it first appears, plus data for statistics.".into());
	doc
}

/// `mfe`: the built dashboard bundle, served under `/mfe/`; with `dev` too, `/` is the dashboard's
/// page, its sign-in `/` itself.
pub fn router(state: AppState, mfe: Option<&Path>, dev: Option<Dev>) -> Router {
	let limit = state.cfg.request_timeout.duration();
	let timeout = tower::ServiceBuilder::new()
		.layer(axum::error_handling::HandleErrorLayer::new(move |e: tower::BoxError| async move {
			assert!(e.is::<tower::timeout::error::Elapsed>(), "a route fails in nothing but its time: {e}");
			Refusal::new(Code::TimedOut, format!("not answered within http.request_timeout ({}s)", limit.as_secs()))
		}))
		.layer(tower::timeout::TimeoutLayer::new(limit));
	let api = routes()
		.into_iter()
		.fold(Router::new(), |api, r| {
			let route = Router::new().route(r.path, r.handler());
			api.merge(match r.guard.flags.0 {
				true => route.layer(timeout.clone()),
				false => route,
			})
		})
		.layer(GlobalConcurrencyLimitLayer::new(state.cfg.max_concurrent));
	let doc = document();
	// the probes answer whatever load the API is under
	let mut app = Router::new()
		.route("/health", get(|| async { "ok" }))
		.route("/dashboards", serve(&dashboards::all(&doc), sa_auth::Gate::CONDUCTOR))
		.route("/openapi.json", serve(&doc, sa_auth::Gate::USER.or_conductor()))
		.merge(api)
		.merge(
			// the conductor's own calls, each taking `Conductor`
			Router::new()
				.route(feeds::GMBS, axum::routing::put(apply_snapshot))
				.route(feeds::GONE, get(gone))
				.route(feeds::CHARGES, get(charges))
				.route(feeds::USAGE, get(usage))
				.route(feeds::REVIEWS, get(review_feed))
				.route(feeds::NOTIFICATIONS, get(notifications))
				.route(&format!("{}/{{file}}", feeds::CAPTURES), get(capture_png))
				.layer(timeout)
				// open as long as the process: no timeout
				.route(feeds::CHANGES, get(changes)),
		);
	if let Some(dir) = mfe {
		let bundle = Router::new()
			.fallback_service(ServeDir::new(dir))
			.layer(middleware::from_fn_with_state(bundle_etag(dir), revalidated));
		app = app.nest_service("/mfe", bundle);
		if dev.is_some() {
			let page = include_str!("../../review_archive_web/index.html").replace("{sign_in}", "/");
			// a link to one of the dashboard's views loads the page, which routes there
			app = app.fallback(get(move |uri: axum::http::Uri| {
				let page = page.clone();
				async move {
					match View::parse(uri.path()) {
						Ok(_) => axum::response::Html(page).into_response(),
						Err(e) => (StatusCode::NOT_FOUND, e.to_string()).into_response(),
					}
				}
			}));
		}
	}
	let app = app.with_state(state);
	match dev {
		Some(dev) => app.layer(middleware::from_fn(move |req, next| dev.clone().sign(req, next))),
		None => app,
	}
}

/// What a request is refused with: [`Rejected`]'s words, the gate's, and a bare `internal` for a bug.
#[derive(Clone, Copy, Debug, Serialize, strum::VariantArray)]
#[serde(rename_all = "snake_case")]
pub enum Code {
	NotFound,
	Invalid,
	Busy,
	Conflict,
	Unauthenticated,
	Forbidden,
	/// A sign-in whose address's rows from before people had ids it may not claim.
	Unclaimable,
	/// Past `http.request_timeout`.
	TimedOut,
	Internal,
}

fn routes() -> Vec<ApiRoute> {
	use Method as M;
	let user = sa_auth::Gate::USER.with(Timed(true));
	let operator = user.needs(sa_auth::Archive::Operate);
	vec![
		ApiRoute::new(M::GET, "/targets", operator, list_targets),
		ApiRoute::new(M::POST, "/targets", operator, add_target),
		ApiRoute::new(M::GET, "/targets/{id}", operator, get_target),
		ApiRoute::new(M::PATCH, "/targets/{id}", operator, patch_target),
		ApiRoute::new(M::DELETE, "/targets/{id}", operator, disable_target),
		ApiRoute::new(M::GET, "/targets/{id}/reviews", operator, list_reviews),
		ApiRoute::new(M::GET, "/targets/{id}/runs", operator, list_runs),
		ApiRoute::new(M::POST, "/targets/{id}/scan", operator, scan_target),
		ApiRoute::new(
			M::GET,
			"/targets/{id}/export.zip",
			sa_auth::Gate::USER.with(Timed(false)).needs(sa_auth::Archive::Operate),
			export_zip,
		),
		ApiRoute::new(M::POST, "/captures", operator, capture_place),
		ApiRoute::new(M::GET, dto::CAPTURE, user, capture_webp),
		ApiRoute::new(M::GET, "/jobs/{id}", operator, get_job),
		ApiRoute::new(M::GET, "/reviews/{id}", operator, get_review),
		ApiRoute::new(M::GET, "/stats", operator, stats),
		ApiRoute::new(M::GET, "/webhooks", operator, list_webhooks),
		ApiRoute::new(M::POST, "/webhooks", operator, add_webhook),
		ApiRoute::new(M::DELETE, "/webhooks/{id}", operator, delete_webhook),
		ApiRoute::new(M::GET, "/layouts/{dock}", user, list_layouts),
		ApiRoute::new(M::PUT, "/layouts/{dock}/{band}", operator, publish_layout),
		ApiRoute::new(M::GET, "/me", user, me),
		ApiRoute::new(M::GET, "/me/overview", user, my_overview),
		ApiRoute::new(M::GET, "/me/gmbs", user, my_gmbs),
		ApiRoute::new(M::GET, "/me/stick", user, my_stick),
		ApiRoute::new(M::GET, "/me/settings", user, my_settings),
		ApiRoute::new(M::PATCH, "/me/settings", user, patch_my_settings),
		ApiRoute::new(M::DELETE, "/me/settings", user, reset_my_settings),
		ApiRoute::new(M::GET, "/me/gmbs/{gmb}/events", user, gmb_events),
		ApiRoute::new(M::PATCH, "/me/gmbs/{gmb}/tracks/{target}", user, patch_track),
		ApiRoute::new(M::POST, "/me/gmbs/{gmb}/tracks/{target}/relink", user, relink_track),
		ApiRoute::new(M::POST, "/me/gmbs/{gmb}/tracks/{target}/scan", user, scan_track),
		ApiRoute::new(M::GET, "/me/gmbs/{gmb}/tracks/{target}/board", user, track_board),
		ApiRoute::new(M::GET, "/me/gmbs/{gmb}/tracks/{target}/runs", user, track_runs),
		ApiRoute::new(M::PUT, "/me/gmbs/{gmb}/reinstatements/{review}", user, reinstate),
		ApiRoute::new(M::DELETE, "/me/gmbs/{gmb}/reinstatements/{review}", user, withdraw_reinstatement),
	]
}

/// The whole bundle's digest, for every file in it: the nix store dates each file 1970, so a
/// date tells one build from another for none of them.
fn bundle_etag(dir: &Path) -> header::HeaderValue {
	use sha2::{Digest, Sha256};
	fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
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

impl ErrorCode for Code {
	fn status(self) -> StatusCode {
		match self {
			Self::NotFound => StatusCode::NOT_FOUND,
			Self::Invalid => StatusCode::BAD_REQUEST,
			Self::Busy => StatusCode::TOO_MANY_REQUESTS,
			Self::Conflict => StatusCode::CONFLICT,
			Self::Unauthenticated => StatusCode::UNAUTHORIZED,
			Self::Forbidden | Self::Unclaimable => StatusCode::FORBIDDEN,
			Self::TimedOut => StatusCode::REQUEST_TIMEOUT,
			Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
		}
	}
}

impl From<Denied> for Code {
	fn from(d: Denied) -> Self {
		match d {
			Denied::Unauthenticated => Self::Unauthenticated,
			Denied::Forbidden => Self::Forbidden,
		}
	}
}

impl Refuses<eyre::Report> for Code {
	/// The caller's mistakes with the reason; anything else reported, its details kept in the log.
	fn refusal(e: eyre::Report) -> Refusal<Self> {
		match e.downcast_ref::<Rejected>() {
			Some(Rejected::NotFound(m)) => Refusal::new(Self::NotFound, m.clone()),
			Some(Rejected::Invalid(m)) => Refusal::new(Self::Invalid, m.clone()),
			Some(Rejected::Busy(m)) => Refusal::new(Self::Busy, m.clone()),
			Some(Rejected::Conflict(m)) => Refusal::new(Self::Conflict, m.clone()),
			None => {
				crate::report(&e, "request failed");
				Refusal::new(Self::Internal, "internal error")
			}
		}
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
struct Gmb {
	/// The conductor's id of the member's GMB.
	gmb: i64,
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

/// Every watched place.
async fn list_targets(State(s): State<AppState>) -> ApiResult<Json<Vec<TargetDto>>> {
	Ok(Json(s.archive.targets().await?.into_iter().map(TargetDto::from).collect()))
}

/// Watches a place: a place id, or a Maps URL, resolved through Places, the caller paying.
async fn add_target(State(s): State<AppState>, Vouched(assertion): Vouched, Body(req): Body<NewTarget>) -> ApiResult<Created<TargetDto>> {
	Ok(Created(s.archive.add_target(&req, &assertion).await?.target.into()))
}

/// A target with its last run, what is archived, and the next scheduled scan.
async fn get_target(State(s): State<AppState>, Ids(Id { id }): Ids<Id>) -> ApiResult<Json<TargetDetail>> {
	Ok(Json(s.archive.target_detail(TargetId(id)).await?))
}

/// Changes what the patch names; the rest stays.
async fn patch_target(State(s): State<AppState>, Ids(Id { id }): Ids<Id>, Body(patch): Body<TargetPatch>) -> ApiResult<Json<TargetDto>> {
	Ok(Json(s.archive.update_target(TargetId(id), &patch).await?.into()))
}

/// Disables the target. Nothing archived is deleted.
async fn disable_target(State(s): State<AppState>, Ids(Id { id }): Ids<Id>) -> ApiResult<Json<TargetDto>> {
	let disable = TargetPatch {
		enabled: Some(false),
		..Default::default()
	};
	Ok(Json(s.archive.update_target(TargetId(id), &disable).await?.into()))
}

/// The target's reviews as archived.
async fn list_reviews(State(s): State<AppState>, Ids(Id { id }): Ids<Id>, Params(q): Params<ReviewsQuery>) -> ApiResult<Json<Vec<ReviewDto>>> {
	Ok(Json(s.archive.reviews(TargetId(id), &q).await?))
}

/// The target's runs, newest first.
async fn list_runs(State(s): State<AppState>, Ids(Id { id }): Ids<Id>, Params(q): Params<RunsQuery>) -> ApiResult<Json<Vec<RunDto>>> {
	Ok(Json(s.archive.runs(TargetId(id), &q).await?))
}

/// Scans the target now, ahead of the scheduled scans.
///
/// The same scan already queued is not queued twice: its id comes back. Past the queue's
/// `max_queued_jobs`, 429.
async fn scan_target(State(s): State<AppState>, Ids(Id { id }): Ids<Id>) -> ApiResult<Accepted<JobAccepted>> {
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
async fn capture_place(State(s): State<AppState>, Vouched(assertion): Vouched, Params(q): Params<WaitQuery>, Body(req): Body<CaptureRequest>) -> ApiResult<Captured> {
	// subscribed before queueing, so the job cannot end unseen in between
	let mut finished = s.signals.job_finished.subscribe();
	let job_id = s.archive.enqueue_capture(&req, &assertion).await?;
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
async fn get_job(State(s): State<AppState>, Ids(Id { id }): Ids<Id>) -> ApiResult<Json<JobDto>> {
	Ok(Json(s.archive.job(id).await?))
}

/// A review with every version and capture.
async fn get_review(State(s): State<AppState>, Ids(Id { id }): Ids<Id>) -> ApiResult<Json<ReviewDetail>> {
	Ok(Json(s.archive.review(ReviewId(id)).await?))
}

/// A capture's WebP, provenance in its Exif.
///
/// Only what the archive recorded; for a member, only what shows a review of a place they track.
async fn capture_webp(State(s): State<AppState>, user: User, Caller(caller): Caller, Ids(CaptureFile { file }): Ids<CaptureFile>) -> ApiResult<Webp> {
	let sha = file.strip_suffix(".webp").ok_or_else(|| Refusal::new(Code::NotFound, "no such capture"))?;
	Ok(Webp(match user.permissions.may(sa_auth::Archive::Operate) {
		true => s.archive.capture_webp(sha).await?,
		false => s.archive.member_capture_webp(caller, sha).await?,
	}))
}

/// The target's archive: `manifest.json` and the first capture of each review.
async fn export_zip(State(s): State<AppState>, Ids(Id { id }): Ids<Id>, Params(q): Params<ExportQuery>) -> ApiResult<Zip> {
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
async fn stats(State(s): State<AppState>, WantsCsv(csv): WantsCsv, Params(q): Params<StatsQuery>) -> ApiResult<Stats> {
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
async fn add_webhook(State(s): State<AppState>, Body(hook): Body<NewWebhook>) -> ApiResult<Created<WebhookDto>> {
	Ok(Created(s.archive.add_webhook(&hook).await?))
}

/// Every hook, never its secret.
async fn list_webhooks(State(s): State<AppState>) -> ApiResult<Json<Vec<WebhookDto>>> {
	Ok(Json(s.archive.webhooks().await?))
}

/// Removes the hook and whatever it was still owed.
async fn delete_webhook(State(s): State<AppState>, Ids(Id { id }): Ids<Id>) -> ApiResult<NoContent> {
	s.archive.delete_webhook(id).await?;
	Ok(NoContent)
}

/// Every band's published layout of a dock: what a browser without one of its own starts from.
async fn list_layouts(State(s): State<AppState>, Ids(Dock { dock }): Ids<Dock>) -> ApiResult<Json<Vec<LayoutDto>>> {
	Ok(Json(s.archive.layouts(&dock).await?))
}

/// Publishes a dock's layout at a band for every browser without one of its own.
async fn publish_layout(State(s): State<AppState>, Ids(Slot { dock, band }): Ids<Slot>, Body(layout): Body<NewLayout>) -> ApiResult<NoContent> {
	s.archive.publish_layout(&dock, &band, &layout).await?;
	Ok(NoContent)
}

/// Who is signed in.
async fn me(State(s): State<AppState>, user: User, Caller(caller): Caller) -> ApiResult<Json<Me>> {
	let mine = s.archive.store()?.person_by_id(caller).await?.expect("the gate's vouch found or made them");
	Ok(Json(Me {
		id: caller.0,
		email: mine.email,
		name: mine.name,
		permissions: user.permissions.iter().map(str::to_owned).collect(),
	}))
}

/// The member's gmails, each with its GMBs and their places as tracked, by screenshots over the last 7 days.
async fn my_overview(State(s): State<AppState>, Caller(m): Caller) -> ApiResult<Json<Vec<GmailOverview>>> {
	Ok(Json(s.archive.overview(m).await?))
}

/// The archive's tag on each of the member's GMBs, for the conductor's page.
async fn my_gmbs(State(s): State<AppState>, Caller(m): Caller) -> ApiResult<Json<MyGmbs>> {
	Ok(Json(s.archive.my_gmbs(m).await?))
}

/// Whether reviews stick: of those posted since their place's first scan, the share still listed 1 to 90 days after,
/// over the member's GMBs and by stars, by posts a day, by week and by GMB.
async fn my_stick(State(s): State<AppState>, Caller(m): Caller, Params(q): Params<StickQuery>) -> ApiResult<Json<StickReport>> {
	Ok(Json(s.archive.stick(m, &q).await?))
}

/// What the member chose, each beside its default.
async fn my_settings(State(s): State<AppState>, Caller(m): Caller) -> ApiResult<Json<Settings>> {
	Ok(Json(s.archive.settings(m).await?))
}

/// Chooses what the patch names; the rest stays.
async fn patch_my_settings(State(s): State<AppState>, Caller(m): Caller, Body(req): Body<SettingsPatch>) -> ApiResult<Json<Settings>> {
	Ok(Json(s.archive.patch_settings(m, &req).await?))
}

/// Every setting back to its default.
async fn reset_my_settings(State(s): State<AppState>, Caller(m): Caller) -> ApiResult<Json<Settings>> {
	Ok(Json(s.archive.reset_settings(m).await?))
}

/// Switches the GMB's track of a place on or off, or marks it a favorite; the place's target stays as it is.
async fn patch_track(State(s): State<AppState>, Caller(m): Caller, Ids(Track { gmb, target }): Ids<Track>, Body(req): Body<TrackPatch>) -> ApiResult<NoContent> {
	s.archive.patch_track(m, gmb, TargetId(target), &req).await?;
	Ok(NoContent)
}

/// Asks Google about a tracked place again: its id, name and address become what Google gives now,
/// the member paying as their GMB's gmail.
async fn relink_track(State(s): State<AppState>, Caller(m): Caller, Vouched(assertion): Vouched, Ids(Track { gmb, target }): Ids<Track>) -> ApiResult<Json<TargetDto>> {
	Ok(Json(s.archive.relink(m, gmb, TargetId(target), &assertion).await?))
}

/// Queues a scan of a tracked place, on top of its schedule; the member who asks pays for it in tokens.
///
/// 429 when the GMB is switched off or not verified, Maps is paused, the member is out of
/// tokens, or the queue is full.
async fn scan_track(State(s): State<AppState>, Spends(_, by): Spends<WalkStep, Caller>, Ids(Track { gmb, target }): Ids<Track>) -> ApiResult<Accepted<JobAccepted>> {
	let job_id = s.archive.scan_tracked(by, gmb, TargetId(target)).await?;
	s.signals.job_queued.notify_one();
	Ok(Accepted(JobAccepted { job_id }))
}

/// A tracked place's reviews in four columns: snapshotted, suspended, removed, reinstating.
async fn track_board(State(s): State<AppState>, Caller(m): Caller, Ids(Track { gmb, target }): Ids<Track>) -> ApiResult<Json<Board>> {
	Ok(Json(s.archive.board(m, gmb, TargetId(target)).await?))
}

/// What happened to the GMB's reviews and scans: reviews that appeared, went (suspended with 5 stars, else removed) or
/// returned, appeals asked and withdrawn, failed scans of its place. Whole UTC days, newest first; `next` pages back.
async fn gmb_events(State(s): State<AppState>, Caller(m): Caller, Ids(Gmb { gmb }): Ids<Gmb>, Params(q): Params<EventsQuery>) -> ApiResult<Json<GmbEvents>> {
	Ok(Json(s.archive.gmb_events(m, gmb, q.before.as_deref()).await?))
}

/// A tracked place's runs, newest first.
async fn track_runs(State(s): State<AppState>, Caller(m): Caller, Ids(Track { gmb, target }): Ids<Track>, Params(q): Params<RunsQuery>) -> ApiResult<Json<Vec<RunDto>>> {
	Ok(Json(s.archive.track_runs(m, gmb, TargetId(target), &q).await?))
}

/// Records that the removed review's reinstatement was asked of Google, now.
///
/// Asking again while one is open answers the open one.
async fn reinstate(State(s): State<AppState>, Caller(m): Caller, Ids(Appeal { gmb, review }): Ids<Appeal>) -> ApiResult<Json<ReinstatementDto>> {
	Ok(Json(s.archive.reinstate(m, gmb, ReviewId(review)).await?))
}

/// Withdraws the open request; it stays on record as withdrawn.
async fn withdraw_reinstatement(State(s): State<AppState>, Caller(m): Caller, Ids(Appeal { gmb, review }): Ids<Appeal>) -> ApiResult<NoContent> {
	s.archive.withdraw_reinstatement(m, gmb, ReviewId(review)).await?;
	Ok(NoContent)
}

/// What the archive commits, as it commits: `review_archive.<table>` per table written.
async fn changes(_: Conductor, State(s): State<AppState>) -> ApiResult<Response> {
	let mut end = s.signals.feeds_end.subscribe();
	let topics = s.archive.store()?.changes();
	Ok(feeds::changes(topics, async move {
		// a dropped sender is the process going away: an end as well
		let _ = end.changed().await;
	}))
}

/// The conductor's GMBs and balances, taken whole or not at all.
async fn apply_snapshot(_: Conductor, State(s): State<AppState>, Body(snap): Body<Snapshot>) -> ApiResult<StatusCode> {
	s.archive.apply_snapshot(&snap).await?;
	Ok(StatusCode::NO_CONTENT)
}

/// A GMB's reviews currently gone, newest first; 404 while no track is its.
async fn gone(_: Conductor, State(s): State<AppState>, Ids(gmb): Ids<i64>) -> ApiResult<Json<Vec<feeds::GoneReview>>> {
	Ok(Json(s.archive.gone(gmb).await?))
}

/// What walks cost, for the conductor to charge: the feed past `after`.
async fn charges(_: Conductor, State(s): State<AppState>, Params(q): Params<FeedQuery>) -> ApiResult<Json<ChargePage>> {
	Ok(Json(s.archive.charges(q.after, q.limit).await?))
}

/// What walks did, whoever paid, for the conductor's costs: the feed past `after`.
async fn usage(_: Conductor, State(s): State<AppState>, Params(q): Params<FeedQuery>) -> ApiResult<Json<UsagePage>> {
	Ok(Json(s.archive.usage(q.after, q.limit).await?))
}

/// Each tracked place's reviews as they appear, go and come back: the feed past `after`.
async fn review_feed(_: Conductor, State(s): State<AppState>, Params(q): Params<FeedQuery>) -> ApiResult<Json<feeds::ReviewPage>> {
	Ok(Json(s.archive.review_feed(q.after, q.limit).await?))
}

/// The Alerts feed past `after`: notes on the places live tracks name, each with their GMBs.
async fn notifications(_: Conductor, State(s): State<AppState>, Params(q): Params<FeedQuery>) -> ApiResult<Json<sa_alerts::NotePage>> {
	Ok(Json(s.archive.notifications(q.after, q.limit).await?))
}

/// A capture as PNG, the photo of a `review_gone` note.
async fn capture_png(_: Conductor, State(s): State<AppState>, Ids(file): Ids<String>) -> ApiResult<Response> {
	let sha = file.strip_suffix(".png").ok_or_else(|| Refusal::new(Code::NotFound, "no such capture"))?;
	Ok((
		[(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=31536000, immutable")],
		s.archive.capture_png(sha).await?,
	)
		.into_response())
}
