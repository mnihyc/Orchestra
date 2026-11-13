/** After a reconnect the views of a person's reach are read again behind what they show: no skeleton, no remount, no lost place,
 * text, choice or focus; and what the re-read refuses goes at once. The connection is dropped at a chosen moment, not found by chance. */
import { expect, test, type Page, type Request } from '@playwright/test';

import { ACME, deny, dropConnections, emit, playScene, scenario, signIn } from './support.ts';
import { openHistory } from './change-support.ts';

const THREAD = `/thread/${ACME.mainThread}`;
const main = (page: Page) => page.locator('[data-screen="thread"]:visible');

/** Counts, frame by frame, the skeletons the page draws from now on (inside `within`), and keeps the most there were at once. */
async function watchSkeletons(page: Page, within = 'body'): Promise<() => Promise<number>> {
  await page.evaluate((root) => {
    const held = window as unknown as { __skeletons: number };
    held.__skeletons = 0;
    const look = () => {
      const shown = Array.from(document.querySelectorAll(`${root} .animate-skeleton`)).filter((el) => (el as HTMLElement).getClientRects().length > 0).length;
      held.__skeletons = Math.max(held.__skeletons, shown);
      requestAnimationFrame(look);
    };
    requestAnimationFrame(look);
  }, within);
  return () => page.evaluate(() => (window as unknown as { __skeletons: number }).__skeletons);
}

/** Marks what is on the screen now, to know it again afterwards: the same element, not one drawn again like it. */
async function mark(page: Page, selector: string): Promise<void> {
  await page.evaluate((s) => document.querySelector(s)?.setAttribute('data-kept', 'yes'), selector);
}
const kept = (page: Page, selector: string) => page.locator(`${selector}[data-kept="yes"]`);

/** The connection ends, as it does when the server restarts or the network is lost, and comes back; the page has read its views again when its requests are done. */
async function reconnect(page: Page): Promise<void> {
  const baseline = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/me/bootstrap');
  await dropConnections();
  await baseline;
  // The socket's own request never ends while the page is up; every other request of the API is a read the page is making.
  const reading = new Set<Request>();
  let quietSince = Date.now();
  const reads = (request: Request) => { const { pathname } = new URL(request.url()); return pathname.startsWith('/api/v1/') && pathname !== '/api/v1/ws'; };
  page.on('request', (request) => { if (reads(request)) reading.add(request); });
  const done = (request: Request) => { if (reading.delete(request)) quietSince = Date.now(); };
  page.on('requestfinished', done);
  page.on('requestfailed', done);
  await expect.poll(() => reading.size === 0 && Date.now() - quietSince > 900, { timeout: 20_000, intervals: [100] }).toBe(true);
}

