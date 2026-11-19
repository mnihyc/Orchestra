/** Every section of Management, asked in both themes whether anything on it is a serious barrier: each section as it is drawn, with what opens above it open, for a system admin, and the refusal that somebody who is none reads. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

/** Each section, by its address and a thing that says it has been drawn. */
const SECTIONS = {
  people: [
    ['/admin/overview', 'Notice for everybody'],
    ['/admin/users', 'Grace Hopper'],
    ['/admin/orgs', 'Orphans Inc'],
    ['/admin/sign-in', 'Acme single sign-on'],
    ['/admin/email', 'This section opens when that part is joined'],
  ],
  machines: [
    ['/admin/hosts', 'build-box'],
    ['/admin/runtimes', 'Claude Max runtime'],
    ['/admin/capabilities', 'Debugger access'],
    ['/admin/harnesses', 'Native fork'],
  ],
  system: [
    ['/admin/agents', 'Claude Code'],
    ['/admin/plugins', 'podman'],
    ['/admin/settings', 'Search the settings'],
    ['/admin/storage', 'Environments that take the most'],
    ['/admin/pki', 'The authority'],
    ['/admin/audit', 'Audit trail · System'],
  ],
} as const;

async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

for (const [group, sections] of Object.entries(SECTIONS)) {
  test(`the sections of ${group}, each drawn with what it holds`, async ({ page }) => {
    test.setTimeout(360_000);
    await signIn(page);
    for (const [path, sign] of sections) {
      await page.goto(path);
      await expect(page.getByText(sign).first()).toBeVisible();
      // A chart is drawn on a canvas that arrives after the rest; wait for it, so that what is asked is what a reader sees.
      if (path === '/admin/overview') await expect(page.getByRole('group', { name: /^Threads by state/ }).locator('canvas')).toBeVisible();
      await expectAccessibleInBothThemes(page);
    }
  });
}

test('the Overview with a part that has stopped answering, with its table of values open, and the span of a year', async ({ page }) => {
  test.setTimeout(240_000);
  await fetch(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/admin/state`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ component: { name: 'gateway', ok: false, detail: 'No heartbeat for 3 minutes', seen_at: null } }),
  });
  await open(page, '/admin/overview?span=year');
  await expect(page.getByText('No heartbeat for 3 minutes')).toBeVisible();
  await expect(page.getByRole('group', { name: /^Threads by state/ }).locator('canvas')).toBeVisible();
  await page.getByRole('region', { name: 'Threads by state' }).getByText('Values as a table').click();
  await expect(page.getByRole('table', { name: /Threads by state: Values as a table/ })).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('what opens above the users: making a person, the link of a new password, and a change to a system admin', async ({ page }) => {
  test.setTimeout(240_000);
  await open(page, '/admin/users');
  await page.getByRole('button', { name: 'New user' }).click();
  const dialog = page.getByRole('dialog', { name: 'Make a new user' });
  await expect(dialog).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await dialog.getByRole('button', { name: 'Make user' }).click();
  await expect(dialog.getByText('Give a name.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await dialog.getByRole('button', { name: 'Cancel' }).click();

  await page.getByRole('button', { name: 'Actions for Grace Hopper' }).click();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
  await page.getByRole('menuitem', { name: 'Reset link…' }).click();
  const link = page.getByRole('dialog', { name: 'Reset link for Grace Hopper' });
  await link.getByRole('button', { name: 'Make the link' }).click();
  await expect(link.getByRole('textbox', { name: 'Link' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await link.getByRole('button', { name: 'Done' }).click();

  await page.getByRole('button', { name: 'Actions for Ada Lovelace' }).click();
  await page.getByRole('menuitem', { name: 'Suspend…' }).click();
  await expect(page.getByRole('dialog', { name: 'Suspend Ada Lovelace?' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
});

test('what opens above the rest: an owner named, a provider added, a capability and a build', async ({ page }) => {
  test.setTimeout(300_000);
  await signIn(page);

  await page.goto('/admin/orgs');
  await page.getByRole('button', { name: 'Name an owner…' }).click();
  await expect(page.getByRole('dialog', { name: 'Name an owner of Orphans Inc' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/admin/sign-in');
  await page.getByRole('button', { name: 'Add an identity provider' }).click();
  await expect(page.getByRole('dialog', { name: 'Add an identity provider' })).toBeVisible();
  await page.getByRole('dialog').getByRole('button', { name: 'Add the provider' }).click();
  await expect(page.getByRole('dialog').getByText('Give the client id the provider made for this system.')).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/admin/capabilities');
  await page.getByRole('button', { name: 'Add a capability' }).click();
  await expect(page.getByRole('dialog', { name: 'Add a capability' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');

  await page.goto('/admin/harnesses');
  await page.getByRole('button', { name: 'Add a build' }).click();
  await expect(page.getByRole('dialog', { name: 'Add a build' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="dialog"]');
  await page.keyboard.press('Escape');
});

test('the refusal that somebody who is no system admin reads at the address of Management, and the account menu of one who is', async ({ page }) => {
  test.setTimeout(240_000);
  await open(page, '/admin/users', PEOPLE.vera);
  await expect(page.getByText('Management is for system administrators')).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('button', { name: 'Your account' }).click();
  await page.getByRole('menuitem', { name: 'Sign out' }).click();
  await page.waitForURL(/\/login/);
  await signIn(page);
  await page.getByRole('button', { name: 'Your account' }).click();
  await expect(page.getByRole('menuitem', { name: 'Management' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[role="menu"]');
});
