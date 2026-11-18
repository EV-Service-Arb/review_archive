//! The dashboard: a person's GMBs (the conductor's, under their managing gmails), the place
//! each tracks, and per place a board of its reviews — snapshotted, suspended, removed, reinstating. The Service-Arb panel mounts it and
//! forwards its calls to the archive: the panel's session cookie and CSRF header sign them.

#![cfg(target_arch = "wasm32")]
#![allow(clippy::useless_format)] // rsx! lowers every "{x}" to a format!

mod board;
mod cards;
mod filter;
mod grafana;
mod settings;

use std::sync::OnceLock;

use dioxus::prelude::*;
use ev_lib::{
	i18n::{Locale, Messages, Translator},
	t,
	uikit_dioxus::{Badge, Tone},
	views::Routed,
};
use review_archive_client::{
	Client,
	dto::{GmailDto, GmailOverview, GmbSummary, LocationSummary, Me, OnGoogle},
};
use review_archive_core::views::{Board, View};
use sa_auth::{Permission, PermissionSet};
use wasm_bindgen::{JsCast, prelude::*};

const TAG: &str = "mfe-review-archive-dashboard";

/// English only, for now: each key's English is written where it renders.
fn catalogue(_: Locale) -> Messages {
	Messages::new()
}

/// The element's attributes.
struct Host {
	/// The API's base URL, resolved against the page.
	api: String,
	/// Where a browser signs in; it comes back to the same-origin path in `return_to`.
	sign_in: String,
	/// The cookie whose value every request carries as `x-sa-csrf`.
	csrf_cookie: String,
	/// The path the page is served at; the element's views are paths under it.
	base: String,
	/// The host's Grafana base, resolved against the page; required by [`Ui::Grafana`] only.
	grafana: Option<String>,
	/// The tab's `?grafana=` choice, else the host's `ui` attribute.
	ui: Ui,
}

/// The entry module's URL: every file of the bundle sits next to it.
static BUNDLE: OnceLock<String> = OnceLock::new();
static MOUNT: OnceLock<Mount> = OnceLock::new();

/// What the page gives the element when it mounts.
struct Mount {
	locale: Locale,
	/// The element's attributes, or the names of those it left out.
	host: Result<Host, Vec<&'static str>>,
}

/// Which dashboard the page shows, for its whole life: the two mount different hooks and subtrees.
#[derive(Clone, Copy, PartialEq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
enum Ui {
	Legacy,
	/// Grafana draws the overview and a place's board: [`grafana`].
	Grafana,
}

/// Called by the entry module with its own URL; registers the element.
#[wasm_bindgen]
pub fn define(bundle: String) {
	BUNDLE.set(bundle).expect("the entry module runs once");
	let mount = Closure::<dyn Fn(web_sys::Element)>::new(mount);
	ev_lib::mfe::register(TAG, &mount);
	mount.forget(); // the element can mount at any time after registration
}

fn mount(el: web_sys::Element) {
	let page = web_sys::window().expect("a browser").location().href().expect("a page has a URL");
	let asked: Vec<String> = reqwest::Url::parse(&page)
		.expect("a page's URL is a URL")
		.query_pairs()
		.filter(|(k, _)| k == "grafana")
		.map(|(_, v)| v.into_owned())
		.collect();
	// the router drops the query, so a tab keeps what its link asked for across navigation and reloads
	let chosen = match asked.as_slice() {
		[] => session().get_item(UI).expect("sessionStorage is readable").map(|ui| ui.parse().expect("written below")),
		[v] => {
			let ui = match v.as_str() {
				"true" => Ui::Grafana,
				"false" => Ui::Legacy,
				v => panic!("?grafana={v}: a link asks for `true` or `false`"),
			};
			session().set_item(UI, &ui.to_string()).expect("sessionStorage is writable");
			Some(ui)
		}
		many => panic!("?grafana given {} times: {many:?}", many.len()),
	};
	let attr = |name| el.get_attribute(name);
	let default = attr("ui").map(|ui| ui.parse::<Ui>().unwrap_or_else(|_| panic!("ui=\"{ui}\": a host asks for `grafana` or `legacy`")));
	let ui = chosen.or(default);
	let required = match ui {
		Some(Ui::Grafana) => &["api", "sign-in", "csrf-cookie", "base", "ui", "grafana"][..],
		_ => &["api", "sign-in", "csrf-cookie", "base", "ui"],
	};
	let host = match (attr("api"), attr("sign-in"), attr("csrf-cookie"), attr("base"), default, ui, attr("grafana")) {
		(Some(api), Some(sign_in), Some(csrf_cookie), Some(base), Some(_), Some(ui), grafana) if grafana.is_some() || ui == Ui::Legacy => Ok(Host {
			api,
			sign_in,
			csrf_cookie,
			base,
			grafana,
			ui,
		}),
		_ => Err(required.iter().copied().filter(|a| !el.has_attribute(a)).collect()),
	};
	let mut cfg = dioxus::web::Config::new().rootelement(el.clone());
	if let Ok(host) = &host {
		cfg = cfg.history(std::rc::Rc::new(Based::new(&host.base)));
	}
	let locale = ev_lib::mfe::host_locale(&el);
	assert!(MOUNT.set(Mount { locale, host }).is_ok(), "one dashboard per page");
	dioxus::LaunchBuilder::new().with_cfg(cfg).launch(root);
}

