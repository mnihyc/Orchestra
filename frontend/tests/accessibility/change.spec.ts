/** Every dialog of changing where and on what work runs, asked in both themes whether anything on it is a serious barrier, at every step it has: the plan with its choices opened, what stands in its way, the job under way, and the end that worked, failed or was cancelled. */
import { expect, test } from '@playwright/test';
import type { CopyDestinationsRequest } from '../../src/api/copyDestinations';
import type { AgentAccessAvailability, ForkPlan, ForkRequest } from '../../src/generated/api-types';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { BLOCKED, BUILD_HOST, change, conversation, HOST_ONE, MAIN, openBlocked, openHistory, shot, wizard } from '../e2e/change-support.ts';
import { ACME, emit, expectAccessibleInBothThemes, playScene, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async ({ page }) => {
  await scenario();
  await change.mode('manual');
  // These scenes contain only same-org topic destinations; the native fork plan still settles each choice.
  // Supply the newer read-only preflights which the original change double predates.
  await page.route('**/api/v1/copy-destinations/judge', async (route) => {
    const body = route.request().postDataJSON() as CopyDestinationsRequest;
    await route.fulfill({ json: { ...body, source_owned: true, destinations: body.destinations.map((destination) => ({
      destination, cross_root: false, outcome: { state: 'needs_choice', choice: 'fork_plan' },
    })) } });
  });
  await page.route('**/api/v1/threads/*/forks/preflight', async (route) => {
    const response = await route.fetch();
    if (!response.ok()) { await route.fulfill({ response }); return; }
    const plan = await response.json() as ForkPlan;
    const agent_access: AgentAccessAvailability = { preset: { where: 'topic', parts: { documents: 'edit' } }, choices: [
      { where: 'nothing', parts: {} }, ...(['thread', 'topic', 'project'] as const).map((where) => ({ where, parts: { documents: 'edit' as const } })),
    ] };
    await route.fulfill({ response, json: { ...plan, ...(plan.result === 'new' ? { agent_access } : {}) } });
  });
});

