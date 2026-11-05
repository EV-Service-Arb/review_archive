# review_archive — spec

Archive of public place reviews: a WebP screenshot of every review as it first
appears, plus structured data for statistics. Google first; the source layer is a
trait so other platforms slot in later.

Replaces a person manually checking places and saving screenshots.

## Scope

In:

- Watch a list of **targets** (places) on an interval (hours, not seconds).
- On each scan: detect reviews not seen before, store their data, take a WebP
  screenshot of the review card, keep edit history, note reviews that are no
  longer listed.
- Statistics and export over what was stored.

Out — do not build, even as an option:

- Posting, editing or replying to anything; any write call to Google.
- Browser fingerprint masking, "stealth" plugins, proxies, account rotation (the
  accounts are a fixed order, moved along only when one is signed out), CAPTCHA solving. The scanner is a plain headless Chromium at a polite rate; if
  Google blocks it, the run fails and is reported.
- Automatic submission of anything to Google (appeals, reports).

## Sources

`trait ReviewSource { async fn scan(&self, target: &Target, known: &Known) -> Result<Scan> }`

### `maps` — any public place (headless browser)

- Chromium through `browser_manipulation` (Playwright protocol, patchright driver). One
  browser process, targets scanned sequentially.
- URL: `https://www.google.com/maps/place/?q=place_id:<PLACE_ID>&hl=<lang>`.
- EU consent interstitial (`consent.google.com`): click the reject-all button;
  persist the resulting cookies in the profile dir so it is not shown again.
- Each browser profile (`browser.profile_dirs`, primary first) is signed in to its own Google
  account: signed out, Maps shows a place's first few reviews only and will not sort them. That
  walk starts over on the next signed-in profile, and the account stays signed out
  (`profiles(path, signed_out_at, used_at)`) and alerted on until a person signs it in again
  (`scripts/new_google_session.py <profile dir name>`, which clears its `signed_out_at`: the
  primary goes first again). Only `signed_out` switches accounts; `blocked` and `limited_view`
  never do. An idle worker opens Maps on a standby profile unused for `browser.standby_touch`,
  to find its account signed out early. With every account signed out the walk fails
  (`signed_out`) and `serve` exits 78, for the deployment to be held until one is signed in.
- On the place's overview, read the place's name and address (`targets.name`, `targets.address`,
  every walk, so a renamed or moved place follows here; an address the place hides is cleared), and the owner's latest post under "From the owner" (text,
  date as printed); it is kept once per distinct text (`posts`), with when it was first
  and last seen. Only the latest shows there: two posts between scans keep the newer.
- Open the Reviews tab; its histogram gives the place's count and star average (`targets.rating`).
  When the count is the one the last scan read
  (`targets.listed`; ad-hoc captures do not set it) and accounted for, no gap is owed, and every review is
  completed — screenshotted, and replied to by the owner — stop there: the run lists nothing
  and judges nothing. Otherwise sort by *Newest*, scroll the feed. Stop when a full screen of
  cards is already known (cards still without a screenshot do not count, and the walk
  goes on until it has read or passed every review not completed, and until the archive's live
  reviews, the new cards read and `targets.unreachable` add up to the count), or at `max_reviews_per_scan`
  (default 200).
  First scan of a target walks the whole list (bounded by
  `max_reviews_initial`, default 2000), whatever ad-hoc captures stored before it.
- A walk cut short before reaching archived cards — by its limit, or by the page failing
  under it — records its last card (`targets.cut_after`). The next scan reads past that
  card before a screen of known cards may end it, with `max_reviews_initial` as its
  limit, so what lay below is not skipped for good. A failure after cards were read
  keeps them (the run is `partial`); a block by Google still fails the run.
- Per card (`[data-review-id]`): click *More* to expand, extract review id,
  author name, author profile URL, star rating (from `aria-label`), relative
  date text, text, owner reply, photo count; element screenshot → PNG, kept as WebP.