/// The page's history under `base`. The host keeps the element running on its other pages too,
/// where the location is none of its routes: the router stays at the last route under `base`.
struct Based {
	web: dioxus::web::WebHistory,
	base: String,
	last: std::cell::RefCell<String>,
}

impl Based {
	fn new(base: &str) -> Self {
		let web = dioxus::web::WebHistory::new(Some(base.to_owned()), true);
		// a page at the origin's root (`--dev-member`'s) has the empty base: every path is under it
		let base = match base.trim_matches('/') {
			"" => String::new(),
			b => format!("/{b}"),
		};
		let based = Self {
			last: std::cell::RefCell::new(String::new()),
			web,
			base,
		};
		assert!(based.here(), "the element mounts on its own page, under {}", based.base);
		based.last.replace(dioxus::history::History::current_route(&based.web));
		based
	}

	fn here(&self) -> bool {
		let path = web_sys::window().expect("a browser").location().pathname().expect("a page has a path");
		path.strip_prefix(&self.base).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
	}
}

impl dioxus::history::History for Based {
	fn current_route(&self) -> String {
		if self.here() {
			let route = self.web.current_route();
			// at the bare base with a query (`/review_archive?grafana=true`) the route is the query alone
			self.last.replace(if route.starts_with('/') { route } else { format!("/{route}") });
		}
		self.last.borrow().clone()
	}

	fn current_prefix(&self) -> Option<String> {
		self.web.current_prefix()
	}

	fn can_go_back(&self) -> bool {
		self.web.can_go_back()
	}

	fn go_back(&self) {
		self.web.go_back()
	}

	fn can_go_forward(&self) -> bool {
		self.web.can_go_forward()
	}

	fn go_forward(&self) {
		self.web.go_forward()
	}

	fn push(&self, route: String) {
		self.web.push(route)
	}

	fn replace(&self, path: String) {
		self.web.replace(path)
	}

	fn external(&self, url: String) -> bool {
		self.web.external(url)
	}

	fn updater(&self, callback: std::sync::Arc<dyn Fn() + Send + Sync>) {
		self.web.updater(callback)
	}

	fn include_prevent_default(&self) -> bool {
		self.web.include_prevent_default()
	}
}

fn root() -> Element {
	let Mount { locale, host, .. } = MOUNT.get().expect("set before launch");
	let tr = use_context_provider(|| Translator::new(catalogue(*locale), *locale));
	let bundle = BUNDLE.get().expect("defined before any mount");
	let stylesheet = reqwest::Url::parse(bundle).and_then(|b| b.join("mfe.css")).expect("the entry module's URL is a URL");
	rsx! {
		document::Stylesheet { href: stylesheet.to_string() }
		match host {
			Ok(_) => rsx! { Router::<Route> {} },
			Err(missing) => rsx! {
				div { class: "p-6 text-accent-error text-[13px] font-sans",
					{t!(tr, "host.missing", "<{tag}> is missing its attributes: {missing}", tag = TAG, missing = missing.join(", "))}
				}
			},
		}
	}
}

fn host() -> &'static Host {
	MOUNT
		.get()
		.and_then(|m| m.host.as_ref().ok())
		.expect("only the router asks, and it renders once the attributes are read")
}

fn ui() -> Ui {
	host().ui
}

#[derive(Clone)]
struct Api(Client);

/// Bumped after a write, so what was read is read again.
#[derive(Clone, Copy)]
struct Refresh(Signal<u32>);

