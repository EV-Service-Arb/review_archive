//! `serve --dev-member` on a data dir someone already manages places in.

use std::time::Duration;

use review_archive::{
	Archive,
	config::Config,
	core::dto::{Me, NewGmail},
	store::people::{Claim, Seen},
};

/// The worker scans every member's places; the dashboard opens on the one who has them.
#[tokio::test]
async fn the_dev_member_is_whoever_manages_the_archive() {
	let dir = tempfile::tempdir().unwrap();
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
	archive.add_gmail(owner, &NewGmail { gmail: "ops@gmail.com".into() }).await.unwrap();
	archive.close().await;

	let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
	let config = dir.path().join("config.toml");
	std::fs::write(&config, format!("data_dir = {data:?}\nbind = \"127.0.0.1:{port}\"\n[tokens]\nstart_cap_multiple = 2.0\n")).unwrap();
	let mut serve = tokio::process::Command::new(env!("CARGO_BIN_EXE_review_archive"))
		.arg("--config")
		.arg(&config)
		.args(["serve", "--dev-member", "none"])
		.env("APP_ENV", "development")
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let me = async {
		loop {
			if let Ok(r) = reqwest::get(format!("http://127.0.0.1:{port}/me")).await {
				break r.json::<Me>().await.unwrap();
			}
			tokio::time::sleep(Duration::from_millis(100)).await;
		}
	};
	let me = tokio::select! {
		me = me => me,
		status = serve.wait() => panic!("serve ended: {status:?}"),
		() = tokio::time::sleep(Duration::from_secs(60)) => panic!("serve never answered"),
	};
	assert_eq!((me.id, me.email.as_str()), (owner.0, "owner@x.com"));
}
