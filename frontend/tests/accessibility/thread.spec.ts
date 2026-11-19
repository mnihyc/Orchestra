/** Every state of the thread screen, asked in both themes whether anything on it is a serious barrier: a conversation that is over, one that runs, one that waits for an answer, a turn that failed and one that was stopped, a thread that is blocked, a reader who may only watch, a queue and its menu, and the dialog that makes a thread. */
import { expect, type Page, test } from '@playwright/test';

import { agentMessage, threadInput, userMessage } from '../doubles/thread-rows.ts';
import { ADA } from '../doubles/thread-scenes.ts';
import { ACME, emit, expectAccessibleInBothThemes, PEOPLE, playScene, scenario, signIn } from '../e2e/support.ts';

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
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect(page.getByText(/^Running for /)).toBeVisible();
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
  await expect(conversation(page).getByText('The model refused the request.').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('what waits in the queue, and the ways of sending that the button opens', async ({ page }) => {
  await playScene(MAIN, 'running');
  await open(page);
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
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
  await expect(page.getByRole('region', { name: 'You can watch this thread' })).toBeVisible();
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
  await page.getByTestId('main').getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  const form = page.getByRole('dialog', { name: 'New thread in Fix login' });
  await expect(form.getByRole('radio', { name: /OpenAI key/ })).toBeChecked();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('items being picked, with the bar that stands in place of the box, and the number on an item that the chat quotes', async ({ page }) => {
  await playScene(MAIN, 'history');
  await emit(MAIN, { items: [{ ...agentMessage('a-quoted', 'This one is quoted in the chat.', { turn: null, at: new Date().toISOString() }), chat_refs: 1 }] });
  await open(page);
  await expect(conversation(page).getByRole('button', { name: '1 chat message quotes this item' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  const first = conversation(page).locator('[data-item="u1"]');
  await first.hover();
  await first.getByRole('button', { name: 'More acts on this item' }).click();
  await page.getByRole('menuitem', { name: 'Select', exact: true }).click();
  await expect(page.getByRole('toolbar', { name: 'Selected items' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
