//! Real scans of real places. Need a browser and the network, so they are `#[ignore]`d:
//! `cargo test --test live -- --ignored`, and `nix run .#publish` runs them before it tags.
//!
//! `REVIEW_ARCHIVE_CHROME` points at the browser (the devShell sets it),
//! `REVIEW_ARCHIVE_HEADFUL=1` opens a window, and `REVIEW_ARCHIVE_PROFILE` reuses a browser
//! profile instead of a fresh one, never a signed-in one (ARCHITECTURE.md § Testing). Google
//! may answer with a "limited view" of Maps that has no reviews (headless, or a fresh profile);
//! the scan then says so with the page saved.

use review_archive::{
	Archive, CaptureRequest, SessionError,
	config::Config,
	core::dto::{NewGmail, NewTarget, NewTrack, ReviewsQuery, RunStatus},
	store::people::{Claim, Seen},
};

#[path = "support/busy.rs"]
mod busy;

fn starved(failure: &str) {
	if ["review_archive::browser::launch_timeout", "review_archive::browser::unresponsive"]
		.iter()
		.any(|code| failure.contains(code))
	{
		busy::busy(failure.to_owned());
	}
}

/// The Eiffel Tower: public, and reviewed every few minutes.
const PLACE_ID: &str = "ChIJLU7jZClu5kcR4PcOOO6p3I0";

/// Shared from the Maps app; a name search for what it redirects to finds a namesake in Flushing (#37).
const SHARE_LINK: &str = "https://maps.app.goo.gl/xpeH4R7UKWfVeXqC8";
/// Shiny Maids of Hicksville, the place the link shares.
const SHARED_PLACE_ID: &str = "ChIJ89hmOOCBwokRDrDlHKKRT6c";

/// The data dir, kept when the test fails: a failed walk's error names the page it saved under it.
struct Kept(Option<tempfile::TempDir>);

impl Kept {
	fn new() -> Self {
		Self(Some(tempfile::tempdir().unwrap()))
	}

	fn path(&self) -> &std::path::Path {
		self.0.as_ref().expect("taken on drop only").path()
	}
}

impl Drop for Kept {
	fn drop(&mut self) {
		if std::thread::panicking() {
			let dir = self.0.take().expect("taken on drop only").keep();
			eprintln!("kept the data dir: {}", dir.display());
		}
	}
}

fn config(dir: &std::path::Path) -> Config {
	let mut config = Config {
		data_dir: Some(dir.to_owned()),
		..Config::default()
	};
	config.browser.executable = std::env::var_os("REVIEW_ARCHIVE_CHROME").map(Into::into);
	config.browser.lean = Some(vec![]);
	config.browser.headful = std::env::var("REVIEW_ARCHIVE_HEADFUL").is_ok_and(|v| v == "1");
	config.browser.guest = true;
	config.browser.profile_dirs = std::env::var_os("REVIEW_ARCHIVE_PROFILE").map(Into::into).into_iter().collect();
	config.defaults.max_reviews_initial = 12;
	config.defaults.max_reviews_per_scan = 12;
	config.defaults.lang = "fr".into();
	config
}

#[tokio::test]
#[ignore = "live: needs Chrome and the network"]
async fn live_scan_of_a_real_place() {
	let dir = Kept::new();
	let archive = Archive::open(config(dir.path())).await.unwrap();
	let added = archive
		.add_target(&NewTarget {
			place: Some(PLACE_ID.into()),
			label: Some("Tour Eiffel".into()),
			interval: Some("6h".into()),
			..Default::default()
		})
		.await
		.unwrap();
	let id = added.target.id;
	let summary = archive.scan_target(id).await.unwrap();
	archive.close().await;
	println!("{summary}");

	// What Google serves the browser is its call; each answer must come out as ARCHITECTURE's invariants say.
	let error = summary.error.as_deref();
	if summary.status == RunStatus::Failed {
		let error = error.expect("a failed run says why");
		starved(error);
		assert!(error.contains("code: review_archive::google::"), "{summary}");
		assert!(error.contains("-walk-failed.png]"), "{summary}");
		return;
	}
	assert_eq!(error, None, "{summary}");
	assert!((1..=5).contains(&summary.counts.new), "a guest sees at most 5: {summary}");
	assert!(summary.captured >= summary.counts.new * 5 / 6, "{summary}");
	let reviews = archive.reviews(id, &ReviewsQuery::default()).await.unwrap();
	assert!(reviews.iter().all(|r| r.rating.is_some() && r.published_est.is_some()));
	let captured = reviews.iter().find(|r| r.capture_sha256.is_some()).unwrap();
	assert!(archive.review(review_archive::core::ReviewId(captured.id)).await.unwrap().captures[0].width >= 600);
}

