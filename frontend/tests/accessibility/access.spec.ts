/** Every screen of the permission page, asked in both themes whether anything on it is a serious barrier: the short and the full form, a place's People and Defaults, what one person holds, what a change reaches, the refusal with its request, the bypass, and an invitation to one place. */
import { expect, type Page, test } from '@playwright/test';
import { asAdmin } from '../e2e/admin-mode-support.ts';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

async function steer(path: string, body: unknown = {}): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/access${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
}

test.beforeEach(async () => {
  await scenario();
});

const dialog = (page: Page) => page.getByRole('dialog', { name: /^Access to / });

async function share(page: Page, path: string, thing: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
  await page.getByRole('button', { name: `Actions for ${thing}` }).first().click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
  await expect(dialog(page)).toBeVisible();
}

test('the short form of a topic, with the list of people that the field opens', async ({ page }) => {
  await share(page, `/topic/${ACME.fixLogin}`, 'Fix login');
  await expect(dialog(page).getByRole('group', { name: 'Can manage' })).toContainText('Developers');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' }).fill('gra');
  await expect(page.getByRole('option', { name: /Grace Hopper/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="listbox"]');
  await page.keyboard.press('Escape');

  // What a change that takes something away asks first.
  await dialog(page).getByRole('button', { name: 'Make private', exact: true }).click();
  await expectAccessibleInBothThemes(page, '[role="alertdialog"]');
});

test('the full form with its advanced rows, Administration and the history open', async ({ page }) => {
  await share(page, `/topic/${ACME.fixLogin}`, 'Fix login');
  await dialog(page).getByRole('button', { name: 'Permissions' }).click();
  await dialog(page).locator('summary', { hasText: 'Advanced' }).click();
  await dialog(page).locator('summary', { hasText: 'Administration' }).click();
  await dialog(page).getByRole('button', { name: 'History of this page' }).click();
  await expect(dialog(page).getByText('Ada Lovelace made the topic Fix login')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a place’s People and Defaults, with a lock set and a role given', async ({ page }) => {
  await share(page, `/project/${ACME.web}`, 'Web');
  await dialog(page).getByRole('tab', { name: 'People' }).click();
  await expect(dialog(page).getByRole('button', { name: /Developers of Acme/ })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');

  await dialog(page).getByRole('button', { name: 'Add people…' }).click();
  const give = page.getByRole('dialog', { name: 'Give a role in Web' });
  await expect(give).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  // Its first field opens its list as it takes the keyboard: Escape puts the list away, then Cancel the dialog.
  await page.keyboard.press('Escape');
  await give.getByRole('button', { name: 'Cancel', exact: true }).click();

  await dialog(page).getByRole('tab', { name: 'Defaults' }).click();
  await dialog(page).getByRole('radio', { name: 'Threads' }).click();
  await expect(dialog(page).getByText('In a managed environment')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await dialog(page).getByRole('radio', { name: 'Topics' }).click();
  await dialog(page).getByRole('button', { name: 'View Topic: more' }).click();
  await page.getByRole('menuitem', { name: 'Lock' }).click();
  await expect(page.getByRole('alertdialog', { name: 'Lock View Topic?' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="alertdialog"]');
});

test('what one person holds, and an invitation to one place', async ({ page }) => {
  await share(page, `/topic/${ACME.fixLogin}`, 'Fix login');
  // What one person holds is read from the full form.
  await dialog(page).getByRole('button', { name: 'Permissions' }).click();
  await dialog(page).getByRole('button', { name: 'View as…' }).click();
  const view = page.getByRole('dialog', { name: 'View “Fix login” as…' });
  await view.getByRole('combobox', { name: 'Person' }).fill('gra');
  await page.getByRole('option', { name: /Grace Hopper/ }).click();
  await expect(view.getByRole('list', { name: 'What they hold' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');
  // Back to the short form, where people are added.
  await dialog(page).getByRole('button', { name: 'Back to the short form', exact: true }).click();
  await dialog(page).getByRole('combobox', { name: 'Add people, groups or roles' }).fill('dana@example.org');
  await page.getByRole('option', { name: /Send an invitation to dana@example.org/ }).click();
  const invite = page.getByRole('dialog', { name: 'Invite to “Fix login”' });
  await invite.getByRole('button', { name: 'Make the invitation' }).click();
  await expect(invite.getByRole('textbox', { name: 'Link' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a thread on a person’s machine and the page of a provider, which has parts that are not served', async ({ page }) => {
  await steer('/connected', { thread: ACME.mainThread, on: true });
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${ACME.mainThread}`);
  // A thread's header shares it with its own button.
  await page.locator('[data-thread-header]').getByRole('button', { name: 'Share thread', exact: true }).click();
  await expect(dialog(page)).toContainText('runs on Ada Lovelace’s machine');
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  // Access is part of Settings now; the provider no longer has a separate Access tab.
  await page.goto(`/resources/providers/${RUNS_ON.openAiKey}`);
  await page.getByRole('tab', { name: 'Settings', exact: true }).click();
  await page.getByRole('button', { name: 'Permissions' }).click();
  const provider = page.getByRole('dialog', { name: 'Access to “OpenAI key”' });
  // The short form no longer has a Can use group; its full form retains the actual provider permission.
  await provider.getByRole('button', { name: 'Permissions', exact: true }).click();
  await expect(provider.locator('[data-access-permission="provider.use"]')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('a refusal with its request form, and a page that was read only', async ({ page }) => {
  await steer('/refuse', { username: 'vera', node: ACME.web, permissions: ['project.view'] });
  await signIn(page, PEOPLE.vera);
  await page.goto(`/project/${ACME.web}`);
  await expect(page.getByRole('heading', { name: /You don.t have access/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The refusal asks through its own form, opened from Request access, which names who decides.
  await page.getByRole('button', { name: 'Request access', exact: true }).click();
  const asking = page.getByRole('dialog', { name: 'Request access' });
  await expect(asking.getByText('Goes to Ada Lovelace.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await asking.getByRole('button', { name: 'Cancel', exact: true }).or(asking.getByRole('button', { name: 'Close', exact: true })).first().click();

  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: 'Actions for Fix login' }).first().click();
  await page.getByRole('menuitem', { name: 'Share…' }).click();
  await expect(dialog(page).getByText('You can add people up to what you hold, and take back those you added.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('an admin opens private content through the session switch, with its current reach visible', async ({ page, request }) => {
  const made = await request.post(`${DOUBLE}/__double/documents/make`, {
    data: { user: 'dora', node: ACME.fixLogin, title: 'Private accessibility check', private: true, count: 1 },
  });
  expect(made.ok()).toBe(true);
  const document = await made.json() as { id: string };
  await signIn(page, PEOPLE.ada);
  await page.goto(`/doc/${document.id}`);
  await expect(page.locator('[data-main-area]').getByRole('alert')).toHaveText('This does not exist, was deleted, or is not open to you. Return to a place you can open or ask its manager.');
  await expect(page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toHaveCount(0);
  await expectAccessibleInBothThemes(page);
  // The session switch replaced the former per-object bypass button and confirmation.
  await asAdmin(page, true);
  await expect(page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toBeVisible();
  await expect(page.getByRole('img', { name: 'Private · You see this as a system admin', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});
