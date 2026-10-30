/** A thread at work: a turn streamed from frames (text in pieces, a command and its output, a change and its diff, an approval, the end with its usage), a message sent while the agent works, Stop, a connection lost and found with a gap, a thread that is blocked and why, and somebody who may only watch. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

import { ADA, GRACE } from '../doubles/thread-scenes.ts';
import {
  agentMessage,
  ALLOW_DENY,
  approvalItem,
  change,
  command,
  fileChange,
  harnessApproval,
  LOGIN_DIFF,
  threadInput,
  turn,
  userMessage,
} from '../doubles/thread-rows.ts';
import { bodySent, ACME, addApproval, closeThread, configureThread, delta, dropConnections, emit, PEOPLE, playScene, scenario, setPresence, signIn, threadHeld } from './support.ts';

const MAIN = ACME.mainThread;

test.beforeEach(async () => {
  await scenario();
  // The thread starts with nothing in it, and nothing running.
  await emit(MAIN, { thread: { state: 'idle' } });
});

const conversation = (page: Page) => page.getByRole('log', { name: 'Conversation' });
const composer = (page: Page) => page.getByRole('textbox', { name: 'Message to the agent' });
const header = (page: Page) => page.locator('[data-thread-header]');
const running = (page: Page) => header(page).getByText('Running', { exact: true });

async function open(page: Page, person: { login: string; password: string } = PEOPLE.ada, path = `/thread/${MAIN}`): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

const now = () => new Date().toISOString();

/**
 * Takes the conversation to its top, and waits until the page has seen it: the list holds on to the end while it measures its rows
 * (and says it is busy), and a click right behind a scroll is a click on a page that has not yet caught up with the scroll.
 */
async function scrollConversationToTop(page: Page): Promise<void> {
  const log = conversation(page);
  await expect(log).not.toHaveAttribute('aria-busy', 'true');
  await log.evaluate((element) => {
    element.scrollTop = 0;
    return new Promise<void>((resolve) => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          resolve();
        });
      });
    });
  });
}

/** The first message and the turn it started, as the system makes them. */
async function startTurn(prompt = 'Fix the login redirect'): Promise<void> {
  const began = now();
  await emit(MAIN, {
    items: [userMessage('u1', prompt, { author: ADA, turn: 't1', inputId: 'in-1', at: began }), agentMessage('a1', '', { turn: 't1', status: 'in_progress', at: began })],
    turns: [turn('t1', { ordinal: 1, state: 'running', inputId: 'in-1', startedAt: began, phase: { kind: 'working', since: began } })],
    inputs: [{ ...threadInput('in-1', prompt, { author: ADA, position: '1', state: 'delivered' }), turn_id: 't1', delivered_at: began }],
    thread: { state: 'running' },
  });
}

