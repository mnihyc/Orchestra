/** A thread made through the dialog of a topic: what it offers and from what it starts, the thread it makes and opens, and the refusal that says what is missing. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, deny, DOUBLE, PEOPLE, scenario, signIn, threadHeld, mainArea } from './support.ts';
import { id, KIND, RUNS_ON } from '../doubles/fixtures.ts';

test.beforeEach(async () => {
  await scenario();
});

const dialog = (page: Page) => page.getByRole('dialog', { name: 'New thread in Fix login' });
/** The name of the thread: not the name of the environment that can be registered in the same dialog. */
const nameField = /^Name\s*\(optional\)$/;
const tree = (page: Page) => page.getByRole('tree', { name: 'Orgs, projects, topics and threads' });

async function openDialog(page: Page, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await mainArea(page).getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await dialog(page).getByRole('button', { name: 'Options', exact: true }).click();
}

test('a thread is made from the defaults of its place, with a name and a first message, and then opened', async ({ page }) => {
  await openDialog(page);
  const form = dialog(page);

  // What may be run on comes first, and what may not is said below with why.
  const providers = form.getByRole('combobox', { name: 'Provider', exact: true });
  await expect(providers).toHaveValue('OpenAI key');
  await providers.click();
  await expect(page.getByRole('option', { name: /OpenAI key/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('option', { name: /Codex subscription/ })).toBeVisible();
  await expect(page.getByRole('option', { name: /Team Anthropic/ })).toBeVisible();
  await expect(page.getByRole('dialog', { name: 'Provider', exact: true }).locator('[role="option"][aria-disabled="true"]')).toHaveCount(1);
  await page.keyboard.press('Escape');
  // The environment that the topic names is the one chosen, and the agent and the model follow the provider.
  await expect(form.getByRole('combobox', { name: 'Where it runs' })).toHaveValue(/Workspace/u);
  await expect(form.getByRole('combobox', { name: 'Agent' })).toHaveValue(/Codex/u);
  await expect(form.getByRole('combobox', { name: 'Model' })).toContainText('GPT-5.4');

  await form.getByRole('textbox', { name: nameField }).fill('Investigate the flaky test');
  await form.getByRole('textbox', { name: 'First message' }).fill('Please look at the flaky test');
  await form.getByRole('button', { name: 'Create thread' }).click();

  // The thread opens; what was said first waits as its first input; the socket is connected for it.
  await expect(page).toHaveURL(/\/thread\//);
  await expect(page.getByRole('heading', { name: 'Investigate the flaky test' })).toBeVisible();
  await expect(page.getByText('Please look at the flaky test')).toBeVisible();
  await expect(page.getByText('Idle', { exact: true })).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await expect(tree(page).getByRole('treeitem', { name: /Investigate the flaky test/ })).toBeVisible();

  const id = /\/thread\/([^/?]+)/.exec(page.url())?.[1] ?? '';
  const held = await threadHeld(id);
  expect(held.inputs.map((input) => [input.state, input.content[0]?.text])).toEqual([['pending', 'Please look at the flaky test']]);
});

test('a thread made with nothing chosen but a click takes the topic’s environment and the first provider that is ready', async ({ page }) => {
  await openDialog(page);
  await dialog(page).getByRole('button', { name: 'Create thread' }).click();
  await expect(page).toHaveURL(/\/thread\//);
  await expect(page.getByRole('heading', { name: 'New thread' })).toBeVisible();
  await expect(page.getByText('Send the first message')).toBeVisible();
});

test('a refusal says what is missing and whom to ask, and the dialog stays with what was written in it', async ({ page }) => {
  // Give the ordinary caller a real owned environment before testing the lost topic right.
  const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(PEOPLE.dora) });
  expect(login.ok).toBe(true);
  const { access_token: token } = await login.json() as { access_token: string };
  const made = await fetch(`${DOUBLE}/api/v1/environments`, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` }, body: JSON.stringify({ name: 'Owned workspace', home_node_id: ACME.fixLogin, template_id: RUNS_ON.defaultTemplate, host_id: id(KIND.host, 1) }) });
  expect(made.ok).toBe(true);
  await openDialog(page, PEOPLE.dora);
  const form = dialog(page);
  await form.getByRole('combobox', { name: 'Where it runs' }).click();
  await page.getByRole('option', { name: /^Owned workspace/ }).click();
  await expect(form.getByRole('combobox', { name: 'Where it runs' })).toHaveValue(/Owned workspace/);
  await form.getByRole('textbox', { name: nameField }).fill('Not allowed');
  const create = form.getByRole('button', { name: 'Create thread' });
  await expect(create).toBeEnabled();
  // Somebody takes the right away after the page was loaded: the page still believes it, the server does not.
  await deny('dora', ['topic.start_threads']);
  const refused = page.waitForResponse((response) => response.request().method() === 'POST'
    && new URL(response.url()).pathname === `/api/v1/topics/${ACME.fixLogin}/threads`);
  await create.click();
  expect((await refused).status()).toBe(403);

  const refusal = form.getByRole('alert');
  await expect(refusal).toContainText('You do not have access to this.');
  await refusal.getByRole('button', { name: 'Review access', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review access', exact: true });
  await expect(review.getByText('You can’t start threads in Fix login.', { exact: true })).toBeVisible();
  await expect(review).toContainText('Fix login');
  await expect(review).toContainText('Ask Ada Lovelace.');
  await review.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(form.getByRole('textbox', { name: nameField })).toHaveValue('Not allowed');
  await expect.poll(() => new URL(page.url()).pathname).toBe(`/topic/${ACME.fixLogin}`);
  expect(new URL(page.url()).searchParams.get('tab')).toBe('threads');
});

test('a provider that is not ready cannot be chosen, and the dialog says why', async ({ page }) => {
  await openDialog(page);
  const form = dialog(page);
  const providers = form.getByRole('combobox', { name: 'Provider', exact: true });
  await providers.click();
  await expect(page.getByRole('option', { name: /Fresh key/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('option', { name: /Fresh key/ })).toContainText('it needs a credential or a model before it can be used');
  // A vendor-limited provider remains a binding choice and reports its reset time.
  await expect(page.getByRole('option', { name: /Claude subscription/ })).not.toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('option', { name: /Claude subscription/ })).toContainText('At limit until');
  await page.keyboard.press('Escape');
  await expect(providers).toHaveValue('OpenAI key');
});
