//! The dashboard's bundle, built and served by `serve --dev-member`, in Chromium. Needs the devShell's
//! Chromium, `wasm-bindgen` and `tailwindcss`, so `#[ignore]`d: `cargo test --test dashboard -- --ignored`.

use std::{collections::BTreeMap, path::Path, process::Command, time::Duration};

use browser_manipulation::{Browser, Launch, Robot, Viewport};
use review_archive::{
	Archive,
	config::Config,
	core::dto::{NewGmail, NewTrack},
	store::people::{Claim, Seen},
};
use serde::Deserialize;

#[path = "../../review_archive/tests/support/busy.rs"]
mod busy;

const SHORT: &str = "Shiny Maids";
const TWO_LINES: &str = "Long Island Premier Christmas & Holiday Lighting Decorators";
const LONGER: &str = "Long Island Premier Christmas & Holiday Lighting Decorators of Nassau and Suffolk Counties, New York";

#[derive(Debug, Deserialize, PartialEq)]
struct Name {
	lines: u32,
	/// Below the card's top border, in px.
	top: f64,
	/// Cut off with an ellipsis.
	cut: bool,
}

/// Namesakes differ in what follows their common start (#44): a name wraps once, and what wrapping takes is
/// taken from the space above it.
#[tokio::test]
#[ignore = "dashboard: needs the devShell's Chromium, wasm-bindgen and tailwindcss"]
async fn dashboard_a_place_name_wraps_once_into_the_space_above_it() {
	let dir = tempfile::tempdir().unwrap();
	let mfe = dir.path().join("mfe");
	bundle(&mfe);

	let data = dir.path().join("data");
	let archive = Archive::open(Config {
		data_dir: Some(data.clone()),
		..Config::default()
	})
	.await
	.unwrap();
	let seen = Seen {
		sub: "owner",
		email: "owner@x.com",
		email_verified: true,
		name: "owner",
	};
	let Claim::Person(owner) = archive.person(&seen).await.unwrap() else {
		panic!("a fresh archive")
	};
	let gmail = archive.add_gmail(owner, &NewGmail { gmail: "ops@gmail.com".into() }).await.unwrap().id;
	for (i, label) in [SHORT, TWO_LINES, LONGER].into_iter().enumerate() {
		let track = NewTrack {
			place: format!("ChIJdashboardtest{i:0>8}"),
			label: Some(label.into()),
			..Default::default()
		};
		archive.track(owner, gmail, &track).await.unwrap();
	}
	archive.set_gmail_enabled(owner, gmail, false).await.unwrap(); // no scans: there is no browser for them
	archive.close().await;

	let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
	let config = dir.path().join("config.toml");
	std::fs::write(&config, format!("data_dir = {data:?}\nmfe_dir = {mfe:?}\nbind = \"127.0.0.1:{port}\"\n")).unwrap();
	let mut serve = tokio::process::Command::new(env!("CARGO_BIN_EXE_review_archive"))
		.arg("--config")
		.arg(&config)
		.args(["serve", "--dev-member", "none", "--no-login-check"])
		.env("APP_ENV", "development")
		.kill_on_drop(true)
		.spawn()
		.unwrap();

	let browser = Browser::launch(
		Launch::Owned {
			profile: dir.path().join("profile"),
			executable: std::env::var_os("REVIEW_ARCHIVE_CHROME").expect("set by the devShell").into(),
			headless: true,
			viewport: Some(Viewport {
				width: 1280,
				height: 900,
				device_scale_factor: 1.0,
			}),
			args: vec![],
		},
		Robot,
		None,
	)
	.await
	.unwrap_or_else(|e| busy::busy(format!("Chromium did not start: {e}")));
	let mut tab = browser.tab().await.unwrap();
	let names = async {
		while tab.goto(&format!("http://127.0.0.1:{port}/gmails/{gmail}")).await.is_err() {
			tokio::time::sleep(Duration::from_millis(100)).await;
		}
		loop {
			let names: BTreeMap<String, Name> = tab.eval(MEASURE, [SHORT, TWO_LINES, LONGER]).await.unwrap();
			if names.len() == 3 {
				break names;
			}
			tokio::time::sleep(Duration::from_millis(100)).await;
		}
	};
	let names = tokio::select! {
		names = names => names,
		status = serve.wait() => panic!("serve ended: {status:?}"),
		() = tokio::time::sleep(Duration::from_secs(60)) => busy::busy("no card names every place in 60s".to_owned()),
	};
	browser.close().await.unwrap();

	assert_eq!(names[SHORT], Name { lines: 1, top: 16.0, cut: false }, "{names:#?}");
	assert_eq!(names[TWO_LINES], Name { lines: 2, top: 8.0, cut: false }, "{names:#?}");
	assert_eq!(names[LONGER], Name { lines: 2, top: 8.0, cut: true }, "{names:#?}");
}

const MEASURE: &str = r#"labels => Object.fromEntries([...document.querySelectorAll("[data-slot=card] div")]
	.filter(el => el.childElementCount == 0 && labels.includes(el.textContent))
	.map(el => {
		const card = el.closest("[data-slot=card]");
		const box = el.getBoundingClientRect();
		const top = box.top - card.getBoundingClientRect().top - parseFloat(getComputedStyle(card).borderTopWidth);
		const text = document.createRange();
		text.selectNodeContents(el);
		const lines = new Set([...text.getClientRects()].filter(r => r.top < box.bottom).map(r => Math.round(r.top))).size;
		return [el.textContent, { lines, top, cut: el.scrollHeight > el.clientHeight || el.scrollWidth > el.clientWidth }];
	}))"#;

/// As `nix run .#dev-mfe` builds it, in a target dir of its own: the one `cargo test` runs from is locked.
fn bundle(out: &Path) {
	let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
	let target = root.join("target/dashboard-test");
	let status = Command::new(env!("CARGO"))
		.args(["build", "-p", "review_archive_web", "--target", "wasm32-unknown-unknown", "--target-dir"])
		.arg(&target)
		.env("RUSTFLAGS", r#"--cfg=web_sys_unstable_apis --cfg=getrandom_backend="wasm_js""#)
		.current_dir(&root)
		.status()
		.unwrap();
	assert!(status.success(), "building the dashboard: {status}");
	let status = Command::new("bash")
		.arg(root.join("crates/review_archive_web/package.sh"))
		.arg(target.join("wasm32-unknown-unknown/debug/review_archive_web.wasm"))
		.arg(out)
		.status()
		.unwrap();
	assert!(status.success(), "packaging the dashboard: {status}");
}