test('a turn is drawn as its frames arrive: text in pieces, a command and its output, a change and its diff, an approval that is decided, and the end with its usage', async ({ page }) => {
  await open(page);
  await expect(page.getByRole('heading', { name: 'Main thread' })).toBeVisible();
  await expect(page.getByText('Send the first message')).toBeVisible();
  await expect(page.getByText('Idle', { exact: true })).toBeVisible();

  await startTurn();
  await expect(conversation(page).getByText('Fix the login redirect')).toBeVisible();
  await expect(running(page)).toBeVisible();

  // The agent's words arrive in pieces; the text grows and nothing is repeated.
  await delta(MAIN, { item_id: 'a1', field: 'text', offset: 0, len: 12, data: 'I will look ' });
  await expect(conversation(page).getByText('I will look', { exact: false })).toBeVisible();
  await delta(MAIN, { item_id: 'a1', field: 'text', offset: 12, len: 22, data: 'at the login handler.' + ' ' });
  await expect(conversation(page).getByText('I will look at the login handler.', { exact: false })).toBeVisible();
  // A piece that overlaps what is there (a retransmission) changes nothing.
  await delta(MAIN, { item_id: 'a1', field: 'text', offset: 0, len: 12, data: 'I will look ' });
  await expect(conversation(page).getByText('I will look at the login handler.', { exact: false })).toHaveCount(1);

  // A command runs and its output grows under it.
  await emit(MAIN, { items: [command('c1', { command: 'npm test', exit: null, turn: 't1', status: 'in_progress', at: now() })] });
  await expect(conversation(page).getByText('npm test').first()).toBeVisible();
  await conversation(page).getByRole('button', { name: /npm test/, expanded: false }).click();
  await delta(MAIN, { item_id: 'c1', field: 'output', offset: 0, len: 21, data: 'running 19 tests...\n\n' });
  await expect(conversation(page).getByText('running 19 tests...')).toBeVisible();
  await emit(MAIN, { items: [command('c1', { command: 'npm test', output: 'running 19 tests...\n\n19 passed\n', exit: 0, turn: 't1', at: now() })] });
  await expect(conversation(page).getByText('exit 0')).toBeVisible();

  // A change with its difference.
  await emit(MAIN, { items: [fileChange('e1', { changes: [change('src/login.ts', LOGIN_DIFF)], turn: 't1', at: now() })] });
  await expect(conversation(page).getByRole('button', { name: /Edited src\/login\.ts/ })).toBeVisible();
  await expect(conversation(page).getByText('+1 −1').first()).toBeVisible();
  // A recent edit starts as one row too; its actual removed/added lines open on that row's click.
  const diff = conversation(page).getByRole('table', { name: 'Changes to src/login.ts' });
  await expect(diff).toHaveCount(0);
  await conversation(page).getByRole('button', { name: /Edited src\/login\.ts/, expanded: false }).click();
  await expect(diff).toContainText('return redirect(next);');
  await expect(diff).toContainText('return redirect(next ?? "/");');

  // The agent asks to run something; the request stands in the conversation until somebody answers.
  const approval = harnessApproval('ap-1', {
    threadId: MAIN,
    itemId: 'i-ap-1',
    title: 'Run npm install left-pad',
    requester: ADA,
    request: { request_kind: 'command', detail: { command: 'npm install left-pad', cwd: '/work/web', reason: 'The fix needs a padding helper.' }, options: ALLOW_DENY },
  });
  await addApproval(approval);
  await emit(MAIN, { items: [approvalItem('i-ap-1', 'ap-1', { turn: 't1', at: now() })], thread: { state: 'waiting_approval' } });
  await expect(page.getByText('Run npm install left-pad')).toBeVisible();
  await expect(header(page).getByText('Waiting for you', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Allow once · npm install left-pad', exact: true }).click();
  // The card closes to one line that says who answered and how, and the thread works again.
  await expect(conversation(page).getByText(/Allow once by Ada Lovelace/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Allow once · npm install left-pad', exact: true })).toHaveCount(0);
  await expect(running(page)).toBeVisible();

  // The turn ends: the answer, the footer with its usage, and a thread that is idle again.
  await emit(MAIN, {
    items: [agentMessage('a1', 'I will look at the login handler. The redirect now falls back to the home page.', { turn: 't1', at: now(), phase: 'final' })],
    turns: [turn('t1', { ordinal: 1, state: 'completed', inputId: 'in-1', startedAt: now(), completedAt: now(), tokens: 12_345 })],
    thread: { state: 'idle' },
  });
  await expect(conversation(page).getByText('The redirect now falls back to the home page.', { exact: false })).toBeVisible();
  await expect(conversation(page).getByText(/12\.3k tokens/)).toBeVisible();
  await expect(running(page)).toHaveCount(0);
  await expect(page.getByText('Idle', { exact: true })).toBeVisible();
});

test('a message sent while the agent works is queued, another is steered into the turn, and the queue shows each until it is delivered', async ({ page }) => {
  await open(page);
  await startTurn();
  await expect(running(page)).toBeVisible();

  // While the primary action is Stop, the split menu explicitly queues behind the turn.
  await composer(page).fill('Then check the logout page too');
  await page.getByRole('button', { name: 'More ways to send' }).click();
  await page.getByRole('menuitem', { name: /^Queue/ }).click();
  const tray = page.getByRole('region', { name: /^Queued/ });
  await expect(tray.getByText('Then check the logout page too')).toBeVisible();
  await expect(composer(page)).toHaveValue('');

  // The second goes into the turn that runs.
  await composer(page).fill('Use the existing helper');
  await page.getByRole('button', { name: 'More ways to send' }).click();
  await page.getByRole('menuitem', { name: /Steer now/ }).click();
  await expect(tray.getByText('Use the existing helper')).toBeVisible();
  const steering = tray.locator('[data-queued]').filter({ hasText: 'Use the existing helper' });
  await expect(steering.getByRole('button', { name: /^Edit queued message/ })).toBeEnabled();
  await expect(steering.getByRole('button', { name: /^Remove queued message/ })).toBeEnabled();
  await expect(steering.getByRole('combobox')).toBeEnabled();

  const held = await threadHeld(MAIN);
  const mine = held.inputs.filter((input) => input.state === 'pending');
  expect(mine.map((input) => [input.delivery, input.content[0]?.text])).toEqual([
    ['queue', 'Then check the logout page too'],
    ['steer', 'Use the existing helper'],
  ]);
  expect(mine.every((input) => typeof input.client_id === 'string' && input.client_id !== '')).toBe(true);

  // The system delivers the steered one into the turn: it leaves the queue and stands in the conversation.
  const steered = held.inputs.find((input) => input.delivery === 'steer');
  expect(steered).toBeDefined();
  // Handoff keeps the row and its familiar controls, without pretending that an issued
  // delivery can still be edited or recalled. Only the delivered result removes them.
  await emit(MAIN, { inputs: [{ ...threadInput(steered?.id ?? '', 'Use the existing helper', { author: ADA, position: '2', state: 'delivering', delivery: 'steer' }), turn_id: 't1' }] });
  await expect(steering.getByRole('button', { name: /^Edit queued message/ })).toBeDisabled();
  await expect(steering.getByRole('button', { name: /^Remove queued message/ })).toBeDisabled();
  await expect(steering.getByRole('combobox')).toBeDisabled();
  await expect(steering).toHaveAccessibleDescription('Delivery to the agent has started; this message can no longer be changed.');
  await emit(MAIN, {
    items: [userMessage('u2', 'Use the existing helper', { author: ADA, turn: 't1', inputId: steered?.id ?? '', delivery: 'steer', at: now() })],
    inputs: [{ ...threadInput(steered?.id ?? '', 'Use the existing helper', { author: ADA, position: '2', state: 'delivered', delivery: 'steer' }), turn_id: 't1', delivered_at: now() }],
  });
  await expect(conversation(page).getByText('Use the existing helper')).toBeVisible();
  await expect(conversation(page).getByText('steered the running turn')).toBeVisible();
  await expect(tray.getByText('Use the existing helper')).toHaveCount(0);
  await expect(tray.getByText('Then check the logout page too')).toBeVisible();

  // What waits can be taken back.
  await tray.getByRole('button', { name: 'Remove queued message 1', exact: true }).click();
  await expect(page.getByRole('region', { name: /^Queued/ })).toHaveCount(0);
  expect((await threadHeld(MAIN)).inputs.filter((input) => input.state === 'pending')).toEqual([]);
});

test('an agent that cannot be steered refuses it, and the menu offers no steering from then on', async ({ page }) => {
  await configureThread(MAIN, { cannot_steer: true });
  await open(page);
  await startTurn();
  await expect(running(page)).toBeVisible();

  await composer(page).fill('Use the existing helper');
  await page.getByRole('button', { name: 'More ways to send' }).click();
  await page.getByRole('menuitem', { name: /Steer now/ }).click();
  // Nothing was sent: the words are back in the box, and the refusal is read beside it.
  await expect(page.getByRole('alert')).toContainText('The agent does not support what this needs.');
  await expect(composer(page)).toHaveValue('Use the existing helper');
  // Only the message that began the turn is held: the refused one never reached the system.
  expect((await threadHeld(MAIN)).inputs.map((input) => input.id)).toEqual(['in-1']);

  await page.getByRole('button', { name: 'More ways to send' }).click();
  await expect(page.getByRole('menuitem', { name: /Steer now/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByText('This agent cannot be steered.')).toBeVisible();
});

test('Stop ends the turn that runs, and the thread can be spoken to again', async ({ page }) => {
  await open(page);
  await startTurn();
  await expect(running(page)).toBeVisible();

  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(conversation(page).getByText('Stopped by Ada Lovelace')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toHaveCount(0);
  await expect(running(page)).toHaveCount(0);
  const held = await threadHeld(MAIN);
  expect(held.turns.map((entry) => entry.state)).toEqual(['interrupted']);

  // Sending now starts the next turn, so the button says Send.
  await composer(page).fill('Try again');
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
});

test('a connection that is lost and found again leaves nothing twice and nothing missing', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page);
  await expect(conversation(page).getByText('Now do the same')).toHaveCount(0);
  await expect(conversation(page).getByText('The redirect now falls back to the home page when')).toBeVisible();

  // A frame that never arrives: the next one names the sequence it follows, and the page fetches what it lacks.
  await emit(MAIN, { items: [agentMessage('lost-1', 'This frame was lost on the way.', { turn: 't2', at: now(), phase: 'final' })] }, true);
  await emit(MAIN, { items: [agentMessage('after-1', 'This frame arrived after the lost one.', { turn: 't2', at: now(), phase: 'final' })] });
  await expect(conversation(page).getByText('This frame arrived after the lost one.')).toBeVisible();
  await expect(conversation(page).getByText('This frame was lost on the way.')).toHaveCount(1);
  await expect(conversation(page).getByText('This frame arrived after the lost one.')).toHaveCount(1);

  // The connection goes while rows are made; when it is back, they are there once each.
  await dropConnections();
  await emit(MAIN, { items: [agentMessage('while-1', 'Made while the connection was down.', { turn: 't2', at: now(), phase: 'final' })] });
  await emit(MAIN, { items: [agentMessage('while-2', 'Also made while it was down.', { turn: 't2', at: now(), phase: 'final' })] });
  await expect(conversation(page).getByText('Made while the connection was down.')).toBeVisible();
  await expect(conversation(page).getByText('Also made while it was down.')).toBeVisible();
  await expect(conversation(page).getByText('Made while the connection was down.')).toHaveCount(1);
  await expect(conversation(page).getByText('Also made while it was down.')).toHaveCount(1);
  await expect(conversation(page).getByText('This frame was lost on the way.')).toHaveCount(1);
  await expect(conversation(page).getByText('The redirect now falls back to the home page when')).toHaveCount(1);
});

test('a thread that is blocked has a compact state and says why above the box, and words written to it wait', async ({ page }) => {
  await open(page, PEOPLE.ada, `/thread/${ACME.idleThread}`);
  await expect(page.getByRole('heading', { name: 'Main thread' })).toBeVisible();
  await expect(header(page).getByText('Blocked', { exact: true })).toBeVisible();
  const reason = page.getByText('Claude subscription has reached its limit');
  await expect(reason).toHaveCount(1);
  await expect(reason).toBeVisible();
  // Words can still be written: they are delivered when the thread can work again.
  await composer(page).fill('Try again when the limit is over');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText('Try again when the limit is over')).toBeVisible();
  expect((await threadHeld(ACME.idleThread)).inputs.map((input) => [input.state, input.content[0]?.text])).toEqual([['pending', 'Try again when the limit is over']]);
});

test('somebody who may only watch sees the conversation, no composer, and what they would need to work with the thread', async ({ page }) => {
  await playScene(MAIN, 'waiting');
  await open(page, PEOPLE.vera);
  await expect(conversation(page).getByText('Please fix it, and run the tests.')).toBeVisible();
  await expect(composer(page)).toHaveCount(0);
  const watching = page.getByRole('region', { name: 'You can watch this thread' });
  await expect(watching).toBeVisible();
  await expect(watching).toContainText('to talk to its agent');
  await expect(watching.getByRole('button', { name: 'Try again', exact: true })).toHaveCount(0);
  await expect(watching.getByRole('button', { name: 'Request access', exact: true })).toBeVisible();
  const controls = page.getByRole('group', { name: 'Message controls' });
  await expect(controls).toHaveCount(0);
  await expect(header(page).getByText('Waiting for an answer', { exact: true })).toBeVisible();
  await expect(header(page).getByText('Waiting for you', { exact: true })).toHaveCount(0);
});

for (const viewer of [false, true]) {
  test(`a question renders from its delivered item without a second approval read (viewer: ${viewer})`, async ({ page }) => {
    const approval = harnessApproval('inline-question', { threadId: MAIN, itemId: 'inline-question-item', title: 'Choose scope', requester: ADA,
      request: { request_kind: 'user_input', options: [], detail: { questions: [{ id: 'scope', header: 'Scope', question: 'Which part should I change?', multiple: false, options: null, allow_other: true }] } },
    });
    await addApproval(approval);
    await emit(MAIN, { items: [approvalItem('inline-question-item', approval.id, { turn: 'question-turn' })],
      turns: [turn('question-turn', { ordinal: 1, state: 'running', inputId: 'question-input' })], thread: { state: 'waiting_approval' } });
    const reads: string[] = [];
    const decisions: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'GET' && path === '/api/v1/approvals/inline-question') reads.push(path);
      if (request.method() === 'POST' && path === '/api/v1/approvals/inline-question/decision') decisions.push(path);
    });
    // A legacy lookup is unavailable: the delivered snapshot must suffice on a cold page.
    await page.route('**/api/v1/approvals/inline-question', (route) => route.abort());
    await open(page, viewer ? PEOPLE.vera : PEOPLE.ada);
    const card = conversation(page).getByRole('region', { name: 'Choose scope', exact: true });
    const field = card.getByRole('textbox', { name: 'Which part should I change?', exact: true });
    const send = card.getByRole('button', { name: 'Send answers', exact: true });
    if (viewer) {
      await expect(field).toBeDisabled();
      await expect(send).toBeDisabled();
      await expect(card).toContainText('Waiting for an answer');
      await expect(header(page).getByText('Waiting for an answer', { exact: true })).toBeVisible();
      expect(decisions).toEqual([]);
    } else {
      await expect(field).toBeEnabled();
      await expect(header(page).getByText('Waiting for you', { exact: true })).toBeVisible();
      await field.fill('Login');
      await send.click();
      await expect(card).toHaveCount(0);
      await expect(conversation(page).getByRole('button', { name: /Choose scope.*Answered/ })).toBeVisible();
      expect(decisions).toHaveLength(1);
    }
    expect(reads).toEqual([]);
  });
}

test('who is looking at a thread is shown, and a thread taken away from its reader says so', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page);
  await expect(conversation(page)).toBeVisible();
  await setPresence(MAIN, [{ since: now(), user: GRACE }]);
  await header(page).getByRole('button', { name: /^Actions/ }).click();
  await page.getByRole('menuitem', { name: english.thread.header.view, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Looking now: Grace Hopper' }).click();
  const people = page.getByRole('dialog', { name: 'Looking at this thread' });
  await expect(people.getByLabel('Looking now: Grace Hopper')).toBeVisible();
  await page.keyboard.press('Escape');
  await setPresence(MAIN, []);
  await header(page).getByRole('button', { name: /^Actions/ }).click();
  await page.getByRole('menuitem', { name: english.thread.header.view, exact: true }).click();
  await expect(page.getByRole('menuitem', { name: /^Looking now/ })).toHaveCount(0);
  await page.keyboard.press('Escape');

  await closeThread(MAIN, 'revoked');
  await expect(page.getByText('You can no longer see this thread.')).toBeVisible();
});

test('the work of an older turn is folded into one line, which opens to its steps', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page);
  // The turn before the last one is one line; the last shows its steps.
  const fold = conversation(page).getByRole('button', { name: /Read 1 file, searched 1 time/ });
  await expect(fold).toBeVisible();
  await expect(conversation(page).getByText('Searched for “redirect” in src')).toHaveCount(0);
  await expect(conversation(page).getByText('Edited src/login.ts')).toBeVisible();
  // The line is above what is in view: it is brought to the top, and then clicked.
  await scrollConversationToTop(page);
  await fold.click();
  await expect(conversation(page).getByText('Searched for “redirect” in src')).toBeVisible();
  await expect(conversation(page).getByText('Read src/login.ts · lines 40–44')).toBeVisible();
  await fold.click();
  await expect(conversation(page).getByText('Searched for “redirect” in src')).toHaveCount(0);
});

