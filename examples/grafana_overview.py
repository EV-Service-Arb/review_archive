"""Writes grafana/dashboards/ra-overview.json, which is edited here: `python3 examples/grafana_overview.py` from the repo root."""

import json

DS = {"type": "yesoreyeram-infinity-datasource", "uid": "ra-api"}

# Grafana interpolates ${gmail} and ${keys} before Infinity evaluates the JSONata; JSONata's own
# $names must not be a dashboard variable's. Infinity's JSONata (jsonata-go) gives null, not
# undefined, for a key `$lookup` misses, so `$exists` would hold for every row.
ROOT = """(
  $order := $merge($append([], $map($split("${keys}", ","), function($k, $i) { {$k: $i} })));
  $rows := $$[("${gmail}" = "") or ($string(id) = "${gmail}")].(
    $g := $;
    locations.{
      "key": $string($g.id) & ":" & $string(target.id),
      "gmail": $g.id, "gmail_name": $g.gmail, "gmail_on": $g.enabled,
      "target": target.id, "label": target.label, "address": target.address, "place_id": target.place_id,
      "enabled": enabled, "snapshots_7d": snapshots_7d, "snapshots_30d": snapshots_30d,
      "live": live, "listed": listed, "new_7d": new_7d, "posts_7d": posts_7d,
      "post": latest_post.text, "post_at": latest_post.published_est ? latest_post.published_est : latest_post.first_seen,
      "responded": responded, "non_five_star": non_five_star, "suspended": suspended, "removed": removed, "reinstating": reinstating, "held": held,
      "last_run_at": last_run_at, "last_run_status": last_run_status, "last_run_error": last_run_error,
      "scan": scan.state, "scan_since": scan.since, "scan_ahead": scan.ahead, "scan_cost": scan_cost
    }
  );
  $sort($append([], $rows[$type($lookup($order, key)) = "number"]), function($l, $r) { $lookup($order, $l.key) > $lookup($order, $r.key) })
)"""


def query(columns):
	return {
		"refId": "A",
		"datasource": DS,
		"type": "json",
		"source": "url",
		"format": "table",
		"parser": "backend",
		"url": "/me/overview",
		"url_options": {
			"method": "GET",
			"params": [{"key": "rev", "value": "${rev}"}],
			"headers": [{"key": "x-member", "value": "${member}"}],
		},
		"root_selector": ROOT,
		"columns": [{"selector": s, "text": t, "type": ty} for s, t, ty in columns],
	}


def hidden(name):
	return {"name": name, "type": "textbox", "hide": 2, "query": "", "current": {"text": "", "value": ""}, "options": []}


CARD_COLUMNS = [
	("gmail", "gmail", "number"),
	("gmail_name", "gmail_name", "string"),
	("gmail_on", "gmail_on", "boolean"),
	("target", "target", "number"),
	("label", "label", "string"),
	("address", "address", "string"),
	("place_id", "place_id", "string"),
	("enabled", "enabled", "boolean"),
	("snapshots_7d", "snapshots_7d", "number"),
	("snapshots_30d", "snapshots_30d", "number"),
	("live", "live", "number"),
	("listed", "listed", "number"),
	("new_7d", "new_7d", "number"),
	("posts_7d", "posts_7d", "number"),
	("post", "post", "string"),
	("post_at", "post_at", "string"),
	("responded", "responded", "number"),
	("non_five_star", "non_five_star", "number"),
	("suspended", "suspended", "number"),
	("removed", "removed", "number"),
	("reinstating", "reinstating", "number"),
	("held", "held", "boolean"),
	("last_run_at", "last_run_at", "string"),
	("last_run_status", "last_run_status", "string"),
	("last_run_error", "last_run_error", "string"),
	("scan", "scan", "string"),
	("scan_since", "scan_since", "string"),
	("scan_ahead", "scan_ahead", "number"),
	("scan_cost", "scan_cost", "number"),
]

