# Architecture

What the service does and why is in [SPEC.md](SPEC.md). This is where things live.

A cargo workspace of five crates; dependencies point inwards only.

```text
crates/review_archive_core/     no I/O: no browser, database, network or clock
    src/lib.rs                    targets, observations, Known, Scan, Coverage, content hashes;
                                what callers type in (intervals, langs, dates) and Rejected
  src/reconcile.rs              a scan against what is stored → new / changed / unchanged / gone / reappeared
  src/schedule.rs               when a target is next due: interval, jitter, backoff
  src/tokens.rs                 risk tokens: daily renewal, splitting a charge, the walk's meter
  src/relative_date.rs          "il y a 3 semaines" → an estimated timestamp, and its earliest bound
  src/maps/selectors.rs         every assumption about Google's markup, and the in-page scripts
  src/maps/parse.rs             cards out of HTML (tested on tests/fixtures/, insta snapshots)
    src/maps/mod.rs               walk policies, and what a walk may conclude (coverage, gaps)
  src/gbp.rs                    the Business Profile API's JSON; matching API reviews to Maps cards
  src/place.rs                  a place id out of what a person pastes
    src/dto.rs                    the JSON of the HTTP API (bodies and queries), shared with the client
crates/review_archive/          the engine (features: maps, store)
  src/archive.rs                the `Archive` facade
  src/archive/members.rs        what a member does, scoped to them
  src/browser/                  the `Browser` handle over `browser_manipulation`: consent, sorting, the walk, screenshots
  src/sources/                  the `ReviewSource` port; the maps and gbp adapters
  src/store/                    SQLite (runtime sqlx queries, embedded migrations/), WebP blobs, export
  src/store/jobs.rs             the job queue (on-demand scans and ad-hoc captures)
  src/store/events.rs           events into the outbox (hooks, members' Telegram channels), in the scan's own transaction
  src/store/members.rs          per-member state: gmails, tracks, reinstatements, Telegram channels
  src/store/people.rs           people by concierge `sub`; claiming an address's rows from before
  src/store/tokens.rs           the token ledger: balances, who pays for a scan, the account's hourly limit
  src/record.rs                 one scan of one target into the store: run row, source, blobs, reconcile, write
  src/failure.rs                the typed errors (miette codes and help), `describe`, and who a failure waits for
    src/webhooks.rs               where a hook may point; delivering the outbox: signature, retries, the Telegram bot
  src/places.rs                 Places API search for URLs without an id
crates/review_archive_server/   the `review_archive` binary: CLI, HTTP, background loops; thin over `Archive`
  src/http.rs                   the API and its OpenAPI document (utoipa, `GET /openapi.json`), `/mfe/`, the page at `/`,
                                `/grafana/` to the Grafana `serve` runs (`grafana`, `--dev-member` only)
  src/auth.rs                   who is calling: the panel's assertion (`sa_auth`), a person by `sub`; the
                                permission guards; whose `/me` it is (`X-Member`)
  src/worker.rs                 the browser's worker (queued jobs, then due targets) and the deliverer
  src/settings.rs               the environment (ev_lib `settings!`): secrets, APP_ENV
  src/config.rs                 the config (v_utils `Settings`): data dir, bind, and every library and server section
crates/review_archive_client/   typed async client of the HTTP API through the panel, on the core's DTOs; native and wasm
crates/review_archive_web/      the dashboard MFE (dioxus, wasm), over the client; `package.sh` lays out /mfe/, `index.html` is `/`;
                                its `grafana` feature frames Grafana's overview and Stats tile (`nix run .#with-grafana`)
grafana/                        those dashboards, over the API through the Infinity datasource
```

## Using the library

Embed the engine instead of calling a running archive:

```toml
review_archive = { version = "0.1", default-features = false, features = ["maps"] }
```

```rust
use review_archive::{Archive, CaptureRequest, config::Config};

