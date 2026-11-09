/** The existing inherited boolean row controls optional automatic messaging within a topic. */
import { expect, type Page, test } from '@playwright/test';
import type { ResolvedSetting } from '../../src/generated/api-types';
import { ACME, bodySent, scenario, signIn } from './support';

const KEY = 'sharing.agent_messages';
const TITLE = 'Messages within a topic';
const settingPath = (node: string) => `/api/v1/nodes/${node}/settings/${KEY}`;
const projectPath = `/project/${ACME.web}/settings?section=defaults&setting=${KEY}`;
const topicPath = `/topic/${ACME.fixLogin}?tab=settings&section=defaults&setting=${KEY}`;
const rowIn = (page: Page) => page.getByRole('main').locator(`[data-setting="${KEY}"]`);
const switchIn = (page: Page) => rowIn(page).getByRole('switch', { name: TITLE, exact: true });

async function openSetting(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await expect(switchIn(page)).toBeVisible();
}

async function setValue(page: Page, node: string, value: boolean): Promise<void> {
  const reply = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === settingPath(node));
  await switchIn(page).click();
  const response = await reply;
  expect(response.status()).toBe(200);
  expect(await bodySent(response.request())).toMatchObject({ value });
  const saved = await response.json() as ResolvedSetting;
  expect(saved).toMatchObject({ key: KEY, value, own: { value } });
  await expect(switchIn(page)).toBeChecked({ checked: value });
}

async function resetValue(page: Page, node: string, value: boolean): Promise<void> {
  await rowIn(page).getByRole('button', { name: `More about ${TITLE}`, exact: true }).click();
  const reply = page.waitForResponse((response) => response.request().method() === 'DELETE' && new URL(response.url()).pathname === settingPath(node));
  await page.getByRole('menuitem', { name: 'Reset to inherited', exact: true }).click();
  const response = await reply;
  expect(response.status()).toBe(200);
  expect(await response.json() as ResolvedSetting).toMatchObject({ key: KEY, value, own: null });
  await expect(switchIn(page)).toBeChecked({ checked: value });
}

test('topic messaging is Off by default, inherits an explicit project value and resets through the existing row', async ({ page }) => {
  await scenario();
  await signIn(page);
  const requests: string[] = [];
  const writes: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/v1/') && url.pathname !== '/api/v1/ws') requests.push(`${request.method()} ${url.pathname}`);
    if (request.method() !== 'GET' && /^\/api\/v1\/nodes\/[^/]+\/settings\//.test(url.pathname)) writes.push(`${request.method()} ${url.pathname}`);
  });

  await openSetting(page, projectPath);
  await expect(switchIn(page)).not.toBeChecked();
  await openSetting(page, topicPath);
  await expect(switchIn(page)).not.toBeChecked();
  expect(writes).toEqual([]);

  for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await rowIn(page).scrollIntoViewIfNeeded();
    await expect(switchIn(page)).not.toBeChecked();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
    const path = test.info().outputPath(`topic-messages-${width}-${theme}.png`);
    await page.screenshot({ path, fullPage: true, animations: 'disabled' });
    await test.info().attach(`topic-messages-${width}-${theme}`, { path, contentType: 'image/png' });
  }
  expect(writes).toEqual([]);

  await openSetting(page, projectPath);
  await setValue(page, ACME.web, true);
  await page.reload();
  await expect(switchIn(page)).toBeChecked();
  await openSetting(page, topicPath);
  await expect(switchIn(page)).toBeChecked();
  await expect(rowIn(page).getByRole('link', { name: 'Inherited from Web', exact: true })).toBeVisible();

  await setValue(page, ACME.fixLogin, false);
  await resetValue(page, ACME.fixLogin, true);
  await expect(rowIn(page).getByRole('link', { name: 'Inherited from Web', exact: true })).toBeVisible();
  await openSetting(page, projectPath);
  await expect(switchIn(page)).toBeChecked();
  await resetValue(page, ACME.web, false);
  await page.reload();
  await expect(switchIn(page)).not.toBeChecked();
  await openSetting(page, topicPath);
  await expect(switchIn(page)).not.toBeChecked();
  expect(writes).toEqual([
    `PUT ${settingPath(ACME.web)}`, `PUT ${settingPath(ACME.fixLogin)}`,
    `DELETE ${settingPath(ACME.fixLogin)}`, `DELETE ${settingPath(ACME.web)}`,
  ]);
  await test.info().attach('topic-messages-requests', { body: JSON.stringify({ count: requests.length, requests, writes }), contentType: 'application/json' });
});
