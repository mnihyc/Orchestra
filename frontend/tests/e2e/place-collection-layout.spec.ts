/** Collection content starts on the first screen, with one header/bar and metadata or defaults only where requested. */
import { expect, type Page, type Request, test } from '@playwright/test';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, changeTree, expectAccessible, scenario, signIn } from './support.ts';

interface DefaultWrite { value: unknown; expected_revision?: number }
type DefaultWindow = typeof window & { __providerDefaultWrites: DefaultWrite[] };

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
/** A fixture operation has no external work; a missing local answer must end inside the normal test budget. */
const CONTROL_TIMEOUT_MS = 10_000;
async function appearance(locale: 'en' | 'zh-CN') {
  const response = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST',
    headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme: 'system' }), signal: AbortSignal.timeout(CONTROL_TIMEOUT_MS) });
  expect(response.ok).toBe(true);
}

/** Compare to the actual scroll viewport and bottom navigation, rather than inventing a pixel cutoff. */
async function firstContentFits(page: Page, selector: string) {
  const content = page.getByTestId('main').locator(selector).first();
  await expect(content).toBeVisible();
  const measurement = await content.evaluate((element) => {
    const box = element.getBoundingClientRect();
    const scroll = element.closest('[data-scroll-area]')?.getBoundingClientRect();
    const bottom = document.querySelector('[data-bottom-bar]')?.getBoundingClientRect().top ?? window.innerHeight;
    const visibleBottom = Math.min(scroll?.bottom ?? window.innerHeight, bottom, window.innerHeight);
    return { top: box.top, bottom: box.bottom, viewportTop: scroll?.top ?? 0, viewportBottom: visibleBottom };
  });
  expect(measurement.top).toBeGreaterThanOrEqual(measurement.viewportTop);
  expect(measurement.bottom).toBeLessThanOrEqual(measurement.viewportBottom);
}

test.beforeEach(async () => { await scenario(); });