- Selectors live in one module with HTML fixtures under `tests/fixtures/` and
  parser tests on them — the page changes, and the fix must be one file.

### `gbp` — profiles we manage (official API)

- Google Business Profile API, `GET https://mybusiness.googleapis.com/v4/accounts/{account}/locations/{location}/reviews`
  (`pageSize=50`, paginate with `nextPageToken`; a token seen twice, or more than 1000
  pages, fails the run). `account` and `location` are numbers. OAuth2 refresh-token flow;
  `GBP_CLIENT_ID`, `GBP_CLIENT_SECRET`, `GBP_REFRESH_TOKEN` from env.
- The API gives the authoritative, complete list (id, reviewer, starRating,
  comment, createTime, updateTime, reply), so `gone` detection is exact here.
- Screenshots still come from the public Maps page: for each new API review, run
  the `maps` capture on the target's `place_id` and match the card by
  (author name, rating, normalised text prefix). Unmatched → no capture row,
  `capture_pending` stays set, retried on the next scan (Maps lags the API).
- A `gbp` target therefore needs both `gbp_account`/`gbp_location` and
  `place_id`.

### Place id resolution

`target add` resolves through `sa_places` (the conductor's crate, which its GMBs use too): a place id, a Google Maps URL, or a share link (`maps.app.goo.gl`, `share.google`, expanded to the Maps or Search URL behind it; a Search URL's `q` is the name). From a URL, take the id it carries
(`!1s0x…:0x…` in `data=` is the place id, re-encoded). A cid alone (`?cid=`, a Search viewer's `svid`) makes an id Places details answers with the place's own; otherwise search Places API (New)
`places:searchText` (`GOOGLE_MAPS_KEYS`) for the URL's name. A place is never guessed: only
hits bearing that exact name count, and when several do, the operator adds one by its id.

## Storage

SQLite (sqlx, runtime `sqlx::query`, embedded migrations) + content-addressed
blob dir. Everything under one data dir (`/data` in the container).

- `targets(id, label, name, address, kind[maps|gbp], place_id, gbp_account, gbp_location, lang, interval_secs, enabled, created_at, cut_after)`
  — `lang` is a language tag (`fr`, `pt-BR`); anything else is refused.
  — `label` is a person's, `name` and `address` the place's (Places when added, with a key; then every walk);
  a target is shown as `label`, else `name`, else `place_id`.
- `reviews(id, target_id, source_review_id, author, author_url, rating, text, reply, published_raw, published_est, first_seen, last_seen, gone_at, content_hash, capture_pending)`
  — unique `(target_id, source_review_id)`.
  — `published_raw` is the date text `published_est` was estimated from; a later
  reading ("Edited a day ago") does not replace it.
- `review_versions(review_id, seen_at, content_hash, rating, text, reply)` — a
  row whenever `content_hash` changes; history is never overwritten.
- `posts(id, target_id, content_hash, text, published_raw, published_est, first_seen, last_seen)`
- `captures(id, review_id, captured_at, sha256, width, height, page_url, scanner_version, corrupted)`
  — `corrupted`: the card was shot shifted, its left part cut off (#36), told by its right edge
  being Maps' panel edge or map rather than the card; NULL until checked (`serve` checks the
  older ones once). It is kept, its review stays `capture_pending`, and the first capture that
  is not corrupted is the one shown.
- `runs(id, target_id, started_at, finished_at, status[ok|partial|failed], error, n_seen, n_new, n_changed, n_gone, ad_hoc, tokens)`
  — `ad_hoc` marks the runs of ad-hoc captures; `tokens` is what its walk cost (see Tokens), a failed one's too. A run a stopped process left open is
  failed ("interrupted") on the next start of `serve` — a hand-run `scan` still going at
  that moment included; its run is marked ended again, with its real outcome, when it
  finishes.
- `<data>/google_cache/`: every Places answer, one file per request (`sa_places`): details kept
  for good, a search a day; re-linking asks afresh.
- Blobs: `<data>/blobs/<sha256[0..2]>/<sha256>.webp`, lossy (quality 80). Exif carries the provenance:
  capture time (`DateTimeOriginal`, `OffsetTimeOriginal` `+00:00`), page URL (`DocumentName`),
  target label (`ImageDescription`), source review id (`ImageUniqueID`), scanner version
  (`Software`); text as UTF-8.

`gone`: set when a review is absent from a scan that covered its position
(`gbp`: always complete; `maps`: only if the scan walked past its
`published_est`). Cleared if it reappears. Reviews and captures are never
deleted.

## Scheduling

- Per-target `interval_secs`, default `defaults.interval` (1 d), minimum `schedule.min_interval`
  (1 h), ±`schedule.jitter` (10 %). A tracked target goes by its payers' `people.interval_secs`
  instead (`PATCH /me/settings`, every place they pay for): the shortest, a payer who chose
  none counting as `defaults.interval`.
- One scan at a time; a random pause between targets, `schedule.pause_{min,max}` (5–15 s).
- Failure → `runs.status=failed` with the error, exponential backoff for that
  target (`schedule.backoff_base` doubling to `schedule.backoff_cap`: 1 h → 24 h), other targets continue —
  unless Maps cannot work (Google's block or limited view, a Chromium that cannot start or has
  no memory lease): then every Maps walk waits, and one probes on the same backoff. A scan asked
  for (a job) waits only `schedule.job_probe_gap` (30 s) after the last trip, and probes then: it
  clears the pause, or trips it again.
- A failure the code knows is a warning; one it does not is a bug, reported with its trace to
  Sentry and Discord. A place's card on the dashboard shows why its last run failed (or what
  it warned of) on hover.
