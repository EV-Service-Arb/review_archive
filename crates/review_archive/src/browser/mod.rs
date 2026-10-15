//! The one headless Chromium the `maps` source (and the screenshots of `gbp`) run in.
//!
//! A plain browser at a polite rate: no stealth, no fingerprint masking, no proxies. When
//! Google does not serve the full page, the walk fails and says why.

mod host;
mod launch_report;
mod lease;
mod profile;
mod session;

use std::{
	collections::BTreeSet,
	path::{Path, PathBuf},
	sync::Arc,
};

use review_archive_core::{
	Warning,
	maps::{Card, WalkEnd, WalkPolicy, Walked, selectors::js},
	tokens::Meter,
};

use self::{profile::ProfileLock, session::Session};
use crate::{SessionError, config::BrowserConfig};

/// A handle on one Chromium, launched on first use and shared by every clone.
///
/// Walks through one handle run one at a time: the browser has one tab. Callers that
/// already run a scan loop can create one and hand clones to [`Archive::open_with_browser`](crate::Archive::open_with_browser)
/// and to their own code. One profile dir takes one browser at a time, across processes
/// too: a second one fails with the reason.
#[derive(Clone)]
pub struct Browser {
	inner: Arc<Inner>,
}

struct Inner {
	cfg: BrowserConfig,
	profiles: Vec<PathBuf>,
	state: tokio::sync::Mutex<State>,
}

/// The profile, held from [`Browser::claim`] or the first walk until [`Browser::close`].
#[derive(Default)]
struct State {
	signed_out: BTreeSet<PathBuf>,
	/// On [`State::current`]'s profile.
	lock: Option<ProfileLock>,
	session: Option<Session>,
}

impl State {
	fn current<'p>(&self, profiles: &'p [PathBuf]) -> Option<&'p PathBuf> {
		profiles.iter().find(|p| !self.signed_out.contains(*p))
	}

	async fn release(&mut self) {
		if let Some(s) = self.session.take() {
			s.close().await;
		}
		self.lock = None;
	}
}

impl std::fmt::Debug for Browser {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Browser").field("profiles", &self.inner.profiles).finish_non_exhaustive()
	}
}

impl Browser {
	/// A browser on these profile dirs, each signed in to its own Google account, the one walks
	/// go to first first; a profile is what remembers the consent answer between runs. Nothing
	/// starts until the first walk.
	pub fn new(cfg: BrowserConfig, profiles: Vec<PathBuf>) -> Self {
		assert!(!profiles.is_empty(), "a browser needs a profile");
		Self {
			inner: Arc::new(Inner {
				cfg,
				profiles: profiles
					.iter()
					.map(|p| std::path::absolute(p).expect("the working directory exists")) // Chromium refuses a relative one
					.collect(),
				state: tokio::sync::Mutex::default(),
			}),
		}
	}

	/// Takes the profile walks go to for this process without starting Chromium: fails, with
	/// the reason, while another process holds it. Held until [`Self::close`]. With every
	/// account signed out there is none to take, and walks fail.
	pub async fn claim(&self) -> eyre::Result<()> {
		let mut state = self.inner.state.lock().await;
		if state.lock.is_none()
			&& let Some(p) = state.current(&self.inner.profiles)
		{
			state.lock = Some(ProfileLock::acquire(p)?);
		}
		Ok(())
	}

	pub(crate) fn profiles(&self) -> &[PathBuf] {
		&self.inner.profiles
	}

	/// The profile walks go to; `None` once every account is signed out.
	pub(crate) async fn current(&self) -> Option<PathBuf> {
		let state = self.inner.state.lock().await;
		state.current(&self.inner.profiles).cloned()
	}

	pub(crate) async fn signed_out(&self) -> BTreeSet<PathBuf> {
		self.inner.state.lock().await.signed_out.clone()
	}

	/// The accounts signed out are these, as the store says: one a person signed in again is walked on again.
	pub(crate) async fn sync(&self, signed_out: BTreeSet<PathBuf>) {
		let mut state = self.inner.state.lock().await;
		let was = state.current(&self.inner.profiles).cloned();
		state.signed_out = signed_out;
		if state.current(&self.inner.profiles) != was.as_ref() {
			state.release().await;
		}
	}

	/// Walks no longer go to `profile`.
	pub(crate) async fn sign_out(&self, profile: &Path) {
		let mut state = self.inner.state.lock().await;
		if state.current(&self.inner.profiles).is_some_and(|p| p == profile) {
			state.release().await;
		}
		state.signed_out.insert(profile.to_owned());
	}

	/// Opens Maps on `profile` and says whether it is signed in to Google.
	pub(crate) async fn touch(&self, profile: &Path) -> eyre::Result<bool> {
		let mut state = self.inner.state.lock().await;
		// one Chromium at a time, as the memory lease is sized for one; and `profile` may be the one walks hold
		state.release().await;
		let _lock = ProfileLock::acquire(profile)?;
		let session = Session::launch(&self.inner.cfg, profile).await?;
		let signed_out = async {
			let mut page = session.page().await?;
			let signed_out = page.signed_out().await;
			page.close().await?;
			signed_out
		}
		.await;
		session.close().await;
		Ok(!signed_out?)
	}

