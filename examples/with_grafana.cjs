// `nix run .#with-grafana` driven in a headless Chromium, as the member's tab: the overview's cards are
// /me/overview's, a facet narrows them without reloading the frame, a card opens its board with no
// page load and back returns, and (the gmail on) a card's switch writes and redraws.
// `node examples/with_grafana.cjs <member> <gmail> <facet>` inside `nix develop`. The switch writes:
// run it on a copy of the data dir, `serve.lock` held so nothing walks.
const path = require('path');
const { chromium } = require(path.dirname(process.env.PLAYWRIGHT_CLI_JS));
const assert = require('assert');

const BASE = 'http://127.0.0.1:59110';
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
	const frame = await (await page.waitForSelector('iframe[src*="ra-overview"]')).contentFrame();
	await frame.waitForSelector('.ra-card', { timeout: 60000 });
	const labels = await frame.$$eval('.ra-card .ra-label', (els) => els.map((e) => e.textContent));
	assert.deepStrictEqual(labels, g.locations.map((l) => l.target.label), 'cards are /me/overview, in its order');
	console.log('cards match:', labels.length);

	await frame.evaluate(() => (window.marker = 'kept'));
	const held = g.locations.filter((l) => HOLDS[facet](g, l)).length;
	await page.getByRole('button', { name: facet, exact: true }).click();
	await count(frame, held);
	assert.strictEqual(await frame.evaluate(() => window.marker), 'kept', 'the frame did not reload');
	await page.getByRole('button', { name: 'Clear' }).click();
	await count(frame, g.locations.length);
	assert.strictEqual(await frame.evaluate(() => window.marker), 'kept');
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

	await frame.click('.ra-card .ra-label');
	await page.waitForURL(`**/members/${member}/gmails/${gmail}/places/${g.locations[0].target.id}`);
	const stats = await (await page.waitForSelector('iframe[src*="ra-location"]', { timeout: 30000 })).contentFrame();
	await stats.waitForSelector('text=Reviews per run', { timeout: 60000 });
	console.log('board open, its Stats tile drawn');
	await page.goBack();
	await page.waitForURL(`**/members/${member}/gmails/${gmail}`);
	await (await (await page.waitForSelector('iframe[src*="ra-overview"]')).contentFrame()).waitForSelector('.ra-card', { timeout: 60000 });
	assert.strictEqual(documents, 1, 'one page load in all');
	console.log('back on the overview; page loads:', documents);
	await browser.close();
})().catch((e) => {
	console.error(e);
	process.exit(1);
});
