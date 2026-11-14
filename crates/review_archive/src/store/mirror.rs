//! The conductor's GMBs, balances and walk prices, as its last snapshot told them, the feed of what
//! walks cost, which it charges from, and the feed of what they did, whoever paid.

use std::collections::HashMap;

use eyre::WrapErr;
use jiff::Timestamp;
use review_archive_core::{
	Rejected, TargetId, TargetKind,
	dto::{Charge, ChargePage, Country, OnGoogle, Snapshot, UsagePage, Used},
	fmt_ts,
};
use sa_core::prices::Item;
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
	pub async fn apply_snapshot(&self, snap: &Snapshot, now: Timestamp) -> eyre::Result<()> {
		if let Some((sub, b)) = snap.balances.iter().find(|(_, b)| **b < 0) {
			return Err(Rejected::invalid(format!("{sub}'s balance is {b}; balances are 0 or more")).into());
		}
		let walk_prices: Vec<_> = snap.prices.iter().filter(|(i, _)| matches!(i, Item::Walk(_))).collect();
		if let Some((item, p)) = walk_prices.iter().find(|(_, p)| **p < 0) {
			return Err(Rejected::invalid(format!("{item}'s price is {p}; prices are 0 or more")).into());
		}
		if let Some(g) = snap.gmbs.iter().find(|g| !g.removed && g.country.is_none()) {
			return Err(Rejected::invalid(format!("GMB {} is not removed and has no country", g.id)).into());
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
			.bind(g.owner.to_string())
			.bind(g.removed)
			.bind(g.enabled)
			.bind(g.state.as_str())
			.bind(g.country.map(Country::lang))
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
				.bind(sub.to_string())
				.bind(balance)
				.execute(&mut *tx)
				.await
				.wrap_err("mirroring a balance")?;
		}
		sqlx::query("DELETE FROM mirror_prices").execute(&mut *tx).await.wrap_err("dropping the old prices")?;
		for (item, price) in walk_prices {
			sqlx::query("INSERT INTO mirror_prices (item, price) VALUES (?, ?)")
				.bind(item.to_string())
				.bind(price)
				.execute(&mut *tx)
				.await
				.wrap_err("mirroring a price")?;
		}
		sqlx::query("UPDATE mirror_cursor SET charges_through = ?")
			.bind(snap.charges_through)
			.execute(&mut *tx)
			.await
			.wrap_err("moving the charge cursor")?;
		tx.commit().await.wrap_err("committing a snapshot")
	}

	/// The conductor's switch of a GMB, and its state.
	pub(crate) async fn gmb_switch(&self, gmb: i64) -> eyre::Result<(bool, OnGoogle)> {
		let (enabled, state): (bool, String) = sqlx::query_as("SELECT enabled, state FROM gmb_mirror WHERE id = ?")
			.bind(gmb)
			.fetch_optional(&self.pool)
			.await
			.wrap_err("loading a GMB's switch")?
			.ok_or_else(|| Rejected::not_found(format!("no GMB {gmb}")))?;
		Ok((enabled, state.parse().wrap_err_with(|| format!("GMB {gmb}'s state {state:?}"))?))
	}

	/// The feed past `after`, `limit` at most.
	pub async fn charges(&self, after: i64, limit: u32) -> eyre::Result<ChargePage> {
		let rows: Vec<ChargeRow> = sqlx::query_as("SELECT id, run_id, payers, spent, kick_back, label, at FROM token_charges WHERE id > ? ORDER BY id LIMIT ?")
			.bind(after)
			.bind(limit)
			.fetch_all(&self.pool)
			.await
			.wrap_err("reading the charge feed")?;
		let charges = rows
			.into_iter()
			.map(|r| {
				Ok(Charge {
					payers: serde_json::from_str(&r.payers).wrap_err_with(|| format!("charge {}'s payers {:?}", r.id, r.payers))?,
					id: r.id,
					key: format!("run:{}", r.run_id),
					spent: r.spent,
					kick_back: r.kick_back,
					label: r.label,
					at: r.at.parse().wrap_err_with(|| format!("charge {}'s time {:?}", r.id, r.at))?,
				})
			})
			.collect::<eyre::Result<Vec<_>>>()?;
		Ok(ChargePage {
			next: charges.last().map_or(after, |c| c.id),
			charges,
		})
	}

	/// The usage feed past `after`, `limit` at most.
	pub async fn usage(&self, after: i64, limit: u32) -> eyre::Result<UsagePage> {
		let rows: Vec<(i64, String, i64, String)> = sqlx::query_as("SELECT id, item, units, at FROM walk_usage WHERE id > ? ORDER BY id LIMIT ?")
			.bind(after)
			.bind(limit)
			.fetch_all(&self.pool)
			.await
			.wrap_err("reading the usage feed")?;
		let used = rows
			.into_iter()
			.map(|(id, item, units, at)| {
				Ok(Used {
					item: item.parse().wrap_err_with(|| format!("usage {id}'s item"))?,
					at: at.parse().wrap_err_with(|| format!("usage {id}'s time {at:?}"))?,
					id,
					units,
				})
			})
			.collect::<eyre::Result<Vec<_>>>()?;
		Ok(UsagePage {
			next: used.last().map_or(after, |u| u.id),
			used,
		})
	}
}

/// What the run's walk cost its payers, into the feed with the run's end.
pub(super) async fn charge(tx: &mut SqliteConnection, end: &RunEnd<'_>, now: Timestamp) -> eyre::Result<()> {
	let spent = end.meter.spent();
	if end.payers.is_empty() || spent == 0 {
		return Ok(());
	}
	assert!(spent > 0, "a walk spends, never earns");
	sqlx::query(
		"INSERT INTO token_charges (run_id, payers, spent, kick_back, label, at)
		 SELECT r.id, ?, ?, ?, COALESCE(t.label, t.name, t.place_id), ? FROM runs r JOIN targets t ON t.id = r.target_id WHERE r.id = ?",
	)
	.bind(serde_json::to_string(end.payers)?)
	.bind(spent)
	.bind(end.kick_back)
	.bind(fmt_ts(now))
	.bind(end.run.0)
	.execute(&mut *tx)
	.await
	.wrap_err("charging a run")?;
	Ok(())
}

/// What the run's walk did, a row per step, into the usage feed with the run's end.
pub(super) async fn used(tx: &mut SqliteConnection, end: &RunEnd<'_>, now: Timestamp) -> eyre::Result<()> {
	for (&step, &units) in end.meter.steps() {
		sqlx::query("INSERT INTO walk_usage (run_id, item, units, at) VALUES (?, ?, ?, ?)")
			.bind(end.run.0)
			.bind(Item::Walk(step).to_string())
			.bind(units)
			.bind(fmt_ts(now))
			.execute(&mut *tx)
			.await
			.wrap_err("recording a run's usage")?;
	}
	Ok(())
}
