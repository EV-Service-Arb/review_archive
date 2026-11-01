//! One tracked place's page: its gmail and the place over the board of its reviews.

mod dock;

use dioxus::prelude::*;
use review_archive_client::dto::LocationSummary;

use crate::{Gmb, Route, Ui, View};

#[component]
pub fn Board(gmb: Gmb, location: LocationSummary, tab: Option<i64>, children: Element) -> Element {
	let (g, target) = (gmb.id, location.target.id);
	let body = match crate::ui() {
		Ui::Grafana => rsx! { crate::grafana::Location { gmb: g, target, tab } },
		Ui::Legacy => rsx! { dock::Lanes { gmb: g, target } },
	};
	rsx! {
		header { class: "flex h-14 shrink-0 items-center gap-3 border-b border-border px-6",
			Link { class: "text-ink-soft hover:text-ink", to: Route::at(tab, View::Gmail(gmb.gmail.id)), "{gmb.gmail.gmail}" }
			span { class: "text-ink-soft", "/" }
			a {
				class: "font-medium hover:underline",
				href: "https://www.google.com/maps/place/?q=place_id:{location.target.place_id}",
				target: "_blank",
				rel: "noopener noreferrer",
				title: "Open on Google Maps",
				"{location.target.label} ↗"
			}
			div { class: "flex-1" }
			{children}
		}
		crate::filter::reviews::ReviewBar {}
		{body}
	}
}