// No data dir: nothing stored, the reviews and WebPs come back in memory.
let mut config = Config::default();
config.browser.profile_dirs = vec!["/var/lib/my-service/chromium".into()];
config.browser.executable = Some("/usr/bin/chromium".into());
config.browser.lean = Some(vec![]); // memory-for-speed trades: § Memory
let archive = Archive::open(config).await?;
let got = archive.capture_place(&CaptureRequest::new("ChIJLU7jZClu5kcR4PcOOO6p3I0").lang("fr").max_reviews(20)).await?;
for r in &got.scan.reviews {
    // got.webps[&r.source_review_id]: the WebP, provenance in Exif, for each card screenshotted
}
archive.close().await;
```

The process needs the Playwright driver `browser_manipulation` names (`PLAYWRIGHT_CLI_JS`,
`PLAYWRIGHT_NODE_EXE`, `PLAYWRIGHT_SKIP_DRIVER_DOWNLOAD`; this flake's devShell and image set them).

With `store` and `Config::data_dir` set, the same `Archive` also does `add_target`,
`scan_target`, `stats`, `export`, `reviews`, and gives the schedule (`due`, `next_due`).
A caller that owns a Chromium already passes it with `Archive::open_with_browser`; one
with its own platform implements `sources::ReviewSource` and records through
`Archive::record`. With only HTML in hand, `review_archive_core` parses
(`maps::parse::cards`) and reconciles (`reconcile::plan`) without any of it.

## Invariants

- **Nothing is ever written to Google.** Sources read. There is no stealth, no fingerprint
  masking, no proxy, no CAPTCHA solving. The accounts in `browser.profile_dirs` are used in
  their order, and only `signed_out` moves a walk to the next; a flag on the address
  (`blocked`, `limited_view`) never does. When Google does not serve the
  full page — the "unusual traffic" page, or its "limited view" without reviews — the run
  fails and the error says which. A profile signed out of Google cannot walk: Maps shows it
  a place's first few reviews only, unsorted, which is no archive. The walk starts over on the
  next signed-in profile (it is seen before any card is read), and the account is kept signed
  out (`profiles`, SQLite, so a restart keeps it) and logged at error level until a person signs
  it in again with `scripts/new_google_session.py`; the standby accounts are opened on Maps at
  least every `browser.standby_touch`, to find one signed out before a walk needs it. `serve` checks at
  boot, before any walk: Maps opened on the accounts in order until one is signed in. With every
  account signed out the boot or walk fails (`signed_out`, naming each account and since when) and
  `serve` ends with exit 78, which devops holds the deployment on until a person signs one in and
  resumes it. A failed walk saves the
  page as `<data_dir>/artifacts/browser_captures/<UTC time>-<step>.{png,html}` (kept `browser.artifacts_retention`),
  and its error names them as `[<path>]`s — which is what the server's alerts attach, from
  under `<data_dir>/artifacts/` only. A Chromium start that outran Playwright's 30 s fails
  the run as `launch_timeout`: with the memory it needs Chromium starts in 4–6 s, so the limit
  detects a pod short of memory, and is not raised. Each saves the host's figures next to the
  captures as `<UTC time>-launch.txt` (the newest 10 kept), named in its error as a `[<path>]`.
  Read them as `browser/host.rs` documents them, the pod's first: the lease (`leased`) against
  `memory.peak`, the `max` and `oom_kill` counters (since the container started) and the
  cgroup's own memory pressure; `memory.current` includes page cache and sits near its limit
  regardless. The node's IO pressure is inflated wherever tasks wait on `io_uring`
  (TigerBeetle), and says little there.
- **What stops Maps pauses Maps, not a target.** Google flags the address (`blocked`,
  `limited_view`), and Chromium that cannot run (`launch_timeout`, `no_memory`, and a page that
  stopped answering, `unresponsive`) cannot run for any target, so each trips `maps_breaker` (SQLite, so a restart keeps it), logged at error
  level with the error's `[<path>]`s: no Maps walk
  for any target until its probe time (`schedule.backoff_base`, doubling to `schedule.backoff_cap`), then one probes; a walk that
  gets through clears it. `gbp` lists go on, their screenshots wait. What retrying cannot
  fix — changed markup or consent page, a refused GBP grant — halts that source until a
  restart; a Chromium setup that is broken, or every account signed out, ends `serve` with exit 78
  (`EX_CONFIG`), which devops holds rather than restarts.
- **A failure is known, or a bug.** `failure::remedy` says who each waits for; one no arm
  knows is `Unknown`: retried on the backoff, and reported by the server at error level with
  its chain and backtrace (`RUST_LIB_BACKTRACE=1`), which is one Sentry event and a Discord
  line. Known failures, and jobs refused as `Rejected`, are warnings. A queued job's failure
  reaches that report as typed as a scheduled scan's.
- **History is append-only, while anyone tracks it.** Reviews and captures are never deleted; `review_versions` gets a
  row per distinct content, the first sighting included. `gone_at` is set and cleared, never a
  deletion. A place whose last track went is off the schedule, and `defaults.untracked_retention`
  later it is deleted with its archive and runs; the ledger rows and kick-backs of those runs move to
  the `other` run (id 0, on the `other` target no lookup finds). A target never tracked is the
  operator's, and is never purged.
- **A walk that stops short leaves no hole.** A scan stops at a screen of archived cards
  only once it is past the last card a previous walk was cut short at
  (`targets.cut_after`), and past every review still waiting for its screenshot; a
  target's first scan reads the whole list. Ad-hoc captures may record a cut, never
  clear one.
- **`gone` is only concluded where the scan looked.** `gbp` lists everything, so absent means
  gone — unless the API returned fewer reviews than its own `totalReviewCount`. A `maps` walk is
  newest first and stops early; it only judges reviews whose *earliest* possible date (the
  estimate less one unit of its phrase: "a month ago" spans a month) is no earlier than the
  latest possible date of the last card it read. That needs the list to have visibly re-sorted
  to newest and its dates to run newest first; otherwise the run judges nothing. A walk judges
  everything only when it read as many cards as the list's histogram counts — an idle feed
  alone may be a stalled lazy load. A "complete" scan that lists nothing, against an archive
  with live reviews, is taken as a broken response: nothing is marked gone, the run is
  `partial`.
- **One process per browser profile.** `review_archive.lock` in each profile says so, taken
  before Chromium starts on it (`browser_manipulation`'s own lock, which clears stale `Singleton*`
  files, is only taken at launch). The profile is claimed before a run is recorded, so a second `scan` while `serve`
  holds it fails with that reason and leaves no run behind. A browser that failed under a walk
  is closed; the next walk starts a new one.
- **Markup knowledge lives in `selectors.rs`.** A Maps change is fixed there, against fixtures
  refreshed with `scan --dump-html`, and checked by `cargo insta review`.
- **The core has no I/O.** Time comes in as an argument or through `Recorder::now` (a plain
  `fn`, pinned in tests); jitter is derived from the target and its last run, so asking twice
  gives one answer.
- **The caller's mistakes are `Rejected`.** The facade takes the API's DTOs as they come and
  answers `NotFound`, `Invalid` or `Busy` for what the caller got wrong; the server only maps
  them to 404, 400 and 429, and anything else to a logged 500.
- **One write lock per transaction.** A write that reads first begins `IMMEDIATE`, so `serve`'s
  worker, its HTTP side and a hand-run `scan` never fail on each other's writes. A scan's
  reviews, events, cut, run end, job end and token charges commit together.
- **One `serve` per data dir does the background work.** `serve.lock` in the data dir, held
  for the process's life, gates the worker, the deliverer and the blob conversion; a second
  `serve` on the same dir (a rolling update's newcomer) answers HTTP and waits for it.
- **One browser, one queue.** A single worker uses the browser, on one profile at a time
  (another only when an account is signed out, or to touch a standby while idle): queued jobs first
    (`POST /targets/{id}/scan`, `POST /captures`), oldest first — but no more than `worker.jobs_in_a_row`
  while a target is overdue — then the most overdue target, with a `schedule.pause_{min,max}` pause
  between any two. The queue is bounded (`defaults.max_queued_jobs`) and a job already queued is not
  queued twice. Jobs live in SQLite; a restart keeps the queued ones and fails the job and the runs that were running — a
  hand-run `scan` still going then included, until it records how it really ended. An ad-hoc capture is stored under the place's `maps` target for its
  language, or a new disabled one: nothing captured is lost, nothing extra gets scheduled, and
  its run does not count for the target's schedule. No job's run does: a scan a member asks
  for is extra, and never moves the schedule or adds to its backoff.
- **Chromium runs under a memory lease.** The pod's base memory is far below what Chromium
  needs; devops' broker raises the limit to its burst while a `v_utils::memory_lease::Lease` lives
  (`MEMORY_LEASE_URL`). The browser takes one before it starts Chromium and keeps it until that
  Chromium is closed or dies, so a started browser is never shrunk under; WebP encodes take their own
  short ones on top. Waiting for the burst is not part of the 30 s start limit. No lease, no
  Chromium: a broker that refuses, cannot fit the burst or is silent for 60 s fails the run as
  `no_memory`, which pauses Maps. Without `MEMORY_LEASE_URL` the lease is inert (a local run).
- **A stall is not markup.** A page that does not answer a script in 20 s is `unresponsive`,
  and nothing it shows is judged: the selector-miss checks (`markup_changed`, `limited_view`,
  a missing place) only run on a page that answered. Every walk logs the host's figures
  (`browser/host.rs`, per-process PSS included), failed or not.
- **Public facts are shared; per-member state only annotates.** Targets, reviews, versions,
  captures and blobs belong to no one: a place tracked by several members is one target,
  scanned once. What is a member's — gmails, tracks, reinstatements, Telegram channels —
  points at those facts and never alters them (tracking may put a disabled target back on
  the schedule; a target whose every track is switched off is left off it); every member query is scoped by the member's person id in SQL.
- **Tokens are a ledger; a balance is its sum; members pay for the scans of their places.**
  `token_ledger` rows are only added (renewal, an admin's set as a difference, charges). A
  walk is metered by `maps::cost` and stops at what its payers hold and the account's hour
  (`tokens.per_hour`) leaves; it is charged in the run's end transaction. A run that failed,
  or met any failure, kicks its charges back to its payers in that transaction, pending until
  the member claims them; the account's hour counts it still. A tracked place whose members are out of tokens is
  not walked. A scan a member asks for is theirs alone to pay; the operator's jobs and
  ad-hoc looks are no one's.
- **Events are an outbox.** `review.new/changed/gone/reappeared` and `run.failed` are written
  to `webhook_deliveries` in the same transaction as what they report — a row per hook, and
  per Telegram channel of a member tracking the target — and delivered from there, retried
  with backoff (`webhooks.first_retry` doubling, cap `webhooks.retry_cap`, `webhooks.max_attempts` tries), recipients in parallel, at least once. To
  hooks: signed (`X-Signature: sha256=<HMAC-SHA256 of the body>`); `X-Delivery-Id` lets a
  receiver drop repeats. Where a hook may go: with `webhooks.allowed_hosts` empty, public
  addresses only — its URL is checked when added and every address its host resolves to when
  sent; with it set, only the hosts it lists (private addresses allowed), nowhere else.
  Redirects are not followed.
- **Secrets come from the environment only**, through `ev_lib::settings` in the server
  (`PANEL_ASSERTION_KEYS`, `GOOGLE_MAPS_KEY`, `GBP_*`, `TELEGRAM_BOT_TOKEN`, `SENTRY_DSN`,
  `ALERT_WEBHOOK_*`); the
  library takes them as `config::Secrets` and never reads the environment. Each is required
  only by what uses it and a missing one fails that with its name — except
  `PANEL_ASSERTION_KEYS`, required at boot when `APP_ENV=production`
  (`review_archive --print-required-vars` lists what a profile needs).
- **A capture is the review as it first appeared.** Cards are screenshotted when new (or while
  `capture_pending`), after "More" is expanded. It is kept as WebP, its provenance in Exif,
  under its SHA-256. A corrupted one (cut off, #36) is kept too, and the review stays pending.
  The walk scrolls the feed vertically only, with the side panel open; a corrupted shot is
  marked, never retaken in the same pass.

## Testing

Live tests never sign in to Google, nor reuse a signed-in profile: they walk as `browser.guest`,
and a signed-out Maps shows at most 5 reviews of a place, unsorted, which is enough to test a walk and its shots.

## Memory

The Pi gives Chromium a 1Gi burst. The walk needs one Maps page, scrolled and screenshotted card
by card, so each layer is cut to that (`browser/session.rs`, `selectors.rs`):

```text
cgroup (burst)
├─ server + encodes
├─ node driver        a capped heap, no optimizing JIT (`leanNode`, the image's `PLAYWRIGHT_NODE_EXE`): it
│                     carries every screenshot as base64
├─ browser process    one launch: the headed UA is cached in the profile, no probe Chromium; the network
│                     service, audio and GPU run in it, no zygote; no extensions, component updates or
│                     optimization-guide models
│  └─ GPU             no map to draw: tiles, satellite, Street View, 3D blocked (`sel::BLOCKED`)
├─ renderer           one (`--renderer-process-limit=1`, no site isolation); cards already read
│                     are released (`js::RELEASE`: each becomes an empty spacer of its height)
│  └─ frame buffers   1280×900 at 1x; a card alone is rasterized at 2x (`Shot::Element`)
├─ storage service    the one utility process left
└─ page cache         32 MiB disk cache
```

A GPU crash now takes the browser down with it: the walk fails and the next one starts a new
browser. Every walk logs the page's JS heap (`js_heap`, used of limit) beside the host's figures, to
size `js_heap_mib` by.

Always on, as they cost the walk nothing it uses. What costs speed is `browser.lean`, which every
config states (`[]` for none) and the Pi's sets in full. Switch the trades off one at a time when the
host has room:

| `browser.lean` | Costs | Saves |
|---|---|---|
| `{ js_heap_mib = n }` | more GC, slower JS | caps the Maps JS heap (`--max-old-space-size`, `--optimize-for-size`) |
| `small_young_generation` | more frequent minor GC | a 1 MiB young generation |
| `js_lite_mode` | no optimizing JIT, much slower JS | JIT code and type feedback |
| `one_raster_thread` | slower paint | raster buffers per thread |
| `low_end_device` | Chromium's low-RAM mode: fewer caches, lower-quality tiles | broad |

Beside it, and of the same kind: the driver's flags in the image (`flake.nix`), and a longer
`browser.step_wait` in the Pi's config, so V8 collects between scrolls.
