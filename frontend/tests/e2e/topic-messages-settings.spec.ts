/** Explicit sharing superseded the automatic topic messaging setting. */
import { expect, type Page, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support';

const KEY = 'sharing.agent_messages';
const projectPath = `/project/${ACME.web}/settings?section=defaults&setting=${KEY}`;
const topicPath = `/topic/${ACME.fixLogin}?tab=settings&section=defaults&setting=${KEY}`;

async function expectNoEditor(page: Page): Promise<void> {
  const defaults = page.getByRole('region', { name: 'Defaults', exact: true });
  await expect(defaults.getByRole('searchbox', { name: 'Search the settings', exact: true })).toHaveValue(KEY);
  await expect(defaults.getByText('No setting matches.', { exact: true })).toBeVisible();
  await expect(defaults.locator(`[data-setting="${KEY}"]`)).toHaveCount(0);
}

test('the retired topic messaging setting has no editor at project or topic scope', async ({ page }) => {
  await scenario();
  await signIn(page);
  const writes: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (request.method() !== 'GET' && /^\/api\/v1\/nodes\/[^/]+\/settings\//.test(url.pathname)) writes.push(`${request.method()} ${url.pathname}`);
  });

  // Saved settings links still open Defaults, but this former automatic grant has no editable value.
  await page.goto(projectPath);
  await expectNoEditor(page);
  await page.goto(topicPath);
  await expectNoEditor(page);

  for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expectNoEditor(page);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    const path = test.info().outputPath(`topic-messages-${width}-${theme}.png`);
    await page.screenshot({ path, fullPage: true, animations: 'disabled' });
    await test.info().attach(`topic-messages-${width}-${theme}`, { path, contentType: 'image/png' });
  }
  expect(writes).toEqual([]);
});
