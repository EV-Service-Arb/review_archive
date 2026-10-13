//! The memory the pod holds for as long as its Chromium runs.
//!
//! The pod's base limit is far below what Chromium needs; devops' memory-lease broker raises it
//! to the burst only while a lease lives. Without it Chromium thrashes or is killed, so no lease,
//! no Chromium.

use std::{
	future::Future,
	time::{Duration, Instant},
};

use crate::SessionError;

/// Kubelet resizes a pod within seconds; a broker silent for longer than this is not serving.
pub(super) const WAIT: Duration = Duration::from_secs(60);

/// Takes the lease with `acquire`, then runs `start` and times it. Returns the lease beside what
/// `start` gave and how long only `start` took: the time spent waiting for the burst must not
/// count against Chromium's launch limit, or a slow broker would look like a slow host.
///
/// A lease refused, failed, or not granted in `wait` is [`SessionError::NoMemory`], and `start`
/// never runs. Dropping the request on a timeout closes its connection, which the broker sees.
pub(super) async fn around<L, T>(acquire: impl Future<Output = eyre::Result<L>>, wait: Duration, start: impl Future<Output = T>) -> Result<(L, T, Duration), SessionError> {
	let lease = match tokio::time::timeout(wait, acquire).await {
		Ok(Ok(lease)) => lease,
		Ok(Err(e)) => return Err(SessionError::new_no_memory(format!("{e:#}"))),
		Err(_) => return Err(SessionError::new_no_memory(format!("the broker did not grant it in {}s", wait.as_secs()))),
	};
	let started = Instant::now();
	let out = start.await;
	Ok((lease, out, started.elapsed()))
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

	fn flagged() -> (Arc<AtomicBool>, impl Future<Output = ()>) {
		let started = Arc::new(AtomicBool::new(false));
		let flag = started.clone();
		(started, async move { flag.store(true, Ordering::SeqCst) })
	}

	#[tokio::test]
	async fn the_wait_for_a_lease_is_not_part_of_the_start() {
		let acquire = async {
			tokio::time::sleep(Duration::from_millis(300)).await;
			Ok(())
		};
		let start = async { tokio::time::sleep(Duration::from_millis(10)).await };

		let ((), (), took) = around(acquire, Duration::from_secs(5), start).await.unwrap();

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

		let (lease, ended_during_start, _) = around(async move { Ok(Held(flag)) }, Duration::from_secs(5), start).await.unwrap();

		assert!(!ended_during_start && !ended.load(Ordering::SeqCst), "the lease is still held");
		drop(lease);
		assert!(ended.load(Ordering::SeqCst), "dropping it ends the burst");
	}

	#[tokio::test]
	async fn a_refused_lease_is_no_memory_and_chromium_never_starts() {
		let acquire = async { Err::<(), _>(eyre::eyre!("http://lease/x: HTTP/1.0 403 Forbidden")) };
		let (started, start) = flagged();

		let e = around(acquire, Duration::from_secs(5), start).await.unwrap_err();

		assert!(matches!(&e, SessionError::NoMemory { why, .. } if why.contains("403")), "{e:?}");
		assert!(!started.load(Ordering::SeqCst));
	}

	#[tokio::test]
	async fn a_broker_that_never_grants_is_given_up_on_its_request_dropped_and_chromium_never_starts() {
		let ended = Arc::new(AtomicBool::new(false));
		let flag = ended.clone();
		// a request that holds its connection open and never gets "granted"
		let acquire = async move {
			let _pending = Held(flag);
			std::future::pending::<eyre::Result<()>>().await
		};
		let (started, start) = flagged();

		let e = around(acquire, Duration::from_millis(50), start).await.unwrap_err();

		assert!(matches!(e, SessionError::NoMemory { .. }), "{e:?}");
		assert!(!started.load(Ordering::SeqCst));
		assert!(ended.load(Ordering::SeqCst), "the connection is closed, not left waiting in the broker's queue");
	}
}
