/** A single activity highlight selects the left view; a separate accent identifies the current page. */
import { expect, test } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';

for (const theme of ['light', 'dark'] as const) {
  test(`rail clicks dock, shortcuts toggle and only the edge peeks at 1440/${theme}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page, PEOPLE.vera);
    await page.goto('/org/acme');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
    const organization = rail.getByRole('link', { name: /^Acme\b/ });
    const resources = rail.getByRole('button', { name: 'Resources', exact: true });
    const left = page.locator('[data-workbench-bar="left"]');
    const main = page.locator('[data-main-area]');
    const floating = page.getByRole('dialog', { name: 'Left bar', exact: true });
    const edge = page.locator('[data-bar-edge="left"]');
    const requests: Record<string, string[]> = {};
    let phase = '';
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (phase !== '' && url.pathname.startsWith('/api/v1/')) (requests[phase] ??= []).push(`${request.method()} ${url.pathname}${url.search}`);
    });
    const shot = async (state: string) => {
      const directory = process.env.ORCH_NAV_SCREEN_DIR;
      if (directory !== undefined) await page.screenshot({ path: `${directory}/toggle-1440-${theme}-${state}.png` });
    };
    await expect(left).toHaveAttribute('data-active-container', 'navigation');
    await page.waitForLoadState('networkidle');
    phase = 'navigation-toggle'; requests[phase] = [];
    await organization.click();
    await expect(left).toHaveCount(0);
    await expect(organization).toHaveAttribute('aria-expanded', 'false');
    await expect(floating).toHaveCount(0);
    const hiddenWidth = (await main.boundingBox())?.width;
    if (hiddenWidth === undefined) throw new Error('The page must remain visible beside its bars');
    await page.mouse.move(1000, 800);
    await organization.hover();
    await expect(page.getByRole('tooltip')).toHaveText('Show navigation for Acme');
    await shot('hidden');
    await organization.click();
    await expect(left).toHaveAttribute('data-active-container', 'navigation');
    await expect(left.getByRole('tree', { name: 'Orgs, projects, topics and threads' })).toBeVisible();
    await expect(organization).toHaveAttribute('aria-expanded', 'true');
    await expect(floating).toHaveCount(0);
    expect((await main.boundingBox())?.width).toBeLessThan(hiddenWidth);
    await page.mouse.move(1000, 800);
    await organization.hover();
    await expect(page.getByRole('tooltip')).toHaveText('Hide navigation for Acme');
    await shot('navigation-docked');
    await page.waitForLoadState('networkidle');
    expect(requests[phase]).toEqual([]);

    await organization.click();
    phase = 'resources-cold'; requests[phase] = [];
    await resources.click();
    await expect(left).toHaveAttribute('data-active-container', 'resource-navigation');
    await expect(left.getByRole('tree', { name: 'Kinds of resource and what each holds', exact: true })).toBeVisible();
    await expect(resources).toHaveAttribute('aria-pressed', 'true');
    await expect(floating).toHaveCount(0);
    await page.mouse.move(1000, 800);
    await resources.hover();
    await expect(page.getByRole('tooltip')).toHaveText('Hide resources');
    await shot('resources-docked');
    await page.waitForLoadState('networkidle');
    phase = 'resources-warm'; requests[phase] = [];
    await resources.press('Enter');
    await expect(left).toHaveCount(0);
    await expect(resources).toHaveAttribute('aria-pressed', 'false');
    await page.mouse.move(1000, 800);
    await resources.hover();
    await expect(page.getByRole('tooltip')).toHaveText('Show resources');
    await edge.hover();
    await expect(floating).toBeVisible();
    await expect(resources).toHaveAttribute('aria-pressed', 'false');
    expect((await main.boundingBox())?.width).toBe(hiddenWidth);
    await shot('peek');
    await page.keyboard.press('Escape');
    await expect(floating).toHaveCount(0);
    await expect(left).toHaveCount(0);
    await page.mouse.move(1000, 800);
    await edge.hover();
    await expect(floating).toBeVisible();
    await page.mouse.move(1000, 800);
    await expect(floating).toHaveCount(0);
    await edge.hover();
    await expect(floating).toBeVisible();
    await resources.click();
    await expect(floating).toHaveCount(0);
    await expect(left).toHaveAttribute('data-active-container', 'resource-navigation');
    await resources.click();
    await edge.hover();
    await expect(floating).toBeVisible();
    await floating.getByRole('button', { name: 'Keep open as a bar', exact: true }).click();
    await expect(floating).toHaveCount(0);
    await expect(left).toHaveAttribute('data-active-container', 'resource-navigation');
    await shot('pinned');
    await page.keyboard.press('Control+\\');
    await expect(left).toHaveCount(0);
    await page.keyboard.press('Control+\\');
    await expect(left).toBeVisible();
    await expect(floating).toHaveCount(0);
    await page.waitForLoadState('networkidle');
    expect(requests[phase]).toEqual([]);

    phase = '';
    await organization.click();
    await organization.click();
    await edge.hover();
    await expect(floating).toBeVisible();
    await floating.locator(`a[href="/project/${ACME.infra}"]`).first().click();
    await expect(page).toHaveURL(new RegExp(`/project/${ACME.infra}$`));
    await expect(floating).toHaveCount(0);
    await expect(left).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const figures = JSON.stringify(requests, null, 2);
    await test.info().attach('toggle-requests', { body: figures, contentType: 'application/json' });
    const directory = process.env.ORCH_NAV_SCREEN_DIR;
    if (directory !== undefined) await writeFile(`${directory}/toggle-1440-${theme}-requests.json`, figures);
  });
}

test('page accents and the one left-view highlight remain distinct by pointer and keyboard', async ({ page }) => {
  await scenario();
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
  const org = rail.getByRole('link', { name: /^Acme\b/ });
  const inbox = rail.getByRole('link', { name: /^Inbox\b/ });
  const resources = rail.getByRole('button', { name: 'Resource navigation', exact: true });
  await inbox.click();
  await expect(inbox).toHaveAttribute('aria-current', 'page');
  await expect(inbox.locator('[data-rail-page]')).toBeVisible();
  await resources.click();
  await expect(rail.locator('[data-active-left-view]')).toHaveCount(1);
  await expect(resources).toHaveAttribute('aria-pressed', 'true');
  await expect(resources).toHaveAttribute('data-active-left-view', 'resource-navigation');
  await page.mouse.move(1000, 800);
  const selected = await resources.evaluate((element) => getComputedStyle(element).backgroundColor);
  expect(await inbox.evaluate((element) => getComputedStyle(element).backgroundColor)).not.toBe(selected);
  await resources.focus();
  await resources.press('Enter');
  await expect(rail.locator('[data-active-left-view]')).toHaveCount(0);
  await expect(inbox.locator('[data-rail-page]')).toBeVisible();
  await resources.press('Enter');
  await expect(rail.locator('[data-active-left-view]')).toHaveCount(1);
  await org.focus();
  await org.press('Enter');
  await expect(org).toHaveAttribute('data-active-left-view', 'navigation');
  await expect(rail.locator('[data-active-left-view]')).toHaveCount(1);
  await expect(resources).toHaveAttribute('aria-pressed', 'false');
  // A resource's page keeps its small page accent even while Navigation is in front.
  await page.goto('/resources/machines');
  await expect(resources).toHaveAttribute('aria-current', 'page');
  await expect(resources.locator('[data-rail-page]')).toBeVisible();
  await expect(resources).toHaveAttribute('aria-pressed', 'false');
  await expect(org).toHaveAttribute('data-active-left-view', 'navigation');
  await expect(rail.locator('[data-active-left-view]')).toHaveCount(1);
});

for (const width of [1100, 1440]) {
  test(`an org icon returns from settings to its home and opens a previously closed panel at ${width}px`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await signIn(page);
    await page.goto('/org/acme');
    const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
    const org = rail.getByRole('link', { name: /^Acme\b/ });
    const left = page.locator('[data-workbench-bar="left"]');
    await expect(org).toHaveAttribute('data-active-left-view', 'navigation');
    // Only the actual home in view toggles, and this choice is saved in the workspace.
    await org.click();
    await expect(left).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => {
      return Object.keys(localStorage).some((key) => {
        if (!key.endsWith('.workbench.workspace')) return false;
        const text = localStorage.getItem(key);
        if (text === null) return false;
        const stored = JSON.parse(text) as { layout: string };
        const layout = JSON.parse(stored.layout) as { left: { open: boolean } };
        return !layout.left.open;
      });
    })).toBe(true);
    await page.goto('/settings/profile');
    await expect(page.getByRole('heading', { name: 'Profile', exact: true })).toBeVisible();
    await org.click();
    await expect(page).toHaveURL(/\/org\/acme\/?$/);
    await expect(left).toHaveAttribute('data-active-container', 'navigation');
    await expect(left.getByRole('tree', { name: 'Orgs, projects, topics and threads' })).toBeVisible();
    await expect(org).toHaveAttribute('data-active-left-view', 'navigation');
    await expect(rail.locator('[data-active-left-view]')).toHaveCount(1);
    await org.click();
    await expect(left).toHaveCount(0);
    await expect(page).toHaveURL(/\/org\/acme\/?$/);
    await org.press('Enter');
    await expect(left).toHaveAttribute('data-active-container', 'navigation');
    await expect(org).toHaveAttribute('data-active-left-view', 'navigation');
  });
}

test('keyboard org navigation from a thread or resource page opens the org home instead of toggling', async ({ page }) => {
  await scenario();
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
  const org = rail.getByRole('link', { name: /^Acme\b/ });
  for (const path of [`/thread/${ACME.mainThread}`, '/resources/providers', '/org/acme/settings/general', '/settings/interface']) {
    await page.goto(path);
    await page.mouse.move(1000, 800);
    await org.hover();
    await expect(page.getByRole('tooltip')).toHaveText('Open Acme home');
    await org.focus();
    await org.press('Enter');
    await expect(page).toHaveURL(/\/org\/acme\/?$/);
    await expect(page.locator('[data-workbench-bar="left"]')).toHaveAttribute('data-active-container', 'navigation');
    await expect(org).toHaveAttribute('data-active-left-view', 'navigation');
  }
});

test('the clicked org is selected and names the pending tab before its overview finishes', async ({ page }) => {
  await scenario();
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto('/org/ada');
  await page.goto('/settings/profile');
  let release: (() => void) | undefined;
  const waiting = new Promise<void>((resolve) => { release = resolve; });
  let arrived: (() => void) | undefined;
  const requested = new Promise<void>((resolve) => { arrived = resolve; });
  await page.route(`**/nodes/${ACME.org}/children*`, async (route) => { arrived?.(); await waiting; await route.continue(); });
  try {
    const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
    const org = rail.getByRole('link', { name: /^Acme\b/ });
    await org.click();
    await requested;
    await expect(org).toHaveAttribute('data-active-left-view', 'navigation');
    await expect(rail.locator('[data-active-left-view]')).toHaveCount(1);
    await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: 'Acme', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('[data-workbench-bar="left"]').getByRole('link', { name: /^Messages/ })).toHaveAttribute('href', '/org/acme/messages');
  } finally { release?.(); }
});

test('a modified org click keeps its native link behavior and leaves this page and panel alone', async ({ page, context }) => {
  await scenario();
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto('/settings/profile');
  const rail = page.getByRole('navigation', { name: 'Orgs and your own places', exact: true });
  const org = rail.getByRole('link', { name: /^Acme\b/ });
  await rail.getByRole('button', { name: 'Resource navigation', exact: true }).click();
  const otherTab = context.waitForEvent('page');
  await org.click({ modifiers: ['ControlOrMeta'] });
  const opened = await otherTab;
  await expect(opened).toHaveURL(/\/org\/acme\/?$/);
  await expect(page).toHaveURL(/\/settings\/profile$/);
  await expect(page.locator('[data-workbench-bar="left"]')).toHaveAttribute('data-active-container', 'resource-navigation');
  await opened.close();
});
