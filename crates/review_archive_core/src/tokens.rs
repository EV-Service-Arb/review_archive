//! Risk tokens: what a walk costs the signed-in account. Balances are the conductor's; this is
//! the account's own limit and the walk's meter.

use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;

/// The account's own limit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, SmartDefault)]
#[cfg_attr(feature = "settings", derive(schemars::JsonSchema, v_utils::macros::SettingsNested))]
#[serde(default, deny_unknown_fields)]
pub struct Tokens {
	/// Spent by all walks together in any hour at most, whoever pays: Maps closes until older walks leave the window.
	#[default(120)]
	pub per_hour: i64,
}

/// What a walk may spend, and has.
#[derive(Debug)]
pub struct Meter {
	allowance: i64,
	spent: i64,
}

impl Meter {
	/// A walk that may spend `allowance`.
	pub fn new(allowance: i64) -> Self {
		Self { allowance, spent: 0 }
	}

	/// Spends `cost`, whatever is left: the steps a walk cannot do without.
	pub fn spend(&mut self, cost: i64) {
		self.spent += cost;
	}

	/// Spends `cost` if it fits; `false`: the walk is at its budget.
	pub fn try_spend(&mut self, cost: i64) -> bool {
		let fits = self.spent + cost <= self.allowance;
		if fits {
			self.spent += cost;
		}
		fits
	}

	/// What it may still spend.
	pub fn left(&self) -> i64 {
		self.allowance - self.spent
	}

	/// Spent so far.
	pub fn spent(&self) -> i64 {
		self.spent
	}
}
