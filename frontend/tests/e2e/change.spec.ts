/** Changing what a thread runs on and where: a switch planned, blocked, carried out, failed and made again; a fork of a message in the middle; a side thread; an environment updated and moved; several threads at once. */
import { expect, type Page, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { BUILD_HOST, change, conversation, HOST_ONE, MAIN, openBlocked, openHistory, shot, tabTo, wizard } from './change-support.ts';
import { ACME, bodySent, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
  await change.mode('manual');
});

test('a switch of a blocked thread: planned, a blocker resolved in the dialog, carried out with the thread locked, and the thread on its new side', async ({ page }) => {
  await openBlocked(page);
  await shot(page, 'switch-1-blocked');
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog).toBeVisible();

  // The plan is made for the provider that can take it: what changes, what stays, what ends.
  await expect(dialog.getByRole('combobox', { name: 'Agent and provider', exact: true })).toContainText('OpenAI key');
  await expect(dialog.getByRole('combobox', { name: 'Agent and provider', exact: true })).toContainText('Codex');
  await expect(dialog.getByRole('list', { name: 'Steps' })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await shot(page, 'switch-2-plan');

  // The list of models was read before an administrator switched one off: the plan says what stands in the way, and what lifts it.
  await change.model(RUNS_ON.openAiKey, 'gpt-5.4', false);
  await dialog.getByRole('combobox', { name: 'Effort' }).click();
  await page.getByRole('option', { name: 'High' }).click();
  await expect(dialog.getByText('gpt-5.4 is not a model OpenAI key offers')).toBeVisible();
  await expect(dialog.getByText('Choose a model it offers.')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeDisabled();
  await shot(page, 'switch-3-blocked');

  await dialog.getByRole('combobox', { name: 'Model' }).click();
  const modelPlan = page.waitForRequest('**/api/v1/threads/*/forks/preflight');
  await page.getByRole('option', { name: 'Model name unavailable', exact: true }).click();
  const asked: unknown = await bodySent(await modelPlan);
  expect(asked).toMatchObject({ to: { model: 'gpt-5.4-mini' } });
  await expect(dialog.getByText('gpt-5.4 is not a model OpenAI key offers')).toHaveCount(0);
  await expect(dialog.getByRole('combobox', { name: 'Model', exact: true })).toContainText('Model name unavailable');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await shot(page, 'switch-4-resolved');

  // The people who talk to the thread may not use the new provider: the plan says so, and the one who may give them the use does, before the switch.
  await expect(dialog.getByText('Some of the people who can talk to it may not use OpenAI key.')).toBeVisible();
  await dialog.getByRole('checkbox', { name: 'Open OpenAI key to the work in Infra' }).click();
  const before = await change.uses(RUNS_ON.openAiKey);

  // Carried out: the thread is held, and says how far it has come.
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  expect((await change.uses(RUNS_ON.openAiKey)).length).toBeGreaterThan(before.length);
  await shot(page, 'switch-5-progress');
  await dialog.getByRole('button', { name: 'Hide' }).click();
  await expect(dialog).toHaveCount(0);
  const lock = page.locator('[data-lock]');
  await expect(lock).toBeVisible();
  await shot(page, 'switch-6-locked');

  // It goes on without the window, and the thread runs on the new side.
  await change.finish();
  await expect(lock).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Switch provider…' })).toHaveCount(0);
  await expect(conversation(page).getByText(/^Ada Lovelace switched to Codex with OpenAI key(,|$)/)).toBeVisible();
  await shot(page, 'switch-7-done');
});

test('a switch that fails in its checks says where, why, and what was undone, and is tried again', async ({ page }) => {
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();

  // The new side does not pass its checks: what it had made is taken away.
  await change.fail('verify', 'verify_failed', 'the tools could not reach the new side', true);
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  await change.finish();
  await expect(dialog.getByRole('alert')).toContainText('The new side did not pass:');
  await expect(dialog.getByRole('alert')).toContainText('The operation stopped before it finished. Retry it from its controls.');
  await expect(dialog.getByText('It did not work, in the checks')).toHaveCount(0);
  await expect(dialog.getByText('Everything it had made was removed. Main thread is as it was before.')).toBeVisible();
  await shot(page, 'failed-1-result');

  // The thread is as it was: held back by the same provider, not locked.
  await dialog.getByRole('button', { name: 'Close' }).last().click();
  await expect(dialog).toHaveCount(0);
  await expect(page.locator('[data-lock]')).toHaveCount(0);
  await expect(page.getByText('Claude subscription has reached its limit')).toBeVisible();

  // Tried again, it works: the same choices, a new plan.
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  await change.finish();
  await expect(dialog.getByText('This thread now runs on OpenAI key')).toBeVisible();
  await shot(page, 'failed-2-again-done');
  await dialog.getByRole('button', { name: 'Done' }).click();
  await expect(conversation(page).getByText(/^Ada Lovelace switched to Codex with OpenAI key(,|$)/)).toBeVisible();
});

test('a plan that is more than fifteen minutes old is not carried out: it is made again, by the page when it knows and by the server when it says so', async ({ page }) => {
  await page.clock.install({ time: new Date() });
  await page.clock.resume();
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();

  // The page knows the plan is old: it says so, and will not carry it out until it is made again.
  await page.clock.fastForward('16:00');
  await change.clock(16);
  await expect(dialog.getByText('This plan is out of date')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeDisabled();
  await shot(page, 'expired-1-page');
  await dialog.getByRole('button', { name: 'Make the plan again' }).click();
  await expect(dialog.getByText('This plan is out of date')).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();

  // The server says it when the page did not know: the plan is made again and looked over before it is confirmed.
  await change.expire();
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('The plan was older than 15 minutes and has been made again. Look it over, then confirm.')).toBeVisible();
  await shot(page, 'expired-2-server');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
});

test('a fork from a message in the middle goes to a new environment with some of the files, and the new thread opens', async ({ page }) => {
  await openHistory(page);
  await conversation(page).getByText('The handler reads').first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Fork from here…' }).click();
  const dialog = wizard(page, 'Fork “Main thread”');
  await expect(dialog.getByRole('combobox', { name: 'History', exact: true })).toContainText('Carry through a turn');
  await expect(dialog.getByText('The new thread will be yours')).toBeVisible();
  await shot(page, 'fork-1-plan');

  // The history says how far it goes and how it travels.
  await expect(dialog.getByRole('combobox', { name: 'History', exact: true })).toContainText('Carry through a turn');
  await expect(dialog.getByText('Last turn to take')).toBeVisible();
  await shot(page, 'fork-2-history');

  // A new environment, with only some of the files.
  await dialog.getByRole('combobox', { name: 'Environment', exact: true }).click();
  await page.getByRole('option', { name: 'Copy to new', exact: true }).click();
  await expect(dialog.getByRole('combobox', { name: 'Place', exact: true })).toContainText('Automatic');
  await dialog.getByRole('radio', { name: 'By pattern', exact: true }).click();
  await dialog.getByRole('textbox', { name: 'Take these' }).fill('src/**');
  await expect(dialog.getByText('A new environment, Main thread environment, on host-1: yours')).toBeVisible();
  await expect(dialog.getByText(/^Files: \d+ files?, /)).toBeVisible();
  await shot(page, 'fork-3-environment');

  // What the new thread is called, and who can do what with it.
  await dialog.getByRole('button', { name: /^Options…/ }).click();
  await dialog.getByRole('textbox', { name: 'Name of the new thread' }).fill('Redirect experiment');
  await dialog.getByRole('radio', { name: /^Private/ }).click();
  await shot(page, 'fork-4-options');

  await dialog.getByRole('button', { name: 'Fork', exact: true }).click();
  await expect(dialog.getByText('Forking “Main thread”')).toBeVisible();
  await shot(page, 'fork-5-progress');
  await change.finish();
  await expect(dialog.getByText('The new thread is made, and it is yours.')).toBeVisible();
  await shot(page, 'fork-6-done');
  await dialog.getByRole('button', { name: 'Open the new thread' }).click();
  await expect(page).not.toHaveURL(new RegExp(MAIN));
  await expect(page.getByText('made this thread as a fork of Main thread')).toBeVisible();
  await expect(page.getByText('Redirect experiment').first()).toBeVisible();
  await shot(page, 'fork-7-new-thread');
});

test('a side thread is asked from a message, answered inline and in its panel, and kept as a thread', async ({ page }) => {
  await openHistory(page);
  await conversation(page).getByText('Please fix it, and run the tests.').first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Side thread from here…' }).click();
  const ask = page.getByRole('dialog', { name: 'Ask on the side' });
  await expect(ask.getByText('Side mode: Enforced read-only files', { exact: true })).toBeVisible();
  await expect(ask.getByText('It starts from the conversation up to turn 1.')).toBeVisible();
  await ask.getByRole('textbox', { name: 'What to ask' }).fill('Which tests cover the redirect?');
  await shot(page, 'side-1-ask');
  await ask.getByRole('button', { name: 'Ask', exact: true }).click();

  // Where the thread screen places it: under the line that says it began, with its own box.
  const lane = conversation(page).getByRole('region', { name: /^Side thread/ });
  await expect(lane).toBeVisible();
  await expect(lane.getByText('Side mode: Enforced read-only files', { exact: true })).toBeVisible();
  await expect(lane.getByText('Which tests cover the redirect?', { exact: true })).toBeVisible();
  await expect(lane.getByText(/^On the side, about “Which tests cover the redirect\?”/)).toBeVisible();
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Actions for Main thread', exact: true }).click();
  await page.getByRole('menuitem', { name: 'View', exact: true }).hover();
  await expect(page.getByRole('menuitem', { name: '1 side thread', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menuitem', { name: '1 side thread', exact: true })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menu')).toHaveCount(0);
  await lane.scrollIntoViewIfNeeded();
  await shot(page, 'side-2-answered');

  const mainBox = page.getByRole('textbox', { name: 'Message to the agent', exact: true });
  await mainBox.fill('Keep my next main task here');
  await lane.getByRole('textbox').fill('Which test covers expiry?');
  await lane.getByRole('textbox').press('Enter');
  await expect(lane.getByText(/^On the side, about “Which test covers expiry\?”/)).toBeVisible();
  await expect(mainBox).toHaveValue('Keep my next main task here');
  await page.setViewportSize({ width: 400, height: 900 });
  const send = lane.getByRole('button', { name: 'Send', exact: true });
  await expect(send).toBeVisible();
  expect((await send.boundingBox())?.height).toBeLessThanOrEqual(32);
  await lane.getByRole('button', { name: 'Open in panel', exact: true }).click();
  const panel = page.locator('[data-side-panel]');
  await expect(lane).toHaveCount(0);
  await expect(panel.getByText(/^On the side, about “Which test covers expiry\?”/)).toBeVisible();
  await expect(panel.getByRole('textbox')).toHaveValue('');
  // A phone shows one view at a time. Return to Main to read its retained draft, then reopen the side view.
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: 'Conversation', exact: true })).toHaveCount(0);
  await expect(mainBox).toHaveValue('Keep my next main task here');
  await page.getByRole('toolbar', { name: 'View containers', exact: true }).getByRole('button', { name: 'Conversation', exact: true }).click();
  await expect(panel).toBeVisible();
  await page.setViewportSize({ width: 1440, height: 900 });

  // Kept as a thread of the topic: the same wizard, for what it is.
  await panel.getByRole('button', { name: 'Keep as a thread…' }).click();
  const keep = wizard(page, /^Keep “Main thread \(side\)” as a thread/);
  await expect(keep).toBeVisible();
  await expect(keep.getByRole('combobox', { name: 'History', exact: true })).toContainText('Carry');
  await expect(keep.getByRole('combobox', { name: 'Environment', exact: true })).toContainText('Workspace · share files');
  await expect(keep.getByRole('region', { name: 'Changes', exact: true })).toContainText('The new thread will be yours');
  await expect(keep.getByRole('region', { name: 'Stays as it is', exact: true })).toContainText('The source thread is not touched');
  await shot(page, 'side-3-keep');
  await keep.getByRole('button', { name: 'Keep as a thread', exact: true }).click();
  await change.finish();
  await expect(keep.getByText('The new thread is made, and it is yours.')).toBeVisible();
  await shot(page, 'side-4-kept');
  await keep.getByRole('button', { name: 'Open the new thread' }).click();
  await expect(page).not.toHaveURL(new RegExp(MAIN));
});

