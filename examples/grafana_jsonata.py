"""Evaluates a JSONata root over /me/overview with Infinity's own engine, under `nix run .#with-grafana`:
`python3 examples/grafana_jsonata.py <member> '<root>'`. The root yields rows with a string `o`
(`$[0].locations.{"o": $string(...)}`); one that never reads its input comes back null.
"""

import json
import sys
import urllib.request

q = {
	"refId": "A",
	"datasource": {"type": "yesoreyeram-infinity-datasource", "uid": "ra-api"},
	"type": "json",
	"source": "url",
	"format": "table",
	"parser": "backend",
	"url": "/me/overview",
	"url_options": {"method": "GET", "headers": [{"key": "x-member", "value": sys.argv[1]}]},
	"root_selector": sys.argv[2],
	"columns": [{"selector": "o", "text": "o", "type": "string"}],
}
body = json.dumps({"queries": [q], "from": "now-1h", "to": "now"}).encode()
req = urllib.request.Request("http://127.0.0.1:59110/grafana/api/ds/query", body, {"content-type": "application/json"})
try:
	r = json.load(urllib.request.urlopen(req))
except urllib.error.HTTPError as e:
	print(e.code, e.read().decode()[:600])
	sys.exit(1)
for f in r["results"]["A"].get("frames", []):
	names = [x["name"] for x in f["schema"]["fields"]]
	print(names, f["data"]["values"])
