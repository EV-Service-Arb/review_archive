//! A gmail's places as Grafana shows them (`grafana/dashboards`, `nix run .#dev-grafana`), and a
//! place's Stats tile on its board. A card clicked in the frame posts `{kind: "ra-open", gmail,
//! target}` up, and the board opens without a page load.

use dioxus::{
	core::{Runtime, current_scope_id},
	prelude::*,
	web::WebEventExt,
};
use review_archive_client::dto::GmailDto;
use wasm_bindgen::{JsCast, prelude::*};

use crate::{Route, View};

const URL: &str = "http://localhost:59111";

#[component]
pub fn Overview(gmail: GmailDto, tab: Option<i64>) -> Element {
	let mut frame = use_signal(|| None::<web_sys::HtmlIFrameElement>);
	let listener = use_hook(|| {
		let (rt, scope) = (Runtime::current(), current_scope_id());
		let on_message = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
			// every member tab stays mounted, each with its own frame
			let ours = rt.in_scope(scope, || frame.peek().as_ref().and_then(|f| f.content_window()));
			let data = e.data();
			if e.origin() != URL || ours.map(JsValue::from) != e.source().map(JsValue::from) || !data.is_object() {
				return;
			}
			let field = |k: &str| js_sys::Reflect::get(&data, &k.into()).expect("an object's field reads");
			if field("kind").as_string().as_deref() != Some("ra-open") {
				return;
			}
			let id = |k| field(k).as_f64().unwrap_or_else(|| panic!("ra-open's {k} is not an id: {:?}", field(k))) as i64;
			let (gmail, target) = (id("gmail"), id("target"));
			rt.in_scope(scope, || navigator().push(Route::at(tab, View::Place { gmail, target })));
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
	let g = gmail.id;
	rsx! {
		header { class: "flex h-14 shrink-0 items-center gap-3 border-b border-border px-6",
			span { class: "text-ink-soft", "{gmail.gmail}" }
			span { class: "text-ink-soft", "/" }
			span { class: "font-medium", "Locations" }
			div { class: "flex-1" }
			Link { class: "text-ink-soft hover:text-ink", to: Route::at(tab, View::Gmail(g)), "Cards" }
		}
		iframe {
			class: "min-h-0 w-full flex-1 border-0",
			src: "{URL}/d/ra-overview/?kiosk&var-gmail={g}",
			onmounted: move |e| frame.set(Some(e.as_web_event().dyn_into().expect("an iframe element"))),
		}
	}
}

/// `ra-location`'s panels for the place.
#[component]
pub fn Stats(target: i64) -> Element {
	rsx! {
		div { class: "flex h-full flex-col",
			for (panel, grow) in [(3, "flex-[2]"), (1, "flex-1"), (2, "flex-[3]")] {
				iframe { key: "{panel}", class: "w-full min-h-0 border-0 {grow}", src: "{URL}/d-solo/ra-location/?var-target={target}&panelId={panel}" }
			}
		}
	}
}
