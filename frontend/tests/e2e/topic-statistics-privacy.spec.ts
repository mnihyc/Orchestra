/** Native ACL/count decisions are covered by topic_count_privacy.rs. This walk checks
 * that the real bilingual project/topic screens present the reader-scoped answer. */
import { expect, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';
import { chats } from './chat-support.ts';

const WORDS = {
  en: { topics: 'Topics', running: '1 running', approval: '1 approval', blocked: '1 blocked' },
  'zh-CN': { topics: '话题', running: '1 个运行中', approval: '1 个待批准', blocked: '1 个受阻' },
} as const;
const VISIBLE_ACTIVITY = '2025-01-01T10:00:00Z';

/** A recorded reader-scoped topic answer, not an implementation of native permissions. */
function scopedStatistics(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(scopedStatistics);
  if (typeof value !== 'object' || value === null) return value;
  const fields = Object.fromEntries(Object.entries(value).map(([key, item]) => [key, scopedStatistics(item)]));
  if (fields.id !== ACME.upgradeDeps || fields.kind !== 'topic') return fields;
  return { ...fields, thread_count: 1, attention: { running: 0, waiting_approval: 0, blocked: 0 },
    work_attention: { uncommitted: 0, unpushed: 0, failed: 0 }, thread_display_state: null, last_activity_at: VISIBLE_ACTIVITY };
}

for (const locale of ['en', 'zh-CN'] as const) for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`reader-scoped topic activity stays private through project and topic clicks in ${locale} at ${width}px/${theme}`, async ({ page }, info) => {
    await scenario();
    await chats('/locale', { as: 'ada', locale });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    for (const path of [`**/api/v1/projects/${ACME.web}/topics**`, `**/api/v1/nodes/${ACME.upgradeDeps}**`]) {
      await page.route(path, async (route) => {
        if (route.request().method() !== 'GET' || new URL(route.request().url()).pathname.endsWith('/activity')) return route.continue();
        const response = await route.fetch();
        if (!response.ok() || !response.headers()['content-type']?.includes('application/json')) return route.fulfill({ response });
        const body: unknown = await response.json();
        await route.fulfill({ response, json: scopedStatistics(body) });
      });
    }
    const threadHeaders: string[] = [];
    page.on('request', (request) => {
      if (/\/api\/v1\/threads\/[^/]+$/.test(new URL(request.url()).pathname)) threadHeaders.push(request.url());
    });
    await signIn(page);
    await page.goto('/org/acme');
    await page.getByRole('link', { name: 'Web', exact: true }).first().click();
    const topics = page.getByRole('table', { name: WORDS[locale].topics, exact: true });
    const row = topics.getByRole('row').filter({ has: page.getByRole('link', { name: 'Upgrade deps', exact: true }) });
    await expect(row).toBeVisible();
    await expect(row.getByText(WORDS[locale].running, { exact: true })).toHaveCount(0);
    await expect(row.getByText(WORDS[locale].approval, { exact: true })).toHaveCount(0);
    await expect(row.getByText(WORDS[locale].blocked, { exact: true })).toHaveCount(0);
    await expect(row.locator(`time[datetime="${VISIBLE_ACTIVITY}"]`).first()).toBeVisible();
    expect(threadHeaders).toEqual([]);
    await row.getByRole('link', { name: 'Upgrade deps', exact: true }).click();
    await expect(page.getByRole('heading', { name: 'Upgrade deps', exact: true })).toBeVisible();
    await page.getByRole('link', { name: 'Web', exact: true }).first().click();
    await expect(topics.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
    await info.attach(`topic-reader-statistics-${locale}-${width}-${theme}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
  });
}
