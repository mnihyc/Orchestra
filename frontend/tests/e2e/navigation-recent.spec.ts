/** The shipping workbench carries Recent scope, reveals rows, slides hidden bars and retains tab pins. */
import { expect, type Page, test } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { ACME, PEOPLE, configureThread, emit, scenario, signIn } from './support.ts';
import { agentMessage, turn } from '../doubles/thread-rows.ts';
import { DOCS } from './documents-support.ts';

test.beforeEach(async () => { await scenario(); });

async function navigation(page: Page, width: number) {
  const bar = page.locator('[data-workbench-bar="left"]:visible');
  if (width === 400) {
    const control = page.locator('[data-container-choice="left:navigation"]');
    await expect(control).toBeVisible();
    if (!await bar.isVisible()) await control.click();
  } else {
    const control = page.getByRole('button', { name: 'Left bar', exact: true });
    await expect(control).toBeVisible();
    if (await control.getAttribute('aria-pressed') !== 'true') await control.click();
  }
  await expect(bar).toBeVisible();
  return bar;
}
async function shot(page: Page, name: string) {
  const directory = process.env.ORCH_NAV_SCREEN_DIR;
  if (directory !== undefined) await page.screenshot({ path: `${directory}/${name}.png` });
}

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`Recent and the wide table keep the same beginning at ${width}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page, PEOPLE.vera);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-workbench-chrome]')).toBeAttached();
    let bar = await navigation(page, width);
    await expect(bar.locator(`[data-tree-row="${ACME.mainThread}"]`)).toBeVisible();
    const requests: string[] = [];
    page.on('request', (request) => { if (request.method() === 'GET' && new URL(request.url()).pathname === '/api/v1/everything') requests.push(request.url()); });
    await bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Recent', exact: true }).click();
    const recentRow = bar.locator(`[data-tree-row="thread:${ACME.mainThread}"]`);
    await expect(recentRow).toBeVisible();
    const barBounds = await bar.boundingBox();
    const orderBounds = await bar.getByRole('combobox', { name: 'Recent order' }).boundingBox();
    if (barBounds === null || orderBounds === null) throw new Error('The visible bar needs its order control');
    expect(orderBounds.x).toBeGreaterThanOrEqual(barBounds.x);
    expect(orderBounds.x + orderBounds.width).toBeLessThanOrEqual(barBounds.x + barBounds.width);
    const cold = requests.length;
    expect(cold).toBe(1);
    await expect(bar.getByRole('heading', { name: 'Today', exact: true })).toBeVisible();
    await shot(page, `navigation-${width}-${theme}-recent`);
    await bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Tree', exact: true }).click();
    await bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Recent', exact: true }).click();
    await expect(recentRow).toBeVisible();
    const warm = requests.length - cold;
    expect(warm).toBeLessThanOrEqual(1);
    await bar.getByRole('combobox', { name: 'Recent order' }).click();
    await page.getByRole('option', { name: 'Active', exact: true }).click();
    await expect(recentRow).toBeVisible();
    await expect(bar.getByRole('combobox', { name: 'Recent order' })).toHaveText('Active');
    await recentRow.hover();
    await recentRow.locator('button[data-action="actions"]').click();
    await page.getByRole('menuitem', { name: 'Show in tree', exact: true }).click();
    await expect(bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Tree', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(bar.locator(`[data-tree-row="${ACME.mainThread}"]`)).toBeFocused();
    const web = bar.locator(`[data-tree-row="${ACME.web}"]`);
    await web.hover(); await web.locator('button[data-action="actions"]').click();
    await page.getByRole('menuitem', { name: 'Show only this', exact: true }).click();
    await expect(bar.locator('[data-tree-begins]')).toHaveAttribute('data-tree-begins', ACME.web);
    await bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Recent', exact: true }).click();
    await expect(recentRow).toBeVisible();
    // The Projects menu, rather than any row's menu, carries this complete listing state.
    await bar.getByRole('button', { name: 'View options', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Open in Everything', exact: true }).click();
    if (width === 400 && await bar.isVisible()) await page.locator('[data-container-choice="left:navigation"]').click();
    await expect(page).toHaveURL(/\/everything\?/);
    const address = new URL(page.url());
    expect(address.searchParams.get('parent')).toBe(ACME.web);
    expect(address.searchParams.get('arrangement')).toBe('recent');
    expect(address.searchParams.get('order')).toBe('active');
    const table = page.getByRole('treegrid', { name: 'Everything I can see' });
    await expect(table.locator(`a[href="/thread/${ACME.mainThread}"]`)).toBeVisible();
    await expect(table.getByRole('link', { name: /Baseline/ })).toHaveCount(0);
    if (width === 1440) {
      const first = table.locator('[data-everything-row]').first();
      const bounds = await first.boundingBox();
      expect(bounds?.height).toBeLessThanOrEqual(40);
      expect(await table.getByRole('link').first().evaluate((element) => element.getBoundingClientRect().height)).toBeLessThanOrEqual(24);
      const heading = page.locator('[data-workbench-bar="right"] [data-section-header]').first();
      await expect(heading).toBeVisible();
      expect((await heading.boundingBox())?.height).toBe(40);
    }
    await expect(page.locator('[data-main-area]').getByRole('combobox', { name: 'Recent order' })).toHaveText('Active');
    const mainPane = page.locator('[data-main-area]');
    const readsBeforeSelection = requests.length;
    await mainPane.getByRole('button', { name: 'Select', exact: true }).click();
    const ticks = table.getByRole('checkbox');
    await expect(ticks.nth(1)).toBeVisible();
    await ticks.nth(0).click(); await ticks.nth(1).click();
    await expect(mainPane.getByText('2 selected', { exact: true })).toBeVisible();
    await table.getByRole('button', { name: 'All', exact: true }).click();
    await expect(mainPane.getByText(`${await ticks.count()} selected`, { exact: true })).toBeVisible();
    expect(requests.length).toBe(readsBeforeSelection);
    await shot(page, `navigation-${width}-${theme}-selection`);
    await mainPane.getByRole('button', { name: 'Select', exact: true }).click();
    await expect(table.getByRole('checkbox')).toHaveCount(0);
    expect(requests.filter((url) => new URL(url).searchParams.get('recent') === 'active').every((url) => new URL(url).searchParams.has('parent'))).toBe(true);
    await shot(page, `navigation-${width}-${theme}-everything`);
    await page.reload();
    await expect(page.locator('[data-main-area]').getByRole('combobox', { name: 'Recent order' })).toHaveText('Active');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const figures = JSON.stringify({ cold, warm, total: requests.length, requests }, null, 2);
    await test.info().attach('listing-requests', { body: figures, contentType: 'application/json' });
    const directory = process.env.ORCH_NAV_SCREEN_DIR;
    if (directory !== undefined) await writeFile(`${directory}/navigation-${width}-${theme}-requests.json`, figures);
    if (width === 1440) {
      bar = await navigation(page, width);
      const main = page.locator('[data-main-area]');
      const toggle = page.getByRole('button', { name: 'Left bar', exact: true });
      await toggle.click(); await expect(bar).toHaveCount(0);
      const before = await main.boundingBox();
      await page.mouse.move(60, 160);
      const floating = page.getByRole('dialog', { name: 'Left bar', exact: true });
      await expect(floating).toBeVisible();
      expect((await main.boundingBox())?.width).toBe(before?.width);
      await shot(page, `navigation-${width}-${theme}-floating`);
      await floating.getByRole('button', { name: 'Keep open as a bar' }).click();
      await expect(floating).toHaveCount(0); await expect(bar).toBeVisible();
      await toggle.click();
      await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
      await toggle.press('Control+\\');
      await expect(bar).toBeVisible();
      await expect(floating).toHaveCount(0);
      await toggle.press('Control+\\');
      await expect(bar).toHaveCount(0);
      await page.locator('[data-bar-edge="left"]').hover();
      await expect(floating).toBeVisible();
      expect(await floating.evaluate((element) => getComputedStyle(element).animationName)).toBe('none');
      await page.keyboard.press('Escape');
      await expect(floating).toHaveCount(0);
      await expect(toggle).toBeFocused();
      const prior = page.locator('[data-main-area] [role="tab"]').first();
      await prior.click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Pin', exact: true }).click();
      const tab = page.getByRole('tab', { name: 'Everything', exact: true });
      await tab.click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Pin', exact: true }).click();
      await expect(tab.locator('..').locator('[data-pinned]')).toHaveCount(1);
      const first = page.locator('[data-main-area] [role="tab"]').first();
      const from = await tab.boundingBox(), to = await first.boundingBox();
      if (from === null || to === null) throw new Error('Both pinned tabs must be visible to reorder');
      await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
      await page.mouse.down();
      await page.mouse.move(from.x + from.width / 2 + 16, from.y + from.height / 2);
      await expect(page.locator('[id^="DndLiveRegion"]')).toHaveText('Picked up Everything.');
      await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 20 });
      await page.mouse.up();
      await expect(page.locator('[data-main-area] [role="tab"]').first()).toHaveText('Everything');
      await page.reload();
      await expect(page.getByRole('tab', { name: 'Everything', exact: true }).locator('..').locator('[data-pinned]')).toHaveCount(1);
      await shot(page, `navigation-${width}-${theme}-pinned`);
    }
  });
}

test('Recent reports cold and warm frame reads against the shared opening goals without per-row reads', async ({ page }) => {
  await configureThread(ACME.mainThread, { reset: true });
  const completed = '2026-10-15T10:00:01Z';
  await emit(ACME.mainThread, { turns: [
    { ...turn('recorded-one', { ordinal: 1, state: 'completed', inputId: 'input-one', completedAt: completed }), before_snapshot_id: 'copy-one' },
    { ...turn('recorded-two', { ordinal: 2, state: 'completed', inputId: 'input-two', completedAt: completed }), before_snapshot_id: 'copy-two' },
  ], items: [agentMessage('result-one', 'The first change is ready.', { turn: 'recorded-one' }), agentMessage('result-two', 'The second change is ready.', { turn: 'recorded-two' })] });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(page.locator('[data-restore-turn]')).toHaveCount(2);
  await expect(page.locator('[data-main-area]').getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
  const counts: Record<string, string[]> = {};
  let phase = '';
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (phase !== '' && url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') (counts[phase] ??= []).push(`${request.method()} ${url.pathname}${url.search}`);
  });
  for (const arrangement of ['tree', 'recent'] as const) {
    let bar = await navigation(page, 1440);
    await bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: arrangement === 'tree' ? 'Tree' : 'Recent', exact: true }).click();
    await page.waitForLoadState('networkidle');
    phase = `${arrangement}-cold`;
    counts[phase] = [];
    await page.reload();
    await expect(page.locator('[data-restore-turn]')).toHaveCount(2);
    await expect(page.locator('[data-main-area]').getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
    bar = await navigation(page, 1440);
    await expect(bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: arrangement === 'tree' ? 'Tree' : 'Recent', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await page.waitForLoadState('networkidle');
    phase = '';
    await page.getByRole('navigation', { name: 'Orgs and your own places' }).getByRole('link', { name: 'Everything', exact: true }).click();
    await expect(page.locator('[data-main-area]').getByRole('heading', { name: 'Everything', exact: true })).toBeVisible();
    await page.waitForLoadState('networkidle');
    phase = `${arrangement}-warm`;
    counts[phase] = [];
    await bar.locator(`a[href="/thread/${ACME.mainThread}"]`).click();
    await expect(page.locator('[data-main-area]').getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
    await page.waitForLoadState('networkidle');
    phase = '';
  }
  // The frame goal includes admission and all visible metadata; a cache return asks nothing.
  const goals = { cold: 8, warm: 0 };
  const summary = Object.fromEntries(Object.entries(counts).map(([key, requests]) => [key, { total: requests.length, requests }]));
  const figures = JSON.stringify({ goals, summary }, null, 2);
  const directory = process.env.ORCH_NAV_SCREEN_DIR;
  if (directory !== undefined) await writeFile(`${directory}/navigation-frame-requests.json`, figures);
  await test.info().attach('frame-requests', { body: figures, contentType: 'application/json' });
  expect(counts['recent-cold']?.length, figures).toBeLessThanOrEqual(goals.cold);
  expect((counts['recent-cold'] ?? []).filter((url) => url.startsWith('GET /api/v1/everything?'))).toHaveLength(1);
  expect(counts['recent-warm']).toHaveLength(goals.warm);
  for (const request of [...(counts['recent-cold'] ?? []), ...(counts['recent-warm'] ?? [])]) {
    const thread = /^GET \/api\/v1\/threads\/([^/]+)/.exec(request)?.[1];
    if (thread !== undefined) expect(thread).toBe(ACME.mainThread);
  }
});

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`shared page rhythm leaves neighbouring screens readable at ${width}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    for (const [name, href] of [['settings', '/settings/interface'], ['inbox', '/inbox'], ['environments', '/resources/environments'], ['document', `/doc/${DOCS.notes}`]] as const) {
      await page.goto(href);
      const main = page.locator('[data-main-area]');
      if (name === 'document') {
        const article = main.locator(`article[data-document="${DOCS.notes}"]`);
        await expect(article).toBeVisible();
        await expect(article.getByRole('textbox', { name: 'Title', exact: true })).toHaveText('Notes from the canary');
        await expect(article.getByRole('textbox', { name: 'Document text', exact: true })).toBeVisible();
      } else await expect(main.getByRole('heading', { level: 1 }).first()).toBeVisible();
      await page.waitForLoadState('networkidle');
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await shot(page, `rhythm-${width}-${theme}-${name}`);
    }
  });
}
