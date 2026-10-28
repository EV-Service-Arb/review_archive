//! One place's reviews in four tiles of a dock. A card is dragged from Suspended (5 stars) or Removed
//! to Reinstating when its reinstatement was asked of Google, and back when that is withdrawn; the
//! columns keep their own order, and a review a scan lists again goes back to Snapshotted by itself.

use std::rc::Rc;

use dioxus::{prelude::*, web::WebEventExt};
use review_archive_client::dto::{BoardCard, GmailDto, LocationSummary, NewLayout};
use tilers_dioxus::{Band, Config, DockPanel, Group, GroupId, MinSize, PackedApi, PackedArea, PanelId, Step};

use crate::{Api, Badge, Refresh, Route, Tone, View, act, ago, host, may, shown};

#[derive(Clone, Copy, PartialEq)]
enum Column {
	Snapshotted,
	Suspended,
	Removed,
	Reinstating,
}

impl Column {
	/// Where a gone review without an open appeal sits.
	fn gone(rating: Option<i64>) -> Self {
		match rating {
			Some(5) => Self::Suspended,
			_ => Self::Removed,
		}
	}
}

const LANES: [(Column, &str, &str); 4] = [
	(Column::Snapshotted, "snapshotted", "Snapshotted"),
	(Column::Suspended, "suspended", "Suspended"),
	(Column::Removed, "removed", "Removed"),
	(Column::Reinstating, "reinstating", "Reinstating"),
];

/// Tiles past the lanes.
#[cfg(feature = "grafana")]
const EXTRA: &[&str] = &["stats"];
#[cfg(not(feature = "grafana"))]
const EXTRA: &[&str] = &[];

const MIN: MinSize = MinSize::Steps { w: Step(2), h: Step(4) };

/// Maps the dock's chrome onto the kit's tokens: the dock's custom properties, unprefixed.
pub(crate) const DOCK_THEME: [(&str, &str); 11] = [
	("group-bg", "var(--secondary)"),
	("tabstrip-bg", "var(--secondary)"),
	("tab-bg", "var(--secondary)"),
	("tab-active-bg", "var(--background)"),
	("tab-active-fg", "var(--ink)"),
	("tab-border", "var(--border)"),
	("fg", "var(--ink)"),
	("accent", "var(--primary)"),
	("shadow-bg", "var(--hover)"),
	("resize-bg", "var(--border)"),
	("content-pad", "0"),
];

