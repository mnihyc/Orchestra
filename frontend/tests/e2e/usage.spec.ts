/** Usage in a real browser against the fake API: one's own usage and a place's, a span changed and every figure following, a table opened and a row that narrows the screen, who is shown how much, an empty span, a cap made, reached and raised in place, a cap on a provider for a group of its models and one at the speed Fast, and a cap that is set above.
 *
 * The ledger of the fake API holds six uses a day for the last days, the same every run, so that what a
 * span comes to is counted here and not read back from the page: Ada's own two a day, Web's three, and the
 * rest (`DAY` in `usage-support.ts`).
 */
import { expect, type Page, test } from '@playwright/test';

import { ACME, dayOfYear, DAY, makeCap, passed, PEOPLE, PLACES, setCatalogue, setRunning, setWaiting, sum, useNow, writeHistory } from './usage-support.ts';
import { deny, scenario, signIn } from './support.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { showWorkbenchView } from './workbench-support.ts';

test.beforeEach(async () => {
  await scenario();
});

/** The list of figures at the head of the screen. */
const tile = (page: Page, name: 'Turns' | 'Tokens' | 'Credits') => page.getByRole('list', { name: 'Figures of the span' }).getByRole('listitem').filter({ hasText: name });

const webUsage = `/usage?scope=node:${PLACES.web}`;

test('a span is changed and every figure follows it', async ({ page }) => {
  await signIn(page);
  await page.goto('/usage');
  const month = sum(DAY.ada, 30);
  await expect(tile(page, 'Turns')).toContainText(String(month.turns));
  await expect(tile(page, 'Tokens')).toContainText('198M');
  await expect(tile(page, 'Credits')).toContainText('838.2');
  // Of the credits, the part the catalogue estimated says so, and nothing is left out of them.
  await expect(tile(page, 'Credits')).toContainText('≈ 245.7 of them estimated');
  await expect(tile(page, 'Credits')).not.toContainText('without a rate');

  await page.getByRole('radio', { name: '7 days', exact: true }).click();
  const week = sum(DAY.ada, 7);
  await expect(tile(page, 'Turns')).toContainText(String(week.turns));
  await expect(tile(page, 'Tokens')).toContainText('46.2M');
  await expect(tile(page, 'Credits')).toContainText('195.6');
  await expect(tile(page, 'Credits')).toContainText('≈ 57.3 of them estimated');
  // Against the seven days before them, which are the same.
  await expect(tile(page, 'Turns')).toContainText('The same as the 7 days before');
  await expect(page).toHaveURL(/range=7d/);

  // This year counts the days of the year so far; all time reaches back to the first turn the ledger holds.
  await page.getByRole('radio', { name: 'This year', exact: true }).click();
  const year = Math.min(dayOfYear(), 120);
  await expect(tile(page, 'Turns')).toContainText(String(DAY.ada.turns * year));
  await page.getByRole('radio', { name: 'All time', exact: true }).click();
  await expect(tile(page, 'Turns')).toContainText(String(DAY.ada.turns * 120));
  await expect(page.getByRole('radio', { name: 'All time', exact: true })).toBeChecked();
  await expect(page.getByText('Days use UTC',{exact:true})).toHaveCount(1);
});

test('a span of one\'s own is set by its two days, and by dragging across the bars', async ({ page }) => {
  await signIn(page);
  await page.goto('/usage?range=90d');
  await page.getByRole('button', { name: 'Custom…' }).click();
  const today = new Date().toISOString().slice(0, 10);
  const from = new Date(Date.now() - 4 * 86_400_000).toISOString().slice(0, 10);
  await page.getByLabel('From', { exact: true }).fill(from);
  await page.getByLabel('To, including', { exact: true }).fill(today);
  await page.getByRole('button', { name: 'Show', exact: true }).click();
  await expect(tile(page, 'Turns')).toContainText(String(DAY.ada.turns * 5));
  await expect(page).toHaveURL(new RegExp(`from=${from}`));

  // Dragging across the bars of the chart sets the span to the days they cover.
  await page.goto('/usage?range=30d');
  await expect(tile(page, 'Turns')).toContainText(String(DAY.ada.turns * 30));
  await page.waitForLoadState('networkidle');
  const plot = page.locator('.u-over');
  await expect(plot).toBeVisible();
  await plot.scrollIntoViewIfNeeded();
  const box = await plot.boundingBox();
  if (box === null) throw new Error('the chart has no plot');
  await page.mouse.move(box.x + box.width * 0.55, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.85, box.y + box.height / 2, { steps: 6 });
  await page.mouse.up();
  await expect(page).toHaveURL(/from=\d{4}-\d{2}-\d{2}/);
  const kept = new URL(page.url()).searchParams;
  const days = Math.round((Date.parse(`${kept.get('to')}T00:00:00Z`) - Date.parse(`${kept.get('from')}T00:00:00Z`)) / 86_400_000) + 1;
  expect(days).toBeGreaterThan(3);
  expect(days).toBeLessThan(15);
  await expect(tile(page, 'Turns')).toContainText(String(DAY.ada.turns * days));
});

