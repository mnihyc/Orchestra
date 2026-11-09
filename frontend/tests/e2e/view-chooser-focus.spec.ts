/** The one global view chooser keeps its complete choices and focus-mode entrances at both widths. */
import { expect, test, type Page } from '@playwright/test';
import { ACME, replySent, scenario, signIn } from './support.ts';

async function choices(page: Page): Promise<string[]> {
  const chooser = page.getByRole('dialog', { name: 'Views', exact: true });
  await expect(chooser.locator('[role="option"][data-value="commands:view.open.outline"]')).toBeVisible();
  // Command mode renders every ranked row in its ordinary scrolling PaletteList, without virtualization.
  await expect(chooser.locator('[role="option"][data-value^="more:"]')).toHaveCount(0);
  return chooser.locator('[role="option"][data-value^="commands:view.open."]').evaluateAll((options) =>
    options.map((option) => option.getAttribute('data-value') ?? '').sort());
}

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`the global view chooser preserves focus-mode choices at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    const loggedIn = page.waitForResponse((response) => response.request().method() === 'POST'
      && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);
    const auth: unknown = await loggedIn;
    if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') {
      throw new Error('The fake sign-in supplied no access token.');
    }
    const saved = await page.request.patch('/api/v1/me', {
      headers: { Authorization: `Bearer ${auth.access_token}` }, data: { theme },
    });
    expect(saved.ok()).toBe(true);
    await page.goto(`/thread/${ACME.mainThread}`);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const composer = page.getByRole('textbox', { name: 'Message to the agent', exact: true });
    await expect(composer).toBeVisible();
    const titleEntry = page.locator('[data-screen-titlebar]').getByRole('button', { name: 'Add a view', exact: true });
    await expect(titleEntry).toBeVisible();
    await titleEntry.focus();
    await titleEntry.press('Enter');
    const normalChoices = await choices(page);
    expect(normalChoices.length).toBeGreaterThan(0);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog', { name: 'Views', exact: true })).toHaveCount(0);
    await expect(titleEntry).toBeFocused();

    // The focused main group's existing fallback stays available after chrome and bars are hidden.
    await composer.focus();
    const layoutOwner = page.locator(width === 400 ? '[data-mobile-main-tabs]' : '[data-screen-titlebar]:visible');
    await layoutOwner.getByRole('button', { name: 'Layout', exact: true }).click();
    await page.getByRole('menuitemcheckbox', { name: 'Focus mode', exact: true }).click();
    await expect(page.locator('[data-workbench-chrome]:visible, [data-workbench-bar]:visible')).toHaveCount(0);
    const focusEntry = page.getByRole('button', { name: 'Add a view', exact: true });
    await expect(focusEntry).toHaveCount(1);
    await expect(focusEntry).toBeVisible();
    await focusEntry.focus();
    await focusEntry.press('Enter');
    expect(await choices(page)).toEqual(normalChoices);
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog', { name: 'Views', exact: true })).toHaveCount(0);
    await expect(focusEntry).toBeFocused();
    const screenshot = info.outputPath(`view-chooser-focus-${width}-${theme}.png`);
    await page.screenshot({ path: screenshot, animations: 'disabled' });
    await info.attach('focus-view-chooser', { path: screenshot, contentType: 'image/png' });

    const tab = page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { selected: true });
    await tab.focus();
    await page.keyboard.press('Shift+F10');
    await page.getByRole('menuitem', { name: 'Exit focus mode', exact: true }).click();
    await expect(titleEntry).toBeVisible();
    await titleEntry.focus();
    await titleEntry.press('Enter');
    expect(await choices(page)).toEqual(normalChoices);
    const restoredChooser = page.getByRole('dialog', { name: 'Views', exact: true });
    const search = restoredChooser.getByRole('combobox');
    await search.fill('Outline');
    const outline = restoredChooser.locator('[role="option"][data-value="commands:view.open.outline"]');
    await expect(outline).toHaveAttribute('aria-selected', 'true');
    await search.press('Enter');
    await expect(page.getByRole('dialog', { name: 'Views', exact: true })).toHaveCount(0);
    // A single-section container owns one always-open named view, without a redundant fold header.
    const revealed = page.locator('[data-workbench-bar="right"]:visible').getByRole('region', { name: 'Outline', exact: true });
    await expect(revealed).toBeVisible();
    await expect(revealed.getByText('Nothing to go to yet', { exact: true })).toBeVisible();
    await expect(revealed).toBeFocused();
  });
}