test('the model and the effort are changed from the box, and its single control updates at once', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page);
  const change = page.getByRole('button', { name: /^Model and effort: GPT-5\.4 \(High/ });
  await expect(change).toHaveCount(1);
  await change.click();
  const card = page.getByRole('dialog', { name: 'Model and effort' });
  await expect(card.getByText('GPT-5.4 Chat')).toBeVisible();
  await expect(card.getByText('GPT-4.1')).toBeVisible();
  await expect(card.getByText('It is switched off')).toBeVisible();
  await card.getByRole('group', { name: 'Efforts of GPT-5.4', exact: true }).getByRole('menuitemradio', { name: 'Low', exact: true }).click();
  await expect(page.getByRole('button', { name: /^Model and effort: GPT-5\.4 \(Low/ })).toHaveCount(1);
  // A model that takes no effort leaves the thread with none.
  await card.getByRole('menuitemradio', { name: /^GPT-5\.4 Chat/ }).click();
  await expect(page.getByRole('button', { name: /^Model and effort: GPT-5\.4 Chat\. Change/ })).toHaveCount(1);
});

test('what was written and not sent is kept for the thread, and is in the box again when the page is opened anew', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page);
  await composer(page).fill('A half-written thought');
  await expect.poll(async () => (await threadHeld(MAIN)).drafts.ada?.content[0]?.text, { timeout: 30_000 }).toBe('A half-written thought');
  await page.reload();
  await expect(composer(page)).toHaveValue('A half-written thought');
  // Once it is sent it is not kept.
  await composer(page).press('Enter');
  await expect(conversation(page).getByText('A half-written thought')).toBeVisible();
  await expect.poll(async () => (await threadHeld(MAIN)).drafts.ada, { timeout: 30_000 }).toBeUndefined();
});

