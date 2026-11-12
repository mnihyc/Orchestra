/** Every state of the thread screen, asked in both themes whether anything on it is a serious barrier: a conversation that is over, one that runs, one that waits for an answer, a turn that failed and one that was stopped, a thread that is blocked, a reader who may only watch, a queue and its menu, and the dialog that makes a thread. */
import { expect, type Page, test } from '@playwright/test';

import { agentMessage, threadInput, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, emit, expectAccessibleInBothThemes, PEOPLE, playScene, scenario, signIn, mainArea } from '../e2e/support.ts';

const MAIN = ACME.mainThread;

test.beforeEach(async () => {
  await scenario();
  await emit(MAIN, { thread: { state: 'idle' } });
});

const conversation = (page: Page) => page.getByRole('log', { name: 'Conversation' });

async function open(page: Page, person: { login: string; password: string } = PEOPLE.ada, path = `/thread/${MAIN}`): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

test('a conversation that is over: messages, work folded and shown, a change with its difference, and the footer', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page);
  await expect(conversation(page).getByText('The redirect now falls back to the home page when')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The work of the first turn opened too, and the whole of it at the deepest depth.
  await conversation(page).getByRole('button', { name: /Read 1 file, searched 1 time/ }).click();
  await expect(conversation(page).getByText('Searched for “redirect” in src')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a conversation that runs: a thought, a command with its output, text that is being written, and a way to stop it', async ({ page }) => {
  await playScene(MAIN, 'running');
  await open(page);
  await expect(page.getByRole('group', { name: 'Message controls', exact: true }).getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  // "Running for" belongs to the separate Processes view; the conversation owns a named live clock.
  const elapsed = conversation(page).getByRole('button', { name: 'Elapsed time', exact: true });
  await expect(elapsed).toBeVisible();
  await expect(elapsed).toHaveText(/\d/);
  await expectAccessibleInBothThemes(page);
});

test('a request for approval, with what it asks and the ways to answer', async ({ page }) => {
  await playScene(MAIN, 'waiting');
  await open(page);
  await expect(page.getByRole('button', { name: 'Allow once · npm install left-pad', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a turn that failed with its error and one that was stopped', async ({ page }) => {
  await playScene(MAIN, 'failures');
  await open(page);
  // A refusal is said in the reader's language from its code; the vendor's own words stay with the item.
  await expect(conversation(page).getByText('The provider refused this request.').first()).toBeVisible();
  await expect(conversation(page).getByText(/^Stopped by Ada Lovelace/).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('what waits in the queue, and the ways of sending that the button opens', async ({ page }) => {
  await playScene(MAIN, 'running');
  await open(page);
  await expect(page.getByRole('group', { name: 'Message controls', exact: true }).getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await emit(MAIN, {
    inputs: [threadInput('q1', 'After that, check the logout page', { author: ADA, position: '4' })],
    items: [agentMessage('extra', 'Still going', { turn: 't3', status: 'in_progress', phase: 'commentary' })],
  });
  await expect(page.getByRole('region', { name: /^Queued/ }).getByText('After that, check the logout page')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('textbox', { name: 'Message to the agent' }).fill('Use the helper');
  await page.getByRole('button', { name: 'More ways to send' }).click();
  await expect(page.getByRole('menuitem', { name: /Steer now/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
});

test('a thread that is blocked and says why', async ({ page }) => {
  await open(page, PEOPLE.ada, `/thread/${ACME.idleThread}`);
  await expect(page.getByText('Claude subscription has reached its limit')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a reader who may only watch: the conversation, no box to write in, and what they would need', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page, PEOPLE.vera);
  // The box stays where it is, closed, and says why; what they would need is beside it with the way to ask.
  const box = page.getByRole('textbox', { name: 'Message to the agent' });
  await expect(box).toBeDisabled();
  await expect(box).toHaveAttribute('placeholder', 'You can watch this thread');
  await expect(page.getByRole('group', { name: 'Access needed' }).getByRole('button', { name: 'Request access' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a thread with nothing in it yet, and one that has a first message waiting', async ({ page }) => {
  await open(page);
  await expect(page.getByText('Send the first message')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await emit(MAIN, { items: [userMessage('u1', 'A first message', { author: ADA, turn: 't1', inputId: 'in-1' })] });
  await expect(conversation(page).getByText('A first message')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the dialog that makes a thread, with its parts and its refusal', async ({ page }) => {
  await open(page, PEOPLE.ada, `/topic/${ACME.fixLogin}`);
  await mainArea(page).getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  const form = page.getByRole('dialog', { name: 'New thread in Fix login' });
  // The provider picker exposes its selection as the editable input's value.
  await expect(form.getByRole('combobox', { name: 'Provider' })).toHaveValue(/OpenAI key/);
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('items being picked, with the bar that stands in place of the box, and the number on an item that the chat quotes', async ({ page }) => {
  await playScene(MAIN, 'history');
  await emit(MAIN, { items: [{ ...agentMessage('a-quoted', 'This one is quoted in the chat.', { turn: null, at: new Date().toISOString() }), chat_refs: 1 }] });
  await open(page);
  await expect(conversation(page).getByRole('button', { name: '1 chat message quotes this item' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The newest item is in view; the conversation draws only the items near what is shown.
  const first = conversation(page).locator('[data-item="a-quoted"]');
  await first.hover();
  await first.getByRole('button', { name: 'More acts on this item' }).click();
  await page.getByRole('menuitem', { name: 'Select', exact: true }).click();
  await expect(page.getByRole('toolbar', { name: 'Selected items' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});


test('conversation and controls share one keyboard viewport while the history stays virtual', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  // Six native items per turn: sixteen turns exceed the viewport but fit the 100-item opening.
  await playScene(MAIN, 'long', 96); await open(page);
  const viewport = page.locator('[data-screen="thread"]'), log = conversation(page);
  await expect(log).toBeVisible();
  await expect.poll(() => viewport.evaluate((element) => element.scrollHeight - element.clientHeight)).toBeGreaterThan(0);
  expect(await log.evaluate((element) => getComputedStyle(element).overflowY)).toBe('visible');
  const drawn = log.locator(':scope > ol > li[data-index]');
  await expect(drawn.first()).toBeVisible();
  const total = Number(await drawn.first().getAttribute('aria-setsize'));
  expect(await drawn.count()).toBeLessThan(total);
  await log.focus(); await page.keyboard.press('Home');
  await expect.poll(() => viewport.evaluate((element) => element.scrollTop)).toBe(0);
  await page.keyboard.press('End');
  await expect.poll(() => viewport.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toBeInViewport();
  const bounds = await viewport.boundingBox();
  if (bounds === null) throw new Error('The shared thread viewport is visible');
  expect(await log.locator('[data-item]').evaluateAll((items, room) => items.some((item) => {
    const box = item.getBoundingClientRect(); return box.bottom > room.y && box.top < room.y + room.height;
  }), bounds)).toBe(true);
});

// Frames come late on a slow machine, and what the reader did is reported to the page late with them. What the reader did must
// stand however late: the case makes the page late on purpose (a slowed processor, and a stalled page right after the key, in which
// the browser's smooth scroll runs on without the page), so that it does not depend on how fast the machine it runs on is. The
// list is opened afresh each time, for the rows at the top are measured for the first time when the reader first goes there.
for (const [rate, stall] of [[1, 300], [1, 700]] as const) test(`a reader's Home takes the shared viewport to the top and nothing takes them back down when the page is ${rate} times slow and stalled for ${stall} ms`, async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await playScene(MAIN, 'long', 96); await open(page);
  const viewport = page.locator('[data-screen="thread"]'), log = conversation(page);
  const slow = await page.context().newCDPSession(page);
  for (let attempt = 0; attempt < 1; attempt += 1) {
    await expect(log).toBeVisible();
    await expect.poll(() => viewport.evaluate((element) => element.scrollHeight - element.clientHeight)).toBeGreaterThan(1200);
    // The list has settled at its end: the hold on it is over and its rows are measured (thirty frames without a change).
    await expect(viewport).not.toHaveAttribute('aria-busy', 'true');
    await viewport.evaluate((element) => new Promise<void>((resolve) => {
      let frames = 0, extent = element.scrollHeight;
      const tick = () => { if (element.scrollHeight !== extent) { extent = element.scrollHeight; frames = 0; } if (++frames < 30) requestAnimationFrame(tick); else resolve(); };
      requestAnimationFrame(tick);
    }));
    await slow.send('Emulation.setCPUThrottlingRate', { rate });
    await log.focus(); await page.keyboard.press('Home');
    await page.evaluate((milliseconds) => { const until = performance.now() + milliseconds; while (performance.now() < until) { /* the page does nothing else */ } }, stall);
    await expect.poll(() => viewport.evaluate((element) => element.scrollTop), { timeout: 60_000 }).toBe(0);
    // The rows that come into view are measured after the key has been seen: a hundred frames, however slow, never move the reader.
    const highest = await viewport.evaluate((element) => new Promise<number>((resolve) => {
      let frames = 0, most = 0;
      const tick = () => { most = Math.max(most, element.scrollTop); if (++frames < 100) requestAnimationFrame(tick); else resolve(most); };
      requestAnimationFrame(tick);
    }));
    expect(highest, `attempt ${attempt + 1}`).toBe(0);
    await slow.send('Emulation.setCPUThrottlingRate', { rate: 1 });
  }
});

for (const width of [1440, 400]) test(`a reader who scrolled up is shown where the new rows are: the jump stays in view and takes them to the end at ${width}px`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  await playScene(MAIN, 'long', 96); await open(page);
  const viewport = page.locator('[data-screen="thread"]'), log = conversation(page);
  await expect(log).toBeVisible();
  const distanceFromEnd = () => viewport.evaluate((element) => element.scrollHeight - element.scrollTop - element.clientHeight);
  await expect.poll(() => viewport.evaluate((element) => element.scrollHeight - element.clientHeight)).toBeGreaterThan(1200);
  // The reader goes up into the history, well away from the end.
  await log.hover(); await page.mouse.wheel(0, -2400);
  await expect.poll(distanceFromEnd).toBeGreaterThan(1200);
  // New rows arrive below them; the conversation is one that scrolls with its controls, so its whole height is far below.
  await emit(MAIN, { items: [agentMessage('late-row', 'A message that arrived while the reader was up in the history.', { turn: null, at: new Date().toISOString() })] });
  const jump = page.getByRole('button', { name: /new below|Jump to latest/ });
  await expect(jump).toBeVisible();
  await expect(jump).toBeInViewport({ ratio: 1 });
  // It stands inside what the reader sees of the scroll owner, above its foot.
  const [view, button] = await Promise.all([viewport.boundingBox(), jump.boundingBox()]);
  if (view === null || button === null) throw new Error('The thread screen and its jump are visible');
  expect(button.y).toBeGreaterThanOrEqual(view.y);
  expect(button.y + button.height).toBeLessThanOrEqual(view.y + view.height);
  expect(button.y + button.height).toBeGreaterThan(view.y + view.height - 120);
  await jump.click();
  await expect(log.getByText('A message that arrived while the reader was up in the history.')).toBeVisible();
  await expect.poll(distanceFromEnd).toBeLessThan(200);
  // At the end it has done its work and covers nothing.
  await expect(jump).toHaveCount(0);
});
