//! The JSON the archive speaks: what the HTTP API returns and the CLI prints. Shared by
//! the server and the client, so the two cannot drift apart.
//!
//! Timestamps are strings in [`crate::fmt_ts`]'s shape.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::{Target, TargetKind, fmt_ts};

/// A watched place.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TargetDto {
	/// Its id in the archive.
	pub id: i64,
	/// What it is shown as: a person's label, else the place's name, else its place id.
	pub label: String,
	/// `maps` or `gbp`.
	pub kind: TargetKind,
	/// The Google place id.
	pub place_id: String,
	/// The place's address as Google last showed it.
	pub address: Option<String>,
	/// Set for `gbp` targets.
	pub gbp_account: Option<String>,
	/// Set for `gbp` targets.
	pub gbp_location: Option<String>,
	/// UI language of the Maps page.
	pub lang: String,
	/// Seconds between scans, before jitter.
	pub interval_secs: u64,
	/// Disabled targets keep their archive but are not scanned.
	pub enabled: bool,
	/// When it was added.
	pub created_at: String,
}

impl From<Target> for TargetDto {
	fn from(t: Target) -> Self {
		let label = t.to_string();
		let (gbp_account, gbp_location) = t.gbp.map(|g| (g.account, g.location)).unzip();
		Self {
			id: t.id.0,
			label,
			kind: t.kind,
			place_id: t.place_id,
			address: t.address,
			gbp_account,
			gbp_location,
			lang: t.lang,
			interval_secs: t.interval.duration().as_secs(),
			enabled: t.enabled,
			created_at: fmt_ts(t.created_at),
		}
	}
}

/// A review as archived.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReviewDto {
	/// Its id in the archive.
	pub id: i64,
	/// The target it belongs to.
	pub target_id: i64,
	/// The source's id for it.
	pub source_review_id: String,
	/// The reviewer's display name.
	pub author: String,
	/// The reviewer's public profile.
	pub author_url: Option<String>,
	/// 1–5 stars, as last seen.
	pub rating: Option<i64>,
	/// The text, as last seen.
	pub text: Option<String>,
	/// The owner's response, as last seen.
	pub reply: Option<String>,
	/// Photos attached.
	pub photo_count: i64,
	/// The date as the source printed it when `published_est` was made from it.
	pub published_raw: Option<String>,
	/// When it was published, estimated when first seen.
	pub published_est: Option<String>,
	/// When the archive first saw it.
	pub first_seen: String,
	/// When a scan last listed it.
	pub last_seen: String,
	/// Set while a scan that looked where it was did not list it.
	pub gone_at: Option<String>,
	/// No screenshot of it yet.
	pub capture_pending: bool,
	/// The first capture's hash, the one that shows the review as it first appeared; the first
	/// corrupted one only while no other was taken.
	pub capture_sha256: Option<String>,
	/// When that capture was taken.
	pub captured_at: Option<String>,
	/// Where the API serves that capture: `/captures/<sha256>.webp`.
	pub capture_url: Option<String>,
	/// See [`CaptureDto::corrupted`].
	pub capture_corrupted: Option<bool>,
}

/// Where the API serves a capture.
pub fn capture_url(sha256: &str) -> String {
	format!("/captures/{sha256}.webp")
}

/// One target's activity on one UTC day.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DayStats {
	/// The target.
	pub target_id: i64,
	/// `YYYY-MM-DD`.
	pub day: String,
	/// Reviews first seen that day.
	pub new: i64,
	/// Edits seen that day.
	pub changed: i64,
	/// Reviews that went missing that day, whether or not they came back later.
	pub gone: i64,
	/// Mean rating, as first seen, of the reviews first seen that day.
	pub mean_rating: Option<f64>,
	/// Count of reviews first seen that day per star, index 0 = one star.
	pub histogram: [i64; 5],
}

/// How a scan went. Stored as its lowercase name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, strum::AsRefStr, strum::EnumString)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum RunStatus {
	/// Everything it tried worked.
	Ok,
	/// Stored what it saw, with warnings (a missed screenshot, a walk cut short).
	Partial,
	/// Nothing stored; see the error.
	Failed,
}

/// What one scan did to the archive.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Counts {
	/// Distinct reviews listed.
	pub seen: u32,
	/// Archived for the first time.
	pub new: u32,
	/// A new version recorded.
	pub changed: u32,
	/// Newly marked gone.
	pub gone: u32,
}

