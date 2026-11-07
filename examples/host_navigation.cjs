// The dashboard following the conductor, in a headless Chromium against its local stack: it lists the outline's
// selection (`sa:gmbs`) and sets it from its filters (`sa:gmbs-select`); the host's links move the location with
// `pushState` (no reload); leaving for the sites and coming back must find the same running dashboard, not a second
// one mounted beside it.
// `SA_CONDUCTOR_ORIGIN=<origin> node examples/host_navigation.cjs` inside `nix develop`, the origin
// site_conductor's `nix run .#local-stack -- --watch` (the one it prints), its signed-in user holding two GMBs or more.
const path = require('path');
const { chromium } = require(path.dirname(process.env.PLAYWRIGHT_CLI_JS));
const assert = require('assert');

const BASE = process.env.SA_CONDUCTOR_ORIGIN;
if (!BASE) throw new Error("SA_CONDUCTOR_ORIGIN: the local stack's conductor, as its banner prints it");

(async () => {
	const browser = await chromium.launch({ executablePath: process.env.REVIEW_ARCHIVE_CHROME });
	const page = await browser.newPage();
	const errors = [];
	page.on('pageerror', (e) => errors.push(e.message));
	let boots = 0;
	page.on('response', (r) => r.url().endsWith('/review_archive_web_bg.wasm') && (boots += 1));
	await page.addInitScript(() => {
		window.__told = [];
		addEventListener('sa:gmbs', (e) => window.__told.push(e.detail.ids));
	});
	const dashboard = page.locator('mfe-review-archive-dashboard');
	const listed = async () => /(\d+) of (\d+)/.exec(await dashboard.innerText())?.slice(1).map(Number);
	const lists = async (want, why) => {
		await page
			.waitForFunction(([w]) => /(\d+) of (\d+)/.exec(document.querySelector('mfe-review-archive-dashboard')?.innerText ?? '')?.[0] === w, [`${want[0]} of ${want[1]}`], { timeout: 10000 })
			.catch(() => {}); // the assert below says what it lists instead
		assert.deepEqual(await listed(), want, why);
	};
	const select = (ids) => page.evaluate((ids) => dispatchEvent(new CustomEvent('sa:gmbs-select', { detail: { ids } })), ids);
	const told = () => page.evaluate(() => window.__told.at(-1));

	await page.goto(`${BASE}/auth/login?return_to=${encodeURIComponent('/review_archive/gmbs?grafana=false')}`);
	await dashboard.locator(':text("Across locations")').waitFor({ timeout: 60000 });
	const gmbs = (await (await page.request.get(`${BASE}/api/review_archive/me/overview`)).json()).flatMap((g) => g.gmbs);
	const tracks = (ids) => gmbs.filter((b) => ids === null || ids.includes(b.id)).length; // a GMB has one track
	const tracked = gmbs.map((b) => b.id);
	assert.ok(tracked.length >= 2, 'the signed-in user has two GMBs or more');

	await select(null);
	await lists([tracks(null), tracks(null)], 'every GMB');
	await select([]);
	await lists([0, 0], 'none');
	const two = tracked.slice(0, 2);
	await select(two);
	await lists([tracks(two), tracks(two)], 'the outline picked two');

	// the filter row narrows the selection and sets it, Clear gives the two back
	const place = gmbs.find((b) => b.id === two[0]).place_id;
	await dashboard.getByPlaceholder('regex: address, name, place id, gmail').fill(place);
	await lists([tracks([two[0]]), tracks(two)], "a place id: its GMB's tracks, of the two");
	await page.waitForFunction(([id]) => JSON.stringify(window.__told.at(-1)) === JSON.stringify([id]), [two[0]], { timeout: 5000 });
	await dashboard.getByRole('button', { name: 'Clear' }).click();
	await lists([tracks(two), tracks(two)], 'cleared: the two again');
	assert.deepEqual(await told(), [...two].sort((a, b) => a - b), 'the host holds the two again');

	// a selection made elsewhere clears the row
	await dashboard.getByPlaceholder('regex: address, name, place id, gmail').fill(place);
	await lists([tracks([two[0]]), tracks(two)], 'narrowed again');
	await select(null);
	await lists([tracks(null), tracks(null)], "the outline's own pick");
	assert.equal(await dashboard.getByPlaceholder('regex: address, name, place id, gmail').inputValue(), '', 'the row no longer describes the selection');

	await page.getByRole('link', { name: 'Sites', exact: true }).click();
	await page.waitForURL(/\/sites\//);
	await page.getByRole('link', { name: 'Review Archive', exact: true }).click();
	await lists([tracks(null), tracks(null)], 'back from the sites');

	assert.deepEqual(errors, [], 'no page errors');
	assert.equal(boots, 1, 'one dashboard for the page: the bundle booted once');
	console.log('ok');
	await browser.close();
})().catch((e) => {
	console.error(e);
	process.exit(1);
});
