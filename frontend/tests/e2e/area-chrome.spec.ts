/** Global areas own their navigation, and narrow chrome exposes every container above its sheet. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

async function insideStrip(strip: Locator) {
  expect(await strip.evaluate((element) => {
    const box = element.getBoundingClientRect();
    const controls = element.getAttribute('data-container-presentation') === 'icons' ? 'button[data-container-choice]' : 'button[data-container-choice], [data-container-name]';
    return Array.from(element.querySelectorAll<HTMLElement>(controls)).every((child) => {
      const rect = child.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && rect.left >= box.left - 1 && rect.right <= box.right + 1
        && rect.top >= box.top - 1 && rect.bottom <= box.bottom + 1
        && child.scrollWidth <= child.clientWidth + 1 && child.scrollHeight <= child.clientHeight + 1;
    });
  })).toBe(true);
}

test.beforeEach(async () => { await scenario(); });

for (const width of [1100, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`each area's sections replace a remembered org at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    await page.goto('/org/acme'); await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
    const requests: string[] = [];
    const orgFeeds: string[] = [];
    page.on('request', (request) => { if (request.url().includes('/api/v1/')) requests.push(new URL(request.url()).pathname); });
    page.on('websocket', (socket) => socket.on('framesent', (event) => {
      try {
        const frame: unknown = JSON.parse(typeof event.payload === 'string' ? event.payload : event.payload.toString());
        if (typeof frame === 'object' && frame !== null && 'op' in frame && frame.op === 'sub' && 'feed' in frame && typeof frame.feed === 'string' && frame.feed.startsWith('org:')) orgFeeds.push(frame.feed);
      } catch { /* Non-JSON native transport payloads are not navigation subscriptions. */ }
    }));
    for (const [path, section, area] of [
      ['/settings/profile', 'security', locale === 'en' ? 'Settings' : '设置'],
      ['/admin/overview', 'settings', locale === 'en' ? 'Management' : '管理'],
      ['/help', 'keys', locale === 'en' ? 'Help' : '帮助'],
      [`/usage?scope=node:${ACME.org}&range=last_week`, 'performance', locale === 'en' ? 'Usage' : '用量'],
    ] as const) {
      requests.length = 0; orgFeeds.length = 0;
      await page.goto(path);
      if (width === 400) {
        const areaControl = page.locator('[data-container-choice="left:navigation"]');
        await expect(areaControl).toHaveAccessibleName(area); await areaControl.press('Enter');
      }
      const left = page.locator('[data-workbench-bar="left"]:visible');
      await expect(left.locator('[data-section-nav]')).toBeVisible();
      await expect(left.getByRole('tree')).toHaveCount(0);
      await expect(left.locator('[data-bar-scroll]')).toHaveCount(0);
      const nav = left.locator('[data-section-nav]');
      await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);
      const link = nav.locator(`[data-section-id="${section}"]`);
      await expect(link).toBeVisible();
      const destination = await link.getAttribute('href');
      if (destination === null) throw new Error('The section must have a real destination');
      const url = new URL(destination, page.url());
      await link.press('Enter');
      await expect(page).toHaveURL(url.href);
      if (path.startsWith('/usage')) {
        expect(url.searchParams.get('scope')).toBe(`node:${ACME.org}`); expect(url.searchParams.get('range')).toBe('last_week');
      }
      expect(requests.filter((request) => /\/orgs\/[^/]+\/(tree|chats)(?:\/|$)/.test(request))).toEqual([]);
      expect(orgFeeds).toEqual([]);
      await expect(page.locator('[data-workbench-chrome]').getByRole('button', { name: /^(More|Details|更多|详情)$/ })).toHaveCount(0);
    }
    if (width === 400) await page.locator('[data-navigation-rail]').getByRole('link', { name: /^(Home|主页)$/ }).click();
    else await page.getByRole('navigation', { name: /^(Orgs and your own places|组织和个人入口)$/ }).locator('a[href="/org/acme"]').click();
    await expect(page).toHaveURL(/\/org\/acme\/?$/);
    await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
  });

  test(`container names and keyboard remain above sheets at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale); await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
    const strip = page.locator('[data-narrow-containers]');
    const chrome = page.locator('[data-workbench-chrome]');
    if (width === 400) {
      await expect(strip.getByRole('button')).toHaveCount(8);
      const geometry = await strip.evaluate((element) => ({
        height: element.getBoundingClientRect().height,
        buttons: Array.from(element.querySelectorAll<HTMLButtonElement>('button[data-container-choice]')).map((button) => {
          const box = button.getBoundingClientRect(); return { top: box.top, width: box.width, height: box.height };
        }),
      }));
      expect(geometry.buttons).toHaveLength(8);
      // Eight 44px touch targets share one line; one extra pixel is the strip's border.
      expect(geometry.height).toBeLessThanOrEqual(45);
      const top = geometry.buttons[0]?.top;
      if (top === undefined) throw new Error('The phone view strip has no controls');
      for (const button of geometry.buttons) {
        expect(Math.abs(button.top - top)).toBeLessThanOrEqual(1);
        expect(button.width).toBeGreaterThanOrEqual(43); expect(button.height).toBe(44);
      }
      for (const button of await strip.getByRole('button').all()) {
        const name = await button.getAttribute('aria-label');
        if (name === null || name.trim() === '') throw new Error('Every phone view icon needs its name');
        await button.focus(); await expect(page.getByRole('tooltip')).toHaveText(name);
      }
    }
    for (const name of ['conversation', 'team', 'code', 'environment', 'agent']) {
      const choice = strip.locator(`[data-container-choice="right:${name}"]`);
      const words = choice.locator('[data-container-name]');
      if (width === 1100) await expect(words).toBeVisible();
      const title = (await words.innerText()).trim(); expect(title.length).toBeGreaterThan(0);
      expect(title).not.toContain('workbench.'); await expect(choice).toHaveAccessibleName(title);
    }
    await insideStrip(strip);
    await strip.locator('[data-container-choice="right:team"]').click();
    const sheet = page.getByRole('dialog').filter({ has: page.locator('[data-workbench-bar="right"]') });
    await expect(sheet).toBeVisible();
    await expect.poll(async () => {
      const bar = await chrome.boundingBox(), panel = await sheet.boundingBox();
      return bar !== null && panel !== null && panel.y >= bar.y + bar.height - 1;
    }).toBe(true);
    await expect(sheet.locator('[data-container-title] [role="tablist"]')).toHaveCount(0);
    await insideStrip(strip);
    const team = strip.locator('[data-container-choice="right:team"]');
    await team.focus(); await page.keyboard.press('ArrowRight');
    const code = strip.locator('[data-container-choice="right:code"]');
    await expect(code).toBeFocused(); await page.keyboard.press('Enter');
    await expect(code).toHaveAttribute('aria-pressed', 'true');
    await expect(strip.locator('[aria-pressed="true"]')).toHaveCount(1);
    await expect(page.locator('[data-workbench-bar="right"]:visible')).toHaveAttribute('data-active-container', 'code');
    await insideStrip(strip);
    await code.click(); await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(chrome.getByRole('button', { name: /^(More|Details|更多|详情)$/ })).toHaveCount(0);
  });
}

test('eight view icons also fit one row at 360 px without scrolling or a More menu', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 800 }); await page.emulateMedia({ colorScheme: 'dark' });
  await signIn(page); await page.goto(`/thread/${ACME.mainThread}`);
  const strip = page.locator('[data-narrow-containers]');
  await expect(strip.getByRole('button')).toHaveCount(8);
  await insideStrip(strip);
  const size = await strip.boundingBox();
  if (size === null) throw new Error('The phone view strip is missing');
  expect(size.height).toBeLessThanOrEqual(45);
  await expect(strip.getByRole('button', { name: /^(More|更多)$/ })).toHaveCount(0);
});
