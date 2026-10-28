//! One place's page: its gmail and the place over the board of its reviews.

mod dock;

use dioxus::prelude::*;
use review_archive_client::dto::{GmailDto, LocationSummary};

use crate::{Route, Ui, View};

#[component]
pub fn Board(gmail: GmailDto, location: LocationSummary, tab: Option<i64>) -> Element {
	let (g, target) = (gmail.id, location.target.id);
	let body = match crate::ui() {
		Ui::Grafana => rsx! { crate::grafana::Location { gmail: g, target, tab } },
		Ui::Legacy => rsx! { dock::Lanes { gmail: g, target } },
	};
	rsx! {
		header { class: "flex h-14 shrink-0 items-center gap-3 border-b border-border px-6",
			Link { class: "text-ink-soft hover:text-ink", to: Route::at(tab, View::Gmail(g)), "{gmail.gmail}" }
			span { class: "text-ink-soft", "/" }
			a {
				class: "font-medium hover:underline",
				href: "https://www.google.com/maps/place/?q=place_id:{location.target.place_id}",
				target: "_blank",
				rel: "noopener noreferrer",
				title: "Open on Google Maps",
				"{location.target.label} ↗"
			}
		}
		crate::filter::reviews::ReviewBar {}
		{body}
	}
}