/// The last action that failed, shown until the next one.
#[derive(Clone, Copy)]
struct Failure(Signal<Option<String>>);

/// What the signed-in person may do here.
#[derive(Clone)]
struct Held(PermissionSet);

fn may(p: impl Permission) -> bool {
	let Held(held) = consume_context();
	held.may(p)
}

/// A GMB, with the gmail it is under.
#[derive(Clone, Debug, PartialEq)]
struct Gmb {
	id: i64,
	gmail: GmailDto,
	name: String,
	/// The conductor's switch.
	enabled: bool,
	state: OnGoogle,
}

impl From<(&GmailDto, &GmbSummary)> for Gmb {
	fn from((gmail, b): (&GmailDto, &GmbSummary)) -> Self {
		Self {
			id: b.id,
			gmail: gmail.clone(),
			name: b.name.clone(),
			enabled: b.enabled,
			state: b.state,
		}
	}
}

/// Where the dashboard is: one route, the views' (`review_archive_core::views`).
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
enum Route {
	#[layout(Shell)]
		#[route("/:..at", Page)]
		At { at: Routed<View> },
}

impl Route {
	fn at(board: Board) -> Self {
		Self::At {
			at: Routed(View::Mine { view: board }),
		}
	}
}

#[component]
fn Page(at: Routed<View>) -> Element {
	let View::Mine { view } = at.0;
	rsx! { Workspace { at: view } }
}

const UI: &str = "review_archive.ui";

fn session() -> web_sys::Storage {
	web_sys::window()
		.expect("a browser")
		.session_storage()
		.expect("sessionStorage is readable")
		.expect("a page has sessionStorage")
}

/// The API's base URL: `api` resolved against the page.
fn base() -> reqwest::Url {
	directory(&host().api)
}

/// An attribute's URL resolved against the page, as a directory: the panel's `/api/review_archive`
/// names one, and `join` would replace its last segment.
fn directory(attr: &str) -> reqwest::Url {
	let page = web_sys::window().expect("a browser").location().href().expect("a page has a URL");
	let mut dir = reqwest::Url::parse(&page).and_then(|p| p.join(attr)).expect("the attribute resolves against the page");
	if !dir.path().ends_with('/') {
		let path = format!("{}/", dir.path());
		dir.set_path(&path);
	}
	dir
}

fn api() -> Client {
	let window = web_sys::window().expect("a browser");
	let cookies = window
		.document()
		.expect("a page")
		.dyn_into::<web_sys::HtmlDocument>()
		.expect("an HTML page")
		.cookie()
		.expect("the page's cookies are readable");
	let mut headers = reqwest::header::HeaderMap::new();
	// absent, the panel refuses a write with its own answer
	if let Some(token) = cookies.split("; ").find_map(|c| c.strip_prefix(host().csrf_cookie.as_str())?.strip_prefix('=')) {
		headers.insert("x-sa-csrf", reqwest::header::HeaderValue::from_str(token).expect("a cookie value is a valid header value"));
	}
	let http = reqwest::Client::builder()
		.default_headers(headers)
		.build()
		.expect("a browser client takes no setup that can fail");
	Client::ambient(http, base().as_str()).expect("resolved to an absolute URL")
}

/// A failed call, as shown. A 401 means the panel's session is gone: the whole page goes to
/// the element's `sign-in`, which brings the browser back here signed in.
fn shown(e: review_archive_client::Error) -> String {
	if e.status().map(|s| s.as_u16()) == Some(401) {
		let location = web_sys::window().expect("a browser").location();
		let here = format!(
			"{}{}{}",
			location.pathname().expect("a page has a path"),
			location.search().expect("a page has a query"),
			location.hash().expect("a page has a fragment")
		);
		let to = format!("{}?return_to={}", host().sign_in, String::from(js_sys::encode_uri_component(&here)));
		location.set_href(&to).expect("navigating the page");
	}
	e.to_string()
}

/// Runs a write against the API, then reads everything again; a failure is shown.
fn act<F: Future<Output = Result<(), review_archive_client::Error>> + 'static>(f: impl FnOnce(Client) -> F + 'static) {
	let Api(api) = consume_context();
	let Refresh(mut refresh) = consume_context();
	let Failure(mut failure) = consume_context();
	spawn(async move {
		match f(api).await {
			Ok(()) => failure.set(None),
			Err(e) => failure.set(Some(shown(e))),
		}
		refresh += 1;
	});
}

const SHELL: &str = "flex min-h-0 flex-1 bg-background text-ink text-[13px] font-sans";

