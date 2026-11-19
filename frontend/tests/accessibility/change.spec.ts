/** Every dialog of changing where and on what work runs, asked in both themes whether anything on it is a serious barrier, at every step it has: the plan with its choices opened, what stands in its way, the job under way, and the end that worked, failed or was cancelled. */
import { expect, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { BLOCKED, BUILD_HOST, change, conversation, HOST_ONE, MAIN, openBlocked, openHistory, shot, wizard } from '../e2e/change-support.ts';
import { ACME, emit, expectAccessibleInBothThemes, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
  await change.mode('manual');
});

test('a switch: the plan with its provider opened, what stands in the way of it, the job under way, and an end that failed', async ({ page }) => {
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await expectAccessibleInBothThemes(page);

  // The editor of what it runs on is open, and the plan says that a model is not offered any more.
  await change.model(RUNS_ON.openAiKey, 'gpt-5.4', false);
  await dialog.getByRole('button', { name: /^Provider and model/ }).click();
  await dialog.getByRole('combobox', { name: 'Effort' }).click();
  await page.getByRole('option', { name: 'high' }).click();
  await expect(dialog.getByText('Choose a model it offers.')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The environment's editor, and the plan once more clear.
  await dialog.getByRole('combobox', { name: 'Model' }).click();
  await page.getByRole('option', { name: 'gpt-5.4-mini' }).click();
  await dialog.getByRole('button', { name: 'Environment', exact: true }).click();
  await expect(dialog.getByRole('radio', { name: 'New', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('button', { name: 'Environment', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();

  // Under way, and failed with what was undone.
  await change.fail('verify', 'verify_failed', 'the tools could not reach the new side', true);
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await change.finish();
  await expect(dialog.getByText('It did not work, in the checks')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a switch that is cancelled: the question first, then the end that says so', async ({ page }) => {
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  await dialog.getByRole('button', { name: 'Cancel and undo' }).click();
  const sure = page.getByRole('alertdialog', { name: 'Cancel and undo?' });
  await expect(sure).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await sure.getByRole('button', { name: 'Cancel and undo' }).click();
  await expect(dialog.getByText('Cancelled', { exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a switch that is done: the thread held while it is under way, and the thread on its new side', async ({ page }) => {
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  await dialog.getByRole('button', { name: 'Hide' }).click();
  await expect(page.locator('[data-lock]')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await change.finish();
  await expect(page.getByText('switched this thread to OpenAI key · Codex')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a fork: the plan with each of its choices opened, the options, the job, and the end', async ({ page }) => {
  await openHistory(page);
  await conversation(page).getByText('The handler reads').first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Fork from here…' }).click();
  const dialog = wizard(page, 'Fork “Main thread”');
  await expect(dialog.getByText('its history up to turn 1')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('button', { name: 'History', exact: true }).click();
  await expect(dialog.getByRole('radio', { name: 'Up to a turn', exact: true })).toBeChecked();
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('button', { name: 'Provider and model' }).click();
  await expect(dialog.getByRole('combobox', { name: 'Provider' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('button', { name: 'Environment', exact: true }).click();
  await dialog.getByRole('radio', { name: 'New', exact: true }).click();
  await dialog.getByRole('radio', { name: 'Selection', exact: true }).click();
  await dialog.getByRole('textbox', { name: 'Take these' }).fill('src/**');
  await expect(dialog.getByText(/^Files: \d+ files?, /)).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('button', { name: /^Options…/ }).click();
  await expect(dialog.getByRole('textbox', { name: 'Name of the new thread' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('button', { name: 'Fork', exact: true }).click();
  await expect(dialog.getByText('Forking “Main thread”')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await change.finish();
  await expect(dialog.getByText('The new thread is made, and it is yours.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a side thread: the dialog that asks, the lane that answers, the limit that is reached and the thread it is kept as', async ({ page }) => {
  await openHistory(page);
  await conversation(page).getByText('Please fix it, and run the tests.').first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Side thread from here…' }).click();
  const ask = page.getByRole('dialog', { name: 'Ask on the side' });
  await ask.getByRole('textbox', { name: 'What to ask' }).fill('Which tests cover the redirect?');
  await expectAccessibleInBothThemes(page);
  await ask.getByRole('button', { name: 'Ask', exact: true }).click();

  const lane = conversation(page).getByRole('region', { name: /^Side thread/ });
  await expect(lane.getByText(/^On the side, about/)).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await lane.getByRole('button', { name: 'Keep as a thread…' }).click();
  const keep = wizard(page, /^Keep “Main thread \(side\)” as a thread/);
  await expect(keep).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await keep.getByRole('button', { name: 'Cancel' }).click();
  await expect(keep).toHaveCount(0);

  // The limit: the dialog that says how many are open, with a way to close each.
  await change.sides(MAIN, 3);
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Ask on the side…', exact: true }).click();
  await ask.getByRole('textbox', { name: 'What to ask' }).fill('One more question');
  await ask.getByRole('button', { name: 'Ask', exact: true }).click();
  await expect(ask.getByText('Close one to start another.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('an environment: the update, the move that is cancelled, and what is idle on a host', async ({ page }) => {
  await change.host({ id: BUILD_HOST, schedulable: true });
  await change.environment({ id: RUNS_ON.sandbox, status: 'running' });
  await change.idle({ [RUNS_ON.sandbox]: 120 });
  await signIn(page);
  await page.goto(`/resources/environments/${RUNS_ON.sandbox}`);
  await page.getByRole('button', { name: 'Update…' }).click();
  const update = wizard(page, 'Update Sandbox');
  await expect(update.getByText('Brings image and executor up to date')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await update.getByRole('button', { name: 'Update', exact: true }).click();
  await expect(update.getByText('Updating Sandbox')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await change.finish();
  await expect(update.getByText('Sandbox is up to date. The old unit is being removed.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await update.getByRole('button', { name: 'Done' }).click();

  await page.getByRole('button', { name: 'Actions for Sandbox' }).click();
  await page.getByRole('menuitem', { name: 'Move…', exact: true }).click();
  const move = wizard(page, 'Move Sandbox');
  await move.getByRole('button', { name: 'Host', exact: true }).click();
  await move.getByRole('radio', { name: /build-host/ }).click();
  await expect(move.getByText('Moves from host-1 to build-host')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await move.locator('[data-confirm]').click();
  await expect(move.getByText('Moving Sandbox to build-host')).toBeVisible();
  await move.getByRole('button', { name: 'Cancel and undo' }).click();
  await page.getByRole('alertdialog', { name: 'Cancel and undo?' }).getByRole('button', { name: 'Cancel and undo' }).click();
  await expect(move.getByText('Cancelled', { exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await move.getByRole('button', { name: 'Close' }).last().click();

  await page.goto(`/resources/hosts/${HOST_ONE}`);
  await page.getByRole('button', { name: 'Actions for host-1' }).click();
  await page.getByRole('menuitem', { name: 'Recycle idle…' }).click();
  const recycle = page.getByRole('dialog', { name: 'Recycle idle on host-1' });
  await expect(recycle.getByText(/^Stops 1 environment\./)).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('several threads at once: the plans with one that is blocked, the batch under way, and its end', async ({ page }) => {
  await change.rebind(ACME.repro, RUNS_ON.sandbox);
  await change.hold(ACME.repro, RUNS_ON.teamAnthropic);
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('tree').getByRole('button', { name: 'More about Fix login' }).click();
  await page.getByRole('menuitem', { name: 'Switch provider for all threads…' }).click();
  const dialog = wizard(page, 'Switch the threads of “Fix login”');
  await expect(dialog.getByText('Choose the provider these threads should go on with.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('combobox', { name: 'Provider' }).click();
  await page.getByRole('option', { name: /^Team Anthropic/ }).click();
  await dialog.getByRole('combobox', { name: 'Model' }).click();
  await page.getByRole('option', { name: /^Claude Sonnet 5/ }).click();
  await expect(dialog.getByText('1 of 2 threads can be switched; 1 cannot.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.locator('[data-confirm]').click();
  await expect(dialog.getByText('Switching the threads')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await change.finish();
  await expect(dialog.getByText('The switch is over')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('in Chinese: the plan of a switch and of a fork, each with its choices opened, and the end of a job', async ({ page }) => {
  // The language is the person's own, chosen on Interface.
  await signIn(page);
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: 'Language' }).click();
  await page.getByRole('option', { name: '简体中文' }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await page.goto(`/thread/${BLOCKED}`);
  await page.getByRole('button', { name: '切换提供方…' }).click();
  const dialog = wizard(page, '切换“Main thread”');
  await expect(dialog.getByRole('group', { name: '将要发生的事' })).toContainText('让此会话在');
  await expect(dialog.getByRole('button', { name: '切换', exact: true })).toBeEnabled();
  await shot(page, 'zh-switch-1-plan');
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('button', { name: '提供方与模型' }).click();
  await expect(dialog.getByRole('combobox', { name: '提供方' })).toBeVisible();
  await shot(page, 'zh-switch-2-editor');
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('button', { name: '切换', exact: true }).click();
  await expect(dialog.getByText('进度')).toBeVisible();
  await change.finish();
  await expect(dialog.getByText('完成', { exact: true }).first()).toBeVisible();
  await shot(page, 'zh-switch-3-done');
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('button', { name: '完成' }).last().click();

  // A fork of the main thread, its options opened.
  await emit(MAIN, { thread: { state: 'idle' } });
  await page.goto(`/thread/${MAIN}`);
  await page.getByRole('button', { name: '会话菜单' }).click();
  await page.getByRole('menuitem', { name: '分叉…' }).click();
  const fork = wizard(page, '分叉“Main thread”');
  await expect(fork.getByRole('group', { name: '将要发生的事' })).toContainText('分叉此会话，带上');
  await shot(page, 'zh-fork-1-plan');
  await expectAccessibleInBothThemes(page);
  await fork.getByRole('button', { name: /^选项…/ }).click();
  await shot(page, 'zh-fork-2-options');
  await expectAccessibleInBothThemes(page);
});
