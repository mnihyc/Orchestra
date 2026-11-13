/** A hidden pane stays hidden: it opens by its button in the top bar and by its key, and nothing opens because the pointer crossed an edge, left the window
 * over one, rested on a scroll bar or dragged it. A person who wants hidden panes to show on hover says so in Settings, Interface, and then a pane peeks only
 * after the pointer has rested in the strip at its edge, never from a drag, and goes when the pointer leaves it. */
import { expect, test, type Page } from '@playwright/test';

import { PEOPLE, scenario, signIn } from './support.ts';
import { restAtEdge, showHiddenBarsOnHover } from './workbench-support.ts';

const rightBar = (page: Page) => page.getByRole('dialog', { name: 'Right bar', exact: true });
const leftBar = (page: Page) => page.getByRole('dialog', { name: 'Left bar', exact: true });

/** The pane's button in the top bar says whether the pane is open. */
async function hide(page: Page, name: 'Left bar' | 'Right bar'): Promise<void> {
  const button = page.getByRole('button', { name, exact: true });
  await expect(button).not.toHaveAttribute('aria-disabled', 'true');
  if (await button.getAttribute('aria-pressed') === 'true') await button.click();
  await expect(button).toHaveAttribute('aria-pressed', 'false');
}

async function open(page: Page, width: number): Promise<void> {
  await page.setViewportSize({ width, height: 900 });
  await page.goto('/org/acme');
  await expect(page.locator('[data-main-area]')).toBeVisible();
  await hide(page, 'Right bar');
  await hide(page, 'Left bar');
}

/** The pointer goes to the edge as a hand does: across the page, to the last pixel, and out of the window. */
async function crossAndLeave(page: Page, width: number): Promise<void> {
  await page.mouse.move(width / 2, 500);
  await page.mouse.move(width - 1, 500, { steps: 12 });
  await page.mouse.move(width + 40, 500);
}

test.beforeEach(async () => { await scenario(); });

for (const width of [1440, 1200]) {
  test(`a hidden pane stays hidden when the pointer crosses the edge, leaves the window over it, rests on a scroll bar or drags it (${width})`, async ({ page }) => {
    await signIn(page, PEOPLE.ada);
    await open(page, width);
    await expect(rightBar(page)).toHaveCount(0);
    await crossAndLeave(page, width);
    await page.waitForTimeout(900);
    await expect(rightBar(page)).toHaveCount(0);
    // Rests on the scroll bar of the page, then takes hold of it and drags it.
    await page.mouse.move(width - 5, 300);
    await page.waitForTimeout(900);
    await expect(rightBar(page)).toHaveCount(0);
    await page.mouse.down();
    await page.mouse.move(width - 5, 600, { steps: 8 });
    await page.mouse.up();
    await page.waitForTimeout(600);
    await expect(rightBar(page)).toHaveCount(0);
    // The same at the other edge, beside the rail.
    await restAtEdge(page, 'left');
    await page.waitForTimeout(900);
    await expect(leftBar(page)).toHaveCount(0);
    // The button in the top bar still opens it, and the button hides it again.
    await expect(page.getByRole('button', { name: 'Right bar', exact: true })).not.toHaveAttribute('aria-disabled', 'true');
    await page.getByRole('button', { name: 'Right bar', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Right bar', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(page.locator('[data-workbench-bar="right"]')).toBeVisible();
  });
}

test('with hidden panes shown on hover, a pane peeks after the pointer has rested at its edge, not from a crossing, a drag or a window leave, and goes when the pointer leaves it', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await showHiddenBarsOnHover(page);
  await open(page, 1440);
  await expect(rightBar(page)).toHaveCount(0);
  await crossAndLeave(page, 1440);
  await page.waitForTimeout(900);
  await expect(rightBar(page)).toHaveCount(0);
  // A drag that begins at the edge is a drag.
  await page.mouse.move(1000, 300);
  await page.mouse.down();
  await page.mouse.move(1438, 300, { steps: 6 });
  await page.waitForTimeout(700);
  await page.mouse.up();
  await expect(rightBar(page)).toHaveCount(0);
  // A rest of a good while on the body of the scroll bar, short of the window's edge, opens nothing.
  await page.mouse.move(1000, 700);
  await page.mouse.move(1431, 700, { steps: 6 });
  await page.waitForTimeout(900);
  await expect(rightBar(page)).toHaveCount(0);
  // A rest at the very edge peeks, and the person's choice of what is open is not changed by it.
  await page.mouse.move(1439, 700);
  await expect(rightBar(page)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Right bar', exact: true })).toHaveAttribute('aria-pressed', 'false');
  await page.mouse.move(700, 500, { steps: 6 });
  await expect(rightBar(page)).toHaveCount(0);
  await expect(page.locator('[data-workbench-bar="right"]')).toHaveCount(0);
  // The left bar peeks from the strip beside the rail, in the same way.
  await restAtEdge(page, 'left');
  await expect(leftBar(page)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Left bar', exact: true })).toHaveAttribute('aria-pressed', 'false');
  await page.mouse.move(900, 500, { steps: 6 });
  await expect(leftBar(page)).toHaveCount(0);
});