test('a switch: the plan with its provider opened, what stands in the way of it, the job under way, and an end that failed', async ({ page }) => {
  const requested: ForkRequest[] = [];
  await page.route('**/api/v1/threads/*/forks/preflight', async (route) => {
    requested.push(route.request().postDataJSON() as ForkRequest);
    await route.fallback();
  });
  await openBlocked(page);
  await page.getByRole('button', { name: 'Switch provider…' }).click();
  const dialog = wizard(page, 'Switch “Main thread”');
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  // The editor of what it runs on is open, and the plan says that a model is not offered any more.
  await change.model(RUNS_ON.openAiKey, 'gpt-5.4', false);
  // Provider, model, effort and history are inline now; the first audit includes the focused provider picker.
  const effort = dialog.getByRole('combobox', { name: 'Effort' });
  // Either supported effort forces a new plan when it differs from the automatically chosen value.
  const nextEffort = (await effort.innerText()).includes('Low') ? 'Medium' : 'Low';
  await effort.click();
  await page.getByRole('option', { name: nextEffort, exact: true }).click();
  await expect(dialog.getByText('Model not allowed', { exact: true })).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeDisabled();
  await expectAccessibleInBothThemes(page);

  // The model is changed and the plan is once more clear.
  await dialog.getByRole('combobox', { name: 'Model' }).click();
  // This offered mini model deliberately has no display name; its request still identifies the chosen model.
  await page.getByRole('option', { name: 'Model name unavailable', exact: true }).click();
  await expect.poll(() => requested.at(-1)?.to?.model).toBe('gpt-5.4-mini');
  // Compact provider switching leaves environment choices to the environment dialog audited below.
  await expect(dialog.getByRole('combobox', { name: 'History', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await expect(dialog.getByRole('button', { name: 'Switch', exact: true })).toBeEnabled();

  // Under way, and failed with what was undone.
  await change.fail('verify', 'verify_failed', 'the tools could not reach the new side', true);
  await dialog.getByRole('button', { name: 'Switch', exact: true }).click();
  await expect(dialog.getByText('Switching to OpenAI key')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await change.finish();
  // Compact results put the failed check and rollback together in their alert.
  const failed = dialog.getByRole('alert');
  await expect(failed).toContainText('The new side did not pass:');
  await expect(failed).toContainText('Everything it had made was removed. Main thread is as it was before.');
  await expect(dialog.getByRole('button', { name: 'Try again', exact: true })).toBeVisible();
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
  await expect(page.getByText(/switched to Codex with OpenAI key, carrying \d+ items of the conversation/)).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a fork: the plan with each of its choices opened, the options, the job, and the end', async ({ page }) => {
  await openHistory(page);
  await conversation(page).getByText('The handler reads').first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Fork from here…' }).click();
  const dialog = wizard(page, 'Fork “Main thread”');
  // Inline history controls replaced the old summary sentence and radio disclosure.
  await expect(dialog.getByRole('combobox', { name: 'History', exact: true })).toContainText('Carry through a turn');
  await expect(dialog.getByRole('spinbutton', { name: 'Last turn to take' })).toHaveValue('1');
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('combobox', { name: 'History', exact: true }).click();
  await expect(page.getByRole('option', { name: 'Carry through a turn', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await dialog.getByRole('combobox', { name: 'Provider', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Provider', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await dialog.getByRole('combobox', { name: 'Environment', exact: true }).click();
  await page.getByRole('option', { name: 'Copy to new', exact: true }).click();
  await dialog.getByRole('radio', { name: 'By pattern', exact: true }).click();
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
  const ask = page.getByRole('dialog', { name: 'Side thread' });
  await ask.getByRole('textbox', { name: 'What to ask' }).fill('Which tests cover the redirect?');
  await expectAccessibleInBothThemes(page);
  await ask.getByRole('button', { name: 'Start side thread', exact: true }).click();

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
  await page.locator('[data-thread-header] [data-thread-more]').click();
  await page.getByRole('menuitem', { name: 'Side thread…', exact: true }).click();
  await ask.getByRole('textbox', { name: 'What to ask' }).fill('One more question');
  await ask.getByRole('button', { name: 'Start side thread', exact: true }).click();
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
  // The plan lists each actual version change instead of the former summary sentence.
  await expect(update.getByRole('region', { name: 'Changes', exact: true })).toContainText('Image: orch-env-base:0.1.0 → orch-env-base:0.1.1');
  await expect(update.getByRole('region', { name: 'Changes', exact: true })).toContainText('Exec node: 0.1.0 → 0.1.1');
  await expectAccessibleInBothThemes(page);
  await update.getByRole('button', { name: 'Update', exact: true }).click();
  await expect(update.getByText('Updating Sandbox')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await change.finish();
  await expect(update.getByText('Sandbox is up to date. The old unit is being removed.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await update.getByRole('button', { name: 'Done' }).click();

  await page.getByRole('button', { name: 'Actions for Sandbox' }).click();
  await page.getByRole('menuitem', { name: 'Move to another machine…', exact: true }).click();
  const move = wizard(page, 'Move Sandbox');
  await move.getByRole('combobox', { name: 'Place', exact: true }).click();
  await page.getByRole('option', { name: /^build-host/ }).click();
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
  await page.getByRole('tree').getByRole('button', { name: /^Actions for Fix login(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Switch threads…', exact: true }).click();
  const dialog = wizard(page, 'Switch the threads of “Fix login”');
  // The target is chosen inline now, without the old introductory sentence.
  await expect(dialog.getByRole('region', { name: 'What they go to', exact: true })).toBeVisible();
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
  await expect(dialog.getByRole('button', { name: '切换', exact: true })).toBeEnabled();
  await page.keyboard.press('Escape');
  // Compact plans use the inline pickers instead of the former summary sentence.
  await expect(dialog.getByRole('combobox', { name: '代理与提供方', exact: true })).toHaveValue(/OpenAI key/);
  await shot(page, 'zh-switch-1-plan');
  await expectAccessibleInBothThemes(page);

  await dialog.getByRole('combobox', { name: '代理与提供方', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '代理与提供方', exact: true })).toBeVisible();
  await shot(page, 'zh-switch-2-editor');
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await dialog.getByRole('button', { name: '切换', exact: true }).click();
  // Compact progress names the operation directly instead of drawing the former step heading.
  await expect(dialog.getByRole('progressbar', { name: '正在切换到 OpenAI key', exact: true })).toBeVisible();
  await change.finish();
  await expect(dialog.getByText('完成', { exact: true }).first()).toBeVisible();
  await shot(page, 'zh-switch-3-done');
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('button', { name: '完成' }).last().click();

  // A fork of the main thread, its options opened.
  await emit(MAIN, { thread: { state: 'idle' } });
  await playScene(MAIN, 'history');
  await page.goto(`/thread/${MAIN}`);
  await expect(page.getByRole('log', { name: '对话', exact: true }).getByText('Please fix it, and run the tests.')).toBeVisible();
  // The act keeps its handler in the menu of the head.
  await page.locator('[data-thread-header] [data-thread-more]').click();
  const forkAction = page.getByRole('menuitem', { name: '创建副本到…', exact: true });
  await expect(forkAction).toBeVisible();
  await forkAction.click();
  const fork = wizard(page, '分叉“Main thread”');
  await expect(fork.getByRole('combobox', { name: '历史', exact: true })).toBeVisible();
  await shot(page, 'zh-fork-1-plan');
  await expectAccessibleInBothThemes(page);
  await fork.getByRole('button', { name: /^选项…/ }).click();
  await shot(page, 'zh-fork-2-options');
  await expectAccessibleInBothThemes(page);
});
