//! The operations table against the human overview of it in docs/ARCHITECTURE.md.

use std::collections::BTreeSet;

/// `{a,b}` in a path is two paths; `{id}` is a parameter.
fn expand(path: &str) -> Vec<String> {
	let mut at = 0;
	while let Some(open) = path[at..].find('{').map(|i| at + i) {
		let close = open + path[open..].find('}').expect("a brace closes");
		if path[open..close].contains(',') {
			return path[open + 1..close]
				.split(',')
				.flat_map(|alt| expand(&format!("{}{alt}{}", &path[..open], &path[close + 1..])))
				.collect();
		}
		at = close;
	}
	vec![path.to_owned()]
}

#[test]
fn the_overview_names_every_operation_and_no_other() {
	let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/ARCHITECTURE.md")).unwrap();
	let section = &doc[doc.find("## The API").expect("the section")..];
	let block = &section[section.find("```text\n").expect("its block") + 8..];
	let block = &block[..block.find("```").expect("closed")];
	let overview: BTreeSet<(String, String)> = block
		.lines()
		.flat_map(|l| {
			let mut words = l.split_whitespace();
			let method = words.next().expect("a method").to_owned();
			expand(words.next().expect("a path")).into_iter().map(move |p| (method.clone(), p))
		})
		.collect();
	let table: BTreeSet<(String, String)> = review_archive_server::http::document()
		.operations()
		.map(|(path, method, _)| (method.to_uppercase(), path.to_owned()))
		.collect();
	assert_eq!(overview, table);
}

/// Every operation says what it refuses; the document's views are the dashboard's, each parsing its own href
/// (`View::table()` refuses to build otherwise).
#[test]
fn every_operation_lists_its_refusals_and_the_document_its_views() {
	let doc = serde_json::to_value(review_archive_server::http::document()).unwrap();
	for (path, item) in doc["paths"].as_object().unwrap() {
		for (method, op) in item.as_object().unwrap() {
			assert!(op["x-errors"].is_object(), "{method} {path} has no x-errors");
		}
	}
	let views = doc["x-views"]["views"].as_object().unwrap();
	assert!(["pick", "mine.home", "mine.gmb", "theirs.settings"].iter().all(|v| views.contains_key(*v)), "{:?}", views.keys());
}
