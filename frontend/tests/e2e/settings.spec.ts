/** A person's own settings and an org's: a second step switched on and asked for at the next sign-in, a token made, shown once and revoked, and what the interface keeps; then the org's members moved, suspended and ended, an invitation made by the org's rules, a setting set and removed, a locked key, a secret, a channel that fails its Test, and the pages read-only for somebody who may not manage. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, bodySent, expectAccessibleSettled, mailed, PEOPLE, scenario, secretsWritten, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const notices = (page: Page) => page.getByRole('region', { name: /^Notifications/ });

/** Signs in and goes to a page by its address. */
async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
  await page.waitForLoadState('networkidle');
}

/** The settings section list becomes its same searchable choice in a narrow pane. */
async function settingsSection(page: Page, place: string, name: string): Promise<void> {
  const nav = page.getByRole('navigation', { name: `Sections of the settings of ${place}`, exact: true });
  const picker = nav.getByRole('combobox');
  await expect(picker.or(nav.getByRole('link', { name: 'General', exact: true }))).toBeVisible();
  if (await picker.isVisible()) {
    await picker.click();
    await page.getByRole('option', { name, exact: true }).click();
  } else await nav.getByRole('link', { name, exact: true }).click();
}

async function settingsSections(page: Page, place: string, names: string[]): Promise<void> {
  const nav = page.getByRole('navigation', { name: `Sections of the settings of ${place}`, exact: true });
  const picker = nav.getByRole('combobox');
  await expect(picker.or(nav.getByRole('link', { name: 'General', exact: true }))).toBeVisible();
  if (await picker.isVisible()) {
    await picker.click();
    for (const name of names) await expect(page.getByRole('option', { name, exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
  } else for (const name of names) await expect(nav.getByRole('link', { name, exact: true })).toBeVisible();
}

/** Signs out from the menu of the account. */
async function signOut(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Your account' }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(/\/login$/);
}

test.describe('a person’s own settings', () => {
  test('the second step is switched on with an authenticator app, and asked for at the next sign-in', async ({ page }) => {
    await open(page, '/settings/security');
    await expect(page.getByRole('heading', { name: 'Security', level: 1 })).toBeVisible();
    const app = page.getByRole('switch', { name: 'Authenticator app' });
    await expect(app).not.toBeChecked();
    await expectAccessibleSettled(page);

    // Switching it on asks for the password first, and then shows the code to scan and the secret in words.
    await app.click();
    const dialog = page.getByRole('dialog', { name: 'Set up an authenticator app' });
    await dialog.getByLabel('Your password', { exact: true }).fill('not the password');
    await dialog.getByRole('button', { name: 'Continue' }).click();
    await expect(dialog.getByText('That did not work. Check the name and the password or code.')).toBeVisible();
    await dialog.getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
    await dialog.getByRole('button', { name: 'Continue' }).click();
    await expect(dialog.getByRole('img', { name: 'Code to scan with your authenticator app' })).toBeVisible();
    await expect(dialog.getByText('JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP')).toBeVisible();
    await expectAccessibleSettled(page, '[role="dialog"]');

    // It is on when a code from the app was entered, and not before.
    await dialog.getByLabel('Code from the app').fill('000000');
    await dialog.getByRole('button', { name: 'Switch on' }).click();
    await expect(dialog.getByText('That code did not work, or was used already.')).toBeVisible();
    await expect(dialog).toBeVisible();
    await dialog.getByLabel('Code from the app').fill('135790');
    await dialog.getByRole('button', { name: 'Switch on' }).click();
    // Recovery codes take focus after setup; the notification region is hidden behind this modal.
    await page.getByRole('dialog', { name: 'Save your recovery codes' }).getByRole('button', { name: 'Done', exact: true }).click();
    await expect(notices(page).getByText('Authenticator app switched on').first()).toBeVisible();
    await expect(page.getByRole('switch', { name: 'Authenticator app' })).toBeChecked();

    // The next sign-in with the password alone is not enough.
    await signOut(page);
    await page.goto('/login?redirect=%2Fsettings%2Fsecurity');
    await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
    await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByText('Signing in as ada', { exact: true })).toBeVisible();
    await page.getByLabel('Code from your authenticator app').fill('135790');
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page).toHaveURL(/\/settings\/security$/);

    // Switching it off asks for the password and a code.
    await page.goto('/settings/security');
    await page.getByRole('switch', { name: 'Authenticator app' }).click();
    const off = page.getByRole('dialog', { name: 'Switch off the authenticator app' });
    await off.getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
    await off.getByLabel('Code from the app or a saved recovery code', { exact: true }).fill('135790');
    await off.getByRole('button', { name: 'Switch off' }).click();
    await expect(page.getByRole('switch', { name: 'Authenticator app' })).not.toBeChecked();
  });

  test('a code by mail is offered where the orgs allow it, switched on with the code that was mailed', async ({ page }) => {
    // Ada's orgs allow no such code: the way is not on her page at all.
    await open(page, '/settings/security');
    await expect(page.getByRole('switch', { name: 'Authenticator app' })).toBeVisible();
    await expect(page.getByRole('switch', { name: 'Code by mail' })).toHaveCount(0);
    await signOut(page);

    await signIn(page, PEOPLE.dora);
    await page.goto('/settings/security');
    const mail = page.getByRole('switch', { name: 'Code by mail' });
    await expect(mail).not.toBeChecked();
    await expect(page.getByText('We send a code to dora@example.org. This protects you from somebody who has your password, not from somebody who reads your mail.')).toBeVisible();
    await mail.click();
    const dialog = page.getByRole('dialog', { name: 'Use a code by mail as a second step' });
    await dialog.getByLabel('Your password', { exact: true }).fill(PEOPLE.dora.password);
    await dialog.getByRole('button', { name: 'Continue' }).click();
    await expect(dialog.getByText('We sent a code to dora@example.org. Enter it to switch this on.')).toBeVisible();
    await dialog.getByLabel('Code', { exact: true }).fill(await mailed('dora', 'code'));
    await dialog.getByRole('button', { name: 'Switch on' }).click();
    await expect(page.getByRole('switch', { name: 'Code by mail' })).toBeChecked();

    // The line for whoever loses what they switched on closes the section.
    await expect(page.getByText(/^Lost your phone\? Use a saved recovery code\./)).toBeVisible();

    await signOut(page);
    await page.getByLabel('Username or email address').fill(PEOPLE.dora.login);
    await page.getByLabel('Password', { exact: true }).fill(PEOPLE.dora.password);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByText('We sent a code to your address.')).toBeVisible();
  });

  test('a token is made with the permissions ticked and a place, shown once, and revoked', async ({ page }) => {
    await open(page, '/settings/tokens');
    await expect(page.getByText('You have no API tokens.')).toBeVisible();
    await expectAccessibleSettled(page);

    await page.getByRole('button', { name: 'New token…' }).click();
    const dialog = page.getByRole('dialog', { name: 'New API token' });
    // A token needs a name and, if it is to do only some things, at least one of them.
    await dialog.getByRole('button', { name: 'Create token' }).click();
    await expect(dialog.getByText('A token needs a name.')).toBeVisible();
    await dialog.getByLabel('Name').fill('ci');
    await dialog.getByRole('radio', { name: 'Selected', exact: true }).click();
    await dialog.getByRole('radio', { name: 'By area', exact: true }).click();
    await dialog.getByRole('button', { name: 'Create token' }).click();
    await expect(dialog.getByText('Tick at least one permission, or choose everything.')).toBeVisible();
    await dialog.getByRole('checkbox', { name: /View Thread/ }).click();
    await dialog.getByRole('checkbox', { name: /Interact Topic/ }).click();

    // Kept to one place and what lies under it.
    await dialog.getByRole('combobox', { name: 'Only in' }).fill('Web');
    // The list stands above the dialog, outside it.
    const web = page.getByRole('option', { name: 'Acme / Research / Web', exact: true });
    await expect(web).toHaveAccessibleDescription('Project');
    await web.click();
    await expect(dialog.getByText('Acme / Research / Web')).toBeVisible();
    await dialog.getByRole('combobox', { name: 'Good for' }).click();
    await page.getByRole('option', { name: '30 days' }).click();
    await dialog.getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
    await expectAccessibleSettled(page, '[role="dialog"]');
    const created = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/me/tokens'));
    await dialog.getByRole('button', { name: 'Create token' }).click();
    expect(await bodySent((await created).request())).toMatchObject({ name: 'ci', node_id: ACME.web, scopes: ['thread.view', 'topic.interact'] });

    // The secret is shown now, with the warning, and nowhere afterwards.
    const made = page.getByRole('dialog', { name: 'Token created' });
    await expect(made.getByText('This is the only time the token is shown.')).toBeVisible();
    const secret = await made.getByLabel('Token ci').inputValue();
    expect(secret).toMatch(/^opat_[0-9a-f]{32}$/);
    await made.getByRole('button', { name: 'Done' }).click();

    const row = page.getByRole('row', { name: /ci/ });
    await expect(row).toContainText('Acme / Research / Web'.split(' / ').at(-1) ?? '');
    await expect(row).toContainText('2 permissions');
    await expect(row).toContainText('Never used');
    for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await row.scrollIntoViewIfNeeded();
      await expect(row).toBeInViewport();
      await expect(row.getByRole('link', { name: 'Web', exact: true })).toBeVisible();
      await expect(row).toContainText('2 permissions');
      await expect(row).toContainText('Never used');
      const pane = await page.locator('[data-main-area]').boundingBox(), bounds = await row.boundingBox();
      expect(pane).not.toBeNull(); expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(pane!.x);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(pane!.x + pane!.width);
      await test.info().attach(`token-place-${width}-${theme}`, { body: await row.screenshot(), contentType: 'image/png' });
    }
    await page.reload();
    await expect(page.getByRole('row', { name: /ci/ })).toBeVisible();
    await expect(page.getByText(secret)).toHaveCount(0);

    // Revoking asks once, and ends it.
    await page.getByRole('row', { name: /ci/ }).getByRole('button', { name: 'Revoke' }).click();
    const confirm = page.getByRole('alertdialog', { name: 'Revoke ci?' });
    await expect(confirm.getByText('Anything that uses this token stops working at once.')).toBeVisible();
    await confirm.getByRole('button', { name: 'Revoke ci' }).click();
    await expect(page.getByText('You have no API tokens.')).toBeVisible();
  });

  test('a token that does everything its person can is made with one choice and a time', async ({ page }) => {
    await open(page, '/settings/tokens');
    await page.getByRole('button', { name: 'New token…' }).click();
    const dialog = page.getByRole('dialog', { name: 'New API token' });
    await dialog.getByLabel('Name').fill('everything');
    await dialog.getByRole('radio', { name: 'Everything', exact: true }).click();
    await dialog.getByRole('combobox', { name: 'Good for' }).click();
    await page.getByRole('option', { name: 'Never', exact: true }).click();
    await expect(dialog.getByText('A token that never ends stays good if it leaks.')).toBeVisible();
    await dialog.getByRole('combobox', { name: 'Good for' }).click();
    await page.getByRole('option', { name: 'Date', exact: true }).click();
    await dialog.getByRole('button', { name: 'Create token' }).click();
    await expect(dialog.getByText('Choose the day it ends.')).toBeVisible();
    await dialog.getByLabel('Ends on', { exact: true }).fill('2030-01-31');
    await dialog.getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
    await dialog.getByRole('button', { name: 'Create token' }).click();
    await page.getByRole('dialog', { name: 'Token created' }).getByRole('button', { name: 'Done' }).click();
    const row = page.getByRole('row', { name: /everything/ });
    await expect(row).toContainText('Everything I can do');
    await expect(row).toContainText('Everywhere');
    for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await row.scrollIntoViewIfNeeded();
      await expect(row).toBeInViewport();
      await expect(row).toContainText('Everything I can do');
      await expect(row).toContainText('Everywhere');
      await expect(row).toContainText('Never used');
      const pane = await page.locator('[data-main-area]').boundingBox();
      const bounds = await row.boundingBox();
      expect(pane).not.toBeNull(); expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(pane!.x);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(pane!.x + pane!.width);
      await test.info().attach(`token-facts-${width}-${theme}`, { body: await row.screenshot(), contentType: 'image/png' });
    }
  });

  test('a session of another browser is ended after one question; this browser has no such button', async ({ page }) => {
    await open(page, '/settings/security');
    const sessions = page.getByRole('region', { name: 'Where you are signed in' });
    await expect(sessions.getByText('Chrome (Linux)')).toBeVisible();
    await expect(sessions.getByText('This browser')).toBeVisible();
    await expect(sessions.getByRole('button', { name: 'Sign out', exact: true })).toHaveCount(1);
    await sessions.getByRole('button', { name: 'Sign out', exact: true }).click();
    await page.getByRole('alertdialog', { name: 'Sign Safari (macOS) out?' }).getByRole('button', { name: 'Sign it out' }).click();
    await expect(sessions.getByText('Safari (macOS)', { exact: true })).toHaveCount(0);
    await expect(sessions.getByText('This browser', { exact: true })).toBeVisible();
    await expect(sessions.getByRole('button', { name: 'Sign out', exact: true })).toHaveCount(0);
  });

  test('the password is changed after both entries agree, and the page says that others are signed out', async ({ page }) => {
    await open(page, '/settings/security');
    await expect(page.getByText('Changing it signs you out everywhere else.')).toBeVisible();
    await page.getByLabel('Current password', { exact: true }).fill(PEOPLE.ada.password);
    await page.getByLabel('New password', { exact: true }).fill('a much longer password');
    await page.getByLabel('New password again', { exact: true }).fill('a different longer password');
    await page.getByRole('button', { name: 'Change password' }).click();
    await expect(page.getByText('The two passwords differ.')).toBeVisible();
    await page.getByLabel('New password again', { exact: true }).fill('a much longer password');
    await page.getByRole('button', { name: 'Change password' }).click();
    await expect(notices(page).getByText('Password changed').first()).toBeVisible();
    await expect(page.getByText('Safari (macOS)', { exact: true })).toHaveCount(0);
  });

  test('the profile shows the picture the name makes as it is typed, and changing the address asks for the password and waits for the link', async ({ page }) => {
    await open(page, '/settings/profile');
    await expect(page.getByText('Confirmed')).toBeVisible();
    await expectAccessibleSettled(page);

    await page.getByLabel('Display name').fill('Zeta Lovelace');
    await expect(page.getByRole('img', { name: 'How your picture looks' })).toHaveText('Z');
    await expect(page.getByText('Saved', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Undo the change to Display name', exact: true })).toBeVisible();

    await page.getByRole('button', { name: 'Change address…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Change your address' });
    await expect(dialog.getByText('Codes and resets go to your confirmed address until the new one is confirmed. The old address is told of the change.')).toBeVisible();
    await dialog.getByLabel('New address').fill('ada@new.example');
    await dialog.getByLabel('Your password', { exact: true }).fill('not the password');
    await dialog.getByRole('button', { name: 'Change address' }).click();
    await expect(dialog.getByText('That did not work. Check the name and the password or code.')).toBeVisible();
    await dialog.getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
    await dialog.getByRole('button', { name: 'Change address' }).click();
    await expect(page.getByText('Changing to ada@new.example: open the link we sent there.')).toBeVisible();
    // The old address is still the confirmed one meanwhile.
    await expect(page.getByText('ada@example.org')).toBeVisible();
  });

  test('an address that is not confirmed offers the link again', async ({ page }) => {
    // Where the system sends no mail nothing waits for a link, so this person can sign in with an address that is not confirmed.
    await scenario({ meta: { mail: false } });
    await open(page, '/settings/profile', { login: 'unconfirmed', password: 'waiting for a link' });
    await expect(page.getByText('Not confirmed')).toBeVisible();
    await page.getByRole('button', { name: 'Send the link again' }).click();
    await expect(notices(page).getByText('A new link was sent to unconfirmed@example.org.').first()).toBeVisible();
  });

  test('the language, the theme and the density are shown as chosen at once, on Interface', async ({ page }) => {
    // Profile says who one is; how it looks is Interface's.
    await open(page, '/settings/profile');
    await expect(page.getByRole('combobox', { name: 'Language' })).toHaveCount(0);
    await page.goto('/settings/interface');
    await page.getByRole('radio', { name: 'Dark' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
    await page.getByRole('radio', { name: 'Compact' }).click();
    await expect(page.locator('html')).toHaveAttribute('data-density', 'compact');
    await page.getByRole('combobox', { name: 'Language' }).click();
    await page.getByRole('option', { name: '简体中文' }).click();
    await expect(page.getByRole('heading', { name: '界面', level: 1 })).toBeVisible();
    await page.reload();
    // They follow the person, not the page.
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
    await expect(page.getByRole('heading', { name: '界面', level: 1 })).toBeVisible();
    // The account menu repeats the person's current appearance and language choices.
    await page.getByRole('button', { name: '你的账号' }).click();
    await expect(page.getByRole('menuitemradio', { name: '深色', exact: true })).toBeChecked();
    await expect(page.getByRole('menuitemradio', { name: '紧凑', exact: true })).toBeChecked();
    await expect(page.getByRole('menuitemradio', { name: '简体中文', exact: true })).toBeChecked();
    await expect(page.getByRole('menuitemradio', { name: 'English', exact: true })).not.toBeChecked();
  });

  test('what the interface keeps is listed, and each part can be changed or given back', async ({ page }) => {
    await open(page, '/settings/interface');
    await expectAccessibleSettled(page);

    // Nothing writes layouts by name yet, so the page does not list them.
    await expect(page.getByRole('region', { name: 'Arrangements saved by name' })).toHaveCount(0);

    // A viewer chosen for a kind of file is given back to its default.
    const viewers = page.getByRole('region', { name: 'What each kind of file opens with' });
    await expect(viewers.getByText('markdown')).toBeVisible();
    await viewers.getByRole('button', { name: 'Use the default' }).click();
    await expect(viewers.getByText('You have not chosen a viewer for any kind of file')).toBeVisible();

    // A finished tutorial can be shown again; hints that were closed come back together.
    const help = page.getByRole('region', { name: 'Tutorials and hints' });
    await expect(help.getByText('Start a first thread')).toBeVisible();
    await expect(help.getByText('You closed 2 hints.')).toBeVisible();
    await help.getByRole('button', { name: 'Show again' }).click();
    await expect(help.getByText('You have not finished a tutorial.')).toBeVisible();
    await help.getByRole('button', { name: 'Restore tips' }).click();
    await expect(help.getByText('You have not closed any hint.')).toBeVisible();
  });
});

test.describe('an org’s settings', () => {
  const settingsOf = (slug: string, section: string) => `/org/${slug}/settings/${section}`;

  test('the sections are the same at every level, and a department opens from its parent', async ({ page }) => {
    await open(page, settingsOf('acme', 'general'));
    await expect(page.getByRole('heading', { name: 'General', level: 1 })).toBeVisible();
    await settingsSections(page, 'Acme', ['General', 'Departments', 'Members', 'Groups', 'Roles and access', 'Defaults', 'Tags', 'Service accounts', 'Audit', 'Everything', 'Outside Orch']);
    await expectAccessibleSettled(page);

    // The departments are listed with what each holds, and one opens its own settings.
    await settingsSection(page, 'Acme', 'Departments');
    const row = page.getByRole('row', { name: /Research/ });
    await expect(row.getByText('research', { exact: true })).toBeVisible();
    const destination = row.getByRole('link', { name: 'Open the settings of Research' });
    await expect(destination).toHaveAttribute('href', '/org/acme.research/settings/general');
    await destination.click();
    await expect(page).toHaveURL(/\/org\/acme\.research\/settings\/general$/);
    await expect(page.getByRole('heading', { name: 'General', level: 1 })).toBeVisible();
    // A department has a Departments section of its own, and a root org's channels are one line there.
    await settingsSections(page, 'Research', ['Departments']);
  });

  test('the name and the description are changed, a stale change is said, and delete says what it does first', async ({ page }) => {
    await open(page, settingsOf('acme.research', 'general'));
    await page.getByLabel('Name', { exact: true }).fill('Research & Development');
    await page.getByLabel('Description').fill('Where the experiments run');
    await expect(page.getByText('Saved', { exact: true })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'General', level: 1 })).toBeVisible();
    await expect(page.getByLabel('Name', { exact: true })).toHaveValue('Research & Development');
    await expectAccessibleSettled(page);

    // Deleting a department asks two things first, after saying what it does.
    await expect(page.getByText('Deletes this org with everything in it. It stays recoverable from “Recently deleted” until it is purged.')).toBeVisible();
    await page.getByRole('button', { name: 'Delete…' }).click();
    const dialog = page.getByRole('alertdialog', { name: 'Delete the department Research & Development, in Acme?' });
    await expect(dialog.getByText(/Whoever manages Acme can bring it back from “Recently deleted” until .+; then it is gone for good\./)).toBeVisible();
    await expect(dialog.getByText(/become members of Acme, as/)).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
  });

  test('somebody who may not manage reads the pages, and every act says what it takes', async ({ page }) => {
    await open(page, settingsOf('acme', 'general'), PEOPLE.vera);
    await expect(page.getByLabel('Name', { exact: true })).toHaveAttribute('readonly', '');
    await expect(page.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
    await expect(page.getByText('You can’t manage Acme. Ask Ada Lovelace, who owns it.', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Delete…' })).toHaveAttribute('aria-disabled', 'true');

    await page.goto(settingsOf('acme', 'members'));
    await expect(page.getByRole('button', { name: 'Invite…' })).toHaveAttribute('aria-disabled', 'true');
    await page.getByRole('button', { name: 'Actions for Dora Developer' }).click();
    await expect(page.getByRole('menuitem', { name: 'Move to…' })).toHaveAttribute('aria-disabled', 'true');
    await page.keyboard.press('Escape');

    // The policies are read as they are, switched off, with the same sentence.
    await page.goto(settingsOf('acme', 'policies'));
    const row = page.locator('[data-setting="terminal.max_per_person"]');
    await expect(row).toHaveAttribute('data-setting-readonly', 'true');
    await expect(row).toContainText('4');
    await expect(row.locator('input, textarea, [role="spinbutton"], [role="combobox"], [role="switch"]')).toHaveCount(0);
    // What it takes is said once, above the rows, and not again at each of them.
    await expect(page.getByText('You can view these values; ask Ada Lovelace about changes.', { exact: true })).toBeVisible();
    await expect(row).not.toContainText('You can’t');
    await expectAccessibleSettled(page);
  });

  test('a member is moved to another domain, after the dialog says what that changes', async ({ page }) => {
    await open(page, settingsOf('acme', 'members'));
    const row = page.getByRole('row', { name: /Grace Hopper/ });
    await expect(row).toContainText('Acme');
    await expectAccessibleSettled(page);
    await page.getByRole('button', { name: 'Actions for Grace Hopper' }).click();
    await page.getByRole('menuitem', { name: 'Move to…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Move Grace Hopper' });
    await expect(dialog.getByRole('button', { name: 'Move Grace Hopper' })).toHaveAttribute('aria-disabled', 'true');
    await dialog.getByRole('combobox', { name: 'Move to' }).click();
    await page.getByRole('option', { name: /^Acme \/ Research \/ ML/ }).click();
    // What it does is written out before it is done, from the server's plan.
    await expect(dialog.getByText("Grace Hopper's place becomes ML, and no longer Acme.")).toBeVisible();
    await expect(dialog.getByText('Nothing that names Grace Hopper changes:')).toBeVisible();
    await expect(dialog.getByText('They keep what they own, where it is: 2 things in Acme.')).toBeVisible();
    await expectAccessibleSettled(page, '[role="dialog"]');
    await dialog.getByRole('button', { name: 'Move Grace Hopper' }).click();
    await expect(notices(page).getByText('Grace Hopper now belongs to ML').first()).toBeVisible();
    await expect(page.getByRole('row', { name: /Grace Hopper/ })).toContainText('ML');
  });

  test('somebody is held out of the org and let back, with the reason kept', async ({ page }) => {
    await open(page, settingsOf('acme', 'members'));
    await page.getByRole('button', { name: 'Actions for Linus Pauling' }).click();
    await page.getByRole('menuitem', { name: 'Suspend…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Suspend Linus Pauling in Acme' });
    await expect(dialog.getByText('Linus Pauling is held out of Acme and everything in it until this is lifted. What Linus Pauling owns stays theirs.')).toBeVisible();
    await dialog.getByLabel('Reason').fill('Away on leave');
    await dialog.getByRole('button', { name: 'Suspend Linus Pauling' }).click();
    const row = page.getByRole('row', { name: /Linus Pauling/ });
    await expect(row).toContainText('Suspended');
    await expect(row).toContainText('Suspended by Ada Lovelace: Away on leave');

    await page.getByRole('button', { name: 'Actions for Linus Pauling' }).click();
    await page.getByRole('menuitem', { name: 'Resume' }).click();
    await expect(page.getByRole('row', { name: /Linus Pauling/ })).not.toContainText('Suspended');

    // An owner cannot be held out; the entry says so.
    await page.getByRole('button', { name: 'Actions for Ada Lovelace' }).click();
    await expect(page.getByRole('menuitem', { name: 'Suspend…' })).toHaveAttribute('aria-disabled', 'true');
  });

  test('a membership is ended after the dialog lists what is theirs and what becomes of it', async ({ page }) => {
    await open(page, settingsOf('acme', 'members'));
    await page.getByRole('button', { name: 'Actions for Dora Developer' }).click();
    await page.getByRole('menuitem', { name: 'Remove from Acme…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Remove Dora Developer from Acme' });
    await expect(dialog.getByText('This ends the membership of all of Acme.')).toBeVisible();
    await expect(dialog.getByText('Dora Developer owns nothing alone here.')).toBeVisible();
    // Each resource she made is kept unless she is told otherwise; one is a subscription others work on.
    await expect(dialog.getByText('Dora’s API key')).toBeVisible();
    await expect(dialog.getByText('used by 2 people · in 1 thread')).toBeVisible();
    await expect(dialog.getByText("Other people will go on working on Dora Developer's sign-in.")).toBeVisible();
    await dialog.getByRole('radiogroup', { name: 'What becomes of Dora’s API key' }).getByRole('radio', { name: 'Remove' }).click();
    await expectAccessibleSettled(page, '[role="dialog"]');
    await dialog.getByRole('button', { name: 'Remove Dora Developer from Acme' }).click();
    await expect(notices(page).getByText('Dora Developer was removed from Acme').first()).toBeVisible();
    await expect(page.getByRole('row', { name: /Dora Developer/ })).toHaveCount(0);

    // What somebody owns alone is passed on and said so; the last owner is told to name another first.
    await page.getByRole('button', { name: 'Actions for Grace Hopper' }).click();
    await page.getByRole('menuitem', { name: 'Remove from Acme…' }).click();
    const grace = page.getByRole('dialog', { name: 'Remove Grace Hopper from Acme' });
    await expect(grace.getByText('What Grace Hopper owns alone: 1 topic and 1 thread. Each passes to the first owner of the org it lies in.')).toBeVisible();
    await grace.getByRole('button', { name: 'Cancel' }).click();
    await page.getByRole('button', { name: 'Actions for Ada Lovelace' }).click();
    await page.getByRole('menuitem', { name: 'Leave Acme…' }).click();
    const leave = page.getByRole('dialog', { name: 'Leave Acme' });
    await leave.getByRole('button', { name: 'Leave Acme' }).click();
    await expect(leave.getByText('Name another owner of Acme first.')).toBeVisible();
  });

  test('an invitation is made by the org’s rules, which are changed under Policies', async ({ page }) => {
    // Usage is a desktop table column; give this policy-flow check room for its count assertion.
    await page.setViewportSize({ width: 1920, height: 1000 });
    await open(page, settingsOf('acme', 'policies'));
    const forms = page.locator('[data-setting="invitations.forms"]');
    await forms.getByRole('button', { name: /^Edit / }).click();
    await forms.getByRole('checkbox', { name: 'Email', exact: true }).click();
    await forms.getByRole('button', { name: /^Edit / }).click();
    await expect(forms.getByText('Saved', { exact: true })).toBeVisible();
    await forms.getByRole('button', { name: /^Edit / }).click();
    await expect(forms.getByRole('checkbox', { name: 'Email', exact: true })).not.toBeChecked();

    await page.goto(settingsOf('acme', 'members'));
    await expect(page.getByText('Forms allowed: A link to share.')).toBeVisible();
    await expect(page.getByText('Set by Acme.')).toBeVisible();
    await page.getByRole('button', { name: 'Invite…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Invite to Acme' });
    // The form the org leaves out is listed, switched off.
    await expect(dialog.getByRole('radio', { name: 'Email', exact: true })).toBeDisabled();
    await expect(dialog.getByText('Acme does not allow the other form of invitation.')).toBeVisible();
    await dialog.getByLabel('How many people').fill('5');
    await dialog.getByRole('button', { name: 'Create invitation' }).click();
    await expect(dialog.getByText(/Anyone who has this link can join, up to 5 people/)).toBeVisible();
    await dialog.getByRole('button', { name: 'Done' }).click();
    await expect(page.getByRole('table', { name: 'Invitations still good' }).getByRole('row', { name: /Anyone with the link/ })).toContainText('0 of 5');
  });

  test('a setting is set here, shown as coming from here, removed, and falls back to what is above', async ({ page }) => {
    await open(page, settingsOf('acme.research', 'policies'));
    const row = page.locator('[data-setting="env.idle_stop_minutes"]');
    // Edit the value directly; the short link identifies its actual source.
    await expect(row.getByRole('link', { name: 'Inherited from Acme' })).toHaveAttribute('href', /setting=env.idle_stop_minutes/);
    await expect(row.getByRole('spinbutton')).toHaveValue('720');
    await expect(row.getByRole('switch')).toHaveCount(0);
    await expect(row.getByRole('button', { name: /Reset/ })).toHaveCount(0);
    await expect(row.getByRole('button', { name: 'Save' })).toHaveCount(0);
    await expectAccessibleSettled(page);

    await row.getByRole('spinbutton').fill('90');
    await page.keyboard.press('Tab');
    await page.keyboard.press('Tab');
    await expect(row.getByText('Saved', { exact: true })).toBeVisible();
    await expect(row.getByRole('spinbutton')).toHaveValue('90');
    await expect(row.getByText('Set in Research', { exact: true })).toBeVisible();

    // Reset removes this place's contribution, leaving the ancestor unchanged.
    await row.getByRole('button', { name: /^More about / }).click();
    await page.getByRole('menuitem', { name: 'Reset to inherited', exact: true }).click();
    await expect(row.getByRole('link', { name: 'Inherited from Acme' })).toBeVisible();
    await expect(row.getByRole('spinbutton')).toHaveValue('720');
    await expect(row.getByRole('button', { name: /Reset/ })).toHaveCount(0);

    // A native minimum refuses an invalid draft without replacing the inherited value.
    await row.getByRole('spinbutton').fill('-5');
    await page.keyboard.press('Tab');
    await page.keyboard.press('Tab');
    expect(await row.getByRole('spinbutton').evaluate((control) => control instanceof HTMLInputElement && !control.validity.valid)).toBe(true);
    await expect(row.getByRole('spinbutton')).toHaveValue('-5');
    await expect(row.getByRole('link', { name: 'Inherited from Acme' })).toBeVisible();
    await expect(row.getByRole('button', { name: /Reset/ })).toHaveCount(0);
  });

  test('a key locked above is shown as locked, with where, and cannot be changed below', async ({ page }) => {
    await open(page, settingsOf('acme.research', 'policies'));
    const row = page.locator('[data-setting="sandbox.mode"]');
    await expect(row.getByRole('link', { name: 'Locked at Acme' })).toHaveAttribute('href', /setting=sandbox.mode/);
    await expect(row.getByRole('switch')).toHaveCount(0);
    await expect(row.getByRole('combobox')).toHaveCount(0);
    await expect(row.getByRole('button', { name: /Reset/ })).toHaveCount(0);
    await expectAccessibleSettled(page);

    // Where it is locked it is set, and the lock can be lifted by whoever makes the rules there.
    await page.goto(settingsOf('acme', 'policies'));
    const here = page.locator('[data-setting="sandbox.mode"]');
    // The server names the lock of this very row as the one that decides the key; at the place that holds it, it is not a lock from above.
    await expect(here.getByRole('link', { name: 'Locked at Acme' })).toHaveCount(0);
    await expect(here.getByRole('img', { name: 'Locked here: places below cannot change it' })).toBeVisible();
    await expect(here.getByText('A place above decided this')).toHaveCount(0);
    await expect(here.getByRole('combobox')).toBeEnabled();
    await here.getByRole('button', { name: /^More about / }).click();
    await expect(page.getByRole('menuitemcheckbox', { name: 'Lock for places below' })).toBeChecked();
    await page.getByRole('menuitemcheckbox', { name: 'Lock for places below' }).click();
    await here.getByRole('button', { name: /^More about / }).click();
    await expect(page.getByRole('menuitemcheckbox', { name: 'Lock for places below' })).not.toBeChecked();
    await page.keyboard.press('Escape');
    await page.goto(settingsOf('acme.research', 'policies'));
    await expect(page.locator('[data-setting="sandbox.mode"]').getByRole('combobox')).toBeEnabled();
  });

  test('a secret is written, listed by name, and never shown', async ({ page }) => {
    await open(page, settingsOf('acme', 'policies'));
    const secrets = page.getByRole('region', { name: 'Secrets' });
    await expect(secrets.getByText('REGISTRY_TOKEN')).toBeVisible();
    await expect(secrets.getByText('value hidden').first()).toBeVisible();
    await secrets.getByRole('button', { name: 'New secret…' }).click();
    const dialog = page.getByRole('dialog', { name: 'New secret in Acme' });
    await dialog.getByLabel('Name').fill('db_password');
    await expect(dialog.getByLabel('Name')).toHaveValue('DB_PASSWORD');
    await dialog.getByLabel('Value', { exact: true }).fill('correct-horse-staple');
    // The value is hidden as it is typed.
    await expect(dialog.getByLabel('Value', { exact: true })).toHaveAttribute('type', 'password');
    await dialog.getByRole('button', { name: 'Write secret' }).click();
    await expect(notices(page).getByText('DB_PASSWORD written').first()).toBeVisible();
    await expect(secrets.getByText('DB_PASSWORD')).toBeVisible();
    // Nowhere on the page, nor in what the server gives back, is the value.
    await expect(page.getByText('correct-horse-staple')).toHaveCount(0);
    await page.reload();
    await expect(page.getByText('correct-horse-staple')).toHaveCount(0);
    const written = await secretsWritten();
    expect(Object.values(written)).toContain('correct-horse-staple');

    // One that is set here is replaced or removed; one that came from above is left alone.
    await page.getByRole('region', { name: 'Secrets' }).getByRole('button', { name: 'Delete DB_PASSWORD', exact: true }).click();
    await page.getByRole('alertdialog', { name: 'Delete DB_PASSWORD?' }).getByRole('button', { name: 'Delete DB_PASSWORD', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Secrets' }).getByText('DB_PASSWORD')).toHaveCount(0);
    await page.goto(settingsOf('acme.research', 'policies'));
    const below = page.getByRole('region', { name: 'Secrets' });
    await expect(below.getByText('REGISTRY_TOKEN')).toBeVisible();
    await expect(below.getByText('set in Acme')).toBeVisible();
    await expect(below.getByRole('button', { name: 'Delete REGISTRY_TOKEN', exact: true })).toHaveCount(0);
  });

  test('tags are made, recoloured and taken away; the groups of an org keep their people', async ({ page }) => {
    await open(page, settingsOf('acme', 'tags'));
    await expect(page.getByRole('list', { name: 'Tags of Acme' }).getByText('bug')).toBeVisible();
    await page.getByRole('button', { name: 'New tag' }).click();
    const dialog = page.getByRole('dialog', { name: 'New tag' });
    await dialog.getByRole('button', { name: 'Create tag' }).click();
    await expect(dialog.getByText('A tag needs a name.')).toBeVisible();
    await dialog.getByLabel('Name').fill('flaky');
    await dialog.getByRole('radio', { name: 'Purple' }).click();
    await dialog.getByRole('button', { name: 'Create tag' }).click();
    await expect(page.getByRole('list', { name: 'Tags of Acme' }).getByText('flaky')).toBeVisible();
    await expectAccessibleSettled(page);
    await page.getByRole('button', { name: 'Delete flaky' }).click();
    await page.getByRole('alertdialog', { name: 'Delete flaky?' }).getByRole('button', { name: 'Delete flaky' }).click();
    await expect(page.getByRole('list', { name: 'Tags of Acme' }).getByText('flaky')).toHaveCount(0);

    // A department lists the tags above it as usable, and does not offer to change them.
    await page.goto(settingsOf('acme.research', 'tags'));
    await expect(page.getByRole('list', { name: 'Tags of the orgs above' }).getByText('bug')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Delete bug' })).toHaveCount(0);
  });

  test('the groups of an org keep their people, who are added by name and taken out', async ({ page }) => {
    await open(page, settingsOf('acme', 'groups'));
    const table = page.getByRole('table', { name: 'Groups of Acme' });
    await expect(table.getByRole('row', { name: /Backend team/ })).toContainText('3 people');
    await expectAccessibleSettled(page);
    await table.getByRole('button', { name: 'The people in Backend team' }).click();
    const dialog = page.getByRole('dialog', { name: 'People in Backend team' });
    await expect(dialog.getByText('Grace Hopper')).toBeVisible();
    await dialog.getByRole('combobox', { name: 'Add members', exact: true }).fill('vera');
    await page.getByRole('option', { name: /Vera Rubin/ }).click();
    await expect(dialog.getByText('Vera Rubin')).toBeVisible();
    await dialog.getByRole('button', { name: 'Remove Grace Hopper from the group', exact: true }).click();
    await expect(dialog.getByText('Grace Hopper')).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Close', exact: true }).last().click();
    await expect(table.getByRole('row', { name: /Backend team/ })).toContainText('3 people');

    // A new group, then taken away after saying what goes with it.
    await page.getByRole('button', { name: 'New group' }).click();
    const make = page.getByRole('dialog', { name: 'New group in Acme' });
    await make.getByRole('button', { name: 'Create group' }).click();
    await expect(make.getByText('A group needs a name.')).toBeVisible();
    await make.getByLabel('Name', { exact: true }).fill('Release managers');
    await make.getByRole('button', { name: 'Create group' }).click();
    await expect(table.getByRole('row', { name: /Release managers/ })).toBeVisible();
    await table.getByRole('button', { name: 'Delete Release managers' }).click();
    const confirm = page.getByRole('alertdialog', { name: 'Delete Release managers?' });
    await expect(confirm.getByText('The roles the group holds and the entries that name it are removed with it.')).toBeVisible();
    await confirm.getByRole('button', { name: 'Delete Release managers' }).click();
    await expect(table.getByRole('row', { name: /Release managers/ })).toHaveCount(0);
  });

  test('service accounts are made, given a token that is shown once, and removed; the audit trail marks an override', async ({ page }) => {
    await open(page, settingsOf('acme', 'service-accounts'));
    const list = page.getByRole('list', { name: 'Service accounts of Acme' });
    await expect(list.getByText('CI bot')).toBeVisible();
    await page.getByRole('button', { name: 'New service account' }).click();
    const dialog = page.getByRole('dialog', { name: 'New service account in Acme' });
    await dialog.getByRole('button', { name: 'Create service account' }).click();
    await expect(dialog.getByText('A name is needed.')).toBeVisible();
    await dialog.getByLabel('Name', { exact: true }).fill('Deploy bot');
    await dialog.getByLabel('Username').fill('deploy-bot');
    await dialog.getByRole('button', { name: 'Create service account' }).click();
    await expect(list.getByText('Deploy bot')).toBeVisible();
    await list.getByRole('button', { name: 'Tokens of Deploy bot' }).click();
    const tokens = page.getByRole('dialog', { name: 'Tokens of Deploy bot' });
    await expect(tokens).toBeVisible();
    await tokens.getByRole('button', { name: 'Make token' }).click();
    await tokens.getByLabel('Name', { exact: true }).fill('prod');
    await tokens.getByLabel('Your password', { exact: true }).fill(PEOPLE.ada.password);
    await tokens.getByRole('button', { name: 'Create token', exact: true }).click();
    await expect(tokens.getByText('This is the only time the token is shown.')).toBeVisible();
    await expect(tokens.getByLabel('Token prod', { exact: true })).toHaveValue(/^opat_/);
    await expectAccessibleSettled(page, '[role="dialog"]');
    await tokens.getByRole('button', { name: 'Close', exact: true }).last().click();
    await expect(list.getByRole('listitem').filter({ hasText: 'Deploy bot' })).toContainText('1 token');
    await list.getByRole('button', { name: 'Delete Deploy bot', exact: true }).click();
    await page.getByRole('alertdialog', { name: 'Delete Deploy bot?', exact: true }).getByRole('button', { name: 'Delete Deploy bot', exact: true }).click();
    await expect(list.getByText('Deploy bot')).toHaveCount(0);

    await page.goto(settingsOf('acme', 'audit'));
    const trail = page.getByRole('table', { name: 'Audit trail · Acme' });
    await expect(trail.getByText('Override', { exact: true })).toBeVisible();
    await trail.getByRole('button', { name: 'Show details of Permission added', exact: true }).click();
    await expect(trail.locator('[data-trace-detail]').getByText('Project', { exact: true })).toBeVisible();
    await expect(page.getByRole('combobox', { name: 'Exact action', exact: true })).toBeVisible();
    await page.getByRole('combobox', { name: 'Exact action' }).click();
    await page.getByRole('option', { name: 'Place created', exact: true }).click();
    await expect(page).toHaveURL(/action=node\.created/);
    await expect(trail.getByText('Override', { exact: true })).toHaveCount(0);
    await expectAccessibleSettled(page);

    await page.getByRole('combobox', { name: 'Exact action' }).click();
    await page.getByRole('option', { name: 'All actions', exact: true }).click();
    await page.getByRole('combobox', { name: 'Person', exact: true }).fill('Gra');
    await page.getByRole('option', { name: /Grace Hopper/ }).click();
    await expect(page.getByText('No acts match', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Clear person', exact: true }).click();
    await expect(page.getByRole('table', { name: 'Audit trail · Acme' }).getByText('Override', { exact: true })).toBeVisible();
  });

  test('roles held in the place are the permission page’s People, and its Defaults open from the section', async ({ page }) => {
    await open(page, settingsOf('acme', 'roles'));
    await expect(page.getByRole('table', { name: 'Roles held in Acme' }).getByText('Grace Hopper', { exact: true })).toBeVisible();
    await expectAccessibleSettled(page);
    await page.getByRole('button', { name: 'View defaults' }).first().click();
    await expect(page.getByRole('tab', { name: 'Defaults' })).toHaveAttribute('aria-selected', 'true');
  });

  test('a project’s settings and a topic’s are the same sections drawn for them', async ({ page }) => {
    await open(page, `/project/${ACME.web}/settings`);
    await expect(page.getByRole('heading', { name: 'General', level: 1 })).toBeVisible();
    await expect(page.getByLabel('Name', { exact: true })).toHaveValue('Web');
    await expect(page.getByText('Delete Web')).toBeVisible();
    await settingsSection(page, 'Web', 'Defaults');
    await expect(page).toHaveURL(/section=policies/);
    await expect(page.locator('[data-setting="env.idle_stop_minutes"]').getByRole('link', { name: 'Inherited from Acme' })).toBeVisible();
    await expectAccessibleSettled(page);

    // Topic settings use the same frame and actual links as a project's.
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings`);
    const sections = page.getByRole('navigation', { name: 'Sections of the settings of Fix login' });
    await expect(sections).toHaveAttribute('data-current-section', 'general');
    const current = sections.getByRole('combobox');
    await expect(current.or(sections.getByRole('link', { name: 'General', exact: true }))).toBeVisible();
    if (await current.isVisible()) await expect(current).toHaveValue('General');
    else await expect(sections.getByRole('link', { name: 'General', exact: true })).toHaveAttribute('aria-current', 'page');
    await expect(page.locator('[data-setting="env.thread_default"]')).toHaveCount(0);
    await settingsSection(page, 'Fix login', 'Defaults');
    await expect(page).toHaveURL(/section=policies/);
    await expect(page.locator('[data-setting="env.thread_default"]')).toBeVisible();
    await settingsSection(page, 'Fix login', 'Outside Orch');
    await expect(page.getByRole('heading', { name: 'Outside Orch', level: 1 })).toBeVisible();
    await settingsSection(page, 'Fix login', 'General');
    await expect(page).toHaveURL(/section=general/);
    await expect(sections).toHaveAttribute('data-current-section', 'general');
    await expect(page.getByRole('heading', { name: 'General', level: 1 })).toBeVisible();
  });
});

test.describe('channels and destinations', () => {
  const settingsOf = (slug: string, section: string) => `/org/${slug}/settings/${section}`;

  test('a channel is made from the fields its kind declares, tried, switched off and removed', async ({ page }) => {
    await open(page, settingsOf('acme', 'channels'));
    await expect(page.getByText('Setting a channel up sends nothing to anybody')).toBeVisible();
    const list = page.getByRole('list', { name: 'Channels of Acme' });
    await expect(list.getByText('Ops room')).toBeVisible();
    await expectAccessibleSettled(page);

    // The mail server is made from its own fields; what the far side refuses for good refuses the save, in its own words.
    await page.getByRole('button', { name: 'New channel…' }).click();
    const dialog = page.getByRole('dialog', { name: 'New channel in Acme' });
    await dialog.getByRole('combobox', { name: 'Kind of channel' }).click();
    await page.getByRole('option', { name: /Mail server/ }).click();
    await dialog.getByRole('button', { name: 'Create channel' }).click();
    await expect(dialog.getByText('A channel needs a name.')).toBeVisible();
    await expect(dialog.getByText('This is needed.').first()).toBeVisible();
    await dialog.getByLabel('Name', { exact: true }).fill('Acme mail');
    await dialog.getByLabel('Host', { exact: true }).fill('smtp.refuses.example');
    await dialog.getByLabel('Sends from').fill('orch@acme.example');
    await dialog.getByLabel(/^Password/).fill('wrong-password');
    await expectAccessibleSettled(page, '[role="dialog"]');
    await dialog.getByRole('button', { name: 'Create channel' }).click();
    await expect(dialog.getByText('the mail server answered: 535 5.7.8 authentication failed')).toBeVisible();
    await dialog.getByLabel('Host', { exact: true }).fill('smtp.acme.example');
    await dialog.getByRole('button', { name: 'Create channel' }).click();
    await expect(list.getByText('Acme mail')).toBeVisible();
    // An org has one mail server only: the kind is no longer offered.
    await page.getByRole('button', { name: 'New channel…' }).click();
    await page.getByRole('dialog', { name: 'New channel in Acme' }).getByRole('combobox', { name: 'Kind of channel' }).click();
    await expect(page.getByRole('option', { name: /Mail server/ })).toHaveCount(0);
    // The list is put away first, then the dialog is cancelled: one key for each, each waited for.
    await page.keyboard.press('Escape');
    await expect(page.getByRole('listbox')).toHaveCount(0);
    await page.getByRole('dialog', { name: 'New channel in Acme' }).getByRole('button', { name: 'Cancel' }).click();
    await expect(page.getByRole('dialog', { name: 'New channel in Acme' })).toHaveCount(0);

    // A mail server is tried with an address to send to; the answer is shown.
    await list.getByRole('button', { name: 'Test Acme mail' }).click();
    const test = page.getByRole('dialog', { name: 'Test Acme mail' });
    await test.getByLabel('Send the sample to').fill('boss@acme.example');
    await test.getByRole('button', { name: 'Send a test' }).click();
    await expect(test.getByRole('status')).toHaveText('Test accepted by the notifier. This does not prove display on a device.');
    await test.getByRole('button', { name: 'Close', exact: true }).last().click();

    // Switched off, then removed after being told what that ends.
    const mail = list.getByRole('listitem').filter({ has: page.getByRole('heading', { name: 'Acme mail', exact: true }) });
    await mail.getByRole('switch', { name: 'Enable delivery', exact: true }).click();
    await expect(mail.getByRole('switch', { name: 'Enable delivery', exact: true })).not.toBeChecked();
    await mail.getByRole('button', { name: 'Delete Acme mail', exact: true }).click();
    const confirm = page.getByRole('alertdialog', { name: 'Delete Acme mail?' });
    await expect(confirm.getByText('Notification rules using this channel stop sending. People in Acme will no longer receive notifications through it.')).toBeVisible();
    await confirm.getByRole('button', { name: 'Delete Acme mail', exact: true }).click();
    await expect(list.getByText('Acme mail')).toHaveCount(0);
  });

  test('a channel that fails its Test shows the far side’s own words', async ({ page }) => {
    await open(page, settingsOf('acme', 'channels'));
    await page.getByRole('button', { name: 'New channel…' }).click();
    const dialog = page.getByRole('dialog', { name: 'New channel in Acme' });
    await dialog.getByRole('combobox', { name: 'Kind of channel' }).click();
    await page.getByRole('option', { name: /Webhook/ }).click();
    await dialog.getByLabel('Name', { exact: true }).fill('Failing hook');
    await dialog.getByLabel('Address', { exact: true }).fill('https://hooks.example/failing/ops');
    await dialog.getByRole('button', { name: 'Create channel' }).click();
    const list = page.getByRole('list', { name: 'Channels of Acme' });
    await list.getByRole('button', { name: 'Test Failing hook' }).click();
    const test = page.getByRole('dialog', { name: 'Test Failing hook' });
    await test.getByRole('button', { name: 'Send a test' }).click();
    await expect(test.getByText('The test failed')).toBeVisible();
    await expect(test.getByText('HTTP 502 Bad Gateway from hooks.example: upstream timed out after 10s')).toBeVisible();
    await expectAccessibleSettled(page, '[role="dialog"]');
    await test.getByRole('button', { name: 'Close', exact: true }).last().click();
    // What went wrong stays on the channel until a delivery goes through.
    await expect(list.getByText('The last delivery failed')).toBeVisible();
    await expect(list.getByText('HTTP 502 Bad Gateway from hooks.example: upstream timed out after 10s')).toBeVisible();
  });

  test('a secret of a channel is written and never shown, and an edit that leaves it empty keeps it', async ({ page }) => {
    await open(page, settingsOf('acme', 'channels'));
    const list = page.getByRole('list', { name: 'Channels of Acme' });
    await list.getByRole('button', { name: 'Edit Ops room' }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit Ops room' });
    await expect(dialog.getByLabel('Address', { exact: true })).toHaveValue('https://hooks.example/ops');
    await expect(dialog.getByText('A value is set. Leave this empty to keep it.')).toBeVisible();
    await expect(dialog.getByLabel(/^Secret/)).toHaveValue('');
    await dialog.getByLabel('Name', { exact: true }).fill('Ops room (main)');
    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(list.getByText('Ops room (main)')).toBeVisible();
    await expect(page.getByText('signing')).toHaveCount(0);
  });

  test('a department has no channels of its own, and says where they are', async ({ page }) => {
    await open(page, settingsOf('acme.research', 'channels'));
    await expect(page.getByText('Set for all of Acme.')).toBeVisible();
    await page.getByRole('link', { name: 'Open', exact: true }).click();
    await expect(page).toHaveURL(/\/org\/acme\/settings\/channels$/);
  });

  test('destinations are listed for a project, made with the kinds they take, tried and removed', async ({ page }) => {
    await open(page, `/project/${ACME.web}/settings?section=destinations`);
    const list = page.getByRole('list', { name: 'Destinations of Web' });
    await expect(list.getByText('Build room')).toBeVisible();
    await expect(page.getByText('What happens in Web and below it. Nothing from private places below, and nothing people say in chats.')).toBeVisible();
    await expectAccessibleSettled(page);

    // One whose far side fails says so, in its words.
    await list.getByRole('button', { name: 'Test Build room' }).click();
    const test = page.getByRole('dialog', { name: 'Test Build room' });
    await test.getByRole('button', { name: 'Send a test' }).click();
    await expect(test.getByText('HTTP 502 Bad Gateway from hooks.example: upstream timed out after 10s')).toBeVisible();
    await test.getByRole('button', { name: 'Close', exact: true }).last().click();

    // A new one is a webhook or another kind the system has, from that kind's fields, with what it takes.
    await page.getByRole('button', { name: 'New destination…' }).click();
    const dialog = page.getByRole('dialog', { name: 'New destination in Web' });
    await dialog.getByRole('combobox', { name: 'Kind of channel' }).click();
    await page.getByRole('option', { name: /Team room/ }).click();
    await dialog.getByLabel('Name', { exact: true }).fill('Team chat');
    await dialog.getByLabel('Room', { exact: true }).fill('#web');
    await dialog.getByRole('radio', { name: 'Selected kinds', exact: true }).click();
    // Nothing is ticked for the person: the form says to tick one before anything is sent.
    await expect(dialog.getByRole('checkbox', { name: 'Approval requested', exact: true })).not.toBeChecked();
    await dialog.getByRole('button', { name: 'Create destination' }).click();
    await expect(dialog.getByText('Tick at least one kind of event, or choose "Every kind".')).toHaveCount(2);
    await dialog.getByRole('checkbox', { name: 'Approval requested', exact: true }).click();
    await expect(dialog.getByText('Tick at least one kind of event, or choose "Every kind".')).toHaveCount(0);
    await dialog.getByRole('radio', { name: 'All kinds', exact: true }).click();
    await expectAccessibleSettled(page, '[role="dialog"]');
    await dialog.getByRole('button', { name: 'Create destination' }).click();
    await expect(list.getByText('Team chat')).toBeVisible();
    const made = list.getByRole('listitem').filter({ has: page.getByRole('heading', { name: 'Team chat', exact: true }) });
    await expect(made.getByText('Team room', { exact: true })).toBeVisible();
    await expect(made.getByText('What is important', { exact: true })).toBeVisible();
    await expect(made.getByText('Every kind', { exact: true })).toBeVisible();

    await made.getByRole('button', { name: 'Delete Team chat', exact: true }).click();
    await page.getByRole('alertdialog', { name: 'Delete Team chat?' }).getByRole('button', { name: 'Delete Team chat', exact: true }).click();
    await expect(list.getByText('Team chat')).toHaveCount(0);
  });

  test('somebody who may not manage reads them, and every act says what it takes', async ({ page }) => {
    await open(page, `/project/${ACME.web}/settings?section=destinations`, PEOPLE.vera);
    const list = page.getByRole('list', { name: 'Destinations of Web' });
    await expect(list.getByText('Build room')).toBeVisible();
    await expect(page.getByRole('button', { name: 'New destination…' })).toHaveAttribute('aria-disabled', 'true');
    await expect(list.getByRole('button', { name: 'Delete Build room', exact: true })).toHaveCount(0);
    await list.getByRole('button', { name: 'Edit Build room' }).hover();
    await expect(page.getByRole('tooltip')).toContainText('You can’t change the settings for Web.');

    await page.goto(settingsOf('acme', 'channels'));
    await expect(page.getByText('The channels of Acme are set up by whoever makes its rules.')).toBeVisible();
  });
});
