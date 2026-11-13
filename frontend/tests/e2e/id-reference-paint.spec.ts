/** An `id:` reference an agent wrote is a chip from the first paint: the words as written in the box of the chip they will become, no link and
 * no button and nothing to announce until the questions are answered; it then becomes the chip of what it names, or the chip that says no
 * access, without the line moving when the words are the same, and moving once when the name is another. The answers are held at the
 * server, so the page is looked at between the questions and the answers, not left to the order they come in. */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { agentMessage, threadInput, turn, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { DOCS, held } from './documents-support.ts';
import { ACME, emit, PEOPLE, scenario, signIn } from './support.ts';

const MAIN = ACME.mainThread;
const AT = '2026-10-14T10:00:00.000Z';

/** Something to wait on that the test opens when it chooses. */
function gate() {
  let open!: () => void;
  const opened = new Promise<void>((resolve) => { open = resolve; });
  return { opened, open };
}

const conversation = (page: Page) => page.getByRole('log', { name: 'Conversation' });
/** The boxes of the references in the answer, in the order they were written. */
const boxes = (page: Page) => conversation(page).locator('[data-chip-size]');
const resolving = (page: Page) => conversation(page).locator('[data-reference-state="resolving"]');

interface Box { x: number; y: number; width: number; height: number }
/** The box of a reference, its place taken from the paragraph it stands in, so that what moves round the paragraph does not count. */
async function boxOf(chip: Locator): Promise<Box> {
  return chip.evaluate((element) => {
    const at = element.getBoundingClientRect();
    const paragraph = element.closest('p')?.getBoundingClientRect();
    return { x: at.left - (paragraph?.left ?? 0), y: at.top - (paragraph?.top ?? 0), width: at.width, height: at.height };
  });
}
/** The box as it stays: read twice a frame apart until the two agree. */
async function settled(chip: Locator): Promise<Box> {
  let last = await boxOf(chip);
  for (let tries = 0; tries < 20; tries += 1) {
    await chip.page().evaluate(() => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))));
    const now = await boxOf(chip);
    if (now.x === last.x && now.y === last.y && now.width === last.width && now.height === last.height) return now;
    last = now;
  }
  return last;
}

/** The thread open at the answer, with the preview questions held until the gate it returns is opened. */
async function open(page: Page, width: number, text: string) {
  await page.setViewportSize({ width, height: 900 });
  await emit(MAIN, {
    items: [userMessage('u1', 'Where is it written down?', { author: ADA, turn: 't1', at: AT }), agentMessage('a1', text, { turn: 't1', at: AT, phase: 'final' })],
    inputs: [threadInput('in-1', 'Where is it written down?', { author: ADA, position: '1', state: 'delivered' })],
    turns: [turn('t1', { ordinal: 1, state: 'completed', inputId: 'in-1', startedAt: AT, completedAt: AT })],
  });
  const answers = gate();
  await page.route(/\/references\/preview\?/, async (route) => { await answers.opened; await route.continue(); });
  // Vera reads the plan and may not read the private page: its reference is answered "no access".
  await signIn(page, PEOPLE.vera);
  await page.goto(`/thread/${MAIN}`);
  return answers;
}

for (const width of [1440, 400]) {
  test(`a reference is a chip from the first paint and keeps the width of its box when it is answered (${width})`, async ({ page }) => {
    await scenario();
    const plan = (await held(DOCS.plan)).title;
    const answers = await open(page, width, `Written down in [${plan}](id:${DOCS.plan}), not in [my own words](id:${DOCS.private}), told in [Main thread](id:${MAIN}), and not in [the old name](id:${DOCS.plan}).`);

    // Before any answer: four chips with the words as written, none of them a link or a button.
    await expect(boxes(page)).toHaveCount(4);
    await expect(resolving(page)).toHaveCount(4);
    await expect(resolving(page).nth(0)).toHaveText(plan);
    await expect(resolving(page).nth(1)).toHaveText('my own words');
    await expect(resolving(page).nth(2)).toHaveText('Main thread');
    await expect(resolving(page).nth(3)).toHaveText('the old name');
    await expect(conversation(page).getByRole('link')).toHaveCount(0);
    await expect(conversation(page).getByRole('button', { name: 'my own words', exact: true })).toHaveCount(0);
    const before = [await settled(boxes(page).nth(0)), await settled(boxes(page).nth(1)), await settled(boxes(page).nth(2)), await settled(boxes(page).nth(3))];

    answers.open();
    await expect(resolving(page)).toHaveCount(0);
    await expect(conversation(page).getByRole('link', { name: plan, exact: true })).toHaveCount(2);
    await expect(conversation(page).getByRole('link', { name: 'Main thread', exact: true })).toHaveCount(1);
    await expect(conversation(page).getByRole('button', { name: 'my own words', exact: true })).toHaveAttribute('data-reference-state', 'no-access');
    const after = [await settled(boxes(page).nth(0)), await settled(boxes(page).nth(1)), await settled(boxes(page).nth(2)), await settled(boxes(page).nth(3))];

    // A readable reference whose name is the words written (a page and a thread), and one that is unreadable: the same box within a pixel, in the same place.
    for (const index of [0, 1, 2]) {
      expect(Math.abs(after[index]!.width - before[index]!.width), `width of reference ${index}`).toBeLessThanOrEqual(1);
      expect(Math.abs(after[index]!.height - before[index]!.height), `height of reference ${index}`).toBeLessThanOrEqual(1);
      expect(Math.abs(after[index]!.x - before[index]!.x), `left of reference ${index}`).toBeLessThanOrEqual(1);
      expect(Math.abs(after[index]!.y - before[index]!.y), `top of reference ${index}`).toBeLessThanOrEqual(1);
    }
  });
}