test('a break-down is opened into its table, and a row narrows the whole screen', async ({ page }) => {
  await signIn(page);
  await page.goto('/usage');
  await page.getByRole('tab', { name: 'By model' }).click();
  const models = page.getByRole('table', { name: 'By model' });
  await expect(models.getByRole('row')).toHaveCount(3); // the header and Ada's two models
  await expect(models.getByRole('button', { name: /^GPT-5\.4/ })).toBeVisible();
  await expect(models.getByRole('button', { name: /^Claude Opus 5/ })).toBeVisible();

  await models.getByRole('button', { name: /^GPT-5\.4/ }).click();
  // The chip says what the screen is narrowed to, the figures are those of that model alone, and so are the records.
  await expect(page.getByRole('group', { name: 'What the screen is narrowed to' })).toContainText('Model: GPT-5.4');
  await expect(tile(page, 'Turns')).toContainText('30');
  await expect(page).toHaveURL(/model=gpt-5.4/);
  const records = page.getByRole('table', { name: 'Records' });
  await expect(records.getByRole('row').nth(1)).toContainText('GPT-5.4');
  await expect(records.getByText('Claude Opus 5')).toHaveCount(0);

  await page.getByRole('button', { name: 'Remove Model filter: GPT-5.4' }).click();
  await expect(tile(page, 'Turns')).toContainText('60');
  await expect(page.getByRole('group', { name: 'What the screen is narrowed to' })).toHaveCount(0);
});

