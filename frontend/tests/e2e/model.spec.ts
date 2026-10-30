/** What a model can be run with and what a thread runs with: a provider's Models tab with the row editor of a model (its switches off until a manager turns each on, what a thread starts with, a largest context that is refused below the window, groups, the limits that cover it) and a thread's model control (Speed, Access and Context only where the model has them open, where each comes from, a change of model that drops what the new one lacks, a reader who may not change it), with the slash commands and the footer of a turn. */
import { expect, type Page, test } from '@playwright/test';
import { showWorkbenchView } from './workbench-support';

import { id } from '../doubles/fixtures.ts';
import { SOL_A, SOL_B } from '../doubles/model.ts';
import { ADA, GRACE } from '../doubles/thread-scenes.ts';
import { marker, turn } from '../doubles/thread-rows.ts';
import { bodySent, ACME, configureThread, deny, emit, PEOPLE, playScene, scenario, signIn } from './support.ts';
import { makeCap, writeHistory } from './usage-support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const MAIN = ACME.mainThread;
/** The Codex subscription of the fake API: one model that offers a speed, two access programs and a larger context, none of it switched on. */
const CODEX = id(0x22, 2);
/** The OpenAI key: two models, each in a group of its own. */
const OPENAI = id(0x22, 1);

/** What only the fake API has: a model's entry written anew, a place's switch for access programs, a thread's own choices. */
async function control(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** A request made as ada would make it, to read what the fake API holds. */
async function asAda(method: string, path: string): Promise<unknown> {
  const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ login: PEOPLE.ada.login, password: PEOPLE.ada.password }) });
  const { access_token: token } = (await login.json()) as { access_token: string };
  const response = await fetch(`${DOUBLE}/api/v1${path}`, { method, headers: { Authorization: `Bearer ${token}` } });
  return response.json();
}

test.beforeEach(async () => {
  await scenario();
  // Two models whose managers opened more than the usual at the provider the threads run on, and a thread with nothing in it, nothing running,
  // on the one that has all three open.
  await control('/model/entry', { model_name: 'gpt-sol', display_name: 'GPT Sol (A)', parameters: SOL_A });
  await control('/model/entry', { model_name: 'gpt-sol-b', display_name: 'GPT Sol (B)', parameters: SOL_B });
  await control('/model/entry', { model_name: 'gpt-mini', display_name: 'GPT Mini', parameters: { context_window: 200_000 } });
  await emit(MAIN, { thread: { state: 'idle', model: 'gpt-sol', model_display: 'GPT Sol (A)', effort: 'medium' } });
});

