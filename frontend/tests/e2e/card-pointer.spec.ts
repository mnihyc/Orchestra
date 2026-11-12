/** A card opens because a person pointed at its trigger: the pointer moved onto it, or moved on it and rested. A trigger that
 * arrives under a pointer that did not move (a dialog that closes, a list that scrolls) opens no card, and so covers nothing
 * that the person is reaching for. Leaving to anything but the card closes it, and Escape does. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { chats } from './chat-support.ts';
import { DOCS } from './documents-support.ts';
import { workspace } from './inspector-support.ts';
import { ACME, emit, scenario, signIn } from './support.ts';

/** A card waits 700 ms of rest before it opens; a card that is not there after this long is not coming. */
const PAST_THE_REST_MS = 1_500;

const cards = (page: Page) => page.locator('[data-inspection-card]:visible, [data-reference-card]:visible');

/** The pointer's own state: the trigger is under it, though it did not move to get there. */
async function underPointer(trigger: Locator) {
  await expect.poll(() => trigger.evaluate((element) => element.matches(':hover'))).toBe(true);
}

async function passTheRest(page: Page) {
  await page.evaluate((ms) => new Promise<void>((resolve) => window.setTimeout(resolve, ms)), PAST_THE_REST_MS);
}

/** Puts a thread on the screen with its Terminal view open by the same dialog a person uses, the pointer left on the option. */
async function chooseTerminalByMouse(page: Page) {
  await signIn(page);
  await workspace('/reset');
  await emit(ACME.mainThread, { thread: { state: 'idle', directory: 'web' } });
  await page.goto(`/thread/${ACME.mainThread}`);
  await page.getByRole('button', { name: /^(Add a view|添加视图)$/ }).first().click();
  const chooser = page.getByRole('dialog', { name: /^(Views|视图)$/ });
  const option = chooser.locator('[role="option"][data-value="commands:view.open.terminal"]');
  await option.scrollIntoViewIfNeeded();
  await option.click();
  await expect(chooser).toHaveCount(0);
}

test('a card does not open for a chip that comes under the pointer when a dialog closes, and the control beside it can be pressed', async ({ page }) => {
  // Where the chip comes to stand under the option's place depends on the screen: this is the shipped test size.
  await page.setViewportSize({ width: 1280, height: 720 });
  await scenario();
  await chooseTerminalByMouse(page);
  const chip = page.getByRole('button', { name: 'Codex', exact: true }).first();
  await underPointer(chip);
  await passTheRest(page);
  await expect(cards(page)).toHaveCount(0);
  await page.getByRole('button', { name: 'Open a new shell', exact: true }).first().click();
  await expect(page.locator('.xterm-rows')).toContainText('$');
});

test('the card opens when the pointer then moves on the chip, closes when it leaves, and Escape closes it as well', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  await scenario();
  await chooseTerminalByMouse(page);
  const chip = page.getByRole('button', { name: 'Codex', exact: true }).first();
  await underPointer(chip);
  const at = await chip.boundingBox();
  if (at === null) throw new Error('The chip has no box.');
  // The person moves the pointer on the chip: now it is pointed at, and its card follows.
  await page.mouse.move(at.x + at.width / 2 + 3, at.y + at.height / 2 + 1);
  await expect(cards(page)).toHaveCount(1);
  // Leaving to anything but the card closes it after the short grace.
  await page.mouse.move(8, 8, { steps: 4 });
  await expect(cards(page)).toHaveCount(0);
  // Pointed at again, it opens again, and Escape closes it while the pointer stays.
  await page.mouse.move(at.x + at.width / 2, at.y + at.height / 2, { steps: 4 });
  await expect(cards(page)).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(cards(page)).toHaveCount(0);
});

/** The list scrolls by the wheel while the pointer rests: the chip that was below comes to the pointer, which did not move. */
async function chipComesUnderPointer(page: Page, chip: Locator) {
  const scroller = await chip.evaluateHandle((element) => {
    for (let at = element.parentElement; at !== null; at = at.parentElement) {
      const overflow = getComputedStyle(at).overflowY;
      if ((overflow === 'auto' || overflow === 'scroll') && at.scrollHeight > at.clientHeight) return at;
    }
    return document.scrollingElement as HTMLElement;
  });
  const frame = await scroller.evaluate((element) => { const box = element.getBoundingClientRect(); return { x: box.x + box.width / 2, y: box.y + box.height / 2 }; });
  const scrolled = (to: 'end' | 'top') => expect.poll(() => scroller.evaluate((element, where) => where === 'top' ? element.scrollTop === 0 : element.scrollHeight - element.scrollTop - element.clientHeight < 2, to)).toBe(true);
  await page.mouse.move(frame.x, frame.y);
  await page.mouse.wheel(0, 100_000);
  await scrolled('end');
  const box = await chip.boundingBox();
  if (box === null) throw new Error('The chip has no box.');
  const point = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  await page.mouse.wheel(0, -100_000);
  await scrolled('top');
  // The pointer comes to rest where the chip will be, over something else.
  await page.mouse.move(point.x - 30, point.y);
  await page.mouse.move(point.x, point.y);
  await page.mouse.wheel(0, 100_000);
  await scrolled('end');
  await underPointer(chip);
  await passTheRest(page);
  return point;
}

test('a person chip that comes under the pointer when the list scrolls opens no card until the pointer moves on it', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await scenario();
  const seeded = await chats('/seed');
  for (let n = 0; n < 14; n += 1) await chats('/say', { chat: seeded.chat, as: n % 2 === 0 ? 'dora' : 'grace', text: `Filler line ${n} that takes a row of the conversation` });
  await chats('/say', { chat: seeded.chat, as: 'linus', text: 'The last word before the end of the list' });
  await signIn(page);
  await page.goto(`/org/acme/messages?chat=${String(seeded.chat)}`);
  const view = page.locator(`[data-chat-view="${String(seeded.chat)}"]`);
  const chip = view.locator('li[data-message]').filter({ hasText: 'Filler line 11' }).first().getByRole('button', { name: /Grace Hopper/ }).first();
  await expect(chip).toBeVisible();
  const point = await chipComesUnderPointer(page, chip);
  await expect(cards(page)).toHaveCount(0);
  await page.mouse.move(point.x + 2, point.y + 1);
  await expect(cards(page)).toHaveCount(1);
});

test('a reference chip that comes under the pointer when the list scrolls opens no card until the pointer moves on it', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await scenario();
  for (let n = 0; n < 24; n += 1) await chats('/item', { thread: ACME.mainThread, kind: 'agent_message', text: `Filler line ${n} that takes a row of the thread's timeline` });
  const item = await chats('/item', { thread: ACME.mainThread, kind: 'agent_message', text: `Read [the plan](http://127.0.0.1:${process.env.ORCH_E2E_PORT ?? '41872'}/doc/${DOCS.notes}) now` });
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}`);
  const chip = page.locator(`[data-item="${String(item.id)}"]`).first().locator(`a[href="/doc/${DOCS.notes}"]`);
  await expect(chip).toBeVisible();
  const point = await chipComesUnderPointer(page, chip);
  await expect(cards(page)).toHaveCount(0);
  await page.mouse.move(point.x + 2, point.y + 1);
  await expect(cards(page)).toHaveCount(1);
});