test('a record is followed to its thread and says what it was counted at, and where that came from', async ({ page }) => {
  await signIn(page);
  await page.goto('/usage?range=7d');
  const records = page.getByRole('table', { name: 'Records' });
  // A use of a model whose rate is an estimate: its credits carry "≈" and say so when asked.
  const estimate = records.getByRole('row').filter({ hasText: 'Claude Opus 5' }).first();
  await expect(estimate).toContainText('≈');
  await estimate.getByRole('button', { name: 'More about this record' }).click();
  await expect(page.getByRole('dialog', { name: 'More about this record' })).toContainText('An estimate for this model\'s family, from the shipped catalogue');
  await page.keyboard.press('Escape');
  // A use at the provider's own rate says whose it is.
  const own = records.getByRole('row').filter({ hasText: 'GPT-5.4' }).first();
  await own.getByRole('button', { name: 'More about this record' }).click();
  await expect(page.getByRole('dialog', { name: 'More about this record' })).toContainText('From OpenAI key\'s own rate');
  await expect(page.getByRole('dialog', { name: 'More about this record' })).toContainText('2.50 / 0.25 / 15 credits per million tokens');
  await page.keyboard.press('Escape');
  // Its thread opens.
  await records.getByRole('link', { name: 'Main thread' }).first().click();
  await expect(page).toHaveURL(/\/thread\//);
});

test('what was used without any rate reads "–" and is counted beside a sum that says so', async ({ page }) => {
  await signIn(page);
  await page.goto(`/usage?scope=node:${PLACES.evals}`);
  const credits = tile(page, 'Credits');
  // Evals holds Dora's small model (an estimate) and Grace's own (no rate at all): 600 000 tokens of the second add nothing to the sum.
  await expect(credits).toContainText('18M tokens without a rate');
  await expect(credits).toContainText('≈ 18');
  await page.getByRole('tab', { name: 'By model' }).click();
  const table = page.getByRole('table', { name: 'By model' });
  const local = table.getByRole('row').filter({ hasText: 'our-local-model' });
  await expect(local).toContainText('–');
  await expect(local).toContainText('18M tokens without a rate');
  // Whoever reads the reports of a place is shown the rates it counts at: here, in a project, as they stand, to be written for the org.
  await credits.getByRole('button', { name: 'Add rates' }).click();
  await expect(page.getByRole('dialog', { name: 'Rates · Evals' })).toBeVisible();
});

test('nobody is offered the rates of a place whose reports they may not read', async ({ page }) => {
  await signIn(page, PEOPLE.dora);
  await page.goto(`/usage?scope=node:${PLACES.web}`);
  await expect(tile(page, 'Turns')).toBeVisible();
  await expect(page.getByRole('button', { name: 'See the rates' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Add rates' })).toHaveCount(0);
});

test('the figures of a place are as deep as its page lets the reader see', async ({ page }) => {
  // Ada administers Acme and reads every record and every thread.
  await signIn(page);
  await page.goto(webUsage);
  const web = sum(DAY.web, 30);
  await expect(tile(page, 'Turns')).toContainText(String(web.turns));
  await expect(tile(page, 'Tokens')).toContainText('324M');
  await expect(page.getByText('as its page lets you see them')).toHaveCount(0);
  await expect(page.getByRole('tab', { name: 'By thread' })).toBeVisible();
  const people = page.getByRole('table', { name: 'By person' });
  await expect(people.getByRole('row')).toHaveCount(4);
  await expect(page.getByRole('table', { name: 'Records' }).getByRole('row').nth(1)).toBeVisible();
});

test('a viewer reads the figures of a place and nobody else\'s records, and no table by thread', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.goto(webUsage);
  await expect(page.getByText('Summary figures',{exact:true})).toBeVisible();
  await expect(tile(page, 'Turns')).toContainText(String(sum(DAY.web, 30).turns));
  await expect(page.getByRole('tab', { name: 'By person' })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'By thread' })).toHaveCount(0);
  await expect(page.getByText('Only your own records are listed here.')).toBeVisible();
  await expect(page.getByText('No records in this span.')).toBeVisible();
  // Nothing of the Limits card is there to change.
  await expect(page.getByRole('button', { name: 'Add limit…' })).toHaveCount(0);
});

test('somebody the place shows no figures to sees their own usage there, and nothing of anybody else\'s', async ({ page }) => {
  await signIn(page, PEOPLE.dora);
  await page.goto(webUsage);
  await expect(page.getByText('Summary figures',{exact:true})).toBeVisible();
  await deny('dora', ['usage.summary']);
  await page.reload();
  await expect(page.getByText('Your own usage only',{exact:true})).toBeVisible();
  // Dora's own use in Web is one a day; Grace's and Ada's are not counted for her.
  await expect(tile(page, 'Turns')).toContainText(String(sum(DAY.doraInWeb, 30).turns));
  const people = page.getByRole('table', { name: 'By person' });
  await expect(people.getByRole('row')).toHaveCount(2);
  await expect(people).toContainText('Dora Developer');
  await expect(people).not.toContainText('Ada Lovelace');
  // Everyone has their own usage everywhere, whatever a place shows of others.
  await page.goto('/usage');
  await expect(tile(page, 'Turns')).toContainText(String(sum(DAY.dora, 30).turns));
});

test('an empty span says so and draws no broken chart', async ({ page }) => {
  await signIn(page, PEOPLE.vera);
  await page.goto('/usage');
  await expect(page.getByText('Nothing was used in this span.')).toBeVisible();
  await expect(tile(page, 'Turns')).toContainText('0');
  await expect(page.locator('canvas')).toHaveCount(0);
  await expect(page.getByRole('tab', { name: 'By model' })).toBeVisible();
  await expect(page.getByRole('table', { name: 'Records' })).toHaveCount(0);
  await expect(page.getByText('Orch records what was used')).toBeVisible();
});

test('a span in which somebody who uses things did nothing says the same', async ({ page }) => {
  await signIn(page);
  await page.goto('/usage?from=2020-01-01&to=2020-01-31');
  await expect(page.getByText('Nothing was used in this span.')).toBeVisible();
  await expect(page.locator('canvas')).toHaveCount(0);
});

test('the usage board on a project\'s overview, in a period, with the reader among its people', async ({ page }) => {
  // The people breakdown belongs to a report reader; a person's own-only view names no other users.
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${PLACES.web}`);
  const board = page.getByRole('region', { name: 'Usage' });
  await expect(board).toBeVisible();
  await expect(board.getByRole('link', { name: 'Open usage', exact: true })).toContainText('This week');
  await expect(board).toContainText(/\d+ turns/);
  await board.getByRole('link', { name: 'Open usage' }).click();
  await expect(page).toHaveURL(/\/usage\?/);
  await expect(page).toHaveURL(new RegExp(`scope=node(:|%3A)${PLACES.web}`));
  // The overview's one-line board opens the period and people controls in the full report.
  await page.getByRole('group', { name: 'Filters and actions', exact: true }).getByRole('button', { pressed: true }).click();
  const today = new Date();
  const to = today.toISOString().slice(0, 10);
  await page.getByLabel('From', { exact: true }).fill(`${to.slice(0, 8)}01`);
  await page.getByLabel('To, including', { exact: true }).fill(to);
  await page.getByRole('button', { name: 'Show', exact: true }).click();
  await expect(tile(page, 'Turns')).toContainText(String(DAY.web.turns * today.getUTCDate()));
  await page.getByRole('tab', { name: 'By person', exact: true }).click();
  await expect(page.getByRole('table', { name: 'By person', exact: true })).toContainText('Ada Lovelace');
});

test('a cap is made on a place, is reached by what was used, is raised in its row and is shown reached no more', async ({ page }) => {
  await signIn(page);
  await page.goto(webUsage);
  const limits = page.getByRole('region', { name: 'Limits' });
  await expect(limits.getByText('No limit is set here')).toBeVisible();
  await limits.getByRole('button', { name: 'Add limit…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add a limit' });
  // The sentence says what is being made as it is filled in.
  await expect(dialog.getByText('Everyone together may use at most … tokens a day on all models in project Web.')).toBeVisible();
  await dialog.getByLabel('Amount').fill('1M');
  await expect(dialog.getByText('Everyone together may use at most 1M tokens a day on all models in project Web.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(page.getByRole('region', { name: 'Notifications' }).getByText('Limit added')).toBeVisible();

  // Today's use in Web is 10.8M tokens, which is more than the cap: it is reached, and says when it starts again.
  const row = limits.getByRole('listitem').first();
  await expect(row).toContainText('Everyone together on all models in project Web');
  await expect(row.getByText('Reached', { exact: true })).toBeVisible();
  await expect(row).toContainText('10.8M of 1M tokens');
  await expect(row).toContainText('starts again 00:00 UTC');
  await expect(row).toContainText('Set by Ada Lovelace');
  await expect(row.getByRole('meter')).toHaveAttribute('aria-valuenow', '100');

  // Raised where it stands: no longer reached.
  await row.getByRole('button', { name: 'Change amount' }).click();
  await row.getByLabel('Amount').fill('20M');
  await row.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByText('Reached', { exact: true })).toHaveCount(0);
  await expect(row).toContainText('10.8M of 20M tokens');
  await expect(row.getByRole('meter')).toHaveAttribute('aria-valuenow', '54');
});

test('a warning comes before the wall, and what is used afterwards is counted by the cap at once', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '12000000', warn_percent: 80 });
  await signIn(page);
  await page.goto(webUsage);
  const row = page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first();
  await expect(row.getByText('Nearly reached')).toBeVisible();
  await expect(row).toContainText('10.8M of 12M tokens');
  await useNow('A');
  await page.reload();
  await expect(row.getByText('Reached', { exact: true })).toBeVisible();
  await expect(row).toContainText('15.6M of 12M tokens');
});

test('a cap is switched off in its row and brought back, and removed after it is asked once', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '1000000' });
  await signIn(page);
  await page.goto(webUsage);
  const limits = page.getByRole('region', { name: 'Limits' });
  const row = limits.getByRole('listitem').first();
  await expect(row.getByText('Reached', { exact: true })).toBeVisible();
  await row.getByRole('switch', { name: 'In force' }).click();
  await expect(row.getByText('Switched off')).toBeVisible();
  await expect(row.getByText('Reached', { exact: true })).toHaveCount(0);
  await row.getByRole('switch', { name: 'In force' }).click();
  await expect(row.getByText('Reached', { exact: true })).toBeVisible();

  await row.getByRole('button', { name: /Actions for the limit/ }).click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  const confirm = page.getByRole('alertdialog').or(page.getByRole('dialog', { name: 'Delete this limit?' }));
  await expect(confirm).toContainText('Everyone together may use at most 1M tokens a day on all models in project Web.');
  await confirm.getByRole('button', { name: 'Delete limit' }).click();
  await expect(limits.getByText('No limit is set here')).toBeVisible();
});

test('a cap that was set above is shown as such, with where, and is read only here', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: ACME.org, meter: 'tokens', amount: '50000000', period: 'week' });
  await signIn(page);
  await page.goto(webUsage);
  const row = page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first();
  await expect(row).toContainText('Everyone together on all models in Acme');
  await expect(row.getByText('Inherited')).toBeVisible();
  await expect(row.getByRole('link', { name: 'Set on Acme' })).toBeVisible();
  // Nothing in the row changes it: that is done where it is set.
  await expect(row.getByRole('switch')).toHaveCount(0);
  await expect(row.getByRole('button', { name: 'Change amount' })).toHaveCount(0);
  await expect(row.getByRole('button', { name: /Actions for the limit/ })).toHaveCount(0);
  await row.getByRole('link', { name: 'Set on Acme' }).click();
  await expect(page).toHaveURL(new RegExp(`scope=node(:|%3A)${ACME.org}`));
  const own = page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first();
  await expect(own.getByText('Inherited')).toHaveCount(0);
  await expect(own.getByRole('button', { name: 'Change amount' })).toBeVisible();
});

test('a cap on a provider names a group of its models, and another the speed Fast, each read back in words', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=usage`);
  const limits = page.getByRole('region', { name: 'Limits' });
  await expect(limits.getByText('No limit is set here')).toBeVisible();

  await limits.getByRole('button', { name: 'Add limit…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add a limit' });
  await dialog.getByLabel('On which models').fill('front');
  await page.getByRole('option', { name: /the frontier group of models/ }).click();
  await dialog.getByLabel('Amount').fill('5M');
  await expect(dialog.getByText('Everyone together may use at most 5M tokens a day on the frontier group of models on OpenAI key.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(limits.getByRole('listitem').first()).toContainText('Everyone together on the frontier group of models on OpenAI key');

  await limits.getByRole('button', { name: 'Add limit…' }).click();
  const second = page.getByRole('dialog', { name: 'Add a limit' });
  await expect(second.getByRole('checkbox', { name: 'Fast' })).toBeVisible();
  await second.getByRole('checkbox', { name: 'Fast' }).click();
  await second.getByLabel('Amount').fill('1M');
  await expect(second.getByText('Everyone together may use at most 1M tokens a day on all models at the speed Fast on OpenAI key.')).toBeVisible();
  await second.getByRole('button', { name: 'Add limit' }).click();
  await expect(limits.getByRole('listitem').filter({ hasText: 'at the speed Fast' })).toContainText('Everyone together on all models at the speed Fast on OpenAI key');

  // The same cap is refused a second time, in words, and nothing is made twice.
  await limits.getByRole('button', { name: 'Add limit…' }).click();
  const third = page.getByRole('dialog', { name: 'Add a limit' });
  await expect(third.getByRole('checkbox', { name: 'Fast' })).toBeVisible();
  await third.getByRole('checkbox', { name: 'Fast' }).click();
  await third.getByLabel('Amount').fill('3M');
  await third.getByRole('button', { name: 'Add limit' }).click();
  await expect(third.getByText('A limit like this one exists already. Change that one instead.')).toBeVisible();
  await third.getByRole('button', { name: 'Cancel' }).click();
  await expect(limits.getByRole('listitem')).toHaveCount(2);
});

test('a cap for one person, for a group and for a role is made as each is named', async ({ page }) => {
  await signIn(page);
  await page.goto(webUsage);
  const limits = page.getByRole('region', { name: 'Limits' });

  await limits.getByRole('button', { name: 'Add limit…' }).click();
  let dialog = page.getByRole('dialog', { name: 'Add a limit' });
  await dialog.getByRole('combobox', { name: 'Whom it covers' }).click();
  await page.getByRole('option', { name: 'Each person with a role…' }).click();
  await dialog.getByRole('combobox', { name: 'Person, group or role', exact: true }).fill('Developers');
  await page.getByRole('option', { name: 'Developers of Acme > Research > Web', exact: true }).click();
  await dialog.getByLabel('Amount').fill('300');
  await dialog.getByRole('combobox', { name: 'What it counts' }).click();
  await page.getByRole('option', { name: 'credits' }).click();
  await dialog.getByRole('combobox', { name: 'Period' }).click();
  await page.getByRole('option', { name: 'a week' }).click();
  await expect(dialog.getByRole('status')).toHaveText('It says: Developers of Acme > Research > Web may use at most 300 credits a week on all models in project Web.');
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(limits.getByRole('listitem').first()).toContainText('Developers of Web on all models in project Web');

  await limits.getByRole('button', { name: 'Add limit…' }).click();
  dialog = page.getByRole('dialog', { name: 'Add a limit' });
  await dialog.getByRole('combobox', { name: 'Whom it covers' }).click();
  await page.getByRole('option', { name: 'Each member of a group…' }).click();
  await dialog.getByRole('combobox', { name: 'Person, group or role', exact: true }).fill('Operators');
  await page.getByRole('option', { name: /Operators/ }).click();
  await dialog.getByRole('combobox', { name: 'What it counts' }).click();
  await page.getByRole('option', { name: 'turns at once' }).click();
  await dialog.getByLabel('Amount').fill('1');
  await expect(dialog.getByRole('status')).toHaveText('It says: Each member of Operators, Acme may run at most 1 turn at once on all models in project Web.');
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(limits.getByRole('listitem').filter({ hasText: 'Each member of Operators' })).toBeVisible();

  await limits.getByRole('button', { name: 'Add limit…' }).click();
  dialog = page.getByRole('dialog', { name: 'Add a limit' });
  await dialog.getByRole('combobox', { name: 'Whom it covers' }).click();
  await page.getByRole('option', { name: 'One person…' }).click();
  await dialog.getByRole('combobox', { name: 'Person, group or role', exact: true }).fill('Gra');
  await page.getByRole('option', { name: /Grace Hopper/ }).click();
  await dialog.getByLabel('No limit').check();
  await expect(dialog.getByRole('status')).toHaveText('It says: Grace Hopper · acme.grace has no limit on all models in project Web.');
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(limits.getByRole('listitem').filter({ hasText: 'Grace Hopper on all models' })).toContainText('no limit');
});

test('a form that is not complete says what is wrong where it is, and sends nothing', async ({ page }) => {
  await signIn(page);
  await page.goto(webUsage);
  await page.getByRole('region', { name: 'Limits' }).getByRole('button', { name: 'Add limit…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add a limit' });
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(dialog.getByText('Write an amount, or tick No limit.')).toBeVisible();
  await dialog.getByLabel('Amount').fill('0.5');
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(dialog.getByText('Tokens are a whole number; 500k and 2M are understood.')).toBeVisible();
  await dialog.getByLabel('Amount').fill('2M');
  await expect(dialog.getByLabel('Warn at (percent)')).toBeVisible();
  await dialog.getByLabel('Warn at (percent)').fill('150');
  await dialog.getByRole('button', { name: 'Add limit' }).click();
  await expect(dialog.getByText('A percent from 1 to 99.')).toBeVisible();
  // Nothing was sent: the card behind the form still says that no limit is set.
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByRole('region', { name: 'Limits' }).getByText('No limit is set here')).toBeVisible();
});

test('a cap on turns at once says how many run and how many wait, and lists what waits in the order it will run', async ({ page }) => {
  const cap = await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'running', amount: '2' });
  await setRunning([
    { thread: PLACES.mainThread, user: 'ada', provider: RUNS_ON.openAiKey },
    { thread: ACME.repro, user: 'grace', provider: RUNS_ON.openAiKey },
  ]);
  await setWaiting(cap.id, [
    { thread: ACME.depsThread, author: 'dora', since: new Date(Date.now() - 5 * 60_000).toISOString(), mine: false },
    { thread: null, author: null, since: new Date(Date.now() - 2 * 60_000).toISOString(), mine: false },
  ]);
  await signIn(page);
  await page.goto(webUsage);
  const row = page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first();
  await expect(row).toContainText('Everyone together on all models in project Web');
  await expect(row).toContainText('2 of 2 running');
  await expect(row).toContainText('at most 2 turns at once');
  await expect(row.getByText('Reached', { exact: true })).toBeVisible();
  await row.getByRole('button', { name: '2 waiting' }).click();
  const queue = row.getByRole('list', { name: 'Waiting for room, in the order they will run' });
  await expect(queue.getByRole('listitem')).toHaveCount(2);
  await expect(queue.getByRole('listitem').first()).toContainText('Main thread');
  await expect(queue.getByRole('listitem').first()).toContainText('Dora Developer');
  await expect(queue.getByRole('listitem').nth(1)).toContainText('A thread you cannot see');
});

