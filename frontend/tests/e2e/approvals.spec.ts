/** What waits for the person, of the three kinds, decided from the list: an agent's request with the options the harness gave, questions and a form an MCP server asks for, a capability with a note, access requests decided row by row with fewer permissions and an end, and what somebody else decides while the list is open or just before a decision was sent. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { approvalsMade, askAccess, askCapability, notify, settleAs } from './inbox-support.ts';
import { ACME, PEOPLE, playScene, scenario, signIn } from './support.ts';

const REPRO = ACME.repro;
const MAIN = ACME.mainThread;

test.beforeEach(async () => {
  await scenario();
});

const rail = (page: Page) => page.getByRole('navigation', { name: 'Orgs and your own places' });
const card = (page: Page, title: string | RegExp) => page.getByRole('region', { name: title });
// Two requests may ask for the same capability; its short native name is shared, its request ID is not.
const capabilityCard = (page: Page, id: string) => page.locator(`[data-approval="${id}"]`);
const waiting = (count: number) => (count === 0 ? 'Approvals' : `Approvals, ${count} ${count === 1 ? 'approval waits' : 'approvals wait'}`);

async function open(page: Page, path = '/approvals'): Promise<void> {
  await signIn(page);
  await page.goto(path);
}

/** The row of an access card that is about one request, which keeps its place when it is decided. */
const row = (scope: Locator | Page, id: string) => scope.locator(`[data-access-row="${id}"]`);

test('everything that waits is listed, of the three kinds, each with its kind, who asked and where it comes from', async ({ page }) => {
  await playScene(REPRO, 'waiting');
  await askCapability({ requester: 'grace', title: 'GPU for Training box', risk: 'high' });
  await askAccess({ requester: 'dora', title: 'Access to Fix login', object: { kind: 'thread', id: MAIN, name: 'Main thread' }, permissions: ['thread.view'] });
  await open(page);

  await expect(rail(page).getByRole('link', { name: waiting(3) })).toBeVisible();
  await expect(page.getByRole('tab', { name: /Waiting for me/ })).toContainText('3');

  const agent = card(page, 'Run npm install left-pad');
  await expect(agent).toContainText('Agent');
  await expect(agent.getByText('npm install left-pad', { exact: true })).toBeVisible();
  await expect(agent.getByRole('button', { name: 'Allow once' })).toBeVisible();
  await expect(agent.getByRole('button', { name: /^Deny /, exact: true })).toBeVisible();

  const capability = card(page, 'GPU');
  await expect(capability).toContainText('Capability');
  await expect(capability).toContainText('High risk');
  await expect(capability).toContainText('Grace Hopper');
  await expect(capability).toContainText('Training box');
  await expect(capability).toContainText("Grace's laptop");
  await expect(capability).toContainText('The environment is recreated with the GPU.');
  await expect(capability).toContainText('Decided by whoever manages the host');

  const access = card(page, 'Access to Fix login');
  await expect(access).toContainText('Access');
  await expect(access).toContainText('Dora Developer');
  await expect(page.getByRole('heading', { level: 1, name: 'Approvals', exact: true })).toBeVisible();
  for (const [entry, title] of [[agent, 'Run npm install left-pad'], [capability, 'GPU'], [access, 'Access to Fix login']] as const) {
    await expect(entry.getByRole('heading', { level: 2, name: title, exact: true })).toBeVisible();
  }
});

