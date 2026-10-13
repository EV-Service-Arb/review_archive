//! The worker loop on a temp archive, without a browser: `gbp` targets with no credentials
//! fail their scans before any page is opened, which is enough to see what runs first.

use std::{
	io,
	path::PathBuf,
	sync::{Arc, Mutex},
	time::Duration,
};

use jiff::Timestamp;
use review_archive::{
	Archive,
	config::Config,
	core::{
		TargetId,
		dto::{JobKind, JobStatus, NewTarget, TargetPatch},
	},
};
use review_archive_core::schedule::Schedule;
use review_archive_server::worker::{Signals, WorkerConfig, run};
use tokio::sync::watch;

const PLACE: &str = "ChIJLU7jZClu5kcR4PcOOO6p3I0";

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
	fn new(name: &str) -> Self {
		let dir = std::env::temp_dir().join(format!("review_archive-{name}-{}-{}", std::process::id(), Timestamp::now().as_nanosecond()));
		std::fs::create_dir_all(&dir).unwrap();
		Self(dir)
	}
}

impl Drop for TempDir {
	fn drop(&mut self) {
		let _ = std::fs::remove_dir_all(&self.0);
	}
}

async fn gbp_target(archive: &Archive, disabled: bool) -> TargetId {
	let req = NewTarget {
		place: Some(PLACE.into()),
		gbp: Some("1/2".into()),
		..Default::default()
	};
	let id = archive.add_target(&req).await.unwrap().target.id;
	if disabled {
		let off = TargetPatch {
			enabled: Some(false),
			..Default::default()
		};
		archive.update_target(id, &off).await.unwrap();
	}
	id
}

/// On start the worker fails the job a dead process left `running`, then takes the queued
/// job before any target the schedule says is due.
#[tokio::test]
async fn a_restart_fails_the_interrupted_job_and_queued_jobs_go_before_due_targets() {
	let dir = TempDir::new("worker");
	let archive = Archive::open(Config {
		data_dir: Some(dir.0.clone()),
		..Config::default()
	})
	.await
	.unwrap();
	let store = archive.store().unwrap();
	// never scanned and enabled: due now
	let due = gbp_target(&archive, false).await;
	// only reachable through the queue
	let queued_for = gbp_target(&archive, true).await;

	// a previous process took this job and died
	let interrupted = store.enqueue_job(JobKind::Scan, queued_for, None, None, 20, Timestamp::now()).await.unwrap();
	assert_eq!(store.claim_job(Timestamp::now()).await.unwrap().unwrap().id, interrupted);
	let queued = store.enqueue_job(JobKind::Scan, queued_for, None, None, 20, Timestamp::now()).await.unwrap();

	let signals = Signals::default();
	let (stop, stopped) = watch::channel(false);
	let mut finished = signals.job_finished.subscribe();
	let (cfg, schedule) = (WorkerConfig::default(), Schedule::default());
	let worker = run(&archive, &signals, &cfg, &schedule, stopped);
	let observe = async {
		tokio::time::timeout(Duration::from_secs(20), finished.changed()).await.expect("a job finished").unwrap();
		let job = store.job(queued).await.unwrap().unwrap();
		let due_runs = store.runs(due, 10).await.unwrap();
		stop.send(true).unwrap();
		(job, due_runs)
	};
	let (worked, (job, due_runs)) = tokio::join!(worker, observe);
	worked.unwrap();

	let interrupted = store.job(interrupted).await.unwrap().unwrap();
	assert_eq!(interrupted.status, JobStatus::Failed);
	assert!(interrupted.error.unwrap().contains("interrupted"));

	// the queued job ran (and failed: no GBP credentials here), with a run of its own
	assert_eq!(job.status, JobStatus::Failed, "{job:?}");
	assert!(job.run.is_some(), "{job:?}");
	assert!(due_runs.is_empty(), "the due target was scanned before the queued job: {due_runs:?}");
}

/// A browser that cannot be set up (here: no executable configured) is not a passing
/// condition: the worker ends with the error so the process exits and the alert fires. A
/// failure that does pass — like the missing GBP credentials above — never gets this far.
#[tokio::test]
async fn a_browser_that_cannot_be_set_up_ends_the_worker() {
	let dir = TempDir::new("worker-fatal");
	let archive = Archive::open(Config {
		data_dir: Some(dir.0.clone()),
		..Config::default()
	})
	.await
	.unwrap();
	let maps = NewTarget {
		place: Some(PLACE.into()),
		..Default::default()
	};
	archive.add_target(&maps).await.unwrap();

	let signals = Signals::default();
	let (_stop, stopped) = watch::channel(false);
	let (cfg, schedule) = (WorkerConfig::default(), Schedule::default());
	let ended = tokio::time::timeout(Duration::from_secs(20), run(&archive, &signals, &cfg, &schedule, stopped))
		.await
		.expect("the worker ended");

	let e = ended.expect_err("the worker cannot go on");
	assert!(review_archive::describe(&e).contains("review_archive::browser::launch"), "{e:#}");
}

/// What the worker logged, as plain text lines.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl io::Write for Logs {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(buf);
		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

/// Runs the worker on `dir`'s archive until `target` has a run, and returns its log line
/// reporting a failure.
async fn reported(dir: &TempDir, target: TargetId) -> String {
	let logs = Logs::default();
	let writer = logs.clone();
	let subscriber = tracing_subscriber::fmt().with_ansi(false).with_writer(move || writer.clone()).finish();
	let _logs = tracing::subscriber::set_default(subscriber);
	let archive = Archive::open(Config {
		data_dir: Some(dir.0.clone()),
		..Config::default()
	})
	.await
	.unwrap();
	let store = archive.store().unwrap();
	let signals = Signals::default();
	let (stop, stopped) = watch::channel(false);
	let (cfg, schedule) = (WorkerConfig::default(), Schedule::default());
	let observe = async {
		tokio::time::timeout(Duration::from_secs(20), async {
			while store.runs(target, 1).await.unwrap().is_empty() {
				tokio::time::sleep(Duration::from_millis(20)).await;
			}
		})
		.await
		.expect("the target was scanned");
		stop.send(true).unwrap();
	};
	let (worked, ()) = tokio::join!(run(&archive, &signals, &cfg, &schedule, stopped), observe);
	worked.unwrap();
	let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
	let lines: Vec<&str> = text.lines().filter(|l| l.contains(" failed")).collect();
	let [line] = lines[..] else { panic!("one report: {text}") };
	line.to_owned()
}

/// A job's failure is typed when the worker reports it, as a scheduled scan's is: both are told
/// by who they wait for, here a person who has to set the credentials.
#[tokio::test]
async fn a_failed_job_is_reported_as_a_failed_scan_is() {
	let scanned = TempDir::new("report-scan");
	let archive = Archive::open(Config {
		data_dir: Some(scanned.0.clone()),
		..Config::default()
	})
	.await
	.unwrap();
	let due = gbp_target(&archive, false).await;
	drop(archive);
	let scan = reported(&scanned, due).await;

	let queued = TempDir::new("report-job");
	let archive = Archive::open(Config {
		data_dir: Some(queued.0.clone()),
		..Config::default()
	})
	.await
	.unwrap();
	let only_queued = gbp_target(&archive, true).await;
	archive.enqueue_scan(only_queued).await.unwrap();
	drop(archive);
	let job = reported(&queued, only_queued).await;

	for line in [&scan, &job] {
		assert!(
			line.contains(" WARN ") && line.contains("remedy=Human") && line.contains("review_archive::gbp::no_credentials"),
			"{line}"
		);
	}
}
