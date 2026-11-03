"""Evaluates a JSONata root over /me/overview with Infinity's own engine, in the conductor's Grafana
of its local stack (site_conductor's `nix run .#local-stack`, signed in as an admin acting as `<member>`):
`SA_CONDUCTOR_ORIGIN=<its origin> python3 examples/grafana_jsonata.py <member> '<root>'`. The root yields rows with a string `o`
(`$[0].locations.{"o": $string(...)}`); one that never reads its input comes back null.
"""

import http.cookiejar
import json
import os
import sys
import urllib.request

BASE = os.environ.get("SA_CONDUCTOR_ORIGIN") or sys.exit("SA_CONDUCTOR_ORIGIN: the local stack's conductor, as its banner prints it")
session = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
session.open(f"{BASE}/auth/login?return_to=/")  # the stack's dev sign-in: a session at once

q = {
	"refId": "A",
	"datasource": {"type": "yesoreyeram-infinity-datasource", "uid": "review_archive"},
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
req = urllib.request.Request(f"{BASE}/grafana/api/ds/query", body, {"content-type": "application/json"})
try:
	r = json.load(session.open(req))
except urllib.error.HTTPError as e:
	print(e.code, e.read().decode()[:600])
	sys.exit(1)
for f in r["results"]["A"].get("frames", []):
	names = [x["name"] for x in f["schema"]["fields"]]
	print(names, f["data"]["values"])
