//! The dashboard: a person's managing gmails, the places each tracks, and per place a board
//! of its reviews — snapshotted, removed, reinstating. The Service-Arb panel mounts it and
//! forwards its calls to the archive: the panel's session cookie and CSRF header sign them.
//! One who may act as others gets tabs: their own dashboard, and any member's, acting as them.

#![cfg(target_arch = "wasm32")]
#![allow(clippy::useless_format)] // rsx! lowers every "{x}" to a format!

mod board;
#[cfg(not(feature = "grafana"))]
mod cards;
mod filter;
#[cfg(feature = "grafana")]
mod grafana;
mod telegram;
mod tokens;

use std::sync::OnceLock;

use dioxus::prelude::*;
use ev_lib::{
	i18n::{Locale, Messages, Translator},
	t,
	uikit::{
		self, BadgeVariant, Button, ButtonVariant, CommandDialog, CommandEmpty, CommandInput, CommandItem, CommandList, InfoTip, InfoTipContent, InfoTipTrigger, Input, Select,
		SelectContent, SelectItem, SelectTrigger, SelectValue, Size, Spinner, Switch, Tabs, TabsList, TabsTrigger,
	},
};
use review_archive_client::{
	Client,
	dto::{BalanceChange, GmailDto, GmailOverview, IntervalChange, LocationSummary, Me, MemberDto, NewTrack, PlaceDto, TargetDto, TokensChange, TokensDto},
};
use sa_auth::{Members, Permission, PermissionSet, Tokens};
use wasm_bindgen::{JsCast, prelude::*};

const TAG: &str = "mfe-review-archive-dashboard";

/// English only, for now: each key's English is written where it renders.
fn catalogue(_: Locale) -> Messages {
	Messages::new()
}

/// The element's attributes, all required.
struct Host {
	/// The API's base URL, resolved against the page.
	api: String,
	/// Where a browser signs in; it comes back to the same-origin path in `return_to`.
	sign_in: String,
	/// The cookie whose value every request carries as `x-sa-csrf`.
	csrf_cookie: String,
	/// The path the page is served at; the element's views are paths under it.
	base: String,
}

/// The entry module's URL: every file of the bundle sits next to it.
static BUNDLE: OnceLock<String> = OnceLock::new();
/// The host page's language, and its attributes or the names of those it left out.
static MOUNT: OnceLock<(Locale, Result<Host, Vec<&'static str>>)> = OnceLock::new();

/// Called by the entry module with its own URL; registers the element.
#[wasm_bindgen]
pub fn define(bundle: String) {
	BUNDLE.set(bundle).expect("the entry module runs once");
	let mount = Closure::<dyn Fn(web_sys::Element)>::new(mount);
	ev_lib::mfe::register(TAG, &mount);
	mount.forget(); // the element can mount at any time after registration
}

fn mount(el: web_sys::Element) {
	let attr = |name| el.get_attribute(name);
	let host = match (attr("api"), attr("sign-in"), attr("csrf-cookie"), attr("base")) {
		(Some(api), Some(sign_in), Some(csrf_cookie), Some(base)) => Ok(Host { api, sign_in, csrf_cookie, base }),
		_ => Err(["api", "sign-in", "csrf-cookie", "base"].into_iter().filter(|a| !el.has_attribute(a)).collect()),
	};
	let mut cfg = dioxus::web::Config::new().rootelement(el.clone());
	if let Ok(host) = &host {
		cfg = cfg.history(std::rc::Rc::new(dioxus::web::WebHistory::new(Some(host.base.clone()), true)));
	}
	assert!(MOUNT.set((ev_lib::mfe::host_locale(&el), host)).is_ok(), "one dashboard per page");
	dioxus::LaunchBuilder::new().with_cfg(cfg).launch(root);
}

fn root() -> Element {
	let (locale, host) = MOUNT.get().expect("set before launch");
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
		.and_then(|(_, h)| h.as_ref().ok())
		.expect("only the router asks, and it renders once the attributes are read")
}

/// The API as this tab's member: the caller, or the member their tab acts as.
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

/// What a tab shows.
#[derive(Clone, Debug, PartialEq)]
enum View {
	Home,
	Gmail(i64),
	Place { gmail: i64, target: i64 },
	Telegram,
	Tokens,
}

impl dioxus::router::ToRouteSegments for View {
	fn display_route_segments(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Home => write!(f, "/"), // an empty path would be no URL at all: a link to it goes nowhere
			Self::Gmail(g) => write!(f, "/gmails/{g}"),
			Self::Place { gmail, target } => write!(f, "/gmails/{gmail}/places/{target}"),
			Self::Telegram => write!(f, "/telegram"),
			Self::Tokens => write!(f, "/tokens"),
		}
	}
}

