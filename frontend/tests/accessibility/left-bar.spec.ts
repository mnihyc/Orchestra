/** The left bar in every state it has, asked in both themes whether anything on it is a serious barrier: as it comes, with everything a row can carry, narrowed to a place, with chats in the tree, with its menus and the colour dialog open, as the Resources view, and in the drawer of a phone; and the twelve colours of an icon measured against every ground an icon stands on. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { chats } from '../e2e/chat-support.ts';
import { ACME, expectAccessibleInBothThemes, scenario, signIn } from '../e2e/support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** A request only the fake API has. */
async function double(path: string, body: unknown = {}): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
}

test.beforeEach(async () => {
  await scenario();
});

const bar = (page: Page) => page.getByRole('navigation', { name: 'Navigation' });
const rail = (page: Page) => page.getByRole('navigation', { name: 'Orgs and your own places' });
const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });
const row = (page: Page, name: string): Locator => tree(page).getByRole('treeitem', { name: new RegExp(`^(Waiting for approval |Blocked |Running |Starting |Idle )?${name}( |$)`) });

async function viewMenu(page: Page): Promise<Locator> {
  await bar(page).getByRole('button', { name: 'View options' }).click();
  return page.getByRole('menu', { name: 'View options' });
}

async function rowMenu(page: Page, name: string): Promise<Locator> {
  await row(page, name).hover();
  await row(page, name).getByRole('button', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`), exact: true }).click();
  return page.getByRole('menu', { name: new RegExp(`^Actions for ${name}(?: · .+)?$`), exact: true });
}

test('the bar as it comes, and with everything a row can carry: unread threads, a chat that is unread and one that mentions, a quiet chat, a topic that is done, documents, a starred place and folded sections', async ({ page }) => {
  await chats('/say', { node: 'Fix login', as: 'ada', text: 'Shall we start?' });
  await chats('/say', { node: 'Upgrade deps', as: 'ada', text: 'Hello.' });
  await double('/places/follow', { username: 'ada', node: ACME.infra, level: 'all' });
  await signIn(page);
  await page.goto('/org/acme');
  await expect(row(page, 'Web')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await chats('/say', { node: 'Fix login', as: 'dora', text: '@ada can you look?' });
  await chats('/say', { node: 'Upgrade deps', as: 'dora', text: 'Hi.' });
  await chats('/say', { node: 'Migrate CI', as: 'dora', text: 'Done?' });
  await row(page, 'Upgrade deps').press('ArrowRight');
  await row(page, 'Infra').press('ArrowLeft');
  await (await rowMenu(page, 'Shop')).getByRole('menuitem', { name: 'Star' }).click();
  await expect(row(page, 'Fix login').getByText('1 mention of you inside')).toBeAttached();
  await expect(row(page, 'Infra').getByRole('img', { name: 'Unread inside' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // Every section folded: each heading with what waits inside.
  for (const name of ['Starred', 'Projects']) await bar(page).getByRole('button', { name, exact: true }).click();
  await expect(tree(page)).toHaveCount(0);
  await expectAccessibleInBothThemes(page);
});

test('the tree with chats in it, the tree’s own menu and a place’s menu open, and the dialog that gives an icon its colour', async ({ page }) => {
  await chats('/say', { node: 'Fix login', as: 'ada', text: 'Shall we start?' });
  await chats('/say', { node: 'Upgrade deps', as: 'dora', text: 'Which versions?' });
  await chats('/chat', { org: ACME.org, members: ['grace', 'ada', 'dora'], name: 'Release' });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(row(page, 'Main thread').first()).toBeVisible();

  const menu = await viewMenu(page);
  await expectAccessibleInBothThemes(page);
  await menu.getByRole('menuitemcheckbox', { name: 'Chats' }).click();
  await chats('/say', { node: 'Fix login', as: 'dora', text: '@ada can you look?' });
  await row(page, 'Upgrade deps').press('ArrowRight');
  await expect(tree(page).getByRole('treeitem', { name: /^Chat not followed/ })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /^Team chat/ })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /^Group chats/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // A place's menu, opened as the keyboard opens it: the list is longer than the room under its row, and is walked by the arrows.
  await row(page, 'Web').focus();
  await page.keyboard.press('Shift+F10');
  await expect(page.getByRole('menu', { name: /^Actions for Web(?: · .+)?$/ }).getByRole('menuitem', { name: 'Show only this' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('menuitem', { name: 'Colour…' }).click();
  await expect(page.getByRole('dialog', { name: /^Colour of the project Web, in / })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a tree narrowed to a place, with the line of a thing that lies outside it, and the places that stay while the tree scrolls', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 640 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await expect(row(page, 'Repro')).toBeVisible();
  await row(page, 'Repro').evaluate((element) => element.scrollIntoView({ block: 'start' }));
  await expect(bar(page).locator('[data-staying]')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await bar(page).locator('[data-bar-scroll]').evaluate((element) => element.scrollTo(0, 0));
  await (await rowMenu(page, 'Shop')).getByRole('menuitem', { name: 'Show only this' }).click();
  await expect(bar(page).getByRole('group', { name: 'Where the tree begins' })).toBeVisible();
  await expect(tree(page).getByRole('treeitem').first()).toContainText('Main thread is in');
  await expectAccessibleInBothThemes(page);
});

test('the Resources view of the left bar, with a kind open and a thing of it in view', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  await rail(page).getByRole('button', { name: 'Resources' }).click();
  const kinds = page.getByRole('tree', { name: 'Kinds of resource and what each holds' });
  await kinds.getByRole('treeitem', { name: /^Agents / }).press('ArrowRight');
  await kinds.locator('[role="treeitem"][aria-level="2"]').first().getByRole('link').click();
  await expect(page).toHaveURL(/\/resources\/agents\/[^/]+$/);
  await expect(kinds.locator('[role="treeitem"][aria-current="page"]')).toHaveCount(1);
  await expectAccessibleInBothThemes(page);
});

test('the bar in the drawer of a phone: the way to another org at its head, a narrowed tree, and its menus', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await signIn(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  await page.getByRole('button', { name: 'Navigation', exact: true }).click();
  const drawer = page.getByRole('dialog', { name: 'Navigation' });
  await expect(drawer.getByRole('treeitem', { name: /^Shop / })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await drawer.getByRole('button', { name: /Acme/ }).first().click();
  await expect(page.getByRole('menuitem', { name: 'Shared with me' })).toHaveCount(0);
  await expect(page.getByRole('menuitem', { name: 'Resources', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await drawer.getByRole('treeitem', { name: /^Shop / }).getByRole('button', { name: /^Actions for Shop(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Show only this' }).click();
  await expect(drawer.getByRole('group', { name: 'Where the tree begins' })).toBeVisible();
  // In a narrow bar the line names the place the thing lies in, and a screen reader is told in full.
  await expect(drawer.getByRole('treeitem').first()).toContainText('In Infra');
  await expect(drawer.getByRole('treeitem').first()).toHaveAccessibleName(/^Idle-stop rework is in Acme \/ Infra/);
  await expectAccessibleInBothThemes(page);
  await drawer.getByRole('button', { name: 'View options' }).click();
  await expect(page.getByRole('menuitem', { name: 'Show everything' })).toHaveCount(0);
  await expect(page.getByRole('menuitemcheckbox', { name: 'Where I take part', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

/**
 * The least an icon's colour must stand out from its ground by: what the guidelines ask of a graphic that
 * carries meaning (3 to 1). Each kind has its own glyph, so the colour is never the only sign of anything.
 */
const GLYPH_CONTRAST = 3;

// The gallery of the shared controls is part of a development build only: this test runs against the development server.
test('each of the twelve colours of an icon stands out from every ground an icon is drawn on, in both themes', { tag: '@development' }, async ({ page }) => {
  await signIn(page);
  await page.goto('/dev/ui');
  const twelve = page.locator('[data-node-icon="folder"][data-node-color]');
  await expect(twelve.first()).toBeVisible();

  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    // The theme follows the system on a page that was told nothing else; let it be drawn.
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    // Every colour as the browser paints it, read off a canvas: whatever notation the stylesheet computes it in.
    const worst = await page.evaluate(() => {
      const canvas = document.createElement('canvas');
      canvas.width = 1;
      canvas.height = 1;
      const context = canvas.getContext('2d', { willReadFrequently: true });
      if (context === null) throw new Error('no canvas');
      const painted = (color: string): [number, number, number] => {
        context.clearRect(0, 0, 1, 1);
        context.fillStyle = color;
        context.fillRect(0, 0, 1, 1);
        const [red = 0, green = 0, blue = 0] = context.getImageData(0, 0, 1, 1).data;
        return [red, green, blue];
      };
      const luminance = ([red, green, blue]: [number, number, number]): number => {
        const linear = (value: number) => (value / 255 <= 0.04045 ? value / 255 / 12.92 : ((value / 255 + 0.055) / 1.055) ** 2.4);
        return 0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue);
      };
      const probe = document.createElement('span');
      document.body.append(probe);
      const colorOf = (css: string): [number, number, number] => {
        probe.style.color = css;
        return painted(getComputedStyle(probe).color);
      };
      // The grounds an icon stands on: the page, a card, the left bar, a row under the pointer, and the row of the screen in view.
      const grounds = ['--bg', '--surface', '--bg-subtle', '--bg-muted', '--accent-subtle'];
      let least = { ratio: Infinity, colour: 0, ground: '' };
      for (let colour = 1; colour <= 12; colour += 1) {
        const ink = luminance(colorOf(`var(--node-ink-${colour})`));
        for (const ground of grounds) {
          const under = luminance(colorOf(`var(${ground})`));
          const ratio = (Math.max(ink, under) + 0.05) / (Math.min(ink, under) + 0.05);
          if (ratio < least.ratio) least = { ratio, colour, ground };
        }
      }
      probe.remove();
      return least;
    });
    test.info().annotations.push({ type: `least contrast, ${theme}`, description: `${worst.ratio.toFixed(2)} to 1 (colour ${worst.colour} on ${worst.ground})` });
    expect(worst.ratio, `colour ${worst.colour} on ${worst.ground} in the ${theme} theme`).toBeGreaterThanOrEqual(GLYPH_CONTRAST);
  }
});
