/** Starting work, asked in both themes whether anything on it is a serious barrier: the inline starter of a topic and of a project where the defaults are whole, where one is missing, where a choice was made and may be kept, where the person may only ask, and where a harness has not reported; and the dialog with its closed way, its new environment and its refusal, on a large screen and on a phone. */
import { expect, type Page, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, deny, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';
import { agentReports, mayNotMake, setDefaults, starter, startDialog } from '../e2e/start-support.ts';

test.beforeEach(async () => {
  await scenario();
});

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

const missing = (page: Page) => starter(page).getByRole('group', { name: 'What is missing' });

test('the topic starter and the project header where the defaults are whole', async ({ page }) => {
  await open(page, `/topic/${ACME.fixLogin}`);
  // What a thread would run on is named part by part.
  for (const part of ['Codex', 'OpenAI key', 'GPT-5.4', 'High', 'Workspace']) await expect(starter(page).locator('[data-runs-on]')).toContainText(part);
  await expectAccessibleInBothThemes(page);

  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByTestId('main').getByRole('button', { name: 'New topic', exact: true })).toBeVisible();
  await expect(starter(page, 'project')).toHaveCount(0);
  await expectAccessibleInBothThemes(page);
});

test('the starter where a configured default is unavailable, with an explicit replacement and the way to keep it', async ({ page }) => {
  // A provider at its limit can still be chosen, and its threads wait for the reset; one that needs setting up cannot.
  await setDefaults(ACME.idleStop, { 'agent.default_provider_id': RUNS_ON.freshKey });
  await open(page, `/topic/${ACME.idleStop}`);
  await expect(missing(page)).toContainText('it needs a credential or a model before it can be used');
  await expectAccessibleInBothThemes(page);

  await missing(page).getByRole('combobox', { name: 'Provider' }).click();
  await page.getByRole('option', { name: 'Team Anthropic', exact: true }).click();
  await expect(missing(page)).toContainText('This choice is for this thread.');
  await expect(missing(page).getByRole('button', { name: 'Set as default' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the starter of somebody who may not set the defaults, with the request that is open', async ({ page }) => {
  await setDefaults(ACME.idleStop, { 'agent.default_provider_id': RUNS_ON.claudeSubscription });
  await deny('dora', ['topic.manage']);
  await open(page, `/topic/${ACME.idleStop}`, PEOPLE.dora);
  await expect(missing(page)).toContainText('takes managing it');
  await expectAccessibleInBothThemes(page);

  await missing(page).getByRole('button', { name: 'Request access' }).click();
  await expect(page.getByRole('dialog', { name: 'Ask to set the defaults of Idle-stop rework' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the starter where no harness has reported yet', async ({ page }) => {
  for (const agent of ['Codex', 'Scripted agent', 'Codex on ada-laptop']) await agentReports(agent, false);
  await setDefaults(ACME.idleStop, { 'agent.default_provider_id': RUNS_ON.openAiKey, 'agent.default_agent_id': RUNS_ON.codex, 'agent.model': 'gpt-5.4' });
  await open(page, `/topic/${ACME.idleStop}`);
  await expect(missing(page)).toContainText('Codex has not reported its capabilities');
  await expectAccessibleInBothThemes(page);
});

test('the dialog, with the way that is not allowed, the new environment and the refusal of a way', async ({ page }) => {
  await open(page, `/topic/${ACME.fixLogin}`);
  await page.getByTestId('main').getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  const dialog = startDialog(page);
  await dialog.getByRole('button', { name: 'Options', exact: true }).click();
  await expect(dialog.getByRole('radio', { name: /Copy sign-in access/ })).toBeDisabled();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog.getByRole('button', { name: 'New environment…' }).click();
  await expect(page.getByRole('dialog', { name: 'Add environment' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.getByRole('dialog', { name: 'Add environment' }).getByRole('button', { name: 'Cancel' }).click();

  await mayNotMake('ada');
  await page.reload();
  await page.getByTestId('main').getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  await startDialog(page).getByRole('button', { name: 'Options', exact: true }).click();
  // The compact choice keeps a blocked option's reason as its accessible description, and shows it where a pointer rests on the option.
  const placing = startDialog(page).getByRole('radiogroup').getByRole('radio', { name: 'Create new', exact: true });
  await expect(placing).toBeDisabled();
  await expect(placing).toHaveAccessibleDescription(/You may not make an environment here/);
  await placing.hover();
  await expect(page.getByRole('tooltip').filter({ hasText: 'You may not make an environment here' })).toBeVisible();
  await page.mouse.move(0, 0);
  // A disabled option takes no keyboard focus; the same reason heads the disclosure that says how to ask, which the keyboard opens and which is audited open.
  const asking = startDialog(page).locator('summary').filter({ hasText: /^You may not make an environment here$/ });
  await asking.focus();
  await page.keyboard.press('Enter');
  await expect(startDialog(page).getByRole('button', { name: /Send|Request/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the starter and the dialog on a phone', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await open(page, `/topic/${ACME.idleStop}`);
  await expect(starter(page).locator('[data-runs-on]')).toBeVisible();
  await expect(missing(page)).toHaveCount(0);
  await expectAccessibleInBothThemes(page);

  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(starter(page).locator('[data-runs-on]')).toContainText('OpenAI key');
  await starter(page).getByRole('button', { name: 'Thread setup' }).click();
  await expect(startDialog(page)).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});
