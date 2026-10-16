//! The token ledger: members' balances are sums of its rows, which are only ever added.
//! Renewal is written when a balance is read, a day's worth at a time.

use eyre::WrapErr;
use jiff::{SignedDuration, Timestamp};
use review_archive_core::{
	PersonId, Rejected, TargetId,
	dto::{BalanceChange, KickbackDto, LedgerEntry, TokenKind, USAGE_DAYS, Usage, UsageDay},
	fmt_ts,
	maps::cost,
	tokens::{Tokens, accrue, split},
};
use sqlx::{FromRow, SqliteConnection};

use super::{RunId, Store, parse_ts};

/// Who pays for a scan of a target, and how much its walk may spend.
#[derive(Clone, Debug)]
pub struct Bill {
	/// As [`Store::bill`] picks them.
	pub payers: Vec<PersonId>,
	/// Their balances together, within what the account has left this hour.
	pub allowance: i64,
}

/// What the account may still spend this hour.
#[derive(Clone, Copy, Debug)]
pub struct Rail {
	/// Tokens left in the window.
	pub left: i64,
	/// When a walk can start again, if one cannot now.
	pub reopens_at: Option<Timestamp>,
}

#[derive(FromRow)]
struct LedgerRow {
	at: String,
	delta: i64,
	kind: String,
	by_email: Option<String>,
	note: Option<String>,
	run_id: Option<i64>,
	target_id: Option<i64>,
	label: Option<String>,
}

impl Store {
	/// The member's balance at `now`, renewed up to it.
	pub async fn balance(&self, member: PersonId, now: Timestamp, cfg: &Tokens) -> eyre::Result<i64> {
		let mut tx = self.write().await?;
		let b = renewed(&mut tx, member, now, cfg).await?;
		tx.commit().await.wrap_err("committing a renewal")?;
		Ok(b)
	}

	/// Changes the member's balance as an admin (`by`) asks; the new balance comes back.
	pub async fn change_balance(&self, member: PersonId, change: BalanceChange, by: &str, note: Option<&str>, now: Timestamp, cfg: &Tokens) -> eyre::Result<i64> {
		let mut tx = self.write().await?;
		let balance = renewed(&mut tx, member, now, cfg).await?;
		let (kind, delta) = match change {
			BalanceChange::Set(n) if n >= 0 => (TokenKind::Set, n - balance),
			BalanceChange::Grant(n) if n > 0 => (TokenKind::Grant, n),
			BalanceChange::Purchase(n) if n > 0 => (TokenKind::Purchase, n),
			_ => return Err(Rejected::invalid("a balance is set to 0 or more, and grants and purchases add 1 or more").into()),
		};
		sqlx::query("INSERT INTO token_ledger (person_id, at, delta, kind, by_email, note) VALUES (?, ?, ?, ?, ?, ?)")
			.bind(member.0)
			.bind(fmt_ts(now))
			.bind(delta)
			.bind(kind.as_ref())
			.bind(by)
			.bind(note)
			.execute(&mut *tx)
			.await
			.wrap_err("recording a balance change")?;
		tx.commit().await.wrap_err("committing a balance change")?;
		Ok(balance + delta)
	}

	/// The member's kick-backs not claimed yet, newest first.
	pub async fn kickbacks(&self, member: PersonId) -> eyre::Result<Vec<KickbackDto>> {
		let rows: Vec<(i64, String, i64, Option<String>, String)> = sqlx::query_as(
			"SELECT k.run_id, k.at, k.amount, r.error, COALESCE(t.label, t.name, t.place_id)
			 FROM kickbacks k JOIN runs r ON r.id = k.run_id JOIN targets t ON t.id = r.target_id
			 WHERE k.person_id = ? AND k.claimed_at IS NULL ORDER BY k.id DESC",
		)
		.bind(member.0)
		.fetch_all(&self.pool)
		.await
		.wrap_err("listing pending kick-backs")?;
		Ok(rows
			.into_iter()
			.map(|(run_id, at, amount, why, target_label)| KickbackDto {
				run_id,
				target_label,
				at,
				amount,
				why: why.expect("a run that kicks back failed, or met a failure, and says which"),
			})
			.collect())
	}

	/// Adds the member's pending kick-backs to their balance; the new balance comes back.
	pub async fn claim(&self, member: PersonId, now: Timestamp, cfg: &Tokens) -> eyre::Result<i64> {
		let mut tx = self.write().await?;
		let balance = renewed(&mut tx, member, now, cfg).await?;
		let at = fmt_ts(now);
		let added: i64 = sqlx::query_scalar::<_, i64>(
			"INSERT INTO token_ledger (person_id, at, delta, kind, run_id)
			 SELECT person_id, ?, amount, 'kickback', run_id FROM kickbacks WHERE person_id = ? AND claimed_at IS NULL
			 RETURNING delta",
		)
		.bind(&at)
		.bind(member.0)
		.fetch_all(&mut *tx)
		.await
		.wrap_err("claiming kick-backs")?
		.into_iter()
		.sum();
		sqlx::query("UPDATE kickbacks SET claimed_at = ? WHERE person_id = ? AND claimed_at IS NULL")
			.bind(&at)
			.bind(member.0)
			.execute(&mut *tx)
			.await
			.wrap_err("marking kick-backs claimed")?;
		tx.commit().await.wrap_err("committing a claim")?;
		Ok(balance + added)
	}