test('what waits in the queue is put in order, reworded by its author and taken out, one message at a time', async ({ page }) => {
  await playScene(MAIN, 'running');
  await open(page);
  await expect(running(page)).toBeVisible();
  for (const text of ['First queued', 'Second queued', 'Third queued']) {
    await composer(page).fill(text);
    await composer(page).press('Enter');
    await expect(page.getByRole('region', { name: /^Queued/ }).getByText(text)).toBeVisible();
  }
  const tray = page.getByRole('region', { name: /^Queued/ });
  const order = () => tray.locator('[data-queued-preview]').allTextContents();

  await tray.getByRole('button', { name: 'Move queued message 1 down', exact: true }).click();
  await expect.poll(order).toEqual(['Second queued', 'First queued', 'Third queued']);

  await tray.getByRole('button', { name: 'Edit queued message 3', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Words of the queued message' })).toBeFocused();
  await page.keyboard.press('ControlOrMeta+a');
  await page.keyboard.insertText('Third, reworded');
  await page.keyboard.press('Control+Enter');
  await expect.poll(order).toEqual(['Second queued', 'First queued', 'Third, reworded']);

  await tray.getByRole('button', { name: 'Remove queued message 2', exact: true }).click();
  await expect.poll(order).toEqual(['Second queued', 'Third, reworded']);
  expect((await threadHeld(MAIN)).inputs.filter((input) => input.state === 'pending').map((input) => input.content[0]?.text)).toEqual(['Second queued', 'Third, reworded']);
});

test('the first blocked and Stop-held input stays editable and removable in the queue, with no grey conversation copy', async ({ page }) => {
  const held = { ...threadInput('held-first', 'Held words', { author: ADA, position: '1', detail: 'cap_reached' }), queue_held: true };
  await emit(MAIN, { thread: { state: 'blocked', blocked: { reason: 'unbound', detail: null }, queue: { held: true, by: ADA, at: now(), held_inputs: 1, revision: 5 } }, inputs: [held] });
  await page.setViewportSize({ width: 400, height: 900 });
  await open(page);
  const tray = page.getByRole('region', { name: 'Queued (1)', exact: true });
  await expect(tray.getByText('Held words', { exact: true })).toBeVisible();
  await expect(conversation(page).getByText('Held words', { exact: true })).toHaveCount(0);
  await expect(page.getByText('Choose an agent and provider to start.', { exact: true })).toBeVisible();
  await tray.getByRole('button', { name: 'Edit queued message 1', exact: true }).click();
  await tray.getByRole('textbox', { name: 'Words of the queued message' }).fill('Held words, revised');
  const saved = page.waitForRequest((request) => request.method() === 'PATCH' && request.url().endsWith(`/threads/${MAIN}/inputs/held-first`));
  await tray.getByRole('button', { name: 'Save', exact: true }).click();
  expect((await bodySent(await saved))).toEqual({ content: [{ type: 'text', text: 'Held words, revised' }] });
  await expect(tray.getByText('Held words, revised', { exact: true })).toBeVisible();
  await expect(tray.getByText('Held by Stop', { exact: true })).toBeVisible();
  const removed = page.waitForRequest((request) => request.method() === 'DELETE' && request.url().endsWith(`/threads/${MAIN}/inputs/held-first`));
  await tray.getByRole('button', { name: 'Remove queued message 1', exact: true }).click();
  await removed;
  await expect(tray).toHaveCount(0);
  expect((await threadHeld(MAIN)).inputs).toHaveLength(1);
  expect((await threadHeld(MAIN)).inputs[0]?.state).toBe('cancelled');
});

test('Send now changes the existing input, refuses an unsupported steer, and interrupts only when asked again', async ({ page }) => {
  await configureThread(MAIN, { cannot_steer: true });
  await emit(MAIN, { thread: { state: 'running' }, turns: [turn('queue-running', { ordinal: 1, state: 'running', inputId: 'accepted' })], inputs: [threadInput('send-now', 'Use the safer helper', { author: ADA, position: '1' })] });
  await open(page);
  const tray = page.getByRole('region', { name: /^Queued/ });
  const requests: Promise<{ method: string; body: unknown }>[] = [];
  page.on('request', (request) => {
    if (!request.url().includes(`/threads/${MAIN}/inputs`)) return;
    requests.push(bodySent(request).then((body) => ({ method: request.method(), body })));
  });
  await tray.getByRole('button', { name: 'Send queued message 1 now', exact: true }).click();
  await expect(tray.getByRole('button', { name: 'Interrupt and send queued message 1', exact: true })).toBeVisible();
  expect(await Promise.all(requests)).toEqual([{ method: 'PATCH', body: { delivery: 'steer', after_input_id: null } }]);
  expect((await threadHeld(MAIN)).inputs.map((input) => [input.id, input.delivery])).toEqual([['send-now', 'queue']]);
  await tray.getByRole('button', { name: 'Interrupt and send queued message 1', exact: true }).click();
  await expect.poll(async () => (await threadHeld(MAIN)).inputs[0]?.delivery).toBe('interrupt');
  expect(await Promise.all(requests)).toEqual([{ method: 'PATCH', body: { delivery: 'steer', after_input_id: null } }, { method: 'PATCH', body: { delivery: 'interrupt', after_input_id: null } }]);
  expect((await threadHeld(MAIN)).inputs).toHaveLength(1);
});

test('queue order and delivery change through keyboard, drag and the same native PATCH', async ({ page }) => {
  await emit(MAIN, { inputs: [threadInput('one', 'First queued', { author: ADA, position: '1' }), threadInput('two', 'Second queued', { author: ADA, position: '2' }), threadInput('three', 'Third queued', { author: ADA, position: '3' })] });
  await open(page);
  const tray = page.getByRole('region', { name: 'Queued (3)', exact: true });
  const order = () => tray.locator('[data-queued-preview]').allTextContents();
  await expect.poll(order).toEqual(['First queued', 'Second queued', 'Third queued']);
  const writes: Promise<unknown>[] = [], reloads: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() === 'PATCH' && path.includes(`/threads/${MAIN}/inputs/`)) writes.push(bodySent(request));
    if (request.method() === 'GET' && path.endsWith(`/threads/${MAIN}/sync`)) reloads.push(path);
  });
  await tray.locator('[data-queued="two"]').focus();
  const originalRoute = page.url();
  await page.keyboard.press('Alt+ArrowUp');
  await expect.poll(order).toEqual(['Second queued', 'First queued', 'Third queued']);
  await expect(page).toHaveURL(originalRoute);
  await expect(tray.locator('[data-queued="two"]')).toBeFocused();
  await page.keyboard.press('Alt+ArrowUp');
  await expect(page).toHaveURL(originalRoute);
  await expect.poll(order).toEqual(['Second queued', 'First queued', 'Third queued']);
  await pointerQueueDrop(page, tray, 'three', 'two', 'before');
  await expect.poll(order).toEqual(['Third queued', 'Second queued', 'First queued']);
  expect(await Promise.all(writes)).toEqual([{ after_input_id: null }, { after_input_id: null }]);
  expect(reloads).toEqual([]);
  await expect(page).toHaveURL(originalRoute);
  await tray.getByRole('combobox', { name: 'Delivery of queued message 1' }).click();
  const delivered = page.waitForRequest((request) => request.method() === 'PATCH' && request.url().endsWith(`/threads/${MAIN}/inputs/three`));
  await page.getByRole('option', { name: 'Interrupt', exact: true }).click();
  expect((await bodySent(await delivered))).toEqual({ delivery: 'interrupt' });
  await expect.poll(async () => (await threadHeld(MAIN)).inputs.find((input) => input.id === 'three')?.delivery).toBe('interrupt');
  await tray.getByRole('button', { name: 'Send queued message 3 now', exact: true }).click();
  await expect.poll(order).toEqual(['First queued', 'Third queued', 'Second queued']);
});