- A tracked target whose members hold fewer tokens together than a walk's first screen
  costs is held, not scanned, until a balance renews or is topped up. All walks together
  spend `tokens.per_hour` at most in any hour; past it Maps closes until older runs leave
  the window, like the breaker.
- Ad-hoc captures and jobs run on the same browser but are not the target's schedule: their
  runs neither delay the next scan nor count as failures.
- A scan whose browser profile another process holds (a `scan` beside a running
  `serve`) fails with that reason and records no run.

## Interfaces

CLI (`clap`):

- `review_archive target add <place-id|maps-url> [--label] [--lang fr] [--interval 6h] [--gbp <account>/<location>]`
- `review_archive target list | disable <id> | enable <id>`
- `review_archive scan <target-id|--all>` — one pass now, prints a summary.
- `review_archive serve` — scheduler + HTTP.
- `review_archive check-browser` — starts the configured Chromium as a walk does and shoots an element; the image's build runs it.
- `review_archive export --target <id> [--since <date>] --out <dir|file.zip>` — captures (WebP) + `manifest.json`.

HTTP (axum), behind the Service-Arb panel's assertion (see Auth), binds `127.0.0.1` unless
configured:

- `GET /health` (no auth)
- `GET /targets`
- `GET /targets/{id}/reviews?since=&gone=`
- `GET /captures/{sha256}.webp`
- `GET /stats?target=&from=&to=` — per target and per day: new, changed, gone,
  mean rating, rating histogram. `Accept: text/csv` → CSV. `gone` counts what the runs
  of that day marked gone, whether or not it came back later.

Errors are JSON (`{"error": …}`) with 400 for input the archive cannot use (a body or a
query it cannot read included), 404 for what does not exist, 429 when the job queue is
full, and a bare `internal error` for a 5xx. API requests take `http.request_timeout` at most (an export as long as it
needs) and `http.max_concurrent` are served at once; `/health` and `/openapi.json` are outside both limits.

