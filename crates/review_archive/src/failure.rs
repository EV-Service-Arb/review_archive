//! The engine's typed errors, what a failure says, and who it waits for.
#![allow(missing_docs)] // v_utils' `wrap_err` generates constructors and trace fields without docs

use std::fmt::Write as _;

use v_utils::macros::wrap_err;

use crate::Rejected;

/// Who a failure waits for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Remedy {
	/// Passes by itself; the target's backoff retries it.
	Retry,
	/// Maps cannot work now (Google flagged us, or Chromium cannot run): every Maps walk
	/// pauses, and one probes later.
	Pause,
	/// Retrying cannot fix it; a person has to.
	Human,
	/// The process cannot do its work.
	Fatal,
	/// Not a failure this code knows: retried on the backoff like [`Self::Retry`], and a bug.
	Unknown,
}

/// Who `e` waits for.
pub fn remedy(e: &eyre::Report) -> Remedy {
	#[cfg(feature = "maps")]
	if let Some(e) = e.downcast_ref::<SessionError>() {
		use browser_manipulation::ErrorKind;
		return match e {
			SessionError::Blocked { .. } | SessionError::LimitedView { .. } | SessionError::LaunchTimeout { .. } | SessionError::NoMemory { .. } | SessionError::Unresponsive { .. } =>
				Remedy::Pause,
			SessionError::MarkupChanged { .. } | SessionError::Consent { .. } => Remedy::Human,
			// a person fixes the target and its card says why, but one target's place must not halt
			// every Maps walk; a place panel missing is also what a page that did not finish looks like
			SessionError::PlaceNotFound { .. } => Remedy::Retry,
			SessionError::Launch { .. } | SessionError::SignedOut { .. } => Remedy::Fatal,
			SessionError::Browser { source, .. } => match &*source.kind {
				// a slow start is `LaunchTimeout` by then: what is left is the setup
				ErrorKind::Launch(_) | ErrorKind::DriverMismatch { .. } | ErrorKind::ProfileInUse(_) => Remedy::Fatal,
				ErrorKind::Navigation { .. } | ErrorKind::Timeout { .. } => Remedy::Retry,
				ErrorKind::Io { .. } | ErrorKind::Selector { .. } | ErrorKind::Eval { .. } | ErrorKind::Driver { .. } => Remedy::Unknown,
			},
			SessionError::Other(_) => Remedy::Unknown,
		};
	}
	#[cfg(feature = "maps")]
	if let Some(e) = e.downcast_ref::<GbpError>() {
		return match e {
			GbpError::InvalidGrant { .. } | GbpError::Unauthorized { .. } | GbpError::NoCredentials { .. } => Remedy::Human,
			GbpError::Api { status, .. } if *status == 429 || *status >= 500 => Remedy::Retry,
			GbpError::Http { source, .. } if source.is_timeout() || source.is_connect() || source.is_request() => Remedy::Retry,
			GbpError::Api { .. } | GbpError::Http { .. } | GbpError::Other(_) => Remedy::Unknown,
		};
	}
	Remedy::Unknown
}

/// Whether a failed Chromium start says the host was too slow rather than the setup broken:
/// the client's own `Timeout` (the driver's 30 s initialization), or the driver's
/// `Timeout <n>ms exceeded` (Chromium's launch). The driver reports the second as a
/// `ProtocolError` that is only a message, so that one is told by its text. Everything else —
/// a missing driver or executable, a dead pipe — is fatal, as is an error kind that a later
/// playwright-rs adds: restarting the process is the only thing that tells a person the setup
/// is wrong.
#[cfg(feature = "maps")]
pub(crate) fn launch_is_transient(source: &playwright_rs::Error) -> bool {
	use playwright_rs::Error;
	match source {
		Error::Timeout(_) => true,
		Error::ProtocolError(message) => message.contains("Timeout ") && message.contains("ms exceeded"),
		// the rest of a launch's failures are permanent; `non_exhaustive` needs the arm anyway
		_ => false,
	}
}

