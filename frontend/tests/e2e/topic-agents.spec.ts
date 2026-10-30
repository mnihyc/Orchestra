/** R1b S25/S27: real topic agent/default navigation and Settings identity in both languages, widths and themes. */
import { expect, test } from '@playwright/test';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, expectAccessible, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const CONTROL_TIMEOUT_MS = 10_000;
test.beforeEach(async () => { await scenario(); });

for (const locale of ['en', 'zh-CN'] as const) {
  test(`Agents lists real usable profiles and Settings names the page, not its General section (${locale})`, async ({ page }) => {
    await signIn(page);
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ locale, theme: 'system' }), signal: AbortSignal.timeout(CONTROL_TIMEOUT_MS) });
    expect(appearance.ok).toBe(true);
    const reads: string[] = []; const writes: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET') reads.push(path);
      else writes.push(path);
    });
    await page.goto(`/topic/${ACME.fixLogin}?tab=activity`);
    const main = page.getByTestId('main');
    await expect(main.getByRole('heading', { name: 'Fix login', level: 1, exact: true })).toBeVisible();
    expect(reads.filter((path) => path.endsWith('/binding/options'))).toEqual([]);
    const optionsReply = page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname.endsWith(`/nodes/${ACME.fixLogin}/binding/options`));
    await main.getByRole('tab', { name: locale === 'en' ? 'Agents' : '代理', exact: true }).click();
    expect((await optionsReply).ok()).toBe(true);
    const panel = main.locator('[data-topic-agents]');
    await expect(panel).toBeVisible();
    const codex = panel.getByRole('link', { name: 'Codex', exact: true });
    await expect(codex).toHaveCount(1);
    await expect(codex).toHaveAttribute('href', `/resources/agents/${RUNS_ON.codex}`);
    await expect(panel.getByRole('link', { name: 'Claude Code', exact: true })).toHaveCount(0);
    await expect(panel.getByText(locale === 'en' ? 'Default agent' : '默认代理', { exact: true })).toHaveCount(1);
    await expect(panel.getByText(locale === 'en' ? 'Default provider' : '默认提供方', { exact: true })).toHaveCount(1);
    await expect(panel.getByRole('link', { name: 'OpenAI key', exact: true })).toHaveAttribute('href', `/resources/providers/${RUNS_ON.openAiKey}`);
    await expect(panel.getByText('GPT-5.4', { exact: true })).toBeVisible();
    await expect(panel).not.toContainText('harness_unreported');
    await expect(panel).not.toContainText('gpt-5.4');
    expect(reads.filter((path) => path.endsWith('/binding/options'))).toHaveLength(1);
    expect(reads.filter((path) => path.includes('/agent-shares'))).toEqual([]);
    expect(writes.filter((path) => path.includes('/settings/agent.'))).toEqual([]);

    for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
      await page.setViewportSize({ width, height: 900 });
      await expectAccessible(page, theme, '[data-topic-agents]');
      const geometry = await panel.evaluate((element) => {
        const box = element.getBoundingClientRect();
        return { fits: box.right <= window.innerWidth && element.scrollWidth <= element.clientWidth + 1,
          offsets: Array.from(element.querySelectorAll('li')).flatMap((row) => {
            const icon = row.querySelector('svg')?.getBoundingClientRect(); const link = row.querySelector('a')?.getBoundingClientRect();
            return icon === undefined || link === undefined ? [] : [Math.abs(icon.top + icon.height / 2 - link.top - 12)];
          }) };
      });
      expect(geometry.fits).toBe(true);
      for (const offset of geometry.offsets) expect(offset).toBeLessThan(1);
      await page.screenshot({ path: test.info().outputPath(`topic-agents-${locale}-${width}-${theme}.png`), fullPage: true });
    }

    const defaults = panel.getByRole('link', { name: locale === 'en' ? 'Defaults' : '默认设置', exact: true });
    await defaults.focus(); await page.keyboard.press('Enter');
    const settingsName = locale === 'en' ? 'Settings' : '设置';
    await expect(page.getByRole('tab', { name: settingsName, exact: true })).toHaveCount(1);
    await expect(main.getByRole('heading', { level: 1, name: locale === 'en' ? 'Defaults' : '默认设置', exact: true })).toBeVisible();
    await page.locator('[data-section-id="general"]:visible').click();
    const heading = main.getByRole('heading', { level: 1, name: locale === 'en' ? 'General' : '常规', exact: true });
    await expect(heading).toBeVisible();
    await expect(page.getByRole('tab', { name: settingsName, exact: true })).toHaveCount(1);
    await expect(page.getByRole('tab', { name: locale === 'en' ? 'General' : '常规', exact: true })).toHaveCount(0);
    const path = page.locator('[data-screen-path]:visible');
    const topicLink = path.getByRole('link', { name: 'Fix login', exact: true });
    await expect(topicLink).toHaveAttribute('href', `/topic/${ACME.fixLogin}?tab=overview`);
    await expect(path.locator('li').last()).toContainText('Fix login');
    await expect(path.getByText(settingsName, { exact: true })).toHaveCount(0);
    for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
      await page.setViewportSize({ width, height: 900 });
      await expectAccessible(page, theme, '[data-settings-frame]');
      await page.screenshot({ path: test.info().outputPath(`topic-settings-${locale}-${width}-${theme}.png`), fullPage: true });
    }
    await topicLink.focus(); await page.keyboard.press('Enter');
    await expect(main.getByRole('tab', { name: locale === 'en' ? 'Overview' : '概览', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}\\?tab=overview`));
  });
}
