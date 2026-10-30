/** The permission page of a thing: a person added to a line and taken out, a page that stops following its place and is put back, a row that a lock above decides, a change refused because the page moved, the full form's rows, the people and the defaults of a place, a refusal with what is missing and the way on, and somebody from outside. */
import { expect, type Page, test } from '@playwright/test';

import { id, RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';
import { asAdmin } from './admin-mode-support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** A request only the fake API has, which steers what the access pages meet. */
async function steer(path: string, body: unknown = {}): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/access${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
}

test.beforeEach(async () => {
  await scenario();
});

const dialog = (page: Page) => page.getByRole('dialog', { name: /^Access to / });
const line = (page: Page, name: string) => dialog(page).getByRole('group', { name });

/** Signs in, goes to a place and opens its Share from the menu of its row in the left bar. */
async function share(page: Page, path = `/topic/${ACME.fixLogin}`, person: { login: string; password: string } = PEOPLE.ada, thing = 'Fix login'): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
  await page.getByRole('button', { name: `Actions for ${thing}` }).first().click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
  await expect(dialog(page)).toBeVisible();
}

/** On a place the form opens on the place with everything in it, which gives a role; these tests name people on the place's own page. */
async function onlyHere(page: Page): Promise<void> {
  const options = dialog(page).getByRole('button', { name: 'Options', exact: true });
  if (await options.getAttribute('aria-expanded') !== 'true') await options.click();
  await dialog(page).getByRole('combobox', { name: 'What is shared' }).click();
  await page.getByRole('option', { name: 'Place only' }).click();
}

/** Names a person in a line through the field of the short form. */
async function name(page: Page, who: string, choice: string): Promise<void> {
  const field = dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' });
  await field.fill(who.slice(0, 4));
  await page.getByRole('option', { name: new RegExp(who) }).click();
  await onlyHere(page);
  await dialog(page).getByRole('combobox', { name: 'What they can do' }).click();
  await page.getByRole('option', { name: choice }).click();
  await dialog(page).getByRole('button', { name: 'Add', exact: true }).click();
}

