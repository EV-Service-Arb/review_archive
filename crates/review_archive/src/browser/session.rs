//! Driving a plain headless Chromium. No stealth, no fingerprint masking:
//! the scanner is what it looks like, and a block from Google fails the run.

use std::{collections::HashSet, path::Path, time::Duration};

use browser_manipulation::{Browser, ErrorKind, Launch, Robot, Tab, Viewport};
use eyre::WrapErr;
use jiff::Timestamp;
use review_archive_core::{
	Capture, Warning,
	maps::{
		WalkEnd, WalkPolicy, Walked, WalkedCard, cost, parse,
		selectors::{self as sel, js},
	},
	tokens::Meter,
};
use serde::Serialize;
use v_utils::memory_lease::Lease;

use super::{
	host::{self, Ceiling},
	launch_report::{self, Report},
	lease,
};
use crate::{
	SessionError,
	config::{BrowserConfig, Lean},
	failure::launch_is_transient,
};

/// Archived evidence: a card at 1x is too blurry to read. Only the card is rasterized at it.
const CARD_SCALE: f64 = 2.0;

/// How long a page has to answer a script; one that does not is stalled, whatever it shows.
const ANSWER: Duration = Duration::from_secs(20);

/// For a browser that only ever visits Google, and walks one page there: one renderer, a small
/// disk cache, the network, audio and GPU work inside the browser process, and none of the
/// features it would run beside the page (ARCHITECTURE.md § Memory).
const ARGS: &[&str] = &[
	"--renderer-process-limit=1",
	"--disable-site-isolation-trials",
	"--disable-features=SitePerProcess,IsolateOrigins,AutofillServerCommunication,BackForwardCache,AudioServiceOutOfProcess,OptimizationTargetPrediction,OptimizationGuideModelExecution,OptimizationGuideOnDeviceModel,PageContentAnnotations",
	"--enable-features=NetworkServiceInProcess2",
	"--no-zygote", // the driver already passes `--no-sandbox`, which it needs
	"--in-process-gpu",
	"--disk-cache-size=33554432",
	"--disable-notifications",
	"--disable-speech-api",
	"--disable-print-preview",
	"--disable-extensions",
	"--disable-component-update",
	"--disable-default-apps",
	"--disable-domain-reliability",
	"--enable-precise-memory-info", // `performance.memory` unrounded, for the walk's log
];

/// The review list, open and ready to walk.
#[derive(Debug)]
pub(crate) struct Opened {
	pub page_url: String,
	pub sorted: bool,
	pub total: Option<u64>,
	pub name: String,
	pub address: Option<String>,
	pub post: Option<parse::Post>,
}

/// What the sort button opened.
enum SortMenu {
	Open,
	/// Google's sign-in dialog, in place of the menu.
	SignInRequired,
}

pub(crate) struct Session {
	browser: Browser<Robot>,
	cfg: BrowserConfig,
	/// `memory.max` when the lease was granted.
	pub(super) leased: Option<Ceiling>,
	/// Last, so the burst ends only after Chromium has: the field drops in declaration order, and a
	/// pod shrunk under a live Chromium is killed for its memory.
	_lease: Lease,
}

/// One walk's tab.
pub(crate) struct Page<'s> {
	tab: Tab<'s, Robot>,
	cfg: &'s BrowserConfig,
}

