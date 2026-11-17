//! What is derived from the document and committed: the client's operations, and the document the
//! conductor vendors. `REVIEW_ARCHIVE_GEN=1` writes them (`nix run .#gen`); otherwise a stale one fails.

use std::{
	io::Write,
	process::{Command, Stdio},
};

/// `source` as the pre-commit's `rustfmt` leaves it, under the repo's `rustfmt.toml`.
fn formatted(source: String) -> String {
	let mut rustfmt = Command::new("rustfmt")
		.args(["--edition", "2024", "--emit", "stdout"])
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.spawn()
		.expect("rustfmt is on the devShell's PATH");
	rustfmt.stdin.take().expect("piped").write_all(source.as_bytes()).unwrap();
	let out = rustfmt.wait_with_output().unwrap();
	assert!(out.status.success(), "rustfmt refused the generated client");
	String::from_utf8(out.stdout).expect("rustfmt writes UTF-8")
}

#[test]
fn the_committed_client_and_document_are_the_documents() {
	let doc = serde_json::to_value(review_archive_server::http::document()).unwrap();
	let derived = [
		(
			concat!(env!("CARGO_MANIFEST_DIR"), "/../review_archive_client/src/generated.rs"),
			formatted(ev_lib::api::rust_client(&doc, "review_archive_core::dto")),
		),
		(concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.json"), serde_json::to_string_pretty(&doc).unwrap() + "\n"),
	];
	let write = std::env::var_os("REVIEW_ARCHIVE_GEN").is_some();
	for (path, want) in derived {
		match write {
			true => std::fs::write(path, want).unwrap(),
			false => assert!(
				std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}; `nix run .#gen` writes it")) == want,
				"{path} is not the document's: `nix run .#gen`"
			),
		}
	}
}