test('Share opens the page with the field that adds people in focus, and shows the owners, the three lines and who else reaches it', async ({ page }) => {
  await share(page);
  await expect(dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' })).toBeFocused();
  await expect(dialog(page).getByRole('group', { name: 'Owners' })).toContainText('Ada Lovelace');
  await expect(dialog(page).getByRole('group', { name: 'Owners' })).toContainText('Primary owner');
  // As shipped a Developer manages the topic and everybody who can see it reads it.
  await expect(line(page, 'Can manage')).toContainText('Developers');
  await expect(line(page, 'Can view')).toContainText('Everyone in Fix login');
  await expect(line(page, 'Can view')).toContainText('as the system decides');
  await expect(dialog(page).getByText('Admins of Web, Research, Acme, and the system')).toBeVisible();
});

test('a person is added to a line, shows as pending until the server answers, and is taken out again after being told whom that reaches', async ({ page }) => {
  // Where the server does not say whom a change reaches, the dialog says so.
  await steer('/serve', { previews: false });
  await share(page);
  await name(page, 'Grace Hopper', 'Can interact');
  await expect(line(page, 'Can interact')).toContainText('Grace Hopper');
  await expect(dialog(page).getByText('Custom', { exact: true }).first()).toBeVisible();

  await line(page, 'Can interact').getByRole('button', { name: 'Change Grace Hopper’s access' }).click();
  await page.getByRole('menuitem', { name: 'Take Grace Hopper out' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Take Grace Hopper out?' });
  // The server cannot say yet whom this reaches, and the dialog does not pretend that it can.
  await expect(confirm).toContainText('The server cannot say yet whom this reaches');
  await expect(confirm).not.toContainText('Nobody gains or loses anything');
  await confirm.getByRole('button', { name: 'Take out Grace Hopper' }).click();
  await expect(dialog(page).locator('[data-access-roster]')).not.toContainText('Grace Hopper');
});

test('where the server says whom a change reaches, the dialog names them before asking once', async ({ page }) => {
  await steer('/serve', { previews: true });
  await share(page);
  await dialog(page).getByRole('button', { name: 'Make private', exact: true }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Make “Fix login” private?' });
  await expect(confirm).toContainText('Only you and the people named here. It stops following Web.');
  await expect(confirm.getByText(/people lose access/)).toBeVisible();
  await expect(confirm).toContainText('Grace Hopper');
  await confirm.getByRole('button', { name: 'Make Fix login private' }).click();
  await expect(dialog(page).getByText('Private', { exact: true }).first()).toBeVisible();
  await expect(dialog(page).getByRole('group', { name: 'Owners', exact: true })).toContainText('Ada Lovelace');
});

test('a page stops following its place for one row and follows it again, and the whole page is made private and put back', async ({ page }) => {
  await share(page);
  await dialog(page).getByRole('button', { name: 'Permissions' }).click();
  const cell = dialog(page).getByRole('group', { name: 'View Chat' });
  await expect(cell).toContainText('Everyone here');

  await dialog(page).getByRole('button', { name: 'View Chat: more' }).click();
  await page.getByRole('menuitem', { name: 'Stop following Web' }).click();
  await page.getByRole('alertdialog', { name: 'Stop following for View Chat?' }).getByRole('button', { name: 'Stop following Web' }).click();
  await expect(cell).toContainText('Its owners only');
  await expect(cell).not.toContainText('Everyone here');

  await dialog(page).getByRole('button', { name: 'View Chat: more' }).click();
  await page.getByRole('menuitem', { name: 'Follow Web again' }).click();
  await expect(cell).toContainText('Everyone here');

  await dialog(page).getByRole('button', { name: 'Back to the short form' }).click();
  await dialog(page).getByRole('button', { name: 'Make private', exact: true }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Make Fix login private' }).click();
  await expect(dialog(page).locator('[data-access-roster]')).not.toContainText('Everyone in Fix login');
  await expect(dialog(page).getByRole('group', { name: 'Owners', exact: true })).toContainText('Ada Lovelace');

  // Put back: the page follows its place again, and what it named stops being named.
  await dialog(page).getByRole('button', { name: 'As Web decides' }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Follow Web again' }).click();
  await expect(line(page, 'Can view')).toContainText('Everyone in Fix login');
});

test('a row that a lock above decides shows the lock and where it was set, and has no menu', async ({ page }) => {
  await steer('/lock', { place: ACME.org, permission: 'topic.view' });
  await share(page);
  await dialog(page).getByRole('button', { name: 'Permissions' }).click();
  const cell = dialog(page).getByRole('group', { name: 'View Topic' });
  await expect(cell).toContainText('Acme decides this');
  await expect(dialog(page).getByRole('button', { name: 'View Topic: more' })).toHaveCount(0);
  await expect(dialog(page).getByRole('button', { name: 'Add to View Topic' })).toHaveCount(0);
  // The rows beside it are the page's own to change.
  await expect(dialog(page).getByRole('button', { name: 'View Chat: more' })).toBeVisible();
});

test('a change made against a page that somebody else changed is not made: the newer page is shown, and the change can be made again against it', async ({ page }) => {
  await share(page);
  // Vera is named by Grace while Ada has the page open.
  await steer('/touch', { as: 'grace', path: 'nodes', id: ACME.fixLogin, change: { name: [{ choice: 'full_view', holders: [{ kind: 'user', id: id(1, 8) }] }] } });
  await name(page, 'Margaret Hamilton', 'Can view');

  await expect(dialog(page).getByText('This page changed while you were looking at it')).toBeVisible();
  await expect(dialog(page).getByText('Not made: Adding Margaret Hamilton')).toBeVisible();
  // The page is shown as it is now, with Vera on it, and Margaret is not.
  await expect(line(page, 'Can view')).toContainText('Vera Rubin');
  await expect(line(page, 'Can view')).not.toContainText('Margaret Hamilton');

  await dialog(page).getByRole('button', { name: 'Make it again' }).click();
  await expect(line(page, 'Can view')).toContainText('Margaret Hamilton');
  await expect(line(page, 'Can view')).toContainText('Vera Rubin');
  await expect(dialog(page).getByText('This page changed while you were looking at it')).toHaveCount(0);
});

test('the full form lays every row out by area, with the advanced rows and Administration under it', async ({ page }) => {
  await share(page);
  await dialog(page).getByRole('button', { name: 'Permissions' }).click();
  const grid = dialog(page).getByRole('table', { name: 'Permissions by area' });
  for (const area of ['Chat', 'Documents', 'Topic']) await expect(grid.getByRole('rowheader', { name: area })).toBeVisible();
  // Documents are edited, not interacted with.
  await expect(grid.getByText('(Edit)')).toBeVisible();

  await dialog(page).locator('summary', { hasText: 'Advanced' }).click();
  for (const row of ['New documents', 'Comment', 'Archive Topic', 'Go past limits', 'Share outside']) await expect(dialog(page).getByText(row, { exact: true }).first()).toBeVisible();
  await dialog(page).locator('summary', { hasText: 'Administration' }).click();
  for (const row of ['Manage Settings', 'View Usage', 'Usage summary', 'View Audit', 'Manage Access']) await expect(dialog(page).getByText(row, { exact: true }).first()).toBeVisible();

  await dialog(page).getByRole('button', { name: 'History of this page' }).click();
  await expect(dialog(page).getByText('Ada Lovelace made the topic Fix login')).toBeVisible();
});

test('the owners are named, one is made first, and the last person cannot be taken out', async ({ page }) => {
  await share(page);
  const owners = dialog(page).getByRole('group', { name: 'Owners' });
  await owners.getByRole('button', { name: 'Add owner' }).click();
  await page.getByRole('combobox', { name: 'Add an owner' }).fill('gra');
  await page.getByRole('option', { name: /Grace Hopper/ }).click();
  await expect(owners).toContainText('Grace Hopper');

  await owners.getByRole('button', { name: 'Owner Grace Hopper' }).click();
  await page.getByRole('menuitem', { name: 'Make primary owner' }).click();
  await expect(owners.getByRole('button', { name: 'Owner Grace Hopper' })).toContainText('Primary owner');
  await expect(owners.getByRole('button', { name: 'Owner Ada Lovelace' })).not.toContainText('Primary owner');

  await owners.getByRole('button', { name: 'Owner Ada Lovelace' }).click();
  await page.getByRole('menuitem', { name: 'Remove' }).click();
  // Taking oneself out says so, in other words than taking somebody else out.
  await page.getByRole('alertdialog', { name: 'Stop owning “Fix login”?' }).getByRole('button', { name: 'Take me out' }).click();
  await expect(owners).not.toContainText('Ada Lovelace');

  // Grace is the only person left: the menu says who to name first.
  await owners.getByRole('button', { name: 'Owner Grace Hopper' }).click();
  await expect(page.getByRole('menuitem', { name: /Remove/ })).toHaveAttribute('aria-disabled', 'true');
  await expect(page.getByRole('menuitem', { name: /Remove/ })).toContainText('Name another owner first');
});

test('a person is given a role in a place, changed and taken away, and those who hold one from above are summed up', async ({ page }) => {
  await share(page, `/project/${ACME.web}`, PEOPLE.ada, 'Web');
  await dialog(page).getByRole('tab', { name: 'People' }).click();
  await expect(dialog(page).getByRole('button', { name: /Developers of Acme · 3 people/ })).toBeVisible();

  await dialog(page).getByRole('button', { name: 'Add people…' }).click();
  const give = page.getByRole('dialog', { name: 'Give a role in Web' });
  await give.getByRole('combobox', { name: 'Person or group' }).fill('marg');
  await page.getByRole('option', { name: /Margaret Hamilton/ }).click();
  await give.getByRole('combobox', { name: 'Role' }).click();
  await page.getByRole('option', { name: 'Developer' }).click();
  await give.getByRole('button', { name: 'Give the role' }).click();

  const table = dialog(page).getByRole('table', { name: 'Roles held in Web' });
  await expect(table).toContainText('Margaret Hamilton');
  await table.getByRole('combobox', { name: 'Role of Margaret Hamilton' }).click();
  await page.getByRole('option', { name: 'Admin' }).click();
  await expect(table.getByRole('combobox', { name: 'Role of Margaret Hamilton' })).toContainText('Admin');

  await table.getByRole('button', { name: 'Margaret Hamilton: more' }).click();
  await page.getByRole('menuitem', { name: 'Take the role away…' }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Take the role from Margaret Hamilton' }).click();
  await expect(table).toHaveCount(0);
  await expect(dialog(page).getByText('Nobody holds a role here yet')).toBeVisible();
});

test('what a place gives to what is made inside it is a grid for each kind with two for threads, and a row can be locked after being told whom that reaches', async ({ page }) => {
  await share(page, `/project/${ACME.web}`, PEOPLE.ada, 'Web');
  await dialog(page).getByRole('tab', { name: 'Defaults' }).click();
  await expect(dialog(page).getByText(/pages follow these\./)).toBeVisible();
  await dialog(page).getByRole('radio', { name: 'Threads' }).click();
  await expect(dialog(page).getByText('In a managed environment')).toBeVisible();
  await expect(dialog(page).getByText('On a person’s machine')).toBeVisible();

  // On a person's machine nobody but its owner works with a thread, until the default says otherwise.
  const machine = dialog(page).getByRole('region', { name: 'On a person’s machine' });
  await expect(machine.getByRole('group', { name: 'Interact Thread' })).toContainText('—');

  await dialog(page).getByRole('radio', { name: 'Topics' }).click();
  await dialog(page).getByRole('button', { name: 'View Topic: more' }).click();
  await page.getByRole('menuitem', { name: 'Lock' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Lock View Topic?' });
  await expect(confirm).toContainText('Everything inside follows this row exactly');
  await confirm.getByRole('button', { name: 'Lock it' }).click();
  await expect(dialog(page).getByRole('group', { name: 'View Topic' })).toContainText('Locked here');
  await dialog(page).getByRole('button', { name: 'View Topic: more' }).click();
  await page.getByRole('menuitem', { name: 'Unlock' }).click();
  await expect(dialog(page).getByRole('group', { name: 'View Topic' })).not.toContainText('Locked here');
});

test('Share shows the inherited holder, its level and its actual people without an unavailable View as action', async ({ page }) => {
  await share(page);
  await expect(dialog(page).getByRole('button', { name: 'View as…' })).toHaveCount(0);
  const holder = dialog(page).getByRole('button', { name: 'Developers of Fix login', exact: true });
  const row = dialog(page).locator('li[data-access-level]').filter({ has: holder });
  await expect(row).toContainText('Inherited from Web');
  await expect(row).toContainText('Manage');
  await holder.click();
  await expect(row).toContainText('Grace Hopper');
  await expect(row).not.toContainText('acme.grace');
});

test('somebody from outside the org is named by whoever may, with who they are marked, and shown disabled with who can for everybody else', async ({ page }) => {
  await share(page);
  const field = dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' });
  await field.fill('zoe');
  await expect(page.getByRole('option', { name: /Zoe Outsider/ })).toContainText('Not in this org');
  await page.getByRole('option', { name: /Zoe Outsider/ }).click();
  await onlyHere(page);
  await expect(dialog(page).getByText('Zoe Outsider is not in this org and gets access to Fix login only.')).toBeVisible();
  await dialog(page).getByRole('button', { name: 'Add', exact: true }).click();
  await expect(line(page, 'Can view')).toContainText('Outside');

  await steer('/no-outsiders', { username: 'ada', off: true });
  await page.reload();
  await page.getByRole('button', { name: 'Actions for Fix login' }).first().click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
  await dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' }).fill('zoe');
  const option = page.getByRole('option', { name: /Zoe Outsider/ });
  await expect(option).toHaveAttribute('aria-disabled', 'true');
  await expect(option).toContainText('Developers can add someone from outside.');
});

test('an address nobody is registered with is invited to this one place, and the link is shown to copy', async ({ page }) => {
  await share(page);
  await onlyHere(page);
  await dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' }).fill('dana@example.org');
  await page.getByRole('option', { name: /Send an invitation to dana@example.org/ }).click();
  const invite = page.getByRole('dialog', { name: 'Invite to “Fix login”' });
  await expect(invite).toContainText('The invitation is for this one place.');
  await invite.getByRole('button', { name: 'Make the invitation' }).click();
  await expect(invite).toContainText('Not sent: this org has no way to send mail');
  await expect(invite.getByRole('textbox', { name: 'Link' })).toHaveValue(/\/invite\//);
});

test('somebody who reads a page may add people up to what they hold, and reads the rest of it with nothing to change', async ({ page }) => {
  await share(page, `/topic/${ACME.fixLogin}`, PEOPLE.vera, 'Fix login');
  await expect(dialog(page).getByText('You can add people up to what you hold, and take back those you added.')).toBeVisible();
  await expect(dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' })).toBeVisible();
  await expect(dialog(page).getByRole('button', { name: 'Add owner' })).toHaveCount(0);
  await expect(line(page, 'Can manage')).toContainText('Developers');
  // How the page stands is told in words, with nothing to press.
  await expect(dialog(page).getByRole('button', { name: 'How the page stands' })).toHaveCount(0);
  await expect(dialog(page).getByText('As Web decides', { exact: true })).toBeVisible();
  await dialog(page).getByRole('button', { name: 'Permissions' }).click();
  await expect(dialog(page).getByRole('button', { name: /: more$/ })).toHaveCount(0);
});

test('at the width of a phone the dialog fits the window, with the names and the field that adds people inside it', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await share(page);
  const box = await dialog(page).boundingBox();
  expect(box?.x ?? -1).toBeGreaterThanOrEqual(0);
  expect(box === null ? Infinity : box.x + box.width).toBeLessThanOrEqual(390);
  // Nothing inside has to scroll sideways, and the long name of who else reaches it is cut, not pushed out.
  const scrolls = await dialog(page).evaluate((element) => Array.from(element.querySelectorAll('*')).some((inner) => inner.scrollWidth > inner.clientWidth + 1 && getComputedStyle(inner).overflowX === 'visible' && inner.getBoundingClientRect().right > element.getBoundingClientRect().right));
  expect(scrolls).toBe(false);
  await expect(dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' })).toBeVisible();
  await expect(dialog(page).getByRole('button', { name: 'Add owner' })).toBeVisible();
});

test('a refusal says what is missing and whom to ask, and asking through Orch says so where the server does not take requests yet', async ({ page }) => {
  await steer('/serve', { requests: false });
  await steer('/refuse', { username: 'vera', node: ACME.web, permissions: ['project.view'] });
  await signIn(page, PEOPLE.vera);
  await page.goto(`/project/${ACME.web}`);

  await expect(page.getByRole('heading', { name: /You don.t have access/ })).toBeVisible();
  await expect(page.getByText('The project “Web”')).toBeVisible();
  await expect(page.getByText('View Project')).toBeVisible();
  await expect(page.getByText('Ask Ada Lovelace.')).toBeVisible();
  await expect(page.getByRole('button', { name: /Open as an admin/ })).toHaveCount(0);

  // The three choices, with the least that covers what was missing chosen, and a line for a reason.
  await expect(page.getByRole('radio', { name: 'View', exact: true })).toBeChecked();
  await page.getByRole('textbox', { name: /Why do you need it/ }).fill('I review the web work');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText('Asking for access through Orch is not available yet. Ask Ada Lovelace yourself.')).toBeVisible();

  // Where the server takes requests, the request is sent, shows where it stands, and can be taken back.
  await steer('/serve', { requests: true });
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByText('Your request was sent to Ada Lovelace and waits to be decided.')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Take the request back' })).toBeVisible();
});

test('a current refusal never offers an administrator a self-grant, including while the session mode is on', async ({ page }) => {
  await steer('/refuse', { username: 'ada', node: ACME.web, permissions: ['project.view'] });
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}`);

  await expect(page.getByRole('heading', { name: /You don.t have access/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /Open as an admin/ })).toHaveCount(0);
  await asAdmin(page, true);
  await expect(page.getByRole('heading', { name: /You don.t have access/ })).toBeVisible();
  await expect(page.getByRole('alertdialog', { name: /Open as an admin of/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Open as an admin/ })).toHaveCount(0);
});

/** Opens Share from the thread's own header. */
async function shareThread(page: Page): Promise<void> {
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Share', exact: true }).click();
}

/** Opens the permission page of a resource from the Access tab of its own page. */
async function openFromResource(page: Page, path: string): Promise<void> {
  await page.goto(path);
  await page.getByRole('tab', { name: 'Access' }).click();
  await page.getByRole('button', { name: 'Permissions' }).click();
}

test('a thread on a person’s machine says what naming somebody there includes, before it is done', async ({ page }) => {
  await steer('/connected', { thread: ACME.mainThread, on: true });
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${ACME.mainThread}`);
  await shareThread(page);
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page)).toContainText('This thread runs on Ada Lovelace’s machine (Workspace)');
  await dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' }).fill('gra');
  await page.getByRole('option', { name: /Grace Hopper/ }).click();
  await dialog(page).getByRole('combobox', { name: 'What they can do' }).click();
  await page.getByRole('option', { name: 'Can manage' }).click();
  await expect(dialog(page)).toContainText('They can make the agent act on Ada Lovelace’s machine and reach its files and a terminal there');
});

test('the Access tab of each kind of resource opens the permission page of that resource', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  const resources = [
    [`/resources/providers/${RUNS_ON.openAiKey}`, 'OpenAI key'],
    [`/resources/agents/${RUNS_ON.codex}`, 'Codex'],
    [`/resources/environments/${RUNS_ON.workspace}`, 'Workspace'],
    [`/resources/hosts/${RUNS_ON.buildHost}`, 'build-host'],
    [`/resources/templates/${RUNS_ON.defaultTemplate}`, 'Default'],
  ] as const;
  for (const [path, name] of resources) {
    await openFromResource(page, path);
    await expect(page.getByRole('dialog', { name: `Access to “${name}”` })).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toHaveCount(0);
  }
});

test('what goes with a permission is given in the same step where the caller manages it, and the page of a provider is drawn without the parts that are not served', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${ACME.mainThread}`);
  await shareThread(page);
  await expect(dialog(page)).toContainText('Talking to the agent needs gpt-5.4 · high on OpenAI key');
  await dialog(page).getByRole('checkbox', { name: 'Include it for everyone who works in Main thread' }).click();
  await expect(dialog(page).getByRole('checkbox', { name: 'Included for everyone who works in Main thread' })).toBeChecked();

  // A provider's page: the page itself is there, and the history, the people behind a chip and what a person holds say that they are not served yet.
  await page.keyboard.press('Escape');
  await openFromResource(page, `/resources/providers/${RUNS_ON.openAiKey}`);
  const provider = page.getByRole('dialog', { name: 'Access to “OpenAI key”' });
  await expect(provider).toBeVisible();
  await expect(provider.getByRole('group', { name: 'Can use' })).toBeVisible();
  await expect(provider.getByRole('button', { name: 'View as…' })).toHaveCount(0);
  await expect(provider.getByRole('region', { name: 'Who has access' })).toBeVisible();
});