Config: v_utils `Settings` (`--config`, else `$XDG_CONFIG_HOME/review_archive.{toml,nix,…}`;
each key also a flag, e.g. `--schedule-min-interval`) for data dir, bind address, defaults,
pacing (`[schedule]`, `[worker]`, `[http]`, `[webhooks]`, `[browser]`, `[tokens]` — only `per_hour`); every key has a default,
`review_archive config write-defaults` lists them. Secrets only from env.

### HTTP API for other services

Everything the CLI can do is reachable over HTTP, so other services drive the
archive without shelling into its container. JSON in and out; DTOs live in the
library (below) and are shared with the client crate. OpenAPI document at
`GET /openapi.json` (utoipa, as in concierge).

Targets:

- `POST /targets` `{place | maps_url, label?, lang?, interval?, gbp?}` → `201`
  with the target. Same resolution as `target add`.
- `PATCH /targets/{id}` `{label?, lang?, interval?, enabled?}`
- `DELETE /targets/{id}` — disables; archived data is never deleted.
- `GET /targets/{id}` — target with last run, counts, next scheduled scan.

Jobs (one browser, one queue; on-demand jobs go ahead of scheduled scans):

- `POST /targets/{id}/scan` → `202 {job_id}` — scan now.
- `POST /captures` `{place | maps_url, lang?, max_reviews?, review_ids?}` →
  `202 {job_id}` — ad-hoc capture of a place **without** registering a target;
  results are stored under an implicit, disabled target so nothing is lost.
  `?wait=<secs>` (cap 120) blocks and returns the result directly if done.
  `max_reviews` and the number of `review_ids` are at most `max_reviews_initial`.
- The queue holds `max_queued_jobs` (default 20) at most: past that, `429`. Asking again
  for a job already queued (same target, kind and limits) returns its id. After three
  jobs in a row, an overdue scheduled scan goes first.
- `GET /jobs/{id}` → `queued | running | done | failed`, and on `done` the
  reviews with their capture URLs — for a capture of `review_ids`, those of them it
  found.
- `GET /targets/{id}/runs?limit=`

Reviews and captures:

- `GET /reviews/{id}` — review with versions and captures.
- `GET /targets/{id}/export.zip?since=` — same archive as `export`.

Events:

- `POST /webhooks` `{url, events: [review.new, review.changed, review.gone, review.reappeared, run.failed], secret}`,
    `GET /webhooks`, `DELETE /webhooks/{id}`. Deliveries are signed
  (`X-Signature: sha256=<hmac of body>`), retried with backoff, recorded in an
  outbox table so a restart does not drop them.
- A webhook URL is `http(s)`; with `[webhooks] allowed_hosts` empty it may not name a
  loopback, private, link-local, carrier-grade-NAT or otherwise local address, and a
  host name is refused at delivery when it resolves only to such addresses. A non-empty
  `allowed_hosts` is the whole list of hosts webhooks may go to (a service in the same
  cluster, say). Redirects are not followed; a delivery gets 3 s to connect and 10 s in
  all, and a receiver that cannot be reached is not held against the other hooks.

## Members

Several people track their places here. Who manages which GMB is the conductor's: a person's
gmails (a Google manager account, or an alias for one) and the GMBs (Google places) under each.
The archive keeps a mirror of them and annotates them; it adds and removes none.

- A **member** is a person: a concierge account the Service-Arb panel vouches for, kept in
  `people(id, sub UNIQUE, email, name, first_seen, interval_secs)` and found or made by `sub` on every
  request. Anyone signed in to the panel is one; what they may do beyond their own `/me` is
  their `sa:review_archive:*` permissions, granted in concierge. Rows from before people had
  ids were keyed by email: each address is a person without a `sub`, claimed by the first
  sign-in with it — only when concierge says the address is verified and no other account
  already holds it; otherwise the request is a 403 and an error is logged for an admin.
