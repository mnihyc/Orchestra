/** Every screen of resources, asked in both themes whether anything on it is a serious barrier: each list, each page with each of its tabs, and what opens above them, for somebody who may do everything and for somebody who may only read. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

/** Opens a page by the link in its list, as a person would. */
async function openFrom(page: Page, list: string, name: string): Promise<void> {
  await page.goto(list);
  await page.getByRole('link', { name, exact: true }).click();
  await expect(page.getByRole('heading', { name, level: 1 })).toBeVisible();
}

/** Asks about every tab a page has, one after another. */
async function everyTab(page: Page, tabs: readonly (string | RegExp)[]): Promise<void> {
  for (const name of tabs) {
    await page.getByRole('tab', { name }).click();
    await expect(page.getByRole('tab', { name, selected: true })).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
}

test('machines: the list, the dialog that adds one, a machine with its tabs, and what registers on it', async ({ page }) => {
  await open(page, '/resources/machines');
  await expect(page.getByRole('table', { name: 'Machines' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add machine' }).click();
  await expect(page.getByRole('dialog', { name: 'Add machine' }).getByText(/oenr_/)).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await openFrom(page, '/resources/machines', 'ada-laptop');
  await expectAccessibleInBothThemes(page);
  await everyTab(page, ['Registered', 'Policy']);
  await page.getByRole('tab', { name: 'Registered' }).click();

  await page.getByRole('button', { name: 'Register a profile…' }).click();
  await expect(page.getByRole('dialog', { name: 'Register an agent profile from ada-laptop' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Register an environment…' }).click();
  await expect(page.getByRole('dialog', { name: 'Register an environment on ada-laptop' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Register a provider…' }).click();
  await expect(page.getByRole('dialog', { name: /Register a provider/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('providers: the list, the three steps of adding one, and a provider with each of its tabs', async ({ page }) => {
  await open(page, '/resources/providers');
  await expect(page.getByRole('table', { name: 'Providers' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add provider' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add provider' });
  await expect(dialog.getByText('Step 1 of 3')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('radio', { name: /Custom/ }).click();
  await dialog.getByRole('button', { name: 'Next' }).click();
  await expect(dialog.getByText('Step 2 of 3')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('combobox', { name: 'Format' }).click();
  await page.getByRole('option').first().click();
  await dialog.getByLabel('Address', { exact: true }).fill('https://models.example.test/v1');
  await dialog.getByLabel('API key', { exact: true }).fill('sk-test-0000');
  await dialog.getByRole('button', { name: 'Next' }).click();
  await expect(dialog.getByText('Step 3 of 3')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  // A provider on a key: a person may look after it, so every tab is there.
  await page.getByRole('link').filter({ hasText: /./ }).first().waitFor();
  await page.getByRole('table', { name: 'Providers' }).getByRole('link').first().click();
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  const tabs = await page.getByRole('tablist', { name: "Parts of the provider's page" }).getByRole('tab').allInnerTexts();
  expect(tabs.length).toBeGreaterThan(3);
  for (const name of tabs) {
    const label = name.replace(/\s*\d+$/, '').trim();
    await page.getByRole('tab', { name: new RegExp(`^${label}`) }).click();
    await expectAccessibleInBothThemes(page);
  }
});

test('agent profiles: the list, the dialog that adds one, and a profile with each of its tabs', async ({ page }) => {
  await open(page, '/resources/agents');
  await expect(page.getByRole('table', { name: 'Agent profiles' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add agent profile' }).first().click();
  await expect(page.getByRole('dialog', { name: 'Add agent profile' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('table', { name: 'Agent profiles' }).getByRole('link').first().click();
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await everyTab(page, [/^Parameters/, 'Profile', 'Access']);

  const resourceName = await page.getByTestId('main').getByRole('heading', { level: 1 }).innerText();
  await page.getByTestId('main').getByRole('button', { name: `Actions for ${resourceName}`, exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  await expect(page.getByRole('alertdialog')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('environments: the list, a directory on a machine, one that Orch made, and what opens above them', async ({ page }) => {
  await open(page, '/resources/environments');
  await expect(page.getByRole('table', { name: 'Environments' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add environment' }).click();
  await expect(page.getByRole('menu')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
  await page.getByRole('menuitem', { name: /Make from a template/ }).click();
  await expect(page.getByRole('dialog', { name: 'Add environment' }).getByRole('combobox', { name: 'Template' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  // A directory on a person's machine: the limits of that machine bound it.
  await openFrom(page, '/resources/environments', 'Workspace');
  await expectAccessibleInBothThemes(page);
  await everyTab(page, [/^Threads/, 'Machine limits', 'Settings', 'Access', 'Danger zone']);

  // One that Orch made, with a job under way.
  await openFrom(page, '/resources/environments', 'Sandbox');
  await expectAccessibleInBothThemes(page);
  await page.getByRole('button', { name: 'Start', exact: true }).click();
  await expect(page.getByText('Being made').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();

  await page.getByRole('button', { name: /^Actions for Sandbox(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Rename…' }).click();
  await expect(page.getByRole('dialog', { name: 'Rename Sandbox' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: /^Actions for Sandbox(?: · .+)?$/ }).click();
  // An environment is removed, and kept for a while: what its menu says since the kept environments are told.
  await page.getByRole('menuitem', { name: 'Remove…' }).click();
  await expect(page.getByRole('dialog', { name: 'Remove Sandbox?' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('templates: the list, the dialog that makes one, and a template with each of its tabs', async ({ page }) => {
  await open(page, '/resources/templates');
  await expect(page.getByRole('table', { name: 'Templates' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add template' }).click();
  await expect(page.getByRole('dialog', { name: 'Add template' }).getByLabel('Image reference')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await openFrom(page, '/resources/templates', 'Node 22 dev');
  await expectAccessibleInBothThemes(page);
  await everyTab(page, ['Configuration', 'Access', 'Danger zone']);
  await page.getByRole('button', { name: 'Delete…' }).click();
  await expect(page.getByRole('alertdialog')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('hosts: the list, the dialog that enrolls one, and a host with each of its tabs', async ({ page }) => {
  await open(page, '/resources/hosts');
  await expect(page.getByRole('table', { name: 'Hosts' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add host' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add host' });
  await expect(dialog.getByRole('button', { name: 'Make the command' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('button', { name: 'Make the command' }).click();
  await expect(dialog.getByText(/oenr_/)).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await openFrom(page, '/resources/hosts', 'host-1');
  await expectAccessibleInBothThemes(page);
  await everyTab(page, [/What runs on it/, 'Settings', 'Access']);

  await page.getByRole('tab', { name: /What runs on it/ }).click();
  await page.getByRole('button', { name: /^Actions for orch-env-sandbox(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Remove…' }).click();
  await expect(page.getByRole('dialog', { name: 'Remove orch-env-sandbox?' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('somebody who may only read sees the lists and the pages with what they may not do kept from them', async ({ page }) => {
  await open(page, '/resources/providers', PEOPLE.vera);
  // A person who owns nothing is told so, and may still begin: what they may not do is refused when they try.
  await expect(page.getByText('You own no provider')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add provider' }).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);

  for (const [path, table, nothing] of [
    ['/resources/agents', 'Agent profiles', 'No agent profiles yet'],
    ['/resources/machines', 'Machines', 'No machine is connected yet'],
    ['/resources/environments', 'Environments', 'No environments yet'],
    ['/resources/templates', 'Templates', 'No templates yet'],
    ['/resources/hosts', 'Hosts', 'No hosts yet'],
  ] as const) {
    await page.goto(path);
    await expect(page.getByRole('table', { name: table }).or(page.getByText(nothing)).first()).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }

  // A template that was shared with them: its recipe and its access are theirs to read, nothing else.
  await page.goto('/resources/templates');
  await page.getByRole('table', { name: 'Templates' }).getByRole('link').first().click();
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Danger zone' })).toHaveCount(0);
  await page.getByRole('tab', { name: 'Configuration' }).click();
  await expect(page.getByText('You may read this configuration but not change it.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save template' })).toHaveCount(0);
  await everyTab(page, ['Configuration', 'Access']);
});
