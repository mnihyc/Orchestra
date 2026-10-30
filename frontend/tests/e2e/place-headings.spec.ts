/** A place is named with where it is. A dialog has no title bar above it, so a dialog that names a place says what
 * kind of place it is and the way down to it ("Move the topic Fix login, in Acme › Research › Web"): the four threads
 * called "Main thread", or a project and a folder of one name, are never told apart by luck. The check of the screens:
 * each kind of place, every dialog its menu opens, and every heading in that dialog that names the place. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { ACME, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

/** A place of each kind in the fake world, with the way down to it as the title bar draws it. */
const PLACES = [
  { screen: '/org/acme.research', name: 'Research', kind: 'department', above: 'Acme' },
  { screen: `/project/${ACME.web}`, name: 'Web', kind: 'project', above: 'Acme › Research' },
  { screen: `/project/${ACME.checkout}`, name: 'Checkout', kind: 'folder', above: 'Acme › Shop' },
  { screen: `/topic/${ACME.fixLogin}`, name: 'Fix login', kind: 'topic', above: 'Acme › Research › Web' },
] as const;

/** The acts of a place's menu that open a dialog about the place itself. */
const ACTS = ['Rename…', 'Move to…', 'Colour…', 'Archive…', 'Delete…', 'Recently deleted…'] as const;

const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function openDialog(page: Page): Locator {
  return page.getByRole('dialog').or(page.getByRole('alertdialog')).last();
}

for (const place of PLACES) {
  test(`every dialog about the ${place.kind} ${place.name} says where it lies`, async ({ page }) => {
    await signIn(page);
    await page.goto(place.screen);
    const named = new RegExp(`the ${place.kind} ${escaped(place.name)}, in ${escaped(place.above)}`);
    const opened: string[] = [];
    for (const act of ACTS) {
      await page.getByRole('button', { name: new RegExp(`^Actions for ${escaped(place.name)}(?: · .+)?$`) }).first().click();
      // The menu says what may be done once it knows: until then it is checking.
      const menu = page.getByRole('menu');
      await expect(menu).toBeVisible();
      await expect(menu.getByText('Checking what you may do here…')).toHaveCount(0);
      const entry = menu.getByRole('menuitem', { name: act, exact: true });
      if ((await entry.count()) === 0 || (await entry.getAttribute('aria-disabled')) === 'true') {
        await page.keyboard.press('Escape');
        continue;
      }
      await entry.click();
      const dialog = openDialog(page);
      await expect(dialog).toBeVisible();
      await expect(dialog).toHaveAccessibleName(named);
      // Any other heading in it that names the place names it with where it lies as well.
      for (const heading of await dialog.getByRole('heading').allTextContents()) {
        if (heading.includes(place.name)) expect(heading, `${act} of ${place.name}`).toMatch(named);
      }
      opened.push(act);
      await page.keyboard.press('Escape');
      await expect(page.getByRole('dialog').or(page.getByRole('alertdialog'))).toHaveCount(0);
    }
    // Every place offers at least its name, its place and its removal to whoever manages it.
    expect(opened).toEqual(expect.arrayContaining(['Rename…', 'Delete…']));
  });
}

test('an org at the root is named alone, as nothing lies above it', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await page.getByRole('button', { name: /^Actions for Acme(?: · .+)?$/ }).first().click();
  await page.getByRole('menuitem', { name: 'Colour…', exact: true }).click();
  await expect(openDialog(page)).toHaveAccessibleName('Colour of Acme');
});
