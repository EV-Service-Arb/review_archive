#!/usr/bin/env nix-shell
#!nix-shell -i python3 -p "python3.withPackages (p: [p.websockets])"
"""Moves a Google session between Chromium profiles over CDP.

  google_session.py <port> export <cookies.json>   cookies of the browser on <port>, into the file
  google_session.py <port> import <cookies.json>   its Google cookies replace every cookie of the browser on <port>

Both close that browser, so the profile is flushed to disk. For the scanner's profile, import into
the same headless shell build the image runs, started with `--password-store=basic`: the image's
Chromium reads only `v10` cookies.
"""

import asyncio
import json
import os
import sys
import urllib.request

import websockets

GOOGLE = ("google.com", "google.fr", "youtube.com")
FIELDS = ("name", "value", "domain", "path", "secure", "httpOnly", "sameSite", "expires", "priority", "sourceScheme", "sourcePort", "partitionKey")


async def main(port: str, verb: str, path: str) -> None:
	ws_url = json.load(urllib.request.urlopen(f"http://localhost:{port}/json/version"))["webSocketDebuggerUrl"]
	async with websockets.connect(ws_url, max_size=None) as ws:
		n = 0

		async def call(method: str, **params):
			nonlocal n
			n += 1
			await ws.send(json.dumps({"id": n, "method": method, "params": params}))
			while True:
				r = json.loads(await ws.recv())
				if r.get("id") == n:
					if "error" in r:
						raise RuntimeError(f"{method}: {r['error']}")
					return r["result"]

		match verb:
			case "export":
				cookies = (await call("Storage.getCookies"))["cookies"]
				assert any(c["name"] == "__Secure-1PSID" for c in cookies), "the browser is not signed in to Google"
				fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)  # a live session
				os.write(fd, json.dumps(cookies).encode())
				os.close(fd)
				print(f"{len(cookies)} cookies → {path}")
			case "import":
				cookies = [
					{k: c[k] for k in FIELDS if k in c and not (k == "expires" and c[k] < 0)}  # negative: a session cookie
					for c in json.load(open(path))
					if c["domain"].lstrip(".").endswith(GOOGLE)
				]
				await call("Storage.clearCookies")
				await call("Storage.setCookies", cookies=cookies)
				got = len((await call("Storage.getCookies"))["cookies"])
				assert got == len(cookies), f"{len(cookies)} set, {got} kept"
				print(f"{got} cookies imported")
			case _:
				sys.exit(__doc__)
		await call("Browser.close")


if __name__ == "__main__":
	if len(sys.argv) != 4:
		sys.exit(__doc__)
	asyncio.run(main(*sys.argv[1:]))
