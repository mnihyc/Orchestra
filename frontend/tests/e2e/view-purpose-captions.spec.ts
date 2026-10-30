/** Person-named pages keep their purpose and address beside an unrelated owner chip. */
import { expect, test } from '@playwright/test';
import { id } from '../doubles/fixtures.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { resourceFixtures } from '../../e2e/layout-fixtures.ts';
import { inspectLayout } from '../../e2e/layout-rules-dom.ts';
import { duplicateAccessibleNames, LAYOUT_INPUT, screenReady } from '../../e2e/layout-rules-helpers.ts';
import { asAdmin } from './admin-mode-support.ts';
import { caption } from '../../e2e/capture/open.ts';
import { replySent, scenario, signIn } from './support';

for (const width of [400, 1440]) for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['personal space', 'chat'] as const) {
  test(`a single ${kind} tab keeps its purpose and target (${width}/${locale})`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: 'light' });
    await resourceFixtures(page);
    const loggedIn = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);
    const auth: unknown = await loggedIn;
    if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The native sign-in supplied no access token.');
    await asAdmin(page, true);
    const saved = await page.request.patch('/api/v1/me', { headers: { Authorization: 'Bearer ' + auth.access_token }, data: { theme: 'light', locale } });
    expect(saved.ok()).toBe(true);
    const target = kind === 'personal space' ? '/org/ada' : `/chat/${id(9, 1)}`;
    await page.goto(target);
    await screenReady(page, locale, 'light');
    const strip = page.locator(width === 400 ? '[data-mobile-main-tabs]' : '[data-main-area] [data-navigation-strip="views"]');
    const selected = strip.getByRole('tab', { selected: true });
    await expect(selected).toHaveCount(1);
    const tabId = await selected.getAttribute('id');
    if (tabId === null) throw new Error('The selected native view has no tab id.');
    await expect(selected).toHaveAccessibleName(new RegExp(ADA.display_name));
    const purpose = kind === 'personal space' ? locale === 'en' ? /Personal space/ : /个人空间/ : locale === 'en' ? /chat/i : /聊天/;
    await expect(selected).toHaveAccessibleName(purpose);
    await page.goto('/org/acme/settings/general');
    await screenReady(page, locale, 'light');
    // A retained conversation may also name this person. Use the actual General form.
    const nameField = page.getByRole('textbox', { name: caption('@settings.general.name', locale), exact: true });
    const identity = page.locator('[data-main-area] form').filter({ has: nameField });
    await expect(identity.getByRole('textbox', { name: caption('@settings.general.name', locale), exact: true })).toHaveValue('Acme');
    const owner = identity.getByText(ADA.display_name, { exact: true });
    await expect(owner).toBeInViewport();
    const currentName = strip.getByRole('tab', { selected: true }).locator('[data-view-tab-title]');
    await expect(currentName).toBeVisible();
    await expect(currentName).not.toBeEmpty();
    const beforeWidths = await strip.getByRole('tab').evaluateAll((tabs) => tabs.map((tab) => ({
      id: tab.id, width: tab.parentElement!.getBoundingClientRect().width,
    })));
    const inactive = page.locator(`[id="${tabId}"]`);
    const allTabsName = caption('@workbench.allTabs', locale).replace('{{name}}', caption('@workbench.areas.main', locale));
    const phoneTarget = page.locator(`[role="option"][data-value=${JSON.stringify('commands:workspace.focus.' + tabId.slice('mobile-tab-'.length))}]`);
    if (width === 400) {
      await expect(strip.getByRole('tab')).toHaveCount(1);
      await expect(inactive).toHaveCount(0);
      await page.getByRole('button', { name: allTabsName, exact: true }).click();
      await expect(phoneTarget).toHaveAccessibleName(new RegExp(ADA.display_name));
      await expect(phoneTarget).toHaveAccessibleName(purpose);
      await page.keyboard.press('Escape');
      await expect(phoneTarget).toBeHidden();
    } else {
      await expect(inactive).toHaveAttribute('aria-selected', 'false');
      await expect(inactive).toHaveAccessibleName(purpose);
      await expect(strip.getByRole('tab').filter({ hasText: ADA.display_name })).toHaveCount(1);
    }
    expect((await page.evaluate(inspectLayout, LAYOUT_INPUT)).faults.filter((fault) => fault.rule.startsWith('duplicate-'))).toEqual([]);
    expect(await duplicateAccessibleNames(page)).toEqual([]);
    await expect(owner).toBeInViewport();
    await info.attach('purpose-caption', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    if (width === 400) {
      await page.getByRole('button', { name: allTabsName, exact: true }).click();
      await phoneTarget.click();
    } else await inactive.click();
    await screenReady(page, locale, 'light');
    expect(new URL(page.url()).pathname).toBe(target);
    await expect(page.locator(`[id="${tabId}"]`)).toHaveAttribute('aria-selected', 'true');
    const afterWidths = await strip.getByRole('tab').evaluateAll((tabs) => tabs.map((tab) => ({
      id: tab.id, width: tab.parentElement!.getBoundingClientRect().width,
    })));
    if (width === 1440) {
      expect(afterWidths.map((tab) => tab.id)).toEqual(beforeWidths.map((tab) => tab.id));
      for (const [index, tab] of afterWidths.entries()) expect(Math.abs(tab.width - beforeWidths[index]!.width), tab.id).toBeLessThan(1);
    } else {
      await expect(strip.getByRole('tab')).toHaveCount(1);
      await expect(strip.getByRole('tab').locator('[data-view-tab-title]')).toBeVisible();
      for (const tab of afterWidths) expect(tab.width).toBeLessThanOrEqual(width);
    }
    await info.attach('selection-tab-widths', { body: JSON.stringify({ before: beforeWidths, after: afterWidths }), contentType: 'application/json' });
  });
}
