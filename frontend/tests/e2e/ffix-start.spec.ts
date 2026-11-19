/** Starting work as one act: typing in a topic or a project where the defaults are whole, what is said and asked where one is missing, a harness that has not reported yet, and the dialog for whoever wants to choose, where what is shown as chosen is what is sent and a way that cannot be had is shown off with the server's reason. */
import { expect, type Page, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { agentMessage } from '../doubles/thread-rows.ts';
import { ACME, deny, emit, PEOPLE, scenario, signIn } from './support.ts';
import { agentReports, bareEnvironment, mayNotMake, openedThread, requests, requestsOf, serveOptions, setDefaults, starter, starterBox, startDialog } from './start-support.ts';

test.beforeEach(async () => {
  await scenario();
});

const MODEL = 'gpt-5.4';

/** Opens the dialog that makes a thread, from the button of the topic's threads. */
async function openDialog(page: Page, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByTestId('main').getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  await expect(startDialog(page)).toBeVisible();
  await startDialog(page).getByRole('button', { name: 'Options', exact: true }).click();
}

const arrangements = (page: Page) => startDialog(page).getByRole('radiogroup', { name: 'Where the agent runs' });

test('typing in a topic whose defaults are whole starts a thread with one Enter, and its first answer comes into it', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  // The line under the box says what will happen.
  await expect(starter(page).getByText('Codex on OpenAI key, GPT-5.4 (High), in Workspace')).toBeVisible();

  const text = 'Why does the login page send people to the wrong place?';
  await starterBox(page).fill(text);
  await starterBox(page).press('Enter');
  const thread = await openedThread(page);
  await expect(page.getByText(text)).toBeVisible();

  // One request made it, with everything that was shown as chosen, the arrangement among it.
  const made = await requestsOf('POST', new RegExp(`/topics/${ACME.fixLogin}/threads$`));
  expect(made).toHaveLength(1);
  expect(made[0]?.body).toMatchObject({
    binding: { agent_id: RUNS_ON.codex, provider_id: RUNS_ON.openAiKey, arrangement: 'separated' },
    model: MODEL,
    effort: 'high',
    environment: { use: RUNS_ON.workspace },
    first_input: { content: [{ type: 'text', text }] },
  });
  // The topic had everything it needed: no topic and no environment were made.
  expect(await requestsOf('POST', /\/environments$/)).toHaveLength(0);
  expect(await requestsOf('POST', /\/topics$/)).toHaveLength(0);

  // What was typed waits as the first input, and the agent's first words come into the thread that was opened.
  await emit(thread, { items: [agentMessage('a1', 'The redirect falls back to the home page now.', { turn: 't1', at: new Date().toISOString(), phase: 'final' })] });
  await expect(page.getByText('The redirect falls back to the home page now.')).toBeVisible();
});

test('a topic starter remembers its successful environment without changing the topic default', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await page.getByTestId('main').getByRole('button', { name: 'New topic', exact: true }).click();
  await page.getByRole('dialog', { name: 'New topic in Web' }).getByRole('button', { name: 'Create topic', exact: true }).click();
  await expect(page).toHaveURL(/\/topic\//);
  await expect(starter(page).getByText('Codex on OpenAI key, GPT-5.4 (High), in a new environment')).toBeVisible();

  await starterBox(page).fill('Move the session cookie into its own module');
  await starterBox(page).press('Enter');
  const first = await openedThread(page);
  await expect(page.getByText('Move the session cookie into its own module')).toBeVisible();

  // One start creates the requested objects; installing a topic default is a separate explicit act.
  const steps = (await requests())
    .filter((entry) => entry.method !== 'GET' && /\/(projects\/[^/]+\/topics|environments|topics\/[^/]+\/default-environment|topics\/[^/]+\/threads)$/.test(entry.path))
    .map((entry) => `${entry.method} ${entry.path.replace(/[0-9a-f-]{36}/g, '·')}`);
  expect(steps).toEqual(['POST /projects/·/topics', 'POST /topics/·/threads']);
  expect(await requestsOf('PUT', /\/default-environment$/)).toHaveLength(0);
  const start = (await requestsOf('POST', /\/topics\/[^/]+\/threads$/))[0];
  expect(start?.body).toMatchObject({ environment: { new: {} } });
  const topic = /\/topics\/([^/]+)\/threads$/.exec(start?.path ?? '')?.[1];
  // The native atomic-action proof checks the admitted topic is the environment's actual home.
  await expect(page.getByRole('contentinfo', { name: 'Status' }).getByRole('button', { name: 'Environment: Main', exact: true })).toBeVisible();

  // This person's last successful setup can reuse its environment; the topic default remains untouched.
  await page.goto(`/topic/${topic}`);
  await expect(starter(page).getByText('Codex on OpenAI key, GPT-5.4 (High), in Main')).toBeVisible();
  await starterBox(page).fill('And the same for the logout page');
  await starterBox(page).press('Enter');
  expect(await openedThread(page)).not.toBe(first);
  expect(await requestsOf('POST', /\/environments$/)).toHaveLength(0);
  expect(await requestsOf('PUT', /\/default-environment$/)).toHaveLength(0);
});