impl Session {
	pub(crate) async fn launch(cfg: &BrowserConfig, profile_dir: &Path) -> eyre::Result<Self> {
		let executable = cfg.executable.clone().ok_or_else(|| SessionError::new_launch("`browser.executable` is not set".to_owned()))?;
		let lean = cfg
			.lean
			.as_deref()
			.ok_or_else(|| SessionError::new_launch("`browser.lean` is not set (`[]` for none)".to_owned()))?;
		forget_negotiate(profile_dir)?;
		let launch = Launch::Owned {
			profile: profile_dir.to_owned(),
			executable,
			headless: !cfg.headful,
			// wide enough for the desktop layout the selectors know; cards are shot at `CARD_SCALE`, beyond it
			viewport: Some(Viewport {
				width: 1280,
				height: 900,
				device_scale_factor: 1.0,
			}),
			args: args(lean),
		};
		// The burst is taken before Chromium and lives as long as it does: Chromium's start and
		// work need more than the pod's base memory, and the pod is only resized up while a lease lives.
		let acquire = async { Ok((Lease::acquire().await?, host::limit().await)) };
		let ((lease, leased), browser, took) = lease::around(acquire, lease::WAIT, Browser::launch(launch, Robot, cfg.artifacts.clone())).await?;
		let browser = match browser {
			Ok(browser) => browser,
			Err(e) => return Err(launch_failure(e, took, leased, cfg).await),
		};
		Ok(Self {
			browser,
			cfg: cfg.clone(),
			leased,
			_lease: lease,
		})
	}

	pub(crate) async fn close(self) {
		if let Err(e) = self.browser.close().await {
			tracing::warn!(error = %e, "closing Chromium");
		}
	}

	pub(crate) async fn page(&self) -> Result<Page<'_>, SessionError> {
		let mut tab = self.browser.tab().await?;
		tab.block(sel::BLOCKED).await?;
		Ok(Page { tab, cfg: &self.cfg })
	}
}

/// Chromium remembers a site's `WWW-Authenticate: Negotiate` in `Local State`, and from then on
/// hangs every start with `NetworkServiceInProcess2` until it is killed; nothing here authenticates with Negotiate.
fn forget_negotiate(profile_dir: &Path) -> eyre::Result<()> {
	let path = profile_dir.join("Local State");
	let text = match std::fs::read_to_string(&path) {
		Ok(t) => t,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()), // a fresh profile
		Err(e) => return Err(eyre::Report::new(e).wrap_err(format!("reading {}", path.display()))),
	};
	let mut state: serde_json::Value = serde_json::from_str(&text).wrap_err_with(|| format!("parsing {}", path.display()))?;
	let Some(net) = state.get_mut("net").and_then(serde_json::Value::as_object_mut) else {
		return Ok(());
	};
	if net.remove("received_http_auth_negotiate_headers").is_none() {
		return Ok(());
	}
	let tmp = path.with_extension("tmp");
	std::fs::write(&tmp, serde_json::to_vec(&state)?).wrap_err_with(|| format!("writing {}", tmp.display()))?;
	std::fs::rename(&tmp, &path).wrap_err_with(|| format!("replacing {}", path.display()))
}

fn args(lean: &[Lean]) -> Vec<String> {
	let mut args: Vec<String> = ARGS.iter().map(|a| (*a).to_owned()).collect();
	let mut js = Vec::new();
	for l in lean {
		match *l {
			Lean::JsHeapMib(mib) => js.extend([format!("--max-old-space-size={mib}"), "--optimize-for-size".to_owned()]),
			Lean::SmallYoungGeneration => js.push("--max-semi-space-size=1".to_owned()),
			Lean::JsLiteMode => js.push("--lite-mode".to_owned()),
			Lean::OneRasterThread => args.push("--num-raster-threads=1".to_owned()),
			Lean::LowEndDevice => args.push("--enable-low-end-device-mode".to_owned()),
		}
	}
	if !js.is_empty() {
		args.push(format!("--js-flags={}", js.join(" "))); // Chromium keeps only the last `--js-flags`
	}
	args
}

/// How long Playwright gives a start: the driver's initialization in playwright-rs, and
/// Chromium's launch in the driver, are both 30 s and neither is configurable from here.
const LAUNCH_LIMIT: Duration = Duration::from_secs(30);