	/// The member's ledger, newest first; days renewal added nothing to are left out.
	pub async fn ledger(&self, member: PersonId, limit: u32) -> eyre::Result<Vec<LedgerEntry>> {
		let rows: Vec<LedgerRow> = sqlx::query_as(
			"SELECT l.at, l.delta, l.kind, l.by_email, l.note, l.run_id, r.target_id, COALESCE(t.label, t.name, t.place_id) AS label
			 FROM token_ledger l LEFT JOIN runs r ON r.id = l.run_id LEFT JOIN targets t ON t.id = r.target_id
			 WHERE l.person_id = ? AND l.delta != 0 ORDER BY l.id DESC LIMIT ?",
		)
		.bind(member.0)
		.bind(limit)
		.fetch_all(&self.pool)
		.await
		.wrap_err("listing a ledger")?;
		rows.into_iter()
			.map(|r| {
				Ok(LedgerEntry {
					kind: r.kind.parse().wrap_err_with(|| format!("ledger kind {:?}", r.kind))?,
					at: r.at,
					delta: r.delta,
					by: r.by_email,
					note: r.note,
					run_id: r.run_id,
					target_id: r.target_id,
					target_label: r.label,
				})
			})
			.collect()
	}

	/// The member's charges over the [`USAGE_DAYS`] UTC days up to `now`, and what they track now.
	pub async fn usage(&self, member: PersonId, now: Timestamp) -> eyre::Result<Usage> {
		let today = now.to_zoned(jiff::tz::TimeZone::UTC).date();
		let first = today - jiff::Span::new().days(USAGE_DAYS - 1);
		let charged: Vec<(String, i64, i64)> = sqlx::query_as(
			"SELECT substr(at, 1, 10) AS day, COUNT(*), -SUM(delta) FROM token_ledger
			 WHERE person_id = ? AND kind = 'charge' AND delta < 0 AND at >= ? GROUP BY day",
		)
		.bind(member.0)
		.bind(first.to_string())
		.fetch_all(&self.pool)
		.await
		.wrap_err("summing a member's charges by day")?;
		let places_tracked: i64 = sqlx::query_scalar(
			"SELECT COUNT(DISTINCT k.target_id) FROM tracks k JOIN managing_gmails g ON g.id = k.managing_gmail_id
			 WHERE g.person_id = ? AND k.enabled AND g.enabled",
		)
		.bind(member.0)
		.fetch_one(&self.pool)
		.await
		.wrap_err("counting a member's tracked places")?;
		let days = first
			.series(jiff::Span::new().days(1))
			.take(USAGE_DAYS as usize)
			.map(|d| {
				let day = d.to_string();
				let (walks, tokens) = charged.iter().find(|(c, ..)| *c == day).map_or((0, 0), |&(_, w, t)| (w, t)); // a day with no charge row charged nothing
				UsageDay { day, walks, tokens }
			})
			.collect();
		Ok(Usage { days, places_tracked })
	}

	/// Who pays for scanning `target` now, and what the walk may spend: for `job`, the member
	/// who asked for it, or no one for the operator's; for an `ad_hoc` look, no one; else its trackers.
	pub async fn bill(&self, target: TargetId, job: Option<i64>, ad_hoc: bool, now: Timestamp, cfg: &Tokens) -> eyre::Result<Bill> {
		let rail = self.rail(now, cfg).await?;
		let payers = match job {
			Some(id) => {
				let by: Option<i64> = sqlx::query_scalar("SELECT requested_by FROM jobs WHERE id = ?")
					.bind(id)
					.fetch_one(&self.pool)
					.await
					.wrap_err_with(|| format!("loading who asked for job {id}"))?;
				by.map(PersonId).into_iter().collect()
			}
			None if ad_hoc => Vec::new(),
			None => self.trackers(target).await?,
		};
		if payers.is_empty() {
			return Ok(Bill { payers, allowance: rail.left });
		}
		let held = self.held_together(&payers, now, cfg).await?;
		Ok(Bill {
			payers,
			allowance: held.min(rail.left),
		})
	}

	/// Whether the members tracking `target` hold too few tokens together for a walk to read anything.
	pub async fn held(&self, target: TargetId, now: Timestamp, cfg: &Tokens) -> eyre::Result<bool> {
		let payers = self.trackers(target).await?;
		if payers.is_empty() {
			return Ok(false);
		}
		Ok(self.held_together(&payers, now, cfg).await? < cost::FIRST_SCREEN)
	}

