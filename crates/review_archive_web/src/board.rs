//! One tracked place's page: its gmail and the place over the board of its reviews.

mod dock;

use dioxus::prelude::*;
use review_archive_client::dto::LocationSummary;
use review_archive_core::views::Board as View;

use crate::{Gmb, Route, Ui};

#[component]
pub fn Board(gmb: Gmb, location: LocationSummary) -> Element {
	let (g, target) = (gmb.id, location.target.id);
	let body = match crate::ui() {
		Ui::Grafana => rsx! { crate::grafana::Location { gmb: g, target } },
		Ui::Legacy => rsx! { dock::Lanes { gmb: g, target } },
	};
	rsx! {
		header { class: "flex h-14 shrink-0 items-center gap-3 border-b border-border px-6",
			Link { class: "text-ink-soft hover:text-ink", to: Route::at(View::Gmail { gmail: gmb.gmail.id }), "{gmb.gmail.gmail}" }
			span { class: "text-ink-soft", "/" }
			a {
				class: "font-medium hover:underline",
				href: "https://www.google.com/maps/place/?q=place_id:{location.target.place_id}",
				target: "_blank",
				rel: "noopener noreferrer",
				title: "Open on Google Maps",
				"{gmb.name} ↗"
			}
		}
		crate::filter::reviews::ReviewBar {}
		{body}
	}
}