test('an agent’s request is decided from the list with the choices the harness gave, and the conversation says the same', async ({ page }) => {
  await playScene(REPRO, 'waiting');
  await open(page);
  await expect(rail(page).getByRole('link', { name: waiting(1) })).toBeVisible();

  const agent = card(page, 'Run npm install left-pad');
  await agent.getByRole('button', { name: 'Allow once' }).click();
  // The card collapses to one line, under the person's name; the count follows.
  const line = card(page, 'Run npm install left-pad');
  await expect(line.getByRole('button', { name: /Allowed once by Ada Lovelace/ })).toBeVisible();
  await expect(line.getByRole('button', { name: 'Allow once' })).toHaveCount(0);
  await expect(rail(page).getByRole('link', { name: waiting(0), exact: true })).toBeVisible();

  // It opens to the request again.
  await line.getByRole('button', { name: /Allowed once by Ada Lovelace/ }).click();
  await expect(line.getByText('npm install left-pad', { exact: true })).toBeVisible();

  // The same approval, in its conversation.
  await page.goto(`/thread/${REPRO}`);
  await expect(page.getByRole('log', { name: 'Conversation' }).getByRole('button', { name: /Run npm install left-pad Allow once by Ada Lovelace/ })).toBeVisible();
});

test('a request that is refused is decided as refused, with the danger option the harness gave', async ({ page }) => {
  await playScene(REPRO, 'waiting');
  await open(page);
  await card(page, 'Run npm install left-pad').getByRole('button', { name: /^Deny /, exact: true }).click();
  await expect(card(page, 'Run npm install left-pad').getByRole('button', { name: /Denied by Ada Lovelace/ })).toBeVisible();
});

test('questions are answered, and what an MCP server asks for is filled in, from the list', async ({ page }) => {
  await playScene(MAIN, 'everything');
  await page.setViewportSize({ width: 1280, height: 5200 });
  await open(page);

  const questions = card(page, 'The agent asks you');
  await expect(questions.getByRole('button', { name: 'Send answers' })).toBeDisabled();
  await questions.getByRole('radio', { name: /Vitest/ }).click();
  await questions.getByRole('checkbox', { name: /Types/ }).click();
  await questions.getByLabel('Is there anything it should know?').fill('Keep the old tests.');
  await questions.getByRole('button', { name: 'Send answers' }).click();
  await expect(card(page, 'The agent asks you').getByRole('button', { name: /Answered by Ada Lovelace/ })).toBeVisible();

  const form = card(page, 'The tracker asks for details');
  await expect(form.getByRole('button', { name: 'Accept' })).toBeDisabled();
  await form.getByLabel('Title').fill('Login redirect loses the path');
  await form.getByRole('button', { name: 'Accept' }).click();
  await expect(card(page, 'The tracker asks for details').getByRole('button', { name: /by Ada Lovelace/ })).toBeVisible();

  // What the others ask is still there, to be decided.
  await expect(card(page, 'Run rm -rf build && npm run build').getByRole('button', { name: 'Allow once' })).toBeVisible();
  await expect(card(page, 'Change src/login.ts').getByRole('button', { name: 'Allow once' })).toBeVisible();
  await expect(card(page, 'More permissions').getByRole('button', { name: 'Allow once' })).toBeVisible();
});

test('a capability is approved with a note for whoever asked, and another is denied', async ({ page }) => {
  const first = await askCapability({ requester: 'grace', title: 'GPU for Training box' });
  const second = await askCapability({ requester: 'dora', title: 'Debugger for Build box' });
  await open(page);
  await expect(rail(page).getByRole('link', { name: waiting(2) })).toBeVisible();

  const gpu = capabilityCard(page, first.id);
  await expect(gpu.getByRole('heading', { name: 'GPU', exact: true })).toBeVisible();
  await gpu.getByLabel('A note for whoever asked (optional)').fill('Only until Friday.');
  await gpu.getByRole('button', { name: /^Approve /, exact: true }).click();
  await expect(gpu.getByRole('button', { name: /Approved by Ada Lovelace/ })).toBeVisible();

  const other = capabilityCard(page, second.id);
  await expect(other.getByRole('heading', { name: 'GPU', exact: true })).toBeVisible();
  await other.getByRole('button', { name: /^Deny /, exact: true }).click();
  await expect(other.getByRole('button', { name: /Denied by Ada Lovelace/ })).toBeVisible();
  await expect(rail(page).getByRole('link', { name: waiting(0), exact: true })).toBeVisible();

  const made = await approvalsMade();
  const decided = (id: string) => made.find((approval) => approval.id === id);
  expect(decided(first.id)?.state).toBe('approved');
  expect(decided(first.id)?.decision).toEqual({ approve: true, note: 'Only until Friday.' });
  expect(decided(second.id)?.state).toBe('denied');
  expect(decided(second.id)?.decision).toEqual({ approve: false });
});

