/** Each page owns its sections beside global navigation, and narrow chrome names every container above its sheet. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';
import { signInForManagement } from './admin-mode-support.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

/** A persistent workspace keeps its previous layout until the person chooses a thread arrangement. */
async function conversationArrangement(page: Page, locale: 'en' | 'zh-CN') {
  const words = locale === 'en' ? english : chinese;
  await page.getByRole('button', { name: words.workbench.arrangements.label, exact: true }).click();
  await page.getByRole('menuitem', { name: words.workbench.arrangements.conversation.title, exact: true }).click();
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
  test(`each area's sections keep a remembered org in Navigation at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signInForManagement(page); await language(page, locale);
    await page.goto('/org/acme'); await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
    const words = locale === 'en' ? english : chinese;
    for (const [path, section] of [
      ['/settings/profile', 'security'],
      ['/admin/overview', 'settings'],
      ['/help', 'keys'],
      [`/usage?scope=node:${ACME.org}&range=last_week`, 'performance'],
    ] as const) {
      await page.goto(path);
      if (width === 400) {
        const areaControl = page.locator('[data-container-choice="left:navigation"]');
        await expect(areaControl).toHaveAccessibleName(words.views.navigation.title);
        await areaControl.press('Enter');
      }
      const left = page.locator('[data-workbench-bar="left"]:visible');
      // Sections now belong to the page. Global Navigation keeps the org tree and its readers.
      await expect(left.getByRole('tree')).toBeVisible();
      await expect(left.locator('[data-section-nav]')).toHaveCount(0);
      if (width === 400) await expect(left.getByRole('combobox', { name: words.navigation.orgs, exact: true })).toHaveValue('Acme');
      else await expect(left.getByRole('navigation', { name: words.views.navigation.title, exact: true }).getByRole('link', { name: 'Acme', exact: true })).toBeVisible();
      if (width === 400) await left.getByRole('button', { name: words.common.close, exact: true }).press('Enter');
      const main = page.locator('[data-main-area]');
      if (path.startsWith('/settings') || path.startsWith('/admin')) {
        const nav = main.locator('[data-settings-frame] [data-section-nav]');
        await expect(nav).toBeVisible();
        await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);
        const link = nav.locator(`[data-section-id="${section}"]`);
        const destination = await link.getAttribute('href');
        if (destination === null) throw new Error('The section must have a real destination');
        const picker = nav.getByRole('combobox');
        if (await picker.isVisible()) {
          const label = (await link.textContent())?.trim();
          if (label === undefined || label === '') throw new Error('The section must have a name');
          await picker.click();
          await picker.fill(label);
          await expect(page.getByRole('option', { name: label, exact: true })).toBeVisible();
          await picker.press('Enter');
        } else await link.press('Enter');
        await expect(page).toHaveURL(new URL(destination, page.url()).href);
        await expect(nav.locator(`[data-section-id="${section}"]`)).toHaveAttribute('aria-current', 'page');
      } else {
        const tab = main.getByRole('tab', { name: section === 'keys' ? words.help.parts.keys : words.usagePerf.viewNames.performance, exact: true });
        await tab.press('Enter');
        await expect(tab).toHaveAttribute('aria-selected', 'true');
        const url = new URL(page.url());
        expect(url.searchParams.get(section === 'keys' ? 'part' : 'view')).toBe(section);
        if (path.startsWith('/usage')) {
          expect(url.searchParams.get('scope')).toBe(`node:${ACME.org}`);
          expect(url.searchParams.get('range')).toBe('last_week');
        }
      }
      await expect(page.locator('[data-workbench-chrome]').getByRole('button', { name: /^(More|Details|更多|详情)$/ })).toHaveCount(0);
    }
    if (width === 400) await page.locator('[data-navigation-rail]').getByRole('link', { name: /^(Home|主页)$/ }).click();
    else await page.getByRole('navigation', { name: /^(Orgs and your own places|组织和个人入口)$/ }).locator('a[href="/org/acme"]').click();
    await expect(page).toHaveURL(/\/org\/acme\/?(\?list=[^&]*&_list=true)?$/);
    await expect(page.getByRole('heading', { name: 'Acme', exact: true, level: 1 })).toBeVisible();
  });

  test(`container names and keyboard remain above sheets at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale); await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.getByRole('heading', { name: 'Main thread', exact: true })).toBeVisible();
    await conversationArrangement(page, locale);
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
  await conversationArrangement(page, 'en');
  const strip = page.locator('[data-narrow-containers]');
  await expect(strip.getByRole('button')).toHaveCount(8);
  await insideStrip(strip);
  const size = await strip.boundingBox();
  if (size === null) throw new Error('The phone view strip is missing');
  expect(size.height).toBeLessThanOrEqual(45);
  await expect(strip.getByRole('button', { name: /^(More|更多)$/ })).toHaveCount(0);
});
