//! The conductor's GMBs and balances, as its last snapshot told them, and the feed of what
//! walks cost, which it charges from.

use std::collections::HashMap;

use eyre::WrapErr;
use jiff::Timestamp;
use review_archive_core::{
	Rejected, TargetId,
	dto::{ChargeDto, Charges, GmbSnapshot},
	fmt_ts,
};
use sqlx::SqliteConnection;

use super::{RunEnd, Store, members::left_untracked};

#[derive(sqlx::FromRow)]
struct ChargeRow {
	id: i64,
	run_id: i64,
	payers: String,
	spent: i64,
	kick_back: bool,
	label: String,
	at: String,
}

impl Store {
	/// Takes the snapshot in one transaction, or nothing of it: [`Rejected::Conflict`] when it
	/// contradicts the mirror. A GMB newly removed loses its tracks, as an untrack does.
	pub async fn apply_snapshot(&self, snap: &GmbSnapshot, now: Timestamp) -> eyre::Result<()> {
		if let Some((sub, b)) = snap.balances.iter().find(|(_, b)| **b < 0) {
			return Err(Rejected::invalid(format!("{sub}'s balance is {b}; balances are 0 or more")).into());
		}
		let mut tx = self.write().await?;
		let held: Vec<(i64, i64, String, String, bool)> = sqlx::query_as("SELECT id, gmail_id, owner, place_id, removed FROM gmb_mirror")
			.fetch_all(&mut *tx)
			.await
			.wrap_err("loading the mirror")?;
		let given: HashMap<i64, _> = snap.gmbs.iter().map(|g| (g.id, g)).collect();
		if given.len() != snap.gmbs.len() {
			return Err(Rejected::invalid("a GMB id is in the snapshot twice").into());
		}
		let mut removed_now = Vec::new();
		for (id, gmail_id, owner, place_id, removed) in &held {
			let g = given
				.get(id)
				.ok_or_else(|| Rejected::Conflict(format!("GMB {id} is mirrored here and absent from the snapshot")))?;
			if (g.gmail_id, &g.owner, &g.place_id) != (*gmail_id, owner, place_id) {
				return Err(Rejected::Conflict(format!(
					"GMB {id} is gmail {gmail_id}'s ({owner}) on {place_id} here; the snapshot says gmail {}'s ({}) on {}",
					g.gmail_id, g.owner, g.place_id
				))
				.into());
			}
			if g.removed && !removed {
				removed_now.push(*id);
			}
		}
		let (through, last): (i64, Option<i64>) = sqlx::query_as("SELECT (SELECT charges_through FROM mirror_cursor), (SELECT MAX(id) FROM token_charges)")
			.fetch_one(&mut *tx)
			.await
			.wrap_err("loading the charge cursor")?;
		if snap.charges_through < through || snap.charges_through > last.unwrap_or(0) {
			// an empty feed holds nothing past 0
			return Err(Rejected::Conflict(format!(
				"charges_through {} is behind the {through} stored, or past the feed's last charge ({last:?})",
				snap.charges_through
			))
			.into());
		}
		for g in &snap.gmbs {
			sqlx::query(
				"INSERT INTO gmb_mirror (id, gmail_id, gmail, owner, removed, enabled, place_id) VALUES (?, ?, ?, ?, ?, ?, ?)
				 ON CONFLICT (id) DO UPDATE SET gmail = excluded.gmail, removed = excluded.removed, enabled = excluded.enabled",
			)
			.bind(g.id)
			.bind(g.gmail_id)
			.bind(&g.gmail)
			.bind(&g.owner)
			.bind(g.removed)
			.bind(g.enabled)
			.bind(&g.place_id)
			.execute(&mut *tx)
			.await
			.wrap_err("mirroring a GMB")?;
		}
		for gmb in removed_now {
			let targets: Vec<i64> = sqlx::query_scalar("DELETE FROM tracks WHERE gmb_id = ? RETURNING target_id")
				.bind(gmb)
				.fetch_all(&mut *tx)
				.await
				.wrap_err("untracking a removed GMB")?;
			for t in targets {
				left_untracked(&mut tx, TargetId(t), now).await?;
			}
		}
		sqlx::query("DELETE FROM mirror_balances").execute(&mut *tx).await.wrap_err("dropping the old balances")?;
		for (sub, balance) in &snap.balances {
			sqlx::query("INSERT INTO mirror_balances (sub, balance) VALUES (?, ?)")
				.bind(sub)
				.bind(balance)
				.execute(&mut *tx)
				.await
				.wrap_err("mirroring a balance")?;
		}
		sqlx::query("UPDATE mirror_cursor SET charges_through = ?")
			.bind(snap.charges_through)
			.execute(&mut *tx)
			.await
			.wrap_err("moving the charge cursor")?;
		tx.commit().await.wrap_err("committing a snapshot")
	}

