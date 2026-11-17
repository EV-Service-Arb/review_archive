//! What people and other services type in: intervals, languages. Each of these
//! reaches the archive from the CLI or the HTTP API unchecked by anything else.

use review_archive_core::{maps::selectors::place_url, parse_interval};

/// `n * seconds-per-unit` past `u64::MAX` must be refused, not wrapped around (release) or
/// panicked on (debug) — `POST /targets {"interval": "30600000000000w"}` reaches this.
#[test]
fn an_interval_too_long_to_represent_is_an_error() {
	let got = std::panic::catch_unwind(|| parse_interval("30600000000000w"));
	assert!(matches!(got, Ok(Err(_))), "expected Ok(Err(_)), got {got:?}");
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
