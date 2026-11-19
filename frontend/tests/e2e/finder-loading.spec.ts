/** P9: a person opens and types in Find while the real shell aggregate is held, then keeps that same field. */
import { expect, test } from '@playwright/test';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`Ctrl+K opens during Loading and retains its query through readiness at ${width}/${locale}/${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    if (locale === 'zh-CN') {
      await signIn(page); await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    let release: (() => void) | undefined;
    let requested: (() => void) | undefined;
    const held = new Promise<void>((resolve) => { release = resolve; });
    const seen = new Promise<void>((resolve) => { requested = resolve; });
    const reads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && path.startsWith('/api/v1/')) reads.push(path.slice('/api/v1'.length));
    });
    await page.route('**/api/v1/me/bootstrap', async (route) => { requested?.(); await held; await route.continue(); });
    try {
      if (locale === 'en') {
        // An ordinary signed-out deep link keeps the loading destination fixed, independent of landing-page reads.
        await page.goto(`/login?redirect=${encodeURIComponent(`/topic/${ACME.fixLogin}`)}`);
        await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
        await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
        await page.getByRole('button', { name: 'Sign in', exact: true }).click();
        await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}`));
      }
      else await page.goto(`/topic/${ACME.fixLogin}`);
      await seen;
      const chrome = page.locator('[data-workbench-chrome]');
      await expect(chrome).toBeVisible();
      await expect(page.locator('[data-main-area]').getByRole('status', { name: /^(Loading…|加载中…)$/, exact: true })).toBeVisible();
      if (width === 1440) {
        await expect(page.locator('[data-navigation-rail]')).toBeVisible();
        await expect(page.locator('[data-workbench-bar="left"]')).toBeVisible();
      } else await expect(page.locator('[data-narrow-containers]')).toBeVisible();
      expect(reads.filter((path) => /^\/(?:orgs|topics|projects|threads|documents|environments|providers|agents)(?:\/|$)/.test(path))).toEqual([]);
      expect(reads.filter((path) => path.startsWith('/me/') && path !== '/me/bootstrap')).toEqual([]);
      const originalChrome = await chrome.elementHandle();
      await page.screenshot({ path: info.outputPath('bootstrap-held.png'), fullPage: true, animations: 'disabled' });
      await page.keyboard.press('Control+k');
      const dialog = page.getByRole('dialog', { name: /^(Find|查找)$/, exact: true });
      await expect(dialog).toBeVisible();
      const field = dialog.getByRole('combobox');
      await expect(field).toBeFocused(); await field.fill('Acme');
      await expect(field).toHaveValue('Acme');
      // Search is usable before bootstrap answers, rather than merely accepting text into a waiting box.
      await expect(dialog.getByRole('option', { name: /Acme/ }).first()).toBeVisible();
      expect(reads).toContain('/search/names');
      expect(reads.filter((path) => ['/orgs', '/me/state', '/me/favorites', '/me/admin-mode', '/me/chat-states'].includes(path))).toEqual([]);
      expect(reads.filter((path) => path === '/me/bootstrap')).toHaveLength(1);
      const original = await field.elementHandle();
      release?.();
      await expect(page.locator('[data-workbench-chrome]')).toBeAttached();
      expect(await originalChrome.evaluate((element) => element.isConnected)).toBe(true);
      await expect(dialog.getByRole('option', { name: /Acme/ }).first()).toBeVisible();
      expect(await original.evaluate((element) => element.isConnected)).toBe(true);
      await expect(field).toBeFocused(); await expect(field).toHaveValue('Acme');
      expect(reads.filter((path) => path === '/me/bootstrap')).toHaveLength(1);
      // The full frame's later shortcut registration must not add a second dispatcher/toggle.
      await page.keyboard.press('Control+k'); await expect(dialog).toHaveCount(0);
      await page.keyboard.press('Control+k'); await expect(dialog).toHaveCount(1);
      await page.keyboard.press('Escape'); await expect(dialog).toHaveCount(0);
    } finally { release?.(); }
  });
}

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`a failed bootstrap keeps navigation and retries only on request at ${width}/${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    let attempts = 0;
    let release: (() => void) | undefined;
    const held = new Promise<void>((resolve) => { release = resolve; });
    const reads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && path.startsWith('/api/v1/')) reads.push(path.slice('/api/v1'.length));
    });
    await page.route('**/api/v1/me/bootstrap', async (route) => {
      attempts += 1;
      if (attempts === 1) {
        await route.fulfill({ status: 503, contentType: 'application/problem+json', json: {
          type: 'orch:error:internal', code: 'internal', title: 'Workspace read failed', status: 503,
          detail: 'The workspace could not be read.', request_id: 'bootstrap-recovery',
        } });
      } else { await held; await route.continue(); }
    });
    try {
      await page.goto(`/login?redirect=${encodeURIComponent(`/topic/${ACME.fixLogin}`)}`);
      await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
      await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
      await page.getByRole('button', { name: 'Sign in', exact: true }).click();
      const retry = page.locator('[data-main-area]').getByRole('button', { name: 'Try again', exact: true });
      await expect(retry).toBeVisible();
      const chrome = page.locator('[data-workbench-chrome]');
      await expect(chrome).toBeVisible();
      const originalChrome = await chrome.elementHandle();
      if (width === 1440) await expect(page.locator('[data-navigation-rail]')).toBeVisible();
      else await expect(page.locator('[data-narrow-containers]')).toBeVisible();
      await page.screenshot({ path: info.outputPath('bootstrap-failed.png'), fullPage: true, animations: 'disabled' });
      await page.keyboard.press('Control+k');
      const dialog = page.getByRole('dialog', { name: 'Find', exact: true });
      await expect(dialog).toBeVisible();
      await dialog.getByRole('combobox').fill('Acme');
      await expect(dialog.getByRole('option', { name: /Acme/ }).first()).toBeVisible();
      await page.keyboard.press('Escape');
      expect(attempts).toBe(1);
      expect(reads.filter((path) => /^\/(?:orgs|topics|projects|threads|documents|environments|providers|agents)(?:\/|$)/.test(path))).toEqual([]);
      expect(reads.filter((path) => path.startsWith('/me/') && path !== '/me/bootstrap')).toEqual([]);
      await retry.click();
      await expect.poll(() => attempts).toBe(2);
      await expect(page.locator('[data-main-area]').getByRole('status', { name: 'Loading…', exact: true })).toBeVisible();
      expect(await originalChrome.evaluate((element) => element.isConnected)).toBe(true);
      release?.();
      await expect(page.getByRole('heading', { name: 'Fix login', exact: true })).toBeVisible();
      expect(await originalChrome.evaluate((element) => element.isConnected)).toBe(true);
      expect(attempts).toBe(2);
      await expect(retry).toHaveCount(0);
    } finally { release?.(); }
  });
}
