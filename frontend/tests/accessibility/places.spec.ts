/** Every screen of places, asked in both themes whether anything on it is a serious barrier: with what opens above it open, for somebody who may do everything and for somebody who may only read. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

test('an org’s home: its overview, its members with their invitations, and its Activity', async ({ page }) => {
  await open(page, '/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
  await expect(page.getByTestId('main').getByRole('heading', { name: 'Projects' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('tab', { name: /Members/ }).click();
  await expect(page.getByRole('table', { name: 'Members' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('tab', { name: 'Activity' }).click();
  await expect(page.getByText('Ada Lovelace made the org Research')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a department’s home, an org of its own that is empty, and an org that somebody who may only read sees', async ({ page }) => {
  await open(page, '/org/acme.research');
  await expect(page.getByRole('heading', { name: 'Research' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto('/org/ada');
  await expect(page.getByRole('heading', { name: 'Ada Lovelace', level: 1 })).toBeVisible();
  await expect(page.getByText('Nothing here yet')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto('/org/acme');
  await signOutAndIn(page, PEOPLE.vera);
  await page.goto('/org/acme');
  await expect(page.getByRole('button', { name: 'New project', exact: true })).toHaveAttribute('aria-disabled', 'true');
  await expectAccessibleInBothThemes(page);
});

async function signOutAndIn(page: Page, person: { login: string; password: string }): Promise<void> {
  await page.getByRole('button', { name: 'Your account' }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await page.waitForURL(/\/login/);
  await page.getByLabel('Username or email address').fill(person.login);
  await page.getByLabel('Password', { exact: true }).fill(person.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL(/\/org\//);
}

test('a project as a list, as a board and as Activity, and its filters open', async ({ page }) => {
  await open(page, `/project/${ACME.web}`);
  await expect(page.getByRole('table', { name: 'Topics' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Tags' }).click();
  await expect(page.getByRole('checkbox', { name: 'bug' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('combobox', { name: 'Attention' }).click();
  await expect(page.getByRole('option', { name: 'Blocked' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="listbox"]');
  await page.keyboard.press('Escape');

  await page.getByRole('radio', { name: 'Board' }).click();
  await expect(page.getByRole('region', { name: /Needs attention/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('radio', { name: 'Activity' }).click();
  await expect(page.getByText('Ada Lovelace made the topic Fix login')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('a topic’s tabs, one after the other', async ({ page }) => {
  await open(page, `/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The tabs of the topic, not the ones of the right bar that have names of their own.
  const tabs = page.getByTestId('main');
  for (const tab of ['Threads', 'Environments', 'Documents', 'Activity', 'Settings']) {
    await tabs.getByRole('tab', { name: new RegExp(`^${tab}`) }).click();
    await expect(tabs.getByRole('tab', { name: new RegExp(`^${tab}`) })).toHaveAttribute('aria-selected', 'true');
    await expectAccessibleInBothThemes(page);
  }
});

test('the dialogs that make and change places, open', async ({ page }) => {
  await open(page, '/org/acme');

  await page.getByRole('button', { name: 'Actions for Acme', exact: true }).last().click();
  await page.getByRole('menuitem', { name: 'New department…', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'New department in Acme' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'New project', exact: true }).first().click();
  await expect(page.getByRole('dialog', { name: 'New project in Acme' })).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Create project' }).click();
  // Said under the field, and by the field.
  await expect(page.getByRole('dialog').getByText('Give it a name.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.goto(`/org/acme?tab=members`);
  await page.getByRole('button', { name: 'Invite…' }).click();
  await expect(page.getByRole('dialog', { name: 'Invite to Acme' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('dialog').getByRole('textbox', { name: 'Email address' }).fill('dana@example.org');
  await page.getByRole('dialog').getByRole('button', { name: 'Create invitation' }).click();
  await expect(page.getByRole('dialog').getByRole('textbox', { name: 'Link' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.goto(`/project/${ACME.web}`);
  await page.getByRole('button', { name: 'Actions for Web' }).first().click();
  await page.getByRole('menuitem', { name: 'Move to…' }).click();
  const move = page.getByRole('dialog', { name: /^Move the project Web, in / });
  await move.getByRole('radio', { name: 'Acme', exact: true }).check();
  await expect(move.getByText('Nothing inside what is moved is moved or renamed.').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.goto(`/topic/${ACME.upgradeDeps}`);
  await page.getByRole('button', { name: 'Actions for Upgrade deps' }).first().click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  await expect(page.getByRole('alertdialog', { name: /^Delete the topic Upgrade deps, in .+\?$/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'Actions for Upgrade deps' }).first().click();
  await page.getByRole('menuitem', { name: 'Recently deleted…' }).click();
  await expect(page.getByRole('dialog', { name: /^Recently deleted in the topic Upgrade deps, in / })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the left bar with a menu open, starred places, the menu of its org and of the rail', async ({ page }) => {
  await open(page, `/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: 'Star', exact: true }).click();
  await expect(page.getByText('Starred', { exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('treeitem', { name: /^Web / }).click({ button: 'right' });
  await expect(page.getByRole('menu', { name: /^Actions for Web(?: · .+)?$/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('navigation', { name: 'Navigation' }).getByRole('button', { name: /^Actions for Acme(?: · .+)?$/ }).click();
  await expect(page.getByRole('menuitem', { name: 'New project…' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.keyboard.press('Escape');

  await page.getByRole('button', { name: 'New org or join one' }).click();
  await expect(page.getByRole('menuitem', { name: 'New org…' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('the places on a small screen, and in Chinese at the compact density', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await open(page, '/org/acme');
  await expect(page.getByRole('heading', { name: 'Acme' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('table', { name: 'Topics' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto(`/project/${ACME.web}?view=board`);
  await expect(page.getByRole('region', { name: /Needs attention/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The language and the density are the person's own, kept on the server and chosen on Interface.
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto('/settings/interface');
  await page.getByRole('radio', { name: 'Compact' }).click();
  await page.getByRole('combobox', { name: 'Language' }).click();
  await page.getByRole('option', { name: '简体中文' }).click();
  await expect(page.locator('html')).toHaveAttribute('data-density', 'compact');
  await page.goto('/org/acme');
  await expect(page.getByTestId('main').getByRole('heading', { name: '项目' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.goto(`/project/${ACME.web}?view=board`);
  await expect(page.getByTestId('main').getByRole('heading', { name: '需要关注' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.goto(`/topic/${ACME.fixLogin}`);
  await expect(page.getByRole('heading', { name: 'Fix login' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
