// `nix run .#with-grafana` driven in a headless Chromium, as the member's tab: the overview's cards are
// /me/overview's, a facet narrows them, a card opens its board, whose lanes are the board's and whose
// gone cards appeal and withdraw by drag, back returns, and (the gmail on) a card's switch writes and
// redraws — all in one page load and one Grafana frame.
// `node examples/with_grafana.cjs <member> <gmail> <facet> [<base>]` inside `nix develop`, `<base>` the
// serve's (`http://127.0.0.1:59110`, with-grafana's). The switch and the drags write: run it on a copy
// of the data dir, `serve.lock` held so nothing walks.
const path = require('path');
const { chromium } = require(path.dirname(process.env.PLAYWRIGHT_CLI_JS));
const assert = require('assert');

const BASE = process.argv[5] ?? 'http://127.0.0.1:59110';
const [member, gmail] = process.argv.slice(2, 4).map(Number);
const facet = process.argv[4];
const HOLDS = {
	unresponded: (g, l) => l.responded < l.live,
	removed: (g, l) => l.removed > 0,
	reinstating: (g, l) => l.reinstating > 0,
	'new 7d': (g, l) => l.new_7d > 0,
	'posted 7d': (g, l) => l.posts_7d > 0,
	'out of tokens': (g, l) => l.held,
	failing: (g, l) => ['failed', 'partial'].includes(l.last_run_status),
	scanning: (g, l) => l.scan != null,
	off: (g, l) => !l.enabled || !g.enabled,
};

const overview = async () =>
	(await (await fetch(BASE + '/me/overview', { headers: { 'x-member': String(member) } })).json()).find((x) => x.id === gmail);
const count = (frame, n) => frame.waitForFunction((n) => document.querySelectorAll('.ra-card').length === n, n, { timeout: 30000 });

(async () => {
	const g = await overview();
	assert.ok(g, `member ${member} has no gmail ${gmail}`);
	assert.ok(HOLDS[facet], `facets: ${Object.keys(HOLDS).join(', ')}`);
	const browser = await chromium.launch({ executablePath: process.env.REVIEW_ARCHIVE_CHROME, headless: true });
	const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
	page.on('pageerror', (e) => console.log('pageerror:', e.message));
	page.on('response', async (r) => {
		if (r.status() >= 400 && r.url().includes('/grafana/api/ds/query')) console.log(r.status(), (await r.text()).slice(0, 400));
	});
	let documents = 0;
	page.on('request', (r) => r.resourceType() === 'document' && r.frame() === page.mainFrame() && documents++);

	await page.goto(`${BASE}/members/${member}/gmails/${gmail}`);
	const frame = await (await page.waitForSelector('iframe[src*="/grafana/"]')).contentFrame();
	await frame.waitForSelector('.ra-card', { timeout: 60000 });
	const kept = async () => assert.strictEqual(await frame.evaluate(() => window.marker), 'kept', 'the frame did not reload');
	const labels = await frame.$$eval('.ra-card .ra-label', (els) => els.map((e) => e.textContent));
	assert.deepStrictEqual(labels, g.locations.map((l) => l.target.label), 'cards are /me/overview, in its order');
	console.log('cards match:', labels.length);

	await frame.evaluate(() => (window.marker = 'kept'));
	const held = g.locations.filter((l) => HOLDS[facet](g, l)).length;
	await page.getByRole('button', { name: facet, exact: true }).click();
	await count(frame, held);
	await kept();
	await page.getByRole('button', { name: 'Clear' }).click();
	await count(frame, g.locations.length);
	await kept();
	console.log(`facet "${facet}": ${held} cards, the frame kept`);

	if (g.enabled) {
		const was = g.locations[0].enabled;
		for (const want of [!was, was]) {
			await frame.click('.ra-card .ra-switch');
			await frame.waitForFunction((on) => document.querySelector('.ra-card .ra-switch').classList.contains('on') === on, want, { timeout: 30000 });
			assert.strictEqual((await overview()).locations[0].enabled, want);
		}
		assert.ok(page.url().endsWith(`/gmails/${gmail}`), 'the switch is not the card: no board opened');
		console.log('switch: written and redrawn, both ways');
	} else {
		console.log('switch: not tried, the gmail is off');
	}

	const target = g.locations[0].target.id;
	await frame.click('.ra-card .ra-label');
	await page.waitForURL(`**/members/${member}/gmails/${gmail}/places/${target}`);
	let t = Date.now();
	await frame.waitForSelector('text=Reviews per run', { timeout: 30000 });
	await kept();
	const board = async () =>
		(await fetch(`${BASE}/me/gmails/${gmail}/locations/${target}/board`, { headers: { 'x-member': String(member) } })).json();
	const LANES = { snapshotted: 'Snapshotted', suspended: 'Suspended', removed: 'Removed', reinstating: 'Reinstating' };
	// a lane's tab is titled with its count; a lane in a tab not shown is not drawn
	const tab = (lane, n) => frame.locator('.tl-tab').getByText(`${LANES[lane]} · ${n}`, { exact: true });
	const lanes = async (b) => {
		for (const lane of Object.keys(LANES)) {
			await tab(lane, b[lane].length).click({ timeout: 30000 });
			await frame.waitForFunction(
				([lane, n]) => document.querySelectorAll(`.ra-lane[data-lane="${lane}"] .ra-card`).length === n,
				[lane, b[lane].length],
				{ timeout: 30000 }
			);
		}
	};
	const was = await board();
	await lanes(was);
	console.log(`board open, its lanes drawn in ${Date.now() - t} ms:`, Object.keys(LANES).map((l) => `${l} ${was[l].length}`).join(', '));
	// a drag entering a tab shows it, so a card reaches a lane tabbed with its own
	const drag = async (card, to, n) => {
		await frame.locator(card).first().hover();
		await page.mouse.down();
		await tab(to, n).hover();
		await frame.locator(`.ra-lane[data-lane="${to}"]`).hover();
		await page.mouse.up();
	};

	const from = ['removed', 'suspended'].find((l) => was[l].length);
	if (from) {
		const review = was[from][0].review.id;
		// the page writes what the frame posts up, so the API has it a moment after the drop
		const until = async (what, holds) => {
			for (const end = Date.now() + 15000; Date.now() < end; await new Promise((r) => setTimeout(r, 300))) {
				const b = await board();
				if (holds(b)) return b;
			}
			assert.fail(what);
		};
		await tab(from, was[from].length).click();
		await drag(`.ra-lane[data-lane="${from}"] .ra-card`, 'reinstating', was.reinstating.length);
		const appealed = await until(`review ${review} dragged to Reinstating is not appealed`, (b) => b.reinstating.some((c) => c.review.id === review));
		await lanes(appealed);
		// newest appeal first: the card just dragged
		await drag('.ra-lane[data-lane="reinstating"] .ra-card', from, appealed[from].length);
		await lanes(await until(`review ${review} dragged back is not withdrawn`, (b) => b[from].some((c) => c.review.id === review)));
		await kept();
		console.log(`drag: ${from} → reinstating → ${from}, written and redrawn`);
	} else {
		console.log('drag: not tried, nothing is gone');
	}
	await page.goBack();
	await page.waitForURL(`**/members/${member}/gmails/${gmail}`);
	t = Date.now();
	await count(frame, g.locations.length);
	await kept();
	console.log(`back on the overview in ${Date.now() - t} ms`);
	assert.strictEqual(documents, 1, 'one page load in all');
	console.log('back on the overview; page loads:', documents);
	await browser.close();
})().catch((e) => {
	console.error(e);
	process.exit(1);
});
