//! The Grafana dashboards the server builds over its document and the page frames: what a link to
//! one names, and what one posts up to the page.

/// The member's places as cards, and as a table.
pub const OVERVIEW: &str = "ra-overview";
/// One tracked place: its lanes of reviews and its runs.
pub const LOCATION: &str = "ra-location";

/// The variables a link to a dashboard sets (`var-<name>`).
pub mod var {
	/// The person id the dashboard reads as (`x-member`).
	pub const MEMBER: &str = "member";
	/// Bumped after a write: the panels read again.
	pub const REV: &str = "rev";
	/// [`super::OVERVIEW`]: the gmail shown, empty for every one.
	pub const GMAIL: &str = "gmail";
	/// [`super::OVERVIEW`]: the `gmb:target` pairs shown, in their order.
	pub const KEYS: &str = "keys";
	/// [`super::OVERVIEW`]: the GMBs picked, the host's selection.
	pub const GMBS: &str = "gmbs";
	/// [`super::LOCATION`]: the conductor's id of the GMB.
	pub const GMB: &str = "gmb";
	/// [`super::LOCATION`]: the target its track is on.
	pub const TARGET: &str = "target";
	/// [`super::LOCATION`]: the API's base, where the cards' captures are.
	pub const API: &str = "api";
}

/// What a click or a drop in a dashboard posts up to the page (its `kind`).
pub mod up {
	/// A card clicked: its place's board (`gmb`, `target`).
	pub const OPEN: &str = "ra-open";
	/// Scan the card's place now.
	pub const SCAN: &str = "ra-scan";
	/// Its track switched (`on`).
	pub const ENABLE: &str = "ra-enable";
	/// Its track favorited (`on`).
	pub const FAVORITE: &str = "ra-favorite";
	/// Its place asked of Google again.
	pub const RELINK: &str = "ra-relink";
	/// A gone review dropped on Reinstating (`review`).
	pub const REINSTATE: &str = "ra-reinstate";
	/// A reinstating review dropped back where it sits gone (`review`).
	pub const WITHDRAW: &str = "ra-withdraw";
}

/// The location board's review filter, a variable per variant by its `Debug` name: `true`, a card
/// must hold it; `false`, must not; empty, either.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, strum::Display, strum::EnumIter)]
pub enum Review {
	/// Five stars.
	#[strum(to_string = "5-star")]
	FiveStar,
	/// The owner replied.
	#[strum(to_string = "responded")]
	Responded,
	/// Posted over the last 7 days.
	#[strum(to_string = "new 7d")]
	New7d,
}