- `gmb_mirror(id, gmail_id, gmail, owner, removed, enabled, place_id)`: the conductor's GMBs, its ids,
  `owner` a `sub`, `enabled` its switch, which may govern other services too. `PUT /api/conductor/gmbs` (a Conductor assertion, below) upserts every one
  in one transaction and never deletes; a GMB whose gmail, owner or place changes, or one
  missing from the snapshot, is a 409 and nothing is written. A GMB the conductor removes loses
  its tracks, as an untrack does; its appeals stay. GBP reads keep the service's one grant: a
  client adds the service's Google account as a manager of their GBP.
- `tracks(gmb_id, target_id, created_at, enabled, favorite)`. Tracking a GMB finds the target on
  its place, a language and a source, or makes one (enabling a disabled one); a GMB may have
  several. Targets, reviews, captures and blobs stay shared, so two members on one place cost
  one scan and one capture. Scheduling stays per target, its interval its payers' (see
  Scheduling). The operator assigns an existing target with `track <gmb-id> <target-id>`.
- Tracks are switched on or off by their member (`enabled`, on when added;
  `PATCH /me/gmbs/{gmb}/tracks/{target}`). A track is live while it is on and its GMB is, at the
  conductor; a GMB switched off there pays for no walk and asks for none. A target someone tracks
  is scanned while one of its tracks is live; one nobody tracks keeps to its own `enabled`.
  Switching never touches the shared target.
- `reinstatements(gmb_id, review_id, requested_at, withdrawn_at, reinstated_at)`
  record that a member asked Google to reinstate a removed review (the archive asks
  nothing of Google). Withdrawing sets `withdrawn_at`; nothing is deleted. A scan that lists
  the review again sets `reinstated_at` in its transaction, and it returns to the
  Snapshotted column with a "reinstated after N d" badge.
- A member's board of a tracked place (`GET /me/gmbs/{gmb}/tracks/{target}/board`) has four
  columns, each by its own time:
  Snapshotted (listed, by first sighting), Suspended (gone with 5 stars, no open appeal, by
  `gone_at`), Removed (any other gone, no open appeal, by `gone_at`), Reinstating (gone, open
  appeal, by `requested_at`). The board narrows by 5-star and responded, each required or ruled
  out; a card's `non-5-star` and `responded` tags open it with that one ruled out.
- `GET /me/gmbs` answers per GMB not removed `{tracked, rating, reviews}` (`tracked`: a track of it is live) (the place's figures
  from its most recently walked track) and `places_tracked`: what the conductor's `/gmbs` page
  tags a GMB with.

### Tokens

Every Maps walk runs on the operator's signed-in Google account, and each action is
exposure for it. Tokens price that; they are the conductor's, and the members whose places are
scanned pay.

- A walk is metered in `maps::cost`: opening the place 7, the Reviews tab 1, sorting 7
  (each retried click 2), each scroll of the feed 1 — the data requests each sends Google, a
  scroll's ~9 being the unit. "More" and screenshots send none. An unchanged place costs 8;
  a rescan 15 and 1 per screen of cards.
- What a member holds is their balance in the conductor's last snapshot (`mirror_balances`, by
  `sub`; none told is 0) less the equal shares (`ceil(spent / payers)`) of the charges past
  the snapshot's `charges_through` that name them, never below 0. No network is asked: a
  conductor away leaves walks going, and their charges wait in the feed.
- A scheduled scan of a target is paid by the members tracking it (a track live). Its walk may
  spend their holdings together, within what the account has left this hour
  (`tokens.per_hour`); the next scroll past that ends the walk (`WalkEnd::Budget`), recording a
  cut as a limit does — a first scan too — so the next scan goes on from there. A scan a member
  asks for (`POST /me/gmbs/{gmb}/tracks/{target}/scan`) is that member's alone, and refused
  (`429`) while they hold less than a first screen or the GMB is switched off; asked for while the same scan is already
  queued, it is that job's and its first requester pays. Places nobody tracks, the operator's
  jobs and ad-hoc captures are paid by no one and limited by the hour only.
