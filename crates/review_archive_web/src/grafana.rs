//! The locations overview drawn by Grafana ([`OVERVIEW`], built by the server's `dashboards.rs`) in place of the
//! cards, and a place's board ([`LOCATION`]: its lanes and runs, tiled by the tilers plugin,
//! `evmain-tilers-app`). One frame from the host's Grafana (the element's `grafana`) starts with the page
//! and lives as long: its boot takes seconds, moving it to another dashboard or vars does not. A
//! [`Slot`] says where it shows and what; the frame is laid over the slot on screen and steered
//! there, never reloaded. Both post up what is done in them ([`up`]): the overview's clicks, the
//! board's drops.

use dioxus::{
	core::{Runtime, current_scope_id},
	prelude::*,
	web::WebEventExt,
};
use review_archive_client::dto::{NewLayout, TrackPatch};
use review_archive_core::dashboards::{LOCATION, OVERVIEW, up, var};
use strum::IntoEnumIterator;
use wasm_bindgen::{JsCast, prelude::*};

use crate::{
	Picked, Refresh, Route, Selection, act,
	filter::reviews::{Review, Reviews},
	may,
};

const KIOSK: &str = "?kiosk&hideLogo&_dash.hideTimePicker&_dash.hideVariables&_dash.hideLinks";
/// the overview shows its `gmbs` variable
const KIOSK_PICKS: &str = "?kiosk&hideLogo&_dash.hideTimePicker&_dash.hideLinks";

fn grafana() -> reqwest::Url {
	crate::directory(crate::host().grafana.as_deref().expect("required at mount under Ui::Grafana"))
}

fn window() -> web_sys::Window {
	web_sys::window().expect("runs in a page")
}

#[derive(Clone, Copy)]
struct Frame {
	me: i64,
	slots: Signal<Vec<Placed>>,
}

#[derive(Clone)]
struct Placed {
	id: ScopeId,
	el: web_sys::Element,
	/// under [`grafana`]
	path: String,
	on: Option<EventHandler<Up>>,
}

/// What is done in a dashboard: a click in the overview's cards, a pick of its `gmbs`, a drop on the board's lanes.
enum Up {
	Select(Picked),
	Open { gmb: i64 },
	Scan { gmb: i64, target: i64 },
	Enable { gmb: i64, target: i64, on: bool },
	Favorite { gmb: i64, target: i64, on: bool },
	Relink { gmb: i64, target: i64 },
	Reinstate { review: i64 },
	Withdraw { review: i64 },
}

