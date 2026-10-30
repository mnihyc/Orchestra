/** One person in two orgs whose departments carry the same names, and acts in a dialog that leave every view beneath and beside it fresh without a reload. Acme and Beta each hold a department dep2 with a project web and its topic login: the same names are allowed in two orgs and two parents and refused only beside themselves, with the org that has the name and a free one offered; every place that shows such a name says where it is. On the permission page, a role given in its form and taken away in its People shows at once in the People, in the page's lines and in the full form. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

interface Answer {
  status: number;
  body: Record<string, unknown>;
}

/** Asks the fake API as a person, with the token their sign-in gives: how a test makes what the screens then show. */
async function as(person: { login: string; password: string }): Promise<(path: string, body: unknown) => Promise<Answer>> {
  const login = await fetch(`${DOUBLE}/api/v1/auth/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ login: person.login, password: person.password }) });
  if (!login.ok) throw new Error(`the fake API refused the sign-in: ${login.status}`);
  const { access_token: token } = (await login.json()) as { access_token: string };
  return async (path, body) => {
    const answer = await fetch(`${DOUBLE}/api/v1${path}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}`, 'Idempotency-Key': crypto.randomUUID() },
      body: JSON.stringify(body),
    });
    return { status: answer.status, body: (await answer.json()) as Record<string, unknown> };
  };
}

interface Places {
  org: string;
  dep2: string;
  web: string;
  login: string;
}

/** Beta beside Acme, and in each a department dep2 holding a project web with its topic login: all of it allowed. */
async function twoOrgs(): Promise<{ acme: Places; beta: Places }> {
  const post = await as(PEOPLE.ada);
  const made = async (path: string, body: unknown): Promise<string> => {
    const answer = await post(path, body);
    if (answer.status >= 300) throw new Error(`the fake API refused ${path}: ${answer.status} ${JSON.stringify(answer.body)}`);
    const topic = answer.body.topic as { id: string } | undefined;
    return topic?.id ?? (answer.body.id as string);
  };
  const beta = await made('/orgs', { name: 'Beta', slug: 'beta' });
  const inside = async (org: string): Promise<Places> => {
    const dep2 = await made(`/orgs/${org}/orgs`, { name: 'dep2', slug: 'dep2' });
    const web = await made(`/orgs/${dep2}/projects`, { name: 'web' });
    const login = await made(`/projects/${web}/topics`, { name: 'login' });
    return { org, dep2, web, login };
  };
  return { acme: await inside(ACME.org), beta: await inside(beta) };
}

test.beforeEach(async () => {
  await scenario();
});

test('the same names are allowed in two orgs and two parents, and refused beside themselves with the org that has the name and a free one', async ({ page }) => {
  const { acme } = await twoOrgs();
  const post = await as(PEOPLE.ada);
  // A department dep2 inside Acme's dep2 is another address: allowed.
  expect((await post(`/orgs/${acme.dep2}/orgs`, { name: 'dep2', slug: 'dep2' })).status).toBe(200);
  // Beside Acme's dep2 one address would name two orgs: refused, naming it and offering another.
  const again = await post(`/orgs/${ACME.org}/orgs`, { name: 'Department two', slug: 'dep2' });
  expect(again.status).toBe(409);
  expect(again.body.code).toBe('conflict.exists');
  expect((again.body.existing as { name: string }).name).toBe('dep2');
  expect(again.body.suggestion).toBe('dep2-2');

  // The screen says the same, and takes the free short name when asked.
  await signIn(page);
  await page.goto('/org/acme');
  await page.getByRole('button', { name: 'New department', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'New department in Acme' });
  await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('dep2');
  await expect(dialog.getByRole('textbox', { name: 'Short name' })).toHaveValue('dep2');
  await dialog.getByRole('button', { name: 'Create department' }).click();
  await expect(dialog.getByText('dep2 has the short name dep2 here already, and two orgs of one place cannot share an address. Choose another.')).toBeVisible();
  await dialog.getByRole('button', { name: 'Use dep2-2' }).click();
  await expect(dialog.getByRole('textbox', { name: 'Short name' })).toHaveValue('dep2-2');
  await dialog.getByRole('button', { name: 'Create department' }).click();
  // A display name never needs to be unique: Acme now holds two orgs called dep2, at two addresses.
  await expect(page).toHaveURL(/\/org\/acme\.dep2-2$/);
});

/** Opens a place's permission page from the menu of its row in the left bar. */
async function share(page: Page, path: string, thing: string): Promise<void> {
  await page.goto(path);
  await page.getByRole('button', { name: `Actions for ${thing}` }).first().click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
}

test('a place whose name a place of the other org has is named with where it is, and one of its own name alone', async ({ page }) => {
  const { acme, beta } = await twoOrgs();
  await signIn(page);
  // The browser's tab and the workbench's: "dep2 · Acme" against "dep2 · Beta".
  await page.goto(`/org/acme.dep2`);
  await expect(page).toHaveTitle(/^dep2 · Acme · /);
  await page.goto(`/org/beta.dep2`);
  await expect(page).toHaveTitle(/^dep2 · Beta · /);
  await page.goto(`/topic/${acme.login}`);
  await expect(page).toHaveTitle(/^login · Acme · /);
  await page.goto(`/project/${beta.web}`);
  await expect(page).toHaveTitle(/^web · Beta · /);
  // The way down to it says the rest, as it always did.
  await expect(page.getByRole('navigation', { name: 'Where this is' })).toContainText('Beta');
  // The permission page of one of them says which.
  await share(page, `/topic/${acme.login}`, 'login');
  await expect(page.getByRole('dialog', { name: 'Access to “login · Acme”' })).toBeVisible();
  await page.keyboard.press('Escape');
  // A name nobody else has is said alone.
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page).toHaveTitle(/^Fix login · /);
  // The rail names both orgs, which have names of their own.
  const rail = page.getByRole('navigation', { name: 'Orgs and your own places' });
  await expect(rail.getByRole('link', { name: 'Acme', exact: true })).toBeVisible();
  await expect(rail.getByRole('link', { name: 'Beta', exact: true })).toBeVisible();
});

test('a role given in the permission page and taken away in its People shows at once in the People and in the page, without a reload', async ({ page }) => {
  await signIn(page);
  await share(page, `/topic/${ACME.fixLogin}`, 'Fix login');
  const dialog = page.getByRole('dialog', { name: /^Access to / });
  // Grace is given a role in the place with everything in it: the form's first reach.
  await dialog.getByRole('combobox', { name: 'Add people, groups or roles' }).fill('Grac');
  await page.getByRole('option', { name: /Grace Hopper/ }).click();
  await dialog.getByRole('combobox', { name: 'What they can do' }).click();
  await page.getByRole('option', { name: 'Can interact' }).click();
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();

  // The People beside the form list her at once.
  await dialog.getByRole('tab', { name: 'People' }).click();
  const roles = dialog.getByRole('table', { name: 'Roles held in Fix login' });
  await expect(roles).toContainText('Grace Hopper');
  const asked: string[] = [];
  page.on('request', (request) => {
    if (request.method() === 'GET') asked.push(new URL(request.url()).pathname);
  });

  // Taken away there, the role leaves the People, and the page it gave a line on is asked again: no reload.
  await roles.getByRole('button', { name: 'Grace Hopper: more' }).click();
  await page.getByRole('menuitem', { name: 'Take the role away…' }).click();
  await page.getByRole('alertdialog', { name: 'Take the role away from Grace Hopper?' }).getByRole('button', { name: 'Take the role from Grace Hopper' }).click();
  await expect(roles).not.toContainText('Grace Hopper');
  await dialog.getByRole('tab', { name: 'This topic' }).click();
  await expect.poll(() => asked.some((path) => path.endsWith(`/nodes/${ACME.fixLogin}/permissions`))).toBe(true);
  await expect(dialog.getByRole('group', { name: 'Can interact' })).not.toContainText('Grace Hopper');
});
