//! What people and other services type in: intervals, Maps URLs, languages. Each of these
//! reaches the archive from the CLI or the HTTP API unchecked by anything else.

use review_archive_core::{maps::selectors::place_url, parse_interval, place};

/// `n * seconds-per-unit` past `u64::MAX` must be refused, not wrapped around (release) or
/// panicked on (debug) — `POST /targets {"interval": "30600000000000w"}` reaches this.
#[test]
fn an_interval_too_long_to_represent_is_an_error() {
	let got = std::panic::catch_unwind(|| parse_interval("30600000000000w"));
	assert!(matches!(got, Ok(Err(_))), "expected Ok(Err(_)), got {got:?}");
}

/// Google spells a `+` in a place name as `%2B` (a bare `+` in the path means a space).
/// Decoding must turn `%2B` into `+` once, and leave it so.
#[test]
fn a_plus_encoded_in_a_maps_url_stays_in_the_search_query() {
	let got = place::parse("https://www.google.com/maps/place/C%2B%2B+Bar/@48.85,2.33,17z").unwrap();
	assert_eq!(
		got,
		place::Parsed::Search {
			query: "C++ Bar".into(),
			near: Some((48.85, 2.33)),
		}
	);
}

/// The target's `lang` is free text from `POST /targets` / `PATCH`; it must stay the value of
/// `hl` and never add query parameters of its own (here: a second `q=place_id:`, which would
/// open another place and archive its reviews under this target).
#[test]
fn a_lang_cannot_add_query_parameters_to_the_place_url() {
	let url = url::Url::parse(&place_url("ChIJLU7jZClu5kcR4PcOOO6p3I0", "fr&q=place_id:ChIJotherotherotherother")).unwrap();
	let q: Vec<String> = url.query_pairs().filter(|(k, _)| k == "q").map(|(_, v)| v.into_owned()).collect();
	assert_eq!(q, ["place_id:ChIJLU7jZClu5kcR4PcOOO6p3I0"], "{url}");
}

/// A Maps share link expands to a URL whose only id is the feature id in `data=` (#37). It must
/// become that place's id: a search by name took Shiny Maids Queens for Shiny Maids of Hicksville.
#[test]
fn a_maps_url_names_its_place_by_the_feature_id() {
	let got = place::parse(
		"https://www.google.com/maps/place/Shiny+Maids+House+Cleaning+Services+of+Hicksville/@40.7837389,-73.5036176,17z/data=!3m1!4b1!4m6!3m5!1s0x89c281e03866d8f3:0xa74f91a21ce5b00e!8m2!3d40.7837389!4d-73.5036176!16s%2Fg%2F11zy58lwft?entry=tts",
	)
	.unwrap();
	assert_eq!(got, place::Parsed::PlaceId("ChIJ89hmOOCBwokRDrDlHKKRT6c".into()));
}

/// A share.google link (Search's share button) lands on a Google Search for the place's name;
/// that name is what Places is searched for.
#[test]
fn a_google_search_url_searches_for_its_query() {
	let got = place::parse("https://www.google.com/search?hl=en&gl=US&output=search&kgmid=/g/11zgy_6kp_&q=Bayou+Buff+Mobile+Car+Detailing+Houston+TX&shndl=30").unwrap();
	assert_eq!(
		got,
		place::Parsed::Search {
			query: "Bayou Buff Mobile Car Detailing Houston TX".into(),
			near: None,
		}
	);
}