test('a cap that was passed says so on its row, and by whom', async ({ page }) => {
  const cap = await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '1000000' });
  await passed(cap.id, 'ada', 3);
  await signIn(page);
  await page.goto(webUsage);
  const row = page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first();
  await row.getByRole('button', { name: 'let past 3 times' }).click();
  await expect(page.getByRole('dialog', { name: 'Who let turns past this limit' })).toContainText('Ada Lovelace');
});

test('what a cap leaves out for want of a rate is said, with the way to add one for whoever may', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: PLACES.evals, meter: 'credits', amount: '1000' });
  await signIn(page);
  await page.goto(`/usage?scope=node:${PLACES.evals}`);
  const row = page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first();
  // Counted by the day, it leaves out what the day has had so far of the model nobody rated.
  await expect(row).toContainText('600k tokens on models without a rate are not counted');
});

test('a person reads the share of each allowance that counts them, and their usage this month, on their own page', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: ACME.org, meter: 'credits', amount: '500', period: 'month', split: 'each' });
  await signIn(page, PEOPLE.dora);
  await page.goto('/settings/usage');
  await expect(page).toHaveURL(/\/usage(?:\?|$)/);
  await expect(page.getByRole('heading', { name: 'Your usage', level: 1 })).toBeVisible();
  const allowances = page.getByRole('region', { name: 'Limits' });
  await expect(allowances.getByRole('listitem').first()).toContainText('Each person on all models in Acme');
  await expect(allowances.getByRole('listitem').first()).toContainText('credits');
  await expect(allowances.getByRole('listitem').first()).toContainText('starts again');
  // The former settings address now opens the real report, including her allowances.
  await expect(page.getByRole('list', { name: 'Figures of the span' })).toBeVisible();
});

