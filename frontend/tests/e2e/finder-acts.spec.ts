/** Ctrl+K runs the native acts; the shortcut sheet stays readable and operable in both sizes/themes/locales. */
import { expect, test, type Page } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { ACME, bodySent, PEOPLE, scenario, signIn } from './support.ts';
import { change } from './change-support.ts';

type Locale = 'en' | 'zh-CN';
const palette = (page: Page, locale: Locale) => page.getByRole('dialog', { name: caption('@finder.title', locale), exact: true });
async function open(page: Page, locale: Locale, query: string) {
  await page.keyboard.press('Control+k');
  const finder = palette(page, locale); await expect(finder).toBeVisible();
  const field = finder.getByRole('combobox'); await field.fill(query); return finder;
}
async function act(page: Page, locale: Locale, query: string, id: string) {
  const finder = await open(page, locale, query);
  const option = finder.locator(`[role="option"][data-value="commands:${id}"]`);
  await expect(option).toBeVisible(); await expect(option).not.toHaveAttribute('aria-disabled', 'true'); await option.click();
  await expect(finder).toHaveCount(0);
}
async function appearance(page: Page, locale: Locale, theme: 'light' | 'dark') {
  const login = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login'));
  await signIn(page); const auth: unknown = await (await login).json();
  if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The fake sign-in returned no session.');
  const saved = await page.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${auth.access_token}` }, data: { locale, theme } });
  expect(saved.ok()).toBe(true); await page.reload();
  await expect(page.locator('html')).toHaveAttribute('lang', locale); await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
}
test.beforeEach(async () => { await scenario(); });

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`native acts and a searchable shortcut table at ${width}px, ${theme}, ${locale}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await appearance(page, locale, theme); await page.goto(`/thread/${ACME.mainThread}`);
    const finder = await open(page, locale, '>');
    await expect(finder.getByRole('button', { name: caption('@finder.modes.commands', locale), exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(finder.getByRole('switch')).toHaveCount(0);
    const queries: string[] = [];
    const watch = (request: { url(): string }) => { const path = new URL(request.url()).pathname; if (/\/(?:search\/names|settings\/registry|permissions)$/.test(path)) queries.push(path); };
    page.on('request', watch);
    await finder.getByRole('combobox').fill(locale === 'en' ? '> switch agent' : '> 切换 智能体');
    const option = finder.locator('[role="option"][data-value="commands:thread.change-switch"]');
    await expect(option).toBeVisible(); await expect(option).not.toHaveAttribute('aria-disabled', 'true');
    const padding = await finder.getByRole('combobox').evaluate((element) => Number.parseFloat(getComputedStyle(element).paddingLeft));
    expect(padding).toBeGreaterThanOrEqual(8);
    await info.attach('palette-acts', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    expect(queries).toEqual([]); page.off('request', watch);
    await option.click(); await expect(finder).toHaveCount(0);
    const switching = page.getByRole('dialog', { name: locale === 'en' ? /^Switch / : /^切换/ });
    await expect(switching).toBeVisible(); await expect(switching.locator('[data-change-runs]')).toBeVisible();
    await switching.getByRole('button', { name: caption('@common.close', locale), exact: true }).click();

    await act(page, locale, locale === 'en' ? '> new thread' : '> 新建 线程', 'create.thread');
    await expect(page.locator('[data-new-thread]')).toBeVisible();
    await page.getByRole('dialog').getByRole('button', { name: caption('@common.cancel', locale), exact: true }).click();

    await act(page, locale, locale === 'en' ? '> settings audit' : '> 设置 审计', 'go.place-settings.audit');
    await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}\\?tab=settings&section=audit$`));
    await act(page, locale, locale === 'en' ? '> keyboard shortcuts' : '> 键盘 快捷键', 'account.keys');
    const sheet = page.getByRole('dialog', { name: caption('@shell.shortcuts.title', locale), exact: true });
    const box = await sheet.boundingBox();
    if (box === null) throw new Error('The shortcut sheet has no rendered bounds.');
    const closeOffset = await sheet.evaluate((element) => {
      const heading = element.querySelector('h2'); const close = element.querySelector('header button');
      if (heading === null || close === null) throw new Error('The shortcut sheet lost its heading or close control.');
      const range = document.createRange(); range.selectNodeContents(heading); const line = range.getClientRects()[0];
      if (line === undefined) throw new Error('The shortcut heading has no first text line.');
      const box = close.getBoundingClientRect(); return Math.abs(box.top + box.height / 2 - line.top - line.height / 2);
    });
    expect(closeOffset).toBeLessThan(1);
    if (width === 1440) {
      expect(box.width).toBeGreaterThanOrEqual(900);
      const wrapped = await sheet.locator('tbody td[data-table-column="name"] > div > span:first-child').evaluateAll((names) => names.filter((name) => {
        const range = document.createRange(); range.selectNodeContents(name);
        return range.getClientRects().length > 1;
      }).map((name) => name.textContent));
      expect(wrapped).toEqual([]);
    } else {
      expect(box.x).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual(400);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(400);
    }
    const search = sheet.getByRole('searchbox', { name: caption('@shell.keyChoice.search', locale), exact: true });
    await expect(search).toBeFocused(); await search.fill(locale === 'en' ? 'inbox' : '收件箱');
    const table = sheet.getByRole('table', { name: caption('@shell.keyChoice.table', locale), exact: true });
    await expect(table.getByRole('row')).toHaveCount(2);
    const row = table.getByRole('row', { name: locale === 'en' ? /Go to the inbox/ : /收件箱/ });
    const actions = row.locator('[data-key-actions]'); await page.mouse.move(0, 0);
    await expect(actions).toHaveCSS('opacity', '0'); await row.hover(); await expect(actions).toHaveCSS('opacity', '1');
    await page.mouse.move(0, 0); const changeKeys = row.getByRole('button').first(); await changeKeys.focus();
    await expect(actions).toHaveCSS('opacity', '1'); await page.keyboard.press('Enter');
    await expect(sheet.locator('[data-keys="own"]')).toBeFocused(); await page.keyboard.press('Escape');
    await expect(row.getByRole('button').first()).toBeFocused(); await expect(sheet).toBeVisible();
    await search.focus(); await info.attach('shortcut-table', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    await sheet.getByRole('button', { name: caption('@common.close', locale), exact: true }).click();
    await act(page, locale, locale === 'en' ? '> help getting started' : '> 帮助 开始', 'help.article.getting-started');
    await expect(page.getByRole('heading', { name: caption('@craftGuides.gettingStarted', locale), exact: true }).first()).toBeVisible();
    const targetTheme = theme === 'light' ? 'dark' : 'light';
    const themeWrites: Promise<unknown>[] = [];
    page.on('request', (request) => {
      if (request.method() === 'PATCH' && new URL(request.url()).pathname === '/api/v1/me') {
        themeWrites.push(bodySent(request));
      }
    });
    const [themeSaved] = await Promise.all([
      page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname === '/api/v1/me'),
      act(page, locale, locale === 'en' ? `> theme ${targetTheme}` : `> 主题 ${targetTheme === 'dark' ? '深色' : '浅色'}`, `appearance.theme.${targetTheme}`),
    ]);
    expect(themeSaved.ok()).toBe(true);
    await expect(page.locator('html')).toHaveAttribute('data-theme', targetTheme);
    expect(await Promise.all(themeWrites)).toEqual([{ theme: targetTheme }]);
    await page.reload(); await expect(page.locator('html')).toHaveAttribute('data-theme', targetTheme);
  });
}

test('New document creates one native document, and Fork invokes the ordinary one-request act', async ({ page }) => {
  await signIn(page); await page.goto(`/thread/${ACME.mainThread}`);
  const creates: string[] = [];
  page.on('request', (request) => { if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/documents')) creates.push(request.url()); });
  await act(page, 'en', '> new document', 'create.document');
  await expect(page).toHaveURL(/\/doc\//); expect(creates).toHaveLength(1);
  await page.goto(`/thread/${ACME.mainThread}`); await change.mode('auto');
  const forks: string[] = [];
  page.on('request', (request) => { if (request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/forks/now')) forks.push(request.url()); });
  await act(page, 'en', '> fork', 'thread.change-fork-now');
  await expect(page).toHaveURL((url) => url.pathname.startsWith('/thread/') && url.pathname !== `/thread/${ACME.mainThread}`);
  expect(forks).toHaveLength(1);
});

test('administrator reach is the same session act, once in each direction, and is absent for a reader', async ({ page }) => {
  await signIn(page); const writes: Promise<unknown>[] = [];
  page.on('request', (request) => {
    if (request.method() === 'PUT' && new URL(request.url()).pathname.endsWith('/me/admin-mode')) {
      writes.push(bodySent(request));
    }
  });
  await act(page, 'en', '> admin on', 'account.admin.on');
  await expect(page.locator('[data-screen-titlebar]:visible [data-as-admin]')).toHaveAttribute('data-as-admin', 'true');
  await act(page, 'en', '> admin off', 'account.admin.off');
  await expect(page.locator('[data-screen-titlebar]:visible [data-as-admin]')).toHaveAttribute('data-as-admin', 'false');
  expect(await Promise.all(writes)).toEqual([{ as_admin: true }, { as_admin: false }]);
  await page.context().clearCookies(); await page.goto('/login'); await signIn(page, PEOPLE.vera);
  const finder = await open(page, 'en', '> admin');
  await expect(finder.locator('[data-value^="commands:account.admin."]')).toHaveCount(0);
});

test('Ask on the side opens the ordinary form for the fixture with a verified read-only reader', async ({ page }) => {
  await signIn(page); await page.goto(`/thread/${ACME.mainThread}`);
  const finder = await open(page, 'en', '> ask side');
  const ask = finder.locator('[data-value="commands:thread.side-ask"]'); await expect(ask).toBeVisible();
  await expect(ask).not.toHaveAttribute('aria-disabled', 'true');
  await ask.click(); const form = page.getByRole('dialog', { name: caption('@side.ask.title', 'en'), exact: true });
  await expect(form).toBeVisible(); await expect(form.getByRole('textbox').first()).toBeVisible();
});

test('a refused theme act restores the accepted theme, says why, and can be tried again', async ({ page }) => {
  await appearance(page, 'en', 'light');
  let refused = false; let writes = 0;
  await page.route('**/api/v1/me', async (route) => {
    if (route.request().method() !== 'PATCH') { await route.continue(); return; }
    writes += 1;
    if (refused) { await route.continue(); return; }
    refused = true;
    await route.fulfill({ status: 503, json: { type: 'orch:error:internal', code: 'internal', title: 'Preference unavailable', status: 503,
      detail: 'The preference could not be saved. Try again.', request_id: 'palette-preference' } });
  });
  await act(page, 'en', '> theme dark', 'appearance.theme.dark');
  await expect(page.getByText(caption('@shell.account.notSaved', 'en'), { exact: true })).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  const [saved] = await Promise.all([
    page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname === '/api/v1/me'),
    act(page, 'en', '> theme dark', 'appearance.theme.dark'),
  ]);
  expect(saved.ok()).toBe(true);
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  expect(writes).toBe(2);
});

test('a touch screen exposes the shortcut row controls without relying on hover', async ({ browser, baseURL }) => {
  const context = await browser.newContext({ baseURL, viewport: { width: 400, height: 860 }, isMobile: true, hasTouch: true });
  try {
    const page = await context.newPage(); await signIn(page); await act(page, 'en', '> keyboard shortcuts', 'account.keys');
    const sheet = page.getByRole('dialog', { name: 'Keyboard shortcuts', exact: true });
    await sheet.getByRole('searchbox', { name: 'Search keyboard shortcuts' }).fill('inbox');
    await expect(sheet.locator('[data-key-action="inbox"] [data-key-actions]')).toHaveCSS('opacity', '1');
    await expect(sheet.getByRole('button', { name: 'Remove the keys of Go to the inbox' })).toBeVisible();
  } finally { await context.close(); }
});