/// One scan of one target, as reported.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunSummary {
	/// The target.
	pub target: i64,
	/// Its label.
	pub label: String,
	/// How it went.
	pub status: RunStatus,
	/// What it changed.
	#[serde(flatten)]
	pub counts: Counts,
	/// Screenshots stored.
	pub captured: u32,
	/// Whether it saw the whole list.
	pub complete: bool,
	/// The failure, or the warnings of a partial run.
	pub error: Option<String>,
}

impl fmt::Display for RunSummary {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let status = if self.status == RunStatus::Failed { "FAILED" } else { self.status.as_ref() };
		write!(
			f,
			"#{} {:<24} {status:<8} seen {:>4}  new {:>4}  changed {:>3}  gone {:>3}  captured {:>4}{}",
			self.target,
			self.label,
			self.counts.seen,
			self.counts.new,
			self.counts.changed,
			self.counts.gone,
			self.captured,
			if self.complete { "  (whole list)" } else { "" }
		)?;
		if let Some(e) = &self.error {
			write!(f, "\n    {e}")?;
		}
		Ok(())
	}
}

/// `POST /targets`: watch a place. One of `place` and `maps_url`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct NewTarget {
	/// A place id (or a Maps URL; either field takes both).
	pub place: Option<String>,
	/// A Google Maps URL; resolved with the Places API when it carries no place id.
	pub maps_url: Option<String>,
	/// The place's name when unset.
	pub label: Option<String>,
	/// UI language of the Maps page, e.g. `fr`.
	pub lang: Option<String>,
	/// `6h`, `1d`, or seconds; at least an hour.
	pub interval: Option<String>,
	/// `<account>/<location>`: read reviews through the Business Profile API.
	pub gbp: Option<String>,
}

/// `PATCH /targets/{id}`: what to change; absent fields stay.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct TargetPatch {
	/// New label.
	pub label: Option<String>,
	/// New Maps UI language.
	pub lang: Option<String>,
	/// New interval: `6h`, `1d`, or seconds.
	pub interval: Option<String>,
	/// Enable or disable scanning.
	pub enabled: Option<bool>,
}

/// A target with how it is doing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TargetDetail {
	/// The target.
	#[serde(flatten)]
	pub target: TargetDto,
	/// Its latest finished run.
	pub last_run: Option<RunDto>,
	/// Reviews archived.
	pub reviews: i64,
	/// Of which currently gone.
	pub gone: i64,
	/// Screenshots archived.
	pub captures: i64,
	/// When the scheduler scans it next; `null` for a disabled target.
	pub next_scan_at: Option<String>,
}

/// A run of a scan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunDto {
	/// Its id.
	pub id: i64,
	/// The target scanned.
	pub target_id: i64,
	/// When it began.
	pub started_at: String,
	/// When it ended; `null` while running.
	pub finished_at: Option<String>,
	/// `null` until it ends.
	pub status: Option<RunStatus>,
	/// The failure, or the warnings of a partial run.
	pub error: Option<String>,
	/// What it changed.
	#[serde(flatten)]
	pub counts: Counts,
	/// What its walk cost, paid or not.
	pub tokens: i64,
}

/// What a job does. Stored as its lowercase name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, strum::AsRefStr, strum::Display, strum::EnumString)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum JobKind {
	/// A registered target's scan, now.
	Scan,
	/// An ad-hoc capture of a place.
	Capture,
}

/// Where a job is. Stored as its lowercase name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, strum::AsRefStr, strum::Display, strum::EnumString)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum JobStatus {
	/// Waiting for the browser.
	Queued,
	/// In the browser now.
	Running,
	/// Finished; `reviews` holds what it saw.
	Done,
	/// See `error`.
	Failed,
}

impl JobStatus {
	/// Done or failed: it will not change again.
	pub fn is_finished(self) -> bool {
		matches!(self, Self::Done | Self::Failed)
	}
}

/// `202`: the job was queued.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobAccepted {
	/// Poll `GET /jobs/{job_id}`.
	pub job_id: i64,
}

/// A job and, once done, what it saw.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobDto {
	/// Its id.
	pub id: i64,
	/// What it does.
	pub kind: JobKind,
	/// Where it is.
	pub status: JobStatus,
	/// The target it scans (for a capture, the implicit one its results are kept under).
	pub target_id: i64,
	/// When it was queued.
	pub created_at: String,
	/// When the browser took it.
	pub started_at: Option<String>,
	/// When it ended.
	pub finished_at: Option<String>,
	/// Why it failed.
	pub error: Option<String>,
	/// The run it made.
	pub run: Option<RunDto>,
	/// On `done`: the reviews the run listed, newest first, with their capture URLs — only
	/// the requested ones, for a capture of named reviews.
	pub reviews: Option<Vec<ReviewDto>>,
}

