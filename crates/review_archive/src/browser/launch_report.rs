//! The file a slow Chromium start leaves for the alerts to attach.
//!
//! The alerts deliver a log line to Discord only when it points at a file under the artifacts
//! root as `[<path>]`, so a start that timed out is reported by saving the figures and
//! pointing at them. One report per window is enough: the node that starved Chromium once
//! starves every start behind it, and the rest are in each run's `error` and the log.

use std::{
	io,
	path::{Path, PathBuf},
	sync::{Mutex, PoisonError},
	time::Duration,
};

use jiff::{SignedDuration, Timestamp};
use tokio::io::AsyncWriteExt as _;

/// No second report within this long of the last one that was saved.
const WINDOW: SignedDuration = SignedDuration::from_hours(1);

/// The newest reports kept; the directory's own retention only runs when a walk saves a page.
const KEEP: usize = 10;

/// The disk that made Chromium slow can stall a write just as well, and the failed start
/// must not wait on it.
const WRITE_LIMIT: Duration = Duration::from_secs(5);

const SUFFIX: &str = "-launch.txt";

/// What went wrong with one start.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Report<'a> {
	pub reason: &'a str,
	pub took: Duration,
	pub limit: Duration,
	pub host: &'a str,
}

/// When the last report was saved. Held by the browser handle, so the window is the
/// process's, and every handle's.
#[derive(Debug, Default)]
pub(crate) struct Reports {
	last: Mutex<Option<Timestamp>>,
}

impl Reports {
	/// Saves `report` under `dir` and returns where, unless one was saved less than the window
	/// ago or there is no `dir`. A failed save is logged and leaves the window open, so the
	/// next failed start tries again.
	pub(crate) async fn save(&self, dir: Option<&Path>, report: &Report<'_>, now: Timestamp) -> Option<PathBuf> {
		let dir = dir?;
		if self.last.lock().unwrap_or_else(PoisonError::into_inner).is_some_and(|at| now.duration_since(at) < WINDOW) {
			return None;
		}
		let saved = match tokio::time::timeout(WRITE_LIMIT, write(dir, report, now)).await {
			Ok(saved) => saved,
			Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, format!("no answer from the disk in {WRITE_LIMIT:?}"))),
		};
		match saved {
			Ok(path) => {
				*self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(now);
				Some(path)
			}
			Err(e) => {
				tracing::warn!(dir = %dir.display(), error = %e, "the report of the slow Chromium start was not saved");
				None
			}
		}
	}
}

async fn write(dir: &Path, report: &Report<'_>, now: Timestamp) -> io::Result<PathBuf> {
	// the alerts only attach absolute paths
	let dir = std::path::absolute(dir)?;
	tokio::fs::create_dir_all(&dir).await?;
	let path = dir.join(format!("{}{SUFFIX}", now.strftime("%Y%m%dT%H%M%S%.3fZ")));
	let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&path).await?;
	file.write_all(render(report, now).as_bytes()).await?;
	file.flush().await?;
	if let Err(e) = prune(&dir).await {
		tracing::warn!(dir = %dir.display(), error = %e, "pruning the reports of slow Chromium starts");
	}
	Ok(path)
}

fn render(report: &Report<'_>, now: Timestamp) -> String {
	let Report { reason, took, limit, host } = report;
	format!(
		"Chromium did not start in time\nat: {now}\ntook: {:.1}s (limit {}s)\nreason: {reason}\n{host}\n",
		took.as_secs_f64(),
		limit.as_secs()
	)
}