/// A slow start becomes [`SessionError::LaunchTimeout`] with what the host looked like, its
/// figures saved as a file the error names as `[<path>]` for the alerts to attach; any other
/// failure is the setup's, as it came.
async fn launch_failure(e: browser_manipulation::Error, took: Duration, leased: Option<Ceiling>, cfg: &BrowserConfig) -> eyre::Report {
	let ErrorKind::Launch(source) = &*e.kind else { return SessionError::from(e).into() };
	if !launch_is_transient(source) {
		return SessionError::from(e).into();
	}
	let reason = source.to_string().split('\n').next().expect("split yields at least one piece").trim_end().to_owned(); // the rest is the driver's JS stack
	let host = host::Snapshot::capture(leased).await.to_string();
	let report = Report {
		reason: &reason,
		took,
		limit: LAUNCH_LIMIT,
		host: &host,
	};
	let saved = match &cfg.artifacts {
		Some(a) => Some(launch_report::save(&a.dir, &report, Timestamp::now()).await),
		None => None,
	};
	let timeout = eyre::Report::new(SessionError::new_launch_timeout(reason, took, LAUNCH_LIMIT, host));
	match saved {
		Some(Ok(path)) => timeout.wrap_err(format!("figures [{}]", path.display())),
		Some(Err(e)) => timeout.wrap_err(format!("figures not saved: {e}")),
		None => timeout,
	}
}