/** Real pointer input, including the scroll needed to reach a clipped handle in the bounded queue. */
async function pickQueueRow(page: Page, tray: Locator, id: string): Promise<void> {
  const handle = tray.locator(`[data-queued="${id}"] [data-queue-handle]`);
  await handle.scrollIntoViewIfNeeded();
  const from = await handle.boundingBox();
  if (from === null) throw new Error('The picked message has a visible drag handle');
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  // Exceed the board/queue's six-pixel pickup threshold, then carry with actual pointer moves.
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2 - 8, { steps: 4 });
  await expect(tray).toHaveAttribute('data-queue-dragging', id);
}

async function aimQueueRow(page: Page, tray: Locator, targetId: string, edge: 'before' | 'after'): Promise<void> {
  const target = tray.locator(`[data-queued="${targetId}"]`);
  await expect.poll(async () => {
    const bounds = await target.boundingBox();
    const list = await tray.locator('[data-queue-list]').boundingBox();
    if (bounds === null || list === null) throw new Error('The target message and queue have bounds');
    const destination = bounds.y + bounds.height * (edge === 'before' ? 0.25 : 0.75);
    // Keep carrying at the list's edge until this target scrolls into view, then follow its half-row.
    // Re-read its position: a bounding box from before auto-scroll is no longer a pointer destination.
    const y = Math.max(list.y + 2, Math.min(list.y + list.height - 2, destination));
    await page.mouse.move(list.x + list.width / 2, y, { steps: 20 });
    return y === destination && await target.locator(`[data-queue-drop-line="${edge}"]`).isVisible();
  }).toBe(true);
}