test('Parameters uses the same speed, access and context choices and reports what a change of model drops', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${MAIN}`);
  await showWorkbenchView(page, 'Parameters');
  const panel = page.getByTestId('parameters-panel');
  await expect(panel).toBeVisible();
  await panel.getByRole('region', { name: 'Speed', exact: true }).getByRole('radio', { name: 'Fast', exact: true }).click();
  await panel.getByRole('region', { name: 'Access', exact: true }).getByRole('combobox').click();
  await page.getByRole('option', { name: 'Daybreak Blue', exact: true }).click();
  await expect.poll(async () => ((await asAda('GET', `/nodes/${MAIN}/settings`)) as { key: string; own: { value: unknown } | null }[]).find((setting) => setting.key === 'agent.params')?.own?.value).toEqual({ service_tier: 'fast', cyber_access_program: 'Daybreak Blue' });
  await panel.getByRole('region', { name: 'Context', exact: true }).getByLabel('Another size, in tokens').fill('300000');
  await expect(panel.getByRole('region', { name: 'Context', exact: true }).getByLabel('Another size, in tokens')).toHaveValue('300000');
  await expect(panel.getByRole('region', { name: 'Context', exact: true }).getByRole('button', { name: 'Set', exact: true })).toBeEnabled();
  await panel.getByRole('region', { name: 'Context', exact: true }).getByLabel('Another size, in tokens').press('Enter');
  await expect(panel.locator('[data-runs-with="context"]')).toHaveText('300K · chosen for this thread');
  await panel.getByRole('combobox', { name: 'Model', exact: true }).click();
  await page.getByRole('option', { name: 'GPT Mini', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Left behind with the change of model' })).toContainText('GPT Mini has no Fast speed switched on');
  await expect(panel.getByRole('region', { name: 'Speed', exact: true })).toHaveCount(0);
  await expect(panel.getByRole('region', { name: 'Access', exact: true })).toHaveCount(0);
  await expect(panel.getByRole('region', { name: 'Context', exact: true })).toHaveCount(0);
  await expect.poll(async () => ((await asAda('GET', `/nodes/${MAIN}/settings`)) as { key: string; own: { value: unknown } | null }[]).find((setting) => setting.key === 'agent.params')?.own).toBeNull();
});

// ---------------------------------------------------------------- the provider's Models tab

const editor = (page: Page) => page.getByRole('dialog', { name: 'What this model can be run with here' });

async function openModels(page: Page, provider: string): Promise<void> {
  await signIn(page);
  await page.goto(`/resources/providers/${provider}?tab=models`);
  await expect(page.getByRole('table', { name: 'Models' })).toBeVisible();
}

async function openEditor(page: Page, model: string): Promise<void> {
  await page.getByRole('button', { name: `Actions for ${model}`, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Edit options…' }).click();
  await expect(editor(page)).toBeVisible();
}

test("the editor of a model has its speeds, programs and context off until a manager turns each on, with what the source offered", async ({ page }) => {
  await openModels(page, CODEX);
  // At a glance: nothing is on, and what is on offer says so.
  await expect(page.getByRole('row', { name: /GPT-5\.5 Codex/ })).toContainText('Offered, not switched on: Fast, Daybreak Blue, Daybreak Red, context up to 1M');
  await openEditor(page, 'GPT-5.5 Codex');
  const dialog = editor(page);

  const speeds = dialog.getByRole('group', { name: 'Speeds' });
  await expect(speeds.getByRole('switch', { name: 'Fast' })).not.toBeChecked();
  await expect(speeds.getByText('Offered by Codex for this model')).toBeVisible();
  // What a thread starts with is asked only of what is switched on.
  await expect(speeds.getByRole('combobox', { name: 'Threads start with' })).toHaveCount(0);

  const programs = dialog.getByRole('group', { name: 'Access programs' });
  await expect(programs.getByRole('switch', { name: 'Daybreak Blue' })).not.toBeChecked();
  await expect(programs.getByRole('switch', { name: 'Daybreak Red' })).not.toBeChecked();
  await expect(programs.getByText('Offered by this plan')).toHaveCount(2);

  const context = dialog.getByRole('group', { name: 'Context' });
  await expect(context.getByLabel('Its own window, in tokens')).toHaveValue('272000');
  // The largest is empty until a manager fills it, with what was offered beside it.
  await expect(context.getByLabel('Largest, in tokens')).toHaveValue('');
  await expect(context.getByText('Up to 1M · offered by Codex for this model')).toBeVisible();
  await expect(context.getByRole('radiogroup', { name: 'Threads start with' })).toHaveCount(0);
  await expect(context.getByText('The conversation is shortened before it reaches the context, with room kept for the answer. That point is worked out; nobody sets it.')).toBeVisible();
  await expect(dialog.getByText('Switched on, the people who may use this model here can choose it for their turns.')).toBeVisible();
});

test('what a manager switches on is what the list says and what a thread on the model starts with, and the largest context starts threads with all of it', async ({ page }) => {
  await openModels(page, CODEX);
  await openEditor(page, 'GPT-5.5 Codex');
  const dialog = editor(page);

  const speeds = dialog.getByRole('group', { name: 'Speeds' });
  await speeds.getByRole('switch', { name: 'Fast' }).click();
  await speeds.getByRole('combobox', { name: 'Threads start with' }).click();
  // Standard is where a thread starts unless a manager says otherwise.
  await expect(page.getByRole('option', { name: 'Standard' })).toBeVisible();
  await page.getByRole('option', { name: 'Fast' }).click();
  await dialog.getByRole('group', { name: 'Access programs' }).getByRole('switch', { name: 'Daybreak Blue' }).click();

  const context = dialog.getByRole('group', { name: 'Context' });
  await context.getByRole('button', { name: 'Use 1M' }).click();
  await expect(context.getByLabel('Largest, in tokens')).toHaveValue('1000000');
  // Threads start with all of it as shipped; the model's own window is the other choice.
  await expect(context.getByRole('radio', { name: 'Full context · 1M', exact: true })).toBeChecked();
  await context.getByRole('radio', { name: 'Model window · 272K', exact: true }).click();
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('GPT-5.5 Codex was saved', { exact: true })).toBeVisible();

  const row = page.getByRole('row', { name: /GPT-5\.5 Codex/ });
  await expect(row).toContainText('Context 272K, up to 1M');
  await expect(row).toContainText('Speeds Fast');
  await expect(row).toContainText('Programs Daybreak Blue');
  await expect(row).toContainText('Offered, not switched on: Daybreak Red');

  // It is the entry that was written: opened again, the editor reads the same.
  await openEditor(page, 'GPT-5.5 Codex');
  const again = editor(page);
  await expect(again.getByRole('group', { name: 'Speeds' }).getByRole('switch', { name: 'Fast' })).toBeChecked();
  await expect(again.getByRole('group', { name: 'Speeds' }).getByRole('combobox', { name: 'Threads start with' })).toContainText('Fast');
  await expect(again.getByRole('group', { name: 'Context' }).getByRole('radio', { name: 'Model window · 272K', exact: true })).toBeChecked();
});

test("a largest context below the model's own window is refused at its field, and so is a start above the largest; what the server refuses is shown at the field it names", async ({ page }) => {
  await openModels(page, CODEX);
  await openEditor(page, 'GPT-5.5 Codex');
  const dialog = editor(page);
  const context = dialog.getByRole('group', { name: 'Context' });

  await context.getByLabel('Largest, in tokens').fill('200000');
  await expect(context.getByText("The largest context cannot be less than the model's own window (272,000).")).toBeVisible();
  // Saving is held until it is mended: the button says so by being blocked.
  await expect(dialog.getByRole('button', { name: 'Save' })).toHaveAttribute('aria-disabled', 'true');
  // A blocked button keeps the keyboard and the pointer, and a press does nothing: it is pressed regardless of its look.
  await dialog.getByRole('button', { name: 'Save' }).click({ force: true });
  await expect(editor(page)).toBeVisible();

  await context.getByLabel('Largest, in tokens').fill('300000');
  await expect(context.getByText(/The largest context cannot be less/)).toHaveCount(0);
  await context.getByRole('radio', { name: 'Custom' }).click();
  await context.getByLabel('Threads start with, in tokens').fill('500000');
  await expect(context.getByText('A thread cannot start with more than the largest (300,000).')).toBeVisible();
  await context.getByLabel('Threads start with, in tokens').fill('280000');
  await expect(context.getByText(/A thread cannot start with more than/)).toHaveCount(0);

  // What the server refuses is put at the field it names, in its own words.
  await page.route('**/api/v1/providers/*/models/*', async (route) => {
    if (route.request().method() !== 'PATCH') {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 400,
      contentType: 'application/problem+json',
      body: JSON.stringify({ type: 'orch:error:request.invalid', code: 'request.invalid', title: 'x', status: 400, detail: 'x', request_id: '', errors: [{ path: 'parameters.default_effort', message: 'an effort a thread starts with is one of the efforts the model lists' }] }),
    });
  });
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(dialog.getByRole('group', { name: 'Efforts and images' }).getByText('an effort a thread starts with is one of the efforts the model lists')).toBeVisible();
  await expect(editor(page)).toBeVisible();
});

test("a model's groups are chips: put in by typing, taken from the provider's others, taken out, and counted above the list", async ({ page }) => {
  await openModels(page, OPENAI);
  const strip = page.getByRole('region', { name: 'Groups' });
  await expect(strip.getByText('frontier', { exact: true })).toBeVisible();
  await expect(strip.getByText('small', { exact: true })).toBeVisible();
  await expect(strip.getByText('1 model')).toHaveCount(2);
  await expect(strip.getByRole('button', { name: 'Limit this group…' })).toHaveCount(2);

  await openEditor(page, 'GPT-5.4');
  const groups = editor(page).getByRole('group', { name: 'Groups' });
  const chips = groups.getByRole('list', { name: 'Groups of this model' });
  await expect(chips.getByText('frontier')).toBeVisible();
  // The provider's other groups are offered; one is taken, one is typed, one is taken out.
  await groups.getByRole('group', { name: 'Other groups of this provider' }).getByRole('button', { name: 'small' }).click();
  await expect(chips.getByText('small')).toBeVisible();
  await groups.getByLabel('Put it in a group').fill('Big Group');
  await groups.getByLabel('Put it in a group').press('Enter');
  await expect(groups.getByText('A group is named by 1 to 40 of a to z, 0 to 9, - and _, beginning with a letter or a digit.')).toBeVisible();
  await groups.getByLabel('Put it in a group').fill('vision');
  await groups.getByLabel('Put it in a group').press('Enter');
  await expect(chips.getByText('vision')).toBeVisible();
  await groups.getByRole('button', { name: 'Take it out of the group frontier' }).click();
  await expect(chips.getByText('frontier')).toHaveCount(0);
  await editor(page).getByRole('button', { name: 'Save' }).click();
  await expect(page.getByText('GPT-5.4 was saved', { exact: true })).toBeVisible();

  const row = page.getByRole('row').filter({ has: page.getByRole('button', { name: 'Actions for GPT-5.4', exact: true }) });
  await expect(row.getByText('small', { exact: true })).toBeVisible();
  await expect(row.getByText('vision', { exact: true })).toBeVisible();
  // The strip counts what is there now: two models in small, one in vision, and none in frontier.
  await expect(strip.getByText('2 models')).toBeVisible();
  await expect(strip.getByText('frontier', { exact: true })).toHaveCount(0);
});

test('the limits that cover a model are listed by its name, its family and its group, and adding one starts with that model', async ({ page }) => {
  // Fast and the over-272k step count 39 credits today: ((3-2)*5 + 2*0.5 + 0.6*22.5)*2.
  // Forty-five credits warns at 80% while leaving room before the limit is reached.
  await writeHistory(1);
  await makeCap(PEOPLE.ada, { provider_id: CODEX, meter: 'running', amount: '4' });
  await makeCap(PEOPLE.ada, { provider_id: CODEX, meter: 'credits', amount: '45', period: 'month', models: ['@frontier'], warn_percent: 80 });
  await makeCap(PEOPLE.ada, { provider_id: CODEX, meter: 'tokens', amount: '2000000', split: 'each', models: ['gpt-5.5*'] });
  await makeCap(PEOPLE.ada, { provider_id: CODEX, meter: 'running', amount: '1', tiers: ['fast'] });
  await openModels(page, CODEX);
  await openEditor(page, 'GPT-5.5 Codex');
  const limits = editor(page).getByRole('group', { name: 'Limits on this model' });
  const list = limits.getByRole('list', { name: 'Limits on this model' });
  // A cap that names its group and one that names its family; a cap of the whole provider is counted, not listed.
  await expect(list.getByRole('listitem')).toHaveCount(2);
  const group = list.getByRole('listitem').filter({ hasText: 'the frontier group' });
  await expect(group.getByText('Everyone together', { exact: true })).toBeVisible();
  await expect(group.getByText('45 credits a month', { exact: true })).toBeVisible();
  await expect(group).toContainText('39 of 45');
  await expect(group).toContainText('Near its limit');
  const family = list.getByRole('listitem').filter({ hasText: 'gpt-5.5*' });
  await expect(family.getByText('Each person', { exact: true })).toBeVisible();
  await expect(family.getByText('2,000,000 tokens a day', { exact: true })).toBeVisible();
  await expect(limits.getByText('2 caps on the whole provider cover it as well.')).toBeVisible();
  await limits.getByRole('button', { name: 'Add a limit…' }).click();
  const add = page.getByRole('dialog', { name: 'Add a limit', exact: true });
  await expect(add).toBeVisible();
  await expect(add.getByRole('button', { name: 'Take gpt-5.5-codex out', exact: true })).toBeVisible();
  await add.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(add).toHaveCount(0);
});

test('a suggestion that lists a faster speed, a program and a larger context is added with all of it offered and none of it on', async ({ page }) => {
  await openModels(page, CODEX);
  const suggestion = page.getByRole('checkbox', { name: /GPT-6 Sol \(A\)/ });
  await expect(suggestion).toBeVisible();
  await expect(page.getByText('Offered, not switched on: Fast, Daybreak Blue, context up to 1M')).toBeVisible();
  await suggestion.check();
  await page.getByRole('button', { name: 'Add selected' }).click();
  await expect(page.getByText('1 model was added', { exact: true })).toBeVisible();

  const row = page.getByRole('row', { name: /GPT-6 Sol \(A\)/ });
  await expect(row).toContainText('Offered, not switched on: Fast, Daybreak Blue, context up to 1M');
  await expect(row).not.toContainText('Speeds');
  await openEditor(page, 'GPT-6 Sol (A)');
  await expect(editor(page).getByRole('switch', { name: 'Fast' })).not.toBeChecked();
  await expect(editor(page).getByRole('switch', { name: 'Daybreak Blue' })).not.toBeChecked();
  await expect(editor(page).getByLabel('Largest, in tokens')).toHaveValue('');
});

test('the editor is worked by the keyboard alone: opened from a row, a switch turned on, what a thread starts with chosen, saved', async ({ page }) => {
  await openModels(page, CODEX);
  const menu = page.getByRole('button', { name: 'Actions for GPT-5.5 Codex', exact: true });
  await menu.focus();
  await page.keyboard.press('Enter');
  await page.keyboard.press('Enter');
  const dialog = editor(page);
  await expect(dialog).toBeVisible();

  // To the Fast switch by Tab, and turned on with Space.
  const fast = dialog.getByRole('switch', { name: 'Fast' });
  for (let step = 0; step < 20 && !(await fast.evaluate((element) => element === document.activeElement)); step += 1) await page.keyboard.press('Tab');
  await expect(fast).toBeFocused();
  await page.keyboard.press('Space');
  await expect(fast).toBeChecked();
  // What a thread starts with is the next stop, and is chosen with the arrow keys.
  await page.keyboard.press('Tab');
  const starts = dialog.getByRole('group', { name: 'Speeds' }).getByRole('combobox', { name: 'Threads start with' });
  await expect(starts).toBeFocused();
  await page.keyboard.press('Enter');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  await expect(starts).toContainText('Fast');
  // On to the last button of the dialog, which saves.
  const save = dialog.getByRole('button', { name: 'Save' });
  for (let step = 0; step < 80 && !(await save.evaluate((element) => element === document.activeElement)); step += 1) await page.keyboard.press('Tab');
  await expect(save).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByText('GPT-5.5 Codex was saved', { exact: true })).toBeVisible();
  await expect(page.getByRole('row', { name: /GPT-5\.5 Codex/ })).toContainText('Speeds Fast');
});

// ---------------------------------------------------------------- the thread's model control

const trigger = (page: Page, name: string) => page.getByRole('button', { name: new RegExp(`^Model and effort: ${name}`) });
const card = (page: Page) => page.getByRole('dialog', { name: 'Model and effort' });

async function openThread(page: Page, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(`/thread/${MAIN}`);
}

async function openControl(page: Page, name: string): Promise<void> {
  await trigger(page, name).click();
  await expect(card(page)).toBeVisible();
  await expect(card(page).getByRole('menu', { name: 'Models' })).toBeVisible();
}

for (const width of [400, 1440]) {
  test(`model and effort options fill the list without pill borders at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await openThread(page); await openControl(page, 'GPT Sol \\(A\\)');
    const menu = card(page).getByRole('menu', { name: 'Models', exact: true });
    const chosenModel = menu.getByRole('menuitemradio', { name: 'GPT Sol (A)', exact: true, checked: true });
    const chosenEffort = menu.getByRole('group', { name: 'Efforts of GPT Sol (A)', exact: true }).getByRole('menuitemradio', { name: 'Medium', exact: true, checked: true });
    await expect(chosenModel).toBeEnabled(); await expect(chosenEffort).toBeVisible();
    await page.mouse.move(0, 0); await card(page).getByRole('textbox', { name: 'Find a model', exact: true }).focus();
    await expect(chosenModel.locator('xpath=ancestor::li[1]')).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
    const rows = menu.getByRole('menuitemradio'); expect(await rows.count()).toBeGreaterThanOrEqual(4);
    const list = await menu.evaluate((element) => ({ x: element.getBoundingClientRect().left, width: element.clientWidth }));
    for (const row of await rows.all()) {
      const box = await row.boundingBox(); if (box === null) throw new Error('A model or effort option has no rendered bounds');
      expect(Math.abs(box.x - list.x)).toBeLessThanOrEqual(1);
      expect(Math.abs(box.width - list.width)).toBeLessThanOrEqual(1);
      await expect(row.locator('..')).toHaveAttribute('data-choice-layout', 'picker');
      await expect(row).not.toHaveAttribute('aria-pressed');
      for (const edge of ['top', 'right', 'bottom', 'left']) await expect(row).toHaveCSS(`border-${edge}-color`, 'rgba(0, 0, 0, 0)');
      await expect(row).toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
      await expect(row.locator('svg.lucide-check')).toHaveCount(await row.getAttribute('aria-checked') === 'true' ? 1 : 0);
    }
    // The child effort's name is indented; its focus/click target still spans the same list.
    await expect(chosenModel.locator('[data-choice-name]')).toHaveText('GPT Sol (A)');
    await expect(chosenEffort.locator('[data-choice-name]')).toHaveText('Medium');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
  });
}

