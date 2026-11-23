mod dashboard;
mod dev_member;
mod generated;
mod operations;
mod places;
mod worker;

/// What the conductor tells of `owner`'s one verified GMB `id` at `place_id`.
fn snapshot(owner: &str, id: i64, place_id: &str) -> review_archive::core::dto::Snapshot {
	use review_archive::core::dto::{Country, Managed, OnGoogle, Snapshot};
	Snapshot {
		gmbs: vec![Managed {
			id,
			gmail_id: 1,
			gmail: "ops@gmail.com".into(),
			owner: owner.parse().unwrap(),
			removed: false,
			enabled: true,
			place_id: place_id.into(),
			state: OnGoogle::Verified,
			country: Some(Country::US),
			name: format!("GBP {id}"),
			address: None,
		}],
		balances: Default::default(),
		charges_through: 0,
		prices: Default::default(),
	}
}