for (const locale of ['en', 'zh-CN'] as const) {
  test(`the project list leads at 1440 and 400 in both themes (${locale})`, async ({ page }) => {
    await signIn(page); await appearance(locale);
    const openingReads: string[] = [];
    const pending = new Set<Request>();
    page.on('request', (request) => {
      if (request.method() === 'GET' && request.url().includes('/api/v1/')) {
        openingReads.push(new URL(request.url()).pathname); pending.add(request);
      }
    });
    page.on('requestfinished', (request) => pending.delete(request));
    page.on('requestfailed', (request) => pending.delete(request));
    await page.goto(`/project/${ACME.web}?view=list`);
    const main = page.getByTestId('main');
    const header = main.locator('header').filter({ has: page.getByRole('heading', { name: 'Web', exact: true, level: 1 }) });
    const createName = locale === 'en' ? 'New topic' : '新建话题';
    await expect(header.getByRole('button', { name: createName, exact: true })).toHaveCount(1);
    await expect(main.locator('[data-filter-bar]')).toHaveCount(1);
    await expect(main.locator('textarea')).toHaveCount(0);
    await expect(main.locator('[data-provider-default]')).toHaveCount(0);
    await expect(main.locator('[data-project-topics] table tbody tr').first()).toBeVisible();
    let quiet = 0;
    await expect.poll(() => { quiet = pending.size === 0 ? quiet + 1 : 0; return quiet; }, { timeout: CONTROL_TIMEOUT_MS }).toBeGreaterThanOrEqual(2);
    expect(openingReads.filter((path) => /\/providers(?:\/|$)|\/start-options$/.test(path))).toEqual([]);
    expect(openingReads.length, 'the complete project opening, including shell reads').toBeLessThanOrEqual(8);
    const settledReadCount = openingReads.length;
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await firstContentFits(page, '[data-project-topics] tbody tr');
      expect(await main.evaluate((element) => {
        const bar = element.querySelector('[data-filter-bar]');
        const topics = element.querySelector('[data-project-topics]');
        const folders = element.querySelector('[data-places-list]');
        return bar !== null && topics !== null && Boolean(bar.compareDocumentPosition(topics) & Node.DOCUMENT_POSITION_FOLLOWING)
          && (folders === null || Boolean(topics.compareDocumentPosition(folders) & Node.DOCUMENT_POSITION_FOLLOWING));
      })).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      for (const theme of ['light', 'dark'] as const) {
        await expectAccessible(page, theme, '[data-main-area]');
        await firstContentFits(page, '[data-project-topics] tbody tr');
        await page.screenshot({ path: test.info().outputPath(`project-${locale}-${width}-${theme}.png`), fullPage: true });
      }
    }
    expect(openingReads, 'resizing and both-theme inspections start no idle API reads').toHaveLength(settledReadCount);
    const beforeAct = openingReads.length;
    const create = header.getByRole('button', { name: createName, exact: true });
    await create.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('dialog')).toBeVisible();
    expect(openingReads.slice(beforeAct).filter((path) => path.endsWith('/topics') || path.endsWith('/children'))).toEqual([]);
    await page.keyboard.press('Escape');
    await expect(create).toBeFocused();
  });

  test(`root and department places follow their bar without inline descriptions (${locale})`, async ({ page }) => {
    await signIn(page); await appearance(locale);
    await changeTree([{ id: ACME.payroll, name: 'Payroll and benefits for the whole team, with a complete wrapping name' }]);
    for (const [path, title] of [['/org/acme', 'Acme'], ['/org/acme.research', 'Research']] as const) {
      await page.goto(path);
      const main = page.getByTestId('main');
      await expect(main.getByRole('heading', { name: title, level: 1, exact: true })).toBeVisible();
      await expect(main.locator('[data-places-list]').first()).toBeVisible();
      await expect(main.locator('[data-filter-bar]')).toHaveCount(1);
      expect(await main.evaluate((element) => {
        const bar = element.querySelector('[data-filter-bar]');
        const list = element.querySelector('[data-places-list]');
        return bar !== null && list !== null && Boolean(bar.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING)
          && !Array.from(element.querySelectorAll('p')).some((line) => line.textContent === 'acme.research');
      })).toBe(true);
      for (const width of [1440, 400]) {
        await page.setViewportSize({ width, height: 900 });
        // Daily work has its own earlier priority; the places subsection remains compact when reached.
        await main.locator('[data-filter-bar]').evaluate((element) => element.scrollIntoView({ block: 'start' }));
        await firstContentFits(page, '[data-places-list] article');
        const offsets = await main.locator('[data-places-list] article').evaluateAll((rows) => rows.map((row) => {
          const name = row.querySelector('h3 a');
          const icon = row.querySelector('svg');
          if (name === null || icon === null) throw new Error('each place has its title and icon');
          const line = name.getBoundingClientRect(); const sign = icon.getBoundingClientRect();
          return Math.abs(sign.top + sign.height / 2 - line.top - Number.parseFloat(getComputedStyle(name).lineHeight) / 2);
        }));
        for (const offset of offsets) expect(offset).toBeLessThan(1);
        for (const theme of ['light', 'dark'] as const) {
          await expectAccessible(page, theme, '[data-main-area]');
          await page.screenshot({ path: test.info().outputPath(`places-${title}-${locale}-${width}-${theme}.png`), fullPage: true });
        }
      }
    }
  });

  test(`the provider default is changed by name in project settings and its answer updates the row (${locale})`, async ({ page }) => {
    // Observe the clone of the streamed native Request; send its original body unchanged.
    await page.addInitScript(() => {
      const watched = window as DefaultWindow; watched.__providerDefaultWrites = [];
      const send = window.fetch.bind(window);
      window.fetch = (input, init) => {
        if (input instanceof Request && input.method === 'PUT' && new URL(input.url).pathname.endsWith('/settings/agent.default_provider_id')) {
          void input.clone().json().then((body: DefaultWrite) => watched.__providerDefaultWrites.push(body));
        }
        return send(input, init);
      };
    });
    await signIn(page); await appearance(locale);
    await page.goto(`/project/${ACME.web}/settings?section=policies&setting=agent.default_provider_id`);
    const row = page.locator('[data-setting="agent.default_provider_id"]');
    const field = row.getByRole('combobox');
    await expect(field).toHaveValue('OpenAI key');
    await expect(row).not.toContainText(RUNS_ON.openAiKey);
    const settingsReads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && path.endsWith(`/nodes/${ACME.web}/settings`)) settingsReads.push(path);
    });
    await field.click();
    await page.getByRole('option', { name: 'Codex subscription', exact: true }).click();
    await expect(field).toHaveValue('Codex subscription');
    await expect(row.getByRole('status')).toContainText(locale === 'en' ? 'Saved' : '已保存');
    await expect.poll(() => page.evaluate(() => (window as DefaultWindow).__providerDefaultWrites.length)).toBe(1);
    const writes = await page.evaluate(() => (window as DefaultWindow).__providerDefaultWrites);
    expect(writes).toHaveLength(1);
    expect(writes[0]?.value).toBe(RUNS_ON.codexSubscription);
    expect(writes[0]?.expected_revision).toBeGreaterThan(0);
    expect(settingsReads).toEqual([]);
    await expect(row).not.toContainText(RUNS_ON.codexSubscription);
    await page.reload();
    await expect(page.locator('[data-setting="agent.default_provider_id"]').getByRole('combobox')).toHaveValue('Codex subscription');
  });
}
