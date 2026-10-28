use std::collections::HashMap;

use dioxus::prelude::*;
use ev_lib::uikit_dioxus::{Button, ButtonVariant, Size};
#[cfg(not(feature = "grafana"))]
use review_archive_client::dto::BoardCard;
use strum::IntoEnumIterator;

use super::Tri;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, strum::Display, strum::EnumIter)]
pub(crate) enum Review {
	#[strum(to_string = "5-star")]
	FiveStar,
	#[strum(to_string = "responded")]
	Responded,
	#[strum(to_string = "new 7d")]
	New7d,
}

#[cfg(not(feature = "grafana"))] // Grafana narrows the board's queries by them: `examples/grafana_location.py`
impl Review {
	fn holds(self, c: &BoardCard) -> bool {
		match self {
			Self::FiveStar => c.review.rating == Some(5),
			Self::Responded => c.review.reply.is_some(),
			Self::New7d => c
				.review
				.published_est
				.as_deref()
				.is_some_and(|t| t.parse::<jiff::Timestamp>().expect("the archive writes timestamps as RFC 3339").as_second() >= js_sys::Date::now() as i64 / 1000 - 7 * 86_400),
		}
	}
}

/// true, the review must have it; false, must not; absent, either.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reviews(pub(crate) HashMap<Review, bool>);

#[cfg(not(feature = "grafana"))]
impl Reviews {
	pub(crate) fn not(r: Review) -> Self {
		Self(HashMap::from([(r, false)]))
	}

	pub(crate) fn keep(&self, cards: Vec<BoardCard>) -> Vec<BoardCard> {
		cards.into_iter().filter(|c| self.0.iter().all(|(r, want)| r.holds(c) == *want)).collect()
	}
}

#[component]
pub(crate) fn ReviewBar() -> Element {
	let mut reviews: Signal<Reviews> = use_context();
	rsx! {
		div { class: "flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border px-6 py-2",
			for r in Review::iter() {
				Tri {
					key: "{r:?}",
					label: r.to_string(),
					state: reviews.read().0.get(&r).copied(),
					on_change: move |s| {
						let mut w = reviews.write();
						match s {
							Some(want) => w.0.insert(r, want),
							None => w.0.remove(&r),
						};
					},
				}
			}
			div { class: "flex-1" }
			Button {
				variant: ButtonVariant::Ghost,
				size: Size::Sm,
				r#type: "button",
				disabled: *reviews.read() == Reviews::default(),
				onclick: move |_| reviews.set(Reviews::default()),
				"Clear"
			}
		}
	}
}