test('a provider\'s usage counts every thread that ran on it, and says who used how much', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=usage`);
  // The key of Ada's laptop ran A and B (GPT-5.4), D and F: four of the six uses of every day.
  await expect(tile(page, 'Turns')).toContainText(String(4 * 30));
  await page.getByRole('tab', { name: 'By person' }).click();
  await expect(page.getByRole('table', { name: 'By person' })).toContainText('Grace Hopper');
  await expect(page.getByRole('table', { name: 'By person' })).toContainText('Dora Developer');
});

/** The row of a model in the table of rates: the one whose name is exactly that. */
const rateRow = (page: Page, model: string) =>
  page
    .getByRole('dialog', { name: /^Rates · / })
    .getByRole('table', { name: 'Rates of the models' })
    .getByRole('row')
    .filter({ has: page.getByText(model, { exact: true }) });

test('the rates of an org say where each comes from; an estimate is copied to the rate card and changed there, and a model nobody rated is added', async ({ page }) => {
  // This editor fixture has exactly thirty daily uses, independent of its rolling cutoff at UTC midnight.
  await writeHistory(30);
  await signIn(page);
  await page.goto(`/usage?scope=node:${PLACES.org}`);
  await page.getByRole('button', { name: 'See the rates' }).click();
  const dialog = page.getByRole('dialog', { name: 'Rates · Acme' });
  const table = dialog.getByRole('table', { name: 'Rates of the models' });

  // The model that was used and nobody rated is the first row, with how much of it; the others say where their rate comes from.
  await expect(table.getByRole('row').nth(1)).toContainText('our-local-model');
  await expect(table.getByRole('row').nth(1)).toContainText('No rate');
  // Thirty daily uses of the fixture's unrated model, 600k tokens each.
  await expect(table.getByRole('row').nth(1)).toContainText('18M tokens used');
  await expect(rateRow(page, 'GPT-5.4')).toContainText("The provider's own");
  await expect(rateRow(page, 'GPT-5.4')).toContainText('2.5');
  await expect(rateRow(page, 'Claude Opus 5')).toContainText('≈ Estimate · "claude-*"');
  await expect(rateRow(page, 'Claude Opus 5')).toContainText('3');
  await expect(dialog.getByText('Catalogue from Oct 1, 2026 (shipped with this release)', { exact: true })).toBeVisible();

  // Copying an estimate makes a row of the card from its figures; from then on the family follows the card.
  await rateRow(page, 'Claude Opus 5').getByRole('button', { name: 'Copy to the rate card' }).click();
  const form = page.getByRole('dialog', { name: 'Copy to the rate card' });
  await expect(form.getByLabel('Model')).toHaveValue('claude-*');
  await expect(form.getByLabel('Input', { exact: true })).toHaveValue('3');
  await form.getByLabel('Output', { exact: true }).fill('30');
  await form.getByRole('button', { name: 'Save' }).click();
  await expect(form).toHaveCount(0);
  await expect(rateRow(page, 'Claude Opus 5')).toContainText('Rate card · "claude-*"');
  await expect(rateRow(page, 'Claude Sonnet 5')).toContainText('Rate card · "claude-*"');
  await expect(rateRow(page, 'Claude Opus 5')).toContainText('30');

  // A row of the card is changed in the same form.
  await rateRow(page, 'Claude Sonnet 5').getByRole('button', { name: 'Edit' }).click();
  const edit = page.getByRole('dialog', { name: 'Edit a rate' });
  await expect(edit.getByLabel('Output', { exact: true })).toHaveValue('30');
  await edit.getByLabel('Input', { exact: true }).fill('4');
  await edit.getByRole('button', { name: 'Save' }).click();
  await expect(rateRow(page, 'Claude Sonnet 5')).toContainText('4');

  // A model with no rate is added: what is wrong is said beside its field, and nothing is sent until it is mended.
  await table.getByRole('row').nth(1).getByRole('button', { name: 'Add a rate' }).click();
  const add = page.getByRole('dialog', { name: 'Add a rate' });
  await expect(add.getByLabel('Model')).toHaveValue('our-local-model');
  await add.getByRole('button', { name: 'Save' }).click();
  await expect(add.getByText('A number with at most 6 places after the point.')).toHaveCount(3);
  await add.getByLabel('Input', { exact: true }).fill('1');
  await add.getByLabel('Cached', { exact: true }).fill('0.1');
  await add.getByLabel('Output', { exact: true }).fill('5');
  await add.getByRole('button', { name: 'Add a speed' }).click();
  await add.getByLabel('Speed', { exact: true }).fill('fast');
  await add.getByLabel('Multiple').fill('2');
  await add.getByRole('button', { name: 'Save' }).click();
  await expect(add).toHaveCount(0);
  await expect(rateRow(page, 'our-local-model')).toContainText('Rate card · "our-local-model"');
  await expect(rateRow(page, 'our-local-model')).toContainText('fast ×2');
  await expect(rateRow(page, 'our-local-model')).not.toContainText('tokens used');

  // A row taken off the card leaves the model with the next rate in the order: here, the estimate.
  await rateRow(page, 'Claude Opus 5').getByRole('button', { name: 'Edit' }).click();
  await page.getByRole('dialog', { name: 'Edit a rate' }).getByRole('button', { name: 'Remove from the rate card' }).click();
  await page.getByRole('alertdialog', { name: 'Remove claude-* from the rate card?' }).getByRole('button', { name: 'Remove claude-*' }).click();
  await expect(rateRow(page, 'Claude Opus 5')).toContainText('≈ Estimate · "claude-*"');
});

test('estimates are switched off and back, and what one unit of the catalogue is worth is changed with the estimates following it', async ({ page }) => {
  await signIn(page);
  await page.goto(`/usage?scope=node:${PLACES.org}`);
  await page.getByRole('button', { name: 'See the rates' }).click();
  const dialog = page.getByRole('dialog', { name: 'Rates · Acme' });
  await expect(rateRow(page, 'GPT-5.4 Chat')).toContainText('1.25');

  await dialog.getByLabel('One unit of the catalogue is worth (credits)').fill('2');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(rateRow(page, 'GPT-5.4 Chat')).toContainText('2.5');
  await expect(rateRow(page, 'GPT-5.4 Chat')).toContainText('"gpt-5*"');
  // The provider's own rate is no estimate and does not follow it.
  await expect(rateRow(page, 'GPT-5.4')).toContainText('2.5');

  await dialog.getByRole('switch', { name: 'Estimates' }).click();
  await expect(dialog.getByText('Use of a model with no rate is counted in tokens alone and reads – in credits.')).toBeVisible();
  await expect(rateRow(page, 'GPT-5.4 Chat')).toContainText('No rate');
  await expect(rateRow(page, 'GPT-5.4 Chat')).not.toContainText('2.5');
  await dialog.getByRole('switch', { name: 'Estimates' }).click();
  await expect(rateRow(page, 'GPT-5.4 Chat')).toContainText('"gpt-5*"');
});

test('the catalogue is read as it is, and says which family a model that is not in use yet would match', async ({ page }) => {
  await signIn(page);
  await page.goto(`/usage?scope=node:${PLACES.org}`);
  await page.getByRole('button', { name: 'See the rates' }).click();
  await page.getByRole('dialog', { name: 'Rates · Acme' }).getByRole('button', { name: 'See the catalogue' }).click();
  const catalogue = page.getByRole('dialog', { name: 'The catalogue of estimates' });
  const families = catalogue.getByRole('table', { name: 'Families of the catalogue' });
  await expect(families.getByRole('row')).toHaveCount(5); // the header and four families
  await expect(families).toContainText('claude-*');
  await expect(families).toContainText('Prompts over 272k tokens are priced higher.');
  await catalogue.getByLabel('What would this model name match?').fill('Claude-Opus-9');
  await catalogue.getByRole('button', { name: 'Look' }).click();
  await expect(catalogue.getByText('claude-opus-9 matches "claude-*" (Anthropic).')).toBeVisible();
  await catalogue.getByLabel('What would this model name match?').fill('our-model');
  await catalogue.getByRole('button', { name: 'Look' }).click();
  await expect(catalogue.getByText(/No family matches our-model/)).toBeVisible();
});

test('the rates inside a project are read as they stand and are written for the org', async ({ page }) => {
  await signIn(page);
  await page.goto(`/usage?scope=node:${PLACES.web}`);
  await page.getByRole('button', { name: 'See the rates' }).click();
  const dialog = page.getByRole('dialog', { name: 'Rates · Web' });
  await expect(rateRow(page, 'Claude Opus 5')).toContainText('≈ Estimate · "claude-*"');
  await expect(dialog.getByText('Rates are written for an org as a whole, and count in everything inside it.')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Copy to the rate card' })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Add a rate' })).toHaveCount(0);
  await expect(dialog.getByRole('switch', { name: 'Estimates' })).toBeDisabled();
});

test('credits without a rate offer to add one, and the rate is counted from then on', async ({ page }) => {
  await signIn(page);
  await page.goto(`/usage?scope=node:${PLACES.org}`);
  // Of the credits of the span, the part that has no rate says so, with the way to give it one.
  await expect(tile(page, 'Credits')).toContainText('without a rate');
  await tile(page, 'Credits').getByRole('button', { name: 'Add rates' }).click();
  await expect(page.getByRole('dialog', { name: 'Rates · Acme' })).toBeVisible();
});

test('a thread opens its one Usage view with its recorded total and the limits that cover it', async ({ page }) => {
  // Web has used 10.8M tokens today; a cap of 12M is nine tenths used.
  await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '12000000' });
  await signIn(page);
  await page.goto(`/thread/${PLACES.mainThread}`);
  await showWorkbenchView(page, 'Usage');
  const usage = page.locator('[data-thread-usage]');
  await expect(usage.getByText('Everyone together on all models in project Web', { exact: true })).toBeVisible();
  await expect(usage.getByText('Stops within one request', { exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name:'This thread',exact:true })).toBeVisible();
  await expect(page.locator('[data-thread-usage]')).toContainText('tokens');
});

test('a message that would go past a limit says so before it is sent; whoever may let it past does, and the cap counts it', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '1000000' });
  await signIn(page);
  await page.goto(`/thread/${PLACES.mainThread}`);
  const box = page.getByRole('textbox', { name: 'Message to the agent' });
  await box.fill('Go on past it');
  await box.press('Enter');
  const prompt = page.getByRole('dialog', { name: 'A limit is reached' });
  await expect(prompt).toBeVisible();
  await expect(prompt).toContainText('Everyone together on all models in project Web');
  await expect(prompt).toContainText('You may let work past this limit.');
  await prompt.getByRole('button', { name: 'Proceed anyway' }).click();
  await expect(prompt).toHaveCount(0);
  await expect(page.getByText('Go on past it')).toBeVisible();

  // The cap says it was let past, and by whom.
  await page.goto(`/usage?scope=node:${PLACES.web}`);
  const row = page.getByRole('region', { name: 'Limits' }).getByRole('listitem').first();
  await expect(row).toContainText('let past once');
  await row.getByRole('button', { name: 'let past once' }).click();
  await expect(page.getByText('Ada Lovelace').last()).toBeVisible();
});

test('somebody who may not let a message past a limit reads who can, and sends it to wait', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: PLACES.web, meter: 'tokens', amount: '1000000' });
  await signIn(page, PEOPLE.dora);
  await page.goto(`/thread/${ACME.depsThread}`);
  const box = page.getByRole('textbox', { name: 'Message to the agent' });
  await box.fill('Wait for me');
  await box.press('Enter');
  const prompt = page.getByRole('dialog', { name: 'A limit is reached' });
  await expect(prompt).toContainText('Who can let it through: Ada Lovelace');
  await expect(prompt.getByRole('button', { name: 'Proceed anyway' })).toHaveCount(0);
  await prompt.getByRole('button', { name: 'Send and wait' }).click();
  await expect(prompt).toHaveCount(0);
  await expect(page.getByText('Wait for me')).toBeVisible();
});

test('a blocked thread offers to show what limits it', async ({ page }) => {
  await makeCap(PEOPLE.ada, { node_id: ACME.org, meter: 'tokens', amount: '900000000' });
  await signIn(page);
  await page.goto(`/thread/${ACME.idleThread}`);
  await expect(page.locator('[data-thread-header]').getByText('Blocked', { exact: true })).toBeVisible();
  await page.getByRole('contentinfo', { name: 'Status' }).getByRole('button', { name: 'Orch’s limits reached', exact: true }).click();
  const card = page.locator('[data-thread-usage]');
  await expect(card).toContainText('Everyone together on all models in Acme');
  await expect(card).toContainText('Stops within one request');
});

test('a system with no catalogue in force says so, and a model with no rate of its own has no estimate', async ({ page }) => {
  await setCatalogue({ none: true });
  await signIn(page);
  const reading = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/v1/usage/catalogue' && response.request().method() === 'GET');
  await page.goto(`/usage?scope=node:${PLACES.org}`);
  await page.getByRole('button', { name: 'See the rates' }).click();
  const dialog = page.getByRole('dialog', { name: 'Rates · Acme' });
  const response = await reading;
  expect(response.status()).toBe(200);
  const catalogue: unknown = await response.json();
  expect(catalogue).toBeNull();
  await expect(dialog.getByText('No catalogue of estimates is loaded here, so use of a model with no rate has no estimate.')).toBeVisible();
  await expect(rateRow(page, 'Claude Opus 5')).toContainText('No rate');
  await expect(rateRow(page, 'GPT-5.4')).toContainText("The provider's own");
  await dialog.getByRole('button', { name: 'See the catalogue' }).click();
  const estimates = page.getByRole('dialog', { name: 'The catalogue of estimates' });
  await expect(estimates.getByText('No catalogue is in force')).toBeVisible();
  await expect(estimates.getByRole('alert')).toHaveCount(0);
  await expect(estimates.getByRole('textbox')).toHaveCount(0);
});
