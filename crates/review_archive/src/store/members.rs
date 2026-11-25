//! What is a member's own: what their GMBs (the conductor's, mirrored) track, and their
//! appeals. Every query here is scoped by the member; the archive under it
//! (targets, reviews, captures) stays shared.

use std::collections::HashMap;

use eyre::WrapErr;
use jiff::{SignedDuration, Timestamp};
use review_archive_core::{
	PersonId, Rejected, ReviewId, Target, TargetId,
	dto::{
		Board, BoardCard, DayReviews, GmailDto, GmailOverview, GmbEvent, GmbEvents, GmbReviews, GmbSummary, Happened, LocationSummary, MyGmbs, PostDto, ReinstatementDto, ReviewDto,
		ReviewRef, ScanState, StickReport, TrackPatch,
	},
	fmt_ts,
	stick::{self, Tracked, Watched},
	tokens::WalkPrices,
};
use sa_core::feeds::{GONE_MAX, GoneReview};
use sqlx::{FromRow, SqliteConnection};

use super::{REVIEW_SELECT, ReviewRow, Store, TargetRow, parse_ts};

/// The rows a page of [`Store::gmb_events`] aims at; it grows to hold a whole day.
const EVENTS_PAGE: u32 = 200;

/// Days in [`GmbReviews::live_30d`].
const LIVE_DAYS: usize = 30;

/// The target GMB `m`'s figures are from: its most recently walked tracked one. A GMB's
/// tracks in other languages list the same reviews, so its figures are never summed over them.
const WORKING: &str = "SELECT k.target_id FROM tracks k JOIN runs r ON r.target_id = k.target_id
	 WHERE k.gmb_id = m.id AND r.status IN ('ok', 'partial') AND r.ad_hoc = 0
	 ORDER BY r.finished_at DESC, r.id DESC LIMIT 1";

/// The ids of the GMBs of person `?1`, not removed.
const MEMBERS_GMBS: &str = "SELECT m.id FROM gmb_mirror m JOIN people p ON p.sub = m.owner WHERE p.id = ?1 AND NOT m.removed";

/// GMB `?` if it is person `?`'s and not removed, as a scalar subquery.
const MEMBERS_GMB: &str = "SELECT m.id FROM gmb_mirror m JOIN people p ON p.sub = m.owner WHERE m.id = ? AND p.id = ? AND NOT m.removed";

#[derive(FromRow)]
struct LocationRow {
	gmb_id: i64,
	#[sqlx(flatten)]
	target: TargetRow,
	track_enabled: bool,
	favorite: bool,
	new_7d: i64,
	posts_7d: i64,
	post_text: Option<String>,
	post_published_est: Option<String>,
	post_first_seen: Option<String>,
	snapshots_7d: i64,
	snapshots_30d: i64,
	live: i64,
	responded: i64,
	non_five_star: i64,
	captured: i64,
	corrupted: i64,
	listed: Option<i64>,
	rating: Option<f64>,
	gone: i64,
	suspended: i64,
	reinstating: i64,
	last_run_at: Option<String>,
	last_run_status: Option<String>,
	last_run_error: Option<String>,
	running_since: Option<String>,
	queued_since: Option<String>,
	queued_ahead: i64,
	scan_cost: Option<i64>,
}

#[derive(FromRow)]
struct CardRow {
	#[sqlx(flatten)]
	review: ReviewRow,
	requested_at: Option<String>,
	reinstated_at: Option<String>,
}

fn no_gmb(id: i64) -> Rejected {
	Rejected::not_found(format!("no GMB {id}"))
}

fn no_track(gmb: i64, target: TargetId) -> Rejected {
	Rejected::not_found(format!("GMB {gmb} does not track target {target}"))
}

/// A target whose last track went comes off the schedule, and is purged once `defaults.untracked_retention` passes.
pub(super) async fn left_untracked(conn: &mut SqliteConnection, target: TargetId, now: Timestamp) -> eyre::Result<()> {
	sqlx::query("UPDATE targets SET untracked_at = ?, enabled = 0 WHERE id = ?2 AND NOT EXISTS (SELECT 1 FROM tracks WHERE target_id = ?2)")
		.bind(fmt_ts(now))
		.bind(target.0)
		.execute(conn)
		.await
		.wrap_err("marking a target untracked")?;
	Ok(())
}