#[component]
pub fn Board(gmail: GmailDto, location: LocationSummary, tab: Option<i64>) -> Element {
	let Api(api) = use_context();
	let Refresh(refresh) = use_context();
	let (g, target) = (gmail.id, location.target.id);
	// a layout saved without some tile would never show it
	let dock_key = format!(
		"review-archive-board{}",
		LANES.iter().map(|l| l.1).chain(EXTRA.iter().copied()).map(|t| format!("-{t}")).collect::<String>()
	);
	let board = use_resource({
		let api = api.clone();
		move || {
			let api = api.clone();
			async move {
				refresh();
				api.board(g, target).await.map_err(shown)
			}
		}
	});
	let published = use_resource({
		let (api, dock_key) = (api.clone(), dock_key.clone());
		move || {
			let (api, dock_key) = (api.clone(), dock_key.clone());
			async move { api.layouts(&dock_key).await.map_err(shown) }
		}
	});
	let dragging = use_signal(|| None::<Drag>);
	let mut panels = use_signal(Vec::<DockPanel>::new);
	use_effect(move || {
		let Some(Ok(b)) = &*board.read() else { return };
		let mut lists = [b.snapshotted.clone(), b.suspended.clone(), b.removed.clone(), b.reinstating.clone()].into_iter();
		let tiles = LANES.iter().map(|&(column, id, title)| {
			let cards = lists.next().expect("a list per lane");
			DockPanel {
				id: PanelId(id.into()),
				title: format!("{title} · {}", cards.len()),
				content: rsx! { Lane { column, cards, gmail: g, dragging } },
				min: MIN,
			}
		});
		#[cfg(feature = "grafana")]
		let tiles = tiles.chain([DockPanel {
			id: PanelId("stats".into()),
			title: "Stats".into(),
			content: rsx! { crate::grafana::Stats { target } },
			min: MIN,
		}]);
		panels.set(tiles.collect());
	});
	let mut dock = use_signal(|| None::<PackedApi>);
	// the tile `+`: brings back a lane closed with `✕`
	use_context_provider(|| {
		Callback::new(move |group: GroupId| {
			let mut api = dock().expect("tiles exist only after on_band");
			let open = api.tab_ids();
			if let Some(id) = LANES.iter().map(|l| l.1).chain(EXTRA.iter().copied()).map(|t| PanelId(t.into())).find(|p| !open.contains(p)) {
				api.add_tab(group, id);
			}
		})
	});
	let on_publish = may(sa_auth::Archive::Operate).then(|| {
		let (api, dock_key) = (api.clone(), dock_key.clone());
		Rc::new(move |band: Band, layout: String| {
			let (api, dock_key) = (api.clone(), dock_key.clone());
			// the dock calls this outside any scope, so no dioxus `spawn`
			wasm_bindgen_futures::spawn_local(async move {
				let said = match api.publish_layout(&dock_key, &band.to_string(), &NewLayout { layout }).await {
					Ok(()) => format!("Published as everyone's default ({band})"),
					Err(e) => format!("Publishing failed: {}", shown(e)),
				};
				dock.peek().expect("publishing needs the dock").notify(said);
			});
		}) as Rc<dyn Fn(Band, String)>
	});
	let config = Config {
		storage_key: Some(dock_key),
		on_publish,
		..Default::default()
	};
	let theme: String = DOCK_THEME.iter().map(|(k, v)| format!("--tl-{k}: {v};")).collect();
	let on_band = Callback::new(move |mut api: PackedApi| {
		dock.set(Some(api));
		if api.restored() {
			return;
		}
		let Some(Ok(published)) = &*published.peek() else {
			unreachable!("the dock mounts once the published layouts are in")
		};
		if let Some(p) = published.iter().find(|p| p.band == api.band().to_string()) {
			api.load(&p.layout).expect("a published layout loads: this dock saved it");
			return;
		}
		let tiles: Vec<Vec<&str>> = [vec!["snapshotted"], vec!["suspended", "removed", "reinstating"]]
			.into_iter()
			.chain(EXTRA.iter().map(|t| vec![*t]))
			.collect();
		let w = api.cols() / tiles.len() as u32;
		for (i, tabs) in tiles.iter().enumerate() {
			let group = Group {
				id: api.mint_group_id(),
				tabs: tabs.iter().map(|t| PanelId((*t).into())).collect(),
				active: 0,
			};
			let w = if i == tiles.len() - 1 { api.cols() - (tiles.len() as u32 - 1) * w } else { w };
			api.place(group, w, api.rows());
		}
	});
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
		if let Some(Err(e)) = &*board.read() {
			div { class: "border-b border-border px-6 py-2 text-accent-error", "{e}" }
		}
		if let Some(Err(e)) = &*published.read() {
			div { class: "border-b border-border px-6 py-2 text-accent-error", "{e}" }
		}
		div { class: "relative min-h-0 flex-1",
			// on_band places the panels, or loads a published layout, so the dock waits for both
			if !panels.read().is_empty() && matches!(&*published.read(), Some(Ok(_))) {
				div { class: "absolute inset-0", style: theme,
					PackedArea { panels, on_band: Some(on_band), config: Some(config) }
				}
			}
		}
	}
}

/// A card on the move: its review, the lane it left, and the lane it sits in while gone without an appeal.
#[derive(Clone, Copy, PartialEq)]
struct Drag {
	review: i64,
	from: Column,
	gone: Column,
}

impl Drag {
	/// What dropping it onto `to` means, if anything.
	fn onto(self, to: Column) -> Option<bool> {
		match (self.from, to) {
			(Column::Suspended | Column::Removed, Column::Reinstating) => Some(true),
			(Column::Reinstating, to) if to == self.gone => Some(false),
			_ => None,
		}
	}
}