test('the project header makes a topic and its thread dialog keeps the first message', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}`);
  await page.getByTestId('main').getByRole('button', { name: 'New topic', exact: true }).click();
  const topicDialog = page.getByRole('dialog', { name: 'New topic in Web' });
  await topicDialog.getByRole('textbox', { name: /^Name\s*\(optional\)$/ }).fill('Cookie handling');
  await topicDialog.getByRole('button', { name: 'Create topic', exact: true }).click();
  await starterBox(page).fill('Look at the cookie handling');
  await starter(page).getByRole('button', { name: 'Thread setup' }).click();
  const dialog = page.getByRole('dialog', { name: 'New thread in Cookie handling' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Options', exact: true }).click();
  await expect(dialog.getByRole('textbox', { name: 'First message' })).toHaveValue('Look at the cookie handling');
  // Where the topic has no environment yet, a new one is what is ticked, as the place's own rule says.
  await expect(dialog.getByRole('combobox', { name: 'Where it runs' })).toContainText('New');
  await dialog.getByRole('button', { name: 'Create thread' }).click();
  await openedThread(page);
  const steps = (await requests())
    .filter((entry) => entry.method !== 'GET' && /\/(projects\/[^/]+\/topics|environments|topics\/[^/]+\/threads)$/.test(entry.path))
    .map((entry) => `${entry.method} ${entry.path.replace(/[0-9a-f-]{36}/g, '·')}`);
  expect(steps).toEqual(['POST /projects/·/topics', 'POST /topics/·/threads']);
  await expect(page.getByText('Look at the cookie handling')).toBeVisible();
});

test('a second topic started in a project is not refused for the name its environment would have', async ({ page }) => {
  await signIn(page);
  for (const text of ['One', 'Two']) {
    await page.goto(`/project/${ACME.web}`);
    await page.getByTestId('main').getByRole('button', { name: 'New topic', exact: true }).click();
    await page.getByRole('dialog', { name: 'New topic in Web' }).getByRole('button', { name: 'Create topic', exact: true }).click();
    await expect(starter(page).getByText('in a new environment')).toBeVisible();
    await starterBox(page).fill(text);
    await starterBox(page).press('Enter');
    await openedThread(page);
    await expect(page.getByText(text, { exact: true })).toBeVisible();
    await expect(page.getByRole('contentinfo', { name: 'Status' }).getByRole('button', { name: `Environment: ${text === 'One' ? 'Main' : 'New topic'}`, exact: true })).toBeVisible();
  }
  // The server chooses an unused name inside admission, with no rejected standalone environment requests.
  expect(await requestsOf('POST', /\/environments$/)).toHaveLength(0);
  const starts = await requestsOf('POST', /\/topics\/[^/]+\/threads$/);
  expect(starts).toHaveLength(2);
  for (const start of starts) expect(start.body).toMatchObject({ environment: { new: {} } });
});

test('where defaults are absent the starter offers a whole usable setup and Enter does not change place defaults', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  await expect(starter(page).locator('[data-runs-on]')).toContainText('OpenAI key');
  await expect(starter(page).locator('[data-missing-default]')).toHaveCount(0);
  await starterBox(page).fill('Look at why the idle stop waits so long');
  await starterBox(page).press('Enter');
  await openedThread(page);
  expect(await requestsOf('POST', /\/topics\/[^/]+\/threads$/)).toHaveLength(1);
  expect(await requestsOf('PUT', /\/settings\//)).toHaveLength(0);
});

test('where the server does not say yet what a thread here could be bound to, the starter and the dialog say so and make nothing', async ({ page }) => {
  await serveOptions(false);
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(starter(page).getByRole('alert')).toContainText('What a thread here would run on could not be loaded');
  await starterBox(page).fill('Anything');
  await starterBox(page).press('Enter');
  await page.getByTestId('main').getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  await expect(startDialog(page).getByRole('alert')).toContainText('This does not exist, was deleted, or is not open to you.');
  await expect(startDialog(page).getByRole('button', { name: 'Create thread' })).toBeDisabled();
  expect(await requestsOf('POST', /\/threads$/)).toHaveLength(0);
});

test('a successful custom setup is remembered after a reload without writing place defaults', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  await starterBox(page).fill('Check the idle stop');
  await starter(page).getByRole('button', { name: 'Thread setup', exact: true }).click();
  const form = startDialog(page, 'Idle-stop rework');
  await form.getByRole('combobox', { name: 'Provider', exact: true }).click();
  await page.getByRole('option', { name: /Team Anthropic/ }).click();
  await form.getByRole('combobox', { name: 'Agent', exact: true }).click();
  await page.getByRole('option', { name: 'Scripted agent', exact: true }).click();
  await form.getByRole('button', { name: 'Create thread', exact: true }).click();
  await openedThread(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  await page.reload();
  await expect(starter(page).locator('[data-runs-on]')).toContainText('Scripted agent');
  await expect(starter(page).locator('[data-runs-on]')).toContainText('Claude Sonnet 5');
  await starterBox(page).fill('Continue the same setup');
  await starterBox(page).press('Enter');
  await openedThread(page);
  const made = await requestsOf('POST', /\/topics\/[^/]+\/threads$/);
  expect(made).toHaveLength(2);
  for (const request of made) expect(request.body).toMatchObject({ binding: { agent_id: RUNS_ON.scripted, provider_id: RUNS_ON.teamAnthropic }, model: 'claude-sonnet-5' });
  expect(await requestsOf('PUT', /\/settings\//)).toHaveLength(0);
});

test('a person who cannot set place defaults can still start with one Enter and reuse their environment', async ({ page }) => {
  await deny('dora', ['topic.manage']);
  await signIn(page, PEOPLE.dora);
  await page.goto(`/topic/${ACME.idleStop}`);
  await expect(starter(page).locator('[data-runs-on]')).toContainText('OpenAI key');
  await starterBox(page).fill('Check the idle stop');
  await starterBox(page).press('Enter');
  await openedThread(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  await expect(starter(page).locator('[data-runs-on]')).toContainText('Main');
  await starterBox(page).fill('Keep working here');
  await starterBox(page).press('Enter');
  await openedThread(page);
  expect(await requestsOf('PUT', /\/settings\//)).toHaveLength(0);
  expect(await requestsOf('PUT', /\/default-environment$/)).toHaveLength(0);
  expect(await requestsOf('POST', /\/environments$/)).toHaveLength(0);
});

test('where no harness has reported what it can do, the starter says so, offers nothing that cannot run, and comes alive when the report arrives', async ({ page }) => {
  // A fresh system: no profile has a descriptor until a machine has started its harness once.
  for (const agent of ['Codex', 'Scripted agent', 'Codex on ada-laptop']) await agentReports(agent, false);
  await setDefaults(ACME.idleStop, { 'agent.default_provider_id': RUNS_ON.openAiKey, 'agent.default_agent_id': RUNS_ON.codex, 'agent.model': MODEL, 'agent.effort': 'medium' });
  await signIn(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  const panel = starter(page).getByRole('group', { name: 'What is missing' });
  await expect(panel).toContainText('The machine has not reported what Codex can do yet.');
  // Nothing that could not run is offered: no choice to make, and sending waits.
  await expect(panel.getByRole('combobox')).toHaveCount(0);
  await starterBox(page).fill('Look at the idle stop');
  await expect(starter(page).getByRole('button', { name: 'Start thread' })).toHaveAttribute('aria-disabled', 'true');

  // The dialog says the same, and offers no way of placing an agent that nothing is known of.
  await panel.getByRole('button', { name: 'Thread setup' }).click();
  const dialog = startDialog(page, 'Idle-stop rework');
  await expect(dialog.getByRole('button', { name: 'Create thread' })).toBeDisabled();
  await dialog.getByRole('combobox', { name: 'Agent', exact: true }).click();
  const unreported = page.getByRole('option', { name: 'Codex', exact: true });
  await expect(unreported).toHaveAttribute('aria-disabled', 'true');
  await expect(unreported).toHaveAccessibleDescription('its agent program has not reported yet');
  await page.keyboard.press('Escape');
  await expect(arrangements(page)).toHaveCount(0);
  await dialog.getByRole('button', { name: 'Cancel' }).click();

  // The machine reports once, and the starter comes alive by itself.
  await agentReports('Codex', true);
  await expect(starter(page).locator('[data-missing-default]')).toHaveCount(0);
  await expect(starter(page).getByText('Codex on OpenAI key, GPT-5.4 (Medium), in a new environment')).toBeVisible();
  await expect(starterBox(page)).toHaveValue('Look at the idle stop');
  await starterBox(page).press('Enter');
  await openedThread(page);
});

test('a profile that has not reported is waited for, and one that has is offered meanwhile', async ({ page }) => {
  await agentReports('Codex', false);
  await setDefaults(ACME.idleStop, { 'agent.default_provider_id': RUNS_ON.openAiKey, 'agent.default_agent_id': RUNS_ON.codex, 'agent.model': MODEL, 'agent.effort': 'medium' });
  await signIn(page);
  await page.goto(`/topic/${ACME.idleStop}`);
  const panel = starter(page).getByRole('group', { name: 'What is missing' });
  await expect(panel).toContainText('The machine has not reported what Codex can do yet.');
  // What can run is offered, and what cannot is not among it.
  await panel.getByRole('combobox', { name: 'Agent' }).click();
  await expect(page.getByRole('option', { name: 'Scripted agent', exact: true })).toBeVisible();
  await expect(page.getByRole('option', { name: 'Codex', exact: true })).toHaveCount(0);
  await page.getByRole('option', { name: 'Scripted agent', exact: true }).click();
  await expect(starter(page).getByText(/^Scripted agent on OpenAI key, GPT-5\.4/)).toBeVisible();
});

test('the dialog shows as chosen what the server would take, and sends it', async ({ page }) => {
  // This place prefers the arrangement the provider lists second: the server's default is not the first of the provider's list.
  await setDefaults(ACME.fixLogin, { 'binding.prefer': ['key_separated'] });
  await openDialog(page);

  const rows = arrangements(page).getByRole('radio');
  await expect(rows.nth(0)).toHaveAccessibleName(/Forward model calls/);
  await expect(rows.nth(0)).toBeChecked();
  await expect(arrangements(page).getByText('Default')).toHaveCount(1);
  await expect(rows.nth(1)).toHaveAccessibleName(/Run with provider/);

  await startDialog(page).getByRole('button', { name: 'Create thread' }).click();
  await openedThread(page);
  // Nothing was touched, and what is ticked was sent all the same; the thread says it was placed so.
  const made = await requestsOf('POST', new RegExp(`/topics/${ACME.fixLogin}/threads$`));
  expect(made[0]?.body).toMatchObject({ binding: { arrangement: 'key_separated' } });
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Forward model calls');
});

test('the first row that is ticked again is sent, and so is another row', async ({ page }) => {
  await openDialog(page);
  const rows = arrangements(page).getByRole('radio');
  await expect(rows.nth(0)).toHaveAccessibleName(/Run with provider/);
  await expect(rows.nth(0)).toBeChecked();

  // Another row, and back to the first: the choice of the person, whichever it is, is what the request carries.
  await rows.nth(1).click();
  await expect(rows.nth(1)).toBeChecked();
  await rows.nth(0).click();
  await expect(rows.nth(0)).toBeChecked();
  await startDialog(page).getByRole('button', { name: 'Create thread' }).click();
  await openedThread(page);
  expect((await requestsOf('POST', /\/threads$/))[0]?.body).toMatchObject({ binding: { arrangement: 'separated' } });
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Run with provider');

  await page.goBack();
  await page.getByTestId('main').getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  await startDialog(page).getByRole('button', { name: 'Options', exact: true }).click();
  await arrangements(page).getByRole('radio').nth(1).click();
  await startDialog(page).getByRole('button', { name: 'Create thread' }).click();
  await openedThread(page);
  expect((await requestsOf('POST', /\/threads$/))[1]?.body).toMatchObject({ binding: { arrangement: 'key_separated' } });
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Forward model calls');
});

test('a way that cannot be had is off with the server’s reason, and a refusal is never answered by another way', async ({ page }) => {
  await openDialog(page);
  const shared = arrangements(page).getByRole('radio', { name: /Copy sign-in access/ });
  await expect(shared).toBeDisabled();
  await expect(arrangements(page).getByText('Ada Lovelace does not allow this provider to be used this way.')).toBeVisible();

  // A model that the provider lists and the person may not use is there too, off, with why.
  await startDialog(page).getByRole('combobox', { name: 'Model' }).click();
  const closedModel = page.getByRole('option', { name: 'GPT-4.1', exact: true });
  await expect(closedModel).toHaveAttribute('data-disabled', '');
  await expect(closedModel).toContainText('it is switched off');
  await page.keyboard.press('Escape');

  // The environment loses what the harness works through after the answer was read: the server refuses the way that was ticked.
  await bareEnvironment('Workspace');
  await startDialog(page).getByRole('button', { name: 'Create thread' }).click();
  const refusal = startDialog(page).getByRole('alert').filter({ hasText: /the environment has no tools this agent can use/ });
  await expect(refusal).toBeVisible();
  // Nothing was made in its place: the same way was asked for, once, and the dialog still holds it.
  expect(await requestsOf('POST', /\/threads$/)).toHaveLength(1);
  expect((await requestsOf('POST', /\/threads$/))[0]?.body).toMatchObject({ binding: { arrangement: 'separated' } });
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}`));
  await expect(arrangements(page).getByRole('radio').first()).toBeChecked();
});