impl Page<'_> {
	pub(crate) async fn close(self) -> Result<(), SessionError> {
		Ok(self.tab.close().await?)
	}

	/// Opens Maps on no place, and says whether its account corner offers to sign in. A corner that
	/// never shows the link reads as signed in: the next walk finds out for sure.
	pub(crate) async fn signed_out(&mut self) -> Result<bool, SessionError> {
		self.tab.set_timeout(self.cfg.nav_timeout.duration()).await;
		self.tab.goto(sel::MAPS_HOME).await?;
		self.handle_interstitials().await?;
		self.wait_for_any(sel::SIGNED_OUT).await
	}

	/// Opens the place, reads its name, address and the owner's latest post off its overview, and opens its review
	/// list sorted newest first. `Err`: the walk is already over — the place has no reviews,
	/// or `policy` does not want them read.
	pub(crate) async fn open_reviews(&mut self, place_id: &str, lang: &str, policy: &mut dyn WalkPolicy, meter: &mut Meter) -> Result<Result<Opened, Walked>, SessionError> {
		let url = sel::place_url(place_id, lang);
		self.tab.set_timeout(self.cfg.nav_timeout.duration()).await;
		meter.spend(cost::OPEN);
		self.tab.goto(&url).await?;
		self.handle_interstitials().await?;

		// The place panel renders after load; wait for the way into the reviews.
		let has_reviews_tab = self.wait_for_any(sel::REVIEWS_TAB).await?;
		if !self.cfg.guest && self.eval::<bool>(js::ANY, sel::SIGNED_OUT).await? {
			return Err(SessionError::new_signed_out());
		}
		let post = parse::owner_post(&self.eval::<String>(js::FIRST_HTML, sel::OWNER_POST).await?);
		let name = parse::place_name(&self.eval::<String>(js::FIRST_HTML, sel::PLACE_TITLE).await?);
		let address = parse::place_address(&self.eval::<String>(js::FIRST_HTML, sel::PLACE_ADDRESS).await?);
		if !has_reviews_tab {
			if self.eval::<bool>(js::HAS_TEXT, sel::LIMITED_VIEW_TEXT).await? {
				return Err(SessionError::new_limited_view());
			}
			// no place panel at all: Google does not know the id, whatever its markup is now
			let Some(name) = name else {
				return Err(SessionError::new_place_not_found(place_id.to_owned()));
			};
			// The place rendered, and has no star average: nobody has reviewed it yet.
			if !self.eval::<bool>(js::ANY, sel::RATING_SUMMARY).await? {
				tracing::info!(place_id, "the place has no reviews");
				return Ok(Err(Walked::empty(url, name, address, post)));
			}
			return Err(markup_changed("waiting for the reviews tab", sel::REVIEWS_TAB));
		}
		let name = name.ok_or_else(|| markup_changed("reading the place's name", sel::PLACE_TITLE))?;
		meter.spend(cost::REVIEWS);
		if !self.click_first(sel::REVIEWS_TAB).await? {
			return Err(markup_changed("clicking the reviews tab", sel::REVIEWS_TAB));
		}
		if !self.wait_for_any(&[sel::CARD]).await? {
			if self.review_total().await? == Some(0) {
				tracing::info!(place_id, "the review list is empty");
				return Ok(Err(Walked::empty(url, name, address, post)));
			}
			return Err(markup_changed("waiting for the review list", &[sel::CARD]));
		}
		let total = self.review_total().await?;
		if !policy.wants_list(total) {
			tracing::info!(place_id, ?total, "the review count is the last scan's; the list is not read");
			return Ok(Err(Walked::unchanged(self.tab.url(), name, address, total, post)));
		}

		if !self.wait_for_any(sel::SORT_BUTTON).await? {
			return Err(markup_changed("waiting for the sort button", sel::SORT_BUTTON));
		}
		let sorted = match self.cfg.guest {
			true => false, // a guest's sort button opens the sign-in dialog
			false => match self.open_sort_menu(meter).await? {
				SortMenu::Open => self.pick_newest().await?,
				SortMenu::SignInRequired => return Err(SessionError::new_signed_out()),
			},
		};

		Ok(Ok(Opened {
			page_url: self.tab.url(),
			sorted,
			total,
			name,
			address,
			post,
		}))
	}

	/// Clicks the sort button until the sort menu or Google's sign-in dialog shows.
	async fn open_sort_menu(&mut self, meter: &mut Meter) -> Result<SortMenu, SessionError> {
		let either = [sel::SORT_NEWEST, sel::SIGN_IN_GATE].concat();
		let mut gated = 0;
		// A click that lands while the list is still hydrating is swallowed; retry a few times.
		for attempt in 0..4 {
			meter.spend(if attempt == 0 { cost::SORT } else { cost::SORT_RETRY });
			self.dismiss_promo().await?;
			if !self.click_first(sel::SORT_BUTTON).await? {
				return Err(markup_changed("clicking the sort button", sel::SORT_BUTTON));
			}
			if !self.wait_for_any_within(&either, Duration::from_secs(5)).await? {
				continue;
			}
			if self.eval::<bool>(js::ANY, sel::SORT_NEWEST).await? {
				return Ok(SortMenu::Open);
			}
			// The same dialog also turns up by itself over a fresh profile's list; only a
			// second showing in answer to the button says the menu is gated.
			gated += 1;
			if gated == 2 {
				return Ok(SortMenu::SignInRequired);
			}
		}
		Err(markup_changed("opening the sort menu (or Google's sign-in dialog)", &either))
	}

	/// Picks "newest" in the open sort menu; `false`: the list was not seen to re-sort.
	async fn pick_newest(&mut self) -> Result<bool, SessionError> {
		let before: String = self.eval(js::FIRST_CARD_ID, (sel::CARD, sel::CARD_ID_ATTR)).await?;
		if !self.click_first(sel::SORT_NEWEST).await? {
			return Err(markup_changed("picking 'newest'", sel::SORT_NEWEST));
		}
		// The relevance-sorted cards stay in the DOM until the new list arrives; reading them
		// would archive the wrong end of the list. Wait for the first card to change.
		let deadline = tokio::time::Instant::now() + self.cfg.sort_timeout.duration();
		let sorted = loop {
			tokio::time::sleep(Duration::from_millis(300)).await;
			let now: String = self.eval(js::FIRST_CARD_ID, (sel::CARD, sel::CARD_ID_ATTR)).await?;
			if !now.is_empty() && now != before {
				break true;
			}
			if tokio::time::Instant::now() >= deadline {
				// The newest review can also be the most relevant one — or the click was
				// swallowed. The walk goes on, but its order is not relied on.
				tracing::info!("first card unchanged after sorting by newest; the list's order is unconfirmed");
				break false;
			}
		};
		tokio::time::sleep(self.cfg.step_wait.duration()).await;
		if !self.wait_for_any(&[sel::CARD]).await? {
			return Err(markup_changed("waiting for the list to come back sorted", &[sel::CARD]));
		}
		Ok(sorted)
	}

	/// The page as it is now, into the artifacts; `None` without them.
	pub(crate) async fn save(&self, hint: &str) -> Option<Result<browser_manipulation::Capture, String>> {
		self.tab.capture(hint).await
	}

	/// The review count the list's histogram adds up to.
	async fn review_total(&mut self) -> Result<Option<u64>, SessionError> {
		let labels: Vec<String> = self.eval(js::LABELS, sel::HISTOGRAM_ROW).await?;
		Ok(parse::review_total(&labels))
	}

	/// Walks the open review list, capturing the cards the policy asks for. A page that
	/// fails once cards were read ends the walk [`WalkEnd::Interrupted`] with what it read
	/// — unless Google blocked it, which fails the walk.
	pub(crate) async fn walk(&mut self, policy: &mut dyn WalkPolicy, meter: &mut Meter, max: usize, opened: Opened) -> Result<Walked, SessionError> {
		let page_url = opened.page_url.as_str();
		let mut seen = HashSet::new();
		let mut cards: Vec<WalkedCard> = Vec::new();
		let mut warnings = Vec::new();
		let mut idle = 0;
		if self.click_first(sel::PANEL_COLLAPSED).await? {
			tracing::info!("the side panel was collapsed: opening it");
			if !self.wait_for_any(sel::PANEL_OPEN).await? {
				return Err(markup_changed("opening the side panel", sel::PANEL_OPEN));
			}
		}
		let end = loop {
			let step = async {
				self.dismiss_promo().await?;
				self.eval::<u32>(js::EXPAND_ALL, (sel::CARD, sel::EXPAND)).await?;
				tokio::time::sleep(Duration::from_millis(300)).await;

				// cards already read are spacers by now; one the feed rendered again is dropped as `seen`
				let html: String = self.eval(js::CARDS_HTML, sel::CARD).await?;
				if let Some(dir) = self.cfg.dump_html.as_deref() {
					dump(dir, &html).await?;
				}
				let parsed = parse::cards(&html);
				let on_page: Vec<String> = parsed.iter().map(|c| c.id.clone()).collect();
				let mut read = Vec::new();
				for card in parsed {
					if cards.len() >= max {
						break;
					}
					if !seen.insert(card.id.clone()) {
						continue;
					}
					read.push(card.id.clone());
					let capture = if policy.wants_capture(&card) {
						self.capture(&card.id, page_url)
							.await
							.map_err(|e| warnings.push(Warning::Failure(format!("screenshot of {} failed: {e:#}", card.id))))
							.ok()
					} else {
						None
					};
					policy.observe(&card);
					cards.push((card, capture));
				}
				if !on_page.is_empty() {
					self.eval::<()>(js::RELEASE, (sel::CARD, sel::CARD_ID_ATTR, &on_page)).await?;
				}
				if cards.len() >= max {
					return Ok(Some(WalkEnd::Cap));
				}
				if policy.satisfied() {
					return Ok(Some(WalkEnd::Satisfied));
				}
				idle = if read.is_empty() { idle + 1 } else { 0 };
				if idle >= self.cfg.idle_steps_to_end {
					return Ok(Some(WalkEnd::ReachedEnd));
				}
				self.check_not_blocked()?;
				if !meter.try_spend(cost::STEP) {
					return Ok(Some(WalkEnd::Budget));
				}
				// A list short enough to fit the panel has nothing to scroll and nothing more to load.
				if !self.eval::<bool>(js::SCROLL_FEED, sel::CARD).await? {
					return Ok(Some(WalkEnd::ReachedEnd));
				}
				tokio::time::sleep(self.cfg.step_wait.duration()).await;
				Ok::<_, SessionError>(None)
			};
			match step.await {
				Ok(Some(end)) => break end,
				Ok(None) => {}
				Err(e) if cards.is_empty() || matches!(e, SessionError::Blocked { .. }) => return Err(e),
				Err(e) => {
					warnings.push(Warning::Failure(format!("the walk stopped after {} reviews: {e:#}", cards.len())));
					break WalkEnd::Interrupted;
				}
			}
		};
		Ok(Walked {
			cards,
			end,
			page_url: opened.page_url,
			warnings,
			sorted: opened.sorted,
			total: opened.total,
			name: opened.name,
			address: opened.address,
			post: opened.post,
		})
	}

	async fn capture(&mut self, id: &str, page_url: &str) -> Result<Capture, SessionError> {
		let marked: bool = self.eval(js::MARK_CARD, (sel::CARD, sel::CARD_ID_ATTR, id)).await?;
		if !marked {
			return Err(eyre::eyre!("card is no longer on the page").into());
		}
		// let lazy avatars and photo thumbnails paint after the scroll
		tokio::time::sleep(Duration::from_millis(400)).await;
		let shot = self.tab.element_shot(js::MARKED_CARD, CARD_SCALE).await?;
		let geometry = format!("{:?}", shot.geometry);
		tracing::debug!(card = id, geometry, "card shot");
		Ok(Capture {
			png: shot.png,
			captured_at: Timestamp::now(),
			page_url: page_url.to_owned(),
			geometry,
		})
	}

	async fn handle_interstitials(&mut self) -> Result<(), SessionError> {
		self.check_not_blocked()?;
		if !self.tab.url().contains(sel::CONSENT_HOST) {
			return Ok(());
		}
		tracing::info!("consent page: rejecting all");
		if !self.wait_for_any(sel::CONSENT_REJECT).await? {
			return Err(SessionError::new_consent("no reject button"));
		}
		if !self.click_first(sel::CONSENT_REJECT).await? {
			return Err(SessionError::new_consent("could not click reject"));
		}
		// the answer is a cookie in the profile dir, so the next run goes straight through
		let deadline = tokio::time::Instant::now() + self.cfg.ui_timeout.duration();
		while self.tab.url().contains(sel::CONSENT_HOST) {
			if tokio::time::Instant::now() >= deadline {
				return Err(SessionError::new_consent("still on the consent page after rejecting"));
			}
			tokio::time::sleep(Duration::from_millis(300)).await;
		}
		self.check_not_blocked()
	}

	/// Google's "unusual traffic" page: the run fails, whatever was read before it.
	fn check_not_blocked(&self) -> Result<(), SessionError> {
		let url = self.tab.url();
		if url.contains(sel::BLOCKED_PATH) {
			return Err(SessionError::new_blocked(url));
		}
		Ok(())
	}

	async fn dismiss_promo(&mut self) -> Result<(), SessionError> {
		if self.click_first(sel::PROMO_DISMISS).await? {
			tracing::debug!("dismissed the sign-in dialog");
			tokio::time::sleep(Duration::from_millis(500)).await;
		}
		Ok(())
	}

	async fn click_first(&mut self, sels: &[&str]) -> Result<bool, SessionError> {
		self.eval(js::CLICK_FIRST, sels).await
	}

	/// Whether any of `sels` showed up in time.
	async fn wait_for_any(&mut self, sels: &[&str]) -> Result<bool, SessionError> {
		self.wait_for_any_within(sels, self.cfg.ui_timeout.duration()).await
	}

	async fn wait_for_any_within(&mut self, sels: &[&str], timeout: Duration) -> Result<bool, SessionError> {
		self.tab.set_timeout(timeout).await;
		let shown = self.tab.wait_for_any(sels).await;
		self.tab.set_timeout(self.cfg.ui_timeout.duration()).await;
		match shown {
			Ok(_) => Ok(true),
			// what a timed-out page showed was captured and is dropped here: the caller decides whether not showing up is a failure,
			// and only on a page that still answers
			Err(e) if matches!(*e.kind, ErrorKind::Timeout { .. }) => {
				self.check_not_blocked()?;
				self.eval_while::<bool>("waiting for a control", "true", ()).await?;
				Ok(false)
			}
			Err(e) => Err(e.into()),
		}
	}

	pub(super) async fn eval<T: serde::de::DeserializeOwned>(&mut self, func: &str, arg: impl Serialize) -> Result<T, SessionError> {
		self.eval_while("running a script", func, arg).await
	}

	async fn eval_while<T: serde::de::DeserializeOwned>(&mut self, step: &'static str, func: &str, arg: impl Serialize) -> Result<T, SessionError> {
		match tokio::time::timeout(ANSWER, self.tab.eval(func, arg)).await {
			Ok(r) => Ok(r?),
			Err(_) => Err(SessionError::new_unresponsive(step, ANSWER)),
		}
	}
}