/// The GMBs the host's outline lists (`sa:gmbs`, the conductor's ids): what the dashboard narrows to.
#[derive(Clone, PartialEq)]
enum Picked {
	/// no `sa:gmbs` yet
	Unknown,
	All,
	Only(std::collections::HashSet<i64>),
}

impl Picked {
	fn holds(&self, gmb: i64) -> bool {
		match self {
			Self::All => true,
			Self::Only(ids) => ids.contains(&gmb),
			Self::Unknown => unreachable!("nothing is listed before the host's selection is known"),
		}
	}
}

#[derive(Clone, Copy)]
struct Selection(Signal<Picked>);

/// Asks the host to make `picked` its selection; it answers with `sa:gmbs`.
fn select(picked: &Picked) {
	let ids = match picked {
		Picked::All => JsValue::NULL,
		Picked::Only(ids) => {
			let mut ids: Vec<_> = ids.iter().copied().collect();
			ids.sort_unstable();
			ids.into_iter().map(|id| JsValue::from(id as f64)).collect::<js_sys::Array>().into()
		}
		Picked::Unknown => unreachable!("only a known selection is narrowed"),
	};
	let detail = js_sys::Object::new();
	js_sys::Reflect::set(&detail, &"ids".into(), &ids).expect("setting a field");
	let init = web_sys::CustomEventInit::new();
	init.set_detail(&detail);
	let ev = web_sys::CustomEvent::new_with_event_init_dict("sa:gmbs-select", &init).expect("a plain event");
	web_sys::window().expect("a browser").dispatch_event(&ev).expect("dispatching to a window");
}

#[component]
fn Shell() -> Element {
	let Selection(mut picked) = use_context_provider(|| Selection(Signal::new(Picked::Unknown)));
	let listener = use_hook(|| {
		let (rt, shell) = (dioxus::core::Runtime::current(), dioxus::core::current_scope_id());
		let on_gmbs = Closure::<dyn FnMut(web_sys::CustomEvent)>::new(move |e: web_sys::CustomEvent| {
			let ids = js_sys::Reflect::get(&e.detail(), &"ids".into()).unwrap_or_else(|_| panic!("sa:gmbs without a detail object: {:?}", e.detail()));
			let next = match ids.is_null() {
				true => Picked::All,
				false => Picked::Only(
					ids.dyn_into::<js_sys::Array>()
						.unwrap_or_else(|ids| panic!("sa:gmbs ids is neither null nor an array: {ids:?}"))
						.iter()
						.map(|id| match id.as_f64() {
							Some(n) if n.fract() == 0. => n as i64,
							_ => panic!("sa:gmbs ids holds a non-id: {id:?}"),
						})
						.collect(),
				),
			};
			rt.in_scope(shell, || {
				if *picked.peek() != next {
					picked.set(next);
				}
			});
		});
		let window = web_sys::window().expect("a browser");
		window.add_event_listener_with_callback("sa:gmbs", on_gmbs.as_ref().unchecked_ref()).expect("adding a listener");
		// the host may have told the page before this listened
		window
			.dispatch_event(&web_sys::CustomEvent::new("sa:gmbs-request").expect("a plain event"))
			.expect("dispatching to a window");
		std::rc::Rc::new(on_gmbs)
	});
	use_drop(move || {
		web_sys::window()
			.expect("a browser")
			.remove_event_listener_with_callback("sa:gmbs", listener.as_ref().as_ref().unchecked_ref())
			.expect("removing a listener");
	});
	let me = use_resource(|| async { api().me().await.map_err(shown) });
	let body = match &*me.read() {
		Some(Ok(me)) => rsx! {
			Signed { me: me.clone(),
				Outlet::<Route> {}
			}
		},
		Some(Err(e)) => rsx! { div { class: "{SHELL} p-6 text-accent-error", "{e}" } },
		None => rsx! { div { class: "{SHELL} p-6 text-ink-soft", "Loading…" } },
	};
	rsx! {
		div { class: "flex h-full flex-col",
			{body}
		}
	}
}

#[component]
fn Signed(me: Me, children: Element) -> Element {
	use_context_provider(|| Held(me.permissions.iter().cloned().collect()));
	match ui() {
		Ui::Grafana => rsx! { grafana::Host { {children} } },
		Ui::Legacy => children,
	}
}

