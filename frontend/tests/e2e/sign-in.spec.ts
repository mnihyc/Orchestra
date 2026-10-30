/** Getting in: the first administrator, a wrong and a right password, a second step, a code by mail, a name that is locked, and signing out. */
import { expect, test } from '@playwright/test';

import { expectAccessibleInBothThemes, mailed, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

test('a system with no user leads everybody to the setup, and the first person becomes its administrator', async ({ page }) => {
  await scenario({ scenario: 'fresh' });

  // Whatever address is opened, there is one screen.
  await page.goto('/thread/anything');
  await expect(page).toHaveURL(/\/setup$/);
  await expect(page.getByRole('button', { name: 'Create administrator', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // What is wrong is said beside its field when the form is sent, and nothing is sent.
  await page.getByLabel('Username').fill('Ro');
  await page.getByLabel('Password', { exact: true }).fill('short');
  await page.getByRole('button', { name: 'Create administrator' }).click();
  await expect(page.getByText('Use 3 to 32 lower-case letters')).toBeVisible();
  await expect(page.getByText('Enter your name.')).toBeVisible();
  await expect(page.getByText('Use at least 10 characters.')).toBeVisible();
  await expect(page.getByLabel('Username')).toBeFocused();
  await expectAccessibleInBothThemes(page);

  await page.getByLabel('Username').fill('root');
  await page.getByLabel('Name', { exact: true }).fill('Root Admin');
  await page.getByLabel('Password', { exact: true }).fill('a long enough password');
  await page.getByRole('button', { name: 'Create administrator' }).click();

  // They are signed in, in their own org, inside the workbench.
  await expect(page).toHaveURL(/\/org\/root$/);
  await expect(page.getByRole('navigation', { name: 'Navigation' })).toBeVisible();

  // The setup is over for good: its address leads into the application now.
  await page.goto('/setup');
  await expect(page).toHaveURL(/\/org\/root$/);
});

test('a wrong password is said at its field, and the right one gets in', async ({ page }) => {
  await page.goto('/login');
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
  await page.getByLabel('Password', { exact: true }).fill('not her password');
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page.getByText('Wrong name or password.')).toBeVisible();
  // The name stays, the password is to be typed again, and the keyboard is already there.
  await expect(page.getByLabel('Username or email address')).toHaveValue('ada');
  await expect(page.getByLabel('Password', { exact: true })).toHaveValue('');
  await expect(page.getByLabel('Password', { exact: true })).toBeFocused();
  await expect(page).toHaveURL(/\/login$/);
  await expectAccessibleInBothThemes(page);

  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page).toHaveURL(/\/org\/acme$/);
  await expect(page.getByRole('navigation', { name: 'Navigation' })).toBeVisible();
});

test('somebody with an authenticator is asked for its code after a right password, and gets in with it', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('Username or email address').fill(PEOPLE.grace.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.grace.password);
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page).toHaveTitle(/^One more step/);
  const code = page.getByLabel('Code from your authenticator app');
  await expect(code).toBeFocused();
  await expectAccessibleInBothThemes(page);

  await code.fill('000000');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByText('That code did not work, or was used already.')).toBeVisible();

  await code.fill(PEOPLE.grace.totp);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/org\/acme$/);
});

test('somebody whose second step is a mailed code is sent one at once, and gets in with it', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('Username or email address').fill(PEOPLE.linus.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.linus.password);
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page.getByText('If a code can be sent, it is on its way to your address.')).toBeVisible();
  // Another code is offered only after a minute.
  await expect(page.getByRole('button', { name: /Send another in \d+ seconds?/ })).toBeDisabled();

  await page.getByLabel('Code').fill(await mailed('linus', 'code'));
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/org\/acme$/);
});

test('a code by mail gets somebody in without their password, and the page says the same whatever the name', async ({ page }) => {
  await page.goto('/login');

  // The name is asked for first, and the keyboard is put where it is typed.
  await page.getByRole('button', { name: 'Mail me a code instead' }).click();
  await expect(page.getByText(/^Enter your username or email address/)).toBeVisible();
  await expect(page.getByLabel('Username or email address')).toBeFocused();

  // A name nobody has reads exactly like one that somebody has.
  await page.getByLabel('Username or email address').fill('nobody-at-all');
  await page.getByRole('button', { name: 'Mail me a code instead' }).click();
  const sentence = 'If that name may sign in this way and a code can be sent, it is on its way to its address.';
  await expect(page.getByText(sentence)).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Use your password instead' }).click();
  await page.getByLabel('Username or email address').fill(PEOPLE.margaret.login);
  await page.getByRole('button', { name: 'Mail me a code instead' }).click();
  await expect(page.getByText(sentence)).toBeVisible();

  await page.getByLabel('Code').fill('WRONG-123');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByText('That did not work. Check the code, or ask for another.')).toBeVisible();

  await page.getByLabel('Code').fill(await mailed('margaret', 'code'));
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/org\/acme$/);
});

