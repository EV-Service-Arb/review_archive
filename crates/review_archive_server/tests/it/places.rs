//! Places, asked through the conductor: the archive holds no key, and a relink is bought for the
//! member whose request it is, as their GMB's gmail, carrying the conductor's assertion on it.

use std::{
	sync::{Arc, Mutex},
	time::Duration,
};

use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
use review_archive::{
	Archive,
	config::Config,
	store::people::{Claim, Seen},
};
use serde_json::{Value, json};

const OWNER: &str = "00000000-0000-0000-0000-00000000000b";
const PLACE: &str = "ChIJrelinkedtest00000000";
const MOVED: &str = "ChIJrelinkedtestmoved000";

/// The conductor's queries cache, buying: every buy it was sent, its assertion and body.
async fn conductor() -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
	async fn buy(State(seen): State<Arc<Mutex<Vec<(String, Value)>>>>, headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
		let assertion = headers.get("x-sa-assertion").map(|a| a.to_str().unwrap().to_owned()).unwrap_or_default();
		seen.lock().unwrap().push((assertion, body));
		Json(json!({ "answer": { "id": MOVED, "displayName": { "text": "GBP 2" }, "formattedAddress": "Lyon" } }))
	}
	let seen = Arc::<Mutex<Vec<(String, Value)>>>::default();
	let app = Router::new().route("/api/queries_cache/places/{key}/buy", post(buy)).with_state(seen.clone());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}/api/queries_cache", listener.local_addr().unwrap());
	tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	(url, seen)
}

#[tokio::test]
async fn a_relink_is_bought_by_the_conductor_for_the_member_as_their_gmbs_gmail() {
	let dir = tempfile::tempdir().unwrap();
	let data = dir.path().join("data");
	let archive = Archive::open(Config {
		data_dir: Some(data.clone()),
		..Config::default()
	})
	.await
	.unwrap();
	let seen = Seen {
		sub: OWNER,
		email: "owner@x.com",
		email_verified: true,
		name: "owner",
	};
	let Claim::Person(_) = archive.person(&seen).await.unwrap() else { panic!("a fresh archive") };
	let snapshot = super::snapshot(OWNER, 2, PLACE);
	archive.apply_snapshot(&snapshot).await.unwrap();
	let target = archive
		.targets()
		.await
		.unwrap()
		.into_iter()
		.find(|t| t.place_id == PLACE)
		.expect("the snapshot's GMB is tracked")
		.id;
	archive.close().await;

	let (cache, bought) = conductor().await;
	let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
	let config = dir.path().join("config.toml");
	std::fs::write(&config, format!("data_dir = {data:?}\nbind = \"127.0.0.1:{port}\"\nqueries_cache = {cache:?}\n")).unwrap();
	let mut serve = tokio::process::Command::new(env!("CARGO_BIN_EXE_review_archive"))
		.arg("--config")
		.arg(&config)
		.args(["serve", "--dev-member", "none", "--no-login-check"])
		.env("APP_ENV", "development")
		.kill_on_drop(true)
		.spawn()
		.unwrap();
	let relink = async {
		let url = format!("http://127.0.0.1:{port}/me/gmbs/2/tracks/{}/relink", target.0);
		loop {
			if let Ok(r) = reqwest::Client::new().post(&url).send().await {
				break (r.status(), r.json::<Value>().await.unwrap());
			}
			tokio::time::sleep(Duration::from_millis(100)).await;
		}
	};
	let (status, relinked) = tokio::select! {
		r = relink => r,
		status = serve.wait() => panic!("serve ended: {status:?}"),
		() = tokio::time::sleep(Duration::from_secs(60)) => panic!("serve never answered"),
	};
	assert_eq!(status, reqwest::StatusCode::OK, "{relinked}");
	assert_eq!(relinked["place_id"], MOVED, "as the conductor answered: {relinked}");
	let bought = bought.lock().unwrap().clone();
	assert_eq!(bought.len(), 1, "{bought:?}");
	let (assertion, body) = &bought[0];
	assert!(!assertion.is_empty(), "the request's assertion, handed back");
	assert_eq!(body["gbp"], 2, "as the member's GMB: {body}");
	assert!(
		body["request"].as_str().unwrap().starts_with(&format!("GET\nhttps://places.googleapis.com/v1/places/{PLACE}\n")),
		"{body}"
	);
}