/// `e` for a text sink (the log, a run's `error`, a webhook): the chain on the first line,
/// then the diagnostic's help and code. `[<path>]`s in it are what the alerts attach.
pub fn describe(e: &eyre::Report) -> String {
	let mut out = format!("{e:#}");
	let diagnostic: Option<&dyn miette::Diagnostic> = None
		.or_else(|| e.downcast_ref::<Rejected>().map(|d| d as _))
		.or_else(|| e.downcast_ref::<PlacesError>().map(|d| d as _));
	#[cfg(feature = "maps")]
	let diagnostic = diagnostic
		.or_else(|| e.downcast_ref::<SessionError>().map(|d| d as _))
		.or_else(|| e.downcast_ref::<GbpError>().map(|d| d as _));
	if let Some(d) = diagnostic {
		if let Some(help) = d.help() {
			write!(out, "\nhelp: {help}").expect("writing to a String");
		}
		if let Some(code) = d.code() {
			write!(out, "\ncode: {code}").expect("writing to a String");
		}
	}
	out
}

#[cfg(feature = "maps")]
/// A Maps page that did not go where the walk needed it to. The page it was on is saved
/// by the walk, which says where around this error.
#[wrap_err]
#[derive(Debug, miette::Diagnostic, thiserror::Error)]
pub enum SessionError {
	#[leaf]
	#[error("blocked by Google (\"unusual traffic\" page at {url})")]
	#[diagnostic(code(review_archive::google::blocked), help("Google flagged the address we scan from: every Maps walk pauses, and one probes later"))]
	Blocked { url: String },
	#[leaf]
	#[error("Google served its \"limited view\" of Maps, which has no reviews")]
	#[diagnostic(
		code(review_archive::google::limited_view),
		help("this browser session is not trusted with the full page: every Maps walk pauses, and one probes later")
	)]
	LimitedView,
	#[leaf]
	#[error("the scanner's browser is signed out of Google, which then shows a place's first few reviews only, unsorted")]
	#[diagnostic(
		code(review_archive::google::signed_out),
		help("sign the browser profile (`browser.profile_dir`, else `<data_dir>/chromium-profile`) in to a Google account; `serve` exits 78, for the deployment to be held until it is")
	)]
	SignedOut,
	#[leaf]
	#[error("{step}: none of {selectors:?} on the page")]
	#[diagnostic(
		code(review_archive::maps::markup_changed),
		help("the Maps markup changed: fix selectors.rs against fixtures refreshed with `scan --dump-html`; Maps stays halted until a restart")
	)]
	MarkupChanged { step: &'static str, selectors: Vec<String> },
	#[leaf]
	#[error("Google shows no place for {place_id}")]
	#[diagnostic(
		code(review_archive::maps::place_not_found),
		help("the place was removed or merged into another: check the target's place id, or disable it")
	)]
	PlaceNotFound { place_id: String },
	#[leaf]
	#[error("consent page: {why}")]
	#[diagnostic(
		code(review_archive::maps::consent),
		help("the consent page changed: fix CONSENT_* in selectors.rs; Maps stays halted until a restart")
	)]
	Consent { why: &'static str },
	#[leaf]
	#[error("Chromium did not start: {reason}")]
	#[diagnostic(code(review_archive::browser::launch), help("check `browser.executable` and the memory the process has"))]
	Launch { reason: String },
	#[leaf]
	#[error("Chromium did not start in {:.1}s (limit {}s): {reason}; {host}", .took.as_secs_f64(), .limit.as_secs())]
	#[diagnostic(
		code(review_archive::browser::launch_timeout),
		help(
			"with the memory it needs Chromium starts in 4-6 s, so look at the pod's first: the lease's size against what a walk peaks at, the cgroup's `max` and `oom_kill` counters and its memory pressure; only then the node's. Every Maps walk pauses, and one probes later"
		)
	)]
	LaunchTimeout {
		reason: String,
		took: std::time::Duration,
		limit: std::time::Duration,
		host: String,
	},
	#[leaf]
	#[error("no memory for Chromium: {why}")]
	#[diagnostic(
		code(review_archive::browser::no_memory),
		help(
			"Chromium is not started without its memory lease: see the memory-lease broker's log (devops crates/memory-lease, `MEMORY_LEASE_URL`) and review-archive's `memoryBudgets` entry in devops' flake.nix. Every Maps walk pauses, and one probes later"
		)
	)]
	NoMemory { why: String },
	#[leaf]
	#[error("the page did not answer in {}s while {step}", .waited.as_secs())]
	#[diagnostic(
		code(review_archive::browser::unresponsive),
		help(
			"Chromium is starved, most often of memory: see the walk's host figures in the log (`peak` against `leased`, the `max` counter, per-process PSS). Nothing on the page was judged. Every Maps walk pauses, and one probes later"
		)
	)]
	Unresponsive { step: &'static str, waited: std::time::Duration },
	#[foreign]
	#[error("driving Chromium")]
	#[diagnostic(forward(source))]
	Browser(browser_manipulation::Error),
	#[error(transparent)]
	Other(#[from] eyre::Report),
}

#[cfg(feature = "maps")]
/// The Business Profile API failing us.
#[wrap_err]
#[derive(Debug, miette::Diagnostic, thiserror::Error)]
pub enum GbpError {
	/// Google no longer takes the refresh token, or the OAuth client.
	#[leaf]
	#[error("refreshing the GBP access token: {status}: {body}")]
	#[diagnostic(
		code(review_archive::gbp::invalid_grant),
		help("re-issue GBP_REFRESH_TOKEN (revoked, expired, or issued to another GBP_CLIENT_ID); gbp scans stay halted until a restart")
	)]
	InvalidGrant {
		/// The token endpoint's status.
		status: u16,
		/// What it said.
		body: String,
	},
	/// A fresh access token is refused too.
	#[leaf]
	#[error("GET {url}: still unauthorized after refreshing the access token")]
	#[diagnostic(
		code(review_archive::gbp::unauthorized),
		help("the token lacks the business.manage scope, or its account lost the location; gbp scans stay halted until a restart")
	)]
	Unauthorized {
		/// What was asked for.
		url: String,
	},
	/// The credentials are not configured.
	#[leaf]
	#[error("GBP_CLIENT_ID, GBP_CLIENT_SECRET and GBP_REFRESH_TOKEN are not all set, and gbp targets need them")]
	#[diagnostic(code(review_archive::gbp::no_credentials), help("set them; gbp scans stay halted until a restart"))]
	NoCredentials,
	/// Any other refusal.
	#[leaf]
	#[error("{what}: {status}: {body}")]
	#[diagnostic(code(review_archive::gbp::api))]
	Api {
		/// The call.
		what: String,
		/// Its status.
		status: u16,
		/// What it said.
		body: String,
	},
	/// The request itself failed, or its answer did not decode.
	#[foreign]
	#[diagnostic(code(review_archive::gbp::http))]
	Http(reqwest::Error),
	/// Anything else.
	#[error(transparent)]
	Other(#[from] eyre::Report),
}