	pub(crate) async fn held_together(&self, members: &[PersonId], now: Timestamp, cfg: &Tokens) -> eyre::Result<i64> {
		let mut tx = self.write().await?;
		let mut held = 0;
		for m in members {
			held += renewed(&mut tx, *m, now, cfg).await?;
		}
		tx.commit().await.wrap_err("committing renewals")?;
		Ok(held)
	}

	/// What the walks of the last hour left of `per_hour`.
	pub async fn rail(&self, now: Timestamp, cfg: &Tokens) -> eyre::Result<Rail> {
		let window: Vec<(String, i64)> = sqlx::query_as("SELECT finished_at, tokens FROM runs WHERE finished_at > ? AND tokens > 0 ORDER BY finished_at")
			.bind(fmt_ts(now - SignedDuration::from_hours(1)))
			.fetch_all(&self.pool)
			.await
			.wrap_err("summing the last hour's tokens")?;
		let mut spent: i64 = window.iter().map(|(_, t)| t).sum();
		let left = (cfg.per_hour - spent).max(0);
		let mut reopens_at = None;
		if left < cost::FIRST_SCREEN {
			for (at, t) in &window {
				spent -= t;
				if cfg.per_hour - spent >= cost::FIRST_SCREEN {
					reopens_at = Some(parse_ts(at)? + SignedDuration::from_hours(1));
					break;
				}
			}
			assert!(reopens_at.is_some(), "per_hour ({}) is below a walk's first screen ({})", cfg.per_hour, cost::FIRST_SCREEN);
		}
		Ok(Rail { left, reopens_at })
	}

	/// Members with a track of `target` on, under a gmail that is on.
	async fn trackers(&self, target: TargetId) -> eyre::Result<Vec<PersonId>> {
		let ids: Vec<i64> = sqlx::query_scalar(
			"SELECT DISTINCT g.person_id FROM tracks k JOIN managing_gmails g ON g.id = k.managing_gmail_id
			 WHERE k.target_id = ? AND k.enabled AND g.enabled ORDER BY g.person_id",
		)
		.bind(target.0)
		.fetch_all(&self.pool)
		.await
		.wrap_err("listing who tracks a target")?;
		Ok(ids.into_iter().map(PersonId).collect())
	}
}

/// The member's balance, renewed up to `now` inside the caller's write. A member seen for
/// the first time starts with a day's worth.
async fn renewed(tx: &mut SqliteConnection, member: PersonId, now: Timestamp, cfg: &Tokens) -> eyre::Result<i64> {
	let (balance, last): (i64, Option<String>) = sqlx::query_as("SELECT COALESCE(SUM(delta), 0), MAX(CASE WHEN kind = 'accrual' THEN at END) FROM token_ledger WHERE person_id = ?")
		.bind(member.0)
		.fetch_one(&mut *tx)
		.await
		.wrap_err("summing a balance")?;
	let (delta, at) = match last {
		None => (cfg.daily.min(cfg.cap), now),
		Some(last) => {
			let last = parse_ts(&last)?;
			match accrue(balance, last, now, cfg.daily, cfg.cap) {
				(_, at) if at == last => return Ok(balance),
				renewal => renewal,
			}
		}
	};
	// a day with nothing added is still written: renewal counts from it, not from the last top-up
	sqlx::query("INSERT INTO token_ledger (person_id, at, delta, kind) VALUES (?, ?, ?, 'accrual')")
		.bind(member.0)
		.bind(fmt_ts(at))
		.bind(delta)
		.execute(&mut *tx)
		.await
		.wrap_err("recording a renewal")?;
	Ok(balance + delta)
}

/// Charges `spent` to `payers` for `run`, within their balances; with `kick_back`, each is
/// owed back what they paid.
pub(super) async fn charge(tx: &mut SqliteConnection, run: RunId, spent: i64, payers: &[PersonId], kick_back: bool, now: Timestamp) -> eyre::Result<()> {
	let mut balances = Vec::with_capacity(payers.len());
	for m in payers {
		let b: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(delta), 0) FROM token_ledger WHERE person_id = ?")
			.bind(m.0)
			.fetch_one(&mut *tx)
			.await
			.wrap_err("summing a balance")?;
		assert!(b >= 0, "charges stay within a balance and a set is never below 0");
		balances.push((*m, b));
	}
	for (m, paid) in split(spent, &balances) {
		sqlx::query("INSERT INTO token_ledger (person_id, at, delta, kind, run_id) VALUES (?, ?, ?, 'charge', ?)")
			.bind(m.0)
			.bind(fmt_ts(now))
			.bind(-paid)
			.bind(run.0)
			.execute(&mut *tx)
			.await
			.wrap_err("charging a run")?;
		if kick_back {
			sqlx::query("INSERT INTO kickbacks (person_id, run_id, amount, at) VALUES (?, ?, ?, ?)")
				.bind(m.0)
				.bind(run.0)
				.bind(paid)
				.bind(fmt_ts(now))
				.execute(&mut *tx)
				.await
				.wrap_err("kicking a run back")?;
		}
	}
	Ok(())
}
