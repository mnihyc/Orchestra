/** Actual browser geometry for marked names, org marks, and complete words on the sign-in tiles. */
import { expect, test } from '@playwright/test';
import type { EverythingRowPage, Machine, MeBootstrapReply, Org } from '../../src/generated/api-types.ts';
import { scenario, signIn } from './support.ts';
import { signInForManagement } from './admin-mode-support.ts';

test.beforeEach(async () => { await scenario(); });

test('a marked machine keeps its whole short name at 1440 and 400 pixels', async ({ page }) => {
  // These are presentation projections of full fixture rows, not a second authority evaluator.
  const marked = (machine: Machine): Machine => ({ ...machine, name: 'alice-laptop', access: { ...machine.access!,
    permissions: machine.access?.permissions ?? [], administers: machine.access?.administers ?? false, admin_only: [], participating: false, admin_access: { place: 'system', private: false } } });
  // Resource lists now read the native inventory; direct reads retain the same presentation fixture.
  await page.route(/\/api\/v1\/everything(?:\?|$)/, async (route) => {
    const response = await route.fetch();
    const body = await response.json() as EverythingRowPage;
    const first = body.items.find((row) => row.resource?.kind === 'machine');
    if (first === undefined) throw new Error('The scenario needs a complete machine fixture');
    await route.fulfill({ response, json: { ...body, items: body.items.map((row) => row.id === first.id && row.resource?.kind === 'machine'
      ? { ...row, name: 'alice-laptop', resource: { kind: 'machine', value: marked(row.resource.value) } } : row) } });
  });
  await page.route(/\/api\/v1\/machines(?:\?|$)/, async (route) => {
    const response = await route.fetch();
    const body: unknown = await response.json();
    if (!Array.isArray(body)) throw new Error('Expected the complete machines list');
    const machine = (body as Machine[])[0];
    if (machine === undefined) throw new Error('The scenario needs a complete machine fixture');
    await route.fulfill({ json: [marked(machine)] });
  });
  await signInForManagement(page);
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
  const names = (rows: Org[]): Org[] => {
    const own = rows.find((org) => org.org_kind === 'personal');
    const team = rows.find((org) => org.org_kind !== 'personal');
    if (own === undefined || team === undefined) throw new Error('The scenario needs a personal space and a team');
    return [{ ...own, name: 'Ben Barker', color: 4 }, { ...team, name: 'Beta', color: 4 }];
  };
  // The shell receives its orgs in the joined bootstrap rather than a second org-list request.
  await page.route(/\/api\/v1\/me\/bootstrap(?:\?|$)/, async (route) => {
    const response = await route.fetch();
    const body = await response.json() as MeBootstrapReply;
    await route.fulfill({ response, json: { ...body, orgs: names(body.orgs) } });
  });
  await page.route(/\/api\/v1\/orgs(?:\?|$)/, async (route) => {
    const response = await route.fetch();
    const body: unknown = await response.json();
    if (!Array.isArray(body)) throw new Error('Expected the complete org list');
    await route.fulfill({ json: names(body as Org[]) });
  });
  await signIn(page);
  const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
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