/// Holds the frame for everything under it, as `me` until a slot asks for another dashboard.
#[component]
pub fn Host(me: i64, children: Element) -> Element {
	let Frame { slots, .. } = use_context_provider(|| Frame { me, slots: Signal::new(Vec::new()) });
	let mut wrap = use_signal(|| None::<web_sys::Element>);
	let mut frame = use_signal(|| None::<web_sys::HtmlIFrameElement>);
	let mut shown = use_signal(|| None::<ScopeId>);
	let mut failed = use_signal(|| None::<String>);
	let publish = may(sa_auth::Archive::Operate);
	use_future(move || async move {
		let (mut steered, mut style) = (String::new(), String::new());
		// the frame's `gmbs` last reported, so a pick is told once while its echo comes back
		let mut picked = Vec::new();
		loop {
			wasm_bindgen_futures::JsFuture::from(js_sys::Promise::new(&mut |done, _| {
				window().request_animation_frame(&done).expect("requestAnimationFrame takes a function");
			}))
			.await
			.expect("an animation frame never rejects");
			let (Some(w), Some(f)) = (wrap.peek().clone(), frame.peek().clone()) else { continue };
			// a hidden tab's or tile's slot is not displayed, so has no box
			let on_screen = slots.peek().iter().find_map(|s| {
				let r = s.el.get_bounding_client_rect();
				(r.width() > 0. && r.height() > 0.).then(|| (s.id, s.path.clone(), r))
			});
			let next = match &on_screen {
				Some((_, _, r)) => {
					let o = w.get_bounding_client_rect();
					format!("left:{}px;top:{}px;width:{}px;height:{}px", r.left() - o.left(), r.top() - o.top(), r.width(), r.height())
				}
				// still laid out at full size, so the hidden boot draws and queries its panels
				None => "inset:0;width:100%;height:100%;visibility:hidden;pointer-events:none".into(),
			};
			if next != style {
				f.style().set_css_text(&format!("position:absolute;border:0;{next}"));
				style = next;
			}
			let id = on_screen.as_ref().map(|(id, ..)| *id);
			if *shown.peek() != id {
				shown.set(id);
			}
			let Some((_, path, _)) = on_screen else { continue };
			let inner = f.content_window().expect("a mounted frame has a window");
			// the frame's first document is about:blank, whose history takes no Grafana URL
			if inner.location().pathname().expect("a same-origin location reads") == "blank" {
				continue;
			}
			if path == steered {
				let gmbs = |url: reqwest::Url| {
					let mut v: Vec<String> = url
						.query_pairs()
						.filter(|(k, _)| k.strip_prefix("var-") == Some(var::GMBS))
						.map(|(_, v)| v.into_owned())
						.collect();
					v.sort();
					v
				};
				let here = gmbs(reqwest::Url::parse(&inner.location().href().expect("a same-origin location reads")).expect("a location is a URL"));
				// a URL without the variable says nothing of it
				if !here.is_empty() && here != gmbs(grafana().join(&steered).expect("a slot's path is relative")) && here != picked {
					let up = match here.as_slice() {
						[all] if all == "$__all" => Picked::All,
						[none] if none.is_empty() => Picked::Only(Default::default()),
						ids => Picked::Only(
							ids.iter()
								.map(|id| id.parse().unwrap_or_else(|_| panic!("var-gmbs={id}: the variable's values are GMB ids")))
								.collect(),
						),
					};
					picked = here;
					let on = slots.peek().iter().find(|s| Some(s.id) == *shown.peek()).and_then(|s| s.on);
					on.expect("only the overview has gmbs, and it takes what is done in it").call(Up::Select(up));
				}
				continue;
			}
			picked.clear();
			// Grafana's router follows its window's location on popstate; replaced, so back stays the page's
			let url = grafana().join(&path).expect("a slot's path is relative");
			inner
				.history()
				.expect("a window has a history")
				.replace_state_with_url(&JsValue::NULL, "", Some(url.as_str()))
				.expect("a same-origin URL");
			inner
				.dispatch_event(&web_sys::PopStateEvent::new("popstate").expect("a plain event"))
				.expect("dispatching to a window");
			steered = path;
		}
	});
	let listener = use_hook(|| {
		let (rt, host) = (Runtime::current(), current_scope_id());
		let on_message = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
			rt.in_scope(host, || {
				let ours = frame.peek().as_ref().and_then(|f| f.content_window());
				let data = e.data();
				if e.origin() != grafana().origin().ascii_serialization() || ours.map(JsValue::from) != e.source().map(JsValue::from) || !data.is_object() {
					return;
				}
				let field = |k: &str| js_sys::Reflect::get(&data, &k.into()).expect("an object's field reads");
				if let Some(tilers) = field("tilers").as_string() {
					// the published layouts of a dashboard the plugin docks are under its uid
					let path = e
						.source()
						.expect("checked to be the frame")
						.unchecked_into::<web_sys::Window>()
						.location()
						.pathname()
						.expect("a same-origin location reads");
					let uid = path.rsplit_once("/d/").expect("the plugin docks dashboards at …/d/<uid>").1.trim_end_matches('/').to_owned();
					let text = |k: &str| field(k).as_string().unwrap_or_else(|| panic!("tilers' {tilers} has no string {k}: {:?}", field(k)));
					match tilers.as_str() {
						"ready" => {
							spawn(async move {
								match crate::api().list_layouts(&uid).await {
									Ok(layouts) => {
										let published = js_sys::Object::new();
										for l in layouts {
											js_sys::Reflect::set(&published, &l.band.into(), &l.layout.into()).expect("setting a field");
										}
										tell(frame, "host", &[("published", published.into()), ("publish", publish.into())]);
									}
									Err(e) => failed.set(Some(crate::shown(e))),
								}
							});
						}
						"publish" => {
							let (band, layout) = (text("band"), text("layout"));
							spawn(async move {
								let said = match crate::api().publish_layout(&uid, &band, &NewLayout { layout }).await {
									Ok(()) => format!("Published as everyone's default ({band})"),
									Err(e) => format!("Publishing failed: {}", crate::shown(e)),
								};
								tell(frame, "notify", &[("text", said.into())]);
							});
						}
						_ => panic!("the tilers plugin posts its host only ready and publish, not {tilers}"),
					}
					return;
				}
				let kind = field("kind").as_string();
				let id = |k| field(k).as_f64().unwrap_or_else(|| panic!("{kind:?}'s {k} is not an id: {:?}", field(k))) as i64;
				let up = match kind.as_deref() {
					Some(up::OPEN) => Up::Open { gmb: id("gmb") },
					Some(up::SCAN) => Up::Scan {
						gmb: id("gmb"),
						target: id("target"),
					},
					Some(up::ENABLE) => Up::Enable {
						gmb: id("gmb"),
						target: id("target"),
						on: field("on").as_bool().unwrap_or_else(|| panic!("ra-enable's on is not a bool: {:?}", field("on"))),
					},
					Some(up::FAVORITE) => Up::Favorite {
						gmb: id("gmb"),
						target: id("target"),
						on: field("on").as_bool().unwrap_or_else(|| panic!("ra-favorite's on is not a bool: {:?}", field("on"))),
					},
					Some(up::RELINK) => Up::Relink {
						gmb: id("gmb"),
						target: id("target"),
					},
					Some(up::REINSTATE) => Up::Reinstate { review: id("review") },
					Some(up::WITHDRAW) => Up::Withdraw { review: id("review") },
					_ => return,
				};
				// a click can only come from the slot the frame is over
				let at = shown.peek().expect("a click in a shown frame");
				let on = slots.peek().iter().find(|s| s.id == at).expect("the shown slot is placed").on;
				on.expect("only a slot that takes what is done in it posts").call(up);
			})
		});
		window()
			.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref())
			.expect("adding a listener");
		std::rc::Rc::new(on_message)
	});
	use_drop(move || {
		window()
			.remove_event_listener_with_callback("message", listener.as_ref().as_ref().unchecked_ref())
			.expect("removing a listener");
	});
	// it boots on the caller's overview, the likeliest first look, and is steered from there
	let src = use_hook(|| format!("{}d/{OVERVIEW}/{KIOSK}&var-{}={me}", grafana(), var::MEMBER));
	rsx! {
		div {
			class: "relative flex min-h-0 flex-1 flex-col",
			onmounted: move |e| wrap.set(Some(e.as_web_event())),
			if let Some(e) = failed() {
				div { class: "border-b border-border px-6 py-2 text-accent-error", "{e}" }
			}
			{children}
			iframe {
				style: "position:absolute;border:0;inset:0;width:100%;height:100%;visibility:hidden;pointer-events:none",
				src,
				onmounted: move |e| frame.set(Some(e.as_web_event().dyn_into().expect("an iframe element"))),
			}
		}
	}
}

