/** Every screen of usage, asked in both themes whether anything on it is a serious barrier: one's own usage with its tables open, a place's for somebody who reads reports and for somebody who reads figures, an empty span, a place with each kind of cap, the form that makes a cap, a provider's two tabs, the page of one's own allowances, the board on an overview, the rates of an org with the form of a rate and the catalogue, and a thread with a cap: its chip, the prompt at a limit and what limits a blocked thread. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';
import { ACME, makeCap, PLACES, setRunning, setWaiting, useNow } from '../e2e/usage-support.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';

test.beforeEach(async () => {
  await scenario();
});

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

test('one\'s own usage: the figures, the chart, and each table', async ({ page }) => {
  await open(page, '/usage');
  await expect(page.getByRole('list', { name: 'Figures of the span' })).toBeVisible();
  await expect(page.locator('canvas')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  for (const name of ['By project', 'By model', 'By provider', 'By agent', 'By environment', 'By speed', 'By thread']) {
    await page.getByRole('tab', { name }).click();
    await expect(page.getByRole('tab', { name, selected: true })).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
});

test('one\'s own usage with a table narrowing it, the chart as a table, and the card that picks a span', async ({ page }) => {
  await open(page, '/usage');
  await page.getByRole('tab', { name: 'By model' }).click();
  await page.getByRole('table', { name: 'By model' }).getByRole('button', { name: /^GPT-5\.4/ }).click();
  await expect(page.getByRole('list', { name: 'What the screen is narrowed to' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByText('Show the figures as a table').click();
  await expect(page.getByRole('table', { name: /Use over time/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('button', { name: 'Custom…' }).click();
  await expect(page.getByRole('dialog', { name: 'Choose two days' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a span with nothing in it', async ({ page }) => {
  await open(page, '/usage', PEOPLE.vera);
  await expect(page.getByText('Nothing was used in this span.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a place for somebody who reads its reports, with a cap of each kind, and the record that opens', async ({ page }) => {
  const running = await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'running', amount: '2' });
  await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '12000000', warn_percent: 80 });
  await makeCap(PEOPLE.ada, { node_id: ACME.org, meter: 'credits', amount: '300', period: 'week', split: 'each' });
  await useNow('A');
  await setRunning([
    { thread: PLACES.mainThread, user: 'ada', provider: RUNS_ON.openAiKey },
    { thread: ACME.repro, user: 'grace', provider: RUNS_ON.openAiKey },
  ]);
  await setWaiting(running.id, [{ thread: ACME.depsThread, author: 'dora', since: new Date().toISOString(), mine: false }]);
  await open(page, `/usage?scope=node:${PLACES.web}`);
  const limits = page.getByRole('region', { name: 'Limits' });
  // Two caps of the place, and the one of Acme that counts each person, which is a row for each of the three who used something.
  await expect(limits.getByRole('listitem')).toHaveCount(5);
  await expectAccessibleInBothThemes(page);
  await limits.getByRole('button', { name: '1 waiting' }).click();
  await expect(limits.getByRole('list', { name: 'Waiting for room, in the order they will run' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('table', { name: 'Records' }).getByRole('button', { name: 'More about this record' }).first().click();
  await expect(page.getByRole('dialog', { name: 'More about this record' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a place for somebody who reads its figures only, and for one who reads nothing but their own', async ({ page }) => {
  await open(page, `/usage?scope=node:${PLACES.web}`, PEOPLE.vera);
  await expect(page.getByText('as its page lets you see them')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the form that makes a cap, in each of its shapes, and the one that changes it', async ({ page }) => {
  await open(page, `/usage?scope=node:${PLACES.web}`);
  await page.getByRole('region', { name: 'Limits' }).getByRole('button', { name: 'Add limit…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add a limit' });
  await expect(dialog).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await expect(dialog.getByLabel('Warn at (percent)')).toBeVisible();
  await expect(dialog.getByRole('radiogroup', { name: 'At the limit', exact: true })).toBeVisible();
  await dialog.getByLabel('Amount').fill('0.5');
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(dialog.getByText('Tokens are a whole number; 500k and 2M are understood.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await dialog.getByLabel('Amount').fill('2M');
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first()).toBeVisible();
  await page.getByRole('region', { name: 'Limits' }).getByRole('button', { name: /Actions for the limit/ }).click();
  await page.getByRole('menuitem', { name: 'Change…' }).click();
  await expect(page.getByRole('dialog', { name: 'Change a limit' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a provider\'s usage and its limits', async ({ page }) => {
  await makeCap(PEOPLE.ada, { provider_id: RUNS_ON.openAiKey, meter: 'tokens', amount: '5000000', models: ['@frontier'] });
  await open(page, `/resources/providers/${RUNS_ON.openAiKey}?tab=usage`);
  await expect(page.getByRole('list', { name: 'Figures of the span' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('tab', { name: 'Limits' }).click();
  await expect(page.getByRole('region', { name: 'Limits' }).getByRole('listitem')).toHaveCount(1);
  await expectAccessibleInBothThemes(page);
});

test('one\'s own page of usage and limits, and the board on an overview', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: ACME.org, meter: 'credits', amount: '500', period: 'month', split: 'each' });
  await open(page, '/settings/usage', PEOPLE.dora);
  await expect(page.getByRole('region', { name: 'Limits' }).getByRole('listitem')).toHaveCount(1);
  await expectAccessibleInBothThemes(page);
  await page.goto(`/project/${PLACES.web}`);
  await expect(page.getByRole('region', { name: 'Usage' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto('/org/acme');
  await expect(page.getByRole('region', { name: 'Usage' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the rates of an org, the form of a rate with its faults, and the catalogue', async ({ page }) => {
  await open(page, `/usage?scope=node:${PLACES.org}`);
  await page.getByRole('button', { name: 'See the rates' }).click();
  const rates = page.getByRole('dialog', { name: 'Rates · Acme' });
  await expect(rates.getByRole('table', { name: 'Rates of the models' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await rates.getByRole('row').filter({ has: page.getByText('Claude Opus 5', { exact: true }) }).getByRole('button', { name: 'Copy to the rate card' }).click();
  const form = page.getByRole('dialog', { name: 'Copy to the rate card' });
  await form.getByRole('button', { name: 'Add a speed' }).click();
  await form.getByRole('button', { name: 'Add a step' }).click();
  await form.getByRole('button', { name: 'Save' }).click();
  await expect(form.getByText('A speed is named in lower case: fast, batch.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await form.getByRole('button', { name: 'Cancel' }).click();

  await rates.getByRole('button', { name: 'See the catalogue' }).click();
  const catalogue = page.getByRole('dialog', { name: 'The catalogue of estimates' });
  await catalogue.getByLabel('What would this model name match?').fill('claude-opus-9');
  await catalogue.getByRole('button', { name: 'Look' }).click();
  await expect(catalogue.getByText('claude-opus-9 matches "claude-*" (Anthropic).')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a thread with a cap: its chip, its popover, the prompt before a message goes past a limit, and what limits a blocked thread', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '1000000' });
  await open(page, `/thread/${PLACES.mainThread}`);
  await page.getByRole('button', { name: /^Usage/ }).click();
  await expect(page.getByText('Stops within one request')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  const box = page.getByRole('textbox', { name: 'Message to the agent' });
  await box.fill('Go on past it');
  await box.press('Enter');
  const prompt = page.getByRole('dialog', { name: 'This would go past a limit' });
  await expect(prompt).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await prompt.getByRole('button', { name: 'Cancel' }).click();

  await page.goto(`/thread/${ACME.idleThread}`);
  await page.getByRole('button', { name: /^Usage/ }).click();
  await expect(page.locator('[data-thread-usage]')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
