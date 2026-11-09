/** The shared source selector keeps its supplied cost exclusions in both page languages. */
import { expect, test } from '@playwright/test';
import type { SourceDescriptor } from '../../src/generated/api-types.ts';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { scenario } from './support.ts';

const summary = 'Reads organization costs and optional spend limits with a separate admin key. Amounts remain in their reported minor units; Priority Tier costs are not included. No prepaid balance is offered.';
const chinese = '使用单独的管理密钥读取组织费用与可用的支出限额；金额保留提供方报告的最小货币单位，不包含 Priority Tier 费用，也不提供预付余额。';
const descriptor: SourceDescriptor = {
  name: 'anthropic', title: 'Anthropic', contract: 1, summary,
  summary_locales: { en: summary, 'zh-CN': chinese },
  facts: { reports: ['identity', 'spend', 'windows'], credential: 'none', spends_resets: false, min_interval_secs: null, plans: {},
    admin_credential: { labels: { en: 'Admin key', 'zh-CN': '管理密钥' }, reports: ['identity', 'spend', 'windows'] } },
  models: null, presents: null,
  fields: [{ name: 'admin_key', label: 'Admin key', type: 'secret', required: false, help: null, choices: null, default: null, max_bytes: null }],
  docs_url: 'https://platform.claude.com/docs/en/manage-claude/usage-cost-api',
};
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`provider cost caveat survives opening its source ${width}/${theme}/${locale}`, async ({ page }) => {
    await scenario(); await signInScreenRoute(page, { width, theme, locale });
    const path = `/api/v1/providers/${RUNS_ON.openAiKey}`, writes: string[] = [];
    await page.route(`**${path}`, async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      const response = await route.fetch(); expect(response.ok()).toBe(true);
      const body: unknown = await response.json();
      if (!record(body) || !record(body.sources) || !record(body.sources.facts) || body.sources.facts.by !== 'default') throw new Error('The source view fixture needs its native default-source provider.');
      // Only the view's source declaration changes; native access and account facts remain intact.
      body.sources.facts.descriptor = descriptor; body.sources.facts.name = descriptor.name;
      await route.fulfill({ response, json: body });
    });
    page.on('request', (request) => {
      if (request.method() === 'PUT' && new URL(request.url()).pathname === `${path}/sources`) writes.push(request.method());
    });
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=settings`);
    const words = locale === 'en' ? summary : chinese;
    const caveat = page.getByText(words, { exact: true }); await expect(caveat).toBeVisible();
    const box = await caveat.boundingBox(); expect(box).not.toBeNull();
    if (box !== null) { expect(box.x).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual(width); }
    const label = caption('@providerSources.detailsFrom', locale);
    const group = page.getByRole('group', { name: label, exact: true });
    await group.getByRole('combobox', { name: label, exact: true }).click();
    await expect(page.getByRole('listbox').getByRole('option').filter({ hasText: words })).toBeVisible();
    await page.keyboard.press('Escape'); await expect(caveat).toBeVisible();
    expect(writes).toEqual([]);
  });
}
