//! The locations overview drawn by Grafana (`grafana/dashboards/ra-overview.json`) in place of the
//! cards, and a place's Stats tile (`ra-location`), framed from `/grafana/` beside the API. The
//! filter stays ours: the frame is posted the places it keeps (`ra-vars`), and posts back what is
//! clicked (`ra-open`, `ra-scan`, `ra-enable`), or `ra-ready` when it wants the vars again.

use dioxus::{
	core::{Runtime, current_scope_id},
	prelude::*,
	web::WebEventExt,
};
use wasm_bindgen::{JsCast, prelude::*};

use crate::{Refresh, Route, View, act, api, base, shown};

fn grafana() -> reqwest::Url {
	base().join("grafana/").expect("a relative path joins")
}

/// What the overview's panels read: `keys` are the `gmail:target` pairs the filter keeps, in
/// the order shown; `rev` changes whenever the overview was read again.
#[derive(Clone, PartialEq)]
struct Vars {
	keys: String,
	rev: u32,
}

impl Vars {
	fn message(&self) -> JsValue {
		let o = js_sys::Object::new();
		for (k, v) in [("kind", JsValue::from("ra-vars")), ("keys", self.keys.clone().into()), ("rev", self.rev.to_string().into())] {
			js_sys::Reflect::set(&o, &k.into(), &v).expect("setting a field of a plain object");
		}
		o.into()
	}
}

#[component]
pub fn Overview(gmail: Option<i64>, keys: String, tab: Option<i64>) -> Element {
	// `/me` is who signed in: a tab's member is theirs, the caller's own is read here
	let me = use_resource(|| async { api().me().await.map_err(shown) });
	match (tab, &*me.read()) {
		(Some(member), _) => rsx! { Framed { member, gmail, keys, tab } },
		(None, Some(Ok(me))) => rsx! { Framed { member: me.id, gmail, keys, tab } },
		(None, Some(Err(e))) => rsx! { div { class: "p-6 text-accent-error", "{e}" } },
		(None, None) => rsx! {},
	}
}

#[component]
fn Framed(member: i64, gmail: Option<i64>, keys: String, tab: Option<i64>) -> Element {
	let Refresh(refresh) = use_context();
	let mut vars = use_signal(|| Vars {
		keys: keys.clone(),
		rev: *refresh.peek(),
	});
	let mut frame = use_signal(|| None::<web_sys::HtmlIFrameElement>);
	let origin = grafana().origin().ascii_serialization();
	let post = {
		let origin = origin.clone();
		move || {
			if let Some(w) = frame.peek().as_ref().and_then(|f| f.content_window()) {
				w.post_message(&vars.peek().message(), &origin).expect("posting to a frame");
			}
		}
	};
	let again = post.clone();
	use_effect(use_reactive!(|keys| {
		let next = Vars { keys, rev: refresh() };
		if *vars.peek() != next {
			vars.set(next);
			post();
		}
	}));
	let listener = use_hook(|| {
		let (rt, scope) = (Runtime::current(), current_scope_id());
		let on_message = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
			// every member tab stays mounted, each with its own frame
			let ours = rt.in_scope(scope, || frame.peek().as_ref().and_then(|f| f.content_window()));
			let data = e.data();
			if e.origin() != origin || ours.map(JsValue::from) != e.source().map(JsValue::from) || !data.is_object() {
				return;
			}
			let field = |k: &str| js_sys::Reflect::get(&data, &k.into()).expect("an object's field reads");
			let kind = field("kind").as_string();
			let id = |k| field(k).as_f64().unwrap_or_else(|| panic!("{kind:?}'s {k} is not an id: {:?}", field(k))) as i64;
			rt.in_scope(scope, || match kind.as_deref() {
				Some("ra-ready") => again(),
				Some("ra-open") => {
					navigator().push(Route::at(
						tab,
						View::Place {
							gmail: id("gmail"),
							target: id("target"),
						},
					));
				}
				Some("ra-scan") => {
					let (g, t) = (id("gmail"), id("target"));
					act(move |c| async move { c.scan_tracked(g, t).await.map(drop) });
				}
				Some("ra-enable") => {
					let (g, t) = (id("gmail"), id("target"));
					let on = field("on").as_bool().unwrap_or_else(|| panic!("ra-enable's on is not a bool: {:?}", field("on")));
					act(move |c| async move { c.set_track_enabled(g, t, on).await });
				}
				_ => {}
			});
		});
		web_sys::window()
			.expect("a browser")
			.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref())
			.expect("adding a listener");
		std::rc::Rc::new(on_message)
	});
	use_drop(move || {
		web_sys::window()
			.expect("a browser")
			.remove_event_listener_with_callback("message", listener.as_ref().as_ref().unchecked_ref())
			.expect("removing a listener");
	});
	// the first vars ride in the URL; later ones are posted, so the frame never reloads
	let src = use_hook(|| {
		let Vars { keys, rev } = vars.peek().clone();
		let gmail = gmail.map(|g| g.to_string()).unwrap_or_default();
		format!(
			"{}d/ra-overview/?kiosk&_dash.hideTimePicker&_dash.hideVariables&_dash.hideLinks&var-member={member}&var-gmail={gmail}&var-keys={keys}&var-rev={rev}",
			grafana()
		)
	});
	rsx! {
		iframe {
			class: "min-h-0 w-full flex-1 border-0",
			src,
			onmounted: move |e| frame.set(Some(e.as_web_event().dyn_into().expect("an iframe element"))),
		}
	}
}

/// `ra-location` for the place.
#[component]
pub fn Stats(target: i64) -> Element {
	rsx! {
		iframe { class: "h-full w-full border-0", src: "{grafana()}d/ra-location/?kiosk&_dash.hideTimePicker&_dash.hideVariables&_dash.hideLinks&var-target={target}" }
	}
}
