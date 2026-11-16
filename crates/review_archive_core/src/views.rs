//! What the dashboard shows at which path: the server's `x-views`, the page's routes.

use ev_lib::views::Views;
use sa_auth::Members;

/// What one member's tab shows.
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

/// Where the dashboard is: a view of the caller's own, of the member their tab acts as, or the
/// pick of a member to open a tab for.
#[derive(Clone, Debug, PartialEq, Views)]
pub enum View {
	/// The pick of a member to open a tab for.
	#[view("/act-as", needs = Members::ActAs)]
	Pick,
	/// A member's tab, for one who acts as them.
	#[view("/members/{member}", nest, needs = Members::ActAs)]
	Theirs {
		/// Their person id (`GET /members`).
		member: i64,
		/// What their tab shows.
		view: Board,
	},
	/// The caller's own tab.
	#[view("", nest)]
	Mine {
		/// What it shows.
		view: Board,
	},
}
