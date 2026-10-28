//! The locations overview as the kit draws it: a card per place.

use dioxus::{prelude::*, web::WebEventExt};
use ev_lib::uikit_dioxus::{
	Button, ButtonVariant, Card, DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuItemVariant, DropdownMenuTrigger, Size, Spinner, Switch, button_classes,
};
use review_archive_client::dto::{GmailDto, LocationSummary, RunStatus, ScanState, TrackPatch};
use sa_ui::TokenCost;

use crate::{
	Badge, Route, Tone, act, ago,
	filter::reviews::{Review, Reviews},
};

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
	let favorite = loc.favorite;
	let mut cut = use_signal(|| false);
	let mut reviews: Signal<Reviews> = use_context();
	// a tag opens the board without the reviews it counts against; the card's own click clears that
	let narrowed = |r: Review| {
		let to = to.clone();
		move |e: MouseEvent| {
			e.prevent_default();
			e.stop_propagation();
			reviews.set(Reviews::not(r));
			navigator().push(to.clone());
		}
	};
	rsx! {
		div { class: "relative",
		Link {
			class: if loc.enabled && gmail_on { "block h-full text-left" } else { "block h-full text-left opacity-50" },
			to: to.clone(),
			onclick: move |_| reviews.set(Reviews::default()),
			Card { class: if loc.favorite { "h-full gap-3 rounded-lg px-4 pt-2 pb-4 border-accent-warn" } else { "h-full gap-3 rounded-lg px-4 pt-2 pb-4 hover:border-ink-soft" },
				div {
					div { class: "flex min-h-7 flex-col justify-end", // a name that wraps takes the space above it
						div {
							class: "line-clamp-2 text-[14px] leading-5 font-semibold",
							title: if cut() { "{loc.target.label}" },
							onresize: move |e| {
								let name = e.as_web_event().target();
								cut.set(name.scroll_height() > name.client_height());
							},
							"{loc.target.label}"
						}
					}
					if let Some(address) = &loc.target.address {
						div { class: "truncate text-ink-soft", title: "{address}", "{address}" }
					}
					div { class: "truncate font-mono text-[11px] text-ink-soft", "{loc.target.place_id}" }
					if all {
						div { class: "truncate text-[11px] text-ink-soft", "{gmail.gmail}" }
					}
				}
				div { class: "flex items-end gap-6",
					div { class: "flex flex-col gap-1",
						span { class: "text-[28px] font-semibold leading-none", "{loc.snapshots_7d}" }
						span { class: "text-[11px] text-ink-soft", "snapshots 7d" }
					}
					div { class: "flex flex-col gap-1 text-ink-soft",
						span { class: "text-[18px] font-semibold leading-none", "{loc.snapshots_30d}" }
						span { class: "text-[11px]", "snapshots 30d" }
					}
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
					span { onclick: narrowed(Review::Responded),
						Badge { tone: if loc.responded < loc.live { Tone::Warn } else { Tone::Ok },
							span { title: "replied to by the owner / live", "responded {loc.responded}/{loc.live}" }
						}
					}
					span { onclick: narrowed(Review::FiveStar),
						Badge { tone: Tone::Neutral,
							span { title: "listed reviews under 5 stars, or unrated", "non-5-star {loc.non_five_star}" }
						}
					}
					if loc.corrupted > 0 {
						Badge { tone: Tone::Warn,
							span { title: "reviews whose every screenshot is cut off / reviews with one; the next scan retakes them", "corrupted {loc.corrupted}/{loc.captured}" }
						}
					}
					if loc.suspended > 0 {
						Badge { tone: Tone::Bad,
							span { title: "5-star reviews gone from the listing", "suspended {loc.suspended}" }
						}
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
				div { class: "mt-auto flex items-center gap-2 border-t border-border pt-3 pr-28 text-[11px] text-ink-soft",
					span { class: "flex items-center gap-2",
						title: match (&loc.last_run_status, &loc.last_run_error) {
							(Some(RunStatus::Failed | RunStatus::Partial), Some(why)) => Some(why.clone()),
							_ => None,
						},
						span { class: "size-1.5 rounded-full {dot}" }
						"{scan}{last}{status}"
					}
				}
			}
		}
		div { class: "absolute right-2 bottom-2 flex items-center gap-1",
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
			label { class: "inline-flex h-8 cursor-pointer items-center rounded-[var(--control-radius)] px-1.5 transition-all hover:bg-hover has-disabled:cursor-not-allowed has-disabled:hover:bg-transparent", // ↻'s hover, and a click on it toggles
				Switch {
					checked: loc.enabled,
					disabled: !gmail_on,
					on_checked_change: move |on: bool| act(move |c| async move { c.patch_track(id, target, &TrackPatch { enabled: Some(on), favorite: None }).await }),
				}
			}
			DropdownMenu {
				DropdownMenuTrigger { class: button_classes(&ButtonVariant::Ghost, Size::Sm, true, None, "text-ink"),
					svg { class: "size-5", view_box: "0 0 24 24", fill: "currentColor",
						title { "more" }
						circle { cx: "12", cy: "5", r: "2" }
						circle { cx: "12", cy: "12", r: "2" }
						circle { cx: "12", cy: "19", r: "2" }
					}
				}
				DropdownMenuContent { class: "left-auto right-0 z-50",
					DropdownMenuItem {
						onclick: move |_| act(move |c| async move { c.patch_track(id, target, &TrackPatch { enabled: None, favorite: Some(!favorite) }).await }),
						if favorite {
							span { class: "text-accent-warn", "★" }
							"unfavorite"
						} else {
							span { "☆" }
							"favorite"
						}
					}
					DropdownMenuItem { onclick: move |_| act(move |c| async move { c.relink(id, target).await.map(drop) }),
						span { "⟳" }
						span { title: "ask Google for the place's id, name and address again", "re-link" }
					}
					DropdownMenuItem {
						variant: DropdownMenuItemVariant::Destructive,
						onclick: move |_| act(move |c| async move { c.untrack(id, target).await }),
						span { "🗑" }
						span { title: "stop tracking under this gmail; the archive stays", "delete" }
					}
				}
			}
		}
		}
	}
}
