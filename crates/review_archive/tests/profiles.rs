//! A signed-out account moves the scanner to the next in `browser.profile_dirs`, for good until a
//! person re-signs it; with none left, the failure ends `serve`.

use std::path::{Path, PathBuf};

use review_archive::{
	Archive, Remedy, SessionError,
	browser::Browser,
	config::{BrowserConfig, Config},
	core::{Known, Scan, Target, dto::NewTarget, tokens::Meter},
	remedy,
	sources::ReviewSource,
};

/// Maps' "Sign in" corner.
struct SignedOut;

impl ReviewSource for SignedOut {
	async fn scan(&self, _: &Target, _: &Known, _: &mut Meter) -> eyre::Result<Scan> {
		Err(SessionError::new_signed_out().into())
	}
}

fn profiles(dir: &Path) -> [PathBuf; 2] {
	[dir.join("primary"), dir.join("secondary")]
}

async fn open(dir: &Path) -> Archive {
	let mut config = Config {
		data_dir: Some(dir.to_owned()),
		..Config::default()
	};
	config.browser.profile_dirs = profiles(dir).into();
	Archive::open(config).await.unwrap()
}

async fn target(archive: &Archive) -> Target {
	let t = NewTarget {
		place: Some("ChIJprofilestest000000000".into()),
		..Default::default()
	};
	archive.add_target(&t).await.unwrap().target
}

/// The profile `archive` scans on: the one whose lock it takes.
async fn using(archive: &Archive, dir: &Path) -> Option<PathBuf> {
	archive.browser().claim().await.unwrap();
	let mut held = None;
	for p in profiles(dir) {
		if Browser::new(BrowserConfig::default(), vec![p.clone()]).claim().await.is_err() {
			assert!(held.replace(p).is_none(), "one profile at a time");
		}
	}
	archive.close().await;
	held
}

async fn signed_out_rows(archive: &Archive) -> Vec<PathBuf> {
	archive
		.store()
		.unwrap()
		.profiles()
		.await
		.unwrap()
		.into_iter()
		.filter(|p| p.signed_out_at.is_some())
		.map(|p| p.path)
		.collect()
}

#[tokio::test]
async fn a_signed_out_primary_hands_over_to_the_secondary_across_restarts() {
	let dir = tempfile::tempdir().unwrap();
	let [primary, secondary] = profiles(dir.path());
	let archive = open(dir.path()).await;
	let target = target(&archive).await;
	assert_eq!(using(&archive, dir.path()).await, Some(primary.clone()));

	let failed = archive.record(&SignedOut, &target).await.unwrap();
	assert!(failed.failure.is_some());
	assert_eq!(signed_out_rows(&archive).await, vec![primary.clone()]);
	assert_eq!(using(&archive, dir.path()).await, Some(secondary.clone()));

	drop(archive);
	let archive = open(dir.path()).await;
	assert_eq!(using(&archive, dir.path()).await, Some(secondary));
}

#[tokio::test]
async fn with_every_account_signed_out_the_failure_is_fatal() {
	let dir = tempfile::tempdir().unwrap();
	let archive = open(dir.path()).await;
	let target = target(&archive).await;
	archive.record(&SignedOut, &target).await.unwrap();
	let last = archive.record(&SignedOut, &target).await.unwrap().failure.unwrap();
	assert_eq!(remedy(&last), Remedy::Fatal);
	assert_eq!(signed_out_rows(&archive).await, profiles(dir.path()));
	assert_eq!(using(&archive, dir.path()).await, None);
}

#[tokio::test]
async fn a_row_cleared_by_hand_puts_the_primary_first_again_from_the_next_walk() {
	let dir = tempfile::tempdir().unwrap();
	let [primary, secondary] = profiles(dir.path());
	let archive = open(dir.path()).await;
	let target = target(&archive).await;
	archive.record(&SignedOut, &target).await.unwrap();
	assert_eq!(using(&archive, dir.path()).await, Some(secondary));

	// what scripts/new_google_session.py runs on the host
	let db = sqlx::SqlitePool::connect(&format!("sqlite://{}", dir.path().join("review_archive.db").display())).await.unwrap();
	sqlx::query("UPDATE profiles SET signed_out_at = NULL WHERE path = ?")
		.bind(primary.to_str().unwrap())
		.execute(&db)
		.await
		.unwrap();
	db.close().await;

	// no Chromium configured: the walk fails at launch, after taking its profile
	let walked = archive.scan(&target).await.unwrap();
	assert!(walked.failure.is_some());
	assert_eq!(using(&archive, dir.path()).await, Some(primary));
}