/// Posts `{tilers: kind, ..fields}` to the tilers plugin in the frame: its host's side of the plugin's
/// embedding contract (`?_tilers.host`).
fn tell(frame: Signal<Option<web_sys::HtmlIFrameElement>>, kind: &str, fields: &[(&str, JsValue)]) {
	let msg = js_sys::Object::new();
	for (k, v) in [("tilers", JsValue::from(kind))].iter().chain(fields) {
		js_sys::Reflect::set(&msg, &(*k).into(), v).expect("setting a field");
	}
	frame
		.peek()
		.as_ref()
		.and_then(|f| f.content_window())
		.expect("only a mounted frame's plugin asks")
		.post_message(&msg, &grafana().origin().ascii_serialization())
		.expect("posting to a same-origin frame");
}

/// Where the frame shows, and the dashboard (`path`, under [`grafana`]) it shows there.
#[component]
fn Slot(class: String, path: String, on: Option<EventHandler<Up>>) -> Element {
	let Frame { mut slots, .. } = use_context();
	let id = current_scope_id();
	let mut el = use_signal(|| None::<web_sys::Element>);
	use_effect(use_reactive!(|path, on| {
		let Some(el) = el() else { return };
		let mut slots = slots.write();
		slots.retain(|s| s.id != id);
		slots.push(Placed { id, el, path, on });
	}));
	use_drop(move || slots.write().retain(|s| s.id != id));
	rsx! {
		div { class, onmounted: move |e| el.set(Some(e.as_web_event())) }
	}
}