impl dioxus::router::FromRouteSegments for View {
	type Err = String;

	fn from_route_segments(segments: &[&str]) -> Result<Self, String> {
		let id = |s: &str| s.parse::<i64>().map_err(|_| format!("{s:?} is not an id"));
		match segments {
			[] | [""] => Ok(Self::Home),
			["gmails", g] => Ok(Self::Gmail(id(g)?)),
			["gmails", g, "places", t] => Ok(Self::Place { gmail: id(g)?, target: id(t)? }),
			["telegram"] => Ok(Self::Telegram),
			["tokens"] => Ok(Self::Tokens),
			_ => Err(format!("no view at /{}", segments.join("/"))),
		}
	}
}

/// Where the dashboard is, as its URL under the host's `base` says: a view of the caller's
/// own, of the member their tab acts as, or the pick of a member to open a tab for.
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
enum Route {
	#[layout(Shell)]
		#[route("/act-as", PickView)]
		Pick {},
		#[route("/members/:member/:..view", TheirView)]
		Theirs { member: i64, view: View },
		#[route("/:..view", MyView)]
		Mine { view: View },
}

impl Route {
	fn at(member: Option<i64>, view: View) -> Self {
		match member {
			Some(member) => Self::Theirs { member, view },
			None => Self::Mine { view },
		}
	}

	fn member(&self) -> Option<i64> {
		match self {
			Self::Theirs { member, .. } => Some(*member),
			Self::Mine { .. } | Self::Pick {} => None,
		}
	}

	fn view(&self) -> &View {
		match self {
			Self::Theirs { view, .. } | Self::Mine { view } => view,
			Self::Pick {} => &View::Home,
		}
	}
}

/// Each tab's last route, by its member: what a hidden tab keeps showing, and where going
/// back to it lands.
#[derive(Clone, Copy)]
struct Views(Signal<std::collections::HashMap<Option<i64>, Route>>);

/// The routes render nothing themselves — every tab stays mounted under [`Shell`] — they
/// only note where their tab is.
fn remember(route: Route) -> Element {
	let Views(mut views) = use_context();
	use_effect(use_reactive!(|route| {
		views.write().insert(route.member(), route);
	}));
	rsx! {}
}

#[component]
fn MyView(view: View) -> Element {
	remember(Route::Mine { view })
}

#[component]
fn TheirView(member: i64, view: View) -> Element {
	remember(Route::Theirs { member, view })
}

#[component]
fn PickView() -> Element {
	rsx! {}
}

const TABS: &str = "review_archive.tabs";

fn session() -> web_sys::Storage {
	web_sys::window()
		.expect("a browser")
		.session_storage()
		.expect("sessionStorage is readable")
		.expect("a page has sessionStorage")
}