CONTENT = """<div class="ra-grid">
{{#each data}}
<div class="ra-card{{#unless (on this)}} ra-off{{/unless}}" data-i="{{@index}}">
  <div class="ra-head">
    <div class="ra-label">{{label}}</div>
    {{#if address}}<div class="ra-soft ra-cut" title="{{address}}">{{address}}</div>{{/if}}
    <div class="ra-pid">{{place_id}}</div>
    {{#if (allGmails)}}<div class="ra-soft ra-cut ra-small">{{gmail_name}}</div>{{/if}}
  </div>
  <button class="ra-switch{{#if enabled}} on{{/if}}" data-act="enable" title="scanned on the schedule" {{#unless gmail_on}}disabled{{/unless}}></button>
  <div class="ra-big"><b>{{snapshots_7d}}</b> snapshots 7d <span>{{snapshots_30d}}</span> 30d</div>
  <div class="ra-badges">
    <span class="ok" title="archived and still listed / listed on Google">live {{live}}{{#if listed}}/{{listed}}{{/if}}</span>
    <span class="ok" title="posted over the last 7 days, removed ones included">new 7d +{{new_7d}}</span>
    {{#if post}}<span class="ok" title="owner's latest post, {{ago post_at}}:&#10;{{post}}">posts 7d {{posts_7d}}</span>{{/if}}
    <span class="{{#if (lt responded live)}}warn{{else}}ok{{/if}}" title="replied to by the owner / live">responded {{responded}}/{{live}}</span>
    <span class="soft" title="listed reviews under 5 stars, or unrated">non-5-star {{non_five_star}}</span>
    {{#if suspended}}<span class="bad" title="5-star reviews gone from the listing">suspended {{suspended}}</span>{{/if}}
    {{#if removed}}<span class="bad">removed {{removed}}</span>{{/if}}
    {{#if reinstating}}<span class="warn">reinstating {{reinstating}}</span>{{/if}}
    {{#if held}}<span class="warn" title="not scanned: every member tracking it is out of tokens">out of tokens</span>{{/if}}
  </div>
  <div class="ra-run">
    <i class="ra-dot ra-dot-{{dot this}}"></i>{{scanline this}}{{lastline this}}<span title="{{last_run_error}}">{{#if last_run_status}}{{last_run_status}}{{else}}never scanned{{/if}}</span>
    {{#if scan}}<span class="ra-spin"></span>{{else}}<button class="ra-scan" data-act="scan" title="scan now, on top of the schedule: {{scan_cost}} tokens">↻ {{scan_cost}}</button>{{/if}}
  </div>
</div>
{{/each}}
</div>"""

# String concatenation, not template literals: nothing here may read as a dashboard variable.
HELPERS = """const h = context.handlebars;
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
h.registerHelper('allGmails', () => context.grafana.replaceVariables('${gmail}') === '');
h.registerHelper('on', (r) => r.enabled && r.gmail_on);
h.registerHelper('dot', (r) => r.scan === 'queued' ? 'warn' : r.scan === 'running' ? 'run' : (r.last_run_status || 'never'));
h.registerHelper('scanline', (r) => r.scan === 'queued'
  ? 'queued · ' + (r.scan_ahead ? r.scan_ahead + ' ahead' : 'next') + ' · since ' + ago(r.scan_since) + ' · '
  : r.scan === 'running' ? 'scanning · started ' + ago(r.scan_since) + ' · ' : '');
h.registerHelper('lastline', (r) => r.last_run_at ? 'last run ' + ago(r.last_run_at) + ' · ' : '');"""

# The page frames this dashboard (crates/review_archive_web/src/grafana.rs): clicks go up to it,
# the filter's result comes down as the frame's URL, steered without reloading it.
AFTER_RENDER = """const up = (m) => window.parent.postMessage(m, window.location.origin);
context.element.querySelectorAll('.ra-card').forEach((card) => {
  const row = context.data[Number(card.dataset.i)];
  const ids = { gmail: row.gmail, target: row.target };
  card.addEventListener('click', () => up({ kind: 'ra-open', ...ids }));
  card.querySelectorAll('[data-act]').forEach((b) => b.addEventListener('click', (e) => {
    e.stopPropagation();
    up(b.dataset.act === 'scan' ? { kind: 'ra-scan', ...ids } : { kind: 'ra-enable', ...ids, on: !row.enabled });
  }));
});"""

