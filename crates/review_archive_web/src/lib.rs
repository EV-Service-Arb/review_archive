//! The dashboard: a person's GMBs (the conductor's, under their managing gmails), the place
//! each tracks, and per place a board of its reviews — snapshotted, suspended, removed, reinstating. The Service-Arb panel mounts it and
//! forwards its calls to the archive: the panel's session cookie and CSRF header sign them.
//! One who may act as others gets tabs: their own dashboard, and any member's, acting as them.

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
	uikit_dioxus::{
		Badge, Button, ButtonVariant, CommandDialog, CommandEmpty, CommandInput, CommandItem, CommandList, InfoTip, InfoTipContent, InfoTipTrigger, Size, Tabs, TabsList, TabsTrigger, Tone,
	},
	views::Routed,
};
use review_archive_client::{
	Client,
	dto::{GmailDto, GmailOverview, GmbSummary, LocationSummary, Me, MemberDto, OnGoogle},
};
use review_archive_core::views::{Board, View};
use sa_auth::{Members, Permission, PermissionSet};
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
		let base = format!("/{}", base.trim_matches('/'));
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

/// The API, and whose tab it is for (`x-member`): the caller's own (`None`), or the member their tab acts as.
#[derive(Clone)]
struct Api(Client, Option<i64>);

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
	fn at(member: Option<i64>, board: Board) -> Self {
		Self::At {
			at: Routed(match member {
				Some(member) => View::Theirs { member, view: board },
				None => View::Mine { view: board },
			}),
		}
	}

	fn view(&self) -> &View {
		let Self::At { at } = self;
		&at.0
	}

	/// Whose tab: `None` for the caller's own, and for the pick, which opens one.
	fn member(&self) -> Option<i64> {
		match self.view() {
			View::Theirs { member, .. } => Some(*member),
			View::Mine { .. } | View::Pick => None,
		}
	}

	/// What the tab shows; the pick lays over the caller's home.
	fn board(&self) -> &Board {
		match self.view() {
			View::Theirs { view, .. } | View::Mine { view } => view,
			View::Pick => &Board::Home,
		}
	}
}

/// Each tab's last route, by its member: what a hidden tab keeps showing, and where going
/// back to it lands.
#[derive(Clone, Copy)]
struct Views(Signal<std::collections::HashMap<Option<i64>, Route>>);

/// The route renders nothing itself — every tab stays mounted under [`Shell`] — it only notes
/// where its tab is; the pick is no tab's.
#[component]
fn Page(at: Routed<View>) -> Element {
	let Views(mut views) = use_context();
	let route = Route::At { at };
	use_effect(use_reactive!(|route| {
		if *route.view() != View::Pick {
			views.write().insert(route.member(), route);
		}
	}));
	rsx! {}
}

const TABS: &str = "review_archive.tabs";
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
fn act<F: Future<Output = Result<(), review_archive_client::Error>> + 'static>(f: impl FnOnce(Client, Option<i64>) -> F + 'static) {
	let Api(api, tab) = consume_context();
	let Refresh(mut refresh) = consume_context();
	let Failure(mut failure) = consume_context();
	spawn(async move {
		match f(api, tab).await {
			Ok(()) => failure.set(None),
			Err(e) => failure.set(Some(shown(e))),
		}
		refresh += 1;
	});
}

const SHELL: &str = "flex min-h-0 flex-1 bg-background text-ink text-[13px] font-sans";

/// The GMBs the host's outline lists (`sa:gmbs`, the conductor's ids): what the caller's own tab narrows to.
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
	use_context_provider(|| Views(Signal::new(Default::default())));
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
	let route = use_route::<Route>();
	let me = use_resource(|| async { api().me().await.map_err(shown) });
	let body = match &*me.read() {
		Some(Ok(me)) => rsx! { Signed { me: me.clone(), view: route } },
		Some(Err(e)) => rsx! { div { class: "{SHELL} p-6 text-accent-error", "{e}" } },
		None => rsx! { div { class: "{SHELL} p-6 text-ink-soft", "Loading…" } },
	};
	rsx! {
		div { class: "flex h-full flex-col",
			{body}
			Outlet::<Route> {}
		}
	}
}

#[component]
fn Signed(me: Me, view: Route) -> Element {
	use_context_provider(|| Held(me.permissions.iter().cloned().collect()));
	let body = match may(Members::ActAs) {
		true => rsx! { Admin { me: me.id } },
		false => rsx! { Workspace { view } },
	};
	match ui() {
		Ui::Grafana => rsx! { grafana::Host { me: me.id, {body} } },
		Ui::Legacy => body,
	}
}