async function pointerQueueDrop(page: Page, tray: Locator, sourceId: string, targetId: string, edge: 'before' | 'after'): Promise<void> {
  await pickQueueRow(page, tray, sourceId);
  await aimQueueRow(page, tray, targetId, edge);
  await page.mouse.up();
  await expect(tray.locator('[data-queue-drop-line]')).toHaveCount(0);
}

async function queueLanguage(page: Page, locale: 'en' | 'zh-CN'): Promise<void> {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: 'Language', exact: true }).click();
  const saved = page.waitForResponse(async (response) => {
    const request = response.request();
    if (request.method() !== 'PATCH' || !new URL(request.url()).pathname.endsWith('/me')) return false;
    const body = await bodySent(request);
    return typeof body === 'object' && body !== null && 'locale' in body && body.locale === locale;
  });
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  expect((await saved).ok()).toBe(true);
  await page.goto(`/thread/${MAIN}`);
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`pointer queue drag moves gamma at its visible line after arrow/Alt reorders (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    const inputs = ([['alpha', 'alpha, edited'], ['beta', 'beta'], ['gamma', 'gamma'], ['delta', 'delta']] as const)
      .map(([id, text], index) => threadInput(id, text, { author: ADA, position: String(index + 1) }));
    await emit(MAIN, { thread: { state: 'running' }, turns: [turn('drag-running', { ordinal: 1, state: 'running', inputId: 'accepted' })], inputs });
    await open(page); await queueLanguage(page, locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    const words = locale === 'en' ? english : chinese;
    const tray = page.locator('[data-queue]');
    const order = () => tray.locator('[data-queued-preview]').allTextContents();
    await expect.poll(order).toEqual(['alpha, edited', 'beta', 'gamma', 'delta']);
    await tray.getByRole('button', { name: words.composer.queue.moveRowUp.replace('{{number}}', '4'), exact: true }).click();
    await expect.poll(order).toEqual(['alpha, edited', 'beta', 'delta', 'gamma']);
    await tray.locator('[data-queued="alpha"]').focus(); await page.keyboard.press('Alt+ArrowDown');
    await expect.poll(order).toEqual(['beta', 'alpha, edited', 'delta', 'gamma']);
    const moves: Promise<{ id: string; body: unknown }>[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'PATCH' && path.includes(`/threads/${MAIN}/inputs/`)) moves.push(bodySent(request).then((body) => ({ id: path.split('/').at(-1) ?? '', body })));
    });
    await pickQueueRow(page, tray, 'gamma'); await aimQueueRow(page, tray, 'beta', 'before');
    await expect(page.locator('[data-queue-drag-preview]')).toHaveText('gamma');
    expect(moves).toEqual([]);
    await info.attach(`queue-drop-line-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    await page.mouse.up();
    await expect.poll(order).toEqual(['gamma', 'beta', 'alpha, edited', 'delta']);
    expect(await Promise.all(moves)).toEqual([{ id: 'gamma', body: { after_input_id: null } }]);
    await expect(tray.locator('[data-queued="gamma"]')).toBeFocused();
    await pointerQueueDrop(page, tray, 'gamma', 'delta', 'after');
    await expect.poll(order).toEqual(['beta', 'alpha, edited', 'delta', 'gamma']);
    expect(await Promise.all(moves)).toEqual([{ id: 'gamma', body: { after_input_id: null } }, { id: 'gamma', body: { after_input_id: 'delta' } }]);
    const actual = (await threadHeld(MAIN)).inputs.find((input) => input.id === 'gamma');
    expect(actual?.content).toEqual([{ type: 'text', text: 'gamma' }]);
    expect(actual?.delivery).toBe('queue'); expect(actual?.queue_held).toBe(false);
  });
}

