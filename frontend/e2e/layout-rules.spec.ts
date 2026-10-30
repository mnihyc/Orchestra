/** Layout and accessibility for every fake-world route, both languages/themes, on a desktop and a phone. */
import { expect, test, type BrowserContext, type Page, type TestInfo } from '@playwright/test';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { asAdmin } from '../tests/e2e/admin-mode-support.ts';
import { replySent, scenario, signIn } from '../tests/e2e/support.ts';
import { resourceFixtures } from './layout-fixtures.ts';
import { documentTreeFixture } from '../tests/e2e/document-tree-support.ts';
import { SIGNED_IN_ROUTES, SIGNED_OUT_ROUTES, type LayoutRoute } from './layout-routes.ts';
import { auditHighlightedOptions, auditHoverIcons, auditOpenMenus, auditPage, DUPLICATE_ALLOW_LIST, FULL_VALUE_DISCLOSURE, screenReady } from './layout-rules-helpers.ts';
import { caption } from './capture/open.ts';

const normalize = (path: string) => path.replace(/\/$/, '') || '/';
function patterns(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return patterns(path);
    if (!entry.name.endsWith('.tsx')) return [];
    return [...readFileSync(path, 'utf8').matchAll(/createFileRoute\(['"]([^'"]+)['"]\)/g)].map((match) => normalize(match[1] ?? '/'));
  });
}

test('the layout manifest covers every route and every parameterized section without a silent skip', () => {
  const claimed = new Set([...SIGNED_IN_ROUTES, ...SIGNED_OUT_ROUTES].map((row) => row.pattern));
  const declared = patterns(fileURLToPath(new URL('../src/routes', import.meta.url)));
  expect(declared.filter((pattern) => !claimed.has(pattern)), 'Add a concrete fake-world route for every new screen.').toEqual([]);
  const checks = [
    ['features/settings/sections.ts', 'ORG_SECTIONS', '/org/$slug/settings/$section', 'path'],
    ['features/settings/sections.ts', 'PROJECT_SECTIONS', '/project/$projectId/settings', 'section'],
    ['features/settings/sections.ts', 'TOPIC_SECTIONS', '/topic/$topicId', 'section'],
    ['features/resources/tabs.ts', 'PROVIDER_TABS', '/resources/providers/$id', 'tab'],
    ['features/resources/tabs.ts', 'ENVIRONMENT_TABS', '/resources/environments/$id', 'tab'],
    ['features/resources/tabs.ts', 'AGENT_TABS', '/resources/agents/$id', 'tab'],
    ['features/resources/tabs.ts', 'HOST_TABS', '/resources/hosts/$id', 'tab'],
    ['features/resources/tabs.ts', 'TEMPLATE_TABS', '/resources/templates/$id', 'tab'],
    ['features/admin/sections.ts', 'SECTIONS', '/admin/$section', 'id'],
  ] as const;
  for (const [file, constant, pattern, part] of checks) {
    const source = readFileSync(fileURLToPath(new URL(`../src/${file}`, import.meta.url)), 'utf8');
    const array = new RegExp(`export const ${constant}[^=]*=\\s*\\[([\\s\\S]*?)\\]`).exec(source)?.[1];
    expect(array, `${file} ${constant} must remain discoverable.`).toBeDefined();
    const names = Array.from((array ?? '').matchAll(part === 'id' ? /id:\s*'([^']+)'/g : /'([^']+)'/g), (match) => match[1] ?? '');
    expect(names.length, `${constant} must not become an empty coverage check.`).toBeGreaterThan(0);
    const seen = SIGNED_IN_ROUTES.filter((row) => row.pattern === pattern).map((row) => {
      const url = new URL(row.url, 'http://layout.test');
      return part === 'path' || part === 'id' ? url.pathname.split('/').at(-1) : url.searchParams.get(part);
    });
    expect(names.filter((name) => !seen.includes(name)), `Unwalked members of ${constant}`).toEqual([]);
  }
  for (const entry of DUPLICATE_ALLOW_LIST) {
    expect(entry.reason.trim(), entry.text).not.toBe('');
    expect(entry.selectors.length, entry.text).toBeGreaterThan(1);
    expect(entry.selectors, entry.text).not.toContain('*');
  }
});

