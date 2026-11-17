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
