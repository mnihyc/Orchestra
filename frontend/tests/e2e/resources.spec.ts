/** Machines, providers, agent profiles, environments, templates and hosts: a machine added and seen connecting, a provider made on a key with a model, an agent profile and an environment registered on a machine, a refusal that says what is missing, each list narrowed by its search and filters, and what is done to an environment, a template and a host. */
import { expect, type Page, test } from '@playwright/test';
import words from '../../src/i18n/en.json' with { type: 'json' };

import { PEOPLE, replySent, scenario, signIn } from './support.ts';
import { templateJson } from './template-support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** What the fake API's resource part does that no real system can: a machine that enrolls, one that goes away. */
async function resourceControl(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/resources${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** Somebody else, at the same time: a request made with the token of a person, as another browser would make it. */
async function asPerson(person: { login: string; password: string }, method: string, path: string, body?: unknown): Promise<unknown> {
  const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ login: person.login, password: person.password }) });
  const { access_token: token } = (await login.json()) as { access_token: string };
  const response = await fetch(`${DOUBLE}/api/v1${path}`, {
    method,
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) throw new Error(`${method} ${path} answered ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** How long a key is held down when the test presses it: a finger is on a key for about this long, and a stroke of no length is not one. */
const KEY_HELD_MS = 60;

/** A notice as it is drawn, not the line that reads it to a screen reader. */
const notice = (page: Page, text: string) => page.getByText(text, { exact: true });

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

test.beforeEach(async () => {
  await scenario();
});

test('a machine chooses its roles and their handover, keeps Node separate, and is seen connecting', async ({ page }) => {
  await open(page, '/resources/machines');
  await expect(page.getByRole('table', { name: 'Machines' })).toBeVisible();

  await page.getByRole('button', { name: 'Add machine' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toHaveAccessibleName('Add machine');
  await expect(dialog.locator('pre code')).toHaveCount(0);
  await dialog.getByRole('button',{name:'Show command'}).click();
  // The command is the installer's, with the single-use token in it, for each system; the one of this browser is first.
  const system = (await page.evaluate(() => navigator.userAgent.includes('Windows'))) ? 'Windows' : 'Linux';
  await expect(dialog.getByRole('tab', { name: system })).toHaveAttribute('aria-selected', 'true');
  await dialog.getByRole('tab', { name: 'Linux' }).click();
  const command = dialog.locator('pre code');
  await expect(command).toContainText("--roles 'environment,agent'");
  await expect(command).not.toContainText('--delegate');
  for (const [name, description] of [
    ['Environment', 'Let people work in folders of this machine'], ['Agent', 'Run agents here'],
    ['Provider', 'Keep provider keys here'], ['Node', 'Let Orch make environments here'],
  ] as const) {
    await expect(dialog.getByRole('checkbox', { name, exact: true })).toHaveAccessibleDescription(description);
    await expect(dialog.getByText(description, { exact: true })).toBeVisible();
  }
  await expect(dialog.getByText('Waiting for the machine to connect…')).toBeVisible();

  // Connector roles combine. Only Environment hands over its section; Provider keeps its keys locally.
  await dialog.getByRole('checkbox', { name: 'Agent' }).click();
  await dialog.getByRole('checkbox', { name: 'Provider' }).click();
  await dialog.getByRole('group', { name: 'Environment' }).getByRole('switch', { name: 'Full access: Off' }).click();
  await dialog.getByRole('switch', { name: 'Full control' }).click();
  await expect(command).toHaveCount(0);
  await dialog.getByRole('button',{name:'Show command'}).click();
  await expect(command).toContainText("--roles 'environment,provider' --delegate 'environment' --dedicated");
  await expect(dialog.getByRole('group', { name: 'Provider' }).getByRole('switch', { name: 'Full access: Off' })).not.toBeChecked();
  await dialog.getByRole('tab', { name: 'Windows' }).click();
  await expect(command).toContainText("-Roles 'environment,provider' -Delegate 'environment' -Dedicated");
  await expect(command).not.toContainText('-DelegatePolicy');

  // Node has its own program and enrollment. It never combines with the connector's roles.
  await dialog.getByRole('checkbox', { name: 'Node' }).click();
  await expect(dialog).toHaveAccessibleName('Add machine');
  await expect(dialog.getByText(/Runs as root/u)).toBeVisible();
  await dialog.getByRole('button',{name:'Show command'}).click();
  await expect(command).toContainText('install-node.sh');
  await expect(command).not.toContainText('--roles');
  await expect(command).not.toContainText('--delegate-policy');
  for (const name of ['Environment', 'Agent', 'Provider']) await expect(dialog.getByRole('checkbox', { name })).not.toBeChecked();
  await dialog.getByRole('group', { name: 'Node' }).getByRole('switch', { name: 'Full access: Off' }).click();
  await dialog.getByRole('button',{name:'Show command'}).click();
  await expect(command).toContainText('--delegate-policy');

  // Back to a connector: both chosen roles keep their settings. Enroll the exact displayed token.
  await dialog.getByRole('checkbox', { name: 'Provider' }).click();
  await expect(dialog.getByRole('checkbox', { name: 'Node' })).not.toBeChecked();
  await expect(command).toHaveCount(0);
  await dialog.getByRole('checkbox', { name: 'Environment' }).click();
  await dialog.getByRole('group', { name: 'Environment' }).getByRole('switch', { name: 'Full access: On' }).click();
  await dialog.getByRole('button',{name:'Show command'}).click();
  await expect(command).toContainText("--roles 'environment,provider' --dedicated");
  await expect(command).not.toContainText('--delegate');
  const token = /--token '([^']+)'/.exec(await command.innerText())?.[1];
  if (token === undefined) throw new Error('the current connector command has no enrollment token');

  // Another valid enrollment for the same person is a new list row, not this dialog's receipt.
  const unrelated = await asPerson(PEOPLE.ada, 'POST', '/machines/enrollment-tokens', { roles: ['agent'] });
  if (typeof unrelated !== 'object' || unrelated === null || !('token' in unrelated) || typeof unrelated.token !== 'string') throw new Error('the other enrollment has no token');
  await resourceControl('/enroll', { name: 'other-build-vm', token: unrelated.token });
  await expect(dialog).toBeVisible();
  await expect(notice(page, words.machines.add.connected)).toHaveCount(0);

  // The first online connection closes the dialog and opens the machine without another click.
  await resourceControl('/enroll', { name: 'build-vm', token });
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'build-vm', level: 1 })).toBeVisible();
  await expect(notice(page, words.machines.add.connected)).toHaveCount(1);
  await expect(notice(page, words.machines.add.connected)).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Overview' })).toBeVisible();
  await page.getByRole('button',{name:'Help: Roles'}).click();
  await expect(page.getByText(/orch-connector --data-dir .+ roles set environment,provider/u)).toBeVisible();
  await expect(page.getByText('Roles are kept on this machine.', { exact: true })).toBeVisible();
  await expect(page.getByRole('dialog', { name: 'Roles', exact: true })).toContainText('Locally kept roles use the command shown here.');
});

test('a provider is made on a key and given a model', async ({ page }) => {
  await open(page, '/resources/providers');
  await page.getByRole('button', { name: 'Add provider' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add provider' });
  // The interface comes first; examples only fill these ordinary fields.
  await dialog.getByRole('combobox', { name: 'Format' }).click();
  await page.getByRole('option').first().click();
  await dialog.getByLabel('Address', { exact: true }).fill('https://models.example.test/v1');
  await dialog.getByRole('button', { name: 'Next' }).click();

  await dialog.getByLabel('Name', { exact: true }).fill('Example models');
  await dialog.getByRole('button', { name: 'Add provider' }).click();

  // Key editing is on Sign-in and keys, after creation; the keeper does not change.
  await expect(page.getByRole('heading', { name: 'Example models', level: 1 })).toBeVisible();
  await page.getByLabel('Key value',{exact:true}).fill('sk-test-0000');
  await page.getByRole('button',{name:'Save key',exact:true}).click();
  await page.getByRole('tab',{name:/^Models/}).click();
  await expect(page.getByText('No model is listed')).toBeVisible();
  await page.getByRole('button', { name: 'Add model' }).first().click();
  const adding = page.getByRole('dialog', { name: 'Add a model to Example models' });
  await adding.getByLabel('Model ID').fill('example-large');
  await adding.getByLabel('Display name').fill('Example Large');
  await adding.getByRole('button', { name: 'Add model' }).click();
  await expect(notice(page, 'Example Large was added')).toBeVisible();
  await expect(page.getByRole('table', { name: /Models/ }).getByText('example-large')).toBeVisible();
});

test('an agent profile and an environment are registered on a machine', async ({ page }) => {
  await open(page, '/resources/machines');
  await page.getByRole('link', { name: 'ada-laptop' }).click();
  await page.getByRole('tab', { name: /Registered/ }).click();

  await page.getByRole('button', { name: 'Register a profile…' }).click();
  const profile = page.getByRole('dialog', { name: 'Register an agent profile from ada-laptop' });
  await profile.getByLabel(/^Name/).fill('Codex on the laptop');
  await profile.getByRole('button', { name: 'Register profile' }).click();
  await expect(notice(page, 'Codex on the laptop was registered')).toBeVisible();
  // The new profile's page opens; the machine is one step back.
  await expect(page.getByRole('heading', { name: 'Codex on the laptop', level: 1 })).toBeVisible();
  await page.goBack();
  await page.getByRole('tab', { name: /Registered/ }).click();
  const profileLink = page.getByRole('link', { name: 'Codex on the laptop', exact: true });
  await expect(profileLink).toBeVisible();
  // Nested sections must fit their pane, including when a desktop workbench leaves it phone-sized.
  for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(profileLink).toBeInViewport();
    const pane = await page.locator('[data-main-area]').boundingBox();
    const link = await profileLink.boundingBox();
    expect(pane).not.toBeNull(); expect(link).not.toBeNull();
    expect(link!.x).toBeGreaterThanOrEqual(pane!.x);
    expect(link!.x + link!.width).toBeLessThanOrEqual(pane!.x + pane!.width);
    await test.info().attach(`registered-profile-${width}-${theme}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
  }

  const registered = page.locator('[data-resource-section="registered"] [data-section-nav]');
  const groupPicker = registered.getByRole('combobox');
  if (await groupPicker.isVisible()) { await groupPicker.click(); await page.getByRole('option', { name: words.machines.registered.environments, exact: true }).click(); }
  else await registered.getByRole('link', { name: words.machines.registered.environments, exact: true }).click();
  await page.getByRole('button', { name: 'Register an environment…' }).click();
  const environment = page.getByRole('dialog', { name: 'Register an environment on ada-laptop' });
  await environment.getByLabel(/^Name/).fill('Scratch');
  await environment.getByRole('combobox', { name: words.environmentData.workspace, exact: true }).click();
  await page.getByRole('option', { name: words.choiceCatalog.dataSource.existing_directory.name, exact: true }).click();
  // The served root policy blocks an unsafe path before registration and names the problem at its field.
  const directory = environment.getByLabel(words.environmentData.path, { exact: true });
  await directory.fill('/etc');
  await expect(directory).toHaveAttribute('aria-invalid', 'true');
  await expect(environment.getByText(words.environmentData.chooseAllowed, { exact: true })).toBeVisible();
  await expect(environment.getByRole('button', { name: 'Register environment', exact: true })).toHaveAttribute('aria-disabled', 'true');
  await directory.fill('/home/ada/work/scratch');
  await environment.getByRole('button', { name: 'Register environment' }).click();
  await expect(notice(page, 'Scratch was registered')).toBeVisible();

  await page.goto('/resources/environments');
  await expect(page.getByRole('link', { name: 'Scratch' })).toBeVisible();
  const runs = page.getByRole('row', { name: /Select Scratch/ }).getByTitle('/home/ada/work/scratch on ada-laptop', { exact: true });
  await expect(runs).toHaveText('…/work/scratch on ada-laptop');
  await expect(runs).toBeVisible();
});