async function inspect(page: Page, row: LayoutRoute, locale: string, theme: string, info: TestInfo) {
  try {
  await page.goto(row.url);
  await screenReady(page, locale, theme);
  const result = await auditPage(page);
  result.faults.push(...await auditHoverIcons(page, result.icons));
  // Read-only pickers expose descriptions, labels and boundaries that are absent while closed.
  result.faults.push(...await auditOpenMenus(page, result.menus));
  const faults = [...new Map(result.faults.map((fault) => [JSON.stringify(fault), fault])).values()];
  await info.attach('layout-rules', { body: JSON.stringify({ route: row.pattern, url: row.url, note: row.note, locale, theme,
    viewport: page.viewportSize(), faults, cutValues: result.cuts, duplicateAllowList: DUPLICATE_ALLOW_LIST, fullValueDisclosure: FULL_VALUE_DISCLOSURE, appliedAllowances: result.allowances }, null, 2), contentType: 'application/json' });
  expect(faults, `${row.url} ${locale} ${theme} ${page.viewportSize()?.width ?? 0}px`).toEqual([]);
  } finally {
    if (!page.isClosed()) await info.attach('layout-screen', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  }
}

// These are deterministic layout rules, not an intermittent network probe. Each finding is reported once.
test.describe.configure({ retries: 0 });
for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test.describe(`layout rules ${width}px ${theme} ${locale}`, () => {
    // One browser and one fake world per matrix entry; routes remain separate bounded tests with separate evidence.
    let context: BrowserContext | undefined;
    let page: Page;
    test.beforeAll(async ({ browser }, info) => {
      await scenario();
      context = await browser.newContext({ baseURL: info.project.use.baseURL, viewport: { width, height: 900 }, colorScheme: theme,
        reducedMotion: 'reduce', locale: 'en-US', timezoneId: 'UTC' });
      page = await context.newPage();
      await resourceFixtures(page);
      const loggedIn = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
      await signIn(page);
      const auth: unknown = await loggedIn;
      if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The fake sign-in supplied no access token.');
      // Use the actual session switch before changing the language; its ordinary off state is covered by the dedicated suites.
      await asAdmin(page, true);
      const saved = await page.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${auth.access_token}` }, data: { theme, locale } });
      expect(saved.ok(), 'The requested appearance must be persisted through the fake API.').toBe(true);
      await page.reload();
      await screenReady(page, locale, theme);
    });
    test.afterAll(async () => { if (context !== undefined) await context.close(); });
    for (const row of SIGNED_IN_ROUTES) {
      test(row.url, async ({ browserName }, info) => {
        expect(browserName, 'Computed-name evidence uses Chromium accessibility nodes.').toBe('chromium');
        if (row.fixture === 'document-tree') {
          // Folder projections are typed fixtures; keep their routes away from the shared page's later walks.
          const folder = await context!.newPage();
          try { await resourceFixtures(folder); await documentTreeFixture(folder); await inspect(folder, row, locale, theme, info); }
          finally { await folder.close(); }
        } else await inspect(page, row, locale, theme, info);
      });
    }
    test('the native command palette keeps every keyboard and pointer highlight readable', async ({ browserName }, info) => {
      expect(browserName).toBe('chromium');
      await page.goto('/org/acme'); await screenReady(page, locale, theme);
      await page.keyboard.press('Control+k');
      const finder = page.getByRole('dialog', { name: caption('@finder.title', locale), exact: true });
      await expect(finder).toBeVisible(); await finder.getByRole('combobox').fill('>');
      await expect(finder.getByRole('option').first()).toBeVisible();
      const faults = [...(await auditPage(page)).faults.filter((fault) => ['choice-word-width', 'choice-text-contrast', 'duplicate-action'].includes(fault.rule)),
        ...await auditHighlightedOptions(page)];
      await info.attach('palette-layout', { body: JSON.stringify({ locale, theme, width, faults }, null, 2), contentType: 'application/json' });
      await info.attach('palette-highlight', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
      expect(faults).toEqual([]);
      // One active full-value tooltip may consume Escape before the palette. Stop as soon as
      // its open state ends, so an exit animation never sends Escape to an underlying prompt.
      for (let attempt = 0; attempt < 2 && await finder.evaluateAll((elements) => elements.some((element) => element.getAttribute('data-state') === 'open')); attempt += 1) await page.keyboard.press('Escape');
      await expect(finder).toHaveCount(0);
    });
    test('a late ellipsis and a line clamp reveal their full value by pointer and keyboard', async () => {
      await page.goto('/org/acme'); await screenReady(page, locale, theme);
      const full = locale === 'en' ? 'The entire long value, including its final words. '.repeat(8) : '完整的长文本，包括末尾的文字。'.repeat(8);
      await page.evaluate((full) => {
        const box = document.createElement('section'); box.id = 'layout-overflow-example';
        box.style.cssText = 'position:fixed;inset:60px auto auto 16px;width:160px;z-index:var(--layer-dropdown);background:var(--surface);color:var(--text)';
        for (const kind of ['ellipsis', 'clamp']) {
          const value = document.createElement('span'); value.dataset.layoutExample = kind; value.textContent = full;
          value.style.cssText = kind === 'ellipsis' ? 'display:block;width:160px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis' : 'display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;width:160px;overflow:hidden';
          box.append(value);
        }
        document.body.append(box);
      }, full);
      for (const kind of ['ellipsis', 'clamp']) {
        const value = page.locator(`[data-layout-example="${kind}"]`);
        await expect(value).toHaveAttribute('tabindex', '0');
        await value.hover();
        await expect(page.locator('[data-overflow-tooltip]')).toHaveText(full.trim());
        await value.focus();
        await expect(value).toBeFocused();
        await expect(page.locator('[data-overflow-tooltip]')).toHaveText(full.trim());
        await page.keyboard.press('Escape');
        await expect(page.locator('[data-overflow-tooltip]')).toHaveCount(0);
        await page.mouse.move(0, 0);
      }
      await page.locator('#layout-overflow-example').evaluate((box) => box.remove());
    });
  });
  test.describe(`signed-out layout rules ${width}px ${theme} ${locale}`, () => {
    for (const row of SIGNED_OUT_ROUTES) {
      test(row.url, async ({ page }, info) => {
        await scenario(row.scenario ?? { meta: { registration_mode: 'open' } });
        await page.setViewportSize({ width, height: 900 });
        await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
        await page.addInitScript(({ theme, locale }) => {
          localStorage.setItem('orch.appearance', JSON.stringify({ theme, locale, density: 'comfortable' }));
        }, { theme, locale });
        await inspect(page, row, locale, theme, info);
      });
    }
  });
}
