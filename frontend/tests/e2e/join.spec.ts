/** Coming in for the first time, and coming back after losing the way: invitations, registration as the system allows it, confirming an address, and resetting a password. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, mailed, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => {
  await scenario();
});

const fillIdentity = async (page: Page, username: string, name: string, password = 'a long enough password') => {
  await page.getByLabel('Username').fill(username);
  await page.getByLabel('Name', { exact: true }).fill(name);
  await page.getByLabel('Password', { exact: true }).fill(password);
};

test('an invitation says what it is before anything is asked, and a stranger registers with it and arrives where it leads', async ({ page }) => {
  await page.goto('/invite/good-invitation');

  await expect(page.getByText('Ada Lovelace invites you to Acme as Developer.')).toBeVisible();
  await expect(page.getByText(/^Good until Dec 31, 12:00 PM\.$/)).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('link', { name: 'Create an account' }).click();
  await expect(page).toHaveURL(/\/register\?invitation=good-invitation$/);
  // The form stands under what the invitation is.
  await expect(page.getByText('Ada Lovelace invites you to Acme as Developer.')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await fillIdentity(page, 'dana', 'Dana Scully');
  await page.getByRole('button', { name: 'Create account' }).click();

  // One line before they go in: where they are, and that they have an org of their own.
  await expect(page.getByText('You are in Acme.')).toBeVisible();
  await expect(page.getByText('You also have an org of your own, for work that is only yours.')).toBeVisible();
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page).toHaveURL(/\/org\/acme$/);
});

test('a personal link asks for the address it was sent to, and refuses another', async ({ page }) => {
  await page.goto('/register?invitation=personal-invitation');

  await expect(page.getByText('Ada Lovelace invites you to Research as Viewer.')).toBeVisible();
  await expect(page.getByText('This invitation is for the address it was sent to.', { exact: true })).toBeVisible();

  await fillIdentity(page, 'dana', 'Dana Scully');
  await page.getByLabel('Email address').fill('somebody.else@example.org');
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByLabel('Email address')).toHaveAccessibleDescription('This invitation is for the address it was sent to. Use that address.');

  await page.getByLabel('Email address').fill('dana@example.org');
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByText('You are in Acme.')).toBeVisible();
});

test('an invitation that is no longer good says so and no more', async ({ page }) => {
  await page.goto('/invite/spent-invitation');
  await expect(page.getByText('This invitation is no longer good. Ask whoever sent it for a new one.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Create an account' })).toHaveCount(0);
  await expectAccessibleInBothThemes(page);
  await page.getByRole('link', { name: 'Back to sign-in' }).click();
  await expect(page).toHaveURL(/\/login$/);

  // Where people come in by invitation, such a link opens nothing: the form is not shown.
  await page.goto('/register?invitation=spent-invitation');
  await expect(page.getByText('This invitation is no longer good.')).toBeVisible();
  await expect(page.getByText('This system takes new people by invitation.')).toBeVisible();
  await expect(page.getByLabel('Username')).toHaveCount(0);
});

test('checks a reset link before showing a password form and gives an invalid link a way back', async ({ page }) => {
  let release: (() => void) | undefined;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  await page.route('**/api/v1/auth/recovery/not-a-token', async (route) => {
    await gate;
    await route.continue();
  });
  try {
    await page.goto('/reset/not-a-token');
    await expect(page.getByRole('heading', { name: 'Password reset link' })).toBeVisible();
    await expect(page.getByLabel('New password', { exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Set password' })).toHaveCount(0);
    release?.();
    await expect(page.getByText('This link was already used, or it has ended.')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Set a new password' })).toHaveCount(0);
    await expect(page.getByLabel('New password', { exact: true })).toHaveCount(0);
    await page.getByRole('link', { name: 'Back to sign-in' }).click();
    await expect(page).toHaveURL(/\/login$/);
  } finally {
    release?.();
  }
});

test('without an invitation the page says what the system allows', async ({ page }) => {
  await page.goto('/register');
  await expect(page).toHaveURL(/\/login$/);
  await expect(page.getByText('This system takes new people by invitation. Ask somebody in the team you are joining for a link.')).toBeVisible();
  await expect(page.getByLabel('Username or email address')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Create account', exact: true })).toHaveCount(0);
  await expectAccessibleInBothThemes(page);

  await scenario({ meta: { registration_mode: 'closed' } });
  await page.goto('/register');
  await expect(page.getByText('Registration is closed here.')).toBeVisible();

  await scenario({ meta: { registration_mode: 'open' } });
  await page.goto('/login');
  await page.getByRole('link', { name: 'Create an account' }).click();
  await expect(page.getByRole('heading', { level: 1, name: 'Orch (double)' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Create an account', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Create account', exact: true })).toBeVisible();
  await expect(page.getByText('Anybody may register on Orch (double).')).toHaveCount(0);

  // A public-name conflict is refused by the server and belongs to the username field.
  await fillIdentity(page, 'ada', 'Another Ada');
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByLabel('Username')).toHaveAttribute('aria-invalid','true');
  await expect(page.getByLabel('Username')).toHaveAccessibleDescription('A username or root org already uses this public name.');

  // Somebody who came without an invitation lands in their own org.
  await page.getByLabel('Username').fill('newcomer');
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByText('Your account is ready.')).toBeVisible();
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page).toHaveURL(/\/org\/newcomer$/);
});

