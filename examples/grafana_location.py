"""Writes grafana/dashboards/ra-location.json, which is edited here: `python3 examples/grafana_location.py` from the repo root."""

import json

DS = {"type": "yesoreyeram-infinity-datasource", "uid": "ra-api"}
STATUS = [{"type": "value", "options": {"ok": {"color": "green", "index": 0}, "partial": {"color": "orange", "index": 1}, "failed": {"color": "red", "index": 2}}}]


def query(url, columns, root=None, limit=None):
	params = [{"key": "rev", "value": "${rev}"}] + ([{"key": "limit", "value": str(limit)}] if limit else [])
	q = {
		"refId": "A",
		"datasource": DS,
		"type": "json",
		"source": "url",
		"format": "table",
		"parser": "backend",
		"url": url,
		"url_options": {"method": "GET", "params": params, "headers": [{"key": "x-member", "value": "${member}"}]},
		"columns": [{"selector": s, "text": t, "type": ty} for s, t, ty in columns],
	}
	if root:
		q["root_selector"] = root
	return q


def hidden(name):
	return {"name": name, "type": "textbox", "hide": 2, "query": "", "current": {"text": "", "value": ""}, "options": []}


BOARD = "/me/gmails/${gmail}/locations/${target}/board"
RUNS = "/targets/${target}/runs"

CARD = [
	("id", "review.id", "number"),
	("author", "review.author", "string"),
	("rating", "review.rating", "number"),
	("text", "review.text", "string"),
	("reply", "review.reply", "string"),
	("published_raw", "review.published_raw", "string"),
	("first_seen", "review.first_seen", "string"),
	("gone_at", "review.gone_at", "string"),
	("capture_url", "review.capture_url", "string"),
	("corrupted", "review.capture_corrupted", "boolean"),
	("requested_at", "reinstatement.requested_at", "string"),
	("reinstated_at", "reinstatement.reinstated_at", "string"),
]


# The page's review filter, a variable per `filter::reviews::Review` (by its Debug name): "true", the
# card must hold; "false", must not; "", either. Infinity's JSONata gives null for an absent field.
NARROW = {
	"FiveStar": "review.rating = 5",
	"Responded": '$type(review.reply) = "string"',
	"New7d": '$type(review.published_est) = "string" and $toMillis(review.published_est) >= $millis() - 7 * 86400000',
}
KEPT = " and ".join(f'("${{{v}}}" = "" or ({holds}) = ("${{{v}}}" = "true"))' for v, holds in NARROW.items())


LANES = [("snapshotted", "Snapshotted"), ("suspended", "Suspended"), ("removed", "Removed"), ("reinstating", "Reinstating")]
COUNTS = [(f"n_{lane}", f"n_{lane}", "number") for lane, _ in LANES]


