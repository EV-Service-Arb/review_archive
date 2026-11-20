//! The Grafana dashboards of `GET /dashboards`, for the conductor's Grafana, built over the
//! document (`sa_core::dashboards`): a panel names its operation, and what it reads is that
//! operation's answer. The page frames them (`review_archive_web/src/grafana.rs`).

use ev_lib::api::{OpenApi, op_id};
use review_archive_core::dashboards::{LOCATION, OVERVIEW, Review, up, var};
use sa_core::dashboards::{Board, Column, Panel, Query, Select};
use serde_json::{Value, json};
use strum::IntoEnumIterator;

/// The datasource the conductor's Grafana names this service by.
const DATASOURCE: &str = "review_archive";

const STATUS: &str = r#"[{"type": "value", "options": {"ok": {"color": "green", "index": 0}, "partial": {"color": "orange", "index": 1}, "failed": {"color": "red", "index": 2}}}]"#;

/// Every card's column, and where on a board's card it is.
const CARD: [(&str, &str, Column); 12] = [
	("id", "review.id", Column::Number),
	("author", "review.author", Column::String),
	("rating", "review.rating", Column::Number),
	("text", "review.text", Column::String),
	("reply", "review.reply", Column::String),
	("published_raw", "review.published_raw", Column::String),
	("first_seen", "review.first_seen", Column::String),
	("gone_at", "review.gone_at", Column::String),
	("capture_url", "review.capture_url", Column::String),
	("corrupted", "review.capture_corrupted", Column::Boolean),
	("requested_at", "reinstatement.requested_at", Column::String),
	("reinstated_at", "reinstatement.reinstated_at", Column::String),
];

const LANES: [(&str, &str); 4] = [("snapshotted", "Snapshotted"), ("suspended", "Suspended"), ("removed", "Removed"), ("reinstating", "Reinstating")];

/// String concatenation, not template literals: nothing in a script may read as a dashboard variable.
const LOCATION_HELPERS: &str = r#"const h = context.handlebars;
context.markdown.inline.ruler.enableOnly(['text', 'html_inline', 'entity']); // the panel reads its HTML as Markdown: an `_` in a name would open an <em>
const ago = (ts) => {
  if (!ts) return '';
  const s = Math.max(0, Math.floor((Date.now() - Date.parse(ts)) / 1000));
  if (s < 90) return 'just now';
  if (s < 90 * 60) return Math.floor(s / 60) + 'm ago';
  if (s < 36 * 3600) return Math.floor(s / 3600) + 'h ago';
  return Math.floor(s / 86400) + 'd ago';
};
h.registerHelper('ago', ago);
h.registerHelper('stars', (n) => '★'.repeat(Math.max(0, Math.min(5, n || 0))));
h.registerHelper('days', (from, to) => Math.floor((Date.parse(to) - Date.parse(from)) / 86400000));
h.registerHelper('capture', (url) => context.grafana.replaceVariables('@API@').replace(/\/$/, '') + url);"#;

