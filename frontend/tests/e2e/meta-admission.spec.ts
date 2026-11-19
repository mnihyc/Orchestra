/** Production frontend and real cookie restoration; endpoint bodies come unchanged from the E2E backend. */
import { expect, test, type Request, type Route } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

const META = '/api/v1/meta';
const REFRESH = '/api/v1/auth/refresh';
const THREAD = `/thread/${ACME.mainThread}`;
const DRAFT = `/api/v1/threads/${ACME.mainThread}/draft`;

/** Hold only delivery of the first real response. Later requests use their ordinary transport. */
function holdFirstResponse() {
  let release!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  let requested = false;
  let status: number | null = null;
  const handle = async (route: Route) => {
    if (requested) { await route.continue(); return; }
    requested = true;
    const response = await route.fetch();
    status = response.status();
    await held;
    await route.fulfill({ response });
  };
  return { handle, release, status: () => status };
}

for (const theme of ['light', 'dark'] as const) test(`cold thread admission survives AuthFrame leaving before Meta in ${theme}`, async ({ page }, info) => {
  await scenario();
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ colorScheme: theme });
  await signIn(page);
  await page.goto(THREAD);
  const main = page.locator('[data-main-area]');
  const composer = main.getByRole('textbox', { name: 'Message to the agent', exact: true });
  await expect(composer).toBeVisible();
  const draft = `Keep this ${theme} draft through cold admission and tab navigation.`;
  const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === DRAFT);
  await composer.fill(draft);
  expect((await saved).status()).toBe(200);
  await expect(main.locator('[data-draft-mark] [role="status"]')).toHaveCount(0);
  const address = new URL(THREAD, page.url()).href;
  // End setup's readers before collecting the cold document's traffic; the browser keeps its cookie.
  await page.goto('about:blank');

  const meta = holdFirstResponse(), refresh = holdFirstResponse();
  const metaRequests: Request[] = [], refreshRequests: Request[] = [];
  const metaStatuses: number[] = [], refreshStatuses: number[] = [];
  const completedMeta: Request[] = [], failedMeta: Request[] = [];
  const paths: { event: string; method: string; path: string; query: string; status?: number }[] = [];
  const checkpoints: string[] = [];
  const observed = new Set([META, REFRESH, '/api/v1/me/bootstrap', `/api/v1/threads/${ACME.mainThread}/opening`, DRAFT]);
  let documents = 0, pageErrors = 0;
  const entry = (request: Request) => {
    const url = new URL(request.url());
    return { method: request.method(), path: url.pathname, query: url.search };
  };
  page.on('request', (request) => {
    const value = entry(request);
    if (request.isNavigationRequest() && request.frame() === page.mainFrame()) documents += 1;
    if (observed.has(value.path)) paths.push({ event: 'request', ...value });
    if (value.path === META) metaRequests.push(request);
    if (value.path === REFRESH) refreshRequests.push(request);
  });
  page.on('response', (response) => {
    const value = entry(response.request());
    if (observed.has(value.path)) paths.push({ event: 'response', ...value, status: response.status() });
    if (value.path === META) metaStatuses.push(response.status());
    if (value.path === REFRESH) refreshStatuses.push(response.status());
  });
  page.on('requestfinished', (request) => { if (entry(request).path === META) completedMeta.push(request); });
  page.on('requestfailed', (request) => { if (entry(request).path === META) failedMeta.push(request); });
  page.on('pageerror', () => { pageErrors += 1; });
  await page.route((url) => url.pathname === META, meta.handle);
  await page.route((url) => url.pathname === REFRESH, refresh.handle);

  try {
    // A new document restores the real sign-in cookie; no cached JS client or injected auth state.
    await page.goto(address, { waitUntil: 'domcontentloaded' });
    await expect.poll(() => [meta.status(), refresh.status()]).toEqual([200, 200]);
    const language = page.getByRole('radiogroup', { name: 'Language', exact: true });
    const appearance = page.getByRole('radiogroup', { name: 'Theme', exact: true });
    await expect(language).toBeVisible();
    await expect(appearance).toBeVisible();
    await expect(main).toHaveCount(0);
    expect(metaStatuses).toEqual([]);
    expect(refreshStatuses).toEqual([]);
    checkpoints.push('AuthFrame visible; original Meta200 and refresh200 both held');

    refresh.release();
    await expect.poll(() => refreshStatuses).toEqual([200]);
    // Shell chrome mounts on session adoption, independently of route admission's pending Meta.
    await expect(page.locator('[data-workbench-chrome]')).toBeVisible();
    await expect(main).toBeVisible();
    await expect(language).toHaveCount(0);
    await expect(appearance).toHaveCount(0);
    expect(metaStatuses).toEqual([]);
    expect(metaRequests.length).toBe(1);
    expect(failedMeta.length).toBe(0);
    checkpoints.push('Shell visible; AuthFrame removed; original Meta200 still held');

    meta.release();
    await expect(composer).toBeVisible();
    await expect(composer).toHaveValue(draft);
    await expect(page).toHaveURL(address);
    await expect.poll(() => completedMeta.length).toBe(2);
    expect(metaStatuses).toEqual([200, 200]);
    expect(metaRequests.map((request) => entry(request))).toEqual([
      { method: 'GET', path: META, query: '' }, { method: 'GET', path: META, query: '' },
    ]);
    // Only booleans reach an assertion; credentials and response bodies are never logged.
    expect(await Promise.all(metaRequests.map(async (request) => Boolean(await request.headerValue('authorization'))))).toEqual([false, true]);
    expect(refreshRequests.map((request) => entry(request))).toEqual([{ method: 'POST', path: REFRESH, query: '' }]);
    checkpoints.push('Addressed thread and saved draft ready; one authenticated Meta followup completed');

    const active = page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { selected: true });
    const tabId = await active.getAttribute('id');
    if (tabId === null) throw new Error('The addressed thread tab must have a stable identity');
    await page.locator('[data-navigation-rail]').getByRole('link', { name: 'Help', exact: true }).click();
    await expect(page).toHaveURL(new URL('/help', address).href);
    await expect(main.getByRole('searchbox', { name: 'Search the help', exact: true })).toBeVisible();
    await page.locator(`[id="${tabId}"]`).click();
    await expect(page).toHaveURL(address);
    await expect(composer).toHaveValue(draft);
    const away = await page.context().newPage();
    try { await away.bringToFront(); await page.bringToFront(); } finally { await away.close(); }
    await composer.focus();
    await expect(composer).toBeFocused();
    await expect(main.locator('[data-screen="thread"]')).toBeVisible();
    await expect(main.locator('[data-screen-loading]:visible')).toHaveCount(0);
    await expect(page.getByText('This view stopped responding. Reload it to continue.', { exact: true })).toHaveCount(0);
    await expect(page.getByText('This screen could not be shown', { exact: true })).toHaveCount(0);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.mouse.move(0, 0);
    await page.evaluate(() => document.fonts.ready);
    const screenshot = info.outputPath(`meta-admission-settled-${theme}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach(`meta-admission-settled-${theme}`, { path: screenshot, contentType: 'image/png' });
    expect(metaRequests.length).toBe(2);
    expect(completedMeta.length).toBe(2);
    expect(failedMeta.length).toBe(0);
    expect(refreshRequests.length).toBe(1);
    expect(documents).toBe(1);
    expect(pageErrors).toBe(0);
    await expect(page).toHaveURL(address);
    await expect(composer).toHaveValue(draft);
    checkpoints.push('Help navigation, retained-tab return and browser focus stayed quiet; URL and draft retained');
  } finally {
    meta.release(); refresh.release();
    await page.unrouteAll({ behavior: 'wait' });
    await info.attach('meta-admission-paths.json', { contentType: 'application/json', body: JSON.stringify({ checkpoints, paths }, null, 2) });
  }
});