test('a model picker without effort choices has one Model question and no absent control’s help', async ({ page }) => {
  for (const [model_name, display_name] of [['gpt-sol', 'GPT Sol (A)'], ['gpt-sol-b', 'GPT Sol (B)'], ['gpt-mini', 'GPT Mini']]) {
    await control('/model/entry', { model_name, display_name, parameters: { context_window: 200_000, efforts: [] } });
  }
  await openThread(page);
  await openControl(page, 'GPT Sol \\(A\\)');
  const picker = card(page);
  await expect(picker.getByRole('group', { name: /^Efforts of/ })).toHaveCount(0);
  await expect(picker.locator('[data-help-article="resources"][data-help-heading="providers-and-models"]')).toHaveCount(1);
  await expect(picker.getByRole('button', { name: 'Help: Effort', exact: true })).toHaveCount(0);
  await expect(picker.getByRole('button', { name: 'Help: Agent parameters', exact: true })).toHaveCount(0);
  await picker.getByRole('button', { name: 'Help: Model', exact: true }).click();
  const explanation = page.getByRole('dialog', { name: 'Model', exact: true });
  await expect(explanation.getByRole('button', { name: 'Open Model entry', exact: true })).toBeVisible();
  await expect(explanation.getByRole('button', { name: 'Open Effort entry', exact: true })).toHaveCount(0);
});

