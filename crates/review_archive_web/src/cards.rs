//! The locations overview as the kit draws it: a card per place.

use dioxus::prelude::*;
use ev_lib::uikit::{Button, ButtonVariant, Card, Size, Spinner, Switch};
use review_archive_client::dto::{GmailDto, LocationSummary, RunStatus, ScanState};
use sa_ui::TokenCost;

use crate::{Badge, Route, Tone, act, ago};

#[component]
/// `all`: among every gmail's cards, so it names its gmail.
pub fn LocationCard(gmail: GmailDto, all: bool, loc: LocationSummary, to: Route) -> Element {
	let (dot, status) = match loc.last_run_status {
		Some(RunStatus::Ok) => ("bg-positive", "ok"),
		Some(RunStatus::Partial) => ("bg-accent-warn", "partial"),
		Some(RunStatus::Failed) => ("bg-accent-error", "failed"),
		None => ("bg-ink-soft", "never scanned"),
	};
	let last = loc.last_run_at.as_deref().map(|t| format!("last run {} · ", ago(t))).unwrap_or_default();
	let (dot, scan) = match &loc.scan {
		Some(ScanState::Queued { since, ahead: 0 }) => ("bg-accent-warn", format!("queued · next · since {} · ", ago(since))),
		Some(ScanState::Queued { since, ahead }) => ("bg-accent-warn", format!("queued · {ahead} ahead · since {} · ", ago(since))),
		Some(ScanState::Running { since }) => ("animate-pulse bg-positive", format!("scanning · started {} · ", ago(since))),
		None => (dot, String::new()),
	};
	let target = loc.target.id;
	let (gmail_on, gmail) = (gmail.enabled, gmail);
	let id = gmail.id;
	rsx! {
		div { class: "relative",
		Link { class: if loc.enabled && gmail_on { "block h-full text-left" } else { "block h-full text-left opacity-50" }, to,
			Card { class: "h-full gap-3 rounded-lg p-4 py-4 hover:border-ink-soft",
				div { class: "pr-12",
					div { class: "truncate text-[14px] font-semibold", "{loc.target.label}" }
					if let Some(address) = &loc.target.address {
						div { class: "truncate text-ink-soft", title: "{address}", "{address}" }
					}
					div { class: "truncate font-mono text-[11px] text-ink-soft", "{loc.target.place_id}" }
					if all {
						div { class: "truncate text-[11px] text-ink-soft", "{gmail.gmail}" }
					}
				}
				div { class: "flex items-baseline gap-3",
					span { class: "text-[28px] font-semibold leading-none", "{loc.snapshots_7d}" }
					span { class: "text-ink-soft", "snapshots 7d" }
					span { class: "text-[15px] text-ink-soft", "{loc.snapshots_30d}" }
					span { class: "text-ink-soft", "30d" }
				}
				div { class: "flex flex-wrap gap-1.5",
					Badge { tone: Tone::Ok,
						match loc.listed {
							Some(listed) => rsx! { span { title: "archived and still listed / listed on Google", "live {loc.live}/{listed}" } },
							None => rsx! { "live {loc.live}" },
						}
					}
					Badge { tone: Tone::Ok,
						span { title: "posted over the last 7 days, removed ones included", "new 7d +{loc.new_7d}" }
					}
					if let Some(post) = &loc.latest_post {
						Badge { tone: Tone::Ok,
							span { title: "owner's latest post, {ago(post.published_est.as_deref().unwrap_or(&post.first_seen))}:\n{post.text}", "posts 7d {loc.posts_7d}" }
						}
					}
					Badge { tone: if loc.responded < loc.live { Tone::Warn } else { Tone::Ok },
						span { title: "replied to by the owner / live", "responded {loc.responded}/{loc.live}" }
					}
					if loc.removed > 0 {
						Badge { tone: Tone::Bad, "removed {loc.removed}" }
					}
					if loc.reinstating > 0 {
						Badge { tone: Tone::Warn, "reinstating {loc.reinstating}" }
					}
					if loc.held {
						Badge { tone: Tone::Warn,
							span { title: "not scanned: every member tracking it is out of tokens", "out of tokens" }
						}
					}
				}
				div { class: "mt-auto flex items-center gap-2 border-t border-border pt-3 text-[11px] text-ink-soft",
					span { class: "size-1.5 rounded-full {dot}" }
					"{scan}{last}"
					match (&loc.last_run_status, &loc.last_run_error) {
						(Some(RunStatus::Failed | RunStatus::Partial), Some(why)) => rsx! { span { title: "{why}", "{status}" } },
						_ => rsx! { "{status}" },
					}
				}
			}
		}
		Switch {
			class: "absolute top-4 right-4",
			checked: loc.enabled,
			disabled: !gmail_on,
			on_checked_change: move |on: bool| act(move |c| async move { c.set_track_enabled(id, target, on).await }),
		}
		div { class: "absolute right-2 bottom-2",
			if loc.scan.is_some() {
				Button { variant: ButtonVariant::Ghost, size: Size::Sm, icon: true, r#type: "button", disabled: true, Spinner {} }
			} else {
				TokenCost { tokens: loc.scan_cost,
					Button {
						variant: ButtonVariant::Ghost,
						size: Size::Sm,
						icon: true,
						r#type: "button",
						onclick: move |_| act(move |c| async move { c.scan_tracked(id, target).await.map(drop) }),
						span { title: "scan now, on top of the schedule", "↻" }
					}
				}
			}
		}
		}
	}
}
