//! The conductor's snapshot with nothing in it but walk prices: what walks were metered at before the admin priced them.

use review_archive::core::{dto::Snapshot, tokens::WalkStep};

pub fn snapshot() -> Snapshot {
	Snapshot {
		gmbs: vec![],
		balances: Default::default(),
		charges_through: 0,
		prices: [(WalkStep::Open, 7), (WalkStep::Reviews, 1), (WalkStep::Sort, 7), (WalkStep::SortRetry, 2), (WalkStep::Step, 1)]
			.into_iter()
			.map(|(s, p)| (s.into(), p))
			.collect(),
	}
}
