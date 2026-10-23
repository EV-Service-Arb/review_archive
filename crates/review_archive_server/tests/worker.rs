//! The worker loop on a temp archive, without a browser: `gbp` targets with no credentials
//! fail their scans before any page is opened, which is enough to see what runs first.

use std::{path::PathBuf, time::Duration};

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
	let worker = run(&archive, &signals, &cfg, &schedule, true, stopped);
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
	let ended = tokio::time::timeout(Duration::from_secs(20), run(&archive, &signals, &cfg, &schedule, true, stopped))
		.await
		.expect("the worker ended");

	let e = ended.expect_err("the worker cannot go on");
	assert!(review_archive::describe(&e).contains("review_archive::browser::launch"), "{e:#}");
}

/// What restarting cannot fix exits 78 (EX_CONFIG), which devops' observer holds the
/// deployment on instead of restarting it into the same failure.
#[test]
fn serve_whose_worker_cannot_go_on_exits_78() {
	let dir = TempDir::new("serve-fatal");
	let config = dir.0.join("config.toml");
	std::fs::write(
		&config,
		format!(
			"data_dir = {:?}\nbind = \"127.0.0.1:0\"\nbrowser.executable = \"/nonexistent/chromium\"\ntokens.start_cap_multiple = 2.0\n",
			dir.0.join("data")
		),
	)
	.unwrap();
	let cli = |args: &[&str]| {
		let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_review_archive"));
		c.arg("--config").arg(&config).args(args).env("APP_ENV", "development");
		c
	};
	assert!(cli(&["target", "add", PLACE]).status().unwrap().success());

	let mut serve = cli(&["serve", "--dev-member", "none"]).spawn().unwrap();
	let deadline = std::time::Instant::now() + Duration::from_secs(60);
	let status = loop {
		if let Some(status) = serve.try_wait().unwrap() {
			break status;
		}
		if std::time::Instant::now() > deadline {
			serve.kill().unwrap();
			panic!("serve went on without a browser");
		}
		std::thread::sleep(Duration::from_millis(100));
	};
	assert_eq!(status.code(), Some(78));
}

/// Every Google account signed out: `serve` exits 78 at boot, before a walk is due, and records no run.
#[test]
fn serve_with_every_account_signed_out_exits_78_at_boot() {
	let dir = TempDir::new("serve-signed-out");
	let data = dir.0.join("data");
	let config = dir.0.join("config.toml");
	std::fs::write(
		&config,
		format!("data_dir = {data:?}\nbind = \"127.0.0.1:0\"\nbrowser.executable = \"/nonexistent/chromium\"\ntokens.start_cap_multiple = 2.0\n"),
	)
	.unwrap();
	let cli = |args: &[&str]| {
		let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_review_archive"));
		c.arg("--config").arg(&config).args(args).env("APP_ENV", "development");
		c
	};
	assert!(cli(&["target", "add", PLACE, "--interval", "1d"]).status().unwrap().success());
	let rt = tokio::runtime::Runtime::new().unwrap();
	let target = rt.block_on(async {
		let archive = Archive::open(Config {
			data_dir: Some(data.clone()),
			..Config::default()
		})
		.await
		.unwrap();
		archive.store().unwrap().mark_signed_out(&data.join("chromium-profile"), Timestamp::now()).await.unwrap();
		let target = archive.targets().await.unwrap()[0].id;
		archive.close().await;
		target
	});

	let mut serve = cli(&["serve", "--dev-member", "none"]).stderr(std::process::Stdio::piped()).spawn().unwrap();
	let deadline = std::time::Instant::now() + Duration::from_secs(60);
	let status = loop {
		if let Some(status) = serve.try_wait().unwrap() {
			break status;
		}
		if std::time::Instant::now() > deadline {
			serve.kill().unwrap();
			panic!("serve went on with every account signed out");
		}
		std::thread::sleep(Duration::from_millis(100));
	};
	let mut stderr = String::new();
	std::io::Read::read_to_string(&mut serve.stderr.take().unwrap(), &mut stderr).unwrap();
	assert_eq!(status.code(), Some(78), "{stderr}");
	assert!(
		stderr.contains("at boot, every Google account is signed out: ") && stderr.contains("help: ") && stderr.contains("code: review_archive::google::signed_out"),
		"{stderr}"
	);
	rt.block_on(async {
		let archive = Archive::open(Config {
			data_dir: Some(data.clone()),
			..Config::default()
		})
		.await
		.unwrap();
		assert!(archive.runs(target, &Default::default()).await.unwrap().is_empty(), "no walk was tried");
		archive.close().await;
	});
}

/// `--no-login-check`: every account signed out fails the walk, and `serve` goes on serving.
#[test]
fn serve_without_the_login_check_outlives_every_account_signed_out() {
	let dir = TempDir::new("serve-no-login-check");
	let data = dir.0.join("data");
	let config = dir.0.join("config.toml");
	std::fs::write(
		&config,
		format!("data_dir = {data:?}\nbind = \"127.0.0.1:0\"\nbrowser.executable = \"/nonexistent/chromium\"\ntokens.start_cap_multiple = 2.0\n"),
	)
	.unwrap();
	let cli = |args: &[&str]| {
		let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_review_archive"));
		c.arg("--config").arg(&config).args(args).env("APP_ENV", "development");
		c
	};
	assert!(cli(&["target", "add", PLACE, "--interval", "1d"]).status().unwrap().success());
	let rt = tokio::runtime::Runtime::new().unwrap();
	let open = || async {
		Archive::open(Config {
			data_dir: Some(data.clone()),
			..Config::default()
		})
		.await
		.unwrap()
	};
	let target = rt.block_on(async {
		let archive = open().await;
		archive.store().unwrap().mark_signed_out(&data.join("chromium-profile"), Timestamp::now()).await.unwrap();
		let target = archive.targets().await.unwrap()[0].id;
		archive.close().await;
		target
	});

	let mut serve = cli(&["serve", "--dev-member", "none", "--no-login-check"]).spawn().unwrap();
	let deadline = std::time::Instant::now() + Duration::from_secs(60);
	let run = loop {
		assert_eq!(serve.try_wait().unwrap(), None, "serve ended");
		let runs = rt.block_on(async {
			let archive = open().await;
			let runs = archive.runs(target, &Default::default()).await.unwrap();
			archive.close().await;
			runs
		});
		if let Some(run) = runs.into_iter().next() {
			break run;
		}
		assert!(std::time::Instant::now() < deadline, "no walk was tried");
		std::thread::sleep(Duration::from_millis(200));
	};
	assert!(run.error.as_deref().is_some_and(|e| e.contains("signed out of Google")), "{run:?}");
	std::thread::sleep(Duration::from_secs(2));
	assert_eq!(serve.try_wait().unwrap(), None, "serve ended after the walk");
	serve.kill().unwrap();
}