test('the real Parameters panel keeps one approvals guide beside its approval control', async ({ page }) => {
  await openThread(page);
  await showWorkbenchView(page, 'Parameters');
  const panel = page.getByTestId('parameters-panel');
  await expect(panel.getByRole('button', { name: 'How approvals work', exact: true })).toBeVisible();
  await expect(panel.locator('[data-help-article="how-approvals-work"]')).toHaveCount(1);
  await expect(panel.getByRole('button', { name: 'Help: Approval mode', exact: true })).toHaveCount(0);
  await expect(panel.getByRole('button', { name: 'Help: Allowed approval modes', exact: true })).toHaveCount(0);
  await panel.getByRole('button', { name: 'How approvals work', exact: true }).click();
  const source = page.waitForRequest((request) => request.method() === 'GET' && new URL(request.url()).pathname.endsWith(`/nodes/${MAIN}/settings`));
  await page.getByRole('button', { name: 'Open Approval mode entry', exact: true }).click();
  await source;
  await expect(page.getByRole('heading', { name: 'Approval mode', exact: true })).toBeVisible();
});

for (const hadTurns of [false, true]) {
  test(`a model switch reaches another open page within a second ${hadTurns ? 'after a turn' : 'before the first turn'}`, async ({ page, context }) => {
    await configureThread(MAIN, { reset: true });
    await emit(MAIN, { thread: { state: 'idle', model: 'gpt-sol', model_display: 'GPT Sol (A)', effort: 'medium' }, ...(hadTurns ? { turns: [turn('earlier', { ordinal: 1, state: 'completed', inputId: 'earlier-input' })] } : {}) });
    const before = await asAda('GET', `/threads/${MAIN}/sync`) as { turns: unknown[] };
    expect(before.turns).toHaveLength(hadTurns ? 1 : 0);
    await openThread(page);
    const other = await context.newPage();
    await other.goto(`/thread/${MAIN}`);
    let settingsReads = 0;
    other.on('request', (request) => { if (request.method() === 'GET' && new URL(request.url()).pathname.endsWith(`/nodes/${MAIN}/settings`)) settingsReads += 1; });
    await openControl(other, 'GPT Sol');
    await expect(card(other).getByRole('menuitemradio', { name: /^GPT Sol \(A\)/, checked: true })).toBeEnabled();
    const reads = settingsReads;
    await openControl(page, 'GPT Sol');
    const changed = page.waitForRequest((request) => request.method() === 'PUT' && new URL(request.url()).pathname.endsWith(`/nodes/${MAIN}/settings/agent.model`));
    await card(page).getByRole('menuitemradio', { name: /^GPT Mini/ }).click();
    const body: unknown = (await bodySent(await changed));
    expect(body).toMatchObject({ value: 'gpt-mini', expected_revision: 0 });
    await expect(card(other).getByRole('menuitemradio', { name: /^GPT Mini/, checked: true })).toBeVisible({ timeout: 1_000 });
    expect(settingsReads).toBeGreaterThan(reads);
    await expect(trigger(other, 'GPT Mini')).toHaveCount(1);
    await other.close();
  });
}

