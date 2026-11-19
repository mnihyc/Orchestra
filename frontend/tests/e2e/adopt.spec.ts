/** A session begun outside Orch is adopted from its machine's page: listed with Adopt…, the plan said before anything happens, and afterwards the row leads to the thread, whose earlier turns stand under "From the session before Orch". */
import { expect, type Page, test } from '@playwright/test';

import { id as fixtureId, KIND } from '../doubles/fixtures.ts';
import { PEOPLE, scenario, signIn } from './support.ts';

const LAPTOP = fixtureId(KIND.machine, 1);

test.beforeEach(async () => {
  await scenario();
});

async function openSessions(page: Page) {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/resources/machines/${LAPTOP}?tab=sessions`);
  return page.getByRole('list', { name: 'Codex on ada-laptop on Codex subscription' });
}

test('a session is adopted after its plan is read, and its row then leads to the thread with its history', async ({ page }) => {
  const sessions = await openSessions(page);
  await expect(sessions.getByText('Fix the login button')).toBeVisible();
  await expect(sessions.getByText('/home/ada/work/main/web')).toBeVisible();
  await sessions.getByRole('button', { name: 'Adopt “Fix the login button”…' }).click();

  const dialog = page.getByRole('dialog');
  const confirm = dialog.getByRole('button', { name: 'Adopt', exact: true });
  await expect(confirm).toBeDisabled();
  await dialog.getByRole('combobox', { name: 'Topic' }).fill('Fix login');
  await page.getByRole('option', { name: /Fix login/ }).first().click();
  await expect(dialog.getByText('In Workspace, at web')).toBeVisible();
  await expect(dialog.getByText('gpt-5.5-codex, as the session ran')).toBeVisible();
  await expect(dialog.getByText(/Its 2 earlier turns come with it/)).toBeVisible();
  await confirm.click();
  await expect(dialog).toBeHidden();

  const open = sessions.getByRole('link', { name: 'Open thread Fix the login button' });
  await expect(open).toBeVisible();
  await open.click();
  await expect(page.getByText('From the session before Orch', { exact: true })).toBeVisible();
  await expect(page.getByText('Fix the login button: it does nothing')).toBeVisible();
  await expect(page.getByText('All 42 pass.')).toBeVisible();
});

test('a session whose folder none of the person’s environments holds names the folder to register first and adopts nothing', async ({ page }) => {
  const sessions = await openSessions(page);
  await sessions.getByRole('button', { name: 'Adopt “Untitled session”…' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('combobox', { name: 'Topic' }).fill('Fix login');
  await page.getByRole('option', { name: /Fix login/ }).first().click();
  await expect(dialog.getByText('None of your environments on that machine holds /srv/other.')).toBeVisible();
  await expect(dialog.getByText(/The harness does not say which model/)).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Adopt', exact: true })).toBeDisabled();
});