# The lane's cards (`lane` set) after one row of every lane's count, which titles the lanes (`n_<lane>`):
# a lane in a tab not shown queries nothing, and a variable's own query would fetch the board again.
# Infinity reads a root as JSONata only when it starts with `$` or `(`.
def lane_root(lane):
	fields = ", ".join(f'"{name}": {path}' for name, path, _ in CARD)
	counts = ", ".join(f'"n_{l}": $count($$.{l}[{KEPT}])' for l, _ in LANES)
	return f'$append([{{"lane": "", {counts}}}], $$.{lane}[{KEPT}].{{{fields}, "lane": "{lane}"}})'


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
h.registerHelper('stars', (n) => '★'.repeat(Math.max(0, Math.min(5, n || 0))));
h.registerHelper('days', (from, to) => Math.floor((Date.parse(to) - Date.parse(from)) / 86400000));
h.registerHelper('capture', (url) => context.grafana.replaceVariables('${api}').replace(/\\/$/, '') + url);"""

WHEN = {
	"snapshotted": "{{#if published_raw}}{{published_raw}}{{else}}seen {{ago first_seen}}{{/if}}",
	"suspended": "gone {{ago gone_at}}",
	"removed": "gone {{ago gone_at}}",
	"reinstating": "requested {{ago requested_at}}",
}


def content(lane):
	reinstated = '{{#if reinstated_at}}<span class="ok">reinstated after {{days requested_at reinstated_at}}d</span>{{/if}}' if lane == "snapshotted" else ""
	return f"""<div class="ra-lane" data-lane="{lane}">
{{{{#each data}}}}{{{{#if lane}}}}
<article class="ra-card" data-i="{{{{@index}}}}">
  {{{{#if capture_url}}}}<img loading="lazy" src="{{{{capture capture_url}}}}">{{{{/if}}}}
  <div class="ra-who"><span class="ra-stars">{{{{stars rating}}}}</span><span class="ra-author">{{{{author}}}}</span></div>
  {{{{#if text}}}}<p class="ra-text">{{{{text}}}}</p>{{{{/if}}}}
  {{{{#if reply}}}}<p class="ra-text ra-reply">{{{{reply}}}}</p>{{{{/if}}}}
  <div class="ra-foot">
    <span>{WHEN[lane]}</span>
    <div class="ra-badges">
      {reinstated}
      {{{{#if corrupted}}}}<span class="warn" title="the screenshot is cut off; the next scan retakes it">corrupted</span>{{{{/if}}}}
      {{{{#if reply}}}}<span class="ok">responded</span>{{{{else}}}}<span class="warn">no reply</span>{{{{/if}}}}
    </div>
  </div>
</article>
{{{{/if}}}}{{{{/each}}}}
</div>"""


# A drag started in one lane's panel is seen by the others through the frame's document: a `window`
# property set by this code is not seen outside it. A card goes from Suspended or Removed to Reinstating
# (asked of Google), and back to where it sits gone (withdrawn); the page
# (crates/review_archive_web/src/grafana.rs) does the write.
AFTER_RENDER = """const lane = context.element.querySelector('.ra-lane');
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
  ? 'ra-reinstate'
  : d.from === 'reinstating' && to === d.gone ? 'ra-withdraw' : null;
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
});"""

STYLES = """.ra-lane { display: flex; flex-direction: column; gap: 12px; min-height: 100%; padding: 12px; box-sizing: border-box; }
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
.warn { background: rgba(255,152,48,.15); color: #ff9830; }"""


def lane_panel(i, lane, title):
	return {
		"id": 10 + i,
		"type": "marcusolsson-dynamictext-panel",
		"title": f"{title} · ${{n_{lane}}}",
		"dock": {"minWRem": 15.75, "minHRem": 8.5},
		# the gone lanes share one gridPos: the plugin opens them as tabs of one tile
		"gridPos": {"x": 8 * min(i, 1), "y": 0, "w": 8, "h": 22},
		"transparent": True,
		"datasource": DS,
		"targets": [query(BOARD, [(name, name, ty) for name, _, ty in CARD] + [("lane", "lane", "string")] + COUNTS, root=lane_root(lane))],
		"options": {
			"renderMode": "allRows",
			"wrap": False,
			"editors": ["helpers", "afterRender", "styles", "default"],
			"contentPartials": [],
			"helpers": HELPERS,
			"defaultContent": f'<div class="ra-lane" data-lane="{lane}"></div>',
			"content": content(lane),
			"afterRender": AFTER_RENDER,
			"styles": STYLES,
		},
	}


RUN_COLUMNS = [("started_at", "time", "timestamp")]

dashboard = {
	"uid": "ra-location",
	"title": "review_archive · location",
	"tags": ["review_archive"],
	"schemaVersion": 41,
	"editable": True,
	"time": {"from": "now-30d", "to": "now"},
	"refresh": "1m",
	"templating": {"list": [hidden("member"), hidden("gmail"), hidden("target"), hidden("rev"), hidden("api")] + [hidden(v) for v in NARROW] + [hidden(f"n_{lane}") for lane, _ in LANES]},
	"panels": [lane_panel(i, lane, title) for i, (lane, title) in enumerate(LANES)]
	+ [
		{
			"id": 1,
			"type": "state-timeline",
			"title": "Runs",
			"dock": {"minHRem": 3.5},
			"gridPos": {"x": 16, "y": 0, "w": 8, "h": 6},
			"transparent": True,
			"datasource": DS,
			"targets": [query(RUNS, RUN_COLUMNS + [("status", "status", "string")], limit=500)],
			"transformations": [{"id": "sortBy", "options": {"sort": [{"field": "time"}]}}],
			"fieldConfig": {"defaults": {"color": {"mode": "thresholds"}, "mappings": STATUS}, "overrides": []},
			"options": {"showValue": "never", "mergeValues": False, "rowHeight": 0.8, "legend": {"showLegend": False}},
		},
		{
			"id": 2,
			"type": "timeseries",
			"title": "Reviews per run",
			"dock": {"minHRem": 10},
			"gridPos": {"x": 16, "y": 11, "w": 8, "h": 11},
			"transparent": True,
			"datasource": DS,
			"targets": [query(RUNS, RUN_COLUMNS + [(s, s, "number") for s in ("new", "gone", "changed", "seen")], limit=500)],
			"transformations": [{"id": "sortBy", "options": {"sort": [{"field": "time"}]}}],
			"fieldConfig": {
				"defaults": {"custom": {"drawStyle": "bars", "fillOpacity": 60, "lineWidth": 1}, "decimals": 0},
				"overrides": [
					{
						"matcher": {"id": "byName", "options": "seen"},
						"properties": [
							{"id": "custom.drawStyle", "value": "line"},
							{"id": "custom.fillOpacity", "value": 0},
							{"id": "custom.axisPlacement", "value": "right"},
						],
					},
					{"matcher": {"id": "byName", "options": "new"}, "properties": [{"id": "color", "value": {"mode": "fixed", "fixedColor": "green"}}]},
					{"matcher": {"id": "byName", "options": "gone"}, "properties": [{"id": "color", "value": {"mode": "fixed", "fixedColor": "red"}}]},
				],
			},
			"options": {"legend": {"showLegend": True, "displayMode": "list", "placement": "bottom"}, "tooltip": {"mode": "multi"}},
		},
		{
			"id": 3,
			"type": "stat",
			"title": "Last run",
			"dock": {"minHRem": 6.5},
			"gridPos": {"x": 16, "y": 6, "w": 8, "h": 5},
			"transparent": True,
			"datasource": DS,
			"targets": [query(RUNS, [("status", "status", "string"), ("seen", "seen", "number"), ("tokens", "tokens", "number")], limit=1)],
			"fieldConfig": {"defaults": {"color": {"mode": "fixed", "fixedColor": "text"}, "mappings": STATUS}, "overrides": []},
			"options": {
				"reduceOptions": {"calcs": ["lastNotNull"], "fields": "/.*/", "values": False},
				"colorMode": "value",
				"graphMode": "none",
				"textMode": "value_and_name",
				"orientation": "vertical",
			},
		},
	],
}

open("grafana/dashboards/ra-location.json", "w").write(json.dumps(dashboard, indent=2, ensure_ascii=False) + "\n")