- The run's end transaction writes what it cost to `token_charges(id, run_id, payers, spent,
  kick_back, label, at)` when it had payers and spent anything; `kick_back` when it failed or
  met a failure (`Warning::Failure`; a `Warning::Limit` is none): the conductor owes each payer
  back what they paid. A run a stopped process left open writes none. The hour still counts
  the run. Purge leaves the feed alone (no foreign key to `runs`).
- `GET /api/conductor/charges?after=&limit=` (1–500) is the feed, ascending, `next` the page's
  last id. The conductor splits each charge within its balances, owes the kick-backs, and tells
  the archive the cursor in its next snapshot; a `charges_through` behind the stored one or past
  the feed is a 409.
- A place held for tokens shows "out of tokens" on its card (`held`).

### Auth

Served behind the Service-Arb panel at `sa.evinvest.ltd`: its `/api/review_archive/*` reaches
the API (the prefix stripped), and its `/review_archive/mfe/*` the bundle. Nothing else reaches
the service.

- Every authenticated request carries the panel's assertion in `x-sa-assertion` (`sa_auth`):
  an Ed25519 JWS `{aud: review_archive, sub, email, email_verified, name, permissions, method,
  path, exp}`, verified with the panel's public keys (`PANEL_ASSERTION_KEYS`). It names this
  one request — its method and path — and lives 60 s; anything else is a 401. `permissions`
  is the caller's `sa:review_archive:*` slice.
- `sa:review_archive:archive:operate` opens the archive's own routes: targets, scans,
  captures, jobs, reviews, stats, webhooks, export, and any capture's WebP.
  `sa:review_archive:members:act_as` opens `GET /members`.
- `/api/conductor/*` takes a Conductor assertion only (the panel itself, no user); a User or
  Source assertion is a 401. Not served under `--dev-member`.
- `/me` routes are every signed-in person's own, and `GET /captures/{sha}.webp` of the places
  their GMBs track. With `sa:review_archive:members:act_as`, `X-Member: <person id>` acts as
  that member: their GMBs, boards and settings, their writes; anyone else sending it gets 403.
- CSRF is the panel's: it checks its own header on every write before forwarding.
- `GET /dashboards` is the conductor's (an assertion naming `Principal::Conductor`), and no one
  else's: a JSON array of the dashboards in `grafana/` (written by `examples/grafana_*.py`), each with its
  `uid`, for its Grafana; strong `ETag`, `304` on `If-None-Match`.
- `serve --dev-member <alias | permissions | none>` (loopback, outside production): every
  request is one person holding that, without the panel, for a local dashboard
  (`nix run .#dev-mfe`): the owner of a mirrored GMB, so it shows what the worker scans; with
  several, the one `--dev-as <person id>` names; before anyone has one, a made-up `dev`. Its
  balance is the mirror's: standalone, with no conductor, its places are held.
  `--no-login-check` (outside production; `nix run .#dev-mfe` passes it): no boot check of the
  Google accounts, and every one signed out fails walks without ending `serve`.

### Alerts

The conductor's Alerts own rules, audiences, chats and Telegram delivery; the archive only
publishes. Ops alerts stay on Discord.

- `alert_notes(seq, id, rule, text, photo, gmbs, created_at)`: written in the transaction of
  what they report, one per event on a target a live track names. `rule` is `review_new`,
  `review_changed`, `review_gone`, `review_reappeared` (the `review.*` events) or
  `scan_failed` (`run.failed`); `gmbs` the conductor's ids of the live tracks' GMBs; `id` the
  event's UUID. A target's first scan (`Known::initial`) publishes no `review_new`.
- The note is rendered here (`sa_alerts::Note`): "<what> · <place>", the stars, author and
  text, or the run's error; the headline bold. Reviews are public, so there is no `text_pii`;
  no buttons. A `review_gone` whose review has a capture carries it as `photo`
  (`/api/conductor/captures/<sha>.png`) and a caption cut to 1024 UTF-16 units, any other text
  to 4096, an ellipsis marking the cut.
- `GET /api/conductor/notifications?after=&limit=` (1–500) is the feed, ascending by `seq`,
  `next` the page's last; `GET /api/conductor/captures/{sha}.png` a recorded capture as PNG
  (which `sendPhoto` is documented for). Both take a Conductor assertion only. Notes are kept
  24 h, pruned by `serve` when idle.

### Dashboard

`crates/review_archive_web`: a dioxus microfrontend, `<mfe-review-archive-dashboard>`, its
bundle served by the binary under `/mfe/` (`mfe_dir`), which the panel forwards as
`/review_archive/mfe/`; the panel's page at `/review_archive` mounts it under its top bar
and gives it the API base (`/api/review_archive`), its sign-in and its CSRF cookie. A 401
sends the top window to the sign-in with `return_to` = the page, which comes back signed
in. Design: Figma "review_archive / dashboard", on ev_lib's `uikit`. One holding
`members:act_as` gets tabs: their own dashboard, and one per member picked from
`GET /members` at `/act-as` (the panel's account menu links there), acting as them through
`X-Member`; the open tabs outlive a full load of the page, per browser tab. Where it is is its URL — `/gmbs` (the
overview), `/gmbs/{id}` (a GMB's board, a switch between its tracks, or a language to track it
in), `/gmails/{id}`, `/settings`, under `/members/{id}` for a member's tab. The conductor's sidebar, beside it on
every page, lists the person's gmails and GMBs, switches them and links to those views; a member's tab
draws its own list of that member's gmails. Adding a GMB is the conductor's home. A card under a GMB
switched off at the conductor is dimmed, as one whose track is off. Standalone (`--dev-member`), the binary serves the same page at `/`.
`?grafana=true` on its URL has the host's Grafana (the element's `grafana` attribute: its base) draw
the overview and a place's board, for that browser tab (`?grafana=false` undoes it); otherwise the
legacy UI does. A host without the attribute has no Grafana, and the element says so.

## Library

The crate is usable without the server, inside other systems, at the low level:

- `review_archive_core` — no I/O: domain types, DTOs, reconciliation, the Maps
  card parser and selectors, relative dates. Anyone with their own browser or
  their own HTML can parse and reconcile.
- `review_archive` — the engine: `ReviewSource` trait (public, so other
  platforms plug in), `maps` and `gbp` sources, a `Browser` handle that callers
  can own or share, `Store` (SQLite + blobs), and a facade
  `Archive::open(config)` with `capture_place(..)`, `scan_target(..)`,
    `add_target(..)`, `stats(..)`, `export(..)`. Features: `maps` (the browser, and the
  `maps` and `gbp` sources), `store`.
  A caller can also run `capture_place` with no store at all and get the reviews
  and WebP bytes back in memory.
- `review_archive_server` — binary: CLI, scheduler, HTTP, webhooks. Thin over
  the facade; no logic that the library does not also expose.
- `review_archive_client` — typed async HTTP client over the same DTOs, for
  services that call a running archive instead of embedding it; builds for wasm too.
- `review_archive_web` — the dashboard, over the client.

## Repo conventions

Follow `gmaps_optimal_placement` / `aquafix`: edition 2024, nightly via
`v_flakes` (`github:valeratrades/v_flakes?ref=v1.6`), `eyre` + `color-eyre`, the
same `rustfmt.toml`, `v_flakes.github` generated workflows, container via
`v_flakes.container.implement` (mount `/data`, `healthPath = "/health"`,
`chromium` in the image) and `github.containerRelease = { registry = "ghcr.io/service-arb"; }`.
A `v*` tag is a release — do not tag.

## Tests

- Parser tests on saved Maps HTML fixtures (insta snapshots).
- Repository tests on a temp SQLite: new / changed / gone / reappeared.
- Scheduler: interval, jitter bounds, backoff.
- `gbp` client against a stub HTTP server (pagination, token refresh).
- Live scan of one real place — `#[ignore]`, run by hand.