test('a person who may only read is refused, and the refusal says what is missing and whom to ask', async ({ page }) => {
  await open(page, '/resources/agents', PEOPLE.vera);
  await page.getByRole('button', { name: 'Add agent profile' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add agent profile' });
  await dialog.getByRole('combobox', { name: 'Agent program', exact: true }).click();
  await page.getByRole('option', { name: 'Codex' }).click();
  await dialog.getByLabel('Name', { exact: true }).fill('Vera’s profile');
  const refused = page.waitForResponse((response) => response.request().method() === 'POST' && /\/orgs\/[^/]+\/agents$/.test(response.url()));
  await dialog.getByRole('button', { name: 'Add agent profile' }).click();
  const response = await refused;
  expect(response.status()).toBe(403);
  expect(await replySent(response)).toMatchObject({ code: 'authz.denied', required: 'settings.manage' });
  // What is missing, where, and who can give it.
  await dialog.getByRole('button', { name: 'Review access', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review access', exact: true });
  await expect(review).toContainText(words.access.refusal.settings.replace('{{place}}', 'Acme'));
  await expect(review).toContainText('Ada Lovelace');
});

test('every list narrows by what is typed and by its filters, and the address says so', async ({ page }) => {
  await open(page, '/resources/machines');

  // Machines
  await expect(page.getByRole('link', { name: 'ada-laptop' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'build-box' })).toBeVisible();
  await page.getByRole('searchbox', { name: 'Search machines' }).fill('build');
  await expect(page.getByRole('link', { name: 'ada-laptop' })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'build-box' })).toBeVisible();
  await expect(page).toHaveURL(/q=build/);
  await page.getByRole('searchbox', { name: 'Search machines' }).fill('nothing like it');
  await expect(page.getByText('No machine matches that.')).toBeVisible();

  // Providers: the views and the status
  await page.goto('/resources/providers');
  await expect(page.getByRole('table', { name: 'Providers' })).toBeVisible();
  const providers = await page.getByRole('table', { name: 'Providers' }).getByRole('row').count();
  await page.getByRole('button', { name: words.resources.controls.filter, exact: true }).click();
  await page.getByRole('combobox', { name: 'Status' }).click();
  await page.getByRole('option', { name: 'Ready' }).click();
  await expect(page).toHaveURL(/status=ready/);
  expect(await page.getByRole('table', { name: 'Providers' }).getByRole('row').count()).toBeLessThan(providers);

  // Agent profiles
  await page.goto('/resources/agents');
  await expect(page.getByRole('table', { name: 'Agent profiles' })).toBeVisible();
  await page.getByRole('button', { name: words.resources.controls.filter, exact: true }).click();
  await page.getByRole('radio', { name: 'Enabled', exact: true }).click();
  await expect(page).toHaveURL(/state=enabled/);
  await page.getByRole('searchbox', { name: 'Search agent profiles' }).fill('zzz');
  await expect(page.getByText('No agent profile matches.')).toBeVisible();

  // Environments
  await page.goto('/resources/environments');
  await expect(page.getByRole('link', { name: 'Sandbox' })).toBeVisible();
  await page.getByRole('button', { name: words.resources.controls.filter, exact: true }).click();
  await page.getByRole('radio', { name: 'Template', exact: true }).click();
  await expect(page).toHaveURL(/kind=template/);
  await expect(page.getByRole('link', { name: 'Workspace' })).toHaveCount(0);
  await expect(page.getByRole('link', { name: 'Sandbox' })).toBeVisible();
  await page.getByRole('radio', { name: 'All', exact: true }).click();
  await page.getByRole('combobox', { name: 'Status' }).click();
  await page.getByRole('option', { name: 'Offline' }).click();
  await expect(page).toHaveURL(/status=offline/);
  await expect(page.getByRole('link', { name: 'Docs checkout' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Sandbox' })).toHaveCount(0);

  // Templates
  await page.goto('/resources/templates');
  await expect(page.getByRole('link', { name: 'Node 22 dev' })).toBeVisible();
  await page.getByRole('button', { name: words.resources.controls.filter, exact: true }).click();
  await page.getByRole('radio', { name: 'Provider', exact:true }).click();
  await expect(page).toHaveURL(/kind=provider/);
  await expect(page.getByRole('link', { name: 'Provider runtime' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Node 22 dev' })).toHaveCount(0);
  await page.getByRole('radio', { name: 'All' }).click();
  await page.getByRole('searchbox', { name: 'Search templates' }).fill('pnpm');
  await expect(page.getByRole('link', { name: 'Node 22 dev' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Default' })).toHaveCount(0);

  // Hosts
  await page.goto('/resources/hosts');
  await expect(page.getByRole('link', { name: 'host-1' })).toBeVisible();
  await page.getByRole('button', { name: words.resources.controls.filter, exact: true }).click();
  await page.getByRole('radio', { name: 'Paused' }).click();
  await expect(page).toHaveURL(/state=closed/);
  await expect(page.getByRole('link', { name: 'build-host' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'host-1' })).toHaveCount(0);
});

test('an environment that Orch made is started, kept awake and stopped, and the page follows the job', async ({ page }) => {
  await open(page, '/resources/environments');
  await page.getByRole('link', { name: 'Sandbox' }).click();
  await expect(page.getByRole('heading', { name: 'Sandbox', level: 1 })).toBeVisible();

  await page.getByRole('button', { name: 'Start', exact: true }).click();
  // While the job works the page says so; it follows it to its end without being asked.
  await expect(page.getByText('Being made').first()).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Keep awake' })).toBeVisible();

  await page.getByRole('button', { name: 'Keep awake' }).click();
  await expect(notice(page, 'It counts as in use now, so the idle timer starts over')).toBeVisible();
  await page.getByRole('button', { name: 'Stop', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Start', exact: true })).toBeVisible();

  // Deleting it asks for its name.
  await page.getByRole('button', { name: /^Actions for Sandbox(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete Sandbox?' });
  await expect(dialog.getByRole('button', { name: 'Delete Sandbox' })).toHaveAttribute('aria-disabled', 'true');
  await dialog.getByLabel('Type Sandbox to confirm').fill('Sandbox');
  await dialog.getByRole('button', { name: 'Delete Sandbox' }).click();
  await expect(dialog.getByText('It is refused while something still depends on it. Move those threads first.', { exact: true })).toBeVisible();
  await expect(page).toHaveURL(/\/resources\/environments\/[^/]+$/);
});

test('an environment is made from a template on a host, and its page follows the job', async ({ page }) => {
  await open(page, '/resources/environments');
  await page.getByRole('button', { name: 'Add environment' }).click();
  await page.getByRole('menuitem', { name: /Make from a template/ }).click();
  const dialog = page.getByRole('dialog', { name: 'Add environment' });
  // Only a template that is switched on is offered, and only a host that takes new environments with its driver.
  await expect(dialog.getByRole('combobox', { name: 'Template' })).toContainText('Default');
  const host = dialog.getByRole('combobox', { name: 'Host', exact: true });
  await expect(host).toContainText('Automatic');
  await host.click();
  await expect(page.getByRole('option', { name: 'Automatic', exact: true })).toContainText('The one with the most room');
  await page.keyboard.press('Escape');
  await dialog.getByLabel('Name', { exact: true }).fill('Review');
  await dialog.getByRole('button', { name: 'Add environment' }).click();

  await expect(page.getByRole('heading', { name: 'Review', level: 1 })).toBeVisible();
  await expect(notice(page, 'Review is being made')).toBeVisible();
  await expect(page.getByText('Being made').first()).toBeVisible();
  // The job ends by itself, and the page, which asks again while it works, shows what it made.
  await expect(page.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();

  // A name of one's own that is taken is refused where it was typed.
  await page.goto('/resources/environments');
  await page.getByRole('button', { name: 'Add environment' }).click();
  await page.getByRole('menuitem', { name: /Make from a template/ }).click();
  const again = page.getByRole('dialog', { name: 'Add environment' });
  await again.getByLabel('Name', { exact: true }).fill('Review');
  await again.getByRole('button', { name: 'Add environment' }).click();
  await expect(again.getByText('you already have an environment with this name')).toBeVisible();
});

test('a template is made, its recipe is changed, and a change made meanwhile by somebody else is not overwritten', async ({ page }) => {
  await open(page, '/resources/templates');
  await page.getByRole('button', { name: words.templates.list.add }).click();
  const dialog = page.getByRole('dialog', { name: words.templates.new.title });
  // Creation asks only the essentials, preserving the complete example for the settings page.
  await expect(dialog.getByLabel('Image reference')).toHaveValue('node:22-bookworm');
  await expect(dialog.getByRole('switch', { name: 'JSON', exact: true })).toHaveCount(0);
  await dialog.getByLabel('Name', { exact: true }).fill('Python 3.13');
  await dialog.getByLabel('Image reference').fill('python:3.13-bookworm');
  await dialog.getByRole('button', { name: words.templates.new.submit }).click();

  await expect(page.getByRole('heading', { name: 'Python 3.13', level: 1 })).toBeVisible();
  await expect(page.getByText('python:3.13-bookworm').first()).toBeVisible();
  const id = page.url().split('/').at(-1)?.split('?')[0] ?? '';

  await page.getByRole('tab', { name: words.resourceDetails.layout.settings, exact: true }).click();
  await templateJson(page);
  await expect(page.getByLabel('Configuration (JSON)')).toContainText('workspace_mount');
  // A text that is not an object is not sent.
  await page.getByLabel('Configuration (JSON)').fill('[1, 2');
  await expect(page.getByRole('button', { name: 'Save template' })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('button', { name: 'Line 1: Enter valid JSON with an object at its root.', exact: true })).toBeVisible();

  // Somebody else saves first; this page still holds the revision it read.
  await asPerson(PEOPLE.ada, 'PATCH', `/templates/${id}`, { expected_revision: 1, description: 'Changed elsewhere' });
  await page.getByLabel('Configuration (JSON)').fill('{ "image": "python:3.13-slim" }');
  await page.getByRole('button', { name: 'Save template' }).click();
  await expect(page.getByText('Somebody changed this template first')).toBeVisible();

  // A refresh keeps the draft's original fence. Explicitly review before adopting the newer revision.
  await page.getByText('Review changes', { exact: true }).click();
  await expect(page.getByRole('table', { name: 'Review changes' })).toContainText('Changed elsewhere');
  await page.getByRole('button', { name: 'Keep my changes', exact: true }).click();
  await page.getByRole('button', { name: 'Save template' }).click();
  await expect(notice(page, 'Python 3.13 was saved')).toBeVisible();

  // A template that nothing was made from can be deleted; the one that something was made from cannot.
  // The used default belongs to the system, so its manager acts require the explicit administrator mode.
  await page.getByRole('switch', { name: 'As admin: Off', exact: true }).click();
  await expect(page.getByRole('switch', { name: 'As admin: On', exact: true })).toBeChecked();
  await page.goto('/resources/templates/00000000-0000-4000-8026-000000000001?tab=danger');
  const usedName = await page.getByRole('heading', { level: 1 }).innerText();
  await page.getByRole('button', { name: words.resources.act.menuOf.replace('{{name}}', usedName), exact: true }).click();
  const deletion = page.getByRole('menuitem', { name: words.resources.act.delete, exact: true });
  await expect(deletion).toHaveAttribute('aria-disabled', 'true');
  await deletion.hover();
  await expect(page.getByText(/cannot be deleted/)).toBeVisible();
});

test('a host is closed to new environments, stops what runs on it, and shows its capacity', async ({ page }) => {
  await open(page, '/resources/hosts');
  await page.getByRole('link', { name: 'host-1' }).click();
  await expect(page.getByRole('heading', { name: 'host-1', level: 1 })).toBeVisible();
  // What the environments there declare is read against what the host has, and the page says how it is counted.
  await page.locator('[data-resource-details] > summary').click();
  await expect(page.getByRole('meter', { name: 'Memory' })).toBeVisible();
  await expect(page.getByText(/A declaration reserves nothing/)).toBeVisible();

  await page.getByRole('tab', { name: 'Settings' }).click();
  const taking = page.getByRole('switch', { name: words.hosts.settings.takeTitle, exact: true });
  await taking.click(); await expect(taking).not.toBeChecked();
  await expect(page.getByText(words.resourceDetails.layout.saved, { exact: true })).toBeVisible();
  const full = page.getByRole('combobox', { name: words.hosts.settings.fullTitle, exact: true });
  await full.click(); await page.getByRole('option', { name: words.hosts.whenFull.block, exact: true }).click();
  await expect(full).toContainText(words.hosts.whenFull.block); await expect(page.getByText(words.hosts.whenFull.own, { exact: true })).toBeVisible();
  // A choice is saved at once and one more while it is being saved is let go: the save is over when the fields of the page are free again.
  await expect(page.getByLabel('Name', { exact: true })).toBeEnabled();
  // The compact picker keeps keyboard selection; Enter commits the focused choice once.
  await full.focus(); await page.keyboard.press('Space'); await page.keyboard.press('End', { delay: KEY_HELD_MS }); await page.keyboard.press('Enter');
  await expect(page.getByText(words.hosts.settings.fullFollows.replace('{{value}}', words.hosts.whenFull.warn), { exact: true })).toBeVisible();

  await page.getByRole('tab', { name: /What runs on it/ }).click();
  await expect(page.getByRole('link', { name: 'Sandbox', exact: true })).toBeVisible();
  await page.getByRole('button', { name: /^Actions for Sandbox(?: · .+)?$/ }).click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete Sandbox?', exact: true });
  await dialog.getByLabel('Type Sandbox to confirm', { exact: true }).fill('Sandbox');
  await dialog.getByRole('button', { name: 'Delete Sandbox', exact: true }).click();
  await expect(notice(page, 'Sandbox will be deleted')).toBeVisible();
  await expect(page.getByText('Nothing runs on it')).toBeVisible();
});

test('a host is enrolled with a command, and seen when it connects', async ({ page }) => {
  await open(page, '/resources/hosts');
  await page.getByRole('button', { name: 'Add host' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add host' });
  await expect(dialog.getByRole('switch', { name: 'Full access: Off' })).not.toBeChecked();
  await expect(dialog.getByText(/Runs as root/u)).toBeVisible();
  await dialog.getByRole('button', { name: 'Show command' }).click();
  // The command for the system of this browser, with the single-use token in it.
  await expect(dialog.getByText(/oenr_/)).toBeVisible();
  await expect(dialog.locator('pre code')).not.toContainText('--delegate-policy');
  await expect(dialog.getByText('Waiting for the machine to connect…')).toBeVisible();

  // The host runs the command, and the dialog sees it come.
  await resourceControl('/enroll-host', { name: 'ci-host' });
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'ci-host', level: 1 })).toBeVisible();
  await expect(notice(page, words.hosts.add.connected)).toHaveCount(1);
  await expect(notice(page, words.hosts.add.connected)).toBeVisible();
});