/// What an ad-hoc capture reads.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct CaptureLimits {
	/// Cards read at most; the per-scan default when unset, `defaults.max_reviews_initial`
	/// at most.
	pub max_reviews: Option<usize>,
	/// Only these Google review ids; the walk stops once all are found.
	pub review_ids: Option<Vec<String>>,
}

/// `POST /captures`: capture a place without registering it. One of `place` and `maps_url`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct CaptureRequest {
	/// A place id (or a Maps URL).
	pub place: Option<String>,
	/// A Google Maps URL.
	pub maps_url: Option<String>,
	/// UI language of the Maps page.
	pub lang: Option<String>,
	/// How much to read.
	#[serde(flatten)]
	pub limits: CaptureLimits,
}

/// One version of a review's content.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VersionDto {
	/// When this content was first seen.
	pub seen_at: String,
	/// Its hash.
	pub content_hash: String,
	/// Stars.
	pub rating: Option<i64>,
	/// Text.
	pub text: Option<String>,
	/// Owner's reply.
	pub reply: Option<String>,
}

/// A stored screenshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CaptureDto {
	/// Its hash, and its name in the blob store.
	pub sha256: String,
	/// When it was taken.
	pub captured_at: String,
	/// Pixels.
	pub width: i64,
	/// Pixels.
	pub height: i64,
	/// The page it was taken on.
	pub page_url: String,
	/// What took it.
	pub scanner_version: String,
	/// The card was shot shifted, its left part cut off; `None` until the archive has looked.
	pub corrupted: Option<bool>,
	/// `/captures/<sha256>.webp`.
	pub url: String,
}

/// A review with its full history.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReviewDetail {
	/// The review as last seen.
	#[serde(flatten)]
	pub review: ReviewDto,
	/// Every distinct content, oldest first.
	pub versions: Vec<VersionDto>,
	/// Every screenshot, oldest first.
	pub captures: Vec<CaptureDto>,
}

/// What a webhook can subscribe to. Stored and sent (`X-Event`) under its dotted name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, strum::AsRefStr, strum::EnumString)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Event {
	/// A review archived for the first time.
	#[serde(rename = "review.new")]
	#[strum(serialize = "review.new")]
	ReviewNew,
	/// A new version of a review.
	#[serde(rename = "review.changed")]
	#[strum(serialize = "review.changed")]
	ReviewChanged,
	/// A review no longer listed where a scan looked.
	#[serde(rename = "review.gone")]
	#[strum(serialize = "review.gone")]
	ReviewGone,
	/// A gone review listed again.
	#[serde(rename = "review.reappeared")]
	#[strum(serialize = "review.reappeared")]
	ReviewReappeared,
	/// A scan failed.
	#[serde(rename = "run.failed")]
	#[strum(serialize = "run.failed")]
	RunFailed,
}

/// `POST /webhooks`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewWebhook {
	/// Where deliveries are POSTed; `http` or `https`, to a host the archive allows.
	pub url: String,
	/// What to deliver.
	pub events: Vec<Event>,
	/// The HMAC-SHA256 key deliveries are signed with (`X-Signature: sha256=<hex>`); 16+
	/// characters. Never returned.
	pub secret: String,
}

/// A dock layout published as the one fresh browsers start from: `GET /layouts/{dock}`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LayoutDto {
	/// `sm`, `md` or `xl`: the dock's width band.
	pub band: String,
	/// The dock's own layout JSON.
	pub layout: String,
}

/// `PUT /layouts/{dock}/{band}`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewLayout {
	/// The dock's own layout JSON.
	pub layout: String,
}

/// A webhook, without its secret.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WebhookDto {
	/// Its id.
	pub id: i64,
	/// Where deliveries go.
	pub url: String,
	/// What it gets.
	pub events: Vec<Event>,
	/// When it was added.
	pub created_at: String,
}

/// The body of a webhook delivery.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventPayload {
	/// What happened.
	pub event: Event,
	/// When the archive recorded it.
	pub occurred_at: String,
	/// The target.
	pub target_id: i64,
	/// The review, for `review.*`.
	pub review: Option<ReviewDto>,
	/// The run, for `run.failed`.
	pub run: Option<RunDto>,
}

