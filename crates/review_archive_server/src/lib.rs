//! The review_archive service: the HTTP API and the background loops, over the library's
//! `Archive`. The binary (`review_archive`) adds the CLI and the process bootstrap.
//!
//! A library target too so the client's tests can serve the real router in-process.
#![feature(error_generic_member_access)]

pub mod auth;
pub mod http;
pub mod worker;

/// Reports a failure by who it waits for. One no code knows is a bug: logged at error level
/// with its chain and backtrace, which the tracing layer sends to Sentry, once, and the log
/// pipeline to Discord. The rest are known, and warned: what pauses or halts a source is
/// logged at error level by the archive, once, when it does.
pub fn report(e: &eyre::Report, what: &str) {
	use review_archive::{Rejected, Remedy, describe, remedy};
	let error = describe(e);
	match (e.downcast_ref::<Rejected>().is_some(), remedy(e)) {
		(false, Remedy::Unknown | Remedy::Fatal) => tracing::error!(error, backtrace = backtrace(e), "{what}"),
		(rejected, remedy) => tracing::warn!(rejected, ?remedy, error, "{what}"),
	}
}

/// The trace closest to where `e` began: its innermost typed error's, else eyre's, captured
/// where it became a report. Both need `RUST_LIB_BACKTRACE=1`.
fn backtrace(e: &eyre::Report) -> String {
	use std::backtrace::{Backtrace, BacktraceStatus};
	if let Some(b) = e
		.chain()
		.filter_map(std::error::request_ref::<Backtrace>)
		.filter(|b| b.status() == BacktraceStatus::Captured)
		.last()
	{
		return b.to_string();
	}
	match e.handler().downcast_ref::<color_eyre::Handler>().and_then(color_eyre::Handler::backtrace) {
		Some(b) => format!("{b:?}"),
		None => "not captured (RUST_LIB_BACKTRACE unset)".to_owned(),
	}
}
