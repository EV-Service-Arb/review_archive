//! Places API (New) against a stub: the text search `target add <maps-url>` does when the
//! URL carries no place id, and the name of a place added by its id.

use std::sync::{Arc, Mutex};

use axum::{
	Json, Router,
	extract::{Path, State},
	http::{HeaderMap, StatusCode},
	response::{IntoResponse, Response},
	routing::{get, post},
};
use review_archive::{
	Rejected,
	places::{details, search},
};
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct Stub {
	got: Arc<Mutex<Vec<(HeaderMap, Value)>>>,
}

async fn search_text(State(s): State<Stub>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
	let query = body["textQuery"].as_str().unwrap_or_default().to_owned();
	s.got.lock().unwrap().push((headers, body));
	if query == "Nowhere" {
		return Json(json!({})).into_response();
	}
	if query == "Broken" {
		return (StatusCode::FORBIDDEN, "API key not valid").into_response();
	}
	Json(json!({ "places": [
		{ "id": "ChIJprocopeprocopeproc", "displayName": { "text": "Le Procope" }, "formattedAddress": "13 Rue de l'Ancienne Comédie, Paris" },
		{ "id": "ChIJsecondsecondsecond" },
		{ "id": "ChIJnamesakenamesakena", "displayName": { "text": "le procope" }, "formattedAddress": "1 Rue de Lyon, Marseille" }
	] }))
	.into_response()
}

async fn place_details(State(s): State<Stub>, headers: HeaderMap, Path(id): Path<String>) -> Response {
	s.got.lock().unwrap().push((headers, json!(id)));
	match id.as_str() {
		"ChIJprocopeprocopeproc" | "ChIJmovedmovedmovedmo" =>
			Json(json!({ "id": "ChIJprocopeprocopeproc", "displayName": { "text": "Le Procope" }, "formattedAddress": "13 Rue de l'Ancienne Comédie, Paris" })).into_response(),
		_ => (StatusCode::NOT_FOUND, "not found").into_response(),
	}
}

/// The stub and its base URL (`…/v1/places`).
async fn stub() -> (Stub, String, tokio::task::JoinHandle<()>) {
	let s = Stub::default();
	let app = Router::new()
		.route("/v1/places:searchText", post(search_text))
		.route("/v1/places/{id}", get(place_details))
		.with_state(s.clone());
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}/v1/places", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	(s, url, server)
}

#[tokio::test]
async fn every_place_bearing_the_name_is_found_and_the_search_is_biased_to_the_map() {
	let (s, url, server) = stub().await;
	let got = search(&reqwest::Client::new(), &format!("{url}:searchText"), "key-123", "Le Procope", Some((48.853, 2.3389)))
		.await
		.unwrap();
	server.abort();
	let ids: Vec<&str> = got.iter().map(|p| p.place_id.as_str()).collect();
	assert_eq!(ids, ["ChIJprocopeprocopeproc", "ChIJnamesakenamesakena"]);
	let (headers, body) = s.got.lock().unwrap()[0].clone();
	assert_eq!(headers["x-goog-api-key"], "key-123");
	assert_eq!(headers["x-goog-fieldmask"], "places.id,places.displayName,places.formattedAddress");
	assert_eq!(body["locationBias"]["circle"]["center"], json!({ "latitude": 48.853, "longitude": 2.3389 }));
}

#[tokio::test]
async fn nothing_or_another_place_found_is_the_callers_mistake_and_a_refusal_is_not() {
	let (_s, url, server) = stub().await;
	let http = reqwest::Client::new();
	let url = format!("{url}:searchText");
	let nothing = search(&http, &url, "k", "Nowhere", None).await.unwrap_err();
	let refused = search(&http, &url, "k", "Broken", None).await.unwrap_err();
	let another = search(&http, &url, "k", "Le Procope of Hicksville", None).await.unwrap_err(); // #37: the first hit was a namesake
	server.abort();
	assert!(matches!(nothing.downcast_ref::<Rejected>(), Some(Rejected::Invalid(_))), "{nothing:#}");
	assert!(matches!(another.downcast_ref::<Rejected>(), Some(Rejected::Invalid(_))), "{another:#}");
	assert!(refused.downcast_ref::<Rejected>().is_none(), "{refused:#}");
	assert!(format!("{refused:#}").contains("403"), "{refused:#}");
}

#[tokio::test]
async fn a_place_added_by_its_id_is_named_and_an_unknown_id_refused() {
	let (s, url, server) = stub().await;
	let http = reqwest::Client::new();
	let got = details(&http, &url, "key-123", "ChIJprocopeprocopeproc").await.unwrap();
	let unknown = details(&http, &url, "key-123", "ChIJunknownunknownunk").await.unwrap_err();
	let moved = details(&http, &url, "key-123", "ChIJmovedmovedmovedmo").await.unwrap();
	server.abort();
	assert_eq!(moved.place_id, "ChIJprocopeprocopeproc", "a moved place comes back under the id Google gives it now");
	assert_eq!((got.name.as_deref(), got.address.as_deref()), (Some("Le Procope"), Some("13 Rue de l'Ancienne Comédie, Paris")));
	assert!(matches!(unknown.downcast_ref::<Rejected>(), Some(Rejected::Invalid(_))), "{unknown:#}");
	let (headers, _) = s.got.lock().unwrap()[0].clone();
	assert_eq!(
		(&headers["x-goog-api-key"], &headers["x-goog-fieldmask"]),
		(&"key-123".parse().unwrap(), &"id,displayName,formattedAddress".parse().unwrap())
	);
}
