//! A Chromium that was too slow to start fails the scan, not the service: the run says why,
//! the target backs off, and no other target is held up by it.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use review_archive::{
	Archive, Remedy, SessionError,
	config::Config,
	core::{Known, Scan, Target, TargetId, dto::NewTarget, tokens::Meter},
	describe, remedy,
	sources::ReviewSource,
};

struct SlowStart;

impl ReviewSource for SlowStart {
	async fn scan(&self, _: &Target, _: &Known, _: &mut Meter) -> eyre::Result<Scan> {
		Err(SessionError::new_launch_timeout(
			"Timeout: Playwright initialization timeout after 30 seconds".to_owned(),
			Duration::from_secs(31),
			Duration::from_secs(30),
			"host: node io pressure some avg10=41.20 avg60=30.00".to_owned(),
		)
		.into())
	}
}

async fn add(archive: &Archive, place: &str) -> Target {
	let t = NewTarget {
		place: Some(place.to_owned()),
		..Default::default()
	};
	archive.add_target(&t).await.unwrap().target
}

async fn due(archive: &Archive, at: Timestamp) -> Vec<TargetId> {
	archive.due(at).await.unwrap().iter().map(|t| t.id).collect()
}

#[tokio::test]
async fn a_slow_start_is_a_failed_run_with_its_host_and_the_target_backs_off() {
	let dir = tempfile::tempdir().unwrap();
	let archive = Archive::open(Config {
		data_dir: Some(dir.path().to_owned()),
		..Config::default()
	})
	.await
	.unwrap();
	let slow = add(&archive, "ChIJlaunchtimeout000001").await;
	let other = add(&archive, "ChIJlaunchtimeout000002").await;

	let recorded = archive.record(&SlowStart, &slow).await.unwrap();
	let failure = recorded.failure.expect("the scan failed");
	assert_eq!(remedy(&failure), Remedy::Retry);

	let error = recorded.summary.error.expect("the run says why");
	assert!(error.contains("avg10=41.20") && error.contains("code: review_archive::browser::launch_timeout"), "{error}");
	assert_eq!(error, describe(&failure));

	// no breaker: Google did not flag us, so the other target is still scanned
	let now = Timestamp::now();
	assert_eq!(due(&archive, now).await, [other.id]);
	// the failed target comes back on its own backoff (an hour, with jitter)
	assert!(due(&archive, now + SignedDuration::from_mins(90)).await.contains(&slow.id));
}