/// "You", then a tab per member opened, kept for this browser tab across the full loads the
/// panel's links make; every tab stays mounted, so switching keeps where
/// each one was. The active tab is the URL's `member`.
#[component]
fn Admin(me: i64) -> Element {
	let route = use_route::<Route>();
	let Views(mut views) = use_context();
	let active = route.member();
	let mut opened = use_signal(|| match session().get_item(TABS).expect("sessionStorage is readable") {
		Some(ids) => ids.split(',').filter(|m| !m.is_empty()).map(|m| m.parse::<i64>().expect("written below")).collect(),
		None => Vec::new(),
	});
	use_effect(move || {
		let ids = opened().iter().map(i64::to_string).collect::<Vec<_>>().join(",");
		session().set_item(TABS, &ids).expect("sessionStorage is writable");
	});
	let members = use_resource(|| async { api().list_members().await.map_err(shown) });
	let label = move |m: i64| match &*members.read() {
		Some(Ok(all)) => match all.iter().find(|x| x.id == m) {
			Some(x) if !x.name.is_empty() => x.name.clone(),
			Some(x) => x.email.clone(),
			None => format!("#{m}"),
		},
		Some(Err(e)) => e.clone(),
		None => format!("#{m}"),
	};
	// a link to a member's view opens their tab
	use_effect(use_reactive!(|active| {
		if let Some(m) = active
			&& !opened.peek().contains(&m)
		{
			opened.write().push(m);
		}
	}));
	let mut tabs = opened();
	if let Some(m) = active
		&& !tabs.contains(&m)
	{
		tabs.push(m);
	}
	// where a tab is: the URL for the active one, else where it was left; a tab never visited starts at its home
	let view_of = move |m: Option<i64>| match views.read().get(&m) {
		Some(r) => r.clone(),
		None => Route::at(m, Board::Home),
	};
	let shown = |m: Option<i64>| match active == m {
		true => "flex min-h-0 flex-1",
		false => "hidden",
	};
	rsx! {
		div { class: "flex min-h-0 flex-1 flex-col bg-background text-ink text-[13px] font-sans",
			if !tabs.is_empty() {
			Tabs {
				// the kit's tabs are keyed by string: "" is the caller's own
				value: active.map(|m| m.to_string()).unwrap_or_default(),
				on_value_change: move |m: String| {
					navigator().push(view_of((!m.is_empty()).then(|| m.parse().expect("a tab's value is its member's id"))));
				},
				class: "border-b border-border bg-secondary px-2 py-1.5",
				div { class: "flex items-center gap-1",
					TabsList { class: "bg-transparent",
						TabsTrigger { value: "", "You" }
						for m in tabs.clone() {
							div { key: "{m}", class: "flex items-center",
								TabsTrigger { value: m.to_string(), {label(m)} }
								Button {
									variant: ButtonVariant::Ghost,
									size: Size::Xs,
									icon: true,
									r#type: "button",
									onclick: move |_| {
										opened.write().retain(|t| *t != m);
										views.write().remove(&Some(m));
										if router().current::<Route>().member() == Some(m) {
											navigator().push(view_of(None));
										}
									},
									"×"
								}
							}
						}
					}
				}
			}
			}
			div { class: shown(None),
				Workspace { view: if matches!(route.view(), View::Mine { .. }) { route.clone() } else { view_of(None) } }
			}
			for m in tabs {
				div { key: "{m}", class: shown(Some(m)),
					Workspace { view: if active == Some(m) { route.clone() } else { view_of(Some(m)) } }
				}
			}
			if *route.view() == View::Pick {
				Picker {
					on_pick: move |m: MemberDto| {
						navigator().replace(view_of((m.id != me).then_some(m.id)));
					},
					on_close: move |_| {
						navigator().replace(view_of(None));
					},
				}
			}
		}
	}
}

