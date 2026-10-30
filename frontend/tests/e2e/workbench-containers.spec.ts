/** Every container, section and tab stays visible and reachable by pointer and keyboard at every workbench width. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

const right = (page: Page) => page.locator('[data-workbench-bar="right"]');
async function chooseContainer(page: Page, name: string) {
  const tab = right(page).getByRole('tab', { name, exact: true });
  await expect(tab).toBeVisible();
  await tab.click();
  await expect(right(page)).toHaveAttribute('data-active-container', name.toLowerCase());
}
const section = (page: Page, name: string) => right(page).getByRole('button', { name, exact: true });

async function expectInside(item: Locator, strip: Locator) {
  const control = await item.boundingBox();
  const box = await strip.boundingBox();
  expect(control).not.toBeNull();
  expect(box).not.toBeNull();
  if (control === null || box === null) return;
  // Subpixel painting may round one CSS pixel either way.
  expect(control.x).toBeGreaterThanOrEqual(box.x - 1);
  expect(control.x + control.width).toBeLessThanOrEqual(box.x + box.width + 1);
  expect(control.y).toBeGreaterThanOrEqual(box.y - 1);
  expect(control.y + control.height).toBeLessThanOrEqual(box.y + box.height + 1);
}

test.beforeEach(async () => { await scenario(); });

test('frame choices keep names and contextual destinations without duplicate captions or help', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.getByRole('button', { name: 'Layout', exact: true }).click();
  const code = page.getByRole('menuitem', { name: 'Code', exact: true });
  await expect(code).toHaveAccessibleName('Code');
  await expect(code).toHaveAccessibleDescription('');
  await expect(code).not.toContainText('Files in the main area, Conversation at the right and Terminal below.');
  await expect(page.getByRole('menu').locator('[data-choice-help]')).toHaveCount(0);
  await page.keyboard.press('Escape');
  // Inspecting the menu does not apply the arrangement.
  await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: 'Main thread', exact: true })).toHaveAttribute('aria-selected', 'true');
  await page.getByRole('button', { name: 'Add a view', exact: true }).first().click();
  const chooser = page.getByRole('dialog', { name: 'Views', exact: true });
  const documents = chooser.getByRole('option', { name: 'Documents', exact: true });
  await expect(documents).toHaveAccessibleName('Documents');
  await expect(documents).toHaveAccessibleDescription('Team');
  await expect(documents.locator('[data-view-group]')).toHaveText('Team');
  await expect(documents).not.toContainText('The documents written beside this work.');
  await page.keyboard.press('Escape');
  await chooseContainer(page, 'Conversation');
  await right(page).getByRole('button', { name: 'Conversation actions', exact: true }).click();
  const outline = page.getByRole('menuitemcheckbox', { name: 'Outline', exact: true });
  await expect(outline).toHaveAccessibleDescription('The points of a conversation worth going to: messages, milestones, approvals, errors and pins.');
  await page.keyboard.press('Escape');
  await right(page).getByRole('button', { name: 'Actions for Outline', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).hover();
  const destination = page.getByRole('menuitem', { name: 'Bottom', exact: true });
  await expect(destination).toHaveAccessibleName('Bottom');
  await expect(destination).toHaveAccessibleDescription('Open the view in the panel below the main area.');
});

test('Enter chooses the named view directly and reveals its one home', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.getByRole('button', { name: 'Add a view', exact: true }).first().click();
  const chooser = page.getByRole('dialog', { name: 'Views', exact: true });
  const documents = chooser.getByRole('option', { name: 'Documents', exact: true });
  await expect(documents.locator('[data-view-group]')).toHaveText('Team');
  await expect(documents).not.toContainText('The documents written beside this work.');
  await expect(chooser.locator('[data-choice-help]')).toHaveCount(0);
  await documents.focus();
  await documents.press('Enter');
  await expect(chooser).toHaveCount(0);
  await expect(right(page)).toHaveAttribute('data-active-container', 'team');
  await expect(section(page, 'Documents')).toHaveAttribute('aria-expanded', 'true');
});

test('a lone section uses the bar title and keeps its actions and saved expansion', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const left = page.locator('[data-workbench-bar="left"]');
  const navigationTitle = left.locator('[data-container-title]');
  await expect(left.locator('[data-section-header]')).toHaveCount(0);
  await expect(navigationTitle.getByRole('button', { name: 'Navigation', exact: true })).toBeVisible();
  await expect(navigationTitle.getByRole('button', { name: 'Help: Navigation', exact: true })).toHaveCount(0);
  await expect(navigationTitle.locator('button[aria-haspopup="menu"]')).toHaveCount(1);
  await expect(navigationTitle.getByRole('button', { name: 'Navigation actions', exact: true })).toBeVisible();
  await expect(left.getByRole('tree', { name: 'Orgs, projects, topics and threads' })).toBeVisible();
  // The title remains a keyboard carry handle after the duplicate header is removed.
  await navigationTitle.getByRole('button', { name: 'Navigation', exact: true }).focus();
  await page.keyboard.press('Space');
  await expect(page.locator('[id^="DndLiveRegion"]')).toHaveText('Picked up Navigation.');
  await page.keyboard.press('Escape');
  await navigationTitle.getByRole('button', { name: 'Navigation actions', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Copy link', exact: true })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Hide', exact: true })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Move to', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');

  // Hiding the second section merges the first; showing it restores its saved closed state.
  await right(page).getByRole('button', { name: 'Conversation actions', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'History', exact: true }).click();
  await expect(right(page).locator('[data-section-header]')).toHaveCount(0);
  const title = right(page).locator('[data-container-title]');
  await expect(title.getByRole('button', { name: 'Help: Outline', exact: true })).toHaveCount(0);
  await expect(right(page).locator('[data-section-view="outline"]')).toBeVisible();
  await title.getByRole('button', { name: 'Add a view', exact: true }).click();
  await page.getByRole('dialog', { name: 'Views' }).getByRole('option', { name: /^Outline/ }).click();
  await expect(right(page).locator('[data-section-view="outline"]')).toBeFocused();
  await title.getByRole('button', { name: 'Conversation actions', exact: true }).click();
  await page.getByRole('menuitemcheckbox', { name: 'History', exact: true }).click();
  await expect(right(page).locator('[data-section-header]')).toHaveCount(2);
  await expect(section(page, 'History')).toHaveAttribute('aria-expanded', 'false');
  await expect(title.getByRole('button', { name: 'Help: Outline', exact: true })).toHaveCount(0);
});

test('container icons keep their names and show labels again when the bar has room', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const names = ['Conversation', 'Team', 'Code', 'Environment', 'Agent'];
  await expect(right(page).getByRole('tablist', { name: 'View containers' }).getByRole('tab')).toHaveCount(names.length);
  for (const name of names) {
    const tab = right(page).getByRole('tab', { name, exact: true });
    await expect(tab).toHaveAccessibleName(name);
    await expect(tab).toHaveText(name === 'Conversation' ? name : '');
    await tab.hover();
    await expect(page.getByRole('tooltip')).toHaveText(name);
  }
  await page.mouse.move(0, 0);
  await page.setViewportSize({ width: 1920, height: 1000 });
  const border = page.getByRole('separator', { name: 'Width of the right bar', exact: true });
  await border.press('Home');
  for (const name of names) await expect(right(page).getByRole('tab', { name, exact: true })).toHaveText(name);
  await expect(right(page).getByRole('button', { name: 'More', exact: true })).toHaveCount(0);
  await expectStripControlsInside(page);
  await border.press('End');
  for (const name of names) await expect(right(page).getByRole('tab', { name, exact: true })).toHaveText(name === 'Conversation' ? name : '');
  await expectStripControlsInside(page);
});

for (const width of [400, 1440]) {
  test(`viewer launch choices have compact state labels without captions or per-option guides at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await signIn(page);
    await page.goto(`/thread/${ACME.mainThread}`);
    const environment = async () => {
      if (width === 400) {
        await page.locator('[data-narrow-containers]').getByRole('button', { name: 'Environment', exact: true }).click();
        return page.getByRole('dialog', { name: 'Environment', exact: true });
      }
      await chooseContainer(page, 'Environment');
      await page.getByRole('separator', { name: 'Width of the right bar', exact: true }).press('End');
      return right(page);
    };
    for (const [sectionName, kind, label, viewer] of [['Browsers', 'browser', 'notion · Open', 'Browser'], ['Displays', 'display', 'Desktop · Open', 'VNC']] as const) {
      const bar = await environment();
      const header = bar.getByRole('button', { name: sectionName, exact: true });
      if (await header.getAttribute('aria-expanded') === 'false') await header.click();
      const body = bar.locator(`[data-section-view="${kind === 'browser' ? 'browsers' : 'displays'}"]`);
      const target = body.getByRole('button', { name: label, exact: true });
      await expect(target).toBeVisible();
      await expect(target).toHaveText(label);
      await expect(body.locator('[data-choice-description], [data-choice-help]')).toHaveCount(0);
      const shape = await target.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const item = element.closest('li')!.getBoundingClientRect();
        const root = getComputedStyle(document.documentElement);
        const token = root.getPropertyValue('--control-sm').trim();
        const height = parseFloat(token) * (token.endsWith('rem') ? parseFloat(root.fontSize) : 1);
        return { height: bounds.height, ordinary: height, width: bounds.width, available: item.width, textFits: element.scrollWidth <= element.clientWidth + 1 };
      });
      expect(Math.abs(shape.height - shape.ordinary)).toBeLessThanOrEqual(1);
      expect(shape.width).toBeLessThan(shape.available);
      expect(shape.textFits).toBe(true);
      if (kind === 'browser') await target.click();
      else { await target.focus(); await target.press('Enter'); }
      if (width === 400) {
        const opened = page.getByRole('dialog', { name: viewer, exact: true });
        await expect(opened).toBeVisible();
        await opened.getByRole('button', { name: 'Close', exact: true }).first().click();
      } else {
        await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: viewer, exact: true })).toHaveAttribute('aria-selected', 'true');
      }
    }
  });
}

test('sections collapse, hide and return with their saved choices', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const history = section(page, 'History');
  await expect(history).toHaveAttribute('aria-expanded', 'false');
  await history.click();
  await expect(history).toHaveAttribute('aria-expanded', 'true');
  await right(page).getByRole('button', { name: 'Conversation actions' }).click();
  await page.getByRole('menuitemcheckbox', { name: 'History', exact: true }).click();
  await expect(history).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => {
    return Object.keys(localStorage).some((key) => {
      if (!key.endsWith('.workbench.thread')) return false;
      return localStorage.getItem(key)?.includes('\\"hidden\\":true') === true;
    });
  })).toBe(true);
  await page.reload();
  await expect(history).toHaveCount(0);
  await right(page).getByRole('button', { name: 'Conversation actions' }).click();
  const show = page.getByRole('menuitemcheckbox', { name: 'History', exact: true });
  await expect(show).toHaveAttribute('aria-checked', 'false');
  await show.click();
  await expect(history).toHaveAttribute('aria-expanded', 'true');
  await chooseContainer(page, 'Team');
  await expect(section(page, 'Chat')).toHaveAttribute('aria-expanded', 'true');
  await expect(section(page, 'Documents')).toBeVisible();
  await expect(section(page, 'People')).toBeVisible();
});

// Shared helpers are intentionally used by the width and keyboard cases below.
export { chooseContainer, expectInside };

test('Open view lists the screen views with their containers and reveals a hidden section with focus', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await right(page).getByRole('button', { name: 'Conversation actions' }).click();
  await page.getByRole('menuitemcheckbox', { name: 'Outline', exact: true }).click();
  await page.getByRole('button', { name: 'Right bar', exact: true }).click();
  await page.keyboard.press('Control+k');
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });
  await finder.getByRole('combobox').fill('> Open view');
  await finder.getByRole('option', { name: /^Open view…/ }).click();
  const chooser = page.getByRole('dialog', { name: 'Views' });
  await expect(chooser.getByRole('option', { name: /^Outline/ })).toContainText('Conversation');
  await expect(chooser.getByRole('option', { name: /^Documents/ })).toContainText('Team');
  await chooser.getByRole('combobox').fill('Outline');
  await chooser.getByRole('option', { name: /^Outline/ }).click();
  await expect(section(page, 'Outline')).toHaveAttribute('aria-expanded', 'true');
  await expect(section(page, 'Outline')).toBeFocused();
  await right(page).getByRole('button', { name: 'Add a view' }).click();
  await expect(page.getByRole('dialog', { name: 'Views' })).toBeVisible();
  await page.keyboard.press('Escape');
});

test('a section is dragged to another container and moved to the bottom from its menu', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await section(page, 'History').dragTo(right(page).getByRole('tab', { name: 'Code', exact: true }));
  await expect(right(page)).toHaveAttribute('data-active-container', 'code');
  await expect(section(page, 'History')).toBeVisible();
  await right(page).getByRole('button', { name: 'Actions for History' }).click();
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).hover();
  await page.getByRole('menuitem', { name: 'Bottom', exact: true }).click();
  await expect(page.getByRole('tablist', { name: 'Bottom', exact: true }).getByRole('tab', { name: 'History', exact: true })).toBeVisible();
  await expect(right(page).getByRole('button', { name: 'History', exact: true })).toHaveCount(0);
});

test('keyboard carrying a section can be cancelled and arrangements restore the same view set', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  await section(page, 'Outline').focus();
  await page.keyboard.press('Space');
  await expect(page.locator('[id^="DndLiveRegion"]')).toHaveText('Picked up Outline.');
  await page.keyboard.press('ArrowLeft');
  await expect(page.locator('[id^="DndLiveRegion"]')).toHaveText(/^Over /);
  await page.keyboard.press('Escape');
  await expect(section(page, 'Outline')).toBeVisible();
  for (const [arrangement, main] of [['Code', 'Files'], ['Review', 'Changes']] as const) {
    await page.getByRole('button', { name: 'Layout', exact: true }).click();
    await page.getByRole('menuitem', { name: arrangement, exact: true }).click();
    await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: main, exact: true })).toBeVisible();
    await expect(right(page).getByRole('button', { name: 'Main thread', exact: true })).toBeVisible();
  }
  await page.getByRole('button', { name: 'Layout', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Reset layout', exact: true }).click();
  await expect(page.getByRole('tablist', { name: 'Main area', exact: true }).getByRole('tab', { name: 'Main thread', exact: true })).toBeVisible();
  await expect(right(page)).toHaveAttribute('data-active-container', 'conversation');
});

test('a phone offers the desktop containers in order and draws their sections in sheets', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const controls = page.locator('[data-narrow-containers]');
  const names = await controls.locator('[data-container-name]').allTextContents();
  expect(names.map((name) => name.trim()).slice(0, 7)).toEqual(['Navigation', 'Resources', 'Conversation', 'Team', 'Code', 'Environment', 'Agent']);
  await controls.getByRole('button', { name: 'Team', exact: true }).click();
  const sheet = page.getByRole('dialog', { name: 'Team', exact: true });
  await expect(sheet.getByRole('button', { name: 'Chat', exact: true })).toHaveAttribute('aria-expanded', 'true');
  for (const name of ['Documents', 'People']) await expect(sheet.getByRole('button', { name, exact: true })).toBeVisible();
  await controls.getByRole('button', { name: 'Code', exact: true }).click();
  const code = page.getByRole('dialog', { name: 'Code', exact: true });
  await expect(code.getByRole('button', { name: 'Changes', exact: true })).toBeVisible();
  await expect(code.getByRole('button', { name: 'Files', exact: true })).toBeVisible();
  await code.getByRole('button', { name: 'Close', exact: true }).first().click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('button', { name: 'Navigation', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Navigation', exact: true }).getByRole('tree', { name: 'Orgs, projects, topics and threads' })).toBeVisible();
});

const THREAD_SECTIONS = [
  ['Conversation', ['Outline', 'History']],
  ['Team', ['Chat', 'Documents', 'People']],
  ['Code', ['Changes', 'Files', 'Git']],
  ['Environment', ['Overview', 'Processes', 'Resources', 'Ports', 'Browsers', 'Displays']],
  ['Agent', ['Parameters', 'Usage', 'Knowledge', 'Given to the agent', 'Diagnostics']],
] as const;

async function reachByTab(page: Page, target: Locator) {
  // One traversal of the actual page is a bound, independent of the number of sections or their actions.
  const stops = await page.locator('a[href],button:not([disabled]),input:not([disabled]),textarea:not([disabled]),select:not([disabled]),[tabindex="0"]').count();
  for (let step = 0; step <= stops; step += 1) {
    if (await target.evaluate((element) => element === document.activeElement)) return;
    await page.keyboard.press('Tab');
  }
  throw new Error(`Keyboard cannot reach ${await target.getAttribute('aria-label')}`);
}

async function expectStripControlsInside(page: Page) {
  const faults = await page.locator('[data-navigation-strip]').evaluateAll((strips) => strips.flatMap((strip) => {
    if (!strip.getClientRects().length) return [];
    const room = strip.getBoundingClientRect();
    return Array.from(strip.querySelectorAll<HTMLElement>('button,a[href],[role="tab"]')).filter((control) => {
      const style = getComputedStyle(control);
      return control.closest('[inert],[hidden],[aria-hidden="true"]') === null && control.getClientRects().length > 0 && style.visibility !== 'hidden';
    }).flatMap((control) => {
      const box = control.getBoundingClientRect();
      return box.left < room.left - 1 || box.right > room.right + 1 || box.top < room.top - 1 || box.bottom > room.bottom + 1 ? [control.getAttribute('aria-label') ?? (control.textContent || control.tagName)] : [];
    });
  }));
  expect(faults, 'Every primary strip control is visible inside its strip').toEqual([]);
}

for (const width of [1280, 1440, 1920, 'minimum bar'] as const) {
  test(`every thread container and section is reached by pointer and keyboard at ${width}`, async ({ page }) => {
    test.slow(); // This case opens every view twice, including the lazy bodies and their data.
    await page.setViewportSize({ width: typeof width === 'number' ? width : 1440, height: 1000 });
    await signIn(page);
    await page.goto(`/thread/${ACME.mainThread}`);
    if (width === 'minimum bar') {
      await page.getByRole('separator', { name: 'Width of the right bar', exact: true }).press('End');
      const minimum = await page.evaluate(() => {
        const style = getComputedStyle(document.documentElement);
        const value = style.getPropertyValue('--right-bar-min').trim();
        return parseFloat(value) * (value.endsWith('rem') ? parseFloat(style.fontSize) : 1);
      });
      await expect.poll(async () => Math.abs((await right(page).boundingBox())!.width - minimum)).toBeLessThanOrEqual(1);
    }
    for (const [container, names] of THREAD_SECTIONS) {
      await chooseContainer(page, container);
      for (const name of names) {
        const header = section(page, name);
        await expect(header).toBeVisible();
        if (await header.getAttribute('aria-expanded') === 'false') await header.click();
        await expect(header).toHaveAttribute('aria-expanded', 'true');
        await expectInside(header, right(page));
        await header.click();
      }
      await expectStripControlsInside(page);
    }
    await chooseContainer(page, 'Conversation');
    const selected = right(page).getByRole('tab', { name: 'Conversation', exact: true });
    await selected.focus();
    for (const [container, names] of THREAD_SECTIONS) {
      const tab = right(page).getByRole('tab', { name: container, exact: true });
      await expect(tab).toBeFocused();
      await expect(tab).toHaveAttribute('aria-selected', 'true');
      for (const name of names) {
        const header = section(page, name);
        await reachByTab(page, header);
        await page.keyboard.press('Enter');
        await expect(header).toHaveAttribute('aria-expanded', 'true');
        await page.keyboard.press('Enter');
        await expect(header).toHaveAttribute('aria-expanded', 'false');
      }
      await tab.focus();
      await tab.press('ArrowRight');
      await expectStripControlsInside(page);
    }
    // The terminal is a bottom-panel view, reached through the Environment container's labelled action.
    await chooseContainer(page, 'Environment');
    await right(page).getByRole('button', { name: 'Open terminal', exact: true }).click();
    await expect(page.getByRole('tablist', { name: 'Bottom', exact: true }).getByRole('tab', { name: 'Terminal', exact: true })).toBeVisible();
    await expectStripControlsInside(page);
  });
}