for (const width of [1440, 400]) {
  test.describe(`${width}`, () => {
    test.beforeEach(async ({ page }) => { await scenario(); await page.setViewportSize({ width, height: 900 }); });

    test(`a long thread read from its middle keeps its place and draws no skeleton when the connection is dropped and restored (${width})`, async ({ page }) => {
      await playScene(ACME.mainThread, 'long'); await signIn(page); await page.goto(THREAD);
      await expect(main(page).getByRole('log', { name: 'Conversation', exact: true })).toBeVisible();
      await main(page).evaluate((el) => { el.scrollTop = (el.scrollHeight - el.clientHeight) / 2; });
      const place = () => main(page).evaluate((el) => {
        const frame = el.getBoundingClientRect();
        const row = Array.from(el.querySelectorAll('[data-item]')).find((item) => item.getBoundingClientRect().bottom > frame.top + 40);
        return { top: Math.round(el.scrollTop), row: (row?.textContent ?? '').trim().slice(0, 40) };
      });
      await expect.poll(async () => (await place()).row).not.toBe('');
      const before = await place();
      await mark(page, '[data-screen="thread"]');
      const skeletons = await watchSkeletons(page);
      await reconnect(page);
      expect(await place()).toEqual(before);
      await expect(kept(page, '[data-screen="thread"]')).toHaveCount(1);
      expect(await skeletons(), 'nothing of the page is drawn as a skeleton').toBe(0);
    });

    test(`a composer keeps its text, its attachment, its open picker and the keyboard when the connection is dropped and restored (${width})`, async ({ page }) => {
      await emit(ACME.mainThread, { thread: { state: 'idle', current_turn_id: null } });
      await signIn(page); await page.goto(THREAD);
      const box = main(page).getByRole('textbox', { name: 'Message to the agent' });
      await box.fill('A half written message to the agent');
      await page.locator('[data-screen="thread"]:visible input[type="file"]').first().setInputFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.from('some notes') });
      await expect(main(page).getByText('notes.txt').first()).toBeVisible();
      // The picker for a file or a document, opened from the composer's own menu.
      await main(page).getByRole('button', { name: 'Add to message' }).click();
      await page.getByRole('menuitem', { name: 'Mention a workspace file…' }).click();
      const picker = page.getByRole('dialog', { name: 'Find a file or document' });
      await expect(picker).toBeVisible();
      await mark(page, '[data-screen="thread"] textarea');
      await mark(page, '[role="dialog"]');
      const skeletons = await watchSkeletons(page);
      await reconnect(page);
      await expect(picker).toBeVisible();
      await expect(kept(page, '[role="dialog"]')).toHaveCount(1);
      await expect(main(page).getByText('notes.txt').first()).toBeVisible();
      // The picker is a dialog over the page: the composer behind it is not in the accessibility tree, and is asked for as the element it is.
      await expect(page.locator('[data-screen="thread"] textarea').first()).toHaveValue('A half written message to the agent');
      await expect(kept(page, '[data-screen="thread"] textarea')).toHaveCount(1);
      expect(await skeletons()).toBe(0);
    });

    test(`dialogs half filled stay what they are when the connection is dropped and restored (${width})`, async ({ page }) => {
      await signIn(page); await page.goto(THREAD);
      await page.locator('[data-thread-header]').getByRole('button', { name: 'Share thread', exact: true }).click();
      const share = page.getByRole('dialog').first();
      await expect(share).toBeVisible();
      const person = share.getByRole('combobox', { name: 'Add people, groups or roles', exact: true });
      await person.fill('Dor');
      await mark(page, '[role="dialog"]');
      const skeletons = await watchSkeletons(page, '[role="dialog"]');
      await reconnect(page);
      await expect(kept(page, '[role="dialog"]')).toHaveCount(1);
      await expect(person).toHaveValue('Dor');
      await expect(person).toBeFocused();
      expect(await skeletons(), 'the dialog draws no skeleton').toBe(0);
    });

    test(`the finder keeps its query and its results when the connection is dropped and restored (${width})`, async ({ page }) => {
      await signIn(page); await page.keyboard.press('Control+k');
      const finder = page.getByRole('dialog', { name: 'Find', exact: true });
      await finder.getByRole('combobox').fill('Infra');
      // The results are all in (the group comes after the project): the row that is marked is not one that a later answer puts another before.
      await expect(page.getByRole('option', { name: /Infra team group/ })).toBeVisible();
      await mark(page, '[role="dialog"]');
      await page.getByRole('option', { name: /Infra project/ }).evaluate((row) => row.setAttribute('data-kept', 'yes'));
      const skeletons = await watchSkeletons(page);
      await reconnect(page);
      await expect(kept(page, '[role="dialog"]')).toHaveCount(1);
      await expect(finder.getByRole('combobox')).toHaveValue('Infra');
      // The row that was there is the very row that is there: the results were not drawn again.
      await expect(page.getByRole('option', { name: /Infra project/ }).and(kept(page, '[role="option"]'))).toHaveCount(1);
      await expect(page.getByRole('option', { name: /Infra/ }).first()).toBeVisible();
      expect(await skeletons(), 'neither the results nor the page behind them are drawn as skeletons').toBe(0);
    });

    test(`a chat keeps the reply being typed, its keyboard and its place when the connection is dropped and restored (${width})`, async ({ page }) => {
      await signIn(page); await page.goto('/');
      if (width < 600) await page.getByRole('button', { name: 'Team', exact: true }).click();
      const reply = page.getByPlaceholder('Message…').first();
      await reply.fill('half a reply to the team');
      await mark(page, 'textarea[placeholder="Message…"]');
      const skeletons = await watchSkeletons(page);
      await reconnect(page);
      await expect(reply).toHaveValue('half a reply to the team');
      await expect(reply).toBeFocused();
      await expect(kept(page, 'textarea[placeholder="Message…"]')).toHaveCount(1);
      expect(await skeletons()).toBe(0);
    });

    test(`what the re-read refuses goes at once: a thread somebody lost sight of while the connection was down (${width})`, async ({ page }) => {
      await openHistory(page);
      await expect(main(page).getByText('The handler reads').first()).toBeVisible();
      await deny('ada', ['thread.view']);
      await reconnect(page);
      await expect(main(page).getByText('The handler reads')).toHaveCount(0);
    });

    test(`a tick of the heartbeat that comes a minute late, after a ping that was answered, leaves a live socket alone (${width})`, async ({ page }) => {
      // The page's timers are held back, as they are in a tab that has been hidden for minutes: the line is alive and answers every ping.
      await page.clock.install({ time: new Date() });
      await page.clock.resume();
      const sockets: { sent: string[] }[] = [];
      page.on('websocket', (socket) => {
        const held = { sent: [] as string[] };
        sockets.push(held);
        socket.on('framesent', (frame) => { if (typeof frame.payload === 'string') held.sent.push(frame.payload); });
      });
      let baselines = 0;
      page.on('request', (request) => { if (new URL(request.url()).pathname === '/api/v1/me/bootstrap') baselines += 1; });
      await signIn(page);
      await page.goto(THREAD);
      await expect(page.locator('[data-thread-header]')).toBeVisible();
      await page.waitForTimeout(1_500);
      const pings = () => sockets.flatMap((socket) => socket.sent).filter((text) => text.includes('"op":"ping"')).length;
      // One heartbeat, which the server answers.
      const before = pings();
      await page.clock.fastForward('00:26');
      await expect.poll(pings).toBeGreaterThan(before);
      await page.waitForTimeout(1_000);
      const opened = sockets.length;
      const read = baselines;
      // The next tick comes seventy seconds on: the age of the last answer is over a minute, and the line has been answering all the time.
      const asked = pings();
      await page.clock.fastForward('01:10');
      await expect.poll(pings).toBeGreaterThan(asked);
      await page.waitForTimeout(2_000);
      expect(sockets, 'no new socket was opened').toHaveLength(opened);
      expect(baselines, 'nothing was read again').toBe(read);
    });
  });
}
