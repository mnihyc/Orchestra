/** Seven ordinary detail pages stay within two screens, with visible tabs and quiet unopened panels. */
import { expect, type Page, type Request, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON, id } from '../doubles/fixtures';
import { scenario, signIn } from './support';

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
  for (const choice of await choices.all()) await expect(choice).toBeInViewport();
  const bounds = await tabs.evaluate((element) => Array.from(element.querySelectorAll<HTMLElement>('[role="tab"]')).map((tab) => {
    const rect = tab.getBoundingClientRect(); return { left: rect.left, right: rect.right, width: rect.width };
  }));
  const width = page.viewportSize()?.width ?? 0;
  for (const bound of bounds) { expect(bound.left).toBeGreaterThanOrEqual(0); expect(bound.width).toBeGreaterThan(0); expect(bound.right).toBeLessThanOrEqual(width + 1); }
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
      reads = []; pending.clear(); await page.goto(path);
      const heading = page.getByRole('heading', { level: 1 }); await expect(heading).toHaveCount(1); await expect(heading).toBeVisible();
      await expect(page.getByRole('button', { name: words.resourceCreation[creationKinds[kind]], exact: true })).toHaveCount(0);
      await expect(page.locator('[data-resource-facts]')).toHaveCount(1); await allTabsVisible(page, words.resourceDetails.overview);
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
