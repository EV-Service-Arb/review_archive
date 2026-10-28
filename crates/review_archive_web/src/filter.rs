//! What narrows the locations grid and a place's board: a regex over a place's text, and facets
//! each required or ruled out. One per tab, kept across its views. The grid's sits over the totals of what it keeps.

pub(crate) mod reviews;

use std::collections::HashMap;

use dioxus::prelude::*;
use ev_lib::uikit_dioxus::{Button, ButtonVariant, Input, Size, Toggle, ToggleVariant};
use regex_lite::{Regex, RegexBuilder};
use review_archive_client::dto::{GmailDto, LocationSummary, RunStatus};
use strum::IntoEnumIterator;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, strum::Display, strum::EnumIter)]
enum Facet {
	#[strum(to_string = "favorite")]
	Favorite,
	#[strum(to_string = "posted 7d")]
	Posted7d,
	#[strum(to_string = "out of tokens")]
	OutOfTokens,
	#[strum(to_string = "failing")]
	Failing,
	#[strum(to_string = "scanning")]
	Scanning,
	#[strum(to_string = "off")]
	Off,
}

impl Facet {
	fn holds(self, g: &GmailDto, l: &LocationSummary) -> bool {
		match self {
			Self::Favorite => l.favorite,
			Self::Posted7d => l.posts_7d > 0,
			Self::OutOfTokens => l.held,
			Self::Failing => matches!(l.last_run_status, Some(RunStatus::Failed | RunStatus::Partial)),
			Self::Scanning => l.scan.is_some(),
			Self::Off => !l.enabled || !g.enabled,
		}
	}
}

/// `facets`: true, the place must have it; false, must not; absent, either.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Filter {
	pattern: String,
	facets: HashMap<Facet, bool>,
}

impl Filter {
	fn regex(&self) -> Result<Option<Regex>, regex_lite::Error> {
		match self.pattern.is_empty() {
			true => Ok(None),
			false => RegexBuilder::new(&self.pattern).case_insensitive(true).build().map(Some),
		}
	}

	/// The cards that pass. An invalid pattern passes every card: the bar shows why.
	pub(crate) fn keep(&self, cards: Vec<(GmailDto, LocationSummary)>) -> Vec<(GmailDto, LocationSummary)> {
		let re = self.regex().ok().flatten();
		cards
			.into_iter()
			.filter(|(g, l)| {
				let text = [l.target.address.as_deref().unwrap_or_default(), &l.target.label, &l.target.place_id, &g.gmail];
				re.as_ref().is_none_or(|re| text.iter().any(|t| re.is_match(t))) && self.facets.iter().all(|(f, want)| f.holds(g, l) == *want)
			})
			.collect()
	}
}

// Grafana's dark canvas and panel, so the block reads as the frame's first panel
#[cfg(feature = "grafana")]
const AROUND: &str = "bg-[#111217] px-4 pt-4";
#[cfg(feature = "grafana")]
const PANEL: &str = "border-[#ccccdc1f] bg-[#181b1f]";
#[cfg(not(feature = "grafana"))]
const AROUND: &str = "px-6 pt-6";
#[cfg(not(feature = "grafana"))]
const PANEL: &str = "border-border bg-card";

/// The filter over what it keeps (`kept`, of `total`) and their totals.
#[component]
pub(crate) fn Bar(kept: Vec<LocationSummary>, total: usize) -> Element {
	let mut filter: Signal<Filter> = use_context();
	let invalid = filter.read().regex().err().map(|e| e.to_string());
	let sum = |f: fn(&LocationSummary) -> i64| kept.iter().map(f).sum::<i64>();
	let live = sum(|l| l.live);
	let responded = match live {
		0 => "–".to_owned(),
		live => format!("{:.0}%", 100. * sum(|l| l.responded) as f64 / live as f64),
	};
	let totals = [
		("live", live.to_string(), ""),
		("new 7d", sum(|l| l.new_7d).to_string(), ""),
		("responded", responded, ""),
		("suspended", sum(|l| l.suspended).to_string(), "text-accent-error"),
		("removed", sum(|l| l.removed).to_string(), "text-accent-error"),
		("reinstating", sum(|l| l.reinstating).to_string(), "text-accent-warn"),
	];
	rsx! {
		div { class: "shrink-0 {AROUND}",
			div { class: "flex flex-col gap-3 rounded-md border px-4 py-3 {PANEL}",
				div { class: "flex flex-wrap items-baseline gap-x-8 gap-y-1",
					span { class: "font-medium", "Across locations" }
					for (label, value, tone) in totals {
						span { key: "{label}", class: "flex items-baseline gap-1.5",
							span { class: "text-ink-soft", "{label}" }
							span { class: "text-[20px] leading-none font-semibold {tone}", "{value}" }
						}
					}
					div { class: "flex-1" }
					span { class: "text-ink-soft", "{kept.len()} of {total}" }
				}
				div { class: "flex flex-wrap items-center gap-1.5",
					span { title: invalid.clone().unwrap_or_default(),
						Input {
							class: if invalid.is_some() { "w-64 border-accent-error text-accent-error" } else { "w-64" },
							size: Size::Sm,
							placeholder: "regex: address, name, place id, gmail",
							value: filter.read().pattern.clone(),
							oninput: move |e: FormEvent| filter.write().pattern = e.value(),
						}
					}
					for f in Facet::iter() {
						Tri {
							key: "{f:?}",
							label: f.to_string(),
							state: filter.read().facets.get(&f).copied(),
							on_change: move |s| {
								let mut w = filter.write();
								match s {
									Some(want) => w.facets.insert(f, want),
									None => w.facets.remove(&f),
								};
							},
						}
					}
					div { class: "flex-1" }
					Button {
						variant: ButtonVariant::Ghost,
						size: Size::Sm,
						r#type: "button",
						disabled: *filter.read() == Filter::default(),
						onclick: move |_| filter.set(Filter::default()),
						"Clear"
					}
				}
			}
		}
	}
}

/// A facet's toggle: `state` true, required; false, ruled out; absent, either.
#[component]
fn Tri(label: String, state: Option<bool>, on_change: EventHandler<Option<bool>>) -> Element {
	rsx! {
		Toggle {
			variant: ToggleVariant::Outline,
			size: Size::Sm,
			class: match state {
				Some(true) => "text-positive",
				Some(false) => "text-accent-error",
				None => "text-ink-soft",
			},
			pressed: state.is_some(),
			// any → must → must not → any
			on_pressed_change: move |_| {
				on_change(match state {
					None => Some(true),
					Some(true) => Some(false),
					Some(false) => None,
				})
			},
			match state {
				Some(true) => rsx! { "✓ {label}" },
				Some(false) => rsx! { "✗ {label}" },
				None => rsx! { "{label}" },
			}
		}
	}
}