test("Speed, Access and Context are absent for a model whose provider enabled none of them, and there once a manager has", async ({ page }) => {
  await emit(MAIN, { thread: { model: 'gpt-mini', model_display: 'GPT Mini', effort: null } });
  await openThread(page);
  await openControl(page, 'GPT Mini');
  await expect(card(page).getByRole('region', { name: 'Speed' })).toHaveCount(0);
  await expect(card(page).getByRole('region', { name: 'Access' })).toHaveCount(0);
  await expect(card(page).getByRole('region', { name: 'Context' })).toHaveCount(0);
  await page.keyboard.press('Escape');

  // A manager of the provider switches a speed on for this model: no thread is made fast by it, it is there as a choice.
  await control('/model/entry', { model_name: 'gpt-mini', display_name: '', parameters: { service_tiers: [{ id: 'fast', label: 'Fast' }], context_window: 200_000 } });
  await page.reload();
  await openControl(page, 'GPT Mini');
  const speed = card(page).getByRole('region', { name: 'Speed' });
  await expect(speed.getByText('Standard', { exact: true }).first()).toBeVisible();
  await expect(speed.getByText('as this model starts')).toBeVisible();
  await expect(speed.getByRole('radio', { name: 'Standard' })).toBeChecked();
  await expect(speed.getByRole('radio', { name: 'Fast' })).not.toBeChecked();
  await expect(card(page).getByRole('region', { name: 'Context' })).toHaveCount(0);
  await expect(trigger(page, 'GPT Mini\\. Change')).toHaveCount(1);
});

test('the control opens on what the model starts with, and says where each comes from', async ({ page }) => {
  await emit(MAIN, { thread: { model: 'gpt-sol-b', model_display: 'GPT Sol (B)', effort: 'low' } });
  await openThread(page);
  // The model's managers made Fast and a context of 200K what a thread on it starts with.
  await expect(trigger(page, 'GPT Sol \\(B\\) \\(Low, Fast, 200K\\)')).toHaveCount(1);
  await openControl(page, 'GPT Sol \\(B\\)');
  const speed = card(page).getByRole('region', { name: 'Speed' });
  await expect(speed.getByRole('radio', { name: 'Fast' })).toBeChecked();
  await expect(speed.locator('[data-runs-with="speed"]')).toHaveText('Fast · as this model starts');
  // A thread cannot ask for Standard where its managers made Fast the start, and the control says so.
  await expect(speed.getByRole('radio', { name: 'Standard' })).toBeDisabled();
  await expect(speed.getByText('Threads on this model start with Fast, as its managers chose; a thread cannot go back to Standard.')).toBeVisible();
  const context = card(page).getByRole('region', { name: 'Context' });
  await expect(context.locator('[data-runs-with="context"]')).toHaveText('200K · as this model starts');
  await expect(context.getByRole('radio', { name: '200K' })).toBeChecked();
  // This model has no access program open.
  await expect(card(page).getByRole('region', { name: 'Access' })).toHaveCount(0);
});