/// The API's base URL: `api` resolved against the page.
fn base() -> reqwest::Url {
	let page = web_sys::window().expect("a browser").location().href().expect("a page has a URL");
	reqwest::Url::parse(&page).and_then(|p| p.join(&host().api)).expect("`api` resolves against the page")
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

#[component]
fn Shell() -> Element {
	use_context_provider(|| Views(Signal::new(Default::default())));
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
	#[cfg(feature = "grafana")]
	let body = rsx! { grafana::Host { me: me.id, {body} } };
	body
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
	let members = use_resource(|| async { api().members().await.map_err(shown) });
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
		None => Route::at(m, View::Home),
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
				Workspace { view: if matches!(route, Route::Mine { .. }) { route.clone() } else { view_of(None) } }
			}
			for m in tabs {
				div { key: "{m}", class: shown(Some(m)),
					Workspace { view: if active == Some(m) { route.clone() } else { view_of(Some(m)) } }
				}
			}
			if route == (Route::Pick {}) {
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

/// fzf over everyone who signed in here: name and email, and their balance.
#[component]
fn Picker(on_pick: EventHandler<MemberDto>, on_close: EventHandler<()>) -> Element {
	let tr: Translator = use_context();
	let members = use_resource(|| async { api().members().await.map_err(shown) });
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
									uikit::Badge { variant: BadgeVariant::Outline, {t!(tr, "picker.unclaimed", "never signed in")} }
								}
								span { "{m.name}" }
								span { class: "flex-1 truncate text-ink-soft", "{m.email}" }
								Balance { member: m.id, balance: m.balance }
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

/// A member's balance in the picker, and a change of it.
#[component]
fn Balance(member: i64, balance: i64) -> Element {
	let mut balance = use_signal(|| balance);
	let mut amount = use_signal(String::new);
	let mut failure = use_signal(|| None::<String>);
	let mut change = move |to: fn(i64) -> BalanceChange| {
		let n = match amount().trim().parse::<i64>() {
			Ok(n) => n,
			Err(e) => return failure.set(Some(format!("{:?}: {e}", amount()))),
		};
		spawn(async move {
			match api().change_tokens(member, &TokensChange { change: to(n), note: None }).await {
				Ok(t) => {
					balance.set(t.balance);
					amount.set(String::new());
					failure.set(None);
				}
				Err(e) => failure.set(Some(shown(e))),
			}
		});
	};
	let mut set = change;
	rsx! {
		// the row's click picks the member: these are not that
		div { class: "flex shrink-0 items-center gap-1", onclick: |e| e.stop_propagation(),
			if let Some(e) = failure() {
				span { class: "max-w-40 truncate text-[11px] text-accent-error", title: "{e}", "{e}" }
			}
			span { class: "w-20 text-right font-mono text-ink-soft", "{balance} tokens" }
			if may(Tokens::Grant) {
			Input {
				class: "w-16",
				size: Size::Xs,
				value: amount(),
				oninput: move |e: FormEvent| amount.set(e.value()),
			}
			Button { variant: ButtonVariant::Outline, size: Size::Xs, r#type: "button", onclick: move |_| set(BalanceChange::Set), "set" }
			Button { variant: ButtonVariant::Outline, size: Size::Xs, r#type: "button", onclick: move |_| change(BalanceChange::Grant), "grant" }
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
	let Api(client) = use_context_provider(|| {
		Api(match tab {
			Some(m) => api().as_member(m),
			None => api(),
		})
	});
	let refresh = use_context_provider(|| Refresh(Signal::new(0)));
	use_context_provider(|| Signal::new(filter::Filter::default()));
	let failure = use_context_provider(|| Failure(Signal::new(None)));
	let overview = use_resource(move || {
		let client = client.clone();
		async move {
			refresh.0();
			client.overview().await.map_err(shown)
		}
	});
	// while a scan is queued or walking, its card and the balance it charges are read again
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
				&& gmails.iter().flat_map(|g| &g.locations).any(|l| l.scan.is_some())
			{
				refresh += 1;
			}
		}
	});
	// `/me` is the caller whoever they act as: who a member tab is, and their balance, are read
	// off the members list
	let unknown = tr.clone();
	let tokens = use_resource(move || {
		let unknown = unknown.clone();
		async move {
			refresh.0();
			let mine = api().me().await.map_err(shown)?.tokens;
			let whom = match tab {
				Some(m) => Some(
					api()
						.members()
						.await
						.map_err(shown)?
						.into_iter()
						.find(|x| x.id == m)
						.ok_or_else(|| t!(unknown, "workspace.no_member", "There is no member #{member}", member = m))?,
				),
				None => None,
			};
			Ok::<_, String>((mine, whom))
		}
	});

	let shell = SHELL;
	let gmails: Vec<GmailOverview> = match &*overview.read() {
		Some(Ok(g)) => g.clone(),
		Some(Err(e)) => return rsx! { div { class: "{shell} p-6 text-accent-error", "{e}" } },
		_ => return rsx! { div { class: "{shell} p-6 text-ink-soft", "Loading…" } },
	};
	let (tokens, whom) = match &*tokens.read() {
		Some(Ok((mine, whom))) => (
			Some(whom.as_ref().map_or_else(
				|| mine.clone(),
				|w| TokensDto {
					balance: w.balance,
					kickbacks: Vec::new(),
					..mine.clone()
				},
			)),
			whom.clone(),
		),
		Some(Err(e)) => return rsx! { div { class: "{shell} p-6 text-accent-error", "{e}" } },
		None => (None, None),
	};
	let at = view.view().clone();
	let (picked, target) = match at {
		View::Home | View::Telegram | View::Tokens => (None, None),
		View::Gmail(gmail) => (Some(gmail), None),
		View::Place { gmail, target } => (Some(gmail), Some(target)),
	};
	// a removed gmail's URL shows them all, as home does
	let current = picked.filter(|id| gmails.iter().any(|g| g.gmail.id == *id));
	let scope = current.and_then(|id| gmails.iter().find(|g| g.gmail.id == id)).cloned();
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
			Rail { gmails: gmails.clone(), current, at: at.clone(), tokens: tokens.clone(), tab }
			div { class: "flex min-h-0 min-w-0 flex-1 flex-col",
				if let Some(acting) = acting {
					div { class: "border-b border-border bg-accent-warn/15 px-6 py-2 text-accent-warn", "{acting}" }
				}
				if let Some(e) = failure.0() {
					div { class: "border-b border-border bg-accent-error/15 px-6 py-2 text-accent-error", "{e}" }
				}
				match (at, scope) {
					(View::Telegram, _) => rsx! {
						telegram::Channels { gmails: gmails.iter().map(|g| g.gmail.clone()).collect::<Vec<_>>() }
					},
					(View::Tokens, _) => rsx! {
						tokens::Ledger { gmails: gmails.clone(), tokens, tab }
					},
					(_, None) if gmails.is_empty() => rsx! {
						div { class: "p-6 text-ink-soft", "Add the gmail your places are managed from, on the left." }
					},
					(_, None) => {
						let mut cards: Vec<_> = gmails.iter().flat_map(|g| g.locations.iter().map(|l| (g.gmail.clone(), l.clone()))).collect();
						cards.sort_by_key(|(_, l)| (std::cmp::Reverse(l.snapshots_7d), l.target.id));
						rsx! { Places { cards, under: gmails.iter().map(|g| g.gmail.clone()).collect::<Vec<_>>(), gmail: None, tab } }
					}
					(_, Some(g)) => match target.and_then(|t| g.locations.iter().find(|l| l.target.id == t)) {
						Some(loc) => rsx! {
							board::Board { gmail: g.gmail.clone(), location: loc.clone(), tab }
						},
						None => rsx! {
							Places { cards: g.locations.iter().map(|l| (g.gmail.clone(), l.clone())).collect::<Vec<_>>(), under: vec![g.gmail.clone()], gmail: Some(g.gmail), tab }
						},
					},
				}
			}
		}
	}
}

#[component]
fn Rail(gmails: Vec<GmailOverview>, current: Option<i64>, at: View, tokens: Option<TokensDto>, tab: Option<i64>) -> Element {
	let telegram = at == View::Telegram;
	let aside = telegram || at == View::Tokens;
	let mut adding = use_signal(String::new);
	let all_on = !gmails.is_empty() && gmails.iter().all(|g| g.gmail.enabled);
	let ids: Vec<i64> = gmails.iter().map(|g| g.gmail.id).collect();
	let row = "flex items-center gap-2 rounded-md px-3 py-2 text-left cursor-pointer hover:bg-hover";
	rsx! {
		nav { class: "flex w-60 shrink-0 flex-col overflow-y-auto border-r border-border bg-secondary",
			div { class: "flex items-center gap-2 px-4 py-4",
				span { class: "flex-1 text-[14px] font-semibold", "review_archive" }
				if let Some(t) = tokens {
					if !t.kickbacks.is_empty() {
						button {
							class: "rounded-md px-1.5 py-0.5 font-mono text-positive cursor-pointer hover:bg-hover",
							title: t.kickbacks.iter().fold("added back on click, these failed:".to_owned(), |mut s, k| {
								let why: String = k.why.chars().take(120).collect();
								s += &format!("\n· {}, {}: {why}", k.target_label, ago(&k.at));
								s
							}),
							onclick: move |_| act(|c| async move { c.claim_kickbacks().await.map(drop) }),
							"+{t.kickbacks.iter().map(|k| k.amount).sum::<i64>()}"
						}
					}
					Link {
						class: if at == View::Tokens { "rounded-md px-2 py-0.5 bg-hover text-ink" } else { "rounded-md px-2 py-0.5 text-ink-soft hover:bg-hover" },
						to: Route::at(tab, View::Tokens),
						span { title: "+{t.daily}/day up to {t.cap}", "{t.balance} tokens" }
					}
				}
			}
			div { class: "flex items-center gap-1.5 pl-2 pr-2 pb-1 text-[11px] font-medium uppercase tracking-wide text-ink-soft",
				Link {
					class: if at == View::Home { "rounded-md px-2 py-0.5 bg-hover text-ink" } else { "rounded-md px-2 py-0.5 hover:bg-hover" },
					to: Route::at(tab, View::Home),
					"Managing gmails"
				}
				InfoTip {
					InfoTipTrigger { label: "What a managing gmail is" }
					InfoTipContent { class: "normal-case font-normal tracking-normal flex flex-col gap-2",
						p { "Full gmail address or any alias for it. Used only to group places; it doesn't affect any actions taken." }
						p { "If no managing account is connected, enter the email the place is on, or its shorthand." }
					}
				}
				div { class: "flex-1" }
				Switch {
					checked: all_on,
					disabled: ids.is_empty(),
					on_checked_change: move |on: bool| {
						let ids = ids.clone();
						act(move |c| async move {
							for id in ids {
								c.set_gmail_enabled(id, on).await?;
							}
							Ok(())
						});
					},
				}
			}
			Interval {}
			div { class: "flex flex-col gap-0.5 px-2",
				for g in gmails {
					div { key: "{g.gmail.id}", class: "flex items-center gap-1",
						Link {
							class: format!(
								"{row} flex-1 min-w-0 {} {}",
								if current == Some(g.gmail.id) && !aside { "bg-hover text-ink" } else { "text-ink-soft" },
								if g.gmail.enabled { "" } else { "opacity-50" }
							),
							to: Route::at(tab, View::Gmail(g.gmail.id)),
							span { class: "truncate flex-1", "{g.gmail.gmail}" }
							uikit::Badge { variant: BadgeVariant::Secondary, "{g.locations.len()}" }
						}
						Switch {
							checked: g.gmail.enabled,
							on_checked_change: move |on: bool| {
								let id = g.gmail.id;
								act(move |c| async move { c.set_gmail_enabled(id, on).await });
							},
						}
					}
				}
			}
			form {
				class: "mt-2 flex gap-1 px-3",
				onsubmit: move |e| {
					e.prevent_default();
					let gmail = adding();
					act(move |c| async move { c.add_gmail(&gmail).await.map(drop) });
					adding.set(String::new());
				},
				Input {
					size: Size::Sm,
					placeholder: "+ Add gmail",
					value: adding(),
					oninput: move |e: FormEvent| adding.set(e.value()),
				}
			}
			div { class: "flex-1" }
			Link {
				class: if telegram { "{row} mx-2 mb-3 bg-hover text-ink" } else { "{row} mx-2 mb-3 text-ink-soft" },
				to: Route::at(tab, View::Telegram),
				"Telegram alerts"
			}
		}
	}
}

/// How often every place of every gmail is scanned.
#[component]
fn Interval() -> Element {
	const EVERY: [(&str, u64); 6] = [("1h", 3600), ("6h", 6 * 3600), ("12h", 12 * 3600), ("1d", 86_400), ("2d", 2 * 86_400), ("1w", 7 * 86_400)];
	const OWN: &str = "default"; // each place's own interval
	let Api(api) = use_context();
	let Refresh(refresh) = use_context();
	let chosen = use_resource(move || {
		let api = api.clone();
		async move {
			refresh();
			api.interval().await.map_err(shown)
		}
	});
	let secs = match &*chosen.read() {
		Some(Ok(c)) => c.interval_secs,
		Some(Err(e)) => return rsx! { div { class: "px-4 pb-1 text-accent-error", "{e}" } },
		None => return rsx! {},
	};
	let current = secs.map_or_else(
		|| OWN.to_owned(),
		|s| EVERY.iter().find(|(_, x)| *x == s).map_or_else(|| format!("{s}s"), |(k, _)| (*k).to_owned()),
	);
	let custom = secs.filter(|s| !EVERY.iter().any(|(_, x)| x == s));
	rsx! {
		div { class: "flex items-center gap-2 px-4 pb-2 text-ink-soft",
			span { class: "flex-1", "Scan every" }
			Select {
				value: Some(current),
				on_value_change: move |v: String| {
					act(move |c| async move { c.set_interval(&IntervalChange { interval: (v != OWN).then_some(v) }).await });
				},
				SelectTrigger { size: Size::Sm, class: "w-28", SelectValue {} }
				SelectContent {
					SelectItem { value: OWN, "{OWN}" }
					for (k, _) in EVERY {
						SelectItem { key: "{k}", value: k, "{k}" }
					}
					if let Some(s) = custom {
						SelectItem { value: "{s}s", "{s}s" }
					}
				}
			}
		}
	}
}

/// Places as cards, as given: one gmail's (`gmail`), else every gmail's. A place tracked under
/// two gmails is a card under each: its counts are that gmail's. `under`: the gmails a new place can be tracked under.
#[component]
fn Places(cards: Vec<(GmailDto, LocationSummary)>, under: Vec<GmailDto>, gmail: Option<GmailDto>, tab: Option<i64>) -> Element {
	assert!(!under.is_empty(), "Workspace shows no Places without a gmail");
	let mut place = use_signal(String::new);
	let mut picked = use_signal(|| None::<String>);
	let into = match under.as_slice() {
		[only] => Some(only.id),
		_ => picked().and_then(|gmail| under.iter().find(|g| g.gmail == gmail)).map(|g| g.id),
	};
	let mut choices = use_signal(|| None::<(i64, Vec<PlaceDto>)>);
	let mut pending = use_signal(|| false);
	let mut outcome = use_signal(|| None::<String>);
	let held: Vec<(i64, i64)> = cards.iter().map(|(g, l)| (g.id, l.target.id)).collect();
	let filter: Signal<filter::Filter> = use_context();
	let total = cards.len();
	let shown = filter.read().keep(cards);
	let count = shown.len();
	#[cfg(feature = "grafana")]
	let overview = rsx! {
		grafana::Overview {
			gmail: gmail.as_ref().map(|g| g.id),
			keys: shown.iter().map(|(g, l)| format!("{}:{}", g.id, l.target.id)).collect::<Vec<_>>().join(","),
			tab,
		}
	};
	#[cfg(not(feature = "grafana"))]
	let overview = rsx! {
		main { class: "min-h-0 flex-1 overflow-y-auto p-6",
			if total == 0 {
				div { class: "text-ink-soft",
					if gmail.is_some() { "No places tracked under this gmail yet." } else { "No places tracked yet." }
				}
			} else if shown.is_empty() {
				div { class: "text-ink-soft", "No place matches the filters." }
			}
			div { class: "grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] gap-4",
				for (g, loc) in shown {
					cards::LocationCard {
						key: "{g.id}-{loc.target.id}",
						to: Route::at(tab, View::Place { gmail: g.id, target: loc.target.id }),
						gmail: g,
						all: gmail.is_none(),
						loc,
					}
				}
			}
		}
	};
	rsx! {
		header { class: "flex h-14 shrink-0 items-center gap-3 border-b border-border px-6",
			span { class: "text-ink-soft", {gmail.as_ref().map_or("All gmails", |g| g.gmail.as_str())} }
			span { class: "text-ink-soft", "/" }
			span { class: "font-medium", "Locations" }
			div { class: "flex-1" }
			form {
				class: "flex gap-2",
				onsubmit: move |e: FormEvent| {
					e.prevent_default();
					let id = into.expect("the submit button is disabled until a gmail is picked");
					let gmail = under.iter().find(|g| g.id == id).expect("into is one of under").gmail.clone();
					let input = place();
					let held = held.clone();
					pending.set(true);
					outcome.set(None);
					choices.set(None);
					act(move |c| async move {
						let r = async {
							match <[PlaceDto; 1]>::try_from(c.places(&input).await?) {
								Ok([one]) => {
									let t = c.track(id, &NewTrack { place: one.place_id, ..Default::default() }).await?;
									outcome.set(Some(tracked(&held, id, &gmail, &t)));
								}
								Err(many) => choices.set(Some((id, many))),
							}
							Ok(())
						}
						.await;
						pending.set(false);
						r
					});
					place.set(String::new());
				},
				if under.len() > 1 {
					Select { on_value_change: move |v: String| picked.set(Some(v)),
						SelectTrigger { class: "w-56", SelectValue { placeholder: "under gmail…" } }
						SelectContent {
							for g in under.iter() {
								SelectItem { key: "{g.id}", value: "{g.gmail}", "{g.gmail}" }
							}
						}
					}
				}
				Input {
					class: "w-80",
					placeholder: "Place id or Google Maps URL",
					disabled: pending(),
					value: place(),
					oninput: move |e: FormEvent| place.set(e.value()),
				}
				Button { disabled: into.is_none() || pending(),
					if pending() { Spinner {} " Tracking…" } else { "+ Track place" }
				}
			}
		}
		if let Some(o) = outcome() {
			div { class: "flex items-center gap-2 border-b border-border px-6 py-2",
				span { "{o}" }
				div { class: "flex-1" }
				Button { variant: ButtonVariant::Ghost, size: Size::Xs, r#type: "button", onclick: move |_| outcome.set(None), "dismiss" }
			}
		}
		if let Some((id, many)) = choices() {
			div { class: "flex flex-col gap-1 border-b border-border px-6 py-3",
				div { class: "flex items-center gap-2",
					span { class: "text-ink-soft", "{many.len()} places bear this name; which one?" }
					div { class: "flex-1" }
					Button { variant: ButtonVariant::Ghost, size: Size::Xs, r#type: "button", onclick: move |_| choices.set(None), "cancel" }
				}
				for p in many {
					Button {
						key: "{p.place_id}",
						class: "justify-start text-left",
						variant: ButtonVariant::Outline,
						size: Size::Sm,
						r#type: "button",
						onclick: {
							let place_id = p.place_id.clone();
							let held = held.clone();
							let gmail = under.iter().find(|g| g.id == id).expect("choices are offered under a gmail of under").gmail.clone();
							move |_| {
								let req = NewTrack { place: place_id.clone(), ..Default::default() };
								let (held, gmail) = (held.clone(), gmail.clone());
								pending.set(true);
								act(move |c| async move {
									let t = c.track(id, &req).await;
									pending.set(false);
									outcome.set(Some(tracked(&held, id, &gmail, &t?)));
									Ok(())
								});
								choices.set(None);
							}
						},
						span { class: "font-medium", {p.name.clone().unwrap_or(p.place_id.clone())} }
						if let Some(a) = &p.address { span { class: "text-ink-soft", " · {a}" } }
					}
				}
			}
		}
		filter::Bar { shown: count, total }
		{overview}
	}
}

/// A track of a place its gmail had already is a no-op, which would otherwise look like nothing happened.
fn tracked(held: &[(i64, i64)], gmail: i64, address: &str, t: &TargetDto) -> String {
	match held.contains(&(gmail, t.id)) {
		true => format!("{} is already tracked under {address}", t.label),
		false => format!("Now tracking {} under {address}", t.label),
	}
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
	Ok,
	#[cfg(not(feature = "grafana"))]
	Bad,
	Warn,
}

#[component]
fn Badge(tone: Tone, children: Element) -> Element {
	let (variant, class) = match tone {
		Tone::Ok => (BadgeVariant::Success, ""),
		#[cfg(not(feature = "grafana"))]
		Tone::Bad => (BadgeVariant::Outline, "border-transparent bg-accent-error/15 text-accent-error"),
		Tone::Warn => (BadgeVariant::Outline, "border-transparent bg-accent-warn/15 text-accent-warn"),
	};
	rsx! {
		uikit::Badge { variant, class, {children} }
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
