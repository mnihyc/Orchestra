/** The resources screens as a table and a page are expected to be: the way to a resource in three parts and the way back, the row of kinds above every list and under no resource's name, the acts at the end of every row by pointer, menu and keyboard, rows that share acts, Edit on every page, a hint beside every option that is not plain, and a machine enrolled again. */
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { expect, type Locator, type Page, test } from '@playwright/test';

import { HINTED_FORMS, resourceHintGuide, type HintedForm, type ResourceHintId } from '../../src/features/resources/hints.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** What the fake API's resource part does that no real system can: a machine that runs the command it was given. */
async function resourceControl(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/resources${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** The words of the English catalogue, for the term each hint stands beside. */
const catalogue = JSON.parse(readFileSync(fileURLToPath(new URL('../../src/i18n/en.json', import.meta.url)), 'utf8')) as { help: { hints: Record<string, { title: string }> } };
const termOf = (id: ResourceHintId): string => catalogue.help.hints[id]?.title ?? id;

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

test.beforeEach(async () => {
  await scenario();
});

/** The row of a list that holds the link of a thing. */
const rowOf = (page: Page, table: string, name: string): Locator =>
  page.getByRole('table', { name: table }).getByRole('row').filter({ has: page.getByRole('link', { name, exact: true }) });

/** The names of the entries a menu lists, in order. */
async function entriesOf(menu: Locator): Promise<string[]> {
  const names = await menu.getByRole('menuitem').allInnerTexts();
  return names.map((name) => name.split('\n')[0] ?? '');
}

/** A notice as it is drawn, not the line that reads it to a screen reader. */
const notice = (page: Page, text: string) => page.getByText(text, { exact: true });

const KINDS = [
  { kind: 'Providers', list: '/resources/providers', thing: 'OpenAI key', search: 'OpenAI', param: 'q=OpenAI' },
  { kind: 'Agents', list: '/resources/agents', thing: 'Claude Code', search: 'Claude', param: 'q=Claude' },
  { kind: 'Machines', list: '/resources/machines', thing: 'ada-laptop', search: 'ada', param: 'q=ada' },
  { kind: 'Environments', list: '/resources/environments', thing: 'Workspace', search: 'Work', param: 'q=Work' },
  { kind: 'Templates', list: '/resources/templates', thing: 'Node 22 dev', search: 'Node', param: 'q=Node' },
  { kind: 'Hosts', list: '/resources/hosts', thing: 'host-1', search: 'host-1', param: 'q=host-1' },
] as const;

const pathOf = (page: Page) => page.getByRole('navigation', { name: 'Where this is' });
const kindsOf = (page: Page) => page.getByRole('navigation', { name: 'Kinds of resources' });

for (const entry of KINDS) {
  test(`${entry.kind}: the way to a resource is in three parts, each a link, and one press goes back to the list as it was left`, async ({ page }) => {
    await open(page, entry.list);
    // What is typed in the list stays in it when the way back is taken.
    await page.getByRole('searchbox').fill(entry.search);
    await expect(page).toHaveURL(new RegExp(entry.param));
    await page.getByRole('link', { name: entry.thing, exact: true }).click();
    await expect(page.getByRole('heading', { name: entry.thing, level: 1 })).toBeVisible();

    // Resources / Kind / Name: the first two lead somewhere, the last is where one is.
    const path = pathOf(page);
    await expect(path.getByRole('link', { name: 'Resources' })).toHaveAttribute('href', '/resources/providers');
    await expect(path.getByRole('link', { name: entry.kind })).toHaveAttribute('href', new RegExp(`^${entry.list}`));
    await expect(path.getByText(entry.thing, { exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(path.getByRole('link', { name: entry.thing })).toHaveCount(0);

    await path.getByRole('link', { name: entry.kind }).click();
    await expect(page).toHaveURL(new RegExp(entry.param));
    await expect(page.getByRole('heading', { name: entry.kind, level: 1 })).toBeVisible();
    await expect(page.getByRole('searchbox')).toHaveValue(entry.search);

    // On a narrow screen the path is folded away, and the way back is an arrow beside the name.
    await page.getByRole('link', { name: entry.thing, exact: true }).click();
    const back = page.getByRole('link', { name: `Back to ${entry.kind}` });
    await expect(back).toBeHidden();
    await page.setViewportSize({ width: 390, height: 800 });
    await expect(back).toBeVisible();
    await back.click();
    await expect(page).toHaveURL(new RegExp(entry.param));
    await expect(page.getByRole('heading', { name: entry.kind, level: 1 })).toBeVisible();
  });
}

test('the row of kinds stands above the heading of every list and is on no resource screen', async ({ page }) => {
  await signIn(page);
  for (const entry of KINDS) {
    await page.goto(entry.list);
    const heading = page.getByRole('heading', { name: entry.kind, level: 1 });
    await expect(heading).toBeVisible();
    const kinds = kindsOf(page);
    await expect(kinds).toBeVisible();
    // From top to bottom is from outside to inside: the section's row first, then the kind it opens on.
    const above = (await kinds.boundingBox())?.y ?? Number.POSITIVE_INFINITY;
    const below = (await heading.boundingBox())?.y ?? 0;
    expect(above, `the row of kinds is above the heading of ${entry.kind}`).toBeLessThan(below);

    // Every screen of a resource, every tab of it: its name, then its own tabs, and no row of kinds anywhere.
    await page.getByRole('link', { name: entry.thing, exact: true }).click();
    await expect(page.getByRole('heading', { name: entry.thing, level: 1 })).toBeVisible();
    await expect(kindsOf(page)).toHaveCount(0);
    const parts = page.getByRole('tablist', { name: /^Parts of/ });
    const labels = (await parts.getByRole('tab').allInnerTexts()).map((label) => label.split('\n')[0] ?? label);
    expect(labels.length).toBeGreaterThan(1);
    for (const label of labels) {
      await parts.getByRole('tab', { name: label }).click();
      await expect(kindsOf(page), `${entry.thing}: ${label}`).toHaveCount(0);
    }
  }
});

test('the tabs of an environment stand in the order of its screen, and its heading says the way there', async ({ page }) => {
  await open(page, `/resources/environments/${RUNS_ON.workspace}`);
  await expect(page.getByRole('heading', { name: 'Workspace', level: 1 })).toBeVisible();
  const tabs = (await page.getByRole('tablist', { name: 'Parts of the environment' }).getByRole('tab').allInnerTexts()).map((label) => label.split('\n')[0]);
  expect(tabs).toEqual(['Overview', 'Threads', 'Machine limits', 'Access', 'Settings', 'Danger zone']);
  // The status is said once: under the name, and in its chip, not in both.
  await expect(page.getByText(/threads · 3 topics · Running/)).toHaveCount(0);
});

test('a row has its act at its end, and the same menu by the menu button, the right mouse button and the keyboard', async ({ page }) => {
  await open(page, '/resources/agents');
  const row = rowOf(page, 'Agent profiles', 'Claude Code');

  // The act most used shows when the row is pointed at, and is named by what it does and to what.
  const open_ = row.getByRole('button', { name: 'Open Claude Code' });
  await expect(open_).toHaveCSS('opacity', '0');
  await row.hover();
  await expect(open_).toHaveCSS('opacity', '1');

  // The menu at its end, and the one under the right mouse button, are the same list; what destroys is last.
  await row.getByRole('button', { name: /^Actions for Claude Code(?: · .+)?$/ }).click();
  const menu = page.getByRole('menu', { name: /^Actions for Claude Code(?: · .+)?$/ });
  await expect(menu).toBeVisible();
  const byButton = await entriesOf(menu);
  expect(byButton).toEqual(['Open', 'Disable…', 'Edit…', 'Rename…', 'Share…', 'Copy link', 'Delete…']);
  await page.keyboard.press('Escape');
  await expect(menu).toBeHidden();

  await row.click({ button: 'right' });
  expect(await entriesOf(page.getByRole('menu', { name: /^Actions for Claude Code(?: · .+)?$/ }))).toEqual(byButton);
  await page.keyboard.press('Escape');

  // And with the keyboard: Shift+F10 on whatever of the row has it, and F2 for the name.
  await row.getByRole('link', { name: 'Claude Code', exact: true }).focus();
  await page.keyboard.press('Shift+F10');
  expect(await entriesOf(page.getByRole('menu', { name: /^Actions for Claude Code(?: · .+)?$/ }))).toEqual(byButton);
  await page.keyboard.press('Escape');
  await row.getByRole('link', { name: 'Claude Code', exact: true }).focus();
  await page.keyboard.press('F2');
  await expect(page.getByRole('dialog', { name: 'Rename Claude Code' })).toBeVisible();
  await page.keyboard.press('Escape');

  // An act that asks names what it is about, and its row says what became of it; the button that follows is the opposite act.
  await row.getByRole('button', { name: /^Actions for Claude Code(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Disable…' }).click();
  const ask = page.getByRole('alertdialog', { name: 'Disable Claude Code?' });
  await ask.getByRole('button', { name: 'Disable Claude Code' }).click();
  await expect(row.getByText('Switched off')).toBeVisible();
  await row.hover();
  await row.getByRole('button', { name: 'Enable Claude Code' }).click();
  await expect(row.getByText('Switched off')).toHaveCount(0);
  await expect(notice(page, 'Claude Code was enabled')).toBeVisible();
});

test('in a room that is not wide the row has the menu alone, and the act most used is the first of it', async ({ page }) => {
  await open(page, '/resources/agents');
  const row = rowOf(page, 'Agent profiles', 'Claude Code');
  await row.hover();
  await expect(row.getByRole('button', { name: 'Open Claude Code' })).toBeVisible();

  // A phone, or a window with its bars beside it: no room for a button beside the menu, and no room lost to one that is not seen.
  await page.setViewportSize({ width: 700, height: 800 });
  await expect(row.getByRole('button', { name: 'Open Claude Code' })).toHaveCount(0);
  const menu = row.getByRole('button', { name: /^Actions for Claude Code(?: · .+)?$/ });
  await expect(menu).toBeVisible();
  const table = await page.getByRole('table', { name: 'Agent profiles' }).boundingBox();
  const end = await menu.boundingBox();
  expect((end?.x ?? 0) + (end?.width ?? 0), 'the menu is inside the table').toBeLessThanOrEqual((table?.x ?? 0) + (table?.width ?? 0));
  await menu.click();
  expect((await entriesOf(page.getByRole('menu', { name: /^Actions for Claude Code(?: · .+)?$/ }))).at(0)).toBe('Open');
});

test('an act that opens something goes to the place that holds it: Copy link says that it did', async ({ page, context }) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await open(page, '/resources/templates');
  const row = rowOf(page, 'Templates', 'Node 22 dev');
  await row.getByRole('button', { name: /^Actions for Node 22 dev(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Copy link' }).click();
  await expect(notice(page, 'Link copied')).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toMatch(/\/resources\/templates\/[0-9a-f-]+$/);
});

test('a reader who may not do an act is not offered it, on the row or on the page', async ({ page }) => {
  await open(page, '/resources/providers?view=usable', PEOPLE.dora);
  const row = rowOf(page, 'Providers', 'Team Anthropic');
  await row.click({ button: 'right' });
  // She may use it: what is hers to do with it is to open it, to share what she holds of it (the page says how far) and to pass its address on.
  expect(await entriesOf(page.getByRole('menu', { name: /^Actions for Team Anthropic(?: · .+)?$/ }))).toEqual(['Open', 'Share…', 'Copy link']);
  await page.keyboard.press('Escape');
  // Nothing is theirs to do to several of them at once, so there is no box to tick one.
  await expect(page.getByRole('checkbox', { name: /^Select / })).toHaveCount(0);

  await row.getByRole('link', { name: 'Team Anthropic', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Team Anthropic', level: 1 })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Edit', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: /^Actions for Team Anthropic(?: · .+)?$/ }).click();
  expect(await entriesOf(page.getByRole('menu', { name: /^Actions for Team Anthropic(?: · .+)?$/ }))).toEqual(['Share…', 'Copy link']);
});

test('rows that are ticked have the acts they share above the table, and a destructive one names them', async ({ page }) => {
  await open(page, '/resources/agents');
  await page.getByRole('checkbox', { name: 'Select Codex', exact: true }).check();
  const bar = page.getByRole('toolbar', { name: 'Acts on the ticked rows' });
  await expect(bar.getByText('1 selected')).toBeVisible();
  await page.getByRole('checkbox', { name: 'Select Scripted agent' }).check();
  await expect(bar.getByText('2 selected')).toBeVisible();
  // Both are on, so what they share is switching off and deleting, not switching on.
  await expect(bar.getByRole('button', { name: 'Enable' })).toHaveCount(0);
  await bar.getByRole('button', { name: 'Disable…' }).click();
  const ask = page.getByRole('alertdialog', { name: 'Disable 2 profiles?' });
  await expect(ask.getByText('Codex', { exact: true })).toBeVisible();
  await expect(ask.getByText('Scripted agent', { exact: true })).toBeVisible();
  await ask.getByRole('button', { name: 'Disable 2 profiles' }).click();
  await expect(notice(page, 'Switched off 2 profiles')).toBeVisible();
  await expect(rowOf(page, 'Agent profiles', 'Codex').getByText('Switched off')).toBeVisible();
  await expect(rowOf(page, 'Agent profiles', 'Scripted agent').getByText('Switched off')).toBeVisible();
  await expect(bar).toBeHidden();

  // What is off is switched on by the same means.
  await page.getByRole('checkbox', { name: 'Select all' }).check();
  await expect(bar.getByText('4 selected')).toBeVisible();
  await expect(bar.getByRole('button', { name: 'Enable' })).toHaveCount(0);
  await page.getByRole('checkbox', { name: 'Select all' }).uncheck();
  await page.getByRole('checkbox', { name: 'Select Codex', exact: true }).check();
  await page.getByRole('checkbox', { name: 'Select Scripted agent' }).check();
  await bar.getByRole('button', { name: 'Enable' }).click();
  await expect(notice(page, 'Switched on 2 profiles')).toBeVisible();
});

test('an environment that Orch made is started from its row, and the row says it is working', async ({ page }) => {
  await open(page, '/resources/environments');
  const row = rowOf(page, 'Environments', 'Sandbox');
  await expect(row.getByText('Stopped')).toBeVisible();
  await row.hover();
  await row.getByRole('button', { name: 'Start Sandbox' }).click();
  // Within the moment it is asked: the button gives way to a sign that it is under way.
  await expect(row.getByRole('status')).toBeVisible();
  await expect(row.getByText('Running')).toBeVisible();
  await row.hover();
  await expect(row.getByRole('button', { name: 'Stop Sandbox' })).toBeVisible();
  // A directory on a person's machine has no unit to start, and its row says nothing of the kind.
  await rowOf(page, 'Environments', 'Workspace').getByRole('button', { name: /^Actions for Workspace(?: · .+)?$/ }).click();
  expect(await entriesOf(page.getByRole('menu', { name: /^Actions for Workspace(?: · .+)?$/ }))).toEqual(['Open', 'Disable…', 'Edit…', 'Rename…', 'Share…', 'Copy link', 'Delete…']);
});

test('a template is edited: its name, its description and whether it is on change; its driver is shown, read only, with why', async ({ page }) => {
  await open(page, '/resources/templates');
  await rowOf(page, 'Templates', 'Node 22 dev').getByRole('link', { name: 'Node 22 dev', exact: true }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit Node 22 dev' });

  // What cannot be changed is in the same form, as it is, with the reason beside it.
  await expect(dialog.getByRole('textbox', { name: 'Driver' })).toHaveValue('Docker (docker)');
  await expect(dialog.getByRole('textbox', { name: 'Driver' })).toHaveAttribute('readonly', '');
  await expect(dialog.getByText('The driver is chosen when a template is made.')).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: 'For', exact: true })).toHaveAttribute('readonly', '');

  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Node 22 and pnpm');
  await dialog.getByRole('textbox', { name: 'Description' }).fill('Node with pnpm, for the web team.');
  await dialog.getByRole('switch', { name: 'Enabled', exact: true }).click();
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(notice(page, 'Saved Node 22 and pnpm')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Node 22 and pnpm', level: 1 })).toBeVisible();
  await expect(page.getByText('Node with pnpm, for the web team.')).toBeVisible();
  await expect(pathOf(page).getByText('Node 22 and pnpm', { exact: true })).toBeVisible();

  // It is off now, and the page offers the opposite act at once.
  await page.getByRole('button', { name: /^Actions for Node 22 and pnpm(?: · .+)?$/ }).click();
  await expect(page.getByRole('menuitem', { name: 'Enable' })).toBeVisible();
});

test('an environment is edited: its name and its two switches change, where it runs is read only, and its settings are on their tab', async ({ page }) => {
  await open(page, `/resources/environments/${RUNS_ON.workspace}`);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit Workspace' });
  await expect(dialog.getByRole('textbox', { name: 'Runs' })).toHaveAttribute('readonly', '');
  await expect(dialog.getByText(/Where it runs is chosen when it is made/)).toBeVisible();
  await expect(dialog.getByRole('button', { name: /^Take away / })).toBeVisible();
  await expect(dialog.getByText('Moving it to another place is not offered here yet.')).toHaveCount(0);

  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Main workspace');
  // The main thread works in it, so a terminal is allowed there in the one world: the edit takes it away.
  const shell = dialog.getByRole('switch', { name: 'Allow a terminal' });
  await expect(shell).toBeChecked();
  await shell.click();
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(notice(page, 'Saved Main workspace')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Main workspace', level: 1 })).toBeVisible();
  await expect(page.getByRole('switch', { name: 'Allow a terminal' })).not.toBeChecked();

  // Its own settings are a table of their own, and the form says where.
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await page.getByRole('dialog', { name: 'Edit Main workspace' }).getByRole('button', { name: 'Open Settings' }).click();
  await expect(page.getByRole('tab', { name: 'Settings', selected: true })).toBeVisible();
});

test('Host Edit uses its settings once; the other resource editors keep their guarded fields', async ({ page }) => {
  await signIn(page);

  await page.goto('/resources/hosts');
  await page.getByRole('link', { name: 'build-host', exact: true }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'Settings', selected: true })).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('switch', { name: 'New environments' }).click();
  await expect(page.getByText('Saved', { exact: true })).toBeVisible();
  await page.getByRole('textbox', { name: 'Labels', exact: true }).fill('arm64, gpu');
  await page.getByRole('button', { name: 'Save settings', exact: true }).click();
  await expect(page.getByText('Saved', { exact: true })).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Labels', exact: true })).toHaveValue('arm64, gpu');

  await page.goto('/resources/machines');
  await page.getByRole('link', { name: 'lab-box', exact: true }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  let dialog = page.getByRole('dialog', { name: 'Edit lab-box' });
  await expect(dialog.getByRole('textbox', { name: 'System' })).toHaveAttribute('readonly', '');
  await expect(dialog.getByText('The machine reports it itself, and it changes when the machine does.')).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('lab-pc');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('heading', { name: 'lab-pc', level: 1 })).toBeVisible();

  await page.goto(`/resources/agents/${RUNS_ON.codexOnLaptop}`);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  dialog = page.getByRole('dialog', { name: 'Edit Codex on ada-laptop' });
  await expect(dialog.getByRole('textbox', { name: 'Harness' })).toHaveAttribute('readonly', '');
  await expect(dialog.getByText('The harness is chosen when a profile is made, and a profile keeps it.')).toBeVisible();
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Codex on the laptop');
  await dialog.getByRole('switch', { name: 'Switched on' }).click();
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('heading', { name: 'Codex on the laptop', level: 1 })).toBeVisible();
  await expect(page.getByText('Switched off')).toBeVisible();

  await page.goto(`/resources/providers/${RUNS_ON.openAiKey}`);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  dialog = page.getByRole('dialog', { name: 'Edit OpenAI key' });
  await expect(dialog.getByRole('textbox', { name: 'What it is' })).toHaveAttribute('readonly', '');
  await expect(dialog.getByRole('textbox', { name: 'Endpoints' })).toHaveAttribute('readonly', '');
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('OpenAI team key');
  await dialog.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('heading', { name: 'OpenAI team key', level: 1 })).toBeVisible();
});

test('somebody who changed it first is not overwritten: the form says so and reads it again', async ({ page }) => {
  await open(page, `/resources/hosts`);
  await page.getByRole('link', { name: 'build-host', exact: true }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'Settings', selected: true })).toBeVisible();
  const name = page.getByRole('textbox', { name: 'Name', exact: true });
  await name.fill('build-host-2');
  // The host is renamed from another place while this form is open.
  await page.evaluate(async (id) => {
    const login = await fetch('/api/v1/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ login: 'ada', password: 'correct horse battery' }) });
    const { access_token: token } = (await login.json()) as { access_token: string };
    const read = await fetch(`/api/v1/hosts/${id}`, { headers: { Authorization: `Bearer ${token}` } });
    const host = (await read.json()) as { revision: number };
    await fetch(`/api/v1/hosts/${id}`, { method: 'PATCH', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` }, body: JSON.stringify({ expected_revision: host.revision, labels: ['elsewhere'] }) });
  }, new URL(page.url()).pathname.split('/').at(-1));
  await page.getByRole('button', { name: 'Save settings', exact: true }).click();
  await expect(page.getByText('Somebody changed this host first', { exact: true })).toBeVisible();
  await expect(name).toHaveValue('build-host-2');
});

test('a hint opens by the pointer and by the keyboard, with its parts, and closes on Escape', async ({ page }) => {
  await open(page, `/resources/environments/${RUNS_ON.workspace}`);
  const trigger = page.getByRole('button', { name: 'Help: Allow writing from outside' });
  await trigger.click();
  const card = page.getByRole('dialog', { name: 'Allow writing from outside' });
  await expect(card).toBeVisible();
  // What it is, and which one for me; no part is there without something to say.
  await expect(card.getByRole('heading', { name: 'What this is' })).toBeVisible();
  await expect(card.getByText('Lets people change files here from the web, not only the agents that run in it.')).toBeVisible();
  await expect(card.getByRole('heading', { name: 'Which one for me' })).toBeVisible();
  await expect(card.getByRole('heading', { name: 'Where it is set' })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(card).toBeHidden();

  // By the keyboard: the "?" is reached in the order of the page, opened with Enter, and gives the focus back.
  await page.getByRole('switch', { name: 'Allow writing from outside' }).focus();
  await page.keyboard.press('Shift+Tab');
  await expect(trigger).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(card).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(card).toBeHidden();
  await expect(trigger).toBeFocused();

  // A small screen uses the same bounded card beside its hint.
  await page.setViewportSize({ width: 390, height: 800 });
  await expect(page.getByRole('button', { name: 'Help: Allow a terminal' })).toHaveCount(0);
  await trigger.click();
  await expect(page.getByRole('dialog', { name: 'Allow writing from outside' })).toBeVisible();
});

/** What no provider of the fake world can show: a type of arrangement that its providers cannot have; the component test of the arrangements draws it. */
const NOT_IN_THE_FAKE: readonly ResourceHintId[] = ['arrangement-together'];

test('standalone resource hints stay available and template fields share one guide', async ({ page }) => {
  test.setTimeout(600_000);
  // A step that cannot be done fails where it stands, and does not wait for the end of the walk.
  page.setDefaultTimeout(30_000);
  await signIn(page);
  const missing: string[] = [];
  const seen = new Map<HintedForm, Set<ResourceHintId>>();

  /** Related options share one guide destination; a dialog keeps its own entry apart from its underlying page. */
  const walk = async (form: HintedForm, steps: (has: (id: ResourceHintId) => Promise<void>) => Promise<void>) => {
    const found = new Set<ResourceHintId>();
    seen.set(form, found);
    await steps(async (id) => {
      const destination = resourceHintGuide(id);
      const dialogs = page.getByRole('dialog');
      const root = await dialogs.count() > 0 ? dialogs.last() : page.locator('body');
      const slot = root.locator(`[data-help-control="${id}"]`);
      const where = root.locator(`[data-help-article="${destination.id}"]${destination.heading === null ? '' : `[data-help-heading="${destination.heading}"]`}:visible`);
      // The form is drawn a moment after the page; a hint that is not there within that moment is not there.
      try {
        // Every declared control still contributes its own help, even while its question shares this guide.
        await slot.first().waitFor({ state: 'attached', timeout: 10_000 });
        await where.first().waitFor({ timeout: 10_000 });
      } catch {
        // Named below, with the form it was missing from.
        return;
      }
      await expect(where, `${form}: one guide for ${termOf(id)}`).toHaveCount(1);
      found.add(id);
    });
    for (const id of HINTED_FORMS[form]) if (!found.has(id) && !NOT_IN_THE_FAKE.includes(id)) missing.push(`${form}: ${termOf(id)} (${id})`);
  };

  await walk('add-provider', async (has) => {
    await page.goto('/resources/providers');
    await page.getByRole('button', { name: 'Add provider' }).click();
    const dialog = page.getByRole('dialog', { name: 'Add provider' });
    await expect(dialog.getByText('Step 1 of 3')).toBeVisible();
    await has('provider-kind');
    await dialog.getByRole('radio', { name: /Custom/ }).click();
    await dialog.getByRole('button', { name: 'Next' }).click();
    await expect(dialog.getByText('Step 2 of 3')).toBeVisible();
    await dialog.getByRole('combobox', { name: 'Format' }).click();
    await page.getByRole('option').first().click();
    await has('endpoint-format');
    await has('key-place');
    await dialog.getByLabel('Address', { exact: true }).fill('https://models.example.test/v1');
    await dialog.getByLabel('API key', { exact: true }).fill('sk-test-0000');
    await dialog.getByRole('button', { name: 'Next' }).click();
    await expect(dialog.getByText('Step 3 of 3')).toBeVisible();
    for (const id of ['home', 'who-may-use', 'arrangement-separated', 'arrangement-key_separated'] as const) await has(id);
    await page.keyboard.press('Escape');
  });

  await walk('provider-key', async (has) => {
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=key`);
    await expect(page.getByRole('tab', { name: 'Key', exact: true, selected: true })).toBeVisible();
    await has('endpoint-format');
  });

  await walk('provider-arrangements', async (has) => {
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=arrangements`);
    await expect(page.getByRole('tab', { name: 'How agents run', selected: true })).toBeVisible();
    for (const id of HINTED_FORMS['provider-arrangements']) await has(id);
  });

  await walk('provider-model', async (has) => {
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=models`);
    await page.getByRole('table', { name: /Models/ }).getByRole('button', { name: /^Actions for/ }).first().click();
    await page.getByRole('menuitem', { name: 'Edit options…' }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    for (const id of HINTED_FORMS['provider-model']) await has(id);
    await page.keyboard.press('Escape');
  });

  await walk('register-provider', async (has) => {
    await page.goto(`/resources/machines/${RUNS_ON.laptop}?tab=registered`);
    await page.getByRole('button', { name: 'Register a provider…' }).click();
    await expect(page.getByRole('dialog', { name: /Register a provider/ })).toBeVisible();
    await has('machine-provider-source');
    await has('home');
    await page.keyboard.press('Escape');
  });

  await walk('register-agent', async (has) => {
    await page.getByRole('button', { name: 'Register a profile…' }).click();
    await expect(page.getByRole('dialog', { name: /Register an agent profile/ })).toBeVisible();
    await has('home');
    await page.keyboard.press('Escape');
  });

  await walk('register-environment', async (has) => {
    await page.getByRole('button', { name: 'Register an environment…' }).click();
    await expect(page.getByRole('dialog', { name: /Register an environment/ })).toBeVisible();
    for (const id of HINTED_FORMS['register-environment']) await has(id);
    await page.keyboard.press('Escape');
  });

  await walk('make-environment', async (has) => {
    await page.goto('/resources/environments');
    await page.getByRole('button', { name: 'Add environment' }).click();
    await page.getByRole('menuitem', { name: /Make from a template/ }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    for (const id of HINTED_FORMS['make-environment']) await has(id);
    await page.keyboard.press('Escape');
  });

  await walk('environment-switches', async (has) => {
    await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
    for (const id of HINTED_FORMS['environment-switches']) await has(id);
  });

  await walk('edit-environment', async (has) => {
    await page.getByRole('button', { name: 'Edit', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Edit Workspace' })).toBeVisible();
    for (const id of HINTED_FORMS['edit-environment']) await has(id);
    await page.keyboard.press('Escape');
  });

  await walk('new-template', async (has) => {
    await page.goto('/resources/templates');
    await page.getByRole('button', { name: 'Add template' }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    for (const id of HINTED_FORMS['new-template']) await has(id);
    await page.keyboard.press('Escape');
  });

  await page.getByRole('link', { name: 'Node 22 dev', exact: true }).click();
  await page.getByRole('tab', { name: 'Configuration' }).click();
  await expect(page.getByRole('button', { name: 'Write an environment template', exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: 'Help: Configuration (JSON)', exact: true })).toHaveCount(0);

  await walk('add-host', async (has) => {
    await page.goto('/resources/hosts');
    await page.getByRole('button', { name: 'Add host' }).first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    for (const id of HINTED_FORMS['add-host']) await has(id);
    await page.keyboard.press('Escape');
  });

  await walk('host-settings', async (has) => {
    await page.getByRole('link', { name: 'host-1', exact: true }).click();
    await page.getByRole('tab', { name: 'Settings' }).click();
    for (const id of HINTED_FORMS['host-settings']) await has(id);
  });

  await walk('edit-host', async (has) => {
    await page.getByRole('tab', { name: 'Overview', exact: true }).click();
    await page.getByRole('button', { name: 'Edit', exact: true }).click();
    await expect(page.getByRole('tab', { name: 'Settings', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    for (const id of HINTED_FORMS['edit-host']) await has(id);
  });

  await walk('host-remove-runtime', async (has) => {
    await page.getByRole('tab', { name: /What runs on it/ }).click();
    await page.getByRole('button', { name: /^Actions for orch-env/ }).first().click();
    await page.getByRole('menuitem', { name: 'Remove…' }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await has('host-volumes');
    await page.keyboard.press('Escape');
  });

  await walk('add-machine', async (has) => {
    await page.goto('/resources/machines');
    await page.getByRole('button', { name: 'Add machine' }).click();
    await expect(page.getByRole('dialog', { name: 'Add machine' })).toBeVisible();
    await has('machine-delegate');
    await page.keyboard.press('Escape');
  });

  await walk('agent-profile', async (has) => {
    await page.goto(`/resources/agents/${RUNS_ON.codexOnLaptop}?tab=profile`);
    await expect(page.getByRole('tab', { name: 'Profile', exact: true, selected: true })).toBeVisible();
    for (const id of HINTED_FORMS['agent-profile']) await has(id);
  });

  await walk('edit-agent', async (has) => {
    await page.getByRole('button', { name: 'Edit', exact: true }).click();
    await expect(page.getByRole('dialog', { name: /Edit Codex on ada-laptop/ })).toBeVisible();
    await has('agent-pin');
    await page.keyboard.press('Escape');
  });

  await walk('add-agent', async (has) => {
    await page.goto('/resources/agents');
    await page.getByRole('button', { name: 'Add agent profile' }).first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await has('agent-harness');
    await page.keyboard.press('Escape');
  });

  // Every form of the list was walked; the ones with no hint are named here, form by form.
  expect([...seen.keys()].sort()).toEqual(Object.keys(HINTED_FORMS).sort());
  expect(missing).toEqual([]);
});

test('a field that a descriptor declares brings its own help to its hint, and one that gives none has no "?"', async ({ page }) => {
  await open(page, '/resources/providers');
  await page.getByRole('button', { name: 'Add provider' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add provider' });
  await dialog.getByRole('radio', { name: /Custom/ }).click();
  await dialog.getByRole('button', { name: 'Next' }).click();
  await dialog.getByRole('combobox', { name: 'Format' }).click();
  await page.getByRole('option').first().click();
  await dialog.getByText('Advanced').click();
  const options = dialog.locator('details');

  // The help a descriptor gives is the card of the "?" beside the field, and is no longer a line under it.
  await expect(options.getByText('How the key is sent to this endpoint.')).toHaveCount(0);
  await options.getByRole('button', { name: 'Help: How the key is presented' }).click();
  await expect(page.getByRole('dialog', { name: 'How the key is presented' }).getByText('How the key is sent to this endpoint.')).toBeVisible();
  await page.keyboard.press('Escape');
  await options.getByRole('button', { name: 'Help: Headers', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Headers' }).getByText('Headers sent with every request to this endpoint.')).toBeVisible();
  await page.keyboard.press('Escape');

  // An option that gives no help has no "?": nothing is invented for it.
  await expect(options.getByText('Organization')).toBeVisible();
  await expect(options.getByRole('button', { name: 'Help: Organization' })).toHaveCount(0);
});

test('a machine whose certificate has ended is enrolled again from its row and from its page, and stays the same machine', async ({ page }) => {
  await open(page, '/resources/machines');
  // The row of a machine that cannot connect any more offers the way back as the act most wanted.
  const row = rowOf(page, 'Machines', 'build-box');
  await row.hover();
  await expect(row.getByRole('button', { name: 'Enroll again… build-box' })).toBeVisible();
  await rowOf(page, 'Machines', 'ada-laptop').getByRole('button', { name: /^Actions for ada-laptop(?: · .+)?$/ }).click();
  // A machine that is well offers it in its menu too: a disk can be lost without the certificate having ended.
  await expect(page.getByRole('menuitem', { name: 'Enroll again…' })).toBeVisible();
  await page.keyboard.press('Escape');

  await row.getByRole('link', { name: 'build-box', exact: true }).click();
  await expect(page.getByText('The certificate of build-box has ended')).toBeVisible();
  await page.getByRole('button', { name: 'Enroll again…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Enroll build-box again' });

  // What stays and what goes, and the command for that machine alone, with the token in it.
  await expect(dialog.getByRole('heading', { name: 'What stays' })).toBeVisible();
  await expect(dialog.getByText('Its name, build-box, and its page')).toBeVisible();
  await expect(dialog.getByText(/The 2 things registered from it/)).toBeVisible();
  await expect(dialog.getByRole('heading', { name: 'What goes' })).toBeVisible();
  await expect(dialog.getByText(/Its old certificate/)).toBeVisible();
  await expect(dialog.getByText(/install\.sh .*--token 'oenr_/)).toBeVisible();
  await expect(dialog.getByText('Waiting for build-box to enroll…')).toBeVisible();
  const command = await dialog.getByText(/--token 'oenr_/).first().innerText();
  const token = /(oenr_[0-9a-f]+\.pin)/.exec(command)?.[1] ?? '';

  // The machine runs the command: it is the same machine, with a certificate that stands.
  await resourceControl('/enroll-again', { token });
  await expect(dialog.getByText('build-box is enrolled again')).toBeVisible();
  await dialog.getByRole('button', { name: 'Done' }).click();
  await expect(page.getByText('The certificate of build-box has ended')).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'build-box', level: 1 })).toBeVisible();
  await page.getByRole('tab', { name: /Registered/ }).click();
  await expect(page.getByRole('tab', { name: /Registered/ })).toContainText('2');
});

test('revocation names the connection certificate and a revoked machine can be enrolled again', async ({ page }) => {
  await open(page, `/resources/machines/${RUNS_ON.laptop}?tab=certificate`);
  await page.getByRole('button',{name:'Actions for ada-laptop',exact:true}).click();
  await page.getByRole('menuitem',{name:'Revoke…',exact:true}).click();
  const revoke=page.getByRole('alertdialog',{name:"Revoke ada-laptop's certificate?"});
  await expect(revoke.getByRole('button',{name:"Revoke ada-laptop's certificate",exact:true})).toBeVisible();
  await expect(revoke).toContainText('Its programs and files are kept.');
  await revoke.getByRole('button',{name:'Cancel',exact:true}).click();
  await expect(page.getByRole('button', { name: 'Enroll again…' }).first()).toBeVisible();

  await page.goto('/resources/machines');
  await page.getByRole('link', { name: 'old-mac', exact: true }).click();
  await expect(page.getByText('old-mac was revoked')).toBeVisible();
  await page.getByRole('tab', { name: 'Certificate' }).click();
  await page.getByRole('button',{name:'Actions for old-mac',exact:true}).click();
  await expect(page.getByRole('menuitem',{name:'Revoke…',exact:true})).toBeDisabled();
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Enroll again…' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Enroll old-mac again' });
  await expect(dialog.getByText(/its old certificate was revoked already/i)).toBeVisible();
  const token = /(oenr_[0-9a-f]+\.pin)/.exec(await dialog.getByText(/--token 'oenr_/).first().innerText())?.[1] ?? '';
  await resourceControl('/enroll-again', { token });
  await expect(dialog.getByText('old-mac is enrolled again')).toBeVisible();
});