test('a new environment is made from the dialog, chosen, and the thread is started in it', async ({ page }) => {
  await openDialog(page);
  await startDialog(page).getByRole('button', { name: 'New environment…' }).click();
  const make = page.getByRole('dialog', { name: 'Add environment' });
  await expect(make.getByRole('combobox', { name: 'Belongs to', exact: true })).toHaveValue('Fix login');
  // A host that takes none from the template is there and cannot be chosen, with why.
  await make.getByRole('combobox', { name: 'Host' }).click();
  const closed = page.getByRole('option', { name: /build-host/ });
  await expect(closed).toHaveAttribute('data-disabled', '');
  await expect(closed).toContainText('takes no new environments');
  await page.keyboard.press('Escape');

  await make.getByRole('textbox', { name: 'Name' }).fill('Scratch');
  await make.getByRole('button', { name: 'Add environment' }).click();
  // Back in the dialog that started it, with the new environment chosen.
  await expect(make).toBeHidden();
  const made = startDialog(page).getByRole('combobox', { name: 'Where it runs' });
  await expect(made).toContainText('Scratch');
  expect(((await requestsOf('POST', /\/environments$/))[0]?.body as { home_node_id?: string }).home_node_id).toBe(ACME.fixLogin);

  await startDialog(page).getByRole('button', { name: 'Create thread' }).click();
  await openedThread(page);
  const request = (await requestsOf('POST', new RegExp(`/topics/${ACME.fixLogin}/threads$`)))[0]?.body as { environment?: { use?: string } };
  expect(request.environment?.use).not.toBe(RUNS_ON.workspace);
  await expect(page.getByRole('contentinfo', { name: 'Status' }).getByRole('button', { name: 'Environment: Scratch', exact: true })).toBeVisible();
});

