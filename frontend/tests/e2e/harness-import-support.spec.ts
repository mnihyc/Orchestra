/** Unavailable native history stays labelled in the shipping build and opens its localized guide. */
import { createInstance } from 'i18next';
import { expect, test } from '@playwright/test';
import type { HarnessBuild } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { signInForManagement } from './admin-mode-support';
import { PEOPLE, scenario } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const REASON = 'The exported archive depends on its original home; this build cannot import that archive into a fresh home';

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`unavailable native history stays off at ${width}px in ${theme}/${locale}`, async ({ page, request }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signInForManagement(page);
    const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
    expect(login.ok()).toBe(true);
    const { access_token: token } = await login.json() as { access_token: string };
    const saved = await request.patch(`${DOUBLE}/api/v1/me`, {
      headers: { Authorization: `Bearer ${token}` }, data: { locale, theme },
    });
    expect(saved.ok()).toBe(true);
    const i18n = createInstance();
    await i18n.init({ lng: locale, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
    const t = i18n.getFixedT(locale);
    const build: HarnessBuild = {
      kind: 'codex', version: '0.170.0', os: 'linux', arch: 'x86_64',
      url: 'https://builds.example/codex', sha256: 'c'.repeat(64), is_default: true,
      source: 'release', status: 'checked', check: {
        at: '2026-01-01T00:00:00.000Z', machine: 'Check host', missing: [], off: ['import_native'],
        error: null, stage: 'complete', cases: [{ name: 'history.native_import', feature: 'import_native',
          passed: true, error: null, unsupported: REASON, elapsed_ms: 12 }],
      },
    };
    let reads = 0;
    await page.route((url) => url.pathname === '/api/v1/admin/programs/codex/builds', async (route) => {
      expect(route.request().method()).toBe('GET');
      reads += 1;
      await route.fulfill({ json: { items: [build], next_cursor: null } });
    });
    await page.goto('/admin/harnesses');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.getByRole('table', { name: t('programList.title'), exact: true }).getByRole('link', { name: 'Codex', exact: true }).click();
    const table = page.getByRole('table', { name: t('admin.harnesses.buildsTitle'), exact: true });
    const row = table.getByRole('row', { name: /0\.170\.0/ });
    const off = t('programVersions.offInBuild', { feature: t('programVersions.disabledFeatures.nativeImport') });
    const trigger = row.getByRole('button', { name: off, exact: true });
    await trigger.scrollIntoViewIfNeeded();
    await trigger.focus();
    await page.keyboard.press('Enter');
    const card = page.getByRole('dialog', { name: t('admin.harnesses.checkResults'), exact: true });
    await expect(card).toBeVisible();
    await expect(card).toContainText(`${t('programVersions.checks.nativeImport')} · ${locale === 'en' ? 'Unavailable' : '不可用'}`);
    await expect(card).toContainText(REASON);
    await expect(card).toContainText(off);
    await expect(card).not.toContainText('history.native_import');
    await expect(card).not.toContainText('import_native');
    await expect(card).not.toContainText(`${t('programVersions.checks.nativeImport')} · ${t('admin.harnesses.status.checked')}`);
    const geometry = await card.evaluate((element) => {
      const box = element.getBoundingClientRect();
      return { left: box.left, right: box.right, bottom: box.bottom, width: innerWidth, height: innerHeight,
        content: element.scrollWidth, room: element.clientWidth };
    });
    expect(geometry.left).toBeGreaterThanOrEqual(-1);
    expect(geometry.right).toBeLessThanOrEqual(geometry.width + 1);
    expect(geometry.bottom).toBeLessThanOrEqual(geometry.height + 1);
    expect(geometry.content).toBeLessThanOrEqual(geometry.room + 1);
    expect(reads).toBe(1);
    await page.screenshot({ path: info.outputPath(`unavailable-history-${width}-${theme}-${locale}.png`), caret: 'hide' });
    await info.attach('result-card-geometry', { body: JSON.stringify(geometry), contentType: 'application/json' });
    await page.keyboard.press('Escape');
    await expect(card).toBeHidden();
    await trigger.click();
    await card.getByRole('button', { name: locale === 'en' ? 'History support' : '历史支持', exact: true }).click();
    await expect(page.getByRole('heading', { name: locale === 'en' ? 'Read the method' : '查看传递方式', exact: true })).toBeVisible();
    expect(reads).toBe(1);
  });
}