test('a pointer drop keeps the picked id while live rows change its number, and cancellation or delivery writes nothing', async ({ page }) => {
  const inputs = ['alpha', 'beta', 'delta', 'gamma'].map((id, index) => threadInput(id, id, { author: ADA, position: String(index + 1) }));
  await emit(MAIN, { inputs }); await open(page);
  const tray = page.locator('[data-queue]');
  const order = () => tray.locator('[data-queued-preview]').allTextContents();
  const moves: Promise<{ id: string; body: unknown }>[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (request.method() === 'PATCH' && path.includes(`/threads/${MAIN}/inputs/`)) moves.push(bodySent(request).then((body) => ({ id: path.split('/').at(-1) ?? '', body })));
  });
  await pickQueueRow(page, tray, 'gamma');
  await emit(MAIN, { inputs: [threadInput('new', 'New message', { author: ADA, position: '0' })] });
  await expect(tray.locator('[data-queued="gamma"]')).toHaveAttribute('aria-label', 'Queued message 5');
  await aimQueueRow(page, tray, 'alpha', 'before'); await page.mouse.up();
  await expect.poll(order).toEqual(['New message', 'gamma', 'alpha', 'beta', 'delta']);
  expect(await Promise.all(moves)).toEqual([{ id: 'gamma', body: { after_input_id: 'new' } }]);

  await pickQueueRow(page, tray, 'gamma'); await aimQueueRow(page, tray, 'delta', 'after');
  await page.keyboard.press('Escape'); await page.mouse.up();
  await expect(tray.locator('[data-queue-drop-line]')).toHaveCount(0);
  await expect.poll(order).toEqual(['New message', 'gamma', 'alpha', 'beta', 'delta']);
  expect(moves).toHaveLength(1);
  await pickQueueRow(page, tray, 'gamma'); await page.mouse.move(2, 2, { steps: 10 }); await page.mouse.up();
  expect(moves).toHaveLength(1);

  await pickQueueRow(page, tray, 'gamma');
  await emit(MAIN, { inputs: [{ ...threadInput('gamma', 'gamma', { author: ADA, position: '0.5' }), state: 'delivered' }] });
  await expect(tray.locator('[data-queued="gamma"]')).toHaveCount(0);
  await page.mouse.up(); expect(moves).toHaveLength(1);
  await expect(tray.locator('[data-queue-drop-line]')).toHaveCount(0);
});

test('a pointer reorder preserves a Stop-held message and sends only its new predecessor', async ({ page }) => {
  await emit(MAIN, { thread: { state: 'idle', queue: { held: true, by: ADA, at: now(), held_inputs: 3, revision: 5 } },
    inputs: ['first', 'middle', 'last'].map((id, index) => ({ ...threadInput(id, id, { author: ADA, position: String(index + 1) }), queue_held: true })) });
  await open(page); const tray = page.locator('[data-queue]');
  const request = page.waitForRequest((request) => request.method() === 'PATCH' && request.url().endsWith(`/threads/${MAIN}/inputs/last`));
  await pointerQueueDrop(page, tray, 'last', 'first', 'before');
  expect((await bodySent(await request))).toEqual({ after_input_id: null });
  await expect.poll(() => tray.locator('[data-queued-preview]').allTextContents()).toEqual(['last', 'first', 'middle']);
  const actual = (await threadHeld(MAIN)).inputs.find((input) => input.id === 'last');
  expect(actual?.queue_held).toBe(true); expect(actual?.delivery).toBe('queue');
  await expect(tray.locator('[data-queued="last"]')).toContainText('Held by Stop');
});

