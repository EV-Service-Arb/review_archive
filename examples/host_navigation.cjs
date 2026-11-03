// The dashboard following the conductor's own navigation, in a headless Chromium against its local stack: the host's
// links move the location with `pushState` (no reload), and the element must show where they point; leaving for the
// sites and coming back must find the same running dashboard, not a second one mounted beside it.
// `SA_CONDUCTOR_ORIGIN=<origin> node examples/host_navigation.cjs <gmail> <other gmail>` inside `nix develop`, the origin
// site_conductor's `nix run .#local-stack -- --watch` (the one it prints), its signed-in user holding both gmails.
const path = require('path');
const { chromium } = require(path.dirname(process.env.PLAYWRIGHT_CLI_JS));
const assert = require('assert');

const BASE = process.env.SA_CONDUCTOR_ORIGIN;
if (!BASE) throw new Error("SA_CONDUCTOR_ORIGIN: the local stack's conductor, as its banner prints it");
const [gmail, other] = process.argv.slice(2, 4).map(Number);
assert.ok(gmail && other, 'usage: host_navigation.cjs <gmail> <other gmail>');

(async () => {
	const browser = await chromium.launch({ executablePath: process.env.REVIEW_ARCHIVE_CHROME });
	const page = await browser.newPage();
	const errors = [];
	page.on('pageerror', (e) => errors.push(e.message));
	let boots = 0;
	page.on('response', (r) => r.url().endsWith('/review_archive_web_bg.wasm') && (boots += 1));
	const dashboard = page.locator('mfe-review-archive-dashboard');
	const scope = async () => (await dashboard.innerText()).split('\n')[0];
	const shows = async (want, why) => {
		await page.waitForFunction(
			([w]) => document.querySelector('mfe-review-archive-dashboard')?.innerText.split('\n')[0] === w,
			[want],
			{ timeout: 10000 },
		).catch(() => {}); // the assert below says what it shows instead
		assert.equal(await scope(), want, why);
	};
	const name = async (id) => (await (await page.request.get(`${BASE}/api/v1/gmails`)).json()).gmails.find((g) => g.id === id).gmail;

	await page.goto(`${BASE}/auth/login?return_to=${encodeURIComponent(`/review_archive/gmails/${gmail}`)}`);
	await dashboard.locator(':text("Across locations")').waitFor({ timeout: 60000 });
	await shows(await name(gmail), 'a gmail opened by its URL');

	await page.getByRole('link', { name: 'Places', exact: true }).click();
	await page.waitForURL(/\/review_archive\/$/);
	await shows('All gmails', "the outline's Places: every gmail");

	await page.getByRole('link', { name: await name(other), exact: true }).click();
	await page.waitForURL(new RegExp(`/review_archive/gmails/${other}$`));
	await shows(await name(other), "the outline's gmail");

	await page.getByRole('link', { name: 'Review Archive', exact: true }).click();
	await shows('All gmails', "the top bar's Review Archive");

	await page.goBack();
	await shows(await name(other), 'back');

	await page.getByRole('link', { name: 'Sites', exact: true }).click();
	await page.waitForURL(/\/sites\//);
	await page.getByRole('link', { name: 'Review Archive', exact: true }).click();
	await shows('All gmails', 'back from the sites');

	assert.deepEqual(errors, [], 'no page errors');
	assert.equal(boots, 1, 'one dashboard for the page: the bundle booted once');
	console.log('ok');
	await browser.close();
})().catch((e) => {
	console.error(e);
	process.exit(1);
});
