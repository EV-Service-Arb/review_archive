//! What the dashboard shows at which path: the server's `x-views`, the page's routes.

use ev_lib::views::Views;

/// What the dashboard shows.
#[derive(Clone, Debug, PartialEq, Views)]
pub enum Board {
	/// Every GMB of the member's, by gmail.
	#[view("/")]
	Home,
	/// A gmail's GMBs.
	#[view("/gmails/{gmail}")]
	Gmail {
		/// The conductor's id of the gmail.
		gmail: i64,
	},
	/// A GMB's board: its place's reviews in lanes.
	#[view("/gmbs/{gmb}")]
	Gmb {
		/// The conductor's id of the GMB.
		gmb: i64,
	},
	/// The member's settings.
	#[view("/settings")]
	Settings,
}

/// Where the dashboard is.
#[derive(Clone, Debug, PartialEq, Views)]
pub enum View {
	/// The caller's own.
	#[view("", nest)]
	Mine {
		/// What it shows.
		view: Board,
	},
}
