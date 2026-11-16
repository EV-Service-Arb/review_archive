## HTTP

`GET /health` answers without auth; the operations are in [ARCHITECTURE.md § The API](docs/ARCHITECTURE.md#the-api),
described in full by `GET /openapi.json`.

## When Maps changes

Selectors live in `src/sources/maps/selectors.rs` and nowhere else. Refresh the fixtures with
`review_archive serve --dump-html tmp/dump` and a scan of the place (`POST /targets/{id}/scan`; a
failing step also dumps the whole page and a screenshot there), fix the selectors, and
`cargo insta review` the parser snapshots.

The live test is `cargo test --test live -- --ignored`; it needs a browser and the network.