/// `ra-overview` over the places the filter keeps: `keys`, the `gmb:target` pairs in the order shown.
#[component]
pub fn Overview(gmail: Option<i64>, keys: String, tab: Option<i64>) -> Element {
	let Frame { me, .. } = use_context();
	let Refresh(refresh) = use_context();
	let Selection(selection) = use_context();
	let mut reviews: Signal<Reviews> = use_context();
	let member = tab.unwrap_or(me);
	let gmail = gmail.map(|g| g.to_string()).unwrap_or_default();
	// a member's tab is not the host's selection: its `gmbs` stays the frame's own
	let gmbs = match (tab, selection()) {
		(Some(_), _) => String::new(),
		(None, Picked::All) => format!("&var-{}=$__all", var::GMBS),
		(None, Picked::Only(ids)) if ids.is_empty() => format!("&var-{}=", var::GMBS),
		(None, Picked::Only(ids)) => {
			let mut ids: Vec<_> = ids.into_iter().collect();
			ids.sort_unstable();
			ids.iter().map(|id| format!("&var-{}={id}", var::GMBS)).collect()
		}
		(None, Picked::Unknown) => unreachable!("the overview shows once the selection is known"),
	};
	let path = format!(
		"d/{OVERVIEW}/{KIOSK_PICKS}&var-{}={member}&var-{}={gmail}&var-{}={keys}{gmbs}&var-{}={}",
		var::MEMBER,
		var::GMAIL,
		var::KEYS,
		var::REV,
		refresh()
	);
	let on = move |up: Up| match up {
		Up::Select(picked) if tab.is_none() => crate::select(&picked),
		Up::Select(_) => {}
		Up::Open { gmb } => {
			reviews.set(Reviews::default());
			navigator().push(Route::at(tab, review_archive_core::views::Board::Gmb { gmb }));
		}
		Up::Scan { gmb, target } => act(move |c, tab| async move { c.scan_track(gmb, target, tab).await.map(drop) }),
		Up::Enable { gmb, target, on } => act(move |c, tab| async move { c.patch_track(gmb, target, tab, &TrackPatch { enabled: Some(on), favorite: None }).await }),
		Up::Favorite { gmb, target, on } => act(move |c, tab| async move { c.patch_track(gmb, target, tab, &TrackPatch { enabled: None, favorite: Some(on) }).await }),
		Up::Relink { gmb, target } => act(move |c, tab| async move { c.relink_track(gmb, target, tab).await.map(drop) }),
		Up::Reinstate { .. } | Up::Withdraw { .. } => unreachable!("the overview has no lanes"),
	};
	rsx! {
		Slot { class: "min-h-0 w-full flex-1", path, on }
	}
}

/// `ra-location` for the place: its lanes and runs, narrowed by [`Reviews`] (`var-<Review:?>`: true, false, or empty for either).
/// `api` is where its cards' captures are.
#[component]
pub fn Location(gmb: i64, target: i64, tab: Option<i64>) -> Element {
	let Frame { me, .. } = use_context();
	let Refresh(refresh) = use_context();
	let reviews: Signal<Reviews> = use_context();
	let member = tab.unwrap_or(me);
	let api = js_sys::encode_uri_component(crate::base().as_str());
	let narrowed: String = Review::iter()
		.map(|r| format!("&var-{r:?}={}", reviews.read().0.get(&r).map(bool::to_string).unwrap_or_default()))
		.collect();
	let path = format!(
		"a/evmain-tilers-app/d/{LOCATION}{KIOSK}&_tilers.host&var-{}={member}&var-{}={gmb}&var-{}={target}&var-{}={api}{narrowed}&var-{}={}",
		var::MEMBER,
		var::GMB,
		var::TARGET,
		var::API,
		var::REV,
		refresh()
	);
	let on = move |up: Up| match up {
		Up::Reinstate { review } => act(move |c, tab| async move { c.reinstate(gmb, review, tab).await.map(drop) }),
		Up::Withdraw { review } => act(move |c, tab| async move { c.withdraw_reinstatement(gmb, review, tab).await }),
		Up::Open { .. } | Up::Scan { .. } | Up::Enable { .. } | Up::Favorite { .. } | Up::Relink { .. } | Up::Select(_) => unreachable!("a board has no location cards nor gmbs"),
	};
	rsx! {
		Slot { class: "min-h-0 w-full flex-1", path, on }
	}
}