test('queue previews skip a script delimiter, while Alt arrows outside its rows still walk the real tree', async ({ page }) => {
  await emit(MAIN, { inputs: [threadInput('scripted-preview', '#!fake\n\nsay Review the redirect', { author: ADA, position: '1' })] });
  await signIn(page);
  await page.goto('/org/acme');
  // The ordinary navigation binding still finds a thread before a queue has focus.
  await page.locator('body').press('Alt+ArrowDown');
  await expect(page).toHaveURL(/\/thread\//);
  const firstThread = page.url();
  await page.locator('body').press('Alt+ArrowDown');
  await expect(page).not.toHaveURL(firstThread);
  await expect(page).toHaveURL(/\/thread\//);
  await page.goto(`/thread/${MAIN}`);
  const tray = page.getByRole('region', { name: 'Queued (1)', exact: true });
  await expect(tray.locator('[data-queued-preview]')).toHaveText('say Review the redirect');
  await expect(tray.getByText('#!fake', { exact: true })).toHaveCount(0);
  const up = tray.getByRole('button', { name: 'Move queued message 1 up', exact: true });
  const down = tray.getByRole('button', { name: 'Move queued message 1 down', exact: true });
  await expect(up.locator('svg.lucide-arrow-up')).toBeVisible();
  await expect(down.locator('svg.lucide-arrow-down')).toBeVisible();
  await tray.getByRole('button', { name: 'Edit queued message 1', exact: true }).click();
  await expect(tray.getByRole('textbox')).toBeFocused();
  await page.keyboard.insertText(' carefully');
  const patched = page.waitForRequest((request) => request.method() === 'PATCH' && request.url().endsWith(`/threads/${MAIN}/inputs/scripted-preview`));
  await page.keyboard.press('Control+Enter');
  expect((await bodySent(await patched))).toEqual({ content: [{ type: 'text', text: '#!fake\n\nsay Review the redirect carefully' }] });
  await expect(tray.locator('[data-queued-preview]')).toHaveText('say Review the redirect carefully');
  await expect(tray.locator('[data-queued="scripted-preview"]')).toBeFocused();
  await page.keyboard.press('Alt+ArrowUp');
  await expect(page).toHaveURL(new RegExp(`/thread/${MAIN}$`));
});

test('items are picked across turns from their menus and by their boxes, and Quote puts their words into the box that is written to the agent', async ({ page }) => {
  await playScene(MAIN, 'history');
  await open(page);
  const question = 'Why does the login page send people to the wrong place after they sign in?';
  await expect(conversation(page).getByText(question)).toBeVisible();

  // The menu of an item begins the picking with that item; every item then has a box, and the bar stands in place of the one the message is written in.
  const first = conversation(page).locator('[data-item="u1"]');
  await first.hover();
  await first.getByRole('button', { name: 'More acts on this item' }).click();
  await page.getByRole('menuitem', { name: 'Select', exact: true }).click();
  const bar = page.getByRole('toolbar', { name: 'Selected items' });
  await expect(bar.getByText('1 selected')).toBeVisible();
  await expect(composer(page)).toHaveCount(0);
  await expect(first.getByRole('checkbox', { name: 'Select this item' })).toBeChecked();

  // Another item in another turn is picked by its box, and let go of by pressing it again.
  const answer = conversation(page).locator('[data-item="a1"]');
  await answer.getByRole('checkbox', { name: 'Select this item' }).check();
  await expect(bar.getByText('2 selected')).toBeVisible();
  await answer.getByRole('checkbox', { name: 'Select this item' }).uncheck();
  await expect(bar.getByText('1 selected')).toBeVisible();
  await answer.getByRole('checkbox', { name: 'Select this item' }).check();

  // Quote: their words stand in the box as a quotation, and the bar gives way to it.
  await bar.getByRole('button', { name: 'Quote' }).click();
  await expect(bar).toHaveCount(0);
  await expect(composer(page)).toHaveValue(/^> Why does the login page send people to the wrong place after they sign in\?\n>\n> The handler reads/);

  // Cancel leaves everything as it was.
  await first.hover();
  await first.getByRole('button', { name: 'More acts on this item' }).click();
  await page.getByRole('menuitem', { name: 'Select', exact: true }).click();
  await page.getByRole('toolbar', { name: 'Selected items' }).getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByRole('toolbar', { name: 'Selected items' })).toHaveCount(0);
  await expect(composer(page)).toBeVisible();
});

test('an item that messages of the thread’s chat quote carries their number, which brings the chat to the front', async ({ page }) => {
  await emit(MAIN, { items: [{ ...agentMessage('a1', 'The redirect loses the next parameter.', { turn: null, at: now() }), chat_refs: 2 }] });
  await open(page);
  const count = conversation(page).getByRole('button', { name: '2 chat messages quote this item' });
  await expect(count).toBeVisible();
  await expect(conversation(page).getByRole('button', { name: /chat message quotes this item/ })).toHaveCount(0);
  await count.click();
  await expect(page.getByRole('region', { name: 'Right bar' }).getByRole('textbox', { name: /^Message / })).toBeVisible();
});