test('requests sent together are one card with a row for each thing, decided one by one: fewer permissions, narrower options, an end, and what a terminal reaches said in words', async ({ page }) => {
  const batch = '00000000-0000-4000-8014-000000000001';
  const thread = await askAccess({
    id: '00000000-0000-4000-8012-0000000000a1',
    requester: 'dora',
    batch_id: batch,
    title: 'Access to Fix login',
    object: { kind: 'thread', id: MAIN, name: 'Main thread' },
    permissions: ['thread.view', 'thread.interact', 'terminal.manage'],
    reason: 'I would like to work with the agent on the login redirect.',
    for_thread: { id: MAIN, kind: 'thread', name: 'Main thread', redacted: false },
  });
  const provider = await askAccess({
    id: '00000000-0000-4000-8012-0000000000a2',
    requester: 'dora',
    batch_id: batch,
    title: 'Access to Claude Max',
    object: { kind: 'provider', id: '00000000-0000-4000-8008-000000000001', name: 'Claude Max' },
    permissions: ['provider.use'],
    reason: 'Needed to run that thread.',
    options: { models: ['claude-opus', 'claude-sonnet'], efforts: ['medium', 'high'] },
  });
  await page.setViewportSize({ width: 1280, height: 2400 });
  await open(page);

  // One card, headed by what was asked together, with a row for each.
  const together = card(page, 'Dora Developer asks for access to 2 things');
  await expect(together).toBeVisible();
  await expect(together.locator('[data-access-row]')).toHaveCount(2);
  await expect(rail(page).getByRole('link', { name: waiting(2) })).toBeVisible();

  // A terminal is named in words before it is given, and the words go when the tick does.
  const first = row(together, thread.id);
  await expect(first.getByText("Open a shell in the environment where the thread runs, as that environment's user.")).toBeVisible();
  for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(first.getByRole('checkbox', { name: 'Manage Terminal', exact: true })).toBeVisible();
    const providerRow = row(together, provider.id);
    await expect(providerRow.getByRole('checkbox', { name: 'Use Provider', exact: true })).toBeVisible();
    await test.info().attach(`terminal-permission-${width}-${theme}`, { body: await first.screenshot(), contentType: 'image/png' });
    await test.info().attach(`provider-permission-${width}-${theme}`, { body: await providerRow.screenshot(), contentType: 'image/png' });
  }
  await first.getByRole('checkbox', { name: 'Manage Terminal', exact: true }).click();
  await expect(first.getByText(/Open a shell in the environment/)).toHaveCount(0);
  await first.getByRole('combobox', { name: 'Ends' }).click();
  await page.getByRole('option', { name: '1 week' }).click();
  await first.getByLabel('A note for whoever asked (optional)').fill('For the redirect only.');
  await first.getByRole('button', { name: /^Approve /, exact: true }).click();

  // The first row is decided; the second still waits, and so the card still does.
  await expect(row(together, thread.id)).toContainText('Approved');
  await expect(row(together, provider.id).getByRole('button', { name: /^Approve /, exact: true })).toBeVisible();
  await expect(rail(page).getByRole('link', { name: waiting(1) })).toBeVisible();

  const second = row(together, provider.id);
  await second.getByRole('checkbox', { name: 'claude-opus' }).click();
  await second.getByRole('checkbox', { name: 'high', exact: true }).click();
  await second.getByRole('button', { name: /^Approve /, exact: true }).click();
  await expect(rail(page).getByRole('link', { name: waiting(0), exact: true })).toBeVisible();

  const made = await approvalsMade();
  const first_ = made.find((approval) => approval.id === thread.id);
  expect(first_?.state).toBe('approved');
  const given = first_?.decision as { approve: boolean; permissions: string[]; expires_at: string; note: string };
  expect(given.permissions).toEqual(['thread.view', 'thread.interact']);
  expect(given.note).toBe('For the redirect only.');
  // An end a week away, to within the time the test took.
  const away = Date.parse(given.expires_at) - Date.now();
  expect(away).toBeGreaterThan(6.9 * 24 * 3_600_000);
  expect(away).toBeLessThan(7.1 * 24 * 3_600_000);
  const narrowed = made.find((approval) => approval.id === provider.id)?.decision as { options: { models: string[]; efforts: string[] } };
  expect(narrowed.options).toEqual({ models: ['claude-sonnet'], efforts: ['medium'] });
});