	/// Opens the place's review list, sorted newest first, and walks it: `policy` says
	/// which cards to screenshot and when to stop; `max` caps the cards read, `meter` what
	/// it spends — failed or not, it holds what Google was sent. An account found signed out
	/// hands the walk to the next profile, from the start; with none left, it fails as signed out.
	pub async fn walk(&self, place_id: &str, lang: &str, policy: &mut dyn WalkPolicy, meter: &mut Meter, max: usize) -> eyre::Result<Walked> {
		let mut state = self.inner.state.lock().await;
		//LOOP: one turn per account, and they are finite
		loop {
			let Some(profile) = state.current(&self.inner.profiles).cloned() else {
				return Err(SessionError::new_signed_out().into());
			};
			let mut counted = Counted { policy: &mut *policy, seen: 0 };
			let walked = self.walk_on(&mut state, place_id, lang, &mut counted, meter, max).await;
			if !walked.as_ref().is_err_and(|e| matches!(e.downcast_ref::<SessionError>(), Some(SessionError::SignedOut { .. }))) {
				return walked;
			}
			// signed out shows on the overview or at the sort menu, before any card is read
			assert_eq!(counted.seen, 0, "a walk restarted on the next account would count its cards twice");
			tracing::warn!(profile = %profile.display(), place_id, "signed out of Google: the walk goes on the next account");
			state.release().await;
			state.signed_out.insert(profile);
		}
	}

	async fn walk_on(&self, state: &mut State, place_id: &str, lang: &str, policy: &mut dyn WalkPolicy, meter: &mut Meter, max: usize) -> eyre::Result<Walked> {
		let profile = state.current(&self.inner.profiles).expect("the caller walks on a live account");
		if state.lock.is_none() {
			state.lock = Some(ProfileLock::acquire(profile)?);
		}
		if state.session.is_none() {
			state.session = Some(Session::launch(&self.inner.cfg, profile).await?);
		}
		let session = state.session.as_ref().expect("launched just above");
		let (walked, broken, js_heap) = match session.page().await {
			Ok(mut page) => {
				let walked = match page.open_reviews(place_id, lang, &mut *policy, meter).await {
					Ok(Ok(opened)) => page.walk(policy, meter, max, opened).await,
					Ok(Err(over)) => Ok(over),
					Err(e) => Err(e),
				};
				let walked = match walked {
					Err(e) => {
						let saved = match &e {
							SessionError::Browser { source, .. } => source.capture.as_deref().cloned(),
							_ => page.save("walk-failed").await,
						};
						match saved {
							Some(saved) => Err(eyre::Report::new(e).wrap_err(page_line(saved))),
							None => Err(e.into()),
						}
					}
					Ok(mut w) if w.end == WalkEnd::Interrupted => {
						w.warnings.extend(page.save("walk-interrupted").await.map(|s| Warning::Failure(page_line(s))));
						Ok(w)
					}
					Ok(w) => Ok(w),
				};
				// a page that cannot say has failed the walk already, which says why
				let js_heap = match page.eval::<(u64, u64)>(js::JS_HEAP, ()).await {
					Ok((used, limit)) => format!("{} of {} MiB", used >> 20, limit >> 20),
					Err(e) => format!("not read: {e}"),
				};
				match (page.close().await, walked) {
					(Ok(()), walked) => (walked, false, js_heap),
					(Err(e), Ok(mut w)) => {
						w.warnings.push(Warning::Failure(format!("closing the tab after the walk: {e:#}")));
						(Ok(w), true, js_heap)
					}
					// the walk's own failure says more than the tab that would not close after it
					(Err(_), Err(e)) => (Err(e), true, js_heap),
				}
			}
			Err(e) => (Err(e.into()), true, "no page".to_owned()),
		};
		// A browser that failed under a walk may be dead (a crashed tab, a lost driver):
		// the next walk starts a fresh one rather than fail on it forever.
		let host = host::Snapshot::capture(session.leased).await.to_string();
		tracing::info!(host, js_heap, place_id, failed = walked.is_err(), "the walk's memory, to size the lease by");
		if (broken || matches!(walked, Err(_) | Ok(Walked { end: WalkEnd::Interrupted, .. })))
			&& let Some(s) = state.session.take()
		{
			s.close().await;
		}
		walked
	}

	/// Stops Chromium, if it runs, and lets go of the profile. The next walk starts a new
	/// one; a service that scans every few hours closes it in between rather than keep it
	/// idle.
	pub async fn close(&self) {
		self.inner.state.lock().await.release().await;
	}
}

/// The policy, and how many cards it saw.
struct Counted<'p> {
	policy: &'p mut dyn WalkPolicy,
	seen: usize,
}

impl WalkPolicy for Counted<'_> {
	fn wants_list(&self, total: Option<u64>) -> bool {
		self.policy.wants_list(total)
	}

	fn wants_capture(&self, card: &Card) -> bool {
		self.policy.wants_capture(card)
	}

	fn observe(&mut self, card: &Card) {
		self.seen += 1;
		self.policy.observe(card);
	}

	fn satisfied(&self) -> bool {
		self.policy.satisfied()
	}
}

/// Where the page a walk ended on was saved, as `[<path>]`s the alerts attach; or why it was not.
fn page_line(saved: Result<browser_manipulation::Capture, String>) -> String {
	match saved {
		Ok(c) => format!("page [{}] [{}]", c.png.display(), c.html.display()),
		Err(e) => format!("page not saved: {e}"),
	}
}
