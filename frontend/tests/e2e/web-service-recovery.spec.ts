/** Managed services share the recorded call-rule history and ordinary search cards. */
import { expect, test } from '@playwright/test';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { marker, toolCall } from '../doubles/thread-rows';
import { ACME, DOUBLE, emit, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`managed service recovery keeps its reason and real sources ${locale}/${theme}`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ locale, theme }), signal: AbortSignal.timeout(10_000),
    });
    expect(appearance.ok).toBe(true);
    const words = locale === 'en' ? en : zh;
    const recoveredSource = { title: 'Recovered source', url: 'https://source.example.invalid/recovered' };
    const output = { query: 'Recovered query', results: [recoveredSource] };
    await emit(ACME.mainThread, { items: [
      marker('service-chain-completed', { marker: 'call_recovery', data: {
        action: 'failover', rule: 'service-chain', kind: 'provider_unreachable', attempt: 1, automatic: true, input_id: 'service-input',
      } }),
      toolCall('recovered-web-search', { kind: 'web_search', action: { type: 'search' }, query: output.query, results: output.results },
        { title: 'Search the web: Recovered query', name: 'web_search', output }),
    ] });
    // EN: Open the thread, return from the organization tab, read the fallback reason, and expand its search result.
    // 中文：打开线程，从组织标签返回，查看回退原因，再展开对应的搜索结果。
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('[data-thread-header]')).toBeVisible();
    await page.goto('/org/acme');
    await page.getByRole('tab', { name: /Main thread/ }).first().click();
    const reason = words.calls.timeline.replace('{{action}}', words.calls.actions.failover)
      .replace('{{kind}}', words.calls.causes.provider_unreachable).replace('{{attempt}}', '1').replace('{{rule}}', 'service-chain');
    await expect(page.getByText(reason, { exact: true })).toBeVisible();
    const search = page.locator('[data-step="recovered-web-search"]');
    await search.getByRole('button', { name: /Recovered query/ }).click();
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(search.getByRole('link', { name: /Recovered source/ })).toHaveAttribute('href', recoveredSource.url);
      await expect(page.getByText(reason, { exact: true })).toBeVisible();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`web-recovery-${locale}-${theme}-${width}.png`), fullPage: true });
    }
    // EN: Open the raw result and verify the actual returned address, without a private provider destination.
    // 中文：打开原始结果，核对实际返回的网址，不显示私有供应商目的地。
    await search.getByRole('button', { name: words.timeline.step.showRaw, exact: true }).click();
    const result = `${words.timeline.raw.output}.results.0`;
    await search.getByRole('button', { name: words.ui.json.expand.replace('{{name}}', result), exact: true }).click();
    await expect(search).toContainText(recoveredSource.url);
  });
}
