//! Who pays for a walk and what it may spend. Balances are the conductor's: what a payer holds
//! here is its mirrored balance less the charges the feed holds past the conductor's cursor.

use eyre::WrapErr;
use jiff::{SignedDuration, Timestamp};
use review_archive_core::{TargetId, fmt_ts, maps::cost, tokens::Tokens};

use super::{Store, parse_ts};

/// Who pays for a scan of a target, and how much its walk may spend.
#[derive(Clone, Debug)]
pub struct Bill {
	/// As [`Store::bill`] picks them, by `sub`.
	pub payers: Vec<String>,
	/// What they hold together, within what the account has left this hour.
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

impl Store {
	/// What `payers` hold together: each one's mirrored balance less an equal share of every
	/// charge past `charges_through` naming them, none below 0; a `sub` the conductor told of
	/// no balance for holds 0.
	pub async fn holdings(&self, payers: &[String]) -> eyre::Result<i64> {
		sqlx::query_scalar(
			"WITH owed AS (
			     SELECT j.value AS sub, SUM((c.spent + json_array_length(c.payers) - 1) / json_array_length(c.payers)) AS owed
			     FROM token_charges c, json_each(c.payers) j
			     WHERE c.id > (SELECT charges_through FROM mirror_cursor)
			     GROUP BY j.value
			 )
			 SELECT COALESCE(SUM(MAX(COALESCE(b.balance, 0) - COALESCE(o.owed, 0), 0)), 0)
			 FROM json_each(?) p LEFT JOIN mirror_balances b ON b.sub = p.value LEFT JOIN owed o ON o.sub = p.value",
		)
		.bind(serde_json::to_string(payers)?)
		.fetch_one(&self.pool)
		.await
		.wrap_err("reckoning what payers hold")
	}

	/// Who pays for scanning `target` now, and what the walk may spend: for `job`, the member
	/// who asked for it, or no one for the operator's; for an `ad_hoc` look, no one; else its trackers.
	pub async fn bill(&self, target: TargetId, job: Option<i64>, ad_hoc: bool, now: Timestamp, cfg: &Tokens) -> eyre::Result<Bill> {
		let rail = self.rail(now, cfg).await?;
		let payers = match job {
			Some(id) => {
				let (by, sub): (Option<i64>, Option<String>) = sqlx::query_as("SELECT j.requested_by, p.sub FROM jobs j LEFT JOIN people p ON p.id = j.requested_by WHERE j.id = ?")
					.bind(id)
					.fetch_one(&self.pool)
					.await
					.wrap_err_with(|| format!("loading who asked for job {id}"))?;
				by.map(|by| sub.unwrap_or_else(|| panic!("person {by} asked for job {id} signed in, so has a sub")))
					.into_iter()
					.collect()
			}
			None if ad_hoc => Vec::new(),
			None => self.trackers(target).await?,
		};
		if payers.is_empty() {
			return Ok(Bill { payers, allowance: rail.left });
		}
		let held = self.holdings(&payers).await?;
		Ok(Bill {
			payers,
			allowance: held.min(rail.left),
		})
	}

	/// Whether the members tracking `target` hold too few tokens together for a walk to read anything.
	pub async fn held(&self, target: TargetId) -> eyre::Result<bool> {
		let payers = self.trackers(target).await?;
		if payers.is_empty() {
			return Ok(false);
		}
		Ok(self.holdings(&payers).await? < cost::FIRST_SCREEN)
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

	/// The owners of the GMBs with a track of `target` on, by `sub`.
	async fn trackers(&self, target: TargetId) -> eyre::Result<Vec<String>> {
		sqlx::query_scalar("SELECT DISTINCT m.owner FROM tracks k JOIN gmb_mirror m ON m.id = k.gmb_id WHERE k.target_id = ? AND k.enabled ORDER BY m.owner")
			.bind(target.0)
			.fetch_all(&self.pool)
			.await
			.wrap_err("listing who tracks a target")
	}
}
