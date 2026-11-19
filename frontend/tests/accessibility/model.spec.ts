/** The screens of what a model can be run with, asked in both themes whether anything on them is a serious barrier: a provider's Models tab with its groups, the row editor of a model with each of its parts open, the dialog that adds a model, and a thread's model control with Speed, Access and Context, read-only for somebody who only drives the thread, and the list of slash commands. */
import { expect, type Page, test } from '@playwright/test';

import { id } from '../doubles/fixtures.ts';
import { SOL_A, SOL_B } from '../doubles/model.ts';
import { ACME, deny, emit, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

const MAIN = ACME.mainThread;
const CODEX = id(0x22, 2);
const OPENAI = id(0x22, 1);

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** A model's entry written anew at the provider the threads run on, as its manager does. */
async function writeEntry(model: string, display: string, parameters: unknown): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/model/entry`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ model_name: model, display_name: display, parameters }) });
  if (!response.ok) throw new Error(`the fake API refused the entry of ${model}: ${response.status}`);
}

test.beforeEach(async () => {
  await scenario();
  await writeEntry('gpt-sol', 'GPT Sol (A)', SOL_A);
  await writeEntry('gpt-sol-b', 'GPT Sol (B)', SOL_B);
  await emit(MAIN, { thread: { state: 'idle', model: 'gpt-sol', model_display: 'GPT Sol (A)', effort: 'medium' } });
});

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

const editor = (page: Page) => page.getByRole('dialog', { name: 'What this model can be run with here' });
const card = (page: Page) => page.getByRole('dialog', { name: 'Model and effort' });

test("a provider's Models tab: the groups above the list, each model with what it can be run with and what is only offered, and the suggestions", async ({ page }) => {
  await open(page, `/resources/providers/${CODEX}?tab=models`);
  await expect(page.getByRole('table', { name: 'Models' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Groups' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The provider that has two groups, on the same sign-in.
  await page.goto(`/resources/providers/${OPENAI}?tab=models`);
  await expect(page.getByRole('region', { name: 'Groups' }).getByText('small', { exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the row editor of a model: switches off, then on with what a thread starts with, a refused largest context, groups and limits', async ({ page }) => {
  await open(page, `/resources/providers/${CODEX}?tab=models`);
  await page.getByRole('button', { name: 'Actions for GPT-5.5 Codex', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Edit options…' }).click();
  await expect(editor(page)).toBeVisible();
  await expect(editor(page).getByRole('group', { name: 'Limits on this model' }).getByText('Near its limit')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role=dialog]');

  // Everything on: a speed and a program with what a thread starts with, the largest context with its start, a step of the rate.
  const dialog = editor(page);
  await dialog.getByRole('group', { name: 'Speeds' }).getByRole('switch', { name: 'Fast' }).click();
  await dialog.getByRole('group', { name: 'Access programs' }).getByRole('switch', { name: 'Daybreak Blue' }).click();
  await dialog.getByRole('group', { name: 'Context' }).getByRole('button', { name: 'Use 1M' }).click();
  await dialog.getByRole('group', { name: 'Context' }).getByRole('radio', { name: 'Custom' }).click();
  await dialog.getByRole('group', { name: 'Context' }).getByLabel('Threads start with, in tokens').fill('2000000');
  await expect(dialog.getByText('A thread cannot start with more than the largest (1,000,000).')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role=dialog]');
});

test('the dialog that adds a model', async ({ page }) => {
  await open(page, `/resources/providers/${CODEX}?tab=models`);
  await page.getByRole('button', { name: 'Add model' }).first().click();
  await expect(page.getByRole('dialog', { name: 'Add a model to Codex subscription' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role=dialog]');
});

test("a thread's model control: the models, and Speed, Access and Context with where each comes from", async ({ page }) => {
  await open(page, `/thread/${MAIN}`);
  await page.getByRole('button', { name: /^Model and effort: GPT Sol \(A\)/ }).last().click();
  await expect(card(page).getByRole('region', { name: 'Context' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role=dialog]');

  // Chosen, so that every line says who it was chosen by, and a choice that crosses a step of the rate.
  await card(page).getByRole('region', { name: 'Speed' }).getByRole('radio', { name: 'Fast' }).click();
  await expect(card(page).getByText('Fast · chosen for this thread')).toBeVisible();
  await card(page).getByLabel('Another size, in tokens').fill('1000');
  await card(page).getByLabel('Another size, in tokens').press('Enter');
  await expect(card(page).getByText('At least 32K.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role=dialog]');
});

test('the model control read by somebody who may only drive the thread, and the list of commands in the box', async ({ page }) => {
  await deny('dora', ['thread.manage']);
  await open(page, `/thread/${MAIN}`, PEOPLE.dora);
  await page.getByRole('button', { name: /^Model and effort: GPT Sol \(A\)/ }).last().click();
  await expect(card(page).getByText('Changing the model is for whoever manages this thread: Ada Lovelace')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role=dialog]');
  await page.keyboard.press('Escape');

  await page.getByRole('textbox', { name: 'Message to the agent' }).fill('/');
  await expect(page.getByRole('listbox', { name: 'Commands' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