/// The Places API failing us.
#[wrap_err]
#[derive(Debug, miette::Diagnostic, thiserror::Error)]
pub enum PlacesError {
	/// It refused the search.
	#[leaf]
	#[error("Places text search for {query:?}: {status}: {body}")]
	#[diagnostic(code(review_archive::places::api), help("check GOOGLE_MAPS_KEY, and that the Places API (New) is enabled for its project"))]
	Api {
		/// What was searched for.
		query: String,
		/// Its status.
		status: u16,
		/// What it said.
		body: String,
	},
	/// The request itself failed, or its answer did not decode.
	#[foreign]
	#[diagnostic(code(review_archive::places::http))]
	Http(reqwest::Error),
}

#[cfg(all(test, feature = "maps"))]
mod tests {
	use std::time::Duration;

	use browser_manipulation::ErrorKind;

	use super::*;

	fn report(e: SessionError) -> eyre::Report {
		eyre::Report::new(e).wrap_err("scan of target 95 (somewhere)")
	}

	fn browser(kind: ErrorKind) -> eyre::Report {
		report(SessionError::from(browser_manipulation::Error::from(kind)))
	}

	#[test]
	fn chromium_that_cannot_run_pauses_maps() {
		let e = report(SessionError::new_launch_timeout(
			"Timeout: Playwright initialization timeout after 30 seconds".to_owned(),
			Duration::from_secs(31),
			Duration::from_secs(30),
			"host: node io pressure some avg10=40.00 avg60=35.00".to_owned(),
		));
		assert_eq!(remedy(&e), Remedy::Pause);
		assert_eq!(remedy(&report(SessionError::new_no_memory("403".to_owned()))), Remedy::Pause);
	}

