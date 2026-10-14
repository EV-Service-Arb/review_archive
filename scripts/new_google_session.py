#!/usr/bin/env nix-shell
#!nix-shell -i python3 -p "python3.withPackages (p: [p.websockets])"
"""Signs the deployed scanner in to Google with a session no other browser ever holds.

Opens Playwright's Chromium on a throwaway profile; sign in, then close the window. The session
goes into the scanner's profile on rpi5 and the pod restarts. Every local copy is deleted on the
way out: a session shared with another browser dies when either side rotates it.
"""

import asyncio
import json
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import websockets

HOST, NAMESPACE, PVC, APP = "rpi5", "service-arb", "review-archive-data", "review-archive"
SIGN_IN = "https://accounts.google.com/ServiceLogin?continue=https://www.google.com/maps"
GOOGLE = ("google.com", "google.fr", "youtube.com")
FIELDS = ("name", "value", "domain", "path", "secure", "httpOnly", "sameSite", "expires", "priority", "sourceScheme", "sourcePort", "partitionKey")
FLAKE = Path(__file__).resolve().parent.parent


def build(attr: str) -> Path:
	"""`attr` of playwright-driver from the nixpkgs the image is built with, so the cookies are written by the Chromium that reads them."""
	expr = f"(import (builtins.getFlake {json.dumps(str(FLAKE))}).inputs.v_flakes.default_nixpkgs {{ system = builtins.currentSystem; }}).playwright-driver.{attr}"
	return Path(subprocess.run(["nix", "build", "--no-link", "--print-out-paths", "--impure", "--expr", expr], check=True, capture_output=True, text=True).stdout.strip())


async def cdp(profile: Path, browser: subprocess.Popen, work) -> object:
	port_file = profile / "DevToolsActivePort"
	deadline = time.monotonic() + 30
	while not port_file.exists():
		assert browser.poll() is None, f"Chromium exited with {browser.returncode} before listening"
		assert time.monotonic() < deadline, "Chromium did not listen within 30 s"
		await asyncio.sleep(0.2)
	port, path = port_file.read_text().split()
	async with websockets.connect(f"ws://127.0.0.1:{port}{path}", max_size=None) as ws:
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

		out = await work(call)
		await call("Browser.close")  # flushes the profile to disk
	browser.wait(30)
	return out


def headless(exe: Path, profile: Path, work, *flags: str):
	browser = subprocess.Popen([exe, f"--user-data-dir={profile}", "--password-store=basic", "--remote-debugging-port=0", "--no-first-run", *flags, "about:blank"], stderr=subprocess.DEVNULL)
	try:
		return asyncio.run(cdp(profile, browser, work))
	finally:
		browser.kill()


async def export(call) -> list:
	cookies = (await call("Storage.getCookies"))["cookies"]
	assert any(c["name"] == "__Secure-1PSID" for c in cookies), "the window was closed without signing in to Google"
	return [{k: c[k] for k in FIELDS if k in c and not (k == "expires" and c[k] < 0)} for c in cookies if c["domain"].lstrip(".").endswith(GOOGLE)]  # negative expiry: a session cookie


def main() -> None:
	print("building the image's Chromium…", file=sys.stderr)
	browsers, shell = build("browsers"), build("components.chromium-headless-shell")
	(chrome,) = browsers.glob("chromium-*/chrome-linux64/chrome")
	(shell,) = shell.glob("chrome-headless-shell-linux64/chrome-headless-shell")

	with tempfile.TemporaryDirectory(prefix="google-session-") as tmp:
		signed_in, scanner = Path(tmp, "signed-in"), Path(tmp, "scanner")
		print("sign in to Google in the window, then close it", file=sys.stderr)
		# no CDP while a person signs in: Google refuses sign-in to a browser it sees automated
		subprocess.run([chrome, f"--user-data-dir={signed_in}", "--password-store=basic", "--no-first-run", "--no-default-browser-check", SIGN_IN], stderr=subprocess.DEVNULL, check=True)
		cookies = headless(chrome, signed_in, export, "--headless=new")
		shutil.rmtree(signed_in)

		async def put(call):
			await call("Storage.clearCookies")
			await call("Storage.setCookies", cookies=cookies)
			got = len((await call("Storage.getCookies"))["cookies"])
			assert got == len(cookies), f"{len(cookies)} cookies set, {got} kept"

		headless(shell, scanner, put)
		db = scanner / "Default" / "Cookies"
		with sqlite3.connect(db) as c:
			kinds = {k for (k,) in c.execute("SELECT DISTINCT substr(CAST(encrypted_value AS TEXT), 1, 3) FROM cookies")}
		assert kinds == {"v10"}, f"the image's Chromium reads v10 cookies only, these are {kinds}"

		profile = "$(sudo k3s kubectl get pv $(sudo k3s kubectl -n {ns} get pvc {pvc} -o jsonpath='{{.spec.volumeName}}') -o jsonpath='{{.spec.local.path}}{{.spec.hostPath.path}}')/chromium-profile/Default".format(ns=NAMESPACE, pvc=PVC)
		remote = f"""set -e
d={profile}
sudo tee "$d/Cookies" >/dev/null
sudo rm -f "$d/Cookies-journal"
sudo chown nobody:nogroup "$d/Cookies"
sudo chmod 600 "$d/Cookies"
sudo k3s kubectl -n {NAMESPACE} delete pod -l app={APP} --wait=false"""
		subprocess.run(["ssh", HOST, remote], input=db.read_bytes(), check=True)
	print(f"{len(cookies)} cookies are in the scanner's profile, and nowhere else; the pod restarts", file=sys.stderr)


if __name__ == "__main__":
	main()