/// An error response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ErrorBody {
	/// What went wrong; `internal error` for a 5xx, whose details stay in the server log.
	pub error: String,
}

/// `GET /targets/{id}/reviews`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams), into_params(parameter_in = Query))]
#[serde(default)]
pub struct ReviewsQuery {
	/// `YYYY-MM-DD` or RFC 3339: first seen at or after.
	pub since: Option<String>,
	/// Only gone (`true`) or only listed (`false`) reviews.
	pub gone: Option<bool>,
}

/// `GET /targets/{id}/runs`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams), into_params(parameter_in = Query))]
#[serde(default)]
pub struct RunsQuery {
	/// At most this many, newest first (default 20, at most 500).
	pub limit: Option<u32>,
}

/// `GET /targets/{id}/export.zip`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams), into_params(parameter_in = Query))]
#[serde(default)]
pub struct ExportQuery {
	/// `YYYY-MM-DD` or RFC 3339: reviews first seen at or after.
	pub since: Option<String>,
}

/// `POST /captures`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams), into_params(parameter_in = Query))]
#[serde(default)]
pub struct WaitQuery {
	/// Hold the request up to this many seconds (at most 120) and answer with the finished
	/// job if it finishes by then.
	pub wait: Option<u64>,
}

/// `GET /stats`.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams), into_params(parameter_in = Query))]
#[serde(default)]
pub struct StatsQuery {
	/// One target; every target when unset.
	pub target: Option<i64>,
	/// `YYYY-MM-DD`, inclusive.
	pub from: Option<String>,
	/// `YYYY-MM-DD`, inclusive.
	pub to: Option<String>,
}

/// Day stats as CSV, one row per target and day, the histogram as `stars_1` … `stars_5`.
pub fn stats_csv(rows: &[DayStats]) -> eyre::Result<String> {
	let mut w = csv::Writer::from_writer(Vec::new());
	w.write_record(["target_id", "day", "new", "changed", "gone", "mean_rating", "stars_1", "stars_2", "stars_3", "stars_4", "stars_5"])?;
	for r in rows {
		let mut rec = vec![
			r.target_id.to_string(),
			r.day.clone(),
			r.new.to_string(),
			r.changed.to_string(),
			r.gone.to_string(),
			r.mean_rating.map(|m| format!("{m:.3}")).unwrap_or_default(),
		];
		rec.extend(r.histogram.iter().map(ToString::to_string));
		w.write_record(&rec)?;
	}
	Ok(String::from_utf8(w.into_inner()?)?)
}

/// A managing gmail: the Google manager account a member's GMBs are attached to, as the
/// conductor last told of it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GmailDto {
	/// The conductor's id.
	pub id: i64,
	/// The address.
	pub gmail: String,
}

/// `PATCH /me/gmbs/{gmb}/tracks/{target}`: what to change; absent fields stay.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct TrackPatch {
	/// Switch the track on or off.
	pub enabled: Option<bool>,
	/// Mark the place a favorite of this GMB's, or unmark it.
	pub favorite: Option<bool>,
}

/// A tracked place, as the member's overview shows it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LocationSummary {
	/// The place.
	pub target: TargetDto,
	/// This GMB's track of it is on; it is scanned while any track of it is.
	pub enabled: bool,
	/// Marked a favorite on this GMB.
	pub favorite: bool,
	/// Reviews posted over the last 7 days, as their dates are estimated; gone ones too.
	pub new_7d: i64,
	/// Owner's posts published over the last 7 days, as their dates are estimated.
	pub posts_7d: i64,
	/// The owner's post seen most recently first.
	pub latest_post: Option<PostDto>,
	/// Screenshots taken over the last 7 days.
	pub snapshots_7d: i64,
	/// Screenshots taken over the last 30 days.
	pub snapshots_30d: i64,
	/// Reviews listed now.
	pub live: i64,
	/// Of which the owner has replied to.
	pub responded: i64,
	/// Of which rated under 5 stars, or not rated.
	pub non_five_star: i64,
	/// Reviews with a screenshot stored, gone ones too.
	pub captured: i64,
	/// Of which every screenshot is corrupted.
	pub corrupted: i64,
	/// How many reviews Google says the place has, as of the last scan that read it.
	pub listed: Option<i64>,
	/// Its star average, as Google gave it on the last scan that read it.
	pub rating: Option<f64>,
	/// Gone with 5 stars, with no open appeal under this GMB.
	pub suspended: i64,
	/// Gone with fewer stars, or none, with no open appeal under this GMB.
	pub removed: i64,
	/// Gone, with an open appeal under this GMB.
	pub reinstating: i64,
	/// When its latest finished run ended.
	pub last_run_at: Option<String>,
	/// How it went.
	pub last_run_status: Option<RunStatus>,
	/// Why it failed, or what it warned of: the first line of its `error`.
	pub last_run_error: Option<String>,
	/// Not scanned: every member tracking it is out of tokens.
	pub held: bool,
	/// A scan of it under way or waiting.
	pub scan: Option<ScanState>,
	/// What its latest scheduled walk spent; a first screen's worth for a place never walked.
	pub scan_cost: i64,
}

