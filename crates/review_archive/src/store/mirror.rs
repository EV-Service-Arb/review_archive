//! The conductor's GMBs and balances, as its last snapshot told them, and the feed of what
//! walks cost, which it charges from.

use std::collections::HashMap;

use eyre::WrapErr;
use jiff::Timestamp;
use review_archive_core::{
	Rejected, TargetId, TargetKind, check_lang,
	dto::{ChargeDto, Charges, GbpState, GmbSnapshot},
	fmt_ts,
};
use sqlx::SqliteConnection;

use super::{InsertTarget, RunEnd, Store, insert_target, members::left_untracked, target_on};

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
	/// contradicts the mirror. A removed GMB loses its track, as an untrack does; one not removed
	/// and without a track gets one, on its place in its language.
	pub async fn apply_snapshot(&self, snap: &GmbSnapshot, now: Timestamp) -> eyre::Result<()> {
		if let Some((sub, b)) = snap.balances.iter().find(|(_, b)| **b < 0) {
			return Err(Rejected::invalid(format!("{sub}'s balance is {b}; balances are 0 or more")).into());
		}
		for g in &snap.gmbs {
			match &g.lang {
				Some(lang) => check_lang(lang)?,
				None if g.removed => {}
				None => return Err(Rejected::invalid(format!("GMB {} is not removed and has no lang", g.id)).into()),
			}
		}
		let mut tx = self.write().await?;
		let held: Vec<(i64, String)> = sqlx::query_as("SELECT id, place_id FROM gmb_mirror").fetch_all(&mut *tx).await.wrap_err("loading the mirror")?;
		let given: HashMap<i64, _> = snap.gmbs.iter().map(|g| (g.id, g)).collect();
		if given.len() != snap.gmbs.len() {
			return Err(Rejected::invalid("a GMB id is in the snapshot twice").into());
		}
		for (id, place_id) in &held {
			let g = given
				.get(id)
				.ok_or_else(|| Rejected::Conflict(format!("GMB {id} is mirrored here and absent from the snapshot")))?;
			if g.place_id != *place_id {
				return Err(Rejected::Conflict(format!("GMB {id} is on {place_id} here; the snapshot says {}", g.place_id)).into());
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
				"INSERT INTO gmb_mirror (id, gmail_id, gmail, owner, removed, enabled, state, lang, name, address, place_id) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
				 ON CONFLICT (id) DO UPDATE SET gmail_id = excluded.gmail_id, gmail = excluded.gmail, owner = excluded.owner, removed = excluded.removed,
				   enabled = excluded.enabled, state = excluded.state, lang = excluded.lang, name = excluded.name, address = excluded.address",
			)
			.bind(g.id)
			.bind(g.gmail_id)
			.bind(&g.gmail)
			.bind(&g.owner)
			.bind(g.removed)
			.bind(g.enabled)
			.bind(g.state.as_ref())
			.bind(&g.lang)
			.bind(&g.name)
			.bind(&g.address)
			.bind(&g.place_id)
			.execute(&mut *tx)
			.await
			.wrap_err("mirroring a GMB")?;
		}
		let untracked: Vec<i64> = sqlx::query_scalar("DELETE FROM tracks WHERE gmb_id IN (SELECT id FROM gmb_mirror WHERE removed) RETURNING target_id")
			.fetch_all(&mut *tx)
			.await
			.wrap_err("untracking removed GMBs")?;
		for t in untracked {
			left_untracked(&mut tx, TargetId(t), now).await?;
		}
		// any track will do: relinking may have moved its target off the GMB's place_id
		let trackless: Vec<(i64, String, String, String, Option<String>)> =
			sqlx::query_as("SELECT m.id, m.place_id, m.lang, m.name, m.address FROM gmb_mirror m WHERE NOT m.removed AND NOT EXISTS (SELECT 1 FROM tracks k WHERE k.gmb_id = m.id)")
				.fetch_all(&mut *tx)
				.await
				.wrap_err("listing GMBs without a track")?;
		for (gmb, place_id, lang, name, address) in trackless {
			let target = match target_on(&mut *tx, &self.target_columns, &place_id, &lang, None).await? {
				Some(t) => {
					sqlx::query("UPDATE targets SET enabled = 1, untracked_at = NULL WHERE id = ?")
						.bind(t.id.0)
						.execute(&mut *tx)
						.await
						.wrap_err("putting a target back on the schedule")?;
					t.id
				}
				None => {
					let t = InsertTarget {
						label: None,
						name: Some(name),
						address,
						kind: TargetKind::Maps,
						place_id,
						gbp: None,
						lang,
						interval: self.default_interval,
						enabled: true,
					};
					insert_target(&mut *tx, &t, now).await?
				}
			};
			sqlx::query("INSERT INTO tracks (gmb_id, target_id, created_at) VALUES (?, ?, ?)")
				.bind(gmb)
				.bind(target.0)
				.bind(fmt_ts(now))
				.execute(&mut *tx)
				.await
				.wrap_err("tracking a GMB's place")?;
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

	/// The conductor's switch of a GMB, and its state.
	pub(crate) async fn gmb_switch(&self, gmb: i64) -> eyre::Result<(bool, GbpState)> {
		let (enabled, state): (bool, String) = sqlx::query_as("SELECT enabled, state FROM gmb_mirror WHERE id = ?")
			.bind(gmb)
			.fetch_optional(&self.pool)
			.await
			.wrap_err("loading a GMB's switch")?
			.ok_or_else(|| Rejected::not_found(format!("no GMB {gmb}")))?;
		Ok((enabled, state.parse().wrap_err_with(|| format!("GMB {gmb}'s state {state:?}"))?))
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