STYLES = """.ra-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 16px; }
.ra-card { position: relative; display: flex; flex-direction: column; gap: 12px; padding: 16px; border: 1px solid rgba(128,128,128,.3); border-radius: 8px; cursor: pointer; }
.ra-card:hover { border-color: rgba(128,128,128,.7); }
.ra-off { opacity: .5; }
.ra-head { padding-right: 48px; min-width: 0; }
.ra-label { font-weight: 600; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ra-soft { opacity: .65; }
.ra-cut { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ra-small { font-size: 11px; }
.ra-pid { font-family: monospace; font-size: 11px; opacity: .65; overflow: hidden; text-overflow: ellipsis; }
.ra-big b { font-size: 28px; line-height: 1; }
.ra-big span { font-size: 15px; opacity: .65; margin-left: 12px; }
.ra-badges { display: flex; flex-wrap: wrap; gap: 6px; }
.ra-badges span { font-size: 11px; padding: 1px 8px; border-radius: 999px; }
.ok { background: rgba(115,191,105,.15); color: #73bf69; }
.warn { background: rgba(255,152,48,.15); color: #ff9830; }
.bad { background: rgba(242,73,92,.15); color: #f2495c; }
.soft { background: rgba(107,127,160,.15); color: #6b7fa0; }
.ra-run { margin-top: auto; display: flex; align-items: center; gap: 8px; padding-top: 12px; border-top: 1px solid rgba(128,128,128,.3); font-size: 11px; opacity: .8; }
.ra-dot { width: 6px; height: 6px; border-radius: 999px; background: #8e8e8e; flex-shrink: 0; }
.ra-dot-ok { background: #73bf69; } .ra-dot-partial, .ra-dot-warn { background: #ff9830; } .ra-dot-failed { background: #f2495c; }
.ra-dot-run { background: #73bf69; animation: ra-pulse 1.5s ease-in-out infinite; }
@keyframes ra-pulse { 50% { opacity: .3; } }
.ra-scan { margin-left: auto; padding: 2px 8px; border: 0; border-radius: 6px; background: transparent; color: inherit; cursor: pointer; }
.ra-scan:hover { background: rgba(128,128,128,.2); }
.ra-spin { margin-left: auto; width: 12px; height: 12px; border: 2px solid rgba(128,128,128,.4); border-top-color: currentColor; border-radius: 999px; animation: ra-turn 1s linear infinite; }
@keyframes ra-turn { to { transform: rotate(360deg); } }
.ra-switch { position: absolute; top: 16px; right: 16px; width: 32px; height: 18px; border: 0; border-radius: 999px; background: rgba(128,128,128,.4); cursor: pointer; }
.ra-switch::after { content: ''; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 999px; background: #fff; transition: left .15s; }
.ra-switch.on { background: #73bf69; }
.ra-switch.on::after { left: 16px; }
.ra-switch:disabled { cursor: not-allowed; opacity: .5; }"""

dashboard = {
	"uid": "ra-overview",
	"title": "review_archive · overview",
	"tags": ["review_archive"],
	"schemaVersion": 41,
	"editable": True,
	"time": {"from": "now-30d", "to": "now"},
	"templating": {"list": [hidden("member"), hidden("gmail"), hidden("keys"), hidden("rev")]},
	"panels": [
		{
			"id": 2,
			"type": "marcusolsson-dynamictext-panel",
			"title": "Locations",
			"gridPos": {"x": 0, "y": 0, "w": 24, "h": 22},
			"datasource": DS,
			"targets": [query(CARD_COLUMNS)],
			"options": {
				"renderMode": "allRows",
				"wrap": False,
				"editors": ["helpers", "afterRender", "styles"],
				"contentPartials": [],
				"helpers": HELPERS,
				"defaultContent": "No place to show.",
				"content": CONTENT,
				"afterRender": AFTER_RENDER,
				"styles": STYLES,
			},
		},
		{
			"id": 3,
			"type": "table",
			"title": "All locations",
			"gridPos": {"x": 0, "y": 22, "w": 24, "h": 10},
			"datasource": DS,
			"targets": [
				query(
					[
						("label", "location", "string"),
						("enabled", "on", "boolean"),
						("snapshots_7d", "snapshots 7d", "number"),
						("snapshots_30d", "snapshots 30d", "number"),
						("live", "live", "number"),
						("listed", "listed", "number"),
						("new_7d", "new 7d", "number"),
						("posts_7d", "posts 7d", "number"),
						("responded", "responded", "number"),
						("suspended", "suspended", "number"),
						("removed", "removed", "number"),
						("reinstating", "reinstating", "number"),
						("last_run_at", "last run", "timestamp"),
						("last_run_status", "status", "string"),
					]
				)
			],
			"fieldConfig": {
				"defaults": {"custom": {"align": "auto", "filterable": True}},
				"overrides": [
					{
						"matcher": {"id": "byName", "options": "status"},
						"properties": [
							{"id": "custom.cellOptions", "value": {"type": "color-text"}},
							{"id": "mappings", "value": [{"type": "value", "options": {"ok": {"color": "green", "index": 0}, "partial": {"color": "orange", "index": 1}, "failed": {"color": "red", "index": 2}}}]},
						],
					}
				],
			},
			"options": {"showHeader": True},
		},
	],
}

open("grafana/dashboards/ra-overview.json", "w").write(json.dumps(dashboard, indent=2, ensure_ascii=False) + "\n")