/// Deletes all but the newest [`KEEP`] reports; the names start with a fixed-width UTC time,
/// so by name is by age.
async fn prune(dir: &Path) -> io::Result<()> {
	let mut names = Vec::new();
	let mut entries = tokio::fs::read_dir(dir).await?;
	while let Some(entry) = entries.next_entry().await? {
		if let Some(name) = entry.file_name().to_str().filter(|n| n.ends_with(SUFFIX)) {
			names.push(name.to_owned());
		}
	}
	names.sort_unstable();
	for old in names.iter().take(names.len().saturating_sub(KEEP)) {
		match tokio::fs::remove_file(dir.join(old)).await {
			Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
			_ => {}
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	const REPORT: Report<'static> = Report {
		reason: "Timeout: Playwright initialization timeout after 30 seconds",
		took: Duration::from_millis(30_400),
		limit: Duration::from_secs(30),
		host: "host: node io pressure some avg10=40.00 avg60=35.00",
	};

	fn at(s: &str) -> Timestamp {
		s.parse().unwrap()
	}

	fn names(dir: &Path) -> Vec<String> {
		let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
		names.sort();
		names
	}

	#[tokio::test]
	async fn a_report_is_a_file_with_the_figures() {
		let dir = tempfile::tempdir().unwrap();
		let reports = Reports::default();

		let path = reports.save(Some(&dir.path().join("captures")), &REPORT, at("2026-10-12T14:00:00Z")).await.expect("saved");

		assert!(path.is_absolute() && path.starts_with(dir.path().join("captures")), "{path:?}");
		assert_eq!(path.file_name().unwrap(), "20261012T140000.000Z-launch.txt");
		let text = std::fs::read_to_string(&path).unwrap();
		assert_eq!(
			text,
			"Chromium did not start in time\nat: 2026-10-12T14:00:00Z\ntook: 30.4s (limit 30s)\nreason: Timeout: Playwright initialization timeout after 30 seconds\nhost: node io pressure some avg10=40.00 avg60=35.00\n"
		);
	}

	#[tokio::test]
	async fn a_second_failure_inside_the_window_is_not_reported_and_one_after_it_is() {
		let dir = tempfile::tempdir().unwrap();
		let reports = Reports::default();

		assert!(reports.save(Some(dir.path()), &REPORT, at("2026-10-12T14:00:00Z")).await.is_some());
		assert!(reports.save(Some(dir.path()), &REPORT, at("2026-10-12T14:59:59Z")).await.is_none());
		assert_eq!(names(dir.path()).len(), 1);

		assert!(reports.save(Some(dir.path()), &REPORT, at("2026-10-12T15:00:00Z")).await.is_some());
		assert_eq!(names(dir.path()).len(), 2);
	}

	#[tokio::test]
	async fn no_directory_means_no_report() {
		assert!(Reports::default().save(None, &REPORT, at("2026-10-12T14:00:00Z")).await.is_none());
	}

	#[tokio::test]
	async fn a_save_that_failed_does_not_use_up_the_window() {
		let dir = tempfile::tempdir().unwrap();
		let blocker = dir.path().join("not-a-dir");
		std::fs::write(&blocker, "").unwrap();
		let reports = Reports::default();

		assert!(reports.save(Some(&blocker), &REPORT, at("2026-10-12T14:00:00Z")).await.is_none());
		assert!(reports.save(Some(dir.path()), &REPORT, at("2026-10-12T14:00:01Z")).await.is_some());
	}

	#[tokio::test]
	async fn only_the_newest_reports_are_kept_and_other_files_are_left_alone() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(dir.path().join("20200101T000000.000Z-walk-failed.png"), "").unwrap();
		let reports = Reports::default();

		let mut when = at("2026-10-01T00:00:00Z");
		for _ in 0..KEEP + 3 {
			assert!(reports.save(Some(dir.path()), &REPORT, when).await.is_some());
			when += WINDOW;
		}

		let names = names(dir.path());
		let kept: Vec<_> = names.iter().filter(|n| n.ends_with(SUFFIX)).collect();
		assert_eq!(kept.len(), KEEP, "{names:?}");
		assert_eq!(kept[0].as_str(), "20261001T030000.000Z-launch.txt", "the three oldest went: {names:?}");
		assert!(names.contains(&"20200101T000000.000Z-walk-failed.png".to_owned()));
	}
}