fn markup_changed(step: &'static str, sels: &[&str]) -> SessionError {
	SessionError::new_markup_changed(step, sels.iter().map(ToString::to_string).collect())
}

async fn dump(dir: &Path, html: &str) -> eyre::Result<()> {
	tokio::fs::create_dir_all(dir).await?;
	let path = dir.join(format!("cards-{}.html", Timestamp::now().as_millisecond()));
	tokio::fs::write(&path, format!("<div>\n{html}\n</div>\n"))
		.await
		.wrap_err_with(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{Remedy, remedy};

	fn slow_start() -> browser_manipulation::Error {
		let driver = "browserType.launchPersistentContext: Timeout 30000ms exceeded.\n=========================== logs ===========================\n<launching> /nix/store/x/chromium\n    at Object.launch (/nix/store/y/patchright-core/lib/server/browserType.js:1:1)";
		ErrorKind::Launch(playwright_rs::Error::ProtocolError(driver.to_owned())).into()
	}

	fn config(dir: &Path) -> BrowserConfig {
		BrowserConfig {
			artifacts: Some(browser_manipulation::Artifacts {
				dir: dir.to_owned(),
				retention: Duration::from_secs(3600),
			}),
			..BrowserConfig::default()
		}
	}

	/// The `[<path>]` of a message, as the alert layer finds it.
	fn bracketed(message: &str) -> Option<&str> {
		message.split_once('[')?.1.split_once(']').map(|(path, _)| path)
	}

	#[tokio::test]
	async fn a_slow_start_names_its_saved_figures_and_only_the_driver_s_first_line() {
		let dir = tempfile::tempdir().unwrap();
		let cfg = config(&dir.path().join("browser_captures"));

		for _ in 0..2 {
			let e = launch_failure(slow_start(), Duration::from_secs(31), None, &cfg).await;
			assert_eq!(remedy(&e), Remedy::Pause);
			let text = crate::describe(&e);
			let first = text.lines().next().unwrap();
			assert!(first.contains("Timeout 30000ms exceeded.") && !text.contains("browserType.js"), "{text}");
			let report = bracketed(first).unwrap_or_else(|| panic!("no [path] in {first:?}"));
			let figures = std::fs::read_to_string(report).unwrap();
			assert!(figures.contains("limit 30s") && figures.contains("Timeout 30000ms exceeded."), "{figures}");
		}
		assert_eq!(std::fs::read_dir(dir.path().join("browser_captures")).unwrap().count(), 2, "every slow start saves its figures");
	}

	#[tokio::test]
	async fn figures_that_cannot_be_written_say_so_and_the_failure_stays() {
		let dir = tempfile::tempdir().unwrap();
		let blocker = dir.path().join("not-a-dir");
		std::fs::write(&blocker, "").unwrap();
		let cfg = config(&blocker.join("browser_captures"));

		let e = launch_failure(slow_start(), Duration::from_secs(31), None, &cfg).await;

		assert_eq!(remedy(&e), Remedy::Pause);
		let text = format!("{e:#}");
		assert!(text.starts_with("figures not saved: ") && bracketed(&text).is_none(), "{text}");
	}

	#[tokio::test]
	async fn a_permanent_failure_saves_no_figures() {
		let dir = tempfile::tempdir().unwrap();
		let cfg = config(dir.path());
		let missing: browser_manipulation::Error = ErrorKind::Launch(playwright_rs::Error::ServerNotFound).into();

		let e = launch_failure(missing, Duration::from_secs(1), None, &cfg).await;

		assert_eq!(remedy(&e), Remedy::Fatal);
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
	}
}