test('where addresses are confirmed first, the link that was mailed confirms the address and signs its person in, once', async ({ page }) => {
  await scenario({ meta: { registration_mode: 'open' }, confirmAddresses: true });
  await page.goto('/register');
  await fillIdentity(page, 'newbie', 'New Bie');

  // The address is asked for where it has to be confirmed.
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByLabel('Email address')).toHaveAccessibleDescription('is required here');
  await page.getByLabel('Email address').fill('newbie@example.org');
  await page.getByRole('button', { name: 'Create account' }).click();

  await expect(page.getByText('We sent a link to newbie@example.org.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  const link = await mailed('newbie', 'confirm');

  await page.goto(`/verify-email/${link}`);
  await expect(page.getByText('Your address is confirmed.')).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByRole('link', { name: 'Continue' }).click();
  await expect(page).toHaveURL(/\/org\/newbie$/);

  // The link was good for one use.
  await page.goto(`/verify-email/${link}`);
  await expect(page.getByText('This link was already used, or it has ended.')).toBeVisible();
});

test('somebody who is signed in accepts an invitation where it is shown', async ({ page }) => {
  await signIn(page, PEOPLE.margaret);
  await page.goto('/invite/good-invitation');

  await expect(page.getByText('Signed in as Margaret Hamilton.')).toBeVisible();
  await page.getByRole('button', { name: 'Accept invitation' }).click();

  await expect(page).toHaveURL(/\/org\/acme$/);
});

test('asking for a reset always answers the same, and the link sets a new password that then gets in', async ({ page }) => {
  await page.goto('/login');
  await page.getByRole('link', { name: 'Forgot your password?' }).click();
  await expect(page.getByRole('button', { name: 'Send the link', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  // An address nobody has reads exactly like one that somebody has.
  await page.getByLabel('Email address').fill('nobody@example.org');
  await page.getByRole('button', { name: 'Send the link' }).click();
  await expect(page.getByText('If that address is known here, a link is on its way.')).toBeVisible();

  await page.goto('/recover');
  await page.getByLabel('Email address').fill('ada@example.org');
  await page.getByRole('button', { name: 'Send the link' }).click();
  await expect(page.getByText('If that address is known here, a link is on its way.')).toBeVisible();
  const link = await mailed('ada', 'reset');

  await page.goto(`/reset/${link}`);
  await expect(page.getByRole('heading', { name: 'Set a new password' })).toBeVisible();
  await expectAccessibleInBothThemes(page);
  await page.getByLabel('New password', { exact: true }).fill('short');
  await page.getByRole('button', { name: 'Set password' }).click();
  await expect(page.getByText('Use at least 10 characters.')).toBeVisible();

  await page.getByLabel('New password', { exact: true }).fill('a brand new password');
  await page.getByRole('button', { name: 'Set password' }).click();
  await expect(page.getByText('Your password is changed.')).toBeVisible();

  // The old password no longer gets in; the new one does.
  await page.getByRole('link', { name: 'Sign in' }).click();
  await page.getByLabel('Username or email address').fill('ada');
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByText('Wrong name or password.')).toBeVisible();
  await page.getByLabel('Password', { exact: true }).fill('a brand new password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/org\/ada$/);

  // The link was good for one use.
  await page.goto(`/reset/${link}`);
  await expect(page.getByText('This link was already used, or it has ended.')).toBeVisible();
  await expect(page.getByLabel('New password', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Set a new password' })).toHaveCount(0);
  await page.getByRole('link', { name: 'Back to sign-in' }).click();
  await expect(page).toHaveURL(/\/login$/);
});

test('where the system sends no mail, the page says who can help instead of asking for an address', async ({ page }) => {
  await scenario({ meta: { mail: false } });
  await page.goto('/recover');

  await expect(page.getByText('This system sends no mail. Ask a system administrator for a link that resets your password.')).toBeVisible();
  await expect(page.getByLabel('Email address')).toHaveCount(0);
});
