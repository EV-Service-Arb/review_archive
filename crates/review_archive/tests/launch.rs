//! A Chromium that cannot run — too slow to start, no memory to start in, or a page that stopped
//! answering — is Maps unable to work, as a block is: the run says why, every Maps walk pauses through a restart, and one probe
//! ends it.

use std::{path::Path, time::Duration};

use jiff::{SignedDuration, Timestamp};
use review_archive::{
	Archive, Rejected, Remedy, SessionError,
	config::Config,
	core::{Coverage, Known, Scan, Target, TargetId, dto::NewTarget, tokens::Meter},
	describe, remedy,
	sources::ReviewSource,
};

struct SlowStart;

impl ReviewSource for SlowStart {
	async fn scan(&self, _: &Target, _: &Known, _: &mut Meter) -> eyre::Result<Scan> {
		Err(SessionError::new_launch_timeout(
			"Timeout 30000ms exceeded.".to_owned(),
			Duration::from_secs(31),
			Duration::from_secs(30),
			"host: cgroup memory 214 of 214 MiB incl. page cache".to_owned(),
		)
		.into())
	}
}

struct NoMemory;

impl ReviewSource for NoMemory {
	async fn scan(&self, _: &Target, _: &Known, _: &mut Meter) -> eyre::Result<Scan> {
		Err(SessionError::new_no_memory("http://memory-lease/lease: HTTP/1.0 403 Forbidden".to_owned()).into())
	}
}

struct Stalled;

impl ReviewSource for Stalled {
	async fn scan(&self, _: &Target, _: &Known, _: &mut Meter) -> eyre::Result<Scan> {
		Err(SessionError::new_unresponsive("waiting for a control", Duration::from_secs(20)).into())
	}
}

struct Served;

impl ReviewSource for Served {
	async fn scan(&self, _: &Target, _: &Known, _: &mut Meter) -> eyre::Result<Scan> {
		Ok(Scan {
			reviews: vec![],
			coverage: Coverage::Complete,
			warnings: vec![],
			cut_after: None,
			listed: None,
			post: None,
			name: None,
		})
	}
}

async fn open(dir: &Path) -> Archive {
	Archive::open(Config {
		data_dir: Some(dir.to_owned()),
		..Config::default()
	})
	.await
	.unwrap()
}

async fn due(archive: &Archive, at: Timestamp) -> Vec<TargetId> {
	let mut due: Vec<TargetId> = archive.due(at).await.unwrap().iter().map(|t| t.id).collect();
	due.sort();
	due
}

async fn pauses_maps_until_a_probe(failing: impl ReviewSource, said: &str) {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let mut targets = Vec::new();
	for i in 0..3 {
		let t = NewTarget {
			place: Some(format!("ChIJlaunchtest{i:0>12}")),
			..Default::default()
		};
		targets.push(archive.add_target(&t).await.unwrap().target);
	}

	let recorded = archive.record(&failing, &targets[0]).await.unwrap();
	let failure = recorded.failure.expect("the scan failed");
	assert_eq!(remedy(&failure), Remedy::Pause);
	let error = recorded.summary.error.expect("the run says why");
	assert!(error.contains(said), "{error}");
	assert_eq!(error, describe(&failure));

	let now = Timestamp::now();
	assert_eq!(due(&archive, now).await, [], "no Maps target is walked while Chromium cannot run");
	let refused = archive.enqueue_scan(targets[1].id).await.unwrap_err();
	let Some(Rejected::Busy(why)) = refused.downcast_ref::<Rejected>() else {
		panic!("{refused:#}")
	};
	assert!(why.contains(said) && !why.contains("Google"), "the pause names its reason: {why}");

	let archive = open(dir.path()).await;
	assert_eq!(due(&archive, now).await, []);

	let at_probe = due(&archive, now + SignedDuration::from_mins(61)).await;
	let [probe] = at_probe[..] else { panic!("one probe: {at_probe:?}") };
	let probe = targets.iter().find(|t| t.id == probe).unwrap();
	archive.record(&Served, probe).await.unwrap();
	let mut open_again: Vec<TargetId> = targets.iter().map(|t| t.id).filter(|id| ![targets[0].id, probe.id].contains(id)).collect();
	open_again.sort();
	assert_eq!(due(&archive, now).await, open_again);
}

#[tokio::test]
async fn a_slow_start_pauses_maps_until_a_probe() {
	pauses_maps_until_a_probe(SlowStart, "launch_timeout").await;
}

#[tokio::test]
async fn no_memory_for_chromium_pauses_maps_until_a_probe() {
	pauses_maps_until_a_probe(NoMemory, "no_memory").await;
}

#[tokio::test]
async fn a_page_that_stopped_answering_pauses_maps_until_a_probe() {
	pauses_maps_until_a_probe(Stalled, "unresponsive").await;
}