const LANE_CONTENT: &str = r#"<div class="ra-lane" data-lane="@LANE@">
{{#each data}}{{#if lane}}
<article class="ra-card" data-i="{{@index}}">
  {{#if capture_url}}<img loading="lazy" src="{{capture capture_url}}">{{/if}}
  <div class="ra-who"><span class="ra-stars">{{stars rating}}</span><span class="ra-author">{{author}}</span></div>
  {{#if text}}<p class="ra-text">{{text}}</p>{{/if}}
  {{#if reply}}<p class="ra-text ra-reply">{{reply}}</p>{{/if}}
  <div class="ra-foot">
    <span>@WHEN@</span>
    <div class="ra-badges">
      @REINSTATED@
      {{#if corrupted}}<span class="warn" title="the screenshot is cut off; the next scan retakes it">corrupted</span>{{/if}}
      {{#if reply}}<span class="ok">responded</span>{{else}}<span class="warn">no reply</span>{{/if}}
    </div>
  </div>
</article>
{{/if}}{{/each}}
</div>"#;

/// A drag started in one lane's panel is seen by the others through the frame's document: a
/// `window` property set by this code is not seen outside it. A card goes from Suspended or Removed
/// to Reinstating (asked of Google), and back to where it sits gone (withdrawn); the page does the write.
const LANE_AFTER_RENDER: &str = r#"const lane = context.element.querySelector('.ra-lane');
// the default content's data is {}: no answer yet
if (Array.isArray(context.data)) {
  const n = context.data.find((row) => row.lane === '');
  const vars = Object.fromEntries(Object.keys(n).filter((k) => k.startsWith('n_')).map((k) => ['var-' + k, String(n[k])]));
  const loc = context.grafana.locationService;
  const shown = loc.getSearchObject();
  if (Object.keys(vars).some((k) => String(shown[k]) !== vars[k])) loc.partial(vars, true);
}
const up = (m) => window.parent.postMessage(m, window.location.origin);
const onto = (d, to) => (d.from === 'suspended' || d.from === 'removed') && to === 'reinstating'
  ? '@REINSTATE@'
  : d.from === 'reinstating' && to === d.gone ? '@WITHDRAW@' : null;
const dragged = () => document.body.dataset.raDrag && JSON.parse(document.body.dataset.raDrag);
const done = () => {
  delete document.body.dataset.raDrag;
  document.querySelectorAll('.ra-accepts').forEach((l) => l.classList.remove('ra-accepts'));
};
// Grafana's sanitizer drops `draggable` from the content
const movable = lane.dataset.lane === 'snapshotted' ? [] : lane.querySelectorAll('.ra-card');
movable.forEach((card) => {
  const row = context.data[Number(card.dataset.i)];
  card.draggable = true;
  card.querySelectorAll('img').forEach((img) => (img.draggable = false));
  card.addEventListener('dragstart', (e) => {
    e.dataTransfer.setData('text/plain', String(row.id)); // Firefox starts no drag without data
    const d = { review: row.id, from: lane.dataset.lane, gone: row.rating === 5 ? 'suspended' : 'removed' };
    document.body.dataset.raDrag = JSON.stringify(d);
    document.querySelectorAll('.ra-lane').forEach((l) => l.classList.toggle('ra-accepts', !!onto(d, l.dataset.lane)));
  });
  card.addEventListener('dragend', done);
});
// a lane in a tab is drawn once shown, which a drag over its tab may do
lane.classList.toggle('ra-accepts', !!(dragged() && onto(dragged(), lane.dataset.lane)));
lane.addEventListener('dragover', (e) => {
  const d = dragged();
  if (d && onto(d, lane.dataset.lane)) e.preventDefault();
});
lane.addEventListener('drop', (e) => {
  e.preventDefault();
  const d = dragged();
  done();
  const kind = d && onto(d, lane.dataset.lane);
  if (kind) up({ kind, review: d.review });
});"#;

const LANE_STYLES: &str = r#".ra-lane { display: flex; flex-direction: column; gap: 12px; min-height: 100%; padding: 12px; box-sizing: border-box; }
.ra-lane.ra-accepts { outline: 2px dashed #5794f2; outline-offset: -2px; background: rgba(87,148,242,.05); }
.ra-card { padding: 12px; border: 1px solid rgba(128,128,128,.3); border-radius: 8px; }
.ra-card[draggable=true] { cursor: grab; }
.ra-card img { display: block; width: 100%; border-radius: 6px; background: rgba(128,128,128,.15); }
.ra-who { display: flex; align-items: center; gap: 8px; margin-top: 8px; min-width: 0; }
.ra-stars { color: #ff9830; }
.ra-author { font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ra-text { margin: 4px 0 0; opacity: .7; display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
.ra-reply { margin-top: 8px; padding-left: 8px; border-left: 2px solid rgba(128,128,128,.3); }
.ra-foot { display: flex; align-items: center; justify-content: space-between; gap: 8px; margin-top: 8px; font-size: 11px; opacity: .8; }
.ra-badges { display: flex; gap: 6px; }
.ra-badges span { padding: 1px 8px; border-radius: 999px; white-space: nowrap; }
.ok { background: rgba(115,191,105,.15); color: #73bf69; }
.warn { background: rgba(255,152,48,.15); color: #ff9830; }"#;

/// Grafana interpolates `${gmail}` and `${keys}` before Infinity evaluates the JSONata; JSONata's
/// own `$names` must not be a dashboard variable's. Infinity's JSONata (jsonata-go) gives null, not
/// undefined, for a key `$lookup` misses, so `$exists` would hold for every row.
const OVERVIEW_ROOT: &str = r#"(
  $tie := "@REV@";
  $order := $merge($append([], $map($split("@KEYS@", ","), function($k, $i) { {$k: $i} })));
  $rows := $$[("@GMAIL@" = "") or ($string(id) = "@GMAIL@")].(
    $g := $;
    gmbs[("@GMBS_CSV@" = "*") or ($string(id) in $split("@GMBS_CSV@", ","))].(
    $b := $;
    track.{
      "key": $string($b.id) & ":" & $string(target.id),
      "gmb": $b.id, "gmail": $g.id, "gmail_name": $g.gmail, "state": $b.state = "verified" ? null : $b.state,
      "target": target.id, "label": $b.name, "address": target.address, "place_id": target.place_id,
      "enabled": enabled, "favorite": favorite, "snapshots_7d": snapshots_7d, "snapshots_30d": snapshots_30d,
      "live": live, "listed": listed, "rating": rating, "new_7d": new_7d, "posts_7d": posts_7d,
      "post": latest_post.text, "post_at": latest_post.published_est ? latest_post.published_est : latest_post.first_seen,
      "responded": responded, "corrupted": corrupted, "captured": captured, "non_five_star": non_five_star, "suspended": suspended, "removed": removed, "reinstating": reinstating, "held": held,
      "last_run_at": last_run_at, "last_run_status": last_run_status, "last_run_error": last_run_error,
      "scan": scan.state, "scan_since": scan.since, "scan_ahead": scan.ahead, "scan_cost": scan_cost, "no_walks": scan_cost = null
    })
  );
  $sort($append([], $rows[$type($lookup($order, key)) = "number"]), function($l, $r) { $lookup($order, $l.key) > $lookup($order, $r.key) })
)"#;

const OVERVIEW_COLUMNS: [(&str, Column); 35] = [
	("gmb", Column::Number),
	("gmail", Column::Number),
	("gmail_name", Column::String),
	("target", Column::Number),
	("label", Column::String),
	("state", Column::String),
	("address", Column::String),
	("place_id", Column::String),
	("enabled", Column::Boolean),
	("favorite", Column::Boolean),
	("snapshots_7d", Column::Number),
	("snapshots_30d", Column::Number),
	("live", Column::Number),
	("listed", Column::Number),
	("rating", Column::Number),
	("new_7d", Column::Number),
	("posts_7d", Column::Number),
	("post", Column::String),
	("post_at", Column::String),
	("responded", Column::Number),
	("corrupted", Column::Number),
	("captured", Column::Number),
	("non_five_star", Column::Number),
	("suspended", Column::Number),
	("removed", Column::Number),
	("reinstating", Column::Number),
	("held", Column::Boolean),
	("last_run_at", Column::String),
	("last_run_status", Column::String),
	("last_run_error", Column::String),
	("scan", Column::String),
	("scan_since", Column::String),
	("scan_ahead", Column::Number),
	("scan_cost", Column::Number),
	("no_walks", Column::Boolean),
];

/// The cards' columns the table shows, under its names, in its order.
const TABLE: [(&str, &str); 15] = [
	("label", "location"),
	("enabled", "on"),
	("snapshots_7d", "snapshots 7d"),
	("snapshots_30d", "snapshots 30d"),
	("live", "live"),
	("listed", "listed"),
	("rating", "★ average"),
	("new_7d", "new 7d"),
	("posts_7d", "posts 7d"),
	("responded", "responded"),
	("suspended", "suspended"),
	("removed", "removed"),
	("reinstating", "reinstating"),
	("last_run_at", "last run"),
	("last_run_status", "status"),
];

const OVERVIEW_CONTENT: &str = r#"<div class="ra-grid">
{{#each data}}
<div class="ra-card{{#unless (on this)}} ra-off{{/unless}}{{#if favorite}} ra-fav{{/if}}" data-i="{{@index}}">
  <div class="ra-head">
    <div class="ra-name"><div class="ra-label">{{label}}</div></div>
    {{#if address}}<div class="ra-soft ra-cut" title="{{address}}">{{address}}</div>{{/if}}
    <div class="ra-pid">{{place_id}}</div>
    {{#if (allGmails)}}<div class="ra-soft ra-cut ra-small">{{gmail_name}}</div>{{/if}}
  </div>
  <div class="ra-stats">
    <div><b>{{snapshots_7d}}</b><small>snapshots 7d</small></div>
    <div class="ra-soft"><b class="ra-sub">{{snapshots_30d}}</b><small>snapshots 30d</small></div>
    {{#if rating}}<div class="ra-avg" title="the star average Google gives the place"><b>{{fixed rating 2}}</b><small>★ average</small></div>{{/if}}
  </div>
  <div class="ra-badges">
    {{#if state}}<span class="warn" title="not scanned until the GBP is verified">{{state}}</span>{{/if}}
    <span class="ok" title="archived and still listed / listed on Google">live: {{live}}{{#if listed}}/{{listed}}{{/if}}</span>
    <span class="ok" title="posted over the last 7 days, removed ones included">new 7d: +{{new_7d}}</span>
    {{#if post}}<span class="ok" title="owner's latest post, {{ago post_at}}:&#10;{{post}}">posts 7d: {{posts_7d}}</span>{{/if}}
    <span class="{{#if (lt responded live)}}warn{{else}}ok{{/if}}" title="replied to by the owner / live">responded: {{responded}}/{{live}}</span>
    <span class="soft" title="listed reviews under 5 stars, or unrated">non-5-star: {{non_five_star}}</span>
    {{#if corrupted}}<span class="warn" title="reviews whose every screenshot is cut off / reviews with one; the next scan retakes them">corrupted: {{corrupted}}/{{captured}}</span>{{/if}}
    {{#if suspended}}<span class="bad" title="5-star reviews gone from the listing">suspended: {{suspended}}</span>{{/if}}
    {{#if removed}}<span class="bad">removed: {{removed}}</span>{{/if}}
    {{#if reinstating}}<span class="warn">reinstating: {{reinstating}}</span>{{/if}}
    {{#if held}}<span class="warn" title="not scanned: every member tracking it is out of tokens">out of tokens</span>{{/if}}
    {{#if no_walks}}<span class="warn" title="not scanned: walks are not priced, or cost more than the hour allows; ↻ says which">no walks</span>{{/if}}
  </div>
  <div class="ra-run">
    <span class="ra-line" title="{{last_run_error}}"><i class="ra-dot ra-dot-{{dot this}}"></i>{{scanline this}}{{lastline this}}{{#if last_run_status}}{{last_run_status}}{{else}}never scanned{{/if}}</span>
    {{#if scan}}<span class="ra-btn"><i class="ra-spin"></i></span>{{else}}<button class="ra-btn" data-act="scan" title="scan now, on top of the schedule{{#unless no_walks}}: ≈ {{scan_cost}} tokens{{/unless}}">↻</button>{{/if}}
    <button class="ra-btn" data-act="enable" title="scanned on the schedule"><i class="ra-switch{{#if enabled}} on{{/if}}"></i></button>
    <details class="ra-menu">
      <summary class="ra-btn" title="more">⋮</summary>
      <div>
        <button data-act="favorite">{{#if favorite}}<i class="ra-gold">★</i> unfavorite{{else}}☆ favorite{{/if}}</button>
        <button data-act="relink" title="ask Google for the place's id, name and address again">⟳ re-link</button>
      </div>
    </details>
  </div>
</div>
{{/each}}
</div>"#;

/// String concatenation, not template literals: nothing in a script may read as a dashboard variable.
const OVERVIEW_HELPERS: &str = r#"const h = context.handlebars;
context.markdown.inline.ruler.enableOnly(['text', 'html_inline', 'entity']); // the panel reads its HTML as Markdown: an `_` in a name would open an <em>
const ago = (ts) => {
  if (!ts) return '';
  const s = Math.max(0, Math.floor((Date.now() - Date.parse(ts)) / 1000));
  if (s < 90) return 'just now';
  if (s < 90 * 60) return Math.floor(s / 60) + 'm ago';
  if (s < 36 * 3600) return Math.floor(s / 3600) + 'h ago';
  return Math.floor(s / 86400) + 'd ago';
};
h.registerHelper('ago', ago);
h.registerHelper('lt', (a, b) => a < b);
h.registerHelper('fixed', (n, digits) => n.toFixed(digits));
h.registerHelper('allGmails', () => context.grafana.replaceVariables('@GMAIL@') === '');
h.registerHelper('on', (r) => r.enabled);
h.registerHelper('dot', (r) => r.scan === 'queued' ? 'warn' : r.scan === 'running' ? 'run' : (r.last_run_status || 'never'));
h.registerHelper('scanline', (r) => r.scan === 'queued'
  ? 'queued · ' + (r.scan_ahead ? r.scan_ahead + ' ahead' : 'next') + ' · since ' + ago(r.scan_since) + ' · '
  : r.scan === 'running' ? 'scanning · started ' + ago(r.scan_since) + ' · ' : '');
h.registerHelper('lastline', (r) => r.last_run_at ? 'last run ' + ago(r.last_run_at) + ' · ' : '');"#;

/// Clicks go up to the page; the filter's result comes down as the frame's URL, steered without
/// reloading it.
const OVERVIEW_AFTER_RENDER: &str = r#"const up = (m) => window.parent.postMessage(m, window.location.origin);
context.element.querySelectorAll('.ra-card').forEach((card) => {
  const row = context.data[Number(card.dataset.i)];
  const ids = { gmb: row.gmb, target: row.target };
  const acts = {
    scan: { kind: '@SCAN@', ...ids },
    enable: { kind: '@ENABLE@', ...ids, on: !row.enabled },
    favorite: { kind: '@FAVORITE@', ...ids, on: !row.favorite },
    relink: { kind: '@RELINK@', ...ids },
  };
  card.addEventListener('click', () => up({ kind: '@OPEN@', ...ids }));
  card.querySelector('.ra-menu').addEventListener('click', (e) => e.stopPropagation());
  card.querySelectorAll('[data-act]').forEach((b) => b.addEventListener('click', (e) => {
    e.stopPropagation();
    up(acts[b.dataset.act]);
  }));
  const name = card.querySelector('.ra-label');
  if (name.scrollHeight > name.clientHeight) name.title = name.textContent;
});"#;

const OVERVIEW_STYLES: &str = r#".ra-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 16px; }
.ra-card { position: relative; display: flex; flex-direction: column; gap: 12px; padding: 8px 16px 16px; border: 1px solid rgba(128,128,128,.3); border-radius: 8px; cursor: pointer; }
.ra-card:hover { border-color: rgba(128,128,128,.7); }
.ra-fav, .ra-fav:hover { border-color: #ff9830; }
.ra-off { opacity: .5; }
.ra-head { min-width: 0; }
.ra-name { min-height: 28px; display: flex; flex-direction: column; justify-content: flex-end; }
.ra-label { font-weight: 600; font-size: 14px; line-height: 20px; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
.ra-soft { opacity: .65; }
.ra-cut { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ra-small { font-size: 11px; }
.ra-pid { font-family: monospace; font-size: 11px; opacity: .65; overflow: hidden; text-overflow: ellipsis; }
.ra-stats { display: flex; align-items: flex-end; gap: 24px; }
.ra-stats div { display: flex; flex-direction: column; gap: 4px; }
.ra-stats b { font-size: 28px; font-weight: 600; line-height: 1; }
.ra-stats b.ra-sub { font-size: 18px; }
.ra-stats small { font-size: 11px; opacity: .65; }
.ra-stats .ra-soft small { opacity: 1; }
.ra-stats .ra-avg { margin-left: auto; align-items: flex-end; }
.ra-badges { display: flex; flex-wrap: wrap; gap: 6px; }
.ra-badges span { font-size: 11px; padding: 1px 8px; border-radius: 999px; }
.ok { background: rgba(115,191,105,.15); color: #73bf69; }
.warn { background: rgba(255,152,48,.15); color: #ff9830; }
.bad { background: rgba(242,73,92,.15); color: #f2495c; }
.soft { background: rgba(107,127,160,.15); color: #6b7fa0; }
.ra-run { margin-top: auto; display: flex; align-items: center; gap: 4px; padding-top: 12px; border-top: 1px solid rgba(128,128,128,.3); font-size: 11px; }
.ra-line { flex: 1; min-width: 0; opacity: .8; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.ra-dot { display: inline-block; width: 6px; height: 6px; margin-right: 8px; vertical-align: middle; border-radius: 999px; background: #8e8e8e; }
.ra-dot-ok { background: #73bf69; } .ra-dot-partial, .ra-dot-warn { background: #ff9830; } .ra-dot-failed { background: #f2495c; }
.ra-dot-run { background: #73bf69; animation: ra-pulse 1.5s ease-in-out infinite; }
@keyframes ra-pulse { 50% { opacity: .3; } }
.ra-btn { display: inline-flex; align-items: center; justify-content: center; min-width: 32px; height: 32px; padding: 0 6px; border: 0; border-radius: 6px; background: transparent; color: inherit; font-size: 16px; cursor: pointer; list-style: none; }
.ra-btn::-webkit-details-marker { display: none; }
.ra-btn:hover:not(:disabled) { background: rgba(128,128,128,.2); }
.ra-btn:disabled { cursor: not-allowed; }
.ra-spin { width: 12px; height: 12px; border: 2px solid rgba(128,128,128,.4); border-top-color: currentColor; border-radius: 999px; animation: ra-turn 1s linear infinite; }
@keyframes ra-turn { to { transform: rotate(360deg); } }
.ra-switch { position: relative; display: block; width: 32px; height: 18px; border-radius: 999px; background: rgba(128,128,128,.4); }
.ra-switch::after { content: ''; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 999px; background: #fff; transition: left .15s; }
.ra-switch.on { background: #73bf69; }
.ra-switch.on::after { left: 16px; }
.ra-btn:disabled .ra-switch { opacity: .5; }
.ra-menu { position: relative; }
.ra-menu > div { position: absolute; right: 0; bottom: 100%; z-index: 10; display: flex; flex-direction: column; min-width: 140px; padding: 4px; border: 1px solid rgba(128,128,128,.3); border-radius: 6px; background: #181b1f; }
.ra-menu > div button { display: flex; gap: 8px; padding: 6px 8px; border: 0; border-radius: 4px; background: transparent; color: inherit; font-size: 12px; text-align: left; cursor: pointer; }
.ra-menu > div button:hover { background: rgba(128,128,128,.2); }
.ra-gold { color: #ff9830; font-style: normal; }"#;

pub(super) fn all(doc: &OpenApi) -> Vec<Value> {
	let boards = vec![location(doc), overview(doc)];
	for b in &boards {
		let vars: Vec<&str> = b["templating"]["list"]
			.as_array()
			.expect("every board declares its variables")
			.iter()
			.map(|v| v["name"].as_str().expect("a variable is named"))
			.collect();
		let text = b.to_string();
		for (i, _) in text.match_indices('$') {
			let name: String = text[i + 1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
			assert!(
				!vars.contains(&name.as_str()),
				"`${name}` in {} is its dashboard variable to Grafana, which interpolates it",
				b["uid"]
			);
		}
	}
	boards
}

/// `${name}`: a variable as Grafana interpolates it.
fn v(name: &str) -> String {
	format!("${{{name}}}")
}

fn hidden(name: &str) -> Value {
	json!({ "name": name, "type": "textbox", "hide": 2, "query": "", "current": { "text": "", "value": "" }, "options": [] })
}

/// What a card holds to pass `r`, as the page's own filter judges it (`filter/reviews.rs`).
/// Infinity's JSONata gives null for an absent field.
fn narrow(r: Review) -> &'static str {
	match r {
		Review::FiveStar => "review.rating = 5",
		Review::Responded => r#"$type(review.reply) = "string""#,
		Review::New7d => r#"$type(review.published_est) = "string" and $toMillis(review.published_est) >= $millis() - 7 * 86400000"#,
	}
}

/// The lane's cards (`lane` set) after one row of every lane's count, which titles the lanes
/// (`n_<lane>`): a lane in a tab not shown queries nothing, and a variable's own query would fetch
/// the board again. `rev` in a string only ties the panel to it, so a write re-reads it.
fn lane_root(lane: &str) -> String {
	let kept = Review::iter()
		.map(|r| {
			let want = v(&format!("{r:?}"));
			format!(r#"("{want}" = "" or ({}) = ("{want}" = "true"))"#, narrow(r))
		})
		.collect::<Vec<_>>()
		.join(" and ");
	let fields: String = CARD.iter().map(|(name, path, _)| format!(r#""{name}": {path}, "#)).collect();
	let counts = LANES.iter().map(|(l, _)| format!(r#""n_{l}": $count($$.{l}[{kept}])"#)).collect::<Vec<_>>().join(", ");
	format!(
		r#"($tie := "{}"; $append([{{"lane": "", {counts}}}], $$.{lane}[{kept}].{{{fields}"lane": "{lane}"}}))"#,
		v(var::REV)
	)
}

/// A place's board: its lanes of reviews, tiled by the tilers plugin, and its runs.
fn location(doc: &OpenApi) -> Value {
	let (gmb, target) = (v(var::GMB), v(var::TARGET));
	let track = [("gmb", gmb.as_str()), ("target", target.as_str())];
	let helpers = LOCATION_HELPERS.replace("@API@", &v(var::API));
	let after_render = LANE_AFTER_RENDER.replace("@REINSTATE@", up::REINSTATE).replace("@WITHDRAW@", up::WITHDRAW);
	let lane_columns: Vec<(String, Column)> = CARD
		.iter()
		.map(|(name, _, kind)| (name.to_string(), *kind))
		.chain([("lane".to_owned(), Column::String)])
		.chain(LANES.iter().map(|(l, _)| (format!("n_{l}"), Column::Number)))
		.collect();
	let lane_columns: Vec<(&str, Column)> = lane_columns.iter().map(|(n, k)| (n.as_str(), *k)).collect();
	let mut board = Board::new(doc, DATASOURCE);
	for (i, (lane, title)) in LANES.into_iter().enumerate() {
		let when = match lane {
			"snapshotted" => "{{#if published_raw}}{{published_raw}}{{else}}seen {{ago first_seen}}{{/if}}",
			"reinstating" => "requested {{ago requested_at}}",
			_ => "gone {{ago gone_at}}",
		};
		let reinstated = match lane {
			"snapshotted" => r#"{{#if reinstated_at}}<span class="ok">reinstated after {{days requested_at reinstated_at}}d</span>{{/if}}"#,
			_ => "",
		};
		let root = lane_root(lane);
		let query = Query {
			operation: op_id(super::track_board),
			path: &track,
			params: &[],
			select: Some(Select {
				root_selector: &root,
				columns: &lane_columns,
			}),
		};
		let i = u32::try_from(i).expect("four lanes");
		board = board.panel(
			Panel {
				id: 10 + i,
				kind: "marcusolsson-dynamictext-panel",
				title: &format!("{title} · ${{n_{lane}}}"),
				// the gone lanes share one gridPos: the plugin opens them as tabs of one tile
				grid: [8 * i.min(1), 0, 8, 22],
			},
			query,
			json!({
				"dock": { "minWRem": 15.75, "minHRem": 8.5 },
				"transparent": true,
				"options": {
					"renderMode": "allRows",
					"wrap": false,
					"editors": ["helpers", "afterRender", "styles", "default"],
					"contentPartials": [],
					"helpers": helpers,
					"defaultContent": format!(r#"<div class="ra-lane" data-lane="{lane}"></div>"#),
					"content": LANE_CONTENT.replace("@LANE@", lane).replace("@WHEN@", when).replace("@REINSTATED@", reinstated),
					"afterRender": after_render,
					"styles": LANE_STYLES,
				},
			}),
		);
	}
	let runs_root = format!(r#"($tie := "{}"; $)"#, v(var::REV));
	let runs = |params: &'static [(&'static str, &'static str)], columns: &'static [(&'static str, Column)]| Query {
		operation: op_id(super::track_runs),
		path: &track,
		params,
		select: Some(Select { root_selector: &runs_root, columns }),
	};
	let status: Value = serde_json::from_str(STATUS).expect("a JSON literal");
	let by_start = json!([{ "id": "sortBy", "options": { "sort": [{ "field": "started_at" }] } }]);
	let color = |name: &str, color: &str| json!({ "matcher": { "id": "byName", "options": name }, "properties": [{ "id": "color", "value": { "mode": "fixed", "fixedColor": color } }] });
	board
		.panel(
			Panel {
				id: 1,
				kind: "state-timeline",
				title: "Runs",
				grid: [16, 0, 8, 6],
			},
			runs(&[("limit", "500")], &[("started_at", Column::Timestamp), ("status", Column::String)]),
			json!({
				"dock": { "minHRem": 3.5 },
				"transparent": true,
				"transformations": by_start,
				"fieldConfig": { "defaults": { "color": { "mode": "thresholds" }, "mappings": status }, "overrides": [] },
				"options": { "showValue": "never", "mergeValues": false, "rowHeight": 0.8, "legend": { "showLegend": false } },
			}),
		)
		.panel(
			Panel {
				id: 2,
				kind: "timeseries",
				title: "Reviews per run",
				grid: [16, 11, 8, 11],
			},
			runs(
				&[("limit", "500")],
				&[
					("started_at", Column::Timestamp),
					("new", Column::Number),
					("gone", Column::Number),
					("changed", Column::Number),
					("seen", Column::Number),
				],
			),
			json!({
				"dock": { "minHRem": 10 },
				"transparent": true,
				"transformations": by_start,
				"fieldConfig": {
					"defaults": { "custom": { "drawStyle": "bars", "fillOpacity": 60, "lineWidth": 1 }, "decimals": 0 },
					"overrides": [
						{
							"matcher": { "id": "byName", "options": "seen" },
							"properties": [
								{ "id": "custom.drawStyle", "value": "line" },
								{ "id": "custom.fillOpacity", "value": 0 },
								{ "id": "custom.axisPlacement", "value": "right" },
							],
						},
						color("new", "green"),
						color("gone", "red"),
					],
				},
				"options": { "legend": { "showLegend": true, "displayMode": "list", "placement": "bottom" }, "tooltip": { "mode": "multi" } },
			}),
		)
		.panel(
			Panel {
				id: 3,
				kind: "stat",
				title: "Last run",
				grid: [16, 6, 8, 5],
			},
			runs(&[("limit", "1")], &[("status", Column::String), ("seen", Column::Number), ("tokens", Column::Number)]),
			json!({
				"dock": { "minHRem": 6.5 },
				"transparent": true,
				"fieldConfig": { "defaults": { "color": { "mode": "fixed", "fixedColor": "text" }, "mappings": status }, "overrides": [] },
				"options": {
					"reduceOptions": { "calcs": ["lastNotNull"], "fields": "/.*/", "values": false },
					"colorMode": "value",
					"graphMode": "none",
					"textMode": "value_and_name",
					"orientation": "vertical",
				},
			}),
		)
		.build(json!({
			"uid": LOCATION,
			"title": "review_archive · location",
			"tags": ["review_archive"],
			"schemaVersion": 41,
			"editable": true,
			"time": { "from": "now-30d", "to": "now" },
			"refresh": "1m",
			"templating": {
				"list": ([var::GMB, var::TARGET, var::REV, var::API])
					.into_iter()
					.map(hidden)
					.chain(Review::iter().map(|r| hidden(&format!("{r:?}"))))
					.chain(LANES.iter().map(|(l, _)| hidden(&format!("n_{l}"))))
					.collect::<Vec<_>>(),
			},
		}))
}

/// The member's places as cards, in the order the page's filter keeps them, and as a table.
fn overview(doc: &OpenApi) -> Value {
	let root = OVERVIEW_ROOT
		.replace("@REV@", &v(var::REV))
		.replace("@KEYS@", &v(var::KEYS))
		.replace("@GMAIL@", &v(var::GMAIL))
		.replace("@GMBS_CSV@", &format!("${{{}:csv}}", var::GMBS));
	let after_render = [
		("@SCAN@", up::SCAN),
		("@ENABLE@", up::ENABLE),
		("@FAVORITE@", up::FAVORITE),
		("@RELINK@", up::RELINK),
		("@OPEN@", up::OPEN),
	]
	.into_iter()
	.fold(OVERVIEW_AFTER_RENDER.to_owned(), |js, (at, kind)| js.replace(at, kind));
	let status: Value = serde_json::from_str(STATUS).expect("a JSON literal");
	Board::new(doc, DATASOURCE)
		// the element steers it to the conductor's GMB selection, and a pick here becomes that selection
		.variable(
			var::GMBS,
			Query {
				operation: op_id(super::my_overview),
				path: &[],
				params: &[],
				select: Some(Select {
					root_selector: r#"$$.gmbs.{"__value": $string(id), "__text": name}"#,
					columns: &[("__text", Column::String), ("__value", Column::String)],
				}),
			},
			json!({
				"label": "GMBs",
				"refresh": 1,
				"multi": true,
				"includeAll": true,
				"allValue": "*",
				"allowCustomValue": true,
				"current": { "text": ["All"], "value": ["$__all"] },
				"options": [],
			}),
		)
		.panel(
			Panel {
				id: 2,
				kind: "marcusolsson-dynamictext-panel",
				title: "Locations",
				grid: [0, 0, 24, 22],
			},
			Query {
				operation: op_id(super::my_overview),
				path: &[],
				params: &[],
				select: Some(Select {
					root_selector: &root,
					columns: &OVERVIEW_COLUMNS,
				}),
			},
			json!({
				"options": {
					"renderMode": "allRows",
					"wrap": false,
					"editors": ["helpers", "afterRender", "styles"],
					"contentPartials": [],
					"helpers": OVERVIEW_HELPERS.replace("@GMAIL@", &v(var::GMAIL)),
					"defaultContent": "No place to show.",
					"content": OVERVIEW_CONTENT,
					"afterRender": after_render,
					"styles": OVERVIEW_STYLES,
				},
			}),
		)
		.of_panel(
			Panel {
				id: 3,
				kind: "table",
				title: "All locations",
				grid: [0, 22, 24, 10],
			},
			2,
			json!({
				"transformations": [
					{
						"id": "organize",
						"options": {
							"excludeByName": OVERVIEW_COLUMNS.iter().filter(|(c, _)| !TABLE.iter().any(|(t, _)| t == c)).map(|(c, _)| (c.to_string(), json!(true))).collect::<serde_json::Map<_, _>>(),
							"renameByName": TABLE.iter().map(|(c, shown)| (c.to_string(), json!(shown))).collect::<serde_json::Map<_, _>>(),
							"indexByName": TABLE.iter().enumerate().map(|(i, (c, _))| (c.to_string(), json!(i))).collect::<serde_json::Map<_, _>>(),
						},
					},
					{ "id": "convertFieldType", "options": { "conversions": [{ "targetField": "last run", "destinationType": "time" }] } },
				],
				"fieldConfig": {
					"defaults": { "custom": { "align": "auto", "filterable": true } },
					"overrides": [
						{
							"matcher": { "id": "byName", "options": "status" },
							"properties": [{ "id": "custom.cellOptions", "value": { "type": "color-text" } }, { "id": "mappings", "value": status }],
						}
					],
				},
				"options": { "showHeader": true },
			}),
		)
		.build(json!({
			"uid": OVERVIEW,
			"title": "review_archive · overview",
			"tags": ["review_archive"],
			"schemaVersion": 41,
			"editable": true,
			"time": { "from": "now-30d", "to": "now" },
			"templating": { "list": ([var::GMAIL, var::KEYS, var::REV]).into_iter().map(hidden).collect::<Vec<_>>() },
		}))
}
