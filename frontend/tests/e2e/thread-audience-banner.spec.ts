/** Own bilingual click path through the one audience banner, people card and existing sharing. */
import { expect, test } from '@playwright/test';
import type { Entry, PermissionPage, UserRef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn } from './support';

const reader: UserRef = { id: '00000000-0000-4000-8001-000000000090', kind: 'human', username: 'grace', display_name: 'Grace Hopper', avatar: { text: 'GH', color: 2 } };

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1280]) for (const theme of ['light', 'dark'] as const) {
  test(`thread audience sharing path ${locale}/${width}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await scenario(); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.route(`**/api/v1/nodes/${ACME.mainThread}/permissions`, async (route) => {
      if (route.request().method() !== 'GET') { await route.fallback(); return; }
      const response = await route.fetch(); expect(response.status()).toBe(200);
      const native = await response.json() as PermissionPage;
      const entry: Entry = { id: '00000000-0000-4000-8009-000000000090', holder: { kind: 'user', user: reader }, options: {}, outside: false, read_first: false,
        expires_at: null, note: '', source: 'page', created_by: native.owner, created_at: '2026-11-09T00:00:00Z', revision: 1 };
      // These page snapshots isolate presentation. Real grants, tool delegation and confirmation are
      // decided by the adjacent Rust cases; this browser path makes no authority or write claim.
      const shown: PermissionPage = { ...native, state: 'custom', owners: native.owners.filter((one) => one.holder.kind === 'user' && one.holder.user.id === native.owner?.id),
        rows: native.rows.map((row) => ({ ...row, inherit: false, inherited: null, locked: null, entries: row.permission === 'thread.view' ? [entry] : [] })) };
      await route.fulfill({ json: shown });
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.locator('html').evaluate((element, value) => { element.setAttribute('data-theme', value); }, theme);
    const banner = page.getByRole('status').filter({ has: page.locator('[data-thread-audience]') });
    await expect(banner).toBeVisible();
    await expect(banner).toContainText(words.thread.audience.personal);
    const audience = locale === 'en' ? '1 person' : '1 人';
    await banner.getByRole('button', { name: words.thread.audience.shared.replace('{{audience}}', audience), exact: true }).focus();
    await page.keyboard.press('Enter');
    const people = page.getByRole('dialog', { name: words.thread.audience.title, exact: true });
    await expect(people.getByText(reader.display_name, { exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(people).toHaveCount(0);
    await banner.getByRole('button', { name: words.thread.audience.sharing, exact: true }).click();
    const sharing = page.getByRole('dialog').filter({ has: page.getByRole('combobox', { name: words.access.add.label, exact: true }) });
    await expect(sharing).toBeVisible();
    const overflow = await page.locator('[data-screen="thread"]').evaluate((element) => element.scrollWidth > element.clientWidth);
    expect(overflow).toBe(false);
    // EN: Thread → audience count → Grace’s existing person chip → Escape → Sharing.
    // 中文：会话 → 受众人数 → Grace 的现有人员标签 → Escape → 共享。
    // Component cases additionally apply live native page add/sender/removal/expiry/lock changes.
  });
}