/// The library without a store: reviews and WebPs in memory.
#[tokio::test]
#[ignore = "live: needs Chrome and the network"]
async fn live_capture_without_a_store() {
	let dir = tempfile::tempdir().unwrap();
	let mut config = config(dir.path());
	config.browser.profile_dirs = config.profile_dirs();
	config.data_dir = None;
	let archive = Archive::open(config).await.unwrap();
	let got = archive.capture_place(&CaptureRequest::new(PLACE_ID).lang("fr").max_reviews(5)).await;
	archive.close().await;
	let got = match got {
		Ok(got) => got,
		Err(e) => {
			let failure = review_archive::describe(&e);
			starved(&failure);
			assert!(
				matches!(e.downcast_ref::<SessionError>(), Some(SessionError::LimitedView { .. } | SessionError::Blocked { .. })),
				"{failure}"
			);
			return;
		}
	};
	assert!(!got.scan.reviews.is_empty(), "{:?}", got.scan.warnings);
	assert!(got.scan.reviews.iter().all(|r| r.capture.is_none()), "taken out into webps");
	assert!(got.webps.values().next().unwrap().width >= 600);
	assert!(archive.store().is_err());
}

/// A profile that once met `WWW-Authenticate: Negotiate` keeps a flag in `Local State` that hangs
/// every later start with the network service in the browser process, until it is cleared.
#[tokio::test]
#[ignore = "live: needs Chrome and the network"]
async fn live_capture_on_a_profile_that_met_negotiate() {
	let dir = tempfile::tempdir().unwrap();
	let mut config = config(dir.path());
	let profile = dir.path().join("profile");
	std::fs::create_dir(&profile).unwrap();
	std::fs::write(profile.join("Local State"), r#"{"net":{"received_http_auth_negotiate_headers":true}}"#).unwrap();
	config.browser.profile_dirs = vec![profile];
	config.data_dir = None;
	let archive = Archive::open(config).await.unwrap();
	let got = archive.capture_place(&CaptureRequest::new(PLACE_ID).lang("fr").max_reviews(5)).await;
	archive.close().await;
	if let Err(e) = got {
		let failure = review_archive::describe(&e);
		starved(&failure);
		assert!(
			matches!(e.downcast_ref::<SessionError>(), Some(SessionError::LimitedView { .. } | SessionError::Blocked { .. })),
			"{failure}"
		);
	}
}

/// What the dashboard does with a pasted link: list its places, track the one. No `GOOGLE_MAPS_KEY`:
/// the link carries a feature id, and resolving it must not fall back to a name search.
#[tokio::test]
#[ignore = "live: needs Chrome and the network"]
async fn live_a_share_link_tracks_the_place_it_shares() {
	let dir = Kept::new();
	let archive = Archive::open(config(dir.path())).await.unwrap();
	let seen = Seen {
		sub: "sub",
		email: "member@example.com",
		email_verified: true,
		name: "Member",
	};
	let Claim::Person(member) = archive.person(&seen).await.unwrap() else {
		panic!("a fresh archive refuses nobody")
	};
	let gmail = archive.add_gmail(member, &NewGmail { gmail: "manager@gmail.com".into() }).await.unwrap().id;

	let places = archive.places(SHARE_LINK).await.unwrap();
	assert_eq!(places.iter().map(|p| p.place_id.as_str()).collect::<Vec<_>>(), [SHARED_PLACE_ID], "{places:?}");
	let picked = archive
		.track(
			member,
			gmail,
			&NewTrack {
				place: places[0].place_id.clone(),
				..Default::default()
			},
		)
		.await
		.unwrap();
	let pasted = archive
		.track(
			member,
			gmail,
			&NewTrack {
				place: SHARE_LINK.into(),
				..Default::default()
			},
		)
		.await
		.unwrap();
	assert_eq!(picked.place_id, SHARED_PLACE_ID);
	assert_eq!(pasted.id, picked.id);

	let summary = archive.scan_target(review_archive::core::TargetId(picked.id)).await.unwrap();
	archive.close().await;
	println!("{summary}");
	let error = summary.error.as_deref();
	if summary.status == RunStatus::Failed {
		let error = error.expect("a failed run says why");
		starved(error);
		assert!(error.contains("code: review_archive::google::"), "{summary}");
		return;
	}
	assert_eq!(error, None, "{summary}");
	assert!((1..=5).contains(&summary.counts.new), "a guest sees at most 5: {summary}");
}
