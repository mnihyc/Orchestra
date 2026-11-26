/** Seven ordinary detail pages stay within two screens, with visible tabs and quiet unopened panels. */
import { expect, type Page, type Request, test } from '@playwright/test';
import { resourceFixtures } from '../../e2e/layout-fixtures';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, RUNS_ON, id } from '../doubles/fixtures';
import { DOUBLE, PEOPLE, scenario, signIn } from './support';

const pages = [
  ['agent', `/resources/agents/${RUNS_ON.codexOnLaptop}`], ['machine', `/resources/machines/${RUNS_ON.laptop}`],
  ['environment', `/resources/environments/${RUNS_ON.workspace}`], ['host', `/resources/hosts/${RUNS_ON.buildHost}`],
  ['template', `/resources/templates/${RUNS_ON.defaultTemplate}`], ['mcp', `/resources/mcp/${id(0x31, 1)}`], ['mount', `/resources/mounts/${id(0x32, 1)}`],
] as const;
const creationKinds = { agent: 'agents', machine: 'machines', environment: 'environments', host: 'hosts', template: 'templates', mcp: 'mcp', mount: 'mounts' } as const;

async function allTabsVisible(page: Page, overview: string) {
  const tabs = page.getByRole('tablist').filter({ has: page.getByRole('tab', { name: overview, exact: true }) });
  const choices = tabs.getByRole('tab');
  expect(await choices.count()).toBeGreaterThanOrEqual(3); expect(await choices.count()).toBeLessThanOrEqual(6);
  const width = page.viewportSize()?.width ?? 0;
  // The shared single-row tab strip scrolls by design. Reveal every name without opening its lazy panel.
  for (const choice of await choices.all()) {
    await choice.scrollIntoViewIfNeeded(); await expect(choice).toBeInViewport();
    const bound = await choice.boundingBox();
    expect(bound).not.toBeNull(); expect(bound!.x).toBeGreaterThanOrEqual(0);
    expect(bound!.width).toBeGreaterThan(0); expect(bound!.x + bound!.width).toBeLessThanOrEqual(width + 1);
    const clipping = await choice.evaluate((element) => {
      const own = element.getBoundingClientRect(), strip = element.closest('[role="tablist"]')!.getBoundingClientRect();
      return Math.max(strip.left - own.left, own.right - strip.right, 0);
    });
    // Native scroll offsets round to CSS pixels, including at 200 percent zoom.
    expect(clipping).toBeLessThanOrEqual(1);
  }
  const selected = tabs.getByRole('tab', { name: overview, exact: true });
  await selected.scrollIntoViewIfNeeded(); await expect(selected).toHaveAttribute('aria-selected', 'true');
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`resource details are bounded, reachable and lazy (${locale}/${theme}/${width})`, async ({ page }, info) => {
    test.setTimeout(150_000);
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click(); await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const words = locale === 'en' ? english : chinese;
    // Both HTTP and live reads must project the same native fixture records.
    expect((await page.request.post(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/request-feeds/fixtures`)).ok()).toBe(true);
    let reads: string[] = [];
    const pending = new Set<Request>();
    page.on('request', (request) => { const url = new URL(request.url()); if (url.pathname.startsWith('/api/v1/')) { reads.push(`${request.method()} ${url.pathname}`); pending.add(request); } });
    page.on('requestfinished', (request) => { pending.delete(request); }); page.on('requestfailed', (request) => { pending.delete(request); });
    for (const [kind, path] of pages) {
      reads = []; pending.clear(); await page.mouse.move(0, 0); await page.goto(path);
      const heading = page.getByRole('heading', { level: 1 }); await expect(heading).toHaveCount(1); await expect(heading).toBeVisible();
      await expect(page.getByRole('button', { name: words.resourceCreation[creationKinds[kind]], exact: true })).toHaveCount(0);
      // Independent gateway/detection reports now add facts groups; each belongs to one distinct named section.
      const facts = page.locator('[data-main-area] [data-resource-facts]:visible');
      expect(await facts.count()).toBeGreaterThanOrEqual(1);
      const groups = await facts.evaluateAll((elements) => elements.map((element) => element.getAttribute('aria-label')
        ?? element.closest('section')?.getAttribute('aria-labelledby') ?? 'primary'));
      expect(new Set(groups).size).toBe(groups.length);
      await allTabsVisible(page, words.resourceDetails.overview);
      await expect(page.locator('[data-resource-details][open]')).toHaveCount(0);
      const height = await heading.evaluate((element) => {
        const body = element.closest('header')?.parentElement; if (body === null || body === undefined) throw new Error('The resource lost its page body');
        return body.scrollHeight;
      });
      expect(height, `${kind}: ordinary detail height`).toBeLessThanOrEqual(1800);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await expect.poll(() => [...pending].map((request) => `${request.method()} ${new URL(request.url()).pathname}`)).toEqual([]);
      expect(reads.length, `${kind}: first-load API budget\n${reads.join('\n')}`).toBeLessThanOrEqual(8);
      expect(reads.some((read) => /\/mounts\/[^/]+\/uses|\/machines\/[^/]+\/(policy|programs|mounts|environment-data)|\/environments\/[^/]+\/(mcp|agent-reach)|\/hosts\/[^/]+\/runtimes/.test(read)), `${kind}: unopened panels make no reads`).toBe(false);
      await info.attach(`${kind}-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    }
    // The shared route is left idle for a complete minute; this is an executable gate, never a writer's claimed measurement.
    reads = []; await page.waitForTimeout(60_000); expect(reads).toEqual([]);
    if (locale === 'en' && theme === 'light' && width === 1440) {
      await page.evaluate(() => { document.documentElement.style.zoom = '2'; }); await allTabsVisible(page, words.resourceDetails.overview);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    }
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`keeps machine Access in its six-tab Policy group (${locale}/${theme}/${width})`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    expect((await page.request.post(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/request-feeds/fixtures`)).ok()).toBe(true);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/machines/${RUNS_ON.laptop}?tab=access`);
    const navigation = page.getByRole('tablist', { name: words.machines.tabs.label, exact: true });
    await expect(navigation.getByRole('tab')).toHaveCount(6);
    await expect(navigation.getByRole('tab', { name: words.machines.tabs.policy, exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('[data-resource-section="access"]')).toBeVisible();
    const sections = page.locator('[data-section-nav]').filter({ has: page.locator('a[data-section-id="access"]') });
    // Section navigation follows its pane's container width, including a narrow pane in a wide workbench.
    const choice = sections.getByRole('combobox', { name: words.machines.tabs.policy, exact: true });
    if (await choice.isVisible()) {
      await expect(choice).toHaveValue(words.machines.tabs.access);
      await choice.click();
      await page.getByRole('option', { name: words.machines.tabs.policy, exact: true }).click();
    } else await sections.getByRole('link', { name: words.machines.tabs.policy, exact: true }).click();
    await expect(page.locator('[data-resource-section="policy"]')).toBeVisible();
    await expect(page).toHaveURL(/tab=policy/);
  });
}

for (const [width, height] of [[400, 860], [1440, 900]] as const) {
  test(`resource tabs start in the top quarter of the main area at ${width} by ${height}`, async ({ page }) => {
    test.setTimeout(150_000);
    await scenario();
    await page.setViewportSize({ width, height });
    await signIn(page);
    await resourceFixtures(page);
    // A topic, where Home and Sign in land, opens with the same one-row header as the resources.
    for (const [kind, path] of [...pages, ['provider', `/resources/providers/${RUNS_ON.openAiKey}`], ['skill', '/resources/skills'], ['topic', `/topic/${ACME.fixLogin}`]] as const) {
      await page.goto(path);
      if (kind === 'skill') await page.getByRole('link', { name: 'pdf-tools', exact: true }).first().click();
      const main = page.locator('[data-main-area]:visible').first();
      const tabs = main.getByRole('tablist').filter({ has: page.getByRole('tab', { name: 'Overview', exact: true }) }).first();
      await expect(tabs).toBeVisible();
      const geometry = await tabs.evaluate((element) => {
        const pane = element.closest('[data-main-area]')!.getBoundingClientRect();
        return { top: element.getBoundingClientRect().top - pane.top, height: pane.height };
      });
      expect(geometry.height).toBeGreaterThan(0);
      expect(geometry.top, kind).toBeGreaterThanOrEqual(0);
      expect(geometry.top, kind).toBeLessThan(geometry.height / 4);
      // The name has the room first: the state and the acts beside it give way (to a second row) before it is cut.
      const name = main.getByRole('heading', { level: 1 }).first();
      const cut = await name.evaluate((element) => ({ text: element.textContent, over: element.scrollWidth - element.clientWidth, row: element.parentElement?.closest('header')?.clientWidth ?? 0 }));
      expect(cut.over, `${kind}: "${cut.text}" is shown whole in a ${String(cut.row)} px header`).toBeLessThanOrEqual(1);
    }
  });
}

test('a compact header gives the name its room first at 400, the state and acts moving under it', async ({ page }) => {
  await scenario();
  await page.setViewportSize({ width: 400, height: 860 });
  await signIn(page);
  await resourceFixtures(page);
  // A name that fits the row by itself, but was cut ("Lab workspa…") beside Running, Edit and Details: given to the
  // environment through the world's API, so that every read of the page has it.
  const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(PEOPLE.ada) });
  const { access_token: token } = await login.json() as { access_token: string };
  const headers = { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` };
  const held = await (await fetch(`${DOUBLE}/api/v1/environments/${RUNS_ON.workspace}`, { headers })).json() as { revision: number };
  const renamed = await fetch(`${DOUBLE}/api/v1/environments/${RUNS_ON.workspace}`, { method: 'PATCH', headers, body: JSON.stringify({ name: 'Lab workspace on ada-laptop', expected_revision: held.revision }) });
  expect(renamed.ok).toBe(true);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  const main = page.locator('[data-main-area]:visible').first();
  const name = main.getByRole('heading', { level: 1, name: 'Lab workspace on ada-laptop' });
  await expect(name).toBeVisible();
  expect(await name.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
  // The state and the primary act are still there, whole, and the tabs stay in the top quarter.
  const header = main.locator('header').first();
  await expect(header.getByRole('button', { name: english.common.details, exact: true })).toBeVisible();
  const tabs = main.getByRole('tablist').filter({ has: page.getByRole('tab', { name: 'Overview', exact: true }) }).first();
  const geometry = await tabs.evaluate((element) => ({ top: element.getBoundingClientRect().top - element.closest('[data-main-area]')!.getBoundingClientRect().top, height: element.closest('[data-main-area]')!.getBoundingClientRect().height }));
  expect(geometry.top).toBeLessThan(geometry.height / 4);
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
});
