# review_archive
![Minimum Supported Rust Version](https://img.shields.io/badge/nightly-1.100+-ab6000.svg)
![Lines Of Code](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/review_archive-loc.json)
![Build time](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/review_archive-build.json)
![Test time](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/review_archive-test.json)
<br>
[![errors](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/review_archive-errors.json)](https://github.com/EV-Service-Arb/review_archive/actions/workflows/errors.yml?query=branch%3Amain)
[![warnings](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/review_archive-warnings.json)](https://github.com/EV-Service-Arb/review_archive/actions/workflows/warnings.yml?query=branch%3Amain)

An archive of public place reviews. `review_archive` watches a list of places, and every review it
has not seen before is stored twice: as structured data in SQLite, and as a WebP screenshot of the
review card as it first appeared. Edits are kept as history, never overwritten; a review that stops
being listed is marked gone, and unmarked if it comes back. Statistics and exports are read from
what was stored.

Two sources. `maps` reads any public place through a plain headless Chromium at a polite rate.
`gbp` reads the places we manage through the official Business Profile API, which is complete and
authoritative, and still takes its screenshots from the public Maps page.

It only reads. Nothing is posted, replied to, reported or appealed, and there is no fingerprint
masking, proxy or account rotation, or CAPTCHA solving: if Google blocks the scanner, the run fails
and says so. See [docs/SPEC.md](docs/SPEC.md) for what it does and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for where.
<!-- markdownlint-disable -->
<details>
<summary>
<h2>Installation</h2>
</summary>

```sh
cargo install --path .
```


</details>
<!-- markdownlint-restore -->

## Usage
```sh
# Everything lives under one data dir: `review_archive.db`, `blobs/` with the WebPs, and the
# browser profile that remembers the consent answer. Set it, the bind address and the defaults in
# a TOML file passed as `--config`; secrets only ever come from the environment.
# Scheduler + HTTP on 127.0.0.1:59110. The API wants the conductor's assertion
# (`x-sa-assertion`, keys in PANEL_ASSERTION_KEYS; docs/SPEC.md, Auth): targets, scans, captures,
# stats and exports are its operations, reached through the conductor (`/api/review_archive/*`).
review_archive --config config.toml serve

# Locally, without the conductor: every request signed in as one member holding `sa:admin`,
# the dashboard at http://127.0.0.1:59110/ (`nix run .#dev-mfe` builds and serves it).
review_archive --config config.toml serve --dev-member sa:admin
```

```toml
# config.toml — every key is optional (`review_archive config write-defaults` lists them all),
# but a scan needs `browser.executable` and `browser.lean`
data_dir = "./data"
bind = "127.0.0.1:59110"

[browser]
executable = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
lean = [] # memory-for-speed trades; see docs/ARCHITECTURE.md § Memory

[defaults]
lang = "en"
interval = "1d"
max_reviews_per_scan = 200
max_reviews_initial = 2000
```

## HTTP

`GET /health` answers without auth; the operations are in [ARCHITECTURE.md § The API](docs/ARCHITECTURE.md#the-api),
described in full by `GET /openapi.json`.

## When Maps changes

Selectors live in `src/sources/maps/selectors.rs` and nowhere else. Refresh the fixtures with
`review_archive serve --dump-html tmp/dump` and a scan of the place (`POST /targets/{id}/scan`; a
failing step also dumps the whole page and a screenshot there), fix the selectors, and
`cargo insta review` the parser snapshots.

The live test is `cargo test --test it live -- --ignored`; it needs a browser and the network.


<br>

#### Dev links
- <sup><a href="./docs/ARCHITECTURE.md">architecture</a></sup>
- <sup><a href="https://github.com/EV-Main/.github/blob/main/code/STYLE.md">style</a></sup>
- <sup><a href="https://github.com/EV-Main/.github/tree/main/code/conventions">conventions</a></sup>
- <sup><a href="https://github.com/EV-Main/.github/blob/main/code/conventions/git">contributing</a></sup>

#### License

<sup>
	Licensed under <a href="LICENSE">Blue Oak 1.0.0</a>
</sup>

<br>

<sub>
	Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be licensed as above, without any additional terms or conditions.
</sub>

