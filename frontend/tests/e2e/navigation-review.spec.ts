/** Row facts stay whole in the shipping bar, flat table, phone cards and inline chat states. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });

async function shot(page: Page, name: string) {
  const directory = process.env.ORCH_NAV_SCREEN_DIR;
  if (directory !== undefined) await page.screenshot({ path: `${directory}/${name}.png` });
}
async function navigation(page: Page, width: number) {
  const bar = page.locator('[data-workbench-bar="left"]:visible');
  if (width === 400) {
    const control = page.locator('[data-container-choice="left:navigation"]');
    await expect(control).toBeVisible();
    if (!await bar.isVisible()) await control.click();
  } else {
    const control = page.getByRole('button', { name: 'Left bar', exact: true });
    await expect(control).toBeVisible();
    if (await control.getAttribute('aria-pressed') !== 'true') await control.click();
  }
  await expect(bar).toBeVisible(); return bar;
}
async function wholeTime(row: Locator, phone: boolean) {
  const clock = row.locator('[data-row-time] time');
  expect(await clock.textContent()).not.toContain('…');
  await expect(clock).toHaveCSS('white-space', 'nowrap');
  expect(await row.locator('[data-row-end]').getAttribute('title')).toMatch(/\d{4}.*:/);
  if (phone) {
    await expect(clock).toBeVisible();
    const time = await clock.boundingBox(), acts = await row.locator('[data-row-actions]').boundingBox();
    if (time === null || acts === null) throw new Error('A phone keeps both the time and its menu');
    expect(time.x + time.width).toBeLessThanOrEqual(acts.x);
  }
}
async function namesFirst(list: Locator) {
  for (const name of ['Web handbook', 'Checkout design', 'Idle-stop notes']) {
    const label = list.getByText(name, { exact: true });
    await expect(label).toBeVisible();
    expect(await label.evaluate((element) => element.scrollWidth <= element.clientWidth + 1), `${name} keeps its full width before the place`).toBe(true);
  }
}
async function longNameFits(bar: Locator) {
  expect(await bar.getByText('Web handbook', { exact: true }).evaluate((element) => {
    const link = element.closest('a');
    if (link === null) throw new Error('The shared row retains its link');
    const place = link.querySelector('[data-row-place-words]');
    if (place === null) throw new Error('The shared row retains its place');
    const original = element.textContent;
    try {
      element.textContent = original.repeat(20);
      const bounds = element.getBoundingClientRect(), available = link.getBoundingClientRect();
      return element.scrollWidth > element.clientWidth && bounds.right <= available.right + 1 && getComputedStyle(place).display === 'none';
    } finally { element.textContent = original; }
  })).toBe(true);
}
async function inlineState(bar: Locator, title: string, explanation: string | RegExp) {
  const heading = bar.getByText(title, { exact: true });
  const detail = bar.getByText(explanation, { exact: true });
  await expect(heading).toBeVisible(); await expect(detail).toBeVisible();
  // Read the moving sheet and both lines in one frame rather than comparing successive animation frames.
  const geometry = await heading.evaluate((element) => {
    const panel = element.closest('[data-workbench-bar="right"]');
    const description = element.parentElement?.lastElementChild;
    if (panel === null || description === undefined || description === null || description === element) return null;
    const rect = (node: Element) => { const box = node.getBoundingClientRect(); return { x: box.x, y: box.y, width: box.width, height: box.height }; };
    return { head: rect(element), text: rect(description), bounds: rect(panel), headingColor: getComputedStyle(element).color, detailColor: getComputedStyle(description).color };
  });
  if (geometry === null) throw new Error('The inline empty state must be laid out');
  const { head, text, bounds } = geometry;
  expect(text.y).toBeGreaterThanOrEqual(head.y + head.height - 1);
  expect(text.x + text.width).toBeLessThanOrEqual(bounds.x + bounds.width - 12);
  expect(text.x).toBeGreaterThanOrEqual(bounds.x + 12);
  expect(geometry.detailColor).not.toBe(geometry.headingColor);
}

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) {
  test(`whole compact row facts at ${width}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page, PEOPLE.vera); await page.goto(`/thread/${ACME.mainThread}`);
    const prefix = `review-${width}-${theme}`;
    const bar = await navigation(page, width);
    await shot(page, `${prefix}-tree`);
    await bar.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Recent', exact: true }).click();
    const field = bar.getByRole('searchbox', { name: 'Narrow this tree', exact: true });
    await expect(field).toHaveAttribute('placeholder', 'Narrow…');
    expect(await field.evaluate((element) => {
      const input = element as HTMLInputElement, style = getComputedStyle(input), canvas = document.createElement('canvas'), context = canvas.getContext('2d');
      if (context === null) throw new Error('The placeholder needs its actual font width');
      context.font = style.font;
      return context.measureText(input.placeholder).width <= input.clientWidth - Number.parseFloat(style.paddingLeft) - Number.parseFloat(style.paddingRight);
    })).toBe(true);
    const current = bar.locator(`[data-tree-row="thread:${ACME.mainThread}"]`);
    await expect(current).toBeVisible(); await wholeTime(current, width === 400);
    if (width === 1440) await expect(current.locator('[data-row-time]')).toBeHidden();
    await expect(current.locator('[data-row-actions]')).toBeVisible();
    await expect(current.locator('[data-row-place]')).toHaveText('Fix login');
    await shot(page, `${prefix}-opened`);
    await bar.getByRole('combobox', { name: 'Recent order' }).click();
    await page.getByRole('option', { name: 'Active', exact: true }).click();
    await expect(current.locator('time')).toHaveText('Oct 1');
    await namesFirst(bar);
    await longNameFits(bar);
    const another = bar.locator('[data-tree-row^="document:"]').first();
    await expect(another).toBeVisible();
    await page.mouse.move(width - 5, 10);
    await wholeTime(another, width === 400);
    if (width === 1440) await expect(another.locator('[data-row-time]')).toBeVisible();
    await another.hover();
    if (width === 1440) await expect(another.locator('[data-row-time]')).toBeHidden();
    else await wholeTime(another, true);
    await expect(another.locator('[data-row-actions]')).toBeVisible();
    const hints = await bar.locator('[data-row-place]').evaluateAll((elements) => elements.map((element) => {
      const words = element.querySelector('[data-row-place-words]'), canvas = document.createElement('canvas'), context = canvas.getContext('2d'), style = getComputedStyle(element);
      if (context === null || words === null) throw new Error('A place has a measurable label');
      context.font = style.font;
      return { width: element.getBoundingClientRect().width, minimum: context.measureText('000000').width, visible: getComputedStyle(words).display !== 'none', text: words.textContent };
    }));
    for (const hint of hints) {
      expect(hint.text).not.toContain('/');
      if (hint.width < hint.minimum - 1) expect(hint.visible).toBe(false);
    }
    await shot(page, `${prefix}-active-hover`);
    await bar.getByRole('button', { name: 'View options', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Open in Everything', exact: true }).click();
    if (width === 400 && await bar.isVisible()) await page.locator('[data-container-choice="left:navigation"]').click();
    const table = page.getByRole('treegrid', { name: 'Everything I can see' });
    const first = table.locator('[data-everything-row]').first();
    await expect(first).toBeVisible();
    await namesFirst(table);
    const viewport = page.getByRole('region', { name: 'Everything I can see', exact: true });
    if (width === 1440) {
      expect(await viewport.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
      await expect(table.getByRole('columnheader', { name: 'Kind', exact: true })).toHaveCount(0);
      await expect(table.getByRole('columnheader', { name: 'Owner', exact: true })).toHaveCount(0);
      await expect(first.locator('.flat-field-label:visible')).toHaveCount(0);
      const path = first.locator('[data-row-path]');
      expect(await path.evaluate((element) => {
        const text = element.querySelector('bdi')?.firstChild;
        if (!(text instanceof Text)) throw new Error('A path has its actual nearest suffix');
        const range = document.createRange(); range.setStart(text, Math.max(0, text.length - 3)); range.setEnd(text, text.length);
        const bounds = element.getBoundingClientRect(), suffix = range.getBoundingClientRect();
        return suffix.x >= bounds.x - 1 && suffix.right <= bounds.right + 1;
      })).toBe(true);
    } else {
      expect((await first.boundingBox())?.height).toBeLessThanOrEqual(120);
      for (const label of ['Where', 'Last activity', 'Owner', 'Kind']) await expect(first.getByText(label, { exact: true })).not.toBeVisible();
      await expect(first.getByText('By', { exact: true })).toBeVisible();
      await expect(first.locator('[data-mobile-place]')).not.toContainText('·');
    }
    const icon = first.locator('.flat-name svg').first(), frame = await table.boundingBox(), glyph = await icon.boundingBox();
    if (frame === null || glyph === null) throw new Error('Flat row icon must be visible');
    expect(glyph.x - frame.x).toBeLessThanOrEqual(20);
    await shot(page, `${prefix}-recent-default`);
    const main = page.locator('[data-main-area]');
    await main.getByRole('button', { name: 'Select', exact: true }).click();
    await first.getByRole('checkbox').check();
    await namesFirst(table);
    if (width === 1440) expect(await viewport.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    const tick = await first.getByRole('checkbox').boundingBox(), selectedIcon = await icon.boundingBox();
    if (tick === null || selectedIcon === null) throw new Error('The selected flat row retains its icon');
    expect(selectedIcon.x - (tick.x + tick.width)).toBeLessThanOrEqual(20);
    await shot(page, `${prefix}-recent-selection`);
    await main.getByRole('button', { name: 'Select', exact: true }).click();
    for (const name of ['Kind', 'Owner']) {
      await main.getByRole('button', { name: 'Columns', exact: true }).click();
      const option = page.getByRole('menuitemcheckbox', { name, exact: true });
      await expect(option).toHaveAttribute('aria-checked', 'false');
      await option.click();
      await main.getByRole('button', { name: 'Columns', exact: true }).click();
      await expect(option).toHaveAttribute('aria-checked', 'true');
      await page.keyboard.press('Escape');
    }
    if (width === 1440) {
      const rail = page.locator('[data-visible-scrollbar="horizontal"]');
      await expect(rail).toBeVisible(); expect((await rail.boundingBox())?.height).toBeGreaterThanOrEqual(10);
      await viewport.evaluate((element) => { element.scrollLeft = element.scrollWidth; });
      await expect(table.getByRole('columnheader', { name: 'Kind', exact: true })).toBeVisible();
      const viewportBounds = await viewport.boundingBox(), thumbBounds = await rail.locator(':scope > div').boundingBox();
      if (viewportBounds === null || thumbBounds === null) throw new Error('The visible scrollbar thumb stays inside its frame');
      expect(thumbBounds.x).toBeGreaterThanOrEqual(viewportBounds.x - 1);
      expect(thumbBounds.x + thumbBounds.width).toBeLessThanOrEqual(viewportBounds.x + viewportBounds.width + 1);
      await expect(first.locator('.flat-field-label:visible')).toHaveCount(0);
    } else {
      await expect(first.getByText('Kind', { exact: true })).toBeVisible();
      await expect(first.getByText('Owner', { exact: true })).toBeVisible();
    }
    await shot(page, `${prefix}-explicit-columns`);
    if (width === 400) await page.locator('[data-container-choice="right:team"]').click();
    const right = page.locator('[data-workbench-bar="right"]:visible');
    await inlineState(right, 'No chat on this screen', 'A chat belongs to an org, a project, a topic or a thread.');
    await shot(page, `${prefix}-empty-chat`);
    if (width === 400) await page.keyboard.press('Escape');
    await page.goto(`/thread/${ACME.mainThread}`);
    if (width === 400 && !await right.isVisible()) await page.locator('[data-container-choice="right:team"]').click();
    await inlineState(right, 'No messages yet', /^Say something to the people on this (thread|topic|project)\.$/);
    await shot(page, `${prefix}-empty-conversation`);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}
