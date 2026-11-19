/** The screens of giving, asked in both themes whether anything on them is a serious barrier: the short form of a place with what it covers, the Agents tab and its dialog, a request waiting for its approver, a copy past the limit, the form for several things, and an environment's page with its doors. */
import { expect, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { add, dialog, line, reach, shareAs, stage, steer, tab, ZOE } from '../e2e/give-support.ts';
import { ACME, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

const web = `/project/${ACME.web}`;
const fixLogin = `/topic/${ACME.fixLogin}`;

test('the short form of a place: what it is shared with, what that covers, and a hint opened', async ({ page }) => {
  await shareAs(page, PEOPLE.dora, web, 'Web');
  await expect(dialog(page)).toContainText('Covers 3 topics and folders');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await stage(page, 'Zoe Outsider', 'zoe');
  await expect(dialog(page).getByRole('checkbox', { name: /Send Zoe Outsider a message/ })).toBeVisible();
  await reach(page, 'Only Web itself');
  await expect(dialog(page)).toContainText('Nothing inside Web is opened');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  // The one hint of the application, opened beside what is shared.
  await dialog(page).getByRole('button', { name: /^Help: What sharing a place gives$/ }).click();
  await expect(page.getByText('Sharing a place with everything in it gives a role there')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a reader who shares up to what they hold, and a place that keeps sharing to its owners', async ({ page }) => {
  await shareAs(page, PEOPLE.vera, web, 'Web');
  await expect(dialog(page)).toContainText('You are a Viewer of Web');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await steer('/holders-share', { place: ACME.web, allowed: false });
  await page.reload();
  await page.getByRole('button', { name: 'Actions for Web' }).first().click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
  await expect(dialog(page)).toContainText('Sharing in Web is with a thing’s owners');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('the Agents tab with a share, and the dialog that shares with agents, by area and as a copy', async ({ page }) => {
  await shareAs(page, PEOPLE.ada, fixLogin, 'Fix login');
  await tab(page, 'Agents');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog(page).getByRole('button', { name: 'Share with agents…' }).click();
  const form = page.getByRole('dialog', { name: 'Share “Fix login” with agents' });
  await form.getByRole('combobox', { name: 'Whose agents' }).fill('Web');
  await page.getByRole('option', { name: /^Web/ }).click();
  await form.getByRole('button', { name: 'Permissions' }).click();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await form.getByRole('button', { name: /^Let the agents of Web/ }).click();

  await expect(dialog(page).getByRole('list', { name: 'Shared with agents' })).toContainText('Agents of Web');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog(page).getByRole('list', { name: 'Shared with agents' }).getByRole('combobox', { name: 'Chats' }).click();
  await page.getByRole('option', { name: 'None' }).click();
  await expect(page.getByRole('alertdialog')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="alertdialog"]');
});

test('a copy past the limit, with its size against the limit and the ways on', async ({ page }) => {
  await steer('/shares/snapshot', { limit: 1024 * 1024 });
  await shareAs(page, PEOPLE.ada, fixLogin, 'Fix login');
  await tab(page, 'Agents');
  await dialog(page).getByRole('button', { name: 'Share with agents…' }).click();
  const form = page.getByRole('dialog', { name: 'Share “Fix login” with agents' });
  await form.getByRole('combobox', { name: 'Whose agents' }).fill('Main');
  await page.getByRole('option', { name: /^Main thread/ }).first().click();
  await form.getByRole('radio', { name: /Give a copy/ }).click();
  await form.getByRole('button', { name: /Give a copy to/ }).click();
  await expect(form).toContainText('This copy is over the limit');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a request that waits for its approver, with what is asked as ticks', async ({ page, browser, baseURL }) => {
  await steer('/refuse', { username: 'vera', node: ACME.fixLogin, permissions: ['topic.interact'] });
  const vera = await browser.newContext({ baseURL: baseURL ?? 'http://127.0.0.1:41872', locale: 'en-US', timezoneId: 'UTC' });
  const asker = await vera.newPage();
  await signIn(asker, PEOPLE.vera);
  await asker.goto(fixLogin);
  await asker.getByRole('textbox', { name: /Why do you need it/ }).fill('I take part in the login work');
  await asker.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(asker.getByText(/waits to be decided/)).toBeVisible();
  await vera.close();

  await shareAs(page, PEOPLE.ada, fixLogin, 'Fix login');
  await expect(page.getByRole('region', { name: 'Waiting for you' })).toContainText('Vera Rubin');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

// No list of things that can be ticked opens this form yet: the test reaches the module that draws it by its source path, which only the development server serves.
test('Share from a selection of several things, with the page that is skipped and who can change it', { tag: '@development' }, async ({ page }) => {
  await signIn(page, PEOPLE.dora);
  await page.goto(fixLogin);
  // The menu of every place has Share; the form for several is opened the way a list of rows with ticks opens it: through the module that draws it.
  // The development server serves this module even when Vite pre-bundled the shell's import.
  const url = '/src/features/access/AccessDialog.tsx';
  await page.evaluate(
    async ([module, one, other]) => {
      const loaded = (await import(/* @vite-ignore */ module!)) as { openAccessToMany: (targets: { kind: string; id: string; name: string }[]) => void };
      loaded.openAccessToMany([
        { kind: 'topic', id: one!, name: 'Fix login' },
        { kind: 'topic', id: other!, name: 'Upgrade deps' },
      ]);
    },
    [url, ACME.fixLogin, ACME.upgradeDeps],
  );
  await expect(page.getByRole('dialog', { name: 'Access to 2 topics' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('an environment’s page, its five rows by area and the list of who reaches it', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  await page.getByRole('tab', { name: 'Access' }).click();
  await page.getByRole('button', { name: 'Permissions' }).click();
  await expect(line(page, 'Can view')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog(page).getByRole('button', { name: 'Permissions' }).click();
  await expect(dialog(page).getByRole('table', { name: 'Permissions by area' })).toContainText('They work in whatever its displays show');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await dialog(page).getByRole('button', { name: 'Back to the short form' }).click();

  await dialog(page).getByRole('button', { name: /people reach into it/ }).click();
  await expect(page.getByRole('dialog', { name: 'Who can reach Workspace' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a person from outside is staged with the ticks that go with naming them', async ({ page }) => {
  await shareAs(page, PEOPLE.ada, fixLogin, 'Fix login');
  await reach(page, 'Only Fix login itself');
  await stage(page, ZOE.name, 'zoe');
  await expect(dialog(page).getByRole('checkbox', { name: 'They may take it away as a file' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await add(page);
  await expect(line(page, 'Can view')).toContainText('Zoe Outsider');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});