	#[test]
	fn a_setup_that_is_broken_still_stops_the_process() {
		assert_eq!(remedy(&report(SessionError::new_launch("`browser.executable` is not set".to_owned()))), Remedy::Fatal);
		assert_eq!(remedy(&browser(ErrorKind::ProfileInUse("/data/chromium-profile".into()))), Remedy::Fatal);
		let mismatch = ErrorKind::DriverMismatch {
			cli_js: "/nix/store/x/cli.js".to_owned(),
			found: "1.0.0".to_owned(),
		};
		assert_eq!(remedy(&browser(mismatch)), Remedy::Fatal);
	}

	#[test]
	fn the_network_is_retried_and_what_is_not_known_is_a_bug() {
		let navigation = ErrorKind::Navigation {
			url: "https://x".to_owned(),
			source: playwright_rs::Error::ProtocolError("net::ERR_NAME_NOT_RESOLVED".to_owned()),
		};
		assert_eq!(remedy(&browser(navigation)), Remedy::Retry);
		let io = ErrorKind::Io {
			what: "reading",
			path: "/tmp/x".into(),
			source: std::io::Error::other("gone"),
		};
		assert_eq!(remedy(&browser(io)), Remedy::Unknown);
		assert_eq!(remedy(&report(SessionError::Other(eyre::eyre!("?")))), Remedy::Unknown);
		assert_eq!(remedy(&eyre::eyre!("an error no one typed")), Remedy::Unknown);
	}

	#[test]
	fn only_timeouts_are_transient_launch_failures() {
		use playwright_rs::Error;

		let driver_timeout = "browserType.launchPersistentContext: Timeout 30000ms exceeded.\n=========================== logs ===========================\n";
		for transient in [
			Error::Timeout("Playwright initialization timeout after 30 seconds".to_owned()),
			Error::ProtocolError(driver_timeout.to_owned()),
		] {
			assert!(launch_is_transient(&transient), "{transient}");
		}

		let io = || std::io::Error::other("gone");
		for permanent in [
			Error::ServerNotFound,
			Error::LaunchFailed("spawn node ENOENT".to_owned()),
			Error::DriverMisconfigured("PLAYWRIGHT_CLI_JS /x does not exist".to_owned()),
			Error::BrowserNotInstalled {
				browser_name: "chromium".to_owned(),
				message: "Executable doesn't exist at /usr/bin/chromium".to_owned(),
				playwright_version: "1.0.0".to_owned(),
			},
			Error::ProtocolError("browserType.launchPersistentContext: Failed to launch: spawn ENOMEM".to_owned()),
			Error::ServerError("driver crashed".to_owned()),
			Error::ConnectionFailed("refused".to_owned()),
			Error::TransportError("broken pipe".to_owned()),
			Error::ChannelClosed,
			Error::Io(io()),
			Error::NavigationTimeout {
				url: "https://x".to_owned(),
				duration_ms: 30_000,
			},
			Error::AssertionTimeout("never visible".to_owned()),
			Error::InvalidArgument("viewport".to_owned()),
		] {
			assert!(!launch_is_transient(&permanent), "{permanent}");
			assert_eq!(remedy(&browser(ErrorKind::Launch(permanent))), Remedy::Fatal);
		}
	}

	#[test]
	fn a_slow_start_describes_its_host_and_what_to_do() {
		let e = report(SessionError::new_launch_timeout(
			"Timeout: Playwright initialization timeout after 30 seconds".to_owned(),
			Duration::from_millis(30_400),
			Duration::from_secs(30),
			"host: node io pressure some avg10=40.00 avg60=35.00".to_owned(),
		));
		let text = describe(&e);
		let first = text.lines().next().unwrap();
		assert!(first.contains("did not start in 30.4s (limit 30s)") && first.contains("avg10=40.00"), "{text}");
		assert!(text.contains("\nhelp: ") && text.ends_with("code: review_archive::browser::launch_timeout"), "{text}");
	}
}