test('an access request is denied, with a note, and nothing is given', async ({ page }) => {
  const asked = await askAccess({ requester: 'dora', title: 'Access to Fix login', object: { kind: 'thread', id: MAIN, name: 'Main thread' }, permissions: ['thread.view'] });
  await open(page);
  const one = card(page, 'Access to Fix login');
  await one.getByLabel('A note for whoever asked (optional)').fill('Ask Grace first.');
  await one.getByRole('button', { name: 'Deny' }).click();
  await expect(rail(page).getByRole('link', { name: waiting(0), exact: true })).toBeVisible();
  expect((await approvalsMade()).find((approval) => approval.id === asked.id)?.decision).toEqual({ approve: false, note: 'Ask Grace first.' });
});

test('a request that somebody else decides while the list is open turns into its decided line and says who answered', async ({ page }) => {
  const asked = await askCapability({ requester: 'dora', title: 'GPU for Training box' });
  await open(page);
  await expect(capabilityCard(page, asked.id).getByRole('button', { name: /^Approve /, exact: true })).toBeVisible();
  await expect(rail(page).getByRole('link', { name: waiting(1) })).toBeVisible();

  await settleAs(asked.id, 'grace', 'approved');
  const line = capabilityCard(page, asked.id);
  await expect(line.getByRole('button', { name: /Approved by Grace Hopper/ })).toBeVisible();
  await expect(line.getByText('Already answered by Grace Hopper')).toBeVisible();
  await expect(line.getByRole('button', { name: /^Approve /, exact: true })).toHaveCount(0);
  await expect(rail(page).getByRole('link', { name: waiting(0), exact: true })).toBeVisible();
});

test('an agent’s request decided by somebody else meanwhile is shown as such too', async ({ page }) => {
  await playScene(REPRO, 'waiting');
  await open(page);
  await expect(card(page, 'Run npm install left-pad').getByRole('button', { name: 'Allow once' })).toBeVisible();
  await settleAs('ap-1', 'grace', 'denied');
  const line = card(page, 'Run npm install left-pad');
  await expect(line.getByText('Already answered by Grace Hopper')).toBeVisible();
  await expect(line.getByRole('button', { name: /Denied by Grace Hopper/ })).toBeVisible();
});

test('a decision that comes too late says who got there first, and takes the card as it now stands', async ({ page }) => {
  const asked = await askCapability({ requester: 'dora', title: 'GPU for Training box' });
  await open(page);
  await expect(capabilityCard(page, asked.id).getByRole('button', { name: /^Approve /, exact: true })).toBeVisible();
  // The page's own feed is live before anybody answers behind its back (a row that comes through it is counted on the rail); once it is, it says no more of its own.
  await notify({ kind: 'mention', title: 'Dora wrote to you' });
  await expect(rail(page).getByRole('link', { name: 'Inbox, 1 unread' })).toBeVisible();
  // Nobody tells the page: it finds out by its own try.
  await settleAs(asked.id, 'grace', 'denied', true);
  await capabilityCard(page, asked.id).getByRole('button', { name: /^Approve /, exact: true }).click();
  const line = capabilityCard(page, asked.id);
  await expect(line.getByText('Already answered by Grace Hopper')).toBeVisible();
  await expect(line.getByRole('button', { name: /Denied by Grace Hopper/ })).toBeVisible();
});

