/** The tab's announced context menu and its pointer menu retain the same complete acts. */
import AxeBuilder from '@axe-core/playwright';
import { expect, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support';

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`view-tab menus preserve their acts and focus at ${width}px in ${theme}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    const requests: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (path.startsWith('/api/v1/') && path !== '/api/v1/ws') requests.push(`${request.method()} ${path}`);
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await page.waitForLoadState('networkidle');
    await expect(page).toHaveURL((url) => url.pathname === `/thread/${ACME.mainThread}`);
    const strip = page.getByRole('tablist', { name: 'Main area', exact: true });
    const selected = strip.getByRole('tab', { selected: true });
    const qualifiedCaption = 'Main thread · Fix login';
    if (width === 400) {
      await expect(page.getByRole('tree', { name: 'Orgs, projects, topics and threads', exact: true, includeHidden: true })).toHaveCount(0);
      expect(requests.filter((request) => request === `GET /api/v1/orgs/${ACME.org}/tree`)).toEqual([]);
    }
    await expect(selected).toHaveText(width === 400 ? 'Main thread' : qualifiedCaption);
    const longCaption = await selected.locator('[data-view-tab-title]').evaluate((element) => {
      const previous = element.textContent;
      try {
        element.textContent = 'A very long readable title '.repeat(20);
        const whole = element.parentElement?.parentElement;
        if (whole === undefined || whole === null) throw new Error('The caption belongs to its tab');
        return { width: whole.getBoundingClientRect().width, maximum: 16 * Number.parseFloat(getComputedStyle(document.documentElement).fontSize),
          shortened: element.scrollWidth > element.clientWidth, overflow: getComputedStyle(element).textOverflow };
      } finally { element.textContent = previous; }
    });
    expect(longCaption.width).toBeLessThanOrEqual(longCaption.maximum + 1);
    expect(longCaption.shortened).toBe(true);
    expect(longCaption.overflow).toBe('ellipsis');
    const identity = await selected.getAttribute('id');
    if (identity === null) throw new Error('The actual thread tab must have its stable identity');
    const tab = page.locator(`[role="tab"][id="${identity}"]`);
    await expect(tab).toHaveAttribute('aria-haspopup', 'menu');
    await test.info().attach('thread-initial-requests', { body: JSON.stringify({ count: requests.length, requests, caption: await selected.innerText(), acmeTreeReads: requests.filter((request) => request === `GET /api/v1/orgs/${ACME.org}/tree`) }), contentType: 'application/json' });

    // A normal in-app visit retains this tab and supplies the neighbor selected after Close.
    await page.getByRole('navigation', { name: 'Where this is' }).getByRole('link', { name: 'Fix login', exact: true }).click();
    await expect(strip.getByRole('tab', { name: 'Fix login', exact: true })).toBeVisible();
    if (width === 400) {
      await page.getByRole('button', { name: 'Find tab in Main area', exact: true }).click();
      await page.locator(`[role="option"][data-value="commands:workspace.focus.${identity.replace(/^mobile-tab-/, '')}"]`).click();
    } else await tab.click();
    await expect(tab).toHaveAttribute('aria-selected', 'true');
    await expect(page).toHaveURL((url) => url.pathname === `/thread/${ACME.mainThread}`);
    await expect(tab).toHaveText(qualifiedCaption);
    const pointer = tab.locator('..').locator('button[aria-hidden="true"]');
    const menuLabel = `Menu of ${qualifiedCaption}`;
    await expect(pointer).toHaveAttribute('aria-label', menuLabel);

    await tab.focus();
    await page.keyboard.press('Shift+F10');
    let menu = page.getByRole('menu', { name: menuLabel, exact: true });
    await expect(menu.getByRole('menuitem', { name: 'Pin', exact: true })).toBeVisible();
    await expect(menu.getByRole('menuitem', { name: 'Close', exact: true })).toBeVisible();
    const keyboardActs = await menu.getByRole('menuitem').allTextContents();
    await page.keyboard.press('Escape');
    await expect(tab).toBeFocused();

    // The duplicate pointer target stays visible; the tab is the announced keyboard entrance.
    await expect(pointer).toBeVisible();
    await pointer.click();
    menu = page.getByRole('menu', { name: menuLabel, exact: true });
    expect(await menu.getByRole('menuitem').allTextContents()).toEqual(keyboardActs);
    await menu.getByRole('menuitem', { name: 'Pin', exact: true }).click();
    await expect(tab).toBeFocused();
    await pointer.click();
    await page.getByRole('menu', { name: menuLabel, exact: true }).getByRole('menuitem', { name: 'Unpin', exact: true }).click();
    await expect(tab).toBeFocused();

    const audit = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
    expect(audit.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical').map((violation) => violation.id)).toEqual([]);
    const path = test.info().outputPath(`view-tabs-${width}-${theme}.png`);
    await page.screenshot({ path, fullPage: true, animations: 'disabled' });
    await test.info().attach(`view-tabs-${width}-${theme}`, { path, contentType: 'image/png' });

    if (width === 400) await pointer.click();
    else { await tab.focus(); await page.keyboard.press('Shift+F10'); }
    menu = page.getByRole('menu', { name: menuLabel, exact: true });
    await menu.getByRole('menuitem', { name: 'Close', exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(tab).toHaveCount(0);
    await expect(strip.getByRole('tab', { selected: true })).toBeFocused();
  });
}