/// The signed-in person's dashboard at `at`.
#[component]
fn Workspace(at: Board) -> Element {
	let Api(client) = use_context_provider(|| Api(api()));
	let Selection(selection) = use_context();
	let refresh = use_context_provider(|| Refresh(Signal::new(0)));
	let mut filter = use_context_provider(|| Signal::new(filter::Filter::default()));
	use_context_provider(|| Signal::new(filter::reviews::Reviews::default()));
	let failure = use_context_provider(|| Failure(Signal::new(None)));
	let overview = use_resource(move || {
		let client = client.clone();
		async move {
			refresh.0();
			client.my_overview().await.map_err(shown)
		}
	});
	// the filter row selects for the host: what it narrows (`base`: the selection as it was when it began),
	// and what it asked for that the host has not echoed yet
	let mut base = use_signal(|| None::<Picked>);
	let mut asked = use_signal(Vec::<Picked>::new);
	use_effect(move || {
		let now = selection();
		if now == Picked::Unknown {
			return;
		}
		let mut pending = asked.write();
		match pending.iter().position(|p| *p == now) {
			Some(i) => drop(pending.drain(..=i)),
			// someone else's selection: the row no longer describes it
			None => {
				pending.clear();
				base.set(None);
				if *filter.peek() != filter::Filter::default() {
					filter.set(filter::Filter::default());
				}
			}
		}
	});
	use_effect(move || {
		let narrowing = filter.read().clone();
		let Some(Ok(gmails)) = overview.read().clone() else { return };
		if *selection.peek() == Picked::Unknown {
			return;
		}
		let ask = match narrowing == filter::Filter::default() {
			true => match base.write().take() {
				Some(was) => was,
				None => return,
			},
			false => {
				let from = base.write().get_or_insert_with(|| selection.peek().clone()).clone();
				let cards = gmails
					.iter()
					.flat_map(|g| g.gmbs.iter().map(move |b| (g, b)))
					.filter(|(_, b)| from.holds(b.id))
					.map(|(g, b)| (Gmb::from((&g.gmail, b)), b.track.clone()))
					.collect();
				Picked::Only(narrowing.keep(cards).into_iter().map(|(g, _)| g.id).collect())
			}
		};
		if ask != *selection.peek() && asked.peek().last() != Some(&ask) {
			asked.write().push(ask.clone());
			select(&ask);
		}
	});
	// while a scan is queued or walking, its card is read again
	use_future(move || async move {
		let Refresh(mut refresh) = refresh;
		loop {
			let ms = 2000;
			wasm_bindgen_futures::JsFuture::from(js_sys::Promise::new(&mut |done, _| {
				web_sys::window()
					.expect("runs in a page")
					.set_timeout_with_callback_and_timeout_and_arguments_0(&done, ms)
					.expect("setTimeout takes a function");
			}))
			.await
			.expect("a timer never rejects");
			if let Some(Ok(gmails)) = &*overview.peek()
				&& gmails.iter().flat_map(|g| &g.gmbs).any(|b| b.track.scan.is_some())
			{
				refresh += 1;
			}
		}
	});
	let shell = SHELL;
	let gmails: Vec<GmailOverview> = match &*overview.read() {
		Some(Ok(g)) => g.clone(),
		Some(Err(e)) => return rsx! { div { class: "{shell} p-6 text-accent-error", "{e}" } },
		_ => return rsx! { div { class: "{shell} p-6 text-ink-soft", "Loading…" } },
	};
	let gmb_of = |id: i64| gmails.iter().find_map(|g| g.gmbs.iter().find(|b| b.id == id).map(|b| (g.gmail.clone(), b.clone())));
	let picked = match at {
		Board::Home | Board::Settings => None,
		Board::Gmail { gmail } => Some(gmail),
		Board::Gmb { gmb } => gmb_of(gmb).map(|(g, _)| g.id),
	};
	// a removed gmail's URL shows them all, as home does
	let current = picked.filter(|id| gmails.iter().any(|g| g.gmail.id == *id));
	let listed = match base() {
		Some(base) => base,
		None => selection(),
	};
	rsx! {
		div { class: "{shell}",
			div { class: "flex min-h-0 min-w-0 flex-1 flex-col",
				if let Some(e) = failure.0() {
					div { class: "border-b border-border bg-accent-error/15 px-6 py-2 text-accent-error", "{e}" }
				}
				match at {
					Board::Settings => rsx! {
						settings::Page {}
					},
					Board::Gmb { gmb } => match gmb_of(gmb) {
						Some((gmail, b)) => rsx! { board::Board { gmb: Gmb::from((&gmail, &b)), location: b.track } },
						None => rsx! { div { class: "p-6 text-ink-soft", "No GMB #{gmb} of yours: it may have been removed." } },
					},
					_ if gmails.is_empty() => rsx! {
						div { class: "p-6 text-ink-soft",
							"No GMBs yet. "
							AddGmb {}
						}
					},
					_ if listed == Picked::Unknown => rsx! { div { class: "p-6 text-ink-soft", "Waiting for the outline's selection…" } },
					_ => {
						let mut cards = Vec::new();
						let mut under = Vec::new();
						for g in gmails.iter().filter(|g| current.is_none_or(|c| c == g.gmail.id)) {
							let gmbs: Vec<_> = g.gmbs.iter().filter(|b| listed.holds(b.id)).collect();
							if !gmbs.is_empty() {
								under.push(g.gmail.clone());
							}
							cards.extend(gmbs.into_iter().map(|b| (Gmb::from((&g.gmail, b)), b.track.clone())));
						}
						cards.sort_by_key(|(_, l)| (std::cmp::Reverse(l.snapshots_7d), l.target.id));
						let gmail = match <[GmailDto; 1]>::try_from(under) {
							Ok([one]) => Some(one),
							Err(_) => None,
						};
						rsx! { Places { cards, gmail } }
					}
				}
			}
		}
	}
}