/// fzf over everyone who signed in here: name and email, and what they hold.
#[component]
fn Picker(on_pick: EventHandler<MemberDto>, on_close: EventHandler<()>) -> Element {
	let tr: Translator = use_context();
	let members = use_resource(|| async { api().list_members().await.map_err(shown) });
	rsx! {
		CommandDialog {
			open: true,
			on_open_change: move |open: bool| {
				if !open {
					on_close.call(());
				}
			},
			CommandInput { placeholder: "Member's name or email" }
			CommandList {
				match &*members.read() {
					Some(Ok(members)) => rsx! {
						for m in members.clone() {
							CommandItem {
								key: "{m.id}",
								value: format!("{} {}", m.name, m.email),
								on_select: {
									let m = m.clone();
									move |_| on_pick.call(m.clone())
								},
								if !m.claimed {
									Badge { {t!(tr, "picker.unclaimed", "never signed in")} }
								}
								span { "{m.name}" }
								span { class: "flex-1 truncate text-ink-soft", "{m.email}" }
								span { class: "w-20 shrink-0 text-right font-mono text-ink-soft", "{m.balance} tokens" }
							}
						}
						CommandEmpty { "No member matches." }
					},
					Some(Err(e)) => rsx! { div { class: "p-3 text-accent-error", "{e}" } },
					None => rsx! { div { class: "p-3 text-ink-soft", "Loading…" } },
				}
			}
		}
	}
}