/// A scan of a place under way or waiting.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ScanState {
	/// Waiting for the browser.
	Queued {
		/// When it was asked for.
		since: String,
		/// Jobs before it in the queue.
		ahead: i64,
	},
	/// Walking.
	Running {
		/// When it started.
		since: String,
	},
}

/// A post by a place's owner.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PostDto {
	/// Line breaks kept.
	pub text: String,
	/// When it was posted, estimated when first seen; `null` when the page printed a date.
	pub published_est: Option<String>,
	/// When the archive first saw it.
	pub first_seen: String,
}

/// A GMB: one place a gmail manages, and the archive's tracks of it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GmbSummary {
	/// The conductor's id.
	pub id: i64,
	/// The Google place id.
	pub place_id: String,
	/// The conductor's switch; off, none of its tracks is walked, whatever their own switches.
	pub enabled: bool,
	/// Its places as tracked here, a target per language and source, by snapshots over 7 days,
	/// most first; empty: not tracked.
	pub tracks: Vec<LocationSummary>,
}

/// A gmail and its GMBs.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GmailOverview {
	/// The gmail.
	#[serde(flatten)]
	pub gmail: GmailDto,
	/// Its GMBs, by id.
	pub gmbs: Vec<GmbSummary>,
}

/// An appeal of a removed review.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReinstatementDto {
	/// When it was requested.
	pub requested_at: String,
	/// When a scan listed the review again, if one has.
	pub reinstated_at: Option<String>,
}

/// A review on a board, with this GMB's latest appeal of it that was not withdrawn.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BoardCard {
	/// The review.
	pub review: ReviewDto,
	/// The appeal.
	pub reinstatement: Option<ReinstatementDto>,
}

/// One tracked place's reviews in three columns.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Board {
	/// Listed now, newest first sighting first; a reinstated one carries its appeal.
	pub snapshotted: Vec<BoardCard>,
	/// Gone with 5 stars, without an open appeal, most recently gone first.
	pub suspended: Vec<BoardCard>,
	/// Gone with fewer stars, or none, without an open appeal, most recently gone first.
	pub removed: Vec<BoardCard>,
	/// Gone with an open appeal, most recently requested first.
	pub reinstating: Vec<BoardCard>,
}

/// `GET /me/settings`: what the member chose, each beside what they get without choosing.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Settings {
	/// Seconds between scans of each place the member pays for; a place several pay for goes by
	/// the shortest.
	pub interval_secs: Setting<u64>,
}

/// One of [`Settings`].
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Setting<T> {
	/// `null`: never chosen, or reset.
	pub chosen: Option<T>,
	/// The service's.
	pub default: T,
}

impl<T: Copy> Setting<T> {
	/// What applies.
	pub fn value(&self) -> T {
		self.chosen.unwrap_or(self.default)
	}
}

/// `PATCH /me/settings`: absent fields stay; `DELETE /me/settings` resets them all.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct SettingsPatch {
	/// `6h`, `1d`, or seconds; at least `schedule.min_interval`.
	pub interval: Option<String>,
}

/// `POST /me/gmbs/{gmb}/tracks`: watch the GMB's place. The place is watched once however
/// many members track it; one already watched with this language and source is reused.
#[skip_serializing_none]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct NewTrack {
	/// The place's name when unset.
	pub label: Option<String>,
	/// UI language of the Maps page, e.g. `fr`.
	pub lang: Option<String>,
	/// `<account>/<location>`: read reviews through the Business Profile API, which needs
	/// the archive's Google account among the location's managers.
	pub gbp: Option<String>,
}