test('a new environment that cannot be made says why and keeps the dialog as it was, and one that is cancelled changes nothing', async ({ page }) => {
  await openDialog(page);
  const chosen = startDialog(page).getByRole('combobox', { name: 'Where it runs' });
  await expect(chosen).toContainText('Workspace');

  await startDialog(page).getByRole('button', { name: 'New environment…' }).click();
  const make = page.getByRole('dialog', { name: 'Add environment' });
  await make.getByRole('textbox', { name: 'Name' }).fill('Workspace');
  await make.getByRole('button', { name: 'Add environment' }).click();
  // The name is taken: the form says so beside the name, and stays.
  await expect(make.getByText('you already have an environment with this name')).toBeVisible();
  expect(await requestsOf('POST', /\/environments$/)).toHaveLength(1);

  await make.getByRole('button', { name: 'Cancel' }).click();
  await expect(make).toBeHidden();
  await expect(chosen).toContainText('Workspace');
  await chosen.click();
  await expect(page.getByRole('option', { name: /Workspace/ })).toHaveCount(1);
  await page.keyboard.press('Escape');
});

test('where a person may not make an environment the dialog does not offer one, and says how to ask', async ({ page }) => {
  await mayNotMake('dora');
  await openDialog(page, PEOPLE.dora);
  await expect(startDialog(page).getByRole('button', { name: 'New environment…' })).toHaveCount(0);
  await expect(startDialog(page).getByRole('combobox', { name: 'Where it runs' })).toHaveCount(0);
  await expect(page.getByRole('option', { name: 'New', exact: true })).toHaveCount(0);
  await startDialog(page).getByText('You may not make an environment here').click();
  await expect(startDialog(page).getByRole('button', { name: /Send|Request/ })).toBeVisible();
});