/// One tab's dashboard at `view`: the signed-in person's own, or (its `member`) the one they
/// act as.
#[component]
fn Workspace(view: Route) -> Element {
	let tr: Translator = use_context();
	let tab = view.member();
	let Api(client, _) = use_context_provider(|| Api(api(), tab));
	let Selection(selection) = use_context();
	let refresh = use_context_provider(|| Refresh(Signal::new(0)));
	let mut filter = use_context_provider(|| Signal::new(filter::Filter::default()));
	use_context_provider(|| Signal::new(filter::reviews::Reviews::default()));
	let failure = use_context_provider(|| Failure(Signal::new(None)));
	let overview = use_resource(move || {
		let client = client.clone();
		async move {
			refresh.0();
			client.my_overview(tab).await.map_err(shown)
		}
	});
	// the own tab's filter row selects for the host: what it narrows (`base`: the selection as it was when it began),
	// and what it asked for that the host has not echoed yet
	let mut base = use_signal(|| None::<Picked>);
	let mut asked = use_signal(Vec::<Picked>::new);
	use_effect(move || {
		let now = selection();
		if tab.is_some() || now == Picked::Unknown {
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
		if tab.is_some() || *selection.peek() == Picked::Unknown {
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
	// `/me` is the caller whoever they act as: who a member tab is is read off the members list
	let unknown = tr.clone();
	let whom = use_resource(move || {
		let unknown = unknown.clone();
		async move {
			let Some(m) = tab else { return Ok(None) };
			let found = api()
				.list_members()
				.await
				.map_err(shown)?
				.into_iter()
				.find(|x| x.id == m)
				.ok_or_else(|| t!(unknown, "workspace.no_member", "There is no member #{member}", member = m))?;
			Ok::<_, String>(Some(found))
		}
	});

	let shell = SHELL;
	let gmails: Vec<GmailOverview> = match &*overview.read() {
		Some(Ok(g)) => g.clone(),
		Some(Err(e)) => return rsx! { div { class: "{shell} p-6 text-accent-error", "{e}" } },
		_ => return rsx! { div { class: "{shell} p-6 text-ink-soft", "Loading…" } },
	};
	let whom = match &*whom.read() {
		Some(Ok(whom)) => whom.clone(),
		Some(Err(e)) => return rsx! { div { class: "{shell} p-6 text-accent-error", "{e}" } },
		None => None,
	};
	let at = view.board().clone();
	let gmb_of = |id: i64| gmails.iter().find_map(|g| g.gmbs.iter().find(|b| b.id == id).map(|b| (g.gmail.clone(), b.clone())));
	let picked = match at {
		Board::Home | Board::Settings => None,
		Board::Gmail { gmail } => Some(gmail),
		Board::Gmb { gmb } => gmb_of(gmb).map(|(g, _)| g.id),
	};
	// a removed gmail's URL shows them all, as home does
	let current = picked.filter(|id| gmails.iter().any(|g| g.gmail.id == *id));
	// the host's outline is the caller's GMBs: a member's tab is narrowed by its own rail
	let listed = match (tab, base()) {
		(Some(_), _) => Picked::All,
		(None, Some(base)) => base,
		(None, None) => selection(),
	};
	let acting = tab.map(|m| {
		let who = match whom {
			Some(w) if !w.name.is_empty() => format!("{} <{}>", w.name, w.email),
			Some(w) => w.email,
			None => format!("#{m}"),
		};
		t!(tr, "workspace.acting", "Viewing as {member} — actions apply to their account", member = who)
	});

	rsx! {
		div { class: "{shell}",
			// the person's own gmails are in the conductor's sidebar
			if tab.is_some() {
				Rail { gmails: gmails.clone(), current, at: at.clone(), tab }
			}
			div { class: "flex min-h-0 min-w-0 flex-1 flex-col",
				if let Some(acting) = acting {
					div { class: "border-b border-border bg-accent-warn/15 px-6 py-2 text-accent-warn", "{acting}" }
				}
				if let Some(e) = failure.0() {
					div { class: "border-b border-border bg-accent-error/15 px-6 py-2 text-accent-error", "{e}" }
				}
				match at {
					Board::Settings => rsx! {
						settings::Page {}
					},
					Board::Gmb { gmb } => match gmb_of(gmb) {
						Some((gmail, b)) => rsx! { board::Board { gmb: Gmb::from((&gmail, &b)), location: b.track, tab } },
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
						rsx! { Places { cards, gmail, tab } }
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

#[component]
fn Rail(gmails: Vec<GmailOverview>, current: Option<i64>, at: Board, tab: Option<i64>) -> Element {
	let aside = at == Board::Settings;
	let row = "flex items-center gap-2 rounded-md px-3 py-2 text-left cursor-pointer hover:bg-hover";
	rsx! {
		nav { class: "flex w-60 shrink-0 flex-col overflow-y-auto border-r border-border bg-secondary",
			div { class: "flex items-center gap-2 px-4 py-4",
				span { class: "flex-1 text-[14px] font-semibold", "review_archive" }
			}
			div { class: "flex items-center gap-1.5 pl-2 pr-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-ink-soft",
				Link {
					class: if at == Board::Home { "rounded-md px-2 py-0.5 bg-hover text-ink" } else { "rounded-md px-2 py-0.5 hover:bg-hover" },
					to: Route::at(tab, Board::Home),
					"Managing gmails"
				}
				InfoTip {
					InfoTipTrigger { label: "What a managing gmail is" }
					InfoTipContent { class: "normal-case font-normal tracking-normal flex flex-col gap-2",
						p { "The Google account a group of GMBs is managed from. Gmails and their GMBs are added on the GMBs page." }
					}
				}
				Link {
					class: if at == Board::Settings { "rounded-md p-0.5 bg-hover text-ink" } else { "rounded-md p-0.5 hover:bg-hover" },
					to: Route::at(tab, Board::Settings),
					title: "Settings",
					// lucide `settings`
					svg {
						class: "size-3.5",
						view_box: "0 0 24 24",
						fill: "none",
						stroke: "currentColor",
						stroke_width: "2",
						stroke_linecap: "round",
						stroke_linejoin: "round",
						"aria-hidden": "true",
						path { d: "M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" }
						circle { cx: "12", cy: "12", r: "3" }
					}
				}
			}
			div { class: "flex flex-col gap-0.5 px-2",
				for g in gmails {
					Link {
						key: "{g.gmail.id}",
						class: format!("{row} min-w-0 {}", if current == Some(g.gmail.id) && !aside { "bg-hover text-ink" } else { "text-ink-soft" }),
						to: Route::at(tab, Board::Gmail { gmail: g.gmail.id }),
						span { class: "truncate flex-1", "{g.gmail.gmail}" }
						Badge { "{g.gmbs.len()}" }
					}
				}
			}
			div { class: "mt-2 px-5", AddGmb {} }
		}
	}
}

/// Tracked places as cards, as given; `gmail`: the one gmail they are all under, if one. A place tracked
/// under two GMBs is a card under each: its counts are that GMB's.
#[component]
fn Places(cards: Vec<(Gmb, LocationSummary)>, gmail: Option<GmailDto>, tab: Option<i64>) -> Element {
	let filter: Signal<filter::Filter> = use_context();
	let total = cards.len();
	let shown = filter.read().keep(cards);
	let kept: Vec<LocationSummary> = shown.iter().map(|(_, l)| l.clone()).collect();
	let overview = match ui() {
		Ui::Grafana => rsx! {
			grafana::Overview {
				gmail: gmail.as_ref().map(|g| g.id),
				keys: shown.iter().map(|(g, l)| format!("{}:{}", g.id, l.target.id)).collect::<Vec<_>>().join(","),
				tab,
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
							to: Route::at(tab, Board::Gmb { gmb: g.id }),
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