	/// The place a GMB is on.
	pub(crate) async fn gmb_place(&self, gmb: i64) -> eyre::Result<String> {
		sqlx::query_scalar("SELECT place_id FROM gmb_mirror WHERE id = ?")
			.bind(gmb)
			.fetch_optional(&self.pool)
			.await
			.wrap_err("loading a GMB's place")?
			.ok_or_else(|| Rejected::not_found(format!("no GMB {gmb}")).into())
	}

	/// The conductor's switch of a GMB.
	pub(crate) async fn gmb_enabled(&self, gmb: i64) -> eyre::Result<bool> {
		sqlx::query_scalar("SELECT enabled FROM gmb_mirror WHERE id = ?")
			.bind(gmb)
			.fetch_optional(&self.pool)
			.await
			.wrap_err("loading a GMB's switch")?
			.ok_or_else(|| Rejected::not_found(format!("no GMB {gmb}")).into())
	}

	/// The feed past `after`, `limit` at most.
	pub async fn charges(&self, after: i64, limit: u32) -> eyre::Result<Charges> {
		let rows: Vec<ChargeRow> = sqlx::query_as("SELECT id, run_id, payers, spent, kick_back, label, at FROM token_charges WHERE id > ? ORDER BY id LIMIT ?")
			.bind(after)
			.bind(limit)
			.fetch_all(&self.pool)
			.await
			.wrap_err("reading the charge feed")?;
		let charges = rows
			.into_iter()
			.map(|r| {
				Ok(ChargeDto {
					payers: serde_json::from_str(&r.payers).wrap_err_with(|| format!("charge {}'s payers", r.id))?,
					id: r.id,
					key: format!("run:{}", r.run_id),
					spent: r.spent,
					kick_back: r.kick_back,
					label: r.label,
					at: r.at,
				})
			})
			.collect::<eyre::Result<Vec<_>>>()?;
		Ok(Charges {
			next: charges.last().map_or(after, |c| c.id),
			charges,
		})
	}
}

/// What the run's walk cost its payers, into the feed with the run's end.
pub(super) async fn charge(tx: &mut SqliteConnection, end: &RunEnd<'_>, now: Timestamp) -> eyre::Result<()> {
	if end.payers.is_empty() || end.tokens == 0 {
		return Ok(());
	}
	assert!(end.tokens > 0, "a walk spends, never earns");
	sqlx::query(
		"INSERT INTO token_charges (run_id, payers, spent, kick_back, label, at)
		 SELECT r.id, ?, ?, ?, COALESCE(t.label, t.name, t.place_id), ? FROM runs r JOIN targets t ON t.id = r.target_id WHERE r.id = ?",
	)
	.bind(serde_json::to_string(end.payers)?)
	.bind(end.tokens)
	.bind(end.kick_back)
	.bind(fmt_ts(now))
	.bind(end.run.0)
	.execute(&mut *tx)
	.await
	.wrap_err("charging a run")?;
	Ok(())
}
