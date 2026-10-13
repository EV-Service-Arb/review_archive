//! The file a slow Chromium start leaves for the alerts to attach: the alerts deliver a log line
//! to Discord only when it points at a file under the artifacts root as `[<path>]`.

use std::{
	io,
	path::{Path, PathBuf},
	time::Duration,
};

use jiff::Timestamp;
use tokio::io::AsyncWriteExt as _;

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

/// Saves `report` under `dir`, keeping the newest [`KEEP`], and returns where.
pub(crate) async fn save(dir: &Path, report: &Report<'_>, now: Timestamp) -> io::Result<PathBuf> {
	match tokio::time::timeout(WRITE_LIMIT, write(dir, report, now)).await {
		Ok(saved) => saved,
		Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, format!("no answer from the disk in {WRITE_LIMIT:?}"))),
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
		reason: "Timeout 30000ms exceeded.",
		took: Duration::from_millis(30_400),
		limit: Duration::from_secs(30),
		host: "host: cgroup memory 214 of 214 MiB incl. page cache",
	};

	fn at(s: &str) -> Timestamp {
		s.parse().unwrap()
	}

	#[tokio::test]
	async fn a_report_is_a_file_with_the_figures() {
		let dir = tempfile::tempdir().unwrap();

		let path = save(&dir.path().join("captures"), &REPORT, at("2026-10-12T14:00:00Z")).await.unwrap();

		assert!(path.is_absolute() && path.starts_with(dir.path().join("captures")), "{path:?}");
		assert_eq!(path.file_name().unwrap(), "20261012T140000.000Z-launch.txt");
		let text = std::fs::read_to_string(&path).unwrap();
		assert_eq!(
			text,
			"Chromium did not start in time\nat: 2026-10-12T14:00:00Z\ntook: 30.4s (limit 30s)\nreason: Timeout 30000ms exceeded.\nhost: cgroup memory 214 of 214 MiB incl. page cache\n"
		);
	}

	#[tokio::test]
	async fn only_the_newest_reports_are_kept_and_other_files_are_left_alone() {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(dir.path().join("20200101T000000.000Z-walk-failed.png"), "").unwrap();

		let mut when = at("2026-10-01T00:00:00Z");
		for _ in 0..KEEP + 3 {
			save(dir.path(), &REPORT, when).await.unwrap();
			when += jiff::SignedDuration::from_mins(1);
		}

		let mut names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
		names.sort();
		let kept: Vec<_> = names.iter().filter(|n| n.ends_with(SUFFIX)).collect();
		assert_eq!(kept.len(), KEEP, "{names:?}");
		assert_eq!(kept[0].as_str(), "20261001T000300.000Z-launch.txt", "the three oldest went: {names:?}");
		assert!(names.contains(&"20200101T000000.000Z-walk-failed.png".to_owned()));
	}
}