#[component]
fn Lane(column: Column, cards: Vec<BoardCard>, gmail: i64, dragging: Signal<Option<Drag>>) -> Element {
	let accepts = dragging().is_some_and(|d| d.onto(column).is_some());
	rsx! {
		section {
			class: if accepts { "flex h-full flex-col gap-3 overflow-y-auto p-3 font-sans leading-normal outline-2 -outline-offset-2 outline-dashed outline-primary-ink bg-primary/5" } else { "flex h-full flex-col gap-3 overflow-y-auto p-3 font-sans leading-normal" },
			ondragover: move |e| {
				if accepts {
					e.prevent_default();
				}
			},
			ondrop: move |e| {
				e.prevent_default();
				let Some(d) = dragging.take() else { return };
				let review = d.review;
				match d.onto(column) {
					Some(true) => act(move |c| async move { c.reinstate(gmail, review).await.map(drop) }),
					Some(false) => act(move |c| async move { c.withdraw_reinstatement(gmail, review).await }),
					None => {}
				}
			},
			for card in cards {
				Card { key: "{card.review.id}", card: card.clone(), column, dragging }
			}
		}
	}
}

#[component]
fn Card(card: BoardCard, column: Column, mut dragging: Signal<Option<Drag>>) -> Element {
	let r = &card.review;
	let movable = column != Column::Snapshotted;
	let stars = "★".repeat(r.rating.unwrap_or(0).clamp(0, 5) as usize);
	let when = match column {
		Column::Snapshotted => r.published_raw.clone().unwrap_or_else(|| format!("seen {}", ago(&r.first_seen))),
		Column::Suspended | Column::Removed => format!("gone {}", r.gone_at.as_deref().map(ago).unwrap_or_default()),
		Column::Reinstating => format!("requested {}", card.reinstatement.as_ref().map(|x| ago(&x.requested_at)).unwrap_or_default()),
	};
	let reinstated = card
		.reinstatement
		.as_ref()
		.filter(|_| column == Column::Snapshotted)
		.and_then(|x| Some(days_between(&x.requested_at, x.reinstated_at.as_deref()?)));
	let (id, gone) = (r.id, Column::gone(r.rating));
	rsx! {
		article {
			class: if movable { "rounded-lg border border-border bg-card p-3 cursor-grab" } else { "rounded-lg border border-border bg-card p-3" },
			draggable: movable,
			ondragstart: move |e| {
				// Firefox starts no drag without data on it
				if let Some(dt) = e.as_web_event().data_transfer() {
					dt.set_data("text/plain", &id.to_string()).expect("setting drag data during dragstart");
				}
				dragging.set(Some(Drag { review: id, from: column, gone }));
			},
			ondragend: move |_| dragging.set(None),
			if let Some(url) = r.capture_url.clone() {
				img { class: "w-full rounded-md bg-muted", src: "{host().api.trim_end_matches('/')}{url}" }
			}
			div { class: "mt-2 flex items-center gap-2",
				span { class: "text-accent-warn", "{stars}" }
				span { class: "truncate font-medium", "{r.author}" }
			}
			if let Some(text) = &r.text {
				p { class: "mt-1 line-clamp-3 text-ink-soft", "{text}" }
			}
			if let Some(reply) = &r.reply {
				p { class: "mt-2 line-clamp-3 border-l-2 border-border pl-2 text-ink-soft", "{reply}" }
			}
			div { class: "mt-2 flex items-center justify-between text-[11px] text-ink-soft",
				span { "{when}" }
				div { class: "flex gap-1.5",
					if let Some(days) = reinstated {
						Badge { tone: Tone::Ok, "reinstated after {days}d" }
					}
					if r.capture_corrupted == Some(true) {
						Badge { tone: Tone::Warn, span { title: "the screenshot is cut off; the next scan retakes it", "corrupted" } }
					}
					if r.reply.is_some() {
						Badge { tone: Tone::Ok, "responded" }
					} else {
						Badge { tone: Tone::Warn, "no reply" }
					}
				}
			}
		}
	}
}

fn days_between(from: &str, to: &str) -> i64 {
	match (from.parse::<jiff::Timestamp>(), to.parse::<jiff::Timestamp>()) {
		(Ok(a), Ok(b)) => (b.as_second() - a.as_second()) / 86_400,
		_ => unreachable!("the archive writes timestamps in one shape"),
	}
}
