//! The locations overview drawn by Grafana (`grafana/dashboards/ra-overview.json`) in place of the
//! cards, and a place's Stats tile (`ra-location`, its panels tiled like the board by the tilers
//! plugin, `evmain-tilers-app`). One frame from `/grafana/` starts with the page
//! and lives as long: its boot takes seconds, moving it to another dashboard or vars does not. A
//! [`Slot`] says where it shows and what; the frame is laid over the slot on screen and steered
//! there, never reloaded. The overview posts up what is clicked (`ra-open`, `ra-scan`, `ra-enable`).

use dioxus::{
	core::{Runtime, current_scope_id},
	prelude::*,
	web::WebEventExt,
};
use wasm_bindgen::{JsCast, prelude::*};

use crate::{Refresh, Route, View, act};

const KIOSK: &str = "?kiosk&_dash.hideTimePicker&_dash.hideVariables&_dash.hideLinks";

fn grafana() -> reqwest::Url {
	crate::base().join("grafana/").expect("a relative path joins")
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
	/// under `/grafana/`
	path: String,
	on: Option<EventHandler<Up>>,
}

/// A click in the overview's cards.
enum Up {
	Open { gmail: i64, target: i64 },
	Scan { gmail: i64, target: i64 },
	Enable { gmail: i64, target: i64, on: bool },
}

/// Holds the frame for everything under it, as `me` until a slot asks for another dashboard.
#[component]
pub fn Host(me: i64, children: Element) -> Element {
	let Frame { slots, .. } = use_context_provider(|| Frame { me, slots: Signal::new(Vec::new()) });
	let mut wrap = use_signal(|| None::<web_sys::Element>);
	let mut frame = use_signal(|| None::<web_sys::HtmlIFrameElement>);
	let mut shown = use_signal(|| None::<ScopeId>);
	use_future(move || async move {
		let (mut steered, mut style) = (String::new(), String::new());
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
			let inner = f.content_window().expect("a mounted frame has a window");
			// the frame's first document is about:blank, whose history takes no Grafana URL
			if inner.location().pathname().expect("a same-origin location reads") == "blank" {
				continue;
			}
			let doc = f.content_document().expect("a same-origin frame's document reads");
			let Some(head) = doc.head() else { continue }; // Grafana's document, still parsing
			if doc.get_element_by_id("ra-dock-theme").is_none() {
				let style = doc.create_element("style").expect("creating an element");
				style.set_id("ra-dock-theme");
				style.set_text_content(Some(&dock_theme()));
				head.append_child(&style).expect("appending to the head");
			}
			let Some((_, path, _)) = on_screen else { continue };
			if path == steered {
				continue;
			}
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
				let kind = field("kind").as_string();
				let id = |k| field(k).as_f64().unwrap_or_else(|| panic!("{kind:?}'s {k} is not an id: {:?}", field(k))) as i64;
				let up = match kind.as_deref() {
					Some("ra-open") => Up::Open {
						gmail: id("gmail"),
						target: id("target"),
					},
					Some("ra-scan") => Up::Scan {
						gmail: id("gmail"),
						target: id("target"),
					},
					Some("ra-enable") => Up::Enable {
						gmail: id("gmail"),
						target: id("target"),
						on: field("on").as_bool().unwrap_or_else(|| panic!("ra-enable's on is not a bool: {:?}", field("on"))),
					},
					_ => return,
				};
				// a click can only come from the slot the frame is over
				let at = shown.peek().expect("a click in a shown frame");
				let on = slots.peek().iter().find(|s| s.id == at).expect("the shown slot is placed").on;
				on.expect("only the overview posts clicks").call(up);
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
	let src = use_hook(|| format!("{}d/ra-overview/{KIOSK}&var-member={me}", grafana()));
	rsx! {
		div {
			class: "relative flex min-h-0 flex-1 flex-col",
			onmounted: move |e| wrap.set(Some(e.as_web_event())),
			{children}
			iframe {
				style: "position:absolute;border:0;inset:0;width:100%;height:100%;visibility:hidden;pointer-events:none",
				src,
				onmounted: move |e| frame.set(Some(e.as_web_event().dyn_into().expect("an iframe element"))),
			}
		}
	}
}

/// The board's [`DOCK_THEME`](crate::board::DOCK_THEME) on the plugin's tilers dock, the kit's tokens
/// resolved here: the frame's document has none of them.
fn dock_theme() -> String {
	let kit = window()
		.get_computed_style(&window().document().expect("a page has a document").document_element().expect("a document has a root"))
		.expect("a root has a style")
		.expect("a rendered root has a computed style");
	let token = |name: &str| kit.get_property_value(name).expect("reading a custom property");
	let tokens: String = crate::board::DOCK_THEME
		.iter()
		.filter_map(|(_, v)| v.strip_prefix("var(")?.strip_suffix(')'))
		.collect::<std::collections::BTreeSet<_>>()
		.into_iter()
		.map(|t| format!("{t}: {};", token(t)))
		.collect();
	let dock: String = crate::board::DOCK_THEME.iter().map(|(k, v)| format!("--tl-{k}: {v};")).collect();
	format!(".tl-packed {{ {tokens} {dock} }} body:has(.tl-packed) {{ background: {}; }}", token("--background"))
}

/// Where the frame shows, and the dashboard (`path`, under `/grafana/`) it shows there.
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

/// `ra-overview` over the places the filter keeps: `keys`, the `gmail:target` pairs in the order shown.
#[component]
pub fn Overview(gmail: Option<i64>, keys: String, tab: Option<i64>) -> Element {
	let Frame { me, .. } = use_context();
	let Refresh(refresh) = use_context();
	let member = tab.unwrap_or(me);
	let gmail = gmail.map(|g| g.to_string()).unwrap_or_default();
	let path = format!("d/ra-overview/{KIOSK}&var-member={member}&var-gmail={gmail}&var-keys={keys}&var-rev={}", refresh());
	let on = move |up: Up| match up {
		Up::Open { gmail, target } => {
			navigator().push(Route::at(tab, View::Place { gmail, target }));
		}
		Up::Scan { gmail, target } => act(move |c| async move { c.scan_tracked(gmail, target).await.map(drop) }),
		Up::Enable { gmail, target, on } => act(move |c| async move { c.patch_track(gmail, target, &review_archive_client::dto::TrackPatch { enabled: Some(on), favorite: None }).await }),
	};
	rsx! {
		Slot { class: "min-h-0 w-full flex-1", path, on }
	}
}

/// `ra-location` for the place.
#[component]
pub fn Stats(target: i64) -> Element {
	rsx! {
		Slot { class: "h-full w-full", path: format!("a/evmain-tilers-app/d/ra-location{KIOSK}&var-target={target}") }
	}
}