impl Store {
	/// Changes a member's track; absent fields stay.
	pub async fn patch_track(&self, member: PersonId, gmb: i64, target: TargetId, patch: &TrackPatch) -> eyre::Result<()> {
		let done = sqlx::query(sqlx::AssertSqlSafe(format!(
			"UPDATE tracks SET enabled = COALESCE(?, enabled), favorite = COALESCE(?, favorite) WHERE gmb_id = ({MEMBERS_GMB}) AND target_id = ?"
		)))
		.bind(patch.enabled)
		.bind(patch.favorite)
		.bind(gmb)
		.bind(member.0)
		.bind(target.0)
		.execute(&self.pool)
		.await
		.wrap_err("patching a track")?;
		if done.rows_affected() != 1 {
			return Err(no_track(gmb, target).into());
		}
		Ok(())
	}

	/// The member's gmails, each one's GMBs, and their places as tracked.
	pub async fn overview(&self, member: PersonId, prices: Option<&WalkPrices>, now: Timestamp) -> eyre::Result<Vec<GmailOverview>> {
		let days_ago = |d: i64| fmt_ts(now - SignedDuration::from_hours(24 * d));
		// one snapshot: every location row's GMB is among the GMBs read
		let mut tx = self.pool.begin().await.wrap_err("starting a read")?;
		let gmbs: Vec<(i64, i64, String, String, String, bool, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"SELECT m.id, m.gmail_id, m.gmail, m.name, m.place_id, m.enabled, m.state FROM gmb_mirror m WHERE m.id IN ({MEMBERS_GMBS}) ORDER BY m.gmail, m.gmail_id, m.id"
		)))
		.bind(member.0)
		.fetch_all(&mut *tx)
		.await
		.wrap_err("listing GMBs")?;
		let columns = &self.target_columns;
		let rows: Vec<LocationRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"SELECT k.gmb_id, {columns},
			        k.enabled AS track_enabled, k.favorite,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND r.published_est >= ?2) AS new_7d,
			        (SELECT COUNT(*) FROM posts p WHERE p.target_id = t.id AND p.published_est >= ?2) AS posts_7d,
			        p.text AS post_text, p.published_est AS post_published_est, p.first_seen AS post_first_seen,
			        (SELECT COUNT(*) FROM captures c JOIN reviews r ON r.id = c.review_id WHERE r.target_id = t.id AND c.captured_at >= ?2) AS snapshots_7d,
			        (SELECT COUNT(*) FROM captures c JOIN reviews r ON r.id = c.review_id WHERE r.target_id = t.id AND c.captured_at >= ?3) AS snapshots_30d,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND r.gone_at IS NULL) AS live,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND r.gone_at IS NULL AND r.reply IS NOT NULL) AS responded,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND r.gone_at IS NULL AND r.rating IS NOT 5) AS non_five_star,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND EXISTS (SELECT 1 FROM captures c WHERE c.review_id = r.id)) AS captured,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND EXISTS (SELECT 1 FROM captures c WHERE c.review_id = r.id)
			          AND NOT EXISTS (SELECT 1 FROM captures c WHERE c.review_id = r.id AND c.corrupted IS NOT 1)) AS corrupted,
			        t.listed, t.rating,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND r.gone_at IS NOT NULL) AS gone,
			        (SELECT COUNT(*) FROM reviews r WHERE r.target_id = t.id AND r.gone_at IS NOT NULL AND r.rating = 5
			          AND NOT EXISTS (SELECT 1 FROM reinstatements x WHERE x.review_id = r.id AND x.gmb_id = k.gmb_id
			            AND x.withdrawn_at IS NULL AND x.reinstated_at IS NULL)) AS suspended,
			        (SELECT COUNT(*) FROM reviews r JOIN reinstatements x ON x.review_id = r.id
			          WHERE r.target_id = t.id AND r.gone_at IS NOT NULL AND x.gmb_id = k.gmb_id
			            AND x.withdrawn_at IS NULL AND x.reinstated_at IS NULL) AS reinstating,
			        (SELECT finished_at FROM runs WHERE target_id = t.id AND finished_at IS NOT NULL ORDER BY id DESC LIMIT 1) AS last_run_at,
			        (SELECT status FROM runs WHERE target_id = t.id AND finished_at IS NOT NULL ORDER BY id DESC LIMIT 1) AS last_run_status,
			        (SELECT error FROM runs WHERE target_id = t.id AND finished_at IS NOT NULL ORDER BY id DESC LIMIT 1) AS last_run_error,
			        COALESCE((SELECT started_at FROM runs WHERE target_id = t.id AND finished_at IS NULL ORDER BY id DESC LIMIT 1),
			                 (SELECT started_at FROM jobs WHERE target_id = t.id AND status = 'running' ORDER BY id DESC LIMIT 1)) AS running_since,
			        (SELECT created_at FROM jobs WHERE target_id = t.id AND status = 'queued' AND kind = 'scan' ORDER BY id LIMIT 1) AS queued_since,
			        (SELECT COUNT(*) FROM jobs a WHERE a.status = 'queued'
			          AND a.id < (SELECT id FROM jobs WHERE target_id = t.id AND status = 'queued' AND kind = 'scan' ORDER BY id LIMIT 1)) AS queued_ahead,
			        (SELECT tokens FROM runs WHERE target_id = t.id AND ad_hoc = 0 AND finished_at IS NOT NULL ORDER BY id DESC LIMIT 1) AS scan_cost
			 FROM tracks k
			 JOIN targets t ON t.id = k.target_id
			 LEFT JOIN posts p ON p.id = (SELECT id FROM posts WHERE target_id = t.id ORDER BY first_seen DESC, id DESC LIMIT 1)
			 WHERE k.gmb_id IN ({MEMBERS_GMBS})"
		)))
		.bind(member.0)
		.bind(days_ago(7))
		.bind(days_ago(30))
		.fetch_all(&mut *tx)
		.await
		.wrap_err("loading the overview")?;
		tx.commit().await.wrap_err("ending a read")?;
		let mut tracks = HashMap::with_capacity(rows.len());
		for r in rows {
			let status = r
				.last_run_status
				.as_deref()
				.map(str::parse)
				.transpose()
				.wrap_err_with(|| format!("target {}'s last run has an unknown status", r.target.id))?;
			let held = match prices {
				Some(p) => self.held(TargetId(r.target.id), p).await?,
				None => false, // out of tokens is held at a price; with none, nothing walks
			};
			let track = LocationSummary {
				target: Target::try_from(r.target)?.into(),
				enabled: r.track_enabled,
				favorite: r.favorite,
				new_7d: r.new_7d,
				posts_7d: r.posts_7d,
				latest_post: match (r.post_text, r.post_first_seen) {
					(Some(text), Some(first_seen)) => Some(PostDto {
						text,
						published_est: r.post_published_est,
						first_seen,
					}),
					(None, None) => None,
					_ => unreachable!("both NOT NULL in a joined row"),
				},
				snapshots_7d: r.snapshots_7d,
				snapshots_30d: r.snapshots_30d,
				live: r.live,
				responded: r.responded,
				non_five_star: r.non_five_star,
				captured: r.captured,
				corrupted: r.corrupted,
				listed: r.listed,
				rating: r.rating,
				suspended: r.suspended,
				removed: r.gone - r.suspended - r.reinstating,
				reinstating: r.reinstating,
				last_run_at: r.last_run_at,
				last_run_status: status,
				last_run_error: r.last_run_error.as_deref().and_then(|e| e.lines().next()).map(str::to_owned),
				held,
				scan: match (r.running_since, r.queued_since) {
					(Some(since), _) => Some(ScanState::Running { since }),
					(None, Some(since)) => Some(ScanState::Queued { since, ahead: r.queued_ahead }),
					(None, None) => None,
				},
				scan_cost: prices.map(|p| r.scan_cost.unwrap_or(p.first_screen())), // a place never walked costs at least its first screen
			};
			assert!(tracks.insert(r.gmb_id, track).is_none(), "GMB {} tracks two places; a GMB tracks its one", r.gmb_id);
		}
		let mut out: Vec<GmailOverview> = Vec::new();
		for (id, gmail_id, gmail, name, place_id, enabled, state) in gmbs {
			if out.last().is_none_or(|g| g.gmail.id != gmail_id) {
				out.push(GmailOverview {
					gmail: GmailDto { id: gmail_id, gmail },
					gmbs: vec![],
				});
			}
			out.last_mut().expect("pushed above").gmbs.push(GmbSummary {
				id,
				name,
				place_id,
				enabled,
				state: state.parse().wrap_err_with(|| format!("GMB {id}'s state {state:?}"))?,
				track: tracks.remove(&id).expect("a snapshot tracks every GMB it does not remove, in its transaction"),
			});
		}
		Ok(out)
	}

	/// A tracked target's reviews in the board's four columns, as this GMB sees them.
	// ponytail: every live review in one answer; page `snapshotted` when a place's list outgrows a screen's worth of JSON
	pub async fn board(&self, member: PersonId, gmb: i64, target: TargetId) -> eyre::Result<Board> {
		self.check_track(member, gmb, target).await?;
		let select = REVIEW_SELECT.replacen("SELECT ", "SELECT x.requested_at, x.reinstated_at, ", 1);
		let rows: Vec<CardRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"{select}
			 LEFT JOIN reinstatements x ON x.id = (SELECT MAX(id) FROM reinstatements WHERE review_id = r.id AND gmb_id = ?1 AND withdrawn_at IS NULL)
			 WHERE r.target_id = ?2"
		)))
		.bind(gmb)
		.bind(target.0)
		.fetch_all(&self.pool)
		.await
		.wrap_err("loading a board")?;
		let mut board = Board::default();
		for r in rows {
			let card = BoardCard {
				reinstatement: r.requested_at.map(|requested_at| ReinstatementDto {
					requested_at,
					reinstated_at: r.reinstated_at.clone(),
				}),
				review: ReviewDto::from(r.review),
			};
			let open = card.reinstatement.as_ref().is_some_and(|x| x.reinstated_at.is_none());
			match (card.review.gone_at.is_some(), open) {
				(false, _) => board.snapshotted.push(card),
				(true, false) if card.review.rating == Some(5) => board.suspended.push(card),
				(true, false) => board.removed.push(card),
				(true, true) => board.reinstating.push(card),
			}
		}
		board.snapshotted.sort_by(|a, b| (&b.review.first_seen, b.review.id).cmp(&(&a.review.first_seen, a.review.id)));
		for gone in [&mut board.suspended, &mut board.removed] {
			gone.sort_by(|a, b| (&b.review.gone_at, b.review.id).cmp(&(&a.review.gone_at, a.review.id)));
		}
		let requested = |c: &BoardCard| c.reinstatement.as_ref().map(|x| x.requested_at.clone());
		board.reinstating.sort_by_key(|c| std::cmp::Reverse((requested(c), c.review.id)));
		Ok(board)
	}

	/// The reviews of a GMB's tracked place that are gone, newest gone first, each with the GMB's open appeal.
	pub(crate) async fn gone(&self, gmb: i64) -> eyre::Result<Vec<GoneReview>> {
		let target: i64 = sqlx::query_scalar("SELECT target_id FROM tracks WHERE gmb_id = ?")
			.bind(gmb)
			.fetch_optional(&self.pool)
			.await
			.wrap_err("looking up a GMB's track")?
			.ok_or_else(|| no_gmb(gmb))?;
		let select = REVIEW_SELECT.replacen("SELECT ", "SELECT x.requested_at, x.reinstated_at, ", 1);
		let rows: Vec<CardRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"{select}
			 LEFT JOIN reinstatements x ON x.review_id = r.id AND x.gmb_id = ?1 AND x.withdrawn_at IS NULL AND x.reinstated_at IS NULL
			 WHERE r.target_id = ?2 AND r.gone_at IS NOT NULL
			 ORDER BY r.gone_at DESC, r.id DESC
			 LIMIT ?3"
		)))
		.bind(gmb)
		.bind(target)
		.bind(GONE_MAX)
		.fetch_all(&self.pool)
		.await
		.wrap_err("loading a GMB's gone reviews")?;
		rows.into_iter()
			.map(|CardRow { review: r, requested_at, .. }| {
				Ok(GoneReview {
					rating: r.rating.map(u8::try_from).transpose().wrap_err_with(|| format!("review {}'s rating", r.id))?,
					published_est: r.published_est.as_deref().map(parse_ts).transpose()?,
					first_seen: parse_ts(&r.first_seen)?,
					gone_at: parse_ts(r.gone_at.as_deref().expect("the query's WHERE"))?,
					captured: r.capture_sha256.is_some(),
					replied: r.reply.is_some(),
					reinstatement_requested_at: requested_at.as_deref().map(parse_ts).transpose()?,
					review_id: r.source_review_id,
					author: r.author,
					author_url: r.author_url,
					text: r.text,
					published_raw: r.published_raw,
				})
			})
			.collect()
	}

	/// Opens an appeal of a gone review under a GMB tracking its place; one already open
	/// stays as it is.
	pub async fn reinstate(&self, member: PersonId, gmb: i64, review: ReviewId, now: Timestamp) -> eyre::Result<ReinstatementDto> {
		let (target, gone): (i64, bool) = sqlx::query_as("SELECT target_id, gone_at IS NOT NULL FROM reviews WHERE id = ?")
			.bind(review.0)
			.fetch_optional(&self.pool)
			.await
			.wrap_err("loading a review")?
			.ok_or_else(|| Rejected::not_found(format!("no review {review}")))?;
		self.check_track(member, gmb, TargetId(target)).await?;
		if !gone {
			return Err(Rejected::invalid(format!("review {review} is listed; only a removed review can be appealed")).into());
		}
		let mut tx = self.write().await?;
		sqlx::query(
			"INSERT INTO reinstatements (gmb_id, review_id, requested_at)
			 SELECT ?1, ?2, ?3 WHERE NOT EXISTS (SELECT 1 FROM reinstatements WHERE gmb_id = ?1 AND review_id = ?2 AND withdrawn_at IS NULL AND reinstated_at IS NULL)",
		)
		.bind(gmb)
		.bind(review.0)
		.bind(fmt_ts(now))
		.execute(&mut *tx)
		.await
		.wrap_err("opening an appeal")?;
		let (requested_at, reinstated_at): (String, Option<String>) =
			sqlx::query_as("SELECT requested_at, reinstated_at FROM reinstatements WHERE gmb_id = ? AND review_id = ? AND withdrawn_at IS NULL AND reinstated_at IS NULL")
				.bind(gmb)
				.bind(review.0)
				.fetch_one(&mut *tx)
				.await
				.wrap_err("reading the open appeal")?;
		tx.commit().await.wrap_err("committing an appeal")?;
		Ok(ReinstatementDto { requested_at, reinstated_at })
	}

	/// Withdraws the open appeal of a review: kept, marked withdrawn.
	pub async fn withdraw_reinstatement(&self, member: PersonId, gmb: i64, review: ReviewId, now: Timestamp) -> eyre::Result<()> {
		self.check_gmb(member, gmb).await?;
		let done = sqlx::query("UPDATE reinstatements SET withdrawn_at = ? WHERE gmb_id = ? AND review_id = ? AND withdrawn_at IS NULL AND reinstated_at IS NULL")
			.bind(fmt_ts(now))
			.bind(gmb)
			.bind(review.0)
			.execute(&self.pool)
			.await
			.wrap_err("withdrawing an appeal")?;
		if done.rows_affected() == 0 {
			return Err(Rejected::not_found(format!("no open appeal of review {review} under GMB {gmb}")).into());
		}
		Ok(())
	}

	/// What happened to a member's GMB before `before` (none: ever): its reviews' moves in the review feed, its appeals, its
	/// tracked place's failed scans; newest first, whole UTC days, `EVENTS_PAGE` rows or one day at least.
	pub async fn gmb_events(&self, member: PersonId, gmb: i64, before: Option<Timestamp>) -> eyre::Result<GmbEvents> {
		self.check_gmb(member, gmb).await?;
		const ROWS: &str = "SELECT * FROM (
			SELECT f.at, CASE WHEN f.gone_at IS NOT NULL THEN 'gone'
			                  WHEN EXISTS (SELECT 1 FROM review_feed p WHERE p.review_id = f.review_id AND p.gmb_id = f.gmb_id AND p.seq < f.seq) THEN 'returned'
			                  ELSE 'appeared' END AS kind,
			       f.review_id, f.rating, f.author, NULL AS run, NULL AS error
			FROM review_feed f WHERE f.gmb_id = ?1 AND f.at IS NOT NULL
			UNION ALL
			SELECT x.requested_at, 'reinstatement_set', r.source_review_id, r.rating, r.author, NULL, NULL
			FROM reinstatements x JOIN reviews r ON r.id = x.review_id WHERE x.gmb_id = ?1
			UNION ALL
			SELECT x.withdrawn_at, 'reinstatement_cleared', r.source_review_id, r.rating, r.author, NULL, NULL
			FROM reinstatements x JOIN reviews r ON r.id = x.review_id WHERE x.gmb_id = ?1 AND x.withdrawn_at IS NOT NULL
			UNION ALL
			SELECT u.finished_at, 'scan_failed', NULL, NULL, NULL, u.id, COALESCE(u.error, '')
			FROM runs u JOIN tracks k ON k.target_id = u.target_id WHERE k.gmb_id = ?1 AND u.status = 'failed'
		)";
		type Row = (String, String, Option<String>, Option<i64>, Option<String>, Option<i64>, Option<String>);
		let before_s = before.map(fmt_ts);
		let mut rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ROWS} WHERE (?2 IS NULL OR at < ?2) ORDER BY at DESC LIMIT ?3")))
			.bind(gmb)
			.bind(&before_s)
			.bind(EVENTS_PAGE + 1)
			.fetch_all(&self.pool)
			.await
			.wrap_err("loading a GMB's events")?;
		let day = |at: &str| at[..10].to_owned();
		let next = match rows.len() > EVENTS_PAGE as usize {
			false => None,
			true => {
				let last = day(&rows[EVENTS_PAGE as usize - 1].0);
				rows.retain(|r| day(&r.0) != last);
				let start = format!("{last}T00:00:00Z");
				if rows.is_empty() {
					rows = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ROWS} WHERE (?2 IS NULL OR at < ?2) AND at >= ?3 ORDER BY at DESC")))
						.bind(gmb)
						.bind(&before_s)
						.bind(&start)
						.fetch_all(&self.pool)
						.await
						.wrap_err("loading a GMB's day of events")?;
					Some(start)
				} else {
					Some(format!("{}T00:00:00Z", day(&rows.last().expect("kept a day").0)))
				}
			}
		};
		let events = rows
			.into_iter()
			.map(|(at, kind, review_id, rating, author, run, error)| {
				let review = || -> eyre::Result<ReviewRef> {
					Ok(ReviewRef {
						review_id: review_id.clone().ok_or_else(|| eyre::eyre!("{kind} at {at} names no review"))?,
						rating: rating.map(u8::try_from).transpose().wrap_err("a rating past u8")?,
						author: author.clone(),
					})
				};
				let what = match kind.as_str() {
					"appeared" => Happened::Appeared(review()?),
					"gone" if rating == Some(5) => Happened::Suspended(review()?),
					"gone" => Happened::Removed(review()?),
					"returned" => Happened::Returned(review()?),
					"reinstatement_set" => Happened::ReinstatementSet(review()?),
					"reinstatement_cleared" => Happened::ReinstatementCleared(review()?),
					"scan_failed" => Happened::ScanFailed {
						run: run.expect("selected with the run"),
						error: error.expect("selected with its error"),
					},
					other => unreachable!("selected kinds only, not {other}"),
				};
				Ok(GmbEvent { at, what })
			})
			.collect::<eyre::Result<_>>()?;
		Ok(GmbEvents { events, next })
	}

	/// Whether a capture shows a review of a place the member tracks.
	pub async fn member_sees_capture(&self, member: PersonId, sha256: &str) -> eyre::Result<bool> {
		sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
			"SELECT EXISTS (SELECT 1 FROM captures c JOIN reviews r ON r.id = c.review_id JOIN tracks k ON k.target_id = r.target_id
				                WHERE c.sha256 = ?1 AND k.gmb_id IN ({}))",
			MEMBERS_GMBS.replace("?1", "?2")
		)))
		.bind(sha256)
		.bind(member.0)
		.fetch_one(&self.pool)
		.await
		.wrap_err("checking a capture's owner")
	}

	/// The member's GMBs' tags for the conductor's page.
	pub async fn my_gmbs(&self, member: PersonId, now: Timestamp) -> eyre::Result<MyGmbs> {
		type Row = (i64, Option<i64>, bool, Option<f64>, Option<i64>, u32, u32, bool);
		let mut tx = self.pool.begin().await.wrap_err("starting a read")?;
		let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"SELECT m.id, w.id,
			        m.enabled AND m.state = 'verified' AND EXISTS (SELECT 1 FROM tracks k WHERE k.gmb_id = m.id AND k.enabled),
			        w.rating, w.listed,
			        (SELECT COUNT(*) FROM tracks k JOIN reviews r ON r.target_id = k.target_id
			          WHERE k.gmb_id = m.id AND k.enabled AND r.gone_at IS NOT NULL AND r.rating = 5
			            AND NOT EXISTS (SELECT 1 FROM reinstatements x WHERE x.review_id = r.id AND x.gmb_id = k.gmb_id
			              AND x.withdrawn_at IS NULL AND x.reinstated_at IS NULL)),
			        (SELECT COUNT(*) FROM tracks k JOIN reviews r ON r.target_id = k.target_id JOIN reinstatements x ON x.review_id = r.id
			          WHERE k.gmb_id = m.id AND k.enabled AND r.gone_at IS NOT NULL AND x.gmb_id = k.gmb_id
			            AND x.withdrawn_at IS NULL AND x.reinstated_at IS NULL),
			        EXISTS (SELECT 1 FROM tracks k WHERE k.gmb_id = m.id AND k.enabled
			          AND (SELECT status FROM runs WHERE target_id = k.target_id AND ad_hoc = 0 AND finished_at IS NOT NULL ORDER BY id DESC LIMIT 1) = 'failed')
			 FROM gmb_mirror m
			 LEFT JOIN targets w ON w.id = ({WORKING})
			 WHERE m.id IN ({MEMBERS_GMBS})"
		)))
		.bind(member.0)
		.fetch_all(&mut *tx)
		.await
		.wrap_err("tagging a member's GMBs")?;
		let today = now.to_zoned(jiff::tz::TimeZone::UTC).date();
		let first = today - SignedDuration::from_hours(24 * (LIVE_DAYS - 1) as i64);
		let tomorrow = today.tomorrow().wrap_err("a date past 9999")?;
		// a day's `live` is what its end had listed: first seen before it, not gone by it
		let days: Vec<(i64, String, u32, u32)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"WITH d AS (
			   SELECT r.target_id, date(r.finished_at) AS day, SUM(r.n_gone) AS lost FROM runs r
			   WHERE r.target_id IN (SELECT ({WORKING}) FROM gmb_mirror m WHERE m.id IN ({MEMBERS_GMBS}))
			     AND r.status IN ('ok', 'partial') AND r.ad_hoc = 0 AND r.finished_at >= ?2 AND r.finished_at < ?3
			   GROUP BY r.target_id, day)
			 SELECT d.target_id, d.day,
			        (SELECT COUNT(*) FROM reviews v WHERE v.target_id = d.target_id
			          AND v.first_seen < date(d.day, '+1 day') AND (v.gone_at IS NULL OR v.gone_at >= date(d.day, '+1 day'))),
			        d.lost
			 FROM d"
		)))
		.bind(member.0)
		.bind(first.to_string())
		.bind(tomorrow.to_string())
		.fetch_all(&mut *tx)
		.await
		.wrap_err("counting a member's places' live reviews by day")?;
		let mut live_30d: HashMap<i64, Vec<Option<DayReviews>>> = HashMap::new();
		for (target, day, live, lost) in days {
			let day: jiff::civil::Date = day.parse().wrap_err("SQLite's date()")?;
			let i = usize::try_from(first.until(day).wrap_err("days between dates")?.get_days()).expect("runs from `first` on");
			live_30d.entry(target).or_insert_with(|| vec![None; LIVE_DAYS])[i] = Some(DayReviews { live, lost });
		}
		let places_tracked: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
			"SELECT COUNT(DISTINCT k.target_id) FROM tracks k JOIN gmb_mirror m ON m.id = k.gmb_id
			 WHERE k.enabled AND m.enabled AND m.state = 'verified' AND k.gmb_id IN ({MEMBERS_GMBS})"
		)))
		.bind(member.0)
		.fetch_one(&mut *tx)
		.await
		.wrap_err("counting a member's tracked places")?;
		Ok(MyGmbs {
			gmbs: rows
				.into_iter()
				.map(|(id, working, tracked, rating, reviews, suspended, reinstating, failing)| {
					(
						id,
						GmbReviews {
							tracked,
							rating,
							reviews,
							suspended,
							reinstating,
							failing,
							live_30d: working.and_then(|w| live_30d.get(&w).cloned()).unwrap_or_else(|| vec![None; LIVE_DAYS]), // no run in the window: nothing collected
						},
					)
				})
				.collect(),
			places_tracked,
		})
	}

	/// How the reviews of the member's GMBs' places stick, of `gmbs` only when given.
	pub async fn stick(&self, member: PersonId, gmbs: Option<&[i64]>) -> eyre::Result<StickReport> {
		let mut tx = self.pool.begin().await.wrap_err("starting a read")?;
		type TrackRow = (i64, String, i64, Option<String>, Option<String>);
		let tracks: Vec<TrackRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"SELECT k.gmb_id, m.name, k.target_id,
			        (SELECT MIN(started_at) FROM runs WHERE target_id = k.target_id AND ad_hoc = 0 AND status IN ('ok', 'partial')),
			        (SELECT MAX(finished_at) FROM runs WHERE target_id = k.target_id AND ad_hoc = 0 AND status IN ('ok', 'partial'))
			 FROM tracks k JOIN gmb_mirror m ON m.id = k.gmb_id
			 WHERE k.gmb_id IN ({MEMBERS_GMBS}) ORDER BY k.gmb_id"
		)))
		.bind(member.0)
		.fetch_all(&mut *tx)
		.await
		.wrap_err("listing a member's tracks")?;
		let mut tracked = Vec::new();
		for (gmb, name, target, from, to) in tracks {
			if gmbs.is_some_and(|ids| !ids.contains(&gmb)) {
				continue;
			}
			// a place no scan has read yet has nothing watched
			let (Some(from), Some(to)) = (from, to) else { continue };
			tracked.push(Tracked {
				gmb,
				name,
				target: TargetId(target),
				from: parse_ts(&from)?,
				to: parse_ts(&to)?,
			});
		}
		let targets = serde_json::to_string(&tracked.iter().map(|t| t.target.0).collect::<Vec<_>>()).expect("ids serialize");
		type SeenRow = (i64, Option<i64>, Option<String>, Option<String>, String, Option<String>);
		let rows: Vec<SeenRow> =
			sqlx::query_as("SELECT target_id, rating, published_raw, published_est, first_seen, gone_at FROM reviews WHERE target_id IN (SELECT value FROM json_each(?))")
				.bind(targets)
				.fetch_all(&mut *tx)
				.await
				.wrap_err("loading the tracked places' reviews")?;
		let reviews = rows
			.into_iter()
			.map(|(target, stars, published_raw, published_est, first_seen, gone_at)| {
				Ok(Watched {
					target: TargetId(target),
					stars,
					published_raw,
					published_est: published_est.as_deref().map(parse_ts).transpose()?,
					first_seen: parse_ts(&first_seen)?,
					gone_at: gone_at.as_deref().map(parse_ts).transpose()?,
				})
			})
			.collect::<eyre::Result<Vec<_>>>()?;
		Ok(stick::report(&tracked, &reviews))
	}

	/// [`Rejected::NotFound`] unless the GMB is the member's and not removed.
	async fn check_gmb(&self, member: PersonId, gmb: i64) -> eyre::Result<()> {
		let found: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT EXISTS ({MEMBERS_GMB})")))
			.bind(gmb)
			.bind(member.0)
			.fetch_one(&self.pool)
			.await
			.wrap_err("looking up a GMB")?;
		if !found {
			return Err(no_gmb(gmb).into());
		}
		Ok(())
	}

	/// [`Rejected::NotFound`] unless the member's GMB tracks the target.
	pub(crate) async fn check_track(&self, member: PersonId, gmb: i64, target: TargetId) -> eyre::Result<()> {
		let found: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
			"SELECT EXISTS (SELECT 1 FROM tracks WHERE gmb_id = ({MEMBERS_GMB}) AND target_id = ?)"
		)))
		.bind(gmb)
		.bind(member.0)
		.bind(target.0)
		.fetch_one(&self.pool)
		.await
		.wrap_err("looking up a track")?;
		if !found {
			return Err(no_track(gmb, target).into());
		}
		Ok(())
	}
}