/// A link to the conductor's home, where GMBs are added: the shell routes it (`ev:navigate`),
/// else the browser follows it.
#[component]
fn AddGmb() -> Element {
	const HOME: &str = "/";
	rsx! {
		a {
			class: "text-ink underline-offset-2 hover:underline",
			href: HOME,
			onclick: move |e: MouseEvent| {
				let init = web_sys::CustomEventInit::new();
				init.set_cancelable(true);
				let detail = js_sys::Object::new();
				js_sys::Reflect::set(&detail, &"href".into(), &HOME.into()).expect("setting a field");
				init.set_detail(&detail);
				let ev = web_sys::CustomEvent::new_with_event_init_dict("ev:navigate", &init).expect("a plain event");
				// `false`: the shell took it
				if !web_sys::window().expect("a browser").dispatch_event(&ev).expect("dispatching to a window") {
					e.prevent_default();
				}
			},
			"+ Add a GMB"
		}
	}
}

/// Tracked places as cards, as given; `gmail`: the one gmail they are all under, if one. A place tracked
/// under two GMBs is a card under each: its counts are that GMB's.
#[component]
fn Places(cards: Vec<(Gmb, LocationSummary)>, gmail: Option<GmailDto>) -> Element {
	let filter: Signal<filter::Filter> = use_context();
	let total = cards.len();
	let shown = filter.read().keep(cards);
	let kept: Vec<LocationSummary> = shown.iter().map(|(_, l)| l.clone()).collect();
	let overview = match ui() {
		Ui::Grafana => rsx! {
			grafana::Overview {
				gmail: gmail.as_ref().map(|g| g.id),
				keys: shown.iter().map(|(g, l)| format!("{}:{}", g.id, l.target.id)).collect::<Vec<_>>().join(","),
			}
		},
		Ui::Legacy => rsx! {
			main { class: "min-h-0 flex-1 overflow-y-auto p-6",
				if total == 0 {
					div { class: "text-ink-soft", "No places tracked here." }
				} else if shown.is_empty() {
					div { class: "text-ink-soft", "No place matches the filters." }
				}
				div { class: "grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] gap-4",
					for (g, loc) in shown {
						cards::LocationCard {
							key: "{g.id}-{loc.target.id}",
							to: Route::at(Board::Gmb { gmb: g.id }),
							all: gmail.is_none(),
							gmb: g,
							loc,
						}
					}
				}
			}
		},
	};
	rsx! {
		filter::Bar { kept, total }
		{overview}
	}
}

/// "3h ago" for a stored timestamp.
fn ago(ts: &str) -> String {
	let Ok(t) = ts.parse::<jiff::Timestamp>() else { return ts.to_owned() };
	let secs = (js_sys::Date::now() as i64 / 1000 - t.as_second()).max(0);
	match secs {
		s if s < 90 => "just now".into(),
		s if s < 90 * 60 => format!("{}m ago", s / 60),
		s if s < 36 * 3600 => format!("{}h ago", s / 3600),
		s => format!("{}d ago", s / 86_400),
	}
}
