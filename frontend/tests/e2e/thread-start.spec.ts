/** A thread made through the dialog of a topic: what it offers and from what it starts, the thread it makes and opens, and the refusal that says what is missing. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, deny, PEOPLE, scenario, signIn, threadHeld } from './support.ts';

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
  await page.getByTestId('main').getByRole('tab', { name: /^Threads/ }).click();
  await page.getByRole('tabpanel', { name: /^Threads/ }).getByRole('button', { name: 'New thread', exact: true }).click();
  await expect(dialog(page)).toBeVisible();
  await dialog(page).getByRole('button', { name: 'Options', exact: true }).click();
}

test('a thread is made from the defaults of its place, with a name and a first message, and then opened', async ({ page }) => {
  await openDialog(page);
  const form = dialog(page);

  // What may be run on comes first, and what may not is said below with why.
  const providers = form.getByRole('combobox', { name: 'Provider', exact: true });
  await expect(providers).toContainText('OpenAI key');
  await providers.click();
  await expect(page.getByRole('option', { name: /OpenAI key/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('option', { name: /Codex subscription/ })).toBeVisible();
  await expect(page.getByRole('option', { name: /Team Anthropic/ })).toBeVisible();
  await expect(page.locator('[role="option"][data-disabled]')).toHaveCount(2);
  await page.keyboard.press('Escape');
  // The environment that the topic names is the one chosen, and the agent and the model follow the provider.
  await expect(form.getByRole('combobox', { name: 'Where it runs' })).toContainText('Workspace');
  await expect(form.getByRole('combobox', { name: 'Agent' })).toContainText('Codex');
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
  await openDialog(page, PEOPLE.dora);
  // Somebody takes the right away after the page was loaded: the page still believes it, the server does not.
  await deny('dora', ['topic.start_threads']);
  const form = dialog(page);
  await form.getByRole('textbox', { name: nameField }).fill('Not allowed');
  await form.getByRole('button', { name: 'Create thread' }).click();

  const refusal = form.getByRole('alert');
  await expect(refusal).toContainText('You do not have access to this.');
  await refusal.getByRole('button', { name: 'Review access', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review access', exact: true });
  await expect(review.getByText('You can’t start threads in Fix login.', { exact: true })).toBeVisible();
  await expect(review).toContainText('Fix login');
  await expect(review).toContainText('Ask Ada Lovelace.');
  await review.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(form.getByRole('textbox', { name: nameField })).toHaveValue('Not allowed');
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}\\?tab=threads$`));
});

test('a provider that is not ready cannot be chosen, and the dialog says why', async ({ page }) => {
  await openDialog(page);
  const form = dialog(page);
  const providers = form.getByRole('combobox', { name: 'Provider', exact: true });
  await providers.click();
  await expect(page.getByRole('option', { name: /Fresh key/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('option', { name: /Claude subscription/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('option', { name: /Fresh key/ })).toContainText('it needs a credential or a model before it can be used');
  await expect(page.getByRole('option', { name: /Claude subscription/ })).toContainText('it has reached its limit');
  await page.keyboard.press('Escape');
  await expect(providers).toContainText('OpenAI key');
});