test('a thread with as many side threads open as it may have says so, and one is closed to ask again', async ({ page }) => {
  await openHistory(page);
  await change.sides(MAIN, 4);
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Ask on the side…', exact: true }).click();
  const ask = page.getByRole('dialog', { name: 'Ask on the side' });
  await ask.getByRole('textbox', { name: 'What to ask' }).fill('One more question');
  await ask.getByRole('button', { name: 'Ask', exact: true }).click();
  await expect(ask.getByText('Main thread has 4 side threads open')).toBeVisible();
  await expect(ask.getByText('Close one to start another.')).toBeVisible();
  await shot(page, 'side-5-limit');
  await ask.getByRole('list', { name: 'The open side threads' }).getByRole('button', { name: 'Close' }).first().click();
  await expect(ask.getByText('Main thread has 4 side threads open')).toHaveCount(0);
  await ask.getByRole('button', { name: 'Ask', exact: true }).click();
  await expect(ask).toHaveCount(0);
  await expect(conversation(page).getByText('One more question').first()).toBeVisible();
});

const SANDBOX = RUNS_ON.sandbox;

test('an environment that is behind is brought up to date: its thread is told and held, and says the environment was made again', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/environments/${SANDBOX}`);
  await expect(page.getByText('It is behind')).toBeVisible();
  await page.getByRole('button', { name: 'Actions for Sandbox', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Update…', exact: true }).click();
  const dialog = wizard(page, 'Update Sandbox');
  await expect(dialog.getByRole('region', { name: 'Changes', exact: true })).toContainText('Exec node: 0.1.0 → 0.1.1');
  await expect(dialog.getByText('Image: orch-env-base:0.1.0 → orch-env-base:0.1.1')).toBeVisible();
  await expect(dialog.getByText('1 thread in it')).toBeVisible();
  await expect(dialog.getByText('This environment and its 1 thread stop taking turns until it is done.')).toBeVisible();
  await shot(page, 'update-1-plan');

  // The threads are told first, then held while the new unit takes over.
  await dialog.getByRole('button', { name: 'Update', exact: true }).click();
  await expect(dialog.getByText('Updating Sandbox')).toBeVisible();
  await expect(dialog.getByText('Notice', { exact: true })).toBeVisible();
  await shot(page, 'update-2-progress');
  await change.finish();
  await expect(dialog.getByText('Sandbox is up to date. The old unit is being removed.')).toBeVisible();
  await expect(dialog.getByText('Its thread takes messages again.')).toBeVisible();
  await shot(page, 'update-3-done');
  await dialog.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByText('It is behind')).toHaveCount(0);

  // The thread that runs in it was told.
  await page.goto(`/thread/${ACME.depsThread}`);
  await expect(page.getByText('The environment was made again from Default (updated)')).toBeVisible();
});

test('a move of an environment to another host is cancelled before its commit: what it made is removed and the environment stays where it was', async ({ page }) => {
  await change.host({ id: BUILD_HOST, schedulable: true });
  await signIn(page);
  await page.goto(`/resources/environments/${SANDBOX}`);
  await page.getByRole('button', { name: 'Actions for Sandbox' }).click();
  await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();
  const dialog = wizard(page, 'Move Sandbox');
  await expect(dialog).toBeVisible();
  await dialog.getByRole('combobox', { name: 'Place', exact: true }).click();
  await page.getByRole('option', { name: 'build-host', exact: true }).click();
  await expect(dialog.getByText('Moves from host-1 to build-host')).toBeVisible();
  await shot(page, 'move-1-plan');
  await dialog.locator('[data-confirm]').click();
  await expect(dialog.getByText('Moving Sandbox to build-host')).toBeVisible();
  await shot(page, 'move-2-progress');

  // Cancelled while it is under way: it asks first, and then everything it made is removed.
  await dialog.getByRole('button', { name: 'Cancel and undo' }).click();
  const sure = page.getByRole('alertdialog', { name: 'Cancel and undo?' });
  await expect(sure.getByText('Everything made so far is removed and Sandbox is as it was.')).toBeVisible();
  await shot(page, 'move-3-sure');
  await sure.getByRole('button', { name: 'Cancel and undo' }).click();
  await expect(dialog.getByText('Cancelled', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Everything it had made was removed. Sandbox is as it was before.')).toBeVisible();
  await shot(page, 'move-4-cancelled');
  await dialog.getByRole('button', { name: 'Close' }).last().click();
  await expect(page.getByText('On host host-1')).toBeVisible();
});

/** The menu of a topic in the tree of places: its entries are looked for in the tree, where each place has its own. */
async function openTopicMenu(page: Page, topic: string): Promise<void> {
  await page.getByRole('tree').getByRole('button', { name: `Actions for ${topic} · Web`, exact: true }).click();
}

test('the threads of a topic are switched together: the one that cannot be is named with what stops it, and the others go', async ({ page }) => {
  // Somebody else is switching Repro already, in an environment of its own: its plan is blocked, and the plan of the other thread is not.
  await change.rebind(ACME.repro, RUNS_ON.sandbox);
  await change.hold(ACME.repro, RUNS_ON.teamAnthropic);
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await openTopicMenu(page, 'Fix login');
  await page.getByRole('menuitem', { name: 'Switch provider for all threads…' }).click();
  const dialog = wizard(page, 'Switch the threads of “Fix login”');
  await expect(dialog.getByRole('combobox', { name: 'Provider', exact: true })).toContainText('Choose a provider');
  await expect(dialog.getByRole('combobox', { name: 'Agent', exact: true })).toBeDisabled();
  await expect(dialog.getByRole('combobox', { name: 'Model', exact: true })).toBeDisabled();
  await expect(dialog.getByRole('checkbox', { name: 'Main thread' })).toBeChecked();
  await expect(dialog.getByRole('checkbox', { name: 'Repro' })).toBeChecked();
  await shot(page, 'batch-1-start');

  await dialog.getByRole('combobox', { name: 'Provider' }).click();
  await page.getByRole('option', { name: /^Team Anthropic/ }).click();
  await dialog.getByRole('combobox', { name: 'Agent' }).click();
  await page.getByRole('option', { name: /^Codex\b/ }).first().click();
  await dialog.getByRole('combobox', { name: 'Model' }).click();
  await page.getByRole('option', { name: /^Claude Sonnet 5/ }).click();
  await expect(dialog.getByText('1 of 2 threads can be switched; 1 cannot.')).toBeVisible();
  const blocked = dialog.locator('[data-plan-row][data-blocked]');
  await expect(blocked).toHaveCount(1);
  await expect(blocked.getByText('Repro')).toBeVisible();
  await expect(blocked.getByText('Another fork, switch, move or update is already under way here')).toBeVisible();
  await shot(page, 'batch-2-plans');

  // The one that can go goes: one bar for all of it and a row for each thread.
  await dialog.locator('[data-confirm]').click();
  await expect(dialog.getByText('Switching the threads')).toBeVisible();
  await expect(dialog.locator('[data-batch-job]')).toHaveCount(1);
  await shot(page, 'batch-3-progress');
  await change.finish();
  await expect(dialog.getByText('The switch is over')).toBeVisible();
  await expect(dialog.getByText('1 done · 0 under way · 0 waiting · 0 failed · 0 stopped')).toBeVisible();
  await shot(page, 'batch-4-over');
});

test('a batch is watched while it runs, counts the threads that its reader cannot see, and is cancelled and undone', async ({ page }) => {
  // Everything that runs in the environment is meant, the thread that its reader cannot see among it.
  await change.hide([ACME.repro]);
  await signIn(page);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  await page.getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('button', { name: "Switch every thread's provider…" }).click();
  const dialog = wizard(page, 'Switch the threads in “Workspace”');
  await expect(dialog.getByText('1 more that you cannot see')).toBeVisible();
  await dialog.getByRole('combobox', { name: 'Provider' }).click();
  await page.getByRole('option', { name: /^Team Anthropic/ }).click();
  await dialog.getByRole('combobox', { name: 'Agent' }).click();
  await page.getByRole('option', { name: /^Codex\b/ }).first().click();
  await dialog.getByRole('combobox', { name: 'Model' }).click();
  await page.getByRole('option', { name: /^Claude Sonnet 5/ }).click();
  await expect(dialog.locator('[data-batch-summary]')).toContainText('can be switched');
  await expect(dialog.getByText('A thread you cannot see')).toBeVisible();
  await shot(page, 'batch-5-hidden');

  await dialog.locator('[data-confirm]').click();
  await expect(dialog.getByText('Switching the threads')).toBeVisible();
  await expect(dialog.locator('[data-batch-counts]')).toContainText('1 more that you cannot see');
  await shot(page, 'batch-6-watched');
  await dialog.getByRole('button', { name: 'Cancel and undo' }).click();
  const sure = page.getByRole('alertdialog', { name: 'Cancel and undo all of it?' });
  await expect(sure.getByText('The threads whose switch has not committed are as they were. Those that are done stay switched.')).toBeVisible();
  await sure.getByRole('button', { name: 'Cancel and undo' }).click();
  await expect(dialog.getByText('The switch is over')).toBeVisible();
  await expect(dialog.locator('[data-batch-counts]')).toContainText('0 done');
  await expect(dialog.locator('[data-batch-counts]')).toContainText('1 more that you cannot see');
  await shot(page, 'batch-7-cancelled');
});

test('what is idle on a host is listed exactly as it will be stopped, and stopped', async ({ page }) => {
  // The one environment of the host that is made by it is running, and has been idle for two hours.
  await change.environment({ id: SANDBOX, status: 'running' });
  await change.idle({ [SANDBOX]: 120 });
  await signIn(page);
  await page.goto(`/resources/hosts/${HOST_ONE}`);
  await page.getByRole('button', { name: 'Actions for host-1' }).click();
  await page.getByRole('menuitem', { name: 'Recycle idle…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Recycle idle on host-1' });
  await expect(dialog.getByText(/^Stops 1 environment\. Its files stay; it starts again when a message arrives\./)).toBeVisible();
  await expect(dialog.getByRole('listitem').filter({ hasText: 'Sandbox' })).toContainText('idle 120 min');
  await shot(page, 'recycle-1-list');

  // What is asked for decides what is listed, and what is listed is what stops.
  const minutes = dialog.getByRole('spinbutton', { name: 'Idle for at least (minutes)' });
  await minutes.fill('240');
  await expect(dialog.getByText('Nothing here has been idle that long.')).toBeVisible();
  await minutes.fill('60');
  await expect(dialog.getByRole('button', { name: 'Stop 1 environment' })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Stop 1 environment' }).click();
  await expect(dialog.getByText('Stopped 1 environment')).toBeVisible();
  await expect(dialog.getByText('Their files stay. Each starts again when a message arrives for a thread in it.')).toBeVisible();
  await shot(page, 'recycle-2-stopped');
});

test('a fork is made from the keyboard alone: the menu, the choices, the confirmation, the way to the new thread', async ({ page }) => {
  await openHistory(page);
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Actions for Main thread', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('menu')).toBeVisible();
  // The header's Fork to action opens the choices this keyboard flow exercises.
  await page.keyboard.press('Escape');
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Fork to…', exact: true }).focus();
  await page.keyboard.press('Enter');
  const dialog = wizard(page, 'Fork “Main thread”');
  await expect(dialog).toBeVisible();

  // A normal select keeps its current value until Enter confirms the highlighted option.
  await tabTo(page, 'History');
  const history = dialog.getByRole('combobox', { name: 'History', exact: true });
  await expect(history).toBeFocused();
  await expect(history).toContainText('Carry');
  await page.keyboard.press('Enter');
  // Summary can always be chosen: without a summarizer the last turns go briefly.
  for (const next of ['Carry through a turn', 'Carry some turns', 'Summary', 'None']) {
    await page.keyboard.press('ArrowDown', { delay: 60 });
    await expect(page.getByRole('option', { name: next, exact: true })).toBeFocused();
  }
  await page.keyboard.press('Enter');
  await expect(history).toContainText('None');

  // Confirmed with the key that presses a button, and the job is followed to the way into the new thread.
  await tabTo(page, 'Fork');
  await page.keyboard.press('Enter');
  await expect(dialog.getByText('Forking “Main thread”')).toBeVisible();
  await change.finish();
  await expect(dialog.getByText('The new thread is made, and it is yours.')).toBeVisible();
  await tabTo(page, 'Open the new thread');
  await page.keyboard.press('Enter');
  await expect(page).not.toHaveURL(new RegExp(MAIN));
  await expect(page.getByText('made this thread as a fork of Main thread')).toBeVisible();
});

test('a switch is planned, confirmed and hidden from the keyboard alone, and Escape leaves a plan without doing it', async ({ page }) => {
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).focus();
  await page.keyboard.press('Enter');
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();

  // Escape leaves it: nothing was started.
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);
  expect(await change.jobs()).toEqual([]);

  // Asked again, confirmed and watched.
  await page.getByRole('button', { name: 'Switch provider…' }).focus();
  await page.keyboard.press('Enter');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await tabTo(page, 'Switch');
  await page.keyboard.press('Enter');
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  await tabTo(page, 'Hide');
  await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0);
  await expect(page.locator('[data-lock]')).toBeVisible();
  await change.finish();
  await expect(conversation(page).getByText(/^Ada Lovelace switched to Codex with OpenAI key(,|$)/)).toBeVisible();
});

test('a side thread is asked, kept in view and closed from the keyboard alone', async ({ page }) => {
  await openHistory(page);
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Ask on the side…', exact: true }).focus();
  await page.keyboard.press('Enter');
  const ask = page.getByRole('dialog', { name: 'Ask on the side' });
  await expect(ask.getByRole('textbox', { name: 'What to ask' })).toBeFocused();
  await page.keyboard.type('Is the redirect covered?');
  await page.keyboard.press('Enter');
  const lane = conversation(page).getByRole('region', { name: /^Side thread/ });
  await expect(lane.getByText('Is the redirect covered?', { exact: true })).toBeVisible();
  await expect(lane.getByText(/^On the side, about/)).toBeVisible();
  await lane.getByRole('button', { name: 'Close' }).focus();
  await page.keyboard.press('Enter');
  await expect(lane).toHaveCount(0);
  await expect(page.getByText('Main thread (side) · 1 turn · closed', { exact: true })).toBeVisible();
});

test('a composer aside with a lost reply retries its original act after navigation and leaves newer words alone', async ({ page }) => {
  await openHistory(page);
  const original = 'Which test covers the redirect?';
  const newer = 'Next I want to ask about the session cookie';
  const box = page.getByRole('textbox', { name: 'Message to the agent', exact: true });
  const bodies: unknown[] = [];
  let release: () => void = () => undefined;
  const waitForRetry = new Promise<void>((resolve) => { release = resolve; });
  await page.route(`**/api/v1/threads/${MAIN}/side`, async (route) => {
    if (route.request().method() !== 'POST') { await route.continue(); return; }
    const body: unknown = route.request().postDataJSON();
    bodies.push(body);
    if (bodies.length === 1) { await route.fetch(); await route.abort('connectionreset'); return; }
    await waitForRetry;
    const response = await route.fetch();
    await route.fulfill({ response });
  });
  try {
    await box.fill(original);
    await page.getByRole('button', { name: 'More ways to send', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Ask on the side', exact: true }).click();
    const retry = page.getByRole('button', { name: 'Retry aside', exact: true });
    await expect(retry).toBeVisible();
    await expect(box).toHaveValue(original);
    await box.fill(newer);
    await page.getByRole('navigation', { name: 'Orgs and your own places', exact: true }).getByRole('link', { name: /^Inbox/ }).click();
    await expect(page).toHaveURL(/\/inbox$/);
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`/thread/${MAIN}$`));
    await expect(box).toHaveValue(newer);
    await expect(retry).toBeVisible();
    await retry.click();
    await expect(retry).toBeFocused();
    await expect(retry).toHaveAttribute('aria-busy', 'true');
    await expect.poll(() => bodies.length).toBe(2);
    expect(bodies[1]).toEqual(bodies[0]);
    await box.fill(`${newer}, without resending the aside`);
    release();
    await expect(retry).toHaveCount(0);
    await expect(box).toHaveValue(`${newer}, without resending the aside`);
    await page.locator('[data-thread-header]').getByRole('button', { name: 'Actions for Main thread', exact: true }).click();
    await page.getByRole('menuitem', { name: 'View', exact: true }).hover();
    await expect(page.getByRole('menuitem', { name: '1 side thread', exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');
    expect(bodies).toHaveLength(2);
  } finally { release(); }
});