test('the starter and the dialog are used by keyboard alone', async ({ page }) => {
  await signIn(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  const box = starterBox(page);
  await box.focus();
  await page.keyboard.type('A first line');
  // Shift+Enter starts a line; Enter alone would send.
  await page.keyboard.press('Shift+Enter');
  await page.keyboard.type('and a second');
  await expect(box).toHaveValue('A first line\nand a second');

  // The way on to the dialog is two Tabs away, and Escape gives the keyboard back to where it came from.
  await page.keyboard.press('Tab');
  await expect(starter(page).getByRole('button', { name: 'Start thread' })).toBeFocused();
  await page.keyboard.press('Tab');
  const change = starter(page).getByRole('button', { name: 'Thread setup' });
  await expect(change).toBeFocused();
  await page.keyboard.press('Enter');
  const dialog = startDialog(page);
  await expect(dialog).toBeVisible();
  // What was typed comes with it, and the keyboard begins at the top of the dialog.
  await expect(dialog.locator('[data-new-thread]')).toBeFocused();
  await dialog.getByRole('button', { name: 'Options', exact: true }).focus();
  await page.keyboard.press('Enter');
  await expect(dialog.getByRole('textbox', { name: 'First message' })).toHaveValue('A first line\nand a second');
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(change).toBeFocused();

  // And in the dialog: an arrow chooses the next way, and Enter on the last button makes the thread.
  await page.keyboard.press('Enter');
  await expect(dialog.locator('[data-new-thread]')).toBeFocused();
  await dialog.getByRole('button', { name: 'Options', exact: true }).focus();
  await page.keyboard.press('Enter');
  const separated = arrangements(page).getByRole('radio', { name: /Run with provider/ });
  await separated.focus();
  // A key is held for a moment, as a hand holds it: the choice follows the focus a tick after the key goes down.
  await page.keyboard.press('ArrowDown', { delay: 80 });
  await expect(arrangements(page).getByRole('radio', { name: /Forward model calls/ })).toBeChecked();
  const create = dialog.getByRole('button', { name: 'Create thread' });
  for (let presses = 0; presses < 60 && !(await create.evaluate((element) => element === document.activeElement)); presses += 1) await page.keyboard.press('Tab');
  await expect(create).toBeFocused();
  await page.keyboard.press('Enter');
  await openedThread(page);
  await expect(page.getByRole('contentinfo', { name: 'Status' })).toContainText('Forward model calls');
  await expect(page.getByText('A first line')).toBeVisible();
});
