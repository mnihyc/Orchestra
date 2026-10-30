/** Actual browser geometry for marked names, org marks, and complete words on the sign-in tiles. */
import { expect, test } from '@playwright/test';
import type { Machine, Org } from '../../src/generated/api-types.ts';
import { scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

test('a marked machine keeps its whole short name at 1440 and 400 pixels', async ({ page }) => {
  // These are presentation projections of full fixture rows, not a second authority evaluator.
  await page.route('**/api/v1/me/admin-mode', (route) => route.fulfill({ json: { available: true, as_admin: true } }));
  await page.route(/\/api\/v1\/machines(?:\?|$)/, async (route) => {
    const response = await route.fetch();
    const body: unknown = await response.json();
    if (!Array.isArray(body)) throw new Error('Expected the complete machines list');
    const machine = (body as Machine[])[0];
    if (machine === undefined) throw new Error('The scenario needs a complete machine fixture');
    const admin_access = { place: 'system' as const, private: false };
    await route.fulfill({ json: [{ ...machine, name: 'alice-laptop', access: { ...machine.access,
      permissions: machine.access?.permissions ?? [], admin_only: [], participating: false, admin_access } }] });
  });
  await signIn(page);
  await page.goto('/resources/machines');
  const name = page.getByRole('link', { name: 'alice-laptop', exact: true });
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await expect(name).toBeVisible();
    const row = page.getByRole('row').filter({ has: name });
    await expect(row.getByRole('img', { name: 'You see this as a system admin', exact: true })).toBeVisible();
    await expect.poll(() => name.evaluate((element) => element.clientWidth > 0 && element.scrollWidth <= element.clientWidth)).toBe(true);
    await expect.poll(() => row.evaluate((element) => element.getBoundingClientRect().right <= window.innerWidth)).toBe(true);
  }
});

test('the actual org rail distinguishes a team from a personal space with the same initials and colour', async ({ page }) => {
  await page.route(/\/api\/v1\/orgs(?:\?|$)/, async (route) => {
    const response = await route.fetch();
    const body: unknown = await response.json();
    if (!Array.isArray(body)) throw new Error('Expected the complete org list');
    const rows = body as Org[];
    const own = rows.find((org) => org.org_kind === 'personal');
    const team = rows.find((org) => org.org_kind !== 'personal');
    if (own === undefined || team === undefined) throw new Error('The scenario needs a personal space and a team');
    await route.fulfill({ json: [{ ...own, name: 'Ben Barker', color: 4 }, { ...team, name: 'Beta', color: 4 }] });
  });
  await signIn(page);
  const rail = page.getByRole('navigation', { name: 'Navigation', exact: true });
  await expect(rail.getByRole('link', { name: /Ben Barker/ })).toContainText('Bn');
  await expect(rail.getByRole('link', { name: 'Beta', exact: true })).toContainText('Bt');
});

for (const locale of ['en', 'zh-CN'] as const) {
  test(`sign-in appearance choices keep whole words at 400 pixels in ${locale}`, async ({ page }) => {
    await page.setViewportSize({ width: 400, height: 900 });
    await page.addInitScript((language) => localStorage.setItem('orch.appearance', JSON.stringify({ locale: language, theme: 'system', density: 'comfortable' })), locale);
    await page.goto('/login');
    const theme = page.locator('footer').getByLabel(locale === 'en' ? 'Theme' : '主题', { exact: true });
    await expect(theme).toBeVisible();
    // Normal script line-breaking is allowed. An English word must not be split across tile lines.
    const broken = await theme.evaluate((element) => {
      const words: string[] = [];
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      let text = walker.nextNode();
      while (text !== null) {
        const contents = text.textContent ?? '';
        for (const match of contents.matchAll(/[A-Za-z]+(?:['’][A-Za-z]+)*/gu)) {
          const range = document.createRange();
          range.setStart(text, match.index);
          range.setEnd(text, match.index + match[0].length);
          if (range.getClientRects().length > 1) words.push(match[0]);
        }
        text = walker.nextNode();
      }
      return words;
    });
    expect(broken).toEqual([]);
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