test('what the person asked for is listed with its answer, and an access request is taken back', async ({ page }) => {
  await askAccess({ requester: 'dora', title: 'Access to Fix login', object: { kind: 'thread', id: MAIN, name: 'Main thread' }, permissions: ['thread.view', 'thread.interact'], reason: 'To follow the redirect work.' });
  const answered = await askAccess({ requester: 'dora', title: 'Access to Repro', object: { kind: 'thread', id: REPRO, name: 'Repro' }, permissions: ['thread.view'] });
  await settleAs(answered.id, 'ada', 'approved');
  await signIn(page, PEOPLE.dora);
  await page.goto('/approvals?tab=requested');

  const asked = card(page, 'Access to Fix login');
  await expect(asked.getByText('View Thread, Interact Thread')).toBeVisible();
  await expect(asked.getByRole('button', { name: 'Approve' })).toHaveCount(0);
  await expect(card(page, 'Access to Repro').getByRole('button', { name: /Approved by Ada Lovelace/ })).toBeVisible();

  await asked.getByRole('button', { name: 'Take the request back' }).click();
  await expect(card(page, 'Access to Fix login').getByRole('button', { name: /Cancelled/ })).toBeVisible();
  expect((await approvalsMade()).find((approval) => approval.title === 'Access to Fix login')?.state).toBe('cancelled');
});

test('History lists what was decided, and each line opens to the request', async ({ page }) => {
  const asked = await askCapability({ requester: 'dora', title: 'GPU for Training box', reason: 'Training needs it.' });
  await settleAs(asked.id, 'grace', 'approved');
  await open(page, '/approvals?tab=history');
  const line = capabilityCard(page, asked.id);
  await expect(line.getByRole('button', { name: /Approved by Grace Hopper/ })).toBeVisible();
  await line.getByRole('button', { name: /Approved by Grace Hopper/ }).click();
  await expect(line.getByText('Training needs it.')).toBeVisible();
});

test('the kind, and everything the person could decide, are kept in the address, and the list follows them', async ({ page }) => {
  await playScene(REPRO, 'waiting');
  const mine = await askCapability({ requester: 'dora', title: 'GPU for Training box' });
  const nearer = await askCapability({ requester: 'dora', title: 'Debugger meant for Grace', deciders: ['grace'], also: ['ada'] });
  await open(page);
  await expect(capabilityCard(page, mine.id)).toBeVisible();
  await expect(capabilityCard(page, nearer.id)).toHaveCount(0);

  await page.getByRole('switch', { name: 'Everything I could decide' }).click();
  await expect(page).toHaveURL(/all=true/);
  await expect(capabilityCard(page, nearer.id)).toBeVisible();
  // It was sent to somebody nearer: the person sees it and is told they cannot answer it.
  await expect(capabilityCard(page, nearer.id).getByText("You can't answer this.")).toBeVisible();

  await page.getByRole('combobox', { name: 'Kind' }).click();
  await page.getByRole('option', { name: 'Agent action', exact: true }).click();
  await expect(page).toHaveURL(/kind=harness/);
  await expect(card(page, 'Run npm install left-pad')).toBeVisible();
  await expect(capabilityCard(page, mine.id)).toHaveCount(0);

  // A link reproduces it.
  await page.reload();
  await expect(page.getByRole('switch', { name: 'Everything I could decide' })).toBeChecked();
  await expect(card(page, 'Run npm install left-pad')).toBeVisible();
});

test('an empty list says what it is, in each tab', async ({ page }) => {
  await open(page);
  await expect(page.getByText('Nothing waits for you')).toBeVisible();
  await page.getByRole('tab', { name: 'Requested by me' }).click();
  await expect(page.getByText('You have asked for nothing').first()).toBeVisible();
  await page.getByRole('tab', { name: 'History' }).click();
  await expect(page.getByText('Nothing was decided yet')).toBeVisible();
});