/// A member's Telegram channel for events.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TgChannelDto {
	/// Its id.
	pub id: i64,
	/// As pasted: `@channel`, `-100…`, or `<group>/<topic>`.
	pub destination: String,
	/// Only this gmail's places; every place the member tracks when unset.
	pub gmail_id: Option<i64>,
	/// What it gets.
	pub events: Vec<Event>,
	/// When it was added.
	pub created_at: String,
}

/// `POST /me/tg-channels`. The archive's bot has to be in the chat and allowed to post.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewTgChannel {
	/// `@channel`, a `-100…` chat id, or `<group>/<topic>` for a forum topic.
	pub destination: String,
	/// Only this gmail's places; every place the member tracks when unset.
	pub gmail_id: Option<i64>,
	/// What to send.
	pub events: Vec<Event>,
}

/// `GET /me`: who is signed in.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Me {
	/// Their person id, what `X-Member` and `/members/{id}` name.
	pub id: i64,
	/// Their concierge email.
	pub email: String,
	/// Their concierge name.
	pub name: String,
	/// What they may do here: `sa:review_archive:*` permissions.
	pub permissions: Vec<String>,
}

/// `GET /members`: someone who signed in here, or an address from before people had ids.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MemberDto {
	/// Their person id.
	pub id: i64,
	/// Their concierge email, as of their last sign-in.
	pub email: String,
	/// Empty until they sign in.
	pub name: String,
	/// `false`: nobody signed in with this address since people got ids.
	pub claimed: bool,
	/// The tokens they hold, as the archive reckons them: the conductor's last word on their
	/// balance less the walks charged since.
	pub balance: i64,
}

/// `GET /me/gmbs`: the archive's tag on each of the member's GMBs, for the conductor's page.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MyGmbs {
	/// By GMB id; removed GMBs are left out.
	pub gmbs: std::collections::BTreeMap<i64, GmbReviews>,
	/// Places with a track on under a GMB switched on, now.
	pub places_tracked: i64,
}

/// A GMB's reviews, as [`MyGmbs`] tells them.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GmbReviews {
	/// It is switched on, and a track of it is on.
	pub tracked: bool,
	/// The place's star average, from its most recently walked tracked target; `null` when never walked.
	pub rating: Option<f64>,
	/// How many reviews Google lists for it, likewise.
	pub reviews: Option<i64>,
}

/// A GMB as the conductor holds it (`PUT /api/conductor/gmbs`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct MirroredGmb {
	/// The conductor's id.
	pub id: i64,
	/// Its gmail's id.
	pub gmail_id: i64,
	/// Its gmail's address.
	pub gmail: String,
	/// The gmail's owner, a concierge `sub`.
	pub owner: String,
	/// The GMB, or its gmail, is removed.
	pub removed: bool,
	/// The conductor's switch; off, the archive walks none of its tracks.
	pub enabled: bool,
	/// The Google place id.
	pub place_id: String,
}

/// `PUT /api/conductor/gmbs`: everything the conductor knows of GMBs and balances, at once.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct GmbSnapshot {
	/// Every GMB it ever had, removed ones included.
	pub gmbs: Vec<MirroredGmb>,
	/// Every person's balance by `sub`, renewed up to now.
	pub balances: std::collections::BTreeMap<String, i64>,
	/// The last [`ChargeDto::id`] those balances include.
	pub charges_through: i64,
}

/// What a walk cost, for the conductor to charge (`GET /api/conductor/charges`).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChargeDto {
	/// Ascending, never reused.
	pub id: i64,
	/// `run:<run id>`.
	pub key: String,
	/// Who pays, by `sub`; never empty.
	pub payers: Vec<String>,
	/// Tokens; more than 0.
	pub spent: i64,
	/// The run failed or met a failure: what each paid is owed back.
	pub kick_back: bool,
	/// The place, as the ledger names it.
	pub label: String,
	/// When the run ended.
	pub at: String,
}

/// A page of the charge feed.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Charges {
	/// Ascending by id.
	pub charges: Vec<ChargeDto>,
	/// The last id of the page; `after` for an empty one.
	pub next: i64,
}

/// `GET /api/conductor/charges`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams), into_params(parameter_in = Query))]
pub struct ChargesQuery {
	/// Charges past this id.
	pub after: i64,
	/// At most this many, 1 to [`MAX_CHARGES`].
	pub limit: u32,
}

/// The largest page of the charge feed.
pub const MAX_CHARGES: u32 = 500;

/// On `/me` routes: the person id of the member an admin acts as.
pub const MEMBER_HEADER: &str = "x-member";