test('a speed, an access program and a context are chosen for the thread, each saying who chose it, and the control shows what the thread runs with', async ({ page }) => {
  await openThread(page);
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · 1M\\. Change')).toHaveCount(1);
  await openControl(page, 'GPT Sol \\(A\\)');
  const speed = card(page).getByRole('region', { name: 'Speed' });
  // What the model starts with: the ordinary speed, no program, all of the largest context. Fast is a choice and no more.
  await expect(speed.locator('[data-runs-with="speed"]')).toHaveText('Standard · as this model starts');
  await expect(speed.getByText('Fast: quicker answers, counted at twice the rate here')).toBeVisible();
  await expect(speed.getByText('Takes effect from the next turn.')).toBeVisible();
  const access = card(page).getByRole('region', { name: 'Access' });
  await expect(access.locator('[data-runs-with="access"]')).toHaveText('None · as this model starts');
  const context = card(page).getByRole('region', { name: 'Context' });
  await expect(context.locator('[data-runs-with="context"]')).toHaveText('1M · as this model starts');
  await expect(context.getByText('Takes effect in a new session.')).toBeVisible();
  // Where the conversation is shortened is in the adapter's words, and nobody sets it.
  await expect(context.getByText(/Codex shortens the conversation before it reaches that size/)).toBeVisible();

  await speed.getByRole('radio', { name: 'Fast' }).click();
  await expect(speed.locator('[data-runs-with="speed"]')).toHaveText('Fast · chosen for this thread');
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · 1M\\. Change')).toHaveCount(1);

  await access.getByRole('combobox', { name: 'Access' }).click();
  await page.getByRole('option', { name: 'Daybreak Blue' }).click();
  await expect(access.locator('[data-runs-with="access"]')).toHaveText('Daybreak Blue · chosen for this thread');
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · Daybreak Blue · 1M\\. Change')).toHaveCount(1);

  await context.getByRole('radio', { name: /^400K/ }).click();
  await expect(context.locator('[data-runs-with="context"]')).toHaveText('400K · chosen for this thread');
  // Another number is chosen in its own field, within what the model opens.
  await context.getByLabel('Another size, in tokens').fill('1000');
  await context.getByLabel('Another size, in tokens').press('Enter');
  await expect(context.getByText('At least 32K.')).toBeVisible();
  await context.getByLabel('Another size, in tokens').fill('2000000');
  await context.getByLabel('Another size, in tokens').press('Enter');
  await expect(context.getByText('At most 1M, the largest open for this model.')).toBeVisible();
  await context.getByLabel('Another size, in tokens').fill('750k');
  await context.getByLabel('Another size, in tokens').fill('750000');
  await context.getByLabel('Another size, in tokens').press('Enter');
  await expect(context.locator('[data-runs-with="context"]')).toHaveText('750K · chosen for this thread');
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · Daybreak Blue · 750K\\. Change')).toHaveCount(1);

  // What was chosen is the thread's own, written as one object, and choosing what the model starts with takes it back.
  await expect.poll(async () => ((await asAda('GET', `/nodes/${MAIN}/settings`)) as { key: string; own: { value: unknown } | null }[]).find((setting) => setting.key === 'agent.params')?.own?.value).toEqual({ service_tier: 'fast', cyber_access_program: 'Daybreak Blue', context: 750_000 });
  await speed.getByRole('radio', { name: 'Standard' }).click();
  await expect(speed.locator('[data-runs-with="speed"]')).toHaveText('Standard · as this model starts');
  await expect.poll(async () => ((await asAda('GET', `/nodes/${MAIN}/settings`)) as { key: string; own: { value: unknown } | null }[]).find((setting) => setting.key === 'agent.params')?.own?.value).toEqual({ cyber_access_program: 'Daybreak Blue', context: 750_000 });
});

test('a context that crosses a step of the rate says so', async ({ page }) => {
  await openThread(page);
  await openControl(page, 'GPT Sol \\(A\\)');
  const context = card(page).getByRole('region', { name: 'Context' });
  await expect(context.getByText('Prompts above 272K are counted at a higher rate.', { exact: true })).toHaveCount(0);
  const high = context.getByRole('radio', { name: '1M · higher rate', exact: true });
  await high.hover();
  await expect(page.getByRole('tooltip')).toHaveText('Prompts above 272K are counted at a higher rate.');
  await expect(high).toHaveAccessibleDescription('Prompts above 272K are counted at a higher rate.');
  await expect(context.locator('[data-choice-selected-description]')).toHaveCount(0);
  await expect(context.getByRole('radio', { name: '272K', exact: true })).toBeVisible();
  await context.getByRole('radio', { name: '272K', exact: true }).click();
  await expect(context.locator('[data-runs-with="context"]')).toHaveText('272K · chosen for this thread');
  // Chosen, the model's own window is still one that crosses nothing: the line is about what is on offer.
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · 272K\\. Change')).toHaveCount(1);
});