test('a name that is locked says which proof is closed, until when, and what else works', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('Username or email address').fill(PEOPLE.bolted.login);
  // The right password is refused like any other while the name is locked.
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.bolted.password);
  const rejected = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login'));
  await page.getByRole('button', { name: 'Sign in' }).click();

  const refusal = page.getByRole('alert').filter({ hasText: 'Too many wrong passwords for this name.' });
  // The lock lasts three hours: the time alone when that is still today, with the date when the day turns in between.
  await expect(refusal).toContainText(/Try again at (?:[A-Z][a-z]{2} \d{1,2}, )?\d{2}:\d{2} [AP]M, sign in on a browser you have used before, or reset the password\./);
  await expect(refusal.getByRole('link', { name: 'Reset the password' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // The response retains the diagnostic code; the screen presents the cause and recovery in words.
  expect(await (await rejected).json()).toMatchObject({ code: 'auth.locked', request_id: expect.any(String) });
  await expect(page).toHaveURL(/\/login$/);
});

test('eight wrong passwords lock a name, for a stranger as for its person', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('Username or email address').fill('ada');
  for (let attempt = 1; attempt <= 8; attempt += 1) {
    await page.getByLabel('Password', { exact: true }).fill(`guess number ${attempt}`);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByText('Wrong name or password.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sign in' })).not.toHaveAttribute('aria-busy', 'true');
  }

  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page.getByText('Too many wrong passwords for this name.')).toBeVisible();
  await expect(page).toHaveURL(/\/login$/);
});

test('signing out ends the session on this browser, and a signed-in address then asks to sign in', async ({ page }) => {
  await signIn(page);

  await page.getByRole('button', { name: 'Your account' }).click();
  await expect(page.getByRole('menu', { name: 'Your account' })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();

  // The plain sign-in, with no way back named: the person left on purpose.
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();

  // Nothing of the session is left: a reload does not bring it back, and its screens are closed.
  await page.goto('/inbox');
  await expect(page).toHaveURL(/\/login\?redirect=%2Finbox$/);

  // Signing in again leads to the screen that was asked for.
  await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/inbox$/);
});

test('a session outlives a reload, though its token was never kept anywhere', async ({ page }) => {
  await signIn(page);
  // Whatever the page keeps in the browser, the token is not among it.
  const kept = await page.evaluate(() => JSON.stringify([Object.entries(localStorage), Object.entries(sessionStorage)]));
  expect(kept).not.toContain('oat_');

  await page.reload();

  await expect(page).toHaveURL(/\/org\/acme$/);
  await expect(page.getByRole('navigation', { name: 'Navigation' })).toBeVisible();
});

test('a sign-in through an identity provider that was refused comes back in words', async ({ page }) => {
  await scenario({ meta: { identity_providers: [{ key: 'acme', name: 'Acme SSO', auto_register: false }] } });
  await page.goto('/login');

  // The password form comes first; the provider uses the same callback flow below it.
  const provider = page.getByRole('link', { name: 'Continue with Acme SSO' });
  await expect(provider).toBeVisible();
  await provider.click();

  await expect(page).toHaveURL(/\/login\?error=denied$/);
  await expect(page.getByText('The sign-in was cancelled at the identity provider.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('where passwords are for administrators only, the form is behind "Administrator sign-in"', async ({ page }) => {
  await scenario({ meta: { local_login: false, identity_providers: [{ key: 'acme', name: 'Acme SSO', auto_register: false }] } });
  await page.goto('/login');

  await expect(page.getByLabel('Password', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Administrator sign-in' }).click();
  await expect(page.getByLabel('Password', { exact: true })).toBeVisible();

  // Somebody who is no administrator is told so, after a right password.
  await page.getByLabel('Username or email address').fill(PEOPLE.grace.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.grace.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByText('Signing in with a password or a mailed code is switched off here.')).toBeVisible();
});
