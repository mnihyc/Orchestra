/** A floating layer whose content arrives after it has opened (a list read from the server, a card's body) takes the room it has then,
 * low on the screen, wide and narrow: it is not left cut at the room measured when it mounted, nor as a sliver of what it holds. */
import { writeFileSync } from 'node:fs';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { LATE_KINDS, type LateKind } from './floating-cases';
import { scenario, signIn } from './support.ts';

const TEXT = 'A complete long name with all its final words';
/** The field's lower edge is some 70 px above the screen's: room for one row below it, a whole screen above. */
const LEFT = 8, TOP = 770;
/** The room a layer keeps from the screen's edge and from what opens it. */
const EDGE = 8, GAP = 6;

/** What a kind of layer is meant never to pass: an intent card shows 22rem (352 px at the root's size) at most. */
const MOST: Partial<Record<LateKind, number>> = { 'late-intent': 352 };

interface Shown { top: number; bottom: number; visible: number; natural: number; hidden: number; room: number; side: string }

/** What a layer shows of its content, against the room round what opened it: `hidden` is what its own height holds back. */
async function measure(layer: Locator, anchor: { x: number; y: number; width: number; height: number }, beside: boolean): Promise<Shown> {
  return layer.evaluate((element, { anchor, beside, edge, gap }) => {
    const box = element.getBoundingClientRect();
    // What the layer's own height holds back: of the layer itself, or of the outermost parts inside it that scroll with no height of their own
    // (a list that holds ten rows however much room there is, is the layer's design and not its room).
    let hidden = 0;
    const scrolls = (part: HTMLElement) => ['auto', 'scroll'].includes(getComputedStyle(part).overflowY) && part.scrollHeight > part.clientHeight + 1;
    const walk = (parent: HTMLElement) => {
      for (const child of Array.from(parent.children)) {
        if (!(child instanceof HTMLElement)) continue;
        if (getComputedStyle(child).overflowY !== 'visible' && getComputedStyle(child).maxHeight === 'none' && scrolls(child)) hidden += child.scrollHeight - child.clientHeight; else walk(child);
      }
    };
    if (scrolls(element as HTMLElement)) hidden = element.scrollHeight - element.clientHeight; else walk(element as HTMLElement);
    // Above and below what opened it; a layer beside it may run from its upper edge down or from its lower edge up.
    const up = beside ? anchor.y + anchor.height - edge : anchor.y - edge - gap;
    const down = beside ? innerHeight - anchor.y - edge : innerHeight - (anchor.y + anchor.height) - edge - gap;
    return { top: box.top, bottom: box.bottom, visible: box.height, natural: box.height + hidden, hidden, room: Math.max(up, down), side: element.getAttribute('data-side') ?? '' };
  }, { anchor, beside, edge: EDGE, gap: GAP });
}

/** Opens one kind of layer and waits for its content to have arrived. */
async function open(page: Page, kind: LateKind): Promise<{ layer: Locator; anchor: Locator; beside: boolean }> {
  const example = page.locator('[data-floating-example]');
  const button = example.getByRole('button').first();
  const wrapped = page.locator('[data-radix-popper-content-wrapper] > :visible').last();
  switch (kind) {
    case 'late-popover': case 'late-detail': case 'late-detail-hover': case 'late-intent':
      // A card of a name also opens, passing, under the pointer.
      if (kind === 'late-detail-hover') await button.hover(); else await button.click();
      await expect(page.getByText(`${TEXT} 90`, { exact: true })).toBeAttached();
      return { layer: wrapped, anchor: button, beside: false };
    case 'late-reference': case 'late-reference-hover':
      if (kind === 'late-reference-hover') await button.hover(); else await button.click();
      await expect(page.locator('[data-reference-card]:visible').getByText('Fix login').first()).toBeVisible();
      return { layer: page.locator('[data-reference-card]:visible').last(), anchor: button, beside: false };
    case 'late-holder':
      // By the keyboard: a pointer resting on the chip asks for its people before it is opened.
      await button.focus();
      await page.keyboard.press('Enter');
      await expect(page.getByRole('dialog').getByText(`${TEXT} 1`, { exact: true })).toBeVisible();
      return { layer: wrapped, anchor: button, beside: false };
    case 'late-menu':
      await button.click();
      await expect(page.getByRole('menuitem', { name: `${TEXT} 80`, exact: true })).toBeAttached();
      return { layer: page.getByRole('menu').last(), anchor: button, beside: false };
    case 'late-submenu': {
      await button.click();
      const parent = page.getByRole('menuitem', { name: 'Nested layer', exact: true });
      await parent.hover();
      await expect(page.getByRole('menu')).toHaveCount(2);
      await expect(page.getByRole('menuitem', { name: `${TEXT} 80`, exact: true })).toBeAttached();
      return { layer: page.getByRole('menu').last(), anchor: parent, beside: true };
    }
    case 'late-context':
      await button.click({ button: 'right' });
      await expect(page.getByRole('menuitem', { name: `${TEXT} 80`, exact: true })).toBeAttached();
      return { layer: page.getByRole('menu').last(), anchor: button, beside: false };
    case 'late-select': {
      // An open select hides everything else from the accessibility tree, its own field too.
      const field = example.locator('[role="combobox"]');
      await field.click({ position: { x: 10, y: 10 } });
      await expect(page.getByRole('option', { name: `${TEXT} 80`, exact: true })).toBeAttached();
      return { layer: wrapped, anchor: field, beside: false };
    }
  }
}

for (const width of [400, 1440]) for (const kind of LATE_KINDS) {
  test(`${kind} takes the room it has when its content arrives (${width})`, { tag: '@development' }, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    // What a card reads is answered late, as a slow server answers.
    await page.route('**/api/v1/references/preview*', async (route) => { await new Promise((done) => setTimeout(done, 600)); await route.continue(); });
    await signIn(page);
    await page.evaluate(async ({ kind, left, top, text }) => {
      const module = '/tests/component/floating-harness.tsx';
      const { mountFloating } = await import(/* @vite-ignore */ module) as { mountFloating: (kind: string, left: number, top: number, text: string) => Promise<void> };
      await mountFloating(kind, left, top, text);
    }, { kind, left: LEFT, top: TOP, text: TEXT });
    const { layer, anchor, beside } = await open(page, kind);
    // Let the layer settle where it puts itself.
    await page.evaluate(() => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))));
    await page.waitForTimeout(150);
    const box = await anchor.boundingBox();
    if (box === null) throw new Error('What opened the layer is not on the screen');
    const shown = await measure(layer, box, beside);
    // Kept beside the case's output, for a person to look at, whether the case passes or not.
    writeFileSync(info.outputPath(`${kind}-${width}.json`), JSON.stringify(shown, null, 2));
    await page.screenshot({ path: info.outputPath(`${kind}-${width}.png`) });
    expect(shown.top, `${kind} stays on the screen`).toBeGreaterThanOrEqual(-1);
    expect(shown.bottom, `${kind} stays on the screen`).toBeLessThanOrEqual(901);
    // Whatever its content holds, the layer shows all of it or all the room there is on either side of what opened it.
    expect(shown.visible, `${kind} shows ${Math.round(shown.visible)} of ${Math.round(shown.natural)} px where ${Math.round(shown.room)} px were to be had`).toBeGreaterThanOrEqual(Math.min(shown.natural, shown.room, MOST[kind] ?? Infinity) - 2);
  });
}