test('Access is absent where the place has not switched access programs on', async ({ page }) => {
  await control('/model/feature', { feature: 'codex.cyber_access', on: false });
  await openThread(page);
  await openControl(page, 'GPT Sol \\(A\\)');
  await expect(card(page).getByRole('region', { name: 'Speed' })).toBeVisible();
  await expect(card(page).getByRole('region', { name: 'Context' })).toBeVisible();
  await expect(card(page).getByRole('region', { name: 'Access' })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await control('/model/feature', { feature: 'codex.cyber_access', on: true });
  await page.reload();
  await openControl(page, 'GPT Sol \\(A\\)');
  await expect(card(page).getByRole('region', { name: 'Access' })).toBeVisible();
});

test("a change of model drops a choice that the new model does not have, with a line saying so, and keeps one that fits", async ({ page }) => {
  await control('/thread/set', { thread: MAIN, params: { service_tier: 'fast', cyber_access_program: 'Daybreak Blue', context: 300_000 } });
  await openThread(page);
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · Daybreak Blue · 300K')).toHaveCount(1);
  await openControl(page, 'GPT Sol \\(A\\)');

  // GPT Sol (B) opens Fast and up to 400K, and no program: the program goes, and the context of 300K fits.
  await card(page).getByRole('menuitemradio', { name: /^GPT Sol \(B\)/ }).click();
  const notice = card(page).getByRole('status').filter({ hasText: 'Left behind with the change of model' });
  await expect(notice).toBeVisible();
  await expect(notice).toContainText('GPT Sol (B) has not switched Daybreak Blue on, so this thread uses no access program.');
  await expect(notice).not.toContainText('Fast');
  await expect(notice).not.toContainText('300K');
  await expect(trigger(page, 'GPT Sol \\(B\\) · medium · Fast · 300K')).toHaveCount(1);
  await expect(card(page).getByRole('region', { name: 'Access' })).toHaveCount(0);
  await expect.poll(async () => ((await asAda('GET', `/nodes/${MAIN}/settings`)) as { key: string; own: { value: unknown } | null }[]).find((setting) => setting.key === 'agent.params')?.own?.value).toEqual({ service_tier: 'fast', context: 300_000 });

  // GPT Mini opens nothing: every choice goes, and the thread runs with what that model starts with.
  await card(page).getByRole('menuitemradio', { name: /^GPT Mini/ }).click();
  await expect(card(page).getByRole('status').filter({ hasText: 'Left behind with the change of model' })).toContainText('GPT Mini has no Fast speed switched on, so this thread runs at Standard.');
  await expect(card(page).getByRole('status').filter({ hasText: 'Left behind with the change of model' })).toContainText('GPT Mini does not open a context of 300K, so this thread works with its own window.');
  await expect(trigger(page, 'GPT Mini\\. Change')).toHaveCount(1);
  await expect(card(page).getByRole('region', { name: 'Speed' })).toHaveCount(0);
  await expect.poll(async () => ((await asAda('GET', `/nodes/${MAIN}/settings`)) as { key: string; own: { value: unknown } | null }[]).find((setting) => setting.key === 'agent.params')?.own).toBeNull();
});

test("a choice is credited to whoever made it, as the history of the thread says", async ({ page }) => {
  await control('/thread/set', { thread: MAIN, params: { service_tier: 'fast' } });
  await emit(MAIN, {
    items: [marker('m-speed', { marker: 'setting_changed', data: { by: GRACE, from: {}, key: 'agent.params', label: 'Speed', to: { service_tier: 'fast' } } })],
  });
  await openThread(page);
  await openControl(page, 'GPT Sol \\(A\\)');
  await expect(card(page).getByRole('region', { name: 'Speed' }).locator('[data-runs-with="speed"]')).toHaveText('Fast · chosen by Grace Hopper');
  // Somebody else changes it while the control is open: the lines of the history say so, and the control reads again.
  await control('/thread/set', { thread: MAIN, params: {} });
  await emit(MAIN, {
    items: [marker('m-speed-2', { marker: 'setting_changed', data: { by: ADA, from: { service_tier: 'fast' }, key: 'agent.params', label: 'Speed', to: {} } })],
  });
  await expect(card(page).getByRole('region', { name: 'Speed' }).locator('[data-runs-with="speed"]')).toHaveText('Standard · as this model starts');
});

test('somebody who only drives the thread reads the control and is told who can change it, and a place that fixes a choice says so', async ({ page }) => {
  // A developer who holds Interact and not Manage on the thread.
  await deny('dora', ['thread.manage']);
  await openThread(page, PEOPLE.dora);
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toBeEnabled();
  await openControl(page, 'GPT Sol \\(A\\)');
  await expect(card(page).getByText('Changing the model is for whoever manages this thread: Ada Lovelace')).toBeVisible();
  await expect(card(page).getByRole('menuitemradio', { name: /^GPT Sol \(B\)/ })).toBeDisabled();
  await expect(card(page).getByRole('radio', { name: 'Fast' })).toBeDisabled();
  await expect(card(page).getByRole('combobox', { name: 'Access' })).toBeDisabled();
  await expect(card(page).getByRole('radio', { name: /^400K/ })).toBeDisabled();
  // What the thread runs with is still read.
  await expect(card(page).getByRole('region', { name: 'Speed' }).locator('[data-runs-with="speed"]')).toHaveText('Standard · as this model starts');
});

test('a place that decides the choices of its threads says so, and the choices stay as they are', async ({ page }) => {
  await control('/thread/set', { thread: MAIN, lock: { scope: 'project', name: 'Web' } });
  await openThread(page);
  await openControl(page, 'GPT Sol \\(A\\)');
  await expect(card(page).getByRole('region', { name: 'Speed' }).getByText('Web decides this for its threads, so it cannot be changed here.')).toBeVisible();
  await expect(card(page).getByRole('radio', { name: 'Fast' })).toBeDisabled();
  // The model itself is still the manager's to change.
  await expect(card(page).getByRole('menuitemradio', { name: /^GPT Sol \(B\)/ })).toBeEnabled();
});

test('the model control is worked by the keyboard alone', async ({ page }) => {
  await openThread(page);
  await trigger(page, 'GPT Sol \\(A\\)').focus();
  await page.keyboard.press('Enter');
  await expect(card(page)).toBeVisible();
  const standard = card(page).getByRole('radio', { name: 'Standard' });
  for (let step = 0; step < 40 && !(await standard.evaluate((element) => element === document.activeElement)); step += 1) await page.keyboard.press('Tab');
  // A group of choices takes one stop, and the arrow keys move within it.
  await expect(standard).toBeFocused();
  await page.keyboard.press('ArrowRight');
  await expect(card(page).getByRole('radio', { name: 'Fast' })).toBeFocused();
  await page.keyboard.press('Space');
  await expect(card(page).getByRole('radio', { name: 'Fast' })).toBeChecked();
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · 1M')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(card(page)).toHaveCount(0);
  await expect(trigger(page, 'GPT Sol \\(A\\)')).toBeFocused();
});

// ---------------------------------------------------------------- the box and the footer

const composer = (page: Page) => page.getByRole('textbox', { name: 'Message to the agent' });

test('/speed, /access and /context are in the box where the model has them, open the control on the choice they name, and make a choice written out', async ({ page }) => {
  await openThread(page);
  await composer(page).fill('/');
  const list = page.getByRole('listbox', { name: 'Commands' });
  await expect(list.getByRole('option')).toHaveCount(7);
  for (const name of ['/model', '/effort', '/speed', '/access', '/context', '/file', '/document']) await expect(list.getByRole('option', { name })).toBeVisible();
  // One begins with what was typed; Enter runs it, and the control opens on its choice.
  await composer(page).fill('/sp');
  await expect(list.getByRole('option')).toHaveCount(1);
  await composer(page).press('Enter');
  await expect(composer(page)).toHaveValue('');
  await expect(card(page)).toBeVisible();
  await expect(card(page).getByRole('radio', { name: 'Standard' })).toBeFocused();
  await page.keyboard.press('Escape');

  // Written out with what to choose, the command makes the choice.
  await composer(page).fill('/speed fast');
  await composer(page).press('Enter');
  await expect(composer(page)).toHaveValue('');
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · 1M')).toHaveCount(1);
  await composer(page).fill('/context 400k');
  await composer(page).press('Enter');
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · 400K')).toHaveCount(1);
  await composer(page).fill('/access daybreak blue');
  await composer(page).press('Enter');
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Fast · Daybreak Blue · 400K')).toHaveCount(1);
  await composer(page).fill('/speed standard');
  await composer(page).press('Enter');
  await expect(trigger(page, 'GPT Sol \\(A\\) · medium · Daybreak Blue · 400K')).toHaveCount(1);

  // What is not open for this model is said, and nothing is sent.
  await composer(page).fill('/speed turbo');
  await composer(page).press('Enter');
  await expect(page.getByRole('alert').filter({ hasText: 'is not open for this model' })).toContainText('Speed “turbo” is not open for this model. Open: Standard, Fast.');
  await expect(composer(page)).toHaveValue('/speed turbo');
  await composer(page).fill('/context 5');
  await composer(page).press('Enter');
  await expect(page.getByRole('alert').filter({ hasText: 'At least 32K.' })).toBeVisible();
  // A message that only begins with a slash goes as a message.
  await composer(page).fill('/etc/hosts is wrong');
  await expect(page.getByRole('listbox', { name: 'Commands' })).toHaveCount(0);
});

test('the commands of a model that has none of the three leave them out, and a reader who may not manage the thread sees them disabled with the reason', async ({ page }) => {
  await emit(MAIN, { thread: { model: 'gpt-mini', model_display: 'GPT Mini', effort: null } });
  await deny('dora', ['thread.manage']);
  await openThread(page, PEOPLE.dora);
  await composer(page).fill('/');
  const list = page.getByRole('listbox', { name: 'Commands' });
  await expect(list.getByRole('option')).toHaveCount(4);
  await expect(list.getByRole('option', { name: /^\/speed/ })).toHaveCount(0);
  await expect(list.getByRole('option', { name: /^\/model/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(list.getByText('Changing the model is for whoever manages this thread: Ada Lovelace').first()).toBeVisible();
  await composer(page).press('Enter');
  await expect(card(page)).toHaveCount(0);
  await expect(page.getByRole('alert').filter({ hasText: 'Changing the model is for whoever manages this thread' })).toBeVisible();
});

test('the footer of a turn says what the turn ran with: its speed, its access program and its context', async ({ page }) => {
  await playScene(MAIN, 'history');
  const ran = (params: Record<string, unknown>) =>
    emit(MAIN, {
      turns: [{ ...turn('t2', { ordinal: 2, state: 'completed', inputId: 'in-2', startedAt: '2026-10-01T10:10:00.000Z', completedAt: '2026-10-01T10:15:00.000Z', model: 'gpt-sol', effort: 'medium', tokens: 41_900, params }), model_display: 'GPT Sol (A)' }],
    });
  await ran({ service_tier: 'fast', cyber_access_program: 'Daybreak Blue', context: 400_000, context_window: 272_000 });
  await openThread(page);
  const footer = page.locator('[data-footer="t2"]');
  await expect(footer).toContainText('GPT Sol (A) · medium');
  await expect(footer).toContainText('Fast · Daybreak Blue · 400K');
  // A turn at the ordinary speed and with no program says nothing of them, and the context it ran with.
  await ran({ context: 1_000_000, context_window: 272_000 });
  await expect(footer).toContainText('GPT Sol (A) · medium');
  await expect(footer).toContainText('1M');
  await expect(footer).not.toContainText('Fast');
  // A turn that recorded nothing says nothing.
  await ran({});
  await expect(footer).toContainText('GPT Sol (A) · medium');
  await expect(footer).not.toContainText('1M');
});
