//! What is a member's own: what their GMBs (the conductor's, mirrored) track, and their
//! appeals. Every query here is scoped by the member; the archive under it
//! (targets, reviews, captures) stays shared.

use eyre::WrapErr;
use jiff::{SignedDuration, Timestamp};
use review_archive_core::{
	PersonId, Rejected, ReviewId, Target, TargetId,
	dto::{Board, BoardCard, GmailDto, GmailOverview, GmbReviews, GmbSummary, LocationSummary, MyGmbs, PostDto, ReinstatementDto, ReviewDto, ScanState, TrackPatch},
	fmt_ts,
	maps::cost,
};
use sqlx::{FromRow, SqliteConnection};

use super::{REVIEW_SELECT, ReviewRow, Store, TargetRow, no_target};

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
	/// Tracks a target under a GMB; tracking it twice is tracking it. `member: None` is
	/// the operator, who may assign any GMB.
	pub async fn track(&self, member: Option<PersonId>, gmb: i64, target: TargetId, now: Timestamp) -> eyre::Result<()> {
		self.check_gmb(member, gmb).await?;
		let mut tx = self.write().await?;
		// back on the schedule it was taken off when its last track went
		let live = sqlx::query("UPDATE targets SET enabled = enabled OR untracked_at IS NOT NULL, untracked_at = NULL WHERE id = ? AND id > 0")
			.bind(target.0)
			.execute(&mut *tx)
			.await
			.wrap_err("tracking a target")?;
		if live.rows_affected() != 1 {
			return Err(no_target(target).into());
		}
		sqlx::query("INSERT INTO tracks (gmb_id, target_id, created_at) VALUES (?, ?, ?) ON CONFLICT DO NOTHING")
			.bind(gmb)
			.bind(target.0)
			.bind(fmt_ts(now))
			.execute(&mut *tx)
			.await
			.wrap_err("tracking a target")?;
		tx.commit().await.wrap_err("committing a track")
	}

	/// Stops tracking; the target and its archive stay, until `defaults.untracked_retention`
	/// after its last track went.
	pub async fn untrack(&self, member: PersonId, gmb: i64, target: TargetId, now: Timestamp) -> eyre::Result<()> {
		let mut tx = self.write().await?;
		let done = sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM tracks WHERE gmb_id = ({MEMBERS_GMB}) AND target_id = ?")))
			.bind(gmb)
			.bind(member.0)
			.bind(target.0)
			.execute(&mut *tx)
			.await
			.wrap_err("untracking a target")?;
		if done.rows_affected() != 1 {
			return Err(no_track(gmb, target).into());
		}
		left_untracked(&mut tx, target, now).await?;
		tx.commit().await.wrap_err("committing an untrack")
	}

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

	/// The member's gmails, each one's GMBs, and their places as tracked, by screenshots over 7 days, most first.
	pub async fn overview(&self, member: PersonId, now: Timestamp) -> eyre::Result<Vec<GmailOverview>> {
		let days_ago = |d: i64| fmt_ts(now - SignedDuration::from_hours(24 * d));
		// one snapshot: every location row's GMB is among the GMBs read
		let mut tx = self.pool.begin().await.wrap_err("starting a read")?;
		let gmbs: Vec<(i64, i64, String, String, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"SELECT m.id, m.gmail_id, m.gmail, m.place_id, m.enabled FROM gmb_mirror m WHERE m.id IN ({MEMBERS_GMBS}) ORDER BY m.gmail, m.gmail_id, m.id"
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
			 WHERE k.gmb_id IN ({MEMBERS_GMBS})
			 ORDER BY snapshots_7d DESC, t.id"
		)))
		.bind(member.0)
		.bind(days_ago(7))
		.bind(days_ago(30))
		.fetch_all(&mut *tx)
		.await
		.wrap_err("loading the overview")?;
		tx.commit().await.wrap_err("ending a read")?;
		let mut out: Vec<GmailOverview> = Vec::new();
		for (id, gmail_id, gmail, place_id, enabled) in gmbs {
			if out.last().is_none_or(|g| g.gmail.id != gmail_id) {
				out.push(GmailOverview {
					gmail: GmailDto { id: gmail_id, gmail },
					gmbs: vec![],
				});
			}
			let g = out.last_mut().expect("pushed above");
			g.gmbs.push(GmbSummary {
				id,
				place_id,
				enabled,
				tracks: vec![],
			});
		}
		for r in rows {
			let status = r
				.last_run_status
				.as_deref()
				.map(str::parse)
				.transpose()
				.wrap_err_with(|| format!("target {}'s last run has an unknown status", r.target.id))?;
			let held = self.held(TargetId(r.target.id)).await?;
			let gmb = out.iter_mut().flat_map(|g| &mut g.gmbs).find(|b| b.id == r.gmb_id).expect("read in the same transaction");
			gmb.tracks.push(LocationSummary {
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
				scan_cost: r.scan_cost.unwrap_or(cost::FIRST_SCREEN), // a place never walked costs at least its first screen
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
		self.check_gmb(Some(member), gmb).await?;
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
	pub async fn my_gmbs(&self, member: PersonId) -> eyre::Result<MyGmbs> {
		let rows: Vec<(i64, bool, Option<f64>, Option<i64>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
			"SELECT m.id,
			        m.enabled AND EXISTS (SELECT 1 FROM tracks k WHERE k.gmb_id = m.id AND k.enabled),
			        w.rating, w.listed
			 FROM gmb_mirror m
			 LEFT JOIN targets w ON w.id = (
			     SELECT k.target_id FROM tracks k JOIN runs r ON r.target_id = k.target_id
			     WHERE k.gmb_id = m.id AND r.status IN ('ok', 'partial') AND r.ad_hoc = 0
			     ORDER BY r.finished_at DESC, r.id DESC LIMIT 1)
			 WHERE m.id IN ({MEMBERS_GMBS})"
		)))
		.bind(member.0)
		.fetch_all(&self.pool)
		.await
		.wrap_err("tagging a member's GMBs")?;
		let places_tracked: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
			"SELECT COUNT(DISTINCT k.target_id) FROM tracks k JOIN gmb_mirror m ON m.id = k.gmb_id
			 WHERE k.enabled AND m.enabled AND k.gmb_id IN ({MEMBERS_GMBS})"
		)))
		.bind(member.0)
		.fetch_one(&self.pool)
		.await
		.wrap_err("counting a member's tracked places")?;
		Ok(MyGmbs {
			gmbs: rows.into_iter().map(|(id, tracked, rating, reviews)| (id, GmbReviews { tracked, rating, reviews })).collect(),
			places_tracked,
		})
	}

	/// [`Rejected::NotFound`] unless the GMB is the member's and not removed (any GMB not removed for the operator).
	pub(crate) async fn check_gmb(&self, member: Option<PersonId>, gmb: i64) -> eyre::Result<()> {
		let found: bool = sqlx::query_scalar(
			"SELECT EXISTS (SELECT 1 FROM gmb_mirror m WHERE m.id = ?1 AND NOT m.removed
			                AND (?2 IS NULL OR m.owner = (SELECT sub FROM people WHERE id = ?2)))",
		)
		.bind(gmb)
		.bind(member.map(|m| m.0))
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

/// The ids of the GMBs of person `?1`, not removed.
const MEMBERS_GMBS: &str = "SELECT m.id FROM gmb_mirror m JOIN people p ON p.sub = m.owner WHERE p.id = ?1 AND NOT m.removed";

/// GMB `?` if it is person `?`'s and not removed, as a scalar subquery.
const MEMBERS_GMB: &str = "SELECT m.id FROM gmb_mirror m JOIN people p ON p.sub = m.owner WHERE m.id = ? AND p.id = ? AND NOT m.removed";
