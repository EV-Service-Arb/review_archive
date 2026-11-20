//! Risk tokens: what a walk costs the signed-in account. Balances and prices are the conductor's;
//! this is the account's own limit and the walk's meter.

use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

pub use sa_core::prices::WalkStep;
use sa_core::prices::{Item, Prices};
use serde::{Deserialize, Serialize};
use smart_default::SmartDefault;
use strum::VariantArray;

/// The account's own limit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, SmartDefault)]
#[cfg_attr(feature = "settings", derive(schemars::JsonSchema, v_utils::macros::SettingsNested))]
#[serde(default, deny_unknown_fields)]
pub struct Tokens {
	/// Spent by all walks together in any hour at most, whoever pays: Maps closes until older walks leave the window.
	#[default(120)]
	pub per_hour: i64,
}

impl Tokens {
	/// What each step of a walk costs at `prices`, or why no walk starts.
	pub fn walk_prices(&self, prices: &Prices) -> Result<WalkPrices, NoWalks> {
		let steps = WalkStep::VARIANTS
			.iter()
			.map(|&s| prices.get(&s.into()).map(|&p| (s, p)).ok_or(NoWalks::Unpriced(s.into())))
			.collect::<Result<_, _>>()?;
		let walk = WalkPrices(steps);
		if walk.first_screen() > self.per_hour {
			return Err(NoWalks::PastTheHour {
				first_screen: walk.first_screen(),
				per_hour: self.per_hour,
			});
		}
		Ok(walk)
	}
}

/// Every [`WalkStep`]'s price, within the hour's limit.
#[derive(Clone, Debug, PartialEq)]
pub struct WalkPrices(BTreeMap<WalkStep, i64>);

impl WalkPrices {
	/// What `step` costs.
	pub fn of(&self, step: WalkStep) -> i64 {
		self.0[&step] // every step, by `Tokens::walk_prices`
	}

	/// A walk is not started on less: with it, it reads at least the list's first screen.
	pub fn first_screen(&self) -> i64 {
		[WalkStep::Open, WalkStep::Reviews, WalkStep::Sort].map(|s| self.of(s)).iter().sum()
	}
}

/// Why no walk starts.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NoWalks {
	/// The admin has not priced a step.
	#[error("walks are not priced: \"{}\" has no price", .0.title())]
	Unpriced(Item),
	/// A walk's first screen alone is more than the hour allows.
	#[error("a walk's first screen costs {first_screen} tokens, more than tokens.per_hour ({per_hour})")]
	PastTheHour {
		/// [`WalkPrices::first_screen`].
		first_screen: i64,
		/// [`Tokens::per_hour`].
		per_hour: i64,
	},
}

/// What a walk may spend, and has.
#[derive(Debug)]
pub struct Meter {
	tariff: Tariff,
	allowance: i64,
	spent: i64,
	steps: Steps,
}
impl Meter {
	/// A walk that may spend `allowance` at `prices`.
	pub fn new(prices: WalkPrices, allowance: i64) -> Self {
		Self {
			tariff: Tariff::Priced(prices),
			allowance,
			spent: 0,
			steps: Steps::default(),
		}
	}

	/// For a run while no walk starts ([`NoWalks`]): spending from it panics.
	pub fn shut() -> Self {
		Self {
			tariff: Tariff::Shut,
			allowance: 0,
			spent: 0,
			steps: Steps::default(),
		}
	}

	/// For a walk nothing of is stored: it counts nothing.
	pub fn uncounted() -> Self {
		Self {
			tariff: Tariff::Uncounted,
			allowance: i64::MAX,
			spent: 0,
			steps: Steps::default(),
		}
	}

	fn cost(&self, step: WalkStep) -> i64 {
		match &self.tariff {
			Tariff::Priced(p) => p.of(step),
			Tariff::Shut => panic!("a walk started while none may: a store's walks are closed (`NoWalks`) before any reaches its meter"),
			Tariff::Uncounted => 0,
		}
	}

	/// Spends `step`'s price, whatever is left: the steps a walk cannot do without.
	pub fn spend(&mut self, step: WalkStep) {
		self.spent += self.cost(step);
		self.steps.add(step);
	}

	/// Spends `step`'s price if it fits; `false`: the walk is at its budget.
	pub fn try_spend(&mut self, step: WalkStep) -> bool {
		let cost = self.cost(step);
		let fits = self.spent + cost <= self.allowance;
		if fits {
			self.spent += cost;
			self.steps.add(step);
		}
		fits
	}

	/// Whether what is left reads at least the list's first screen.
	pub fn affords_first_screen(&self) -> bool {
		match &self.tariff {
			Tariff::Priced(p) => self.allowance - self.spent >= p.first_screen(),
			Tariff::Shut => false,
			Tariff::Uncounted => true,
		}
	}

	/// Spent so far.
	pub fn spent(&self) -> i64 {
		self.spent
	}

	/// How many times each step was done, priced or free, readable while the walk holds the meter.
	pub fn steps(&self) -> Steps {
		self.steps.clone()
	}
}

/// A walk's steps done; every clone reads the same walk.
#[derive(Clone, Debug, Default)]
pub struct Steps(Arc<Mutex<BTreeMap<WalkStep, i64>>>);
impl Steps {
	/// So far.
	pub fn now(&self) -> BTreeMap<WalkStep, i64> {
		self.0.lock().expect("held only to count or copy").clone()
	}

	fn add(&self, step: WalkStep) {
		*self.0.lock().expect("held only to count or copy").entry(step).or_default() += 1;
	}
}

#[derive(Debug)]
enum Tariff {
	Priced(WalkPrices),
	/// No walk starts; one that did would be a bug.
	Shut,
	/// Nothing stored, so no one to charge and no hour to keep.
	Uncounted,
}
