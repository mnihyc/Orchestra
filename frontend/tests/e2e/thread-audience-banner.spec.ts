/** Own bilingual click path through the one audience banner, people card and existing sharing. */
import { expect, test } from '@playwright/test';
import type { Entry, PermissionPage, ResolvedSetting, UserRef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

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
    const audience = { reads: 0, extra: false, personal: true, release: null as (() => void) | null };
    const waiting = new Promise<void>((resolve) => { audience.release = resolve; });
    await page.route(`**/api/v1/nodes/${ACME.mainThread}/settings`, async (route) => {
      if (route.request().method() !== 'GET') { await route.fallback(); return; }
      const response = await route.fetch(); expect(response.status()).toBe(200);
      const native = await response.json() as ResolvedSetting[];
      const option: ResolvedSetting = { key: 'orch_tools.owner_reads', value: audience.personal, own: null, writable: false, locked_by: null, source: { scope: 'org', node: null }, parts: [] };
      await route.fulfill({ json: [...native.filter((row) => row.key !== option.key), option] });
    });
    await page.route(`**/api/v1/nodes/${ACME.mainThread}/permissions`, async (route) => {
      if (route.request().method() !== 'GET') { await route.fallback(); return; }
      audience.reads += 1;
      const response = await route.fetch(); expect(response.status()).toBe(200);
      const native = await response.json() as PermissionPage;
      const entry: Entry = { id: '00000000-0000-4000-8009-000000000090', holder: { kind: 'user', user: reader }, options: {}, outside: false, read_first: false,
        expires_at: null, note: '', source: 'page', created_by: native.owner, created_at: '2026-11-09T00:00:00Z', revision: 1 };
      const extra: Entry = { ...entry, id: '00000000-0000-4000-8009-000000000091', holder: { kind: 'agents', node: { id: ACME.mainThread, kind: 'thread', name: native.object.name, redacted: false } } };
      // These page snapshots isolate presentation. Real grants, tool delegation and confirmation are
      // decided by the adjacent Rust cases; this browser path makes no authority or write claim.
      const shown: PermissionPage = { ...native, state: 'custom', owners: native.owners.filter((one) => one.holder.kind === 'user' && one.holder.user.id === native.owner?.id),
        rows: native.rows.map((row) => ({ ...row, inherit: false, inherited: null, locked: null, entries: row.permission === 'thread.view' ? [entry, ...(audience.extra ? [extra] : [])] : [] })) };
      await waiting;
      await route.fulfill({ json: shown });
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.locator('html').evaluate((element, value) => { element.setAttribute('data-theme', value); }, theme);
    await expect.poll(() => audience.reads).toBe(1);
    await expect(page.getByText(words.thread.audience.loading, { exact: true })).toHaveCount(0);
    await expect(page.locator('[data-thread-audience]')).toHaveCount(0);
    audience.release?.();
    const banner = page.getByRole('status').filter({ has: page.locator('[data-thread-audience]') });
    await expect(banner).toBeVisible();
    await expect(banner).toContainText(words.thread.audience.personal);
    const line = page.locator('[data-thread-audience]');
    const geometry = await line.evaluate((element) => ({ height: Math.ceil(element.getBoundingClientRect().height), line: Math.ceil(parseFloat(getComputedStyle(element).lineHeight)) }));
    expect(geometry.height).toBeLessThanOrEqual(geometry.line);
    const peopleCount = locale === 'en' ? '1 person' : '1 人';
    await banner.getByRole('button', { name: words.thread.audience.shared.replace('{{audience}}', peopleCount), exact: true }).focus();
    await page.keyboard.press('Enter');
    const people = page.getByRole('dialog', { name: words.thread.audience.title, exact: true });
    await expect(people.getByText(reader.display_name, { exact: true })).toBeVisible();
    await expect(page.locator('[data-overflow-tooltip]')).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(people).toHaveCount(0);
    await banner.getByRole('button', { name: words.thread.audience.sharing, exact: true }).click();
    const sharing = page.getByRole('dialog').filter({ has: page.getByRole('combobox', { name: words.access.add.label, exact: true }) });
    await expect(sharing).toBeVisible();
    const overflow = await page.locator('[data-screen="thread"]').evaluate((element) => element.scrollWidth > element.clientWidth);
    expect(overflow).toBe(false);
    await page.keyboard.press('Escape');
    await banner.getByRole('button', { name: words.common.dismiss, exact: true }).click();
    await expect(line).toHaveCount(0);
    const beforeReload = audience.reads;
    await page.reload();
    await page.locator('html').evaluate((element, value) => { element.setAttribute('data-theme', value); }, theme);
    await expect.poll(() => audience.reads).toBeGreaterThan(beforeReload);
    await page.locator('[data-thread-header]').getByRole('button', { name: words.actions.share.label, exact: true }).click();
    await expect(sharing).toContainText(reader.display_name);
    await expect(line).toHaveCount(0);
    await page.keyboard.press('Escape');
    audience.extra = true; audience.personal = false;
    await page.reload();
    await page.locator('html').evaluate((element, value) => { element.setAttribute('data-theme', value); }, theme);
    await expect(banner).toBeVisible();
    await showWorkbenchView(page, 'Parameters');
    const inspector = page.getByTestId('personal-read-state');
    await expect(inspector).toContainText(words.thread.audience.reads.off);
    await expect(inspector).toContainText(words.thread.audience.reads.shared);
    await expect(banner).not.toContainText(words.thread.audience.personal);
    await inspector.getByRole('button', { name: words.thread.audience.reads.guide, exact: true }).click();
    await expect(page.getByRole('heading', { name: words.settingRegistry.orch_tools.owner_reads.title, exact: true, level: 1 })).toBeVisible();
    // EN: Thread (no pending flash) → audience count → person card → Sharing → Close → reload (stays closed) → new audience (returns) → Parameters (Off) → About personal reads.
    // 中文：会话（等待时不闪现提示条）→ 受众人数 → 人员卡片 → 共享 → 关闭 → 刷新（仍关闭）→ 新受众（再次出现）→ 参数（关闭）→ 了解个人读取。
    // Component cases additionally apply live native page add/sender/removal/expiry/lock changes.
  });
}
