//! The memory the pod holds for as long as its Chromium runs.
//!
//! The pod's base limit is far below what Chromium needs; devops' memory-lease broker raises it
//! to the burst only while a lease lives. The lease is a means of keeping Chromium alive, so a
//! broker that is down, refuses or never answers must cost the scan nothing but a warning.

use std::{
	future::Future,
	time::{Duration, Instant},
};

/// Kubelet resizes a pod within seconds; a broker silent for longer than this is down, and the
/// scan is worth more than the burst. The wait is not part of the start's own limit.
pub(super) const WAIT: Duration = Duration::from_secs(60);

/// Takes the lease with `acquire`, then runs `start` and times it. Returns the lease (`None`
/// when there is none to hold) beside what `start` gave and how long only `start` took: the
/// time spent waiting for the burst must not count against Chromium's launch limit, or a slow
/// broker would look like a slow host.
///
/// Dropping the future while it waits drops the lease request, which the broker sees as a
/// closed connection.
pub(super) async fn around<L, T>(acquire: impl Future<Output = eyre::Result<L>>, wait: Duration, start: impl Future<Output = T>) -> (Option<L>, T, Duration) {
	let lease = hold(acquire, wait).await;
	let started = Instant::now();
	let out = start.await;
	(lease, out, started.elapsed())
}

/// `None`, with one warning saying why, when the lease could not be had in `wait`. Without
/// `MEMORY_LEASE_URL` `acquire` succeeds with an inert lease, so a warning here always means
/// a configured broker that did not serve.
async fn hold<L>(acquire: impl Future<Output = eyre::Result<L>>, wait: Duration) -> Option<L> {
	match tokio::time::timeout(wait, acquire).await {
		Ok(Ok(lease)) => Some(lease),
		Ok(Err(e)) => {
			tracing::warn!(error = format!("{e:#}"), "no memory lease; Chromium runs in the pod's base memory");
			None
		}
		Err(_) => {
			tracing::warn!(?wait, "the memory lease broker did not answer; Chromium runs in the pod's base memory");
			None
		}
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	};

	use super::*;

	/// Records when it is dropped, as a real lease ends the burst.
	struct Held(Arc<AtomicBool>);

	impl Drop for Held {
		fn drop(&mut self) {
			self.0.store(true, Ordering::SeqCst);
		}
	}

	#[tokio::test]
	async fn the_wait_for_a_lease_is_not_part_of_the_start() {
		let acquire = async {
			tokio::time::sleep(Duration::from_millis(300)).await;
			Ok(())
		};
		let start = async { tokio::time::sleep(Duration::from_millis(10)).await };

		let (lease, (), took) = around(acquire, Duration::from_secs(5), start).await;

		assert!(lease.is_some());
		assert!(took < Duration::from_millis(200), "{took:?}");
	}

	#[tokio::test]
	async fn a_lease_is_held_through_the_start_and_returned_to_the_caller() {
		let ended = Arc::new(AtomicBool::new(false));
		let flag = ended.clone();
		let start = {
			let ended = ended.clone();
			async move { ended.load(Ordering::SeqCst) }
		};

		let (lease, ended_during_start, _) = around(async move { Ok(Held(flag)) }, Duration::from_secs(5), start).await;

		assert!(!ended_during_start && !ended.load(Ordering::SeqCst), "the lease is still held");
		drop(lease);
		assert!(ended.load(Ordering::SeqCst), "dropping it ends the burst");
	}

	#[tokio::test]
	async fn a_broker_that_refuses_costs_a_warning_and_the_start_goes_on() {
		let acquire = async { Err::<(), _>(eyre::eyre!("http://lease/x: HTTP/1.0 503 Service Unavailable")) };

		let (lease, out, _) = around(acquire, Duration::from_secs(5), async { "started" }).await;

		assert!(lease.is_none());
		assert_eq!(out, "started");
	}

	#[tokio::test]
	async fn a_broker_that_never_answers_is_given_up_on_and_its_request_dropped() {
		let ended = Arc::new(AtomicBool::new(false));
		let flag = ended.clone();
		// a request that holds its connection open and never gets "granted"
		let acquire = async move {
			let _pending = Held(flag);
			std::future::pending::<eyre::Result<()>>().await
		};

		let (lease, out, _) = around(acquire, Duration::from_millis(50), async { "started" }).await;

		assert!(lease.is_none());
		assert_eq!(out, "started");
		assert!(ended.load(Ordering::SeqCst), "the connection is closed, not left waiting in the broker's queue");
	}
}
