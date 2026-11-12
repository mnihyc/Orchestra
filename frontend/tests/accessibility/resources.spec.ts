/** Every screen of resources, asked in both themes whether anything on it is a serious barrier: each list, each page with each of its tabs, and what opens above them, for somebody who may do everything and for somebody who may only read. */
import { expect, type Page, test } from '@playwright/test';

import { signInForManagement } from '../e2e/admin-mode-support.ts';
import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn, mainArea } from '../e2e/support.ts';

test.beforeEach(async ({ page }) => {
  // Audits extend the test deadline; actions retain the normal assertion wait budget.
  page.setDefaultTimeout(30_000);
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

/** Libraries start with usable items; viewers follow the offer to items they may view.
 * An empty access scope has its own message instead of claiming that no resources exist. */
async function openViewableList(page: Page, path: string, table: string): Promise<void> {
  await page.goto(path);
  const contents = page.getByRole('table', { name: table }).or(page.getByText('Nothing you can view here', { exact: true }));
  const wider = page.getByRole('button', { name: 'Show what you can view', exact: true });
  await expect(contents.or(wider).first()).toBeVisible();
  if (await wider.isVisible()) await wider.click();
  await expect(contents.first()).toBeVisible();
}

/** Asks about every tab a page has, one after another. */
async function everyTab(page: Page, tabs: readonly (string | RegExp)[]): Promise<void> {
  for (const name of tabs) {
    await expect(page.getByRole('tab', { name })).toBeVisible();
    await page.getByRole('tab', { name }).click();
    await expect(page.getByRole('tab', { name, selected: true })).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
}

/** Resource sections use links when there is room and the same named chooser when narrow. */
async function section(page: Page, navigation: string, name: string): Promise<void> {
  const nav = page.getByRole('navigation', { name: navigation, exact: true }).last();
  await expect(nav).toBeVisible();
  const link = nav.getByRole('link', { name, exact: true });
  if (await link.isVisible()) await link.click();
  else {
    await nav.getByRole('combobox', { name: navigation, exact: true }).click();
    await page.getByRole('option', { name, exact: true }).click();
  }
}

test('machines: the list, the dialog that adds one, a machine with its tabs, and what registers on it', async ({ page }) => {
  await open(page, '/resources/machines');
  await expect(page.getByRole('table', { name: 'Machines' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add machine' }).click();
  const adding = page.getByRole('dialog', { name: 'Add machine' });
  // The command is made once its choices are read: Show command makes it.
  await expectAccessibleInBothThemes(page);
  await adding.getByRole('button', { name: 'Show command' }).click();
  await expect(adding.getByText(/oenr_/).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await openFrom(page, '/resources/machines', 'ada-laptop');
  await expectAccessibleInBothThemes(page);
  await everyTab(page, [/^Registered here/, 'Policy']);
  await page.getByRole('tab', { name: /^Registered here/ }).click();

  await page.getByRole('button', { name: 'Register a profile…' }).click();
  const profileRegistration = page.getByRole('dialog', { name: 'Register an agent profile from ada-laptop' });
  await expect(profileRegistration).toBeVisible();
  await expectAccessibleInBothThemes(page);
  // Escape may first dismiss the focused program chooser; cancel the dialog itself.
  await profileRegistration.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(profileRegistration).toHaveCount(0);

  await section(page, 'Registered here', 'Environments');
  await page.getByRole('button', { name: 'Register an environment…' }).click();
  const environmentRegistration = page.getByRole('dialog', { name: 'Register an environment on ada-laptop' });
  await expect(environmentRegistration).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await environmentRegistration.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(environmentRegistration).toHaveCount(0);

  await section(page, 'Registered here', 'Providers');
  await page.getByRole('button', { name: 'Register a provider…' }).click();
  await expect(page.getByRole('dialog', { name: 'Add provider', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('providers: the list, both stages of adding one, and a provider with each of its tabs', async ({ page }) => {
  await open(page, '/resources/providers');
  await expect(page.getByRole('table', { name: 'Providers' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add provider' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add provider' });
  // Connection and credentials now share the first stage; placement and sources form the second.
  await expect(dialog.getByRole('combobox', { name: 'Fill from an example', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('combobox', { name: 'Format' }).click();
  await page.getByRole('option').first().click();
  await dialog.getByLabel('Address', { exact: true }).fill('https://models.example.test/v1');
  await dialog.getByLabel('Key', { exact: true }).fill('sk-test-0000');
  await dialog.getByRole('combobox', { name: 'Key role', exact: true }).click();
  await page.getByRole('option', { name: 'Model key', exact: true }).click();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('button', { name: 'Next' }).click();
  await expect(dialog.getByRole('combobox', { name: 'Keep the key on', exact: true })).toBeVisible();
  await expect(dialog.getByRole('combobox', { name: 'Run the connection on', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');
  await expect(dialog).toHaveCount(0);

  // A provider on a key: a person may look after it, so every tab is there.
  // Rows also link their home, so choose the resource by its name.
  await openFrom(page, '/resources/providers', 'OpenAI key');
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
  // The first profile belongs to the system, whose management uses the session switch.
  await signInForManagement(page);
  await page.goto('/resources/agents');
  await expect(page.getByRole('table', { name: 'Agent profiles' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add agent profile' }).first().click();
  await expect(page.getByRole('dialog', { name: 'Add agent profile' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('table', { name: 'Agent profiles' }).getByRole('link', { name: 'Codex', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Codex', level: 1, exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await everyTab(page, [/^Parameters/, 'Settings', 'Access']);

  const resourceName = await mainArea(page).getByRole('heading', { level: 1 }).innerText();
  await mainArea(page).getByRole('button', { name: `Actions for ${resourceName}`, exact: true }).click();
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
  await everyTab(page, [/^Threads/, 'Settings', 'Access']);
  // Machine limits and lifecycle now live beside configuration under Settings.
  await page.getByRole('tab', { name: 'Settings', exact: true }).click();
  for (const name of ['Configuration', 'Switches', 'Machine limits', 'Lifecycle']) {
    await section(page, 'Settings', name);
    await expectAccessibleInBothThemes(page);
  }

  // One that Orch made, with a job under way.
  await openFrom(page, '/resources/environments', 'Sandbox');
  await expectAccessibleInBothThemes(page);
  await page.getByRole('button', { name: 'Start', exact: true }).click();
  await expect(page.getByText('Being made').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();

  // The visible editor owns renaming, so its folded menu omits the duplicate action.
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  const editing = page.getByRole('dialog', { name: 'Edit Sandbox' });
  await expect(editing.getByRole('textbox', { name: 'Name', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await editing.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(editing).toHaveCount(0);

  await page.getByRole('button', { name: /^Actions for Sandbox(?: · .+)?$/ }).click();
  // Deletion retains its content for a while; the confirmation names that logical resource.
  await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Delete Sandbox?', exact: true })).toBeVisible();
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
  // Configuration is named Settings, and the former Danger zone is Lifecycle.
  await everyTab(page, ['Settings', 'Access', 'Lifecycle']);
  await page.getByRole('button', { name: 'Actions for Node 22 dev', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click();
  await expect(page.getByRole('alertdialog')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('hosts: the list, the dialog that enrolls one, and a host with each of its tabs', async ({ page }) => {
  await open(page, '/resources/hosts');
  await expect(page.getByRole('table', { name: 'Hosts' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Add host' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add host' });
  // Hosts use the same enrollment dialog as machines, with Node selected.
  await expect(dialog.getByRole('button', { name: 'Show command', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await dialog.getByRole('button', { name: 'Show command', exact: true }).click();
  await expect(dialog.getByText(/oenr_/)).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await openFrom(page, '/resources/hosts', 'host-1');
  await expectAccessibleInBothThemes(page);
  await everyTab(page, [/What runs on it/, 'Settings', 'Access']);

  await page.getByRole('tab', { name: /What runs on it/ }).click();
  // The physical runtime is named for its environment; removing its volumes stays optional.
  await page.getByRole('tabpanel', { name: /What runs on it/ }).getByRole('button', { name: 'Actions for Sandbox', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click();
  const removal = page.getByRole('dialog', { name: 'Delete Sandbox?', exact: true });
  await expect(removal).toBeVisible();
  await expect(removal.getByRole('checkbox', { name: 'Delete its owned volumes too', exact: true })).not.toBeChecked();
  await expectAccessibleInBothThemes(page);
});

test('somebody who may only read sees the lists and the pages with what they may not do kept from them', async ({ page }) => {
  await open(page, '/resources/providers', PEOPLE.vera);
  // The list opens on what they may use; with nothing there it says so and offers what they may view. They may still begin:
  // what they may not do is refused when they try.
  await expect(page.getByText('Nothing you can use here')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Show what you can view' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add provider' }).first()).toBeVisible();
  await expectAccessibleInBothThemes(page);

  for (const [path, table] of [
    ['/resources/agents', 'Agent profiles'],
    ['/resources/machines', 'Machines'],
    ['/resources/environments', 'Environments'],
    ['/resources/templates', 'Templates'],
    ['/resources/hosts', 'Hosts'],
  ] as const) {
    await openViewableList(page, path, table);
    await expectAccessibleInBothThemes(page);
  }

  // The shared system template permits viewing and use, with its settings read-only.
  await openViewableList(page, '/resources/templates', 'Templates');
  await page.getByRole('table', { name: 'Templates' }).getByRole('link', { name: 'Default', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Default', level: 1, exact: true })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Lifecycle' })).toHaveCount(0);
  await page.getByRole('tab', { name: 'Settings', exact: true }).click();
  await expect(page.getByText(/You can view these values[.;]/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save template' })).toHaveCount(0);
  await everyTab(page, ['Settings', 'Access']);
});
