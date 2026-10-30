/** Management at work: how the system stands and a part that fails, a person made and given a link, suspended and resumed, the last system admin kept, a root org's limit switched, the audit list filtered and its file started, a host that is full, the notice for everybody, who may register, an identity provider, and somebody who is no system admin refused. */
import { expect, type Page, type TestInfo, test } from '@playwright/test';

import { PEOPLE, scenario } from './support.ts';
import { signInForManagement as signIn } from './admin-mode-support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

test.beforeEach(async () => {
  await scenario();
});

/** Tells the fake API how the system's parts stand, or whether it stored anything. */
async function system(body: unknown): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/admin/state`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused the state: ${response.status}`);
}

/** Signs in and goes to a section of Management by its address. */
async function open(page: Page, path: string, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
}

const row = (page: Page, name: string | RegExp) => page.getByRole('row', { name });
const notices = (page: Page) => page.getByRole('region', { name: /^Notifications/ });

/** The current destination is the same link in both presentations; its containing frame chooses which is visible. */
async function currentSection(page: Page, id: string, title: string) {
  const sections = page.getByRole('navigation', { name: 'Management sections', exact: true });
  await expect(sections).toHaveAttribute('data-current-section', id);
  const link = sections.locator(`a[data-section-id="${id}"]`);
  await expect(link).toHaveAttribute('aria-current', 'page'); await expect(link).toHaveAttribute('href', `/admin/${id}`);
  await expect(sections.locator('a[aria-current="page"]')).toHaveCount(1);
  const room = await sections.evaluate((element) => {
    const frame = element.closest<HTMLElement>('[data-settings-frame]');
    if (frame === null) throw new Error('The Management section list must belong to its settings frame');
    return frame.clientWidth;
  });
  // This is SectionNav's existing container threshold, not the browser viewport width.
  if (room < 600) {
    await expect(link).toBeHidden(); await expect(sections.getByRole('combobox', { name: 'Management sections', exact: true })).toHaveValue(title);
  } else await expect(link).toBeVisible();
  return sections;
}

/** Keep the native update warning visible exactly once when its column folds under the unit name. */
async function captureRuntimeWarnings(page: Page, info: TestInfo, width: number) {
  await page.setViewportSize({ width, height: 900 });
  const table = page.getByRole('table', { name: 'Runtimes', exact: true }); await table.scrollIntoViewIfNeeded();
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().filter((animation) => { const end = animation.effect?.getComputedTiming().endTime;
      return animation.playState === 'running' && typeof end === 'number' && Number.isFinite(end); }).map((animation) => animation.finished.catch(() => undefined)));
    await new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
  });
  const geometry = await table.evaluate((element) => {
    const region = element.closest<HTMLElement>('[data-scroll-allow="table"]');
    if (region === null) throw new Error('The runtime table must retain its named local scroll viewport');
    const box = region.getBoundingClientRect();
    return { room: region.clientWidth, left: box.left, right: box.right, width: innerWidth,
      behindColumns: element.querySelectorAll('thead [data-table-column="behind"]').length };
  });
  expect(geometry.behindColumns).toBe(geometry.room >= 640 ? 1 : 0); // Existing Table md room.
  expect(geometry.left).toBeGreaterThanOrEqual(-1); expect(geometry.right).toBeLessThanOrEqual(geometry.width + 1);
  await expect(row(page, /Claude Max runtime/).getByText('Behind in 2 things', { exact: true })).toHaveCount(1);
  await expect(row(page, /Build cache/).getByText('Behind in 1 thing', { exact: true })).toHaveCount(1);
  await expect(row(page, /GPU lab/).first()).toContainText('The image could not be pulled: unauthorized');
  const path = info.outputPath(`admin-runtime-update-warning-${width}.png`);
  await page.screenshot({ path, fullPage: false, caret: 'hide' });
  await info.attach(`runtime-warning-${width}`, { path, contentType: 'image/png' });
  await info.attach(`runtime-warning-${width}-geometry`, { body: JSON.stringify(geometry, null, 2), contentType: 'application/json' });
}

/** Opens the menu of a person's row in the table of users. */
async function openMenu(page: Page, person: string): Promise<void> {
  await page.getByRole('button', { name: `Actions for ${person}` }).click();
}

test.describe('who is in Management', () => {
  test('a system admin finds Management in the account menu, and somebody who is none never does and reads the refusal at its address', async ({ page }) => {
    await signIn(page);
    await page.getByRole('button', { name: 'Your account' }).click();
    await page.getByRole('menuitem', { name: 'Management' }).click();
    await expect(page).toHaveURL(/\/admin\/overview$/);
    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible();

    await page.getByRole('button', { name: 'Your account' }).click();
    await page.getByRole('menuitem', { name: 'Sign out' }).click();
    await page.waitForURL(/\/login/);

    await signIn(page, PEOPLE.vera);
    await page.getByRole('button', { name: 'Your account' }).click();
    await expect(page.getByRole('menuitem', { name: 'Settings' })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Management' })).toHaveCount(0);
    await page.keyboard.press('Escape');

    await page.goto('/admin/users');
    await expect(page.getByText('Management is for system administrators')).toBeVisible();
    await expect(page.getByText("Your account is not one. Ask one of this system's administrators to make the change you need, or to make you one.", { exact: true })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Back to your org', exact: true })).toHaveAttribute('href', '/');
    await expect(page.getByRole('navigation', { name: 'Management sections' })).toHaveCount(0);
    await expect(page.getByRole('table', { name: 'Users' })).toHaveCount(0);
  });

  test('an address that names no section is no screen', async ({ page }) => {
    await open(page, '/admin/nowhere');
    await expect(page.getByText('There is nothing at this address')).toBeVisible();
  });
});

test.describe('the Overview', () => {
  test('says how the system stands, and shows a part that has stopped answering with what it says', async ({ page }) => {
    await system({ component: { name: 'worker', ok: false, detail: 'No heartbeat for 3 minutes', seen_at: new Date(Date.now() - 180_000).toISOString() } });
    await open(page, '/admin/overview');

    await expect(page.getByText('Version 0.1.0')).toBeVisible();
    const parts = page.getByRole('region', { name: 'Parts of the system' });
    await expect(parts.getByRole('listitem').filter({ hasText: 'Worker' })).toContainText('Not answering');
    await expect(parts.getByRole('listitem').filter({ hasText: 'Worker' })).toContainText('No heartbeat for 3 minutes');
    await expect(parts.getByRole('listitem').filter({ hasText: 'Gateway' })).toContainText('Well');
    await expect(parts.getByRole('listitem').filter({ hasText: 'Database' })).toContainText('Well');

    const now = page.getByRole('region', { name: 'Right now' });
    await expect(now.getByText('Machines online')).toBeVisible();
    await expect(now.getByText('Threads running')).toBeVisible();
    await expect(now.getByText('Events waiting')).toBeVisible();
  });

  test('draws what the system stored over a span, which is changed from the address and the control, and says where nothing was stored', async ({ page }) => {
    await open(page, '/admin/overview');
    const threads = page.getByRole('group', { name: /^Threads by state/ });
    await expect(threads).toBeVisible();
    await expect(threads.locator('canvas')).toBeVisible();
    await expect(page.getByText('Running', { exact: true }).first()).toBeVisible();

    // A week, by the control; it is in the address, and the charts hold what they showed while the new span comes.
    await page.getByRole('radio', { name: 'Week' }).click();
    await expect(page).toHaveURL(/span=week/);
    await expect(page.getByRole('group', { name: /^Threads by state/ }).locator('canvas')).toBeVisible();

    // The values are also a table, for whoever does not read a chart by sight.
    await page.getByRole('region', { name: 'Threads by state' }).getByText('Values as a table').click();
    await expect(page.getByRole('table', { name: /Threads by state: Values as a table/ })).toBeVisible();

    await system({ stored: false });
    await page.reload();
    await expect(page.getByText('Nothing is stored for this span yet.').first()).toBeVisible();
  });

  test('puts up a notice for everybody, which every page shows at once, and takes it down', async ({ page }) => {
    await open(page, '/admin/overview');
    await page.getByRole('textbox', { name: 'Text' }).fill('Maintenance tonight at 22:00');
    await page.getByRole('radio', { name: 'Warning' }).click();
    await expect(page.getByText('How it will look')).toBeVisible();
    await page.getByRole('button', { name: 'Put up the notice' }).click();

    await expect(page.getByRole('alert').filter({ hasText: 'Maintenance tonight at 22:00' }).first()).toBeVisible();
    await page.goto('/admin/users');
    await expect(page.getByRole('alert').filter({ hasText: 'Maintenance tonight at 22:00' }).first()).toBeVisible();

    await page.goto('/admin/overview');
    await page.getByRole('button', { name: 'Take the notice down' }).click();
    await expect(page.getByRole('alertdialog', { name: 'Take the notice down?' })).toBeVisible();
    await page.getByRole('alertdialog').getByRole('button', { name: 'Take the notice down' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Maintenance tonight at 22:00' })).toHaveCount(0);
  });
});

test.describe('Users', () => {
  test('makes a person, who is given the link at which they set a password, with an org of their own as asked', async ({ page }) => {
    await open(page, '/admin/users');
    await page.getByRole('button', { name: 'New user' }).click();
    const dialog = page.getByRole('dialog', { name: 'Make a new user' });
    await expect(dialog).toBeVisible();

    // Both things a personal org starts with are ticked, as the system's policies say.
    await expect(dialog.getByRole('checkbox', { name: 'Only they are in it' })).toBeChecked();
    await expect(dialog.getByRole('checkbox', { name: 'No orgs inside it' })).toBeChecked();

    // Nothing is sent that the form can see is wrong, and each fault is said at its field.
    await dialog.getByRole('button', { name: 'Make user' }).click();
    await expect(dialog.getByText('Use 3 to 32 lower-case letters, digits, hyphens or underscores, beginning with a letter or a digit.')).toBeVisible();
    await expect(dialog.getByText('Give a name.')).toBeVisible();

    await dialog.getByRole('textbox', { name: 'Username' }).fill('rosalind');
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('Rosalind Franklin');
    await dialog.getByRole('textbox', { name: /^Address/ }).fill('rosalind@example.org');
    await dialog.getByRole('checkbox', { name: 'Only they are in it' }).uncheck();
    await dialog.getByRole('button', { name: 'Make user' }).click();

    const made = page.getByRole('dialog', { name: 'Rosalind Franklin is made' });
    await expect(made).toBeVisible();
    await expect(made.getByRole('textbox', { name: 'Link' })).toHaveValue(/\/reset\/reset-[0-9a-f-]+$/);
    await expect(made.getByRole('button', { name: 'Copy link' })).toBeVisible();
    await made.getByRole('button', { name: 'Done' }).click();

    await expect(row(page, /Rosalind Franklin/)).toContainText('rosalind@example.org');
    await expect(row(page, /Rosalind Franklin/)).toContainText('Not confirmed yet');

    // Their own org takes members, because the first tick was taken off.
    await page.goto('/admin/orgs?q=Rosalind');
    await expect(row(page, /Rosalind Franklin/).getByRole('switch', { name: 'Takes members' })).toBeChecked();
    await expect(row(page, /Rosalind Franklin/).getByRole('switch', { name: 'Holds orgs inside it' })).not.toBeChecked();

    // A name that is taken is said at the field, and nothing is made twice.
    await page.goto('/admin/users');
    await page.getByRole('button', { name: 'New user' }).click();
    const again = page.getByRole('dialog', { name: 'Make a new user' });
    await again.getByRole('textbox', { name: 'Username' }).fill('rosalind');
    await again.getByRole('textbox', { name: 'Name', exact: true }).fill('Somebody Else');
    await again.getByRole('button', { name: 'Make user' }).click();
    await expect(again.getByText(/taken/i).first()).toBeVisible();
  });

  test('suspends a person after saying what it means, and resumes them', async ({ page }) => {
    await open(page, '/admin/users');
    await openMenu(page, 'Linus Pauling');
    await page.getByRole('menuitem', { name: 'Suspend…' }).click();
    const suspend = page.getByRole('dialog', { name: 'Suspend Linus Pauling?' });
    await expect(suspend.getByText('They cannot sign in anywhere on this system. What they own stays theirs.')).toBeVisible();
    await suspend.getByRole('button', { name: 'Suspend Linus Pauling' }).click();

    await expect(notices(page).getByText('Linus Pauling is suspended')).toBeVisible();
    await expect(row(page, /Linus Pauling/)).toContainText('Suspended');

    // The list can be narrowed to those who may not sign in, and the narrowing is in the address.
    await page.getByRole('radio', { name: 'Suspended' }).click();
    await expect(page).toHaveURL(/status=suspended/);
    await expect(row(page, /Linus Pauling/)).toBeVisible();
    await expect(row(page, /Grace Hopper/)).toHaveCount(0);

    await openMenu(page, 'Linus Pauling');
    await page.getByRole('menuitem', { name: 'Resume…' }).click();
    const resume = page.getByRole('dialog', { name: 'Resume Linus Pauling?' });
    await resume.getByRole('button', { name: 'Resume Linus Pauling' }).click();
    await expect(notices(page).getByText('Linus Pauling can sign in again')).toBeVisible();
    await page.getByRole('radio', { name: 'All' }).click();
    await expect(row(page, /Linus Pauling/)).not.toContainText('Suspended');
  });

  test('keeps the last system admin: she cannot be suspended or unmade, and each change to a system admin is acknowledged first', async ({ page }) => {
    await open(page, '/admin/users');
    await openMenu(page, 'Ada Lovelace');
    await page.getByRole('menuitem', { name: 'Suspend…' }).click();
    const suspend = page.getByRole('dialog', { name: 'Suspend Ada Lovelace?' });
    const confirm = suspend.getByRole('button', { name: 'Suspend Ada Lovelace' });
    await expect(confirm).toBeDisabled();
    await suspend.getByRole('checkbox', { name: /^This person is a system admin/ }).check();
    await expect(confirm).toBeEnabled();
    await confirm.click();
    await expect(suspend.getByText('The last system administrator cannot be removed. Name another one first.')).toBeVisible();
    await suspend.getByRole('button', { name: 'Cancel' }).click();
    await expect(row(page, /Ada Lovelace/)).not.toContainText('Suspended');

    await openMenu(page, 'Ada Lovelace');
    await page.getByRole('menuitem', { name: 'Remove system admin…' }).click();
    const unmake = page.getByRole('dialog', { name: /^Take away the system admin role of Ada Lovelace/ });
    await unmake.getByRole('checkbox', { name: /^This person is a system admin/ }).check();
    await unmake.getByRole('button', { name: /^Take away the role of Ada Lovelace/ }).click();
    await expect(unmake.getByText('The last system administrator cannot be removed. Name another one first.')).toBeVisible();
    await unmake.getByRole('button', { name: 'Cancel' }).click();

    // Making somebody a system admin says what it is and asks to be acknowledged; with a second admin the first can be unmade.
    await openMenu(page, 'Grace Hopper');
    await page.getByRole('menuitem', { name: 'Make system admin…' }).click();
    const grant = page.getByRole('dialog', { name: 'Make Grace Hopper a system admin?' });
    await expect(grant.getByText('A system admin is an Admin of every org here.')).toBeVisible();
    await expect(grant.getByRole('button', { name: 'Make Grace Hopper a system admin' })).toBeDisabled();
    await grant.getByRole('checkbox', { name: /^I understand that a system admin can read and change everything/ }).check();
    await grant.getByRole('button', { name: 'Make Grace Hopper a system admin' }).click();
    await expect(row(page, /Grace Hopper/)).toContainText('System admin');

    await openMenu(page, 'Ada Lovelace');
    await page.getByRole('menuitem', { name: 'Remove system admin…' }).click();
    const second = page.getByRole('dialog', { name: /^Take away the system admin role of Ada Lovelace/ });
    await second.getByRole('checkbox', { name: /^This person is a system admin/ }).check();
    await second.getByRole('button', { name: /^Take away the role of Ada Lovelace/ }).click();
    await expect(notices(page).getByText('Ada Lovelace is no system admin')).toBeVisible();
  });

  test('makes a link to a new password, with its tick for a second step that was lost, and says what to tell somebody whose name is locked', async ({ page }) => {
    await open(page, '/admin/users');

    // A name that strangers' wrong tries have closed says which proof until when, and what to tell the person.
    const locked = row(page, /Bo Lted/);
    await expect(locked).toContainText(/Password locked for new browsers until \d{2}:\d{2}/);
    await expect(locked).toContainText('They can sign in where they signed in before, or with a reset link.');

    await openMenu(page, 'Grace Hopper');
    await page.getByRole('menuitem', { name: 'Reset link…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Reset link for Grace Hopper' });
    const tick = dialog.getByRole('checkbox', { name: 'Also switch their second step off (they lost their phone or their mailbox)' });
    await expect(tick).toBeEnabled();
    await tick.check();
    await dialog.getByRole('button', { name: 'Make the link' }).click();
    await expect(dialog.getByRole('textbox', { name: 'Link' })).toHaveValue(/\/reset\/reset-/);
    await expect(dialog.getByText(/^Good until /)).toBeVisible();
    await expect(dialog.getByText('Using this link also switches their second step off.')).toBeVisible();
    await dialog.getByRole('button', { name: 'Done' }).click();

    // Somebody with no second step has nothing to switch off.
    await openMenu(page, 'Vera Rubin');
    await page.getByRole('menuitem', { name: 'Reset link…' }).click();
    const plain = page.getByRole('dialog', { name: 'Reset link for Vera Rubin' });
    await expect(plain.getByRole('checkbox', { name: /^Also switch their second step off/ })).toBeDisabled();
    await expect(plain.getByText('They have no second step set up.')).toBeVisible();
  });

  test('searches by name and keeps the search in the address', async ({ page }) => {
    await open(page, '/admin/users');
    await page.getByRole('searchbox', { name: 'Search people' }).fill('hopper');
    await expect(page).toHaveURL(/q=hopper/);
    await expect(row(page, /Grace Hopper/)).toBeVisible();
    await expect(row(page, /Ada Lovelace/)).toHaveCount(0);
    await page.getByRole('searchbox', { name: 'Search people' }).fill('nobody at all');
    await expect(page.getByText('Nobody matches.')).toBeVisible();
  });
});

test.describe('Organizations', () => {
  test('switches a root org’s limit and says how many stay, and an org without owners is given one', async ({ page }) => {
    await open(page, '/admin/orgs');
    const acme = row(page, /Acme/);
    const takes = acme.getByRole('switch', { name: 'Takes members' });
    await expect(takes).toBeChecked();
    await takes.click();
    await expect(takes).not.toBeChecked();
    await expect(acme.getByText('No further members. Its 7 people stay.')).toBeVisible();
    await page.reload();
    await expect(row(page, /Acme/).getByRole('switch', { name: 'Takes members' })).not.toBeChecked();
    await row(page, /Acme/).getByRole('switch', { name: 'Takes members' }).click();
    await expect(row(page, /Acme/).getByRole('switch', { name: 'Takes members' })).toBeChecked();

    // The org of a person is closed and flat as shipped, and says nothing of who stays.
    await expect(row(page, /^Ada Lovelace Personal/).getByRole('switch', { name: 'Takes members' })).not.toBeChecked();

    // An org whose owners are gone is given an owner.
    const orphans = row(page, /Orphans Inc/);
    await expect(orphans).toContainText('No owners left');
    await orphans.getByRole('button', { name: 'Name an owner…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Name an owner of Orphans Inc' });
    await dialog.getByRole('combobox', { name: 'Person' }).fill('vera');
    await page.getByRole('option', { name: /Vera Rubin/ }).click();
    await dialog.getByRole('button', { name: 'Name them an owner' }).click();
    await expect(notices(page).getByText('Vera Rubin is an owner of Orphans Inc')).toBeVisible();
    await expect(row(page, /Orphans Inc/)).not.toContainText('No owners left');
    await expect(row(page, /Orphans Inc/)).toContainText('Vera Rubin');
  });

  test('lists the orgs that were deleted and are not purged yet, each with Restore, and every org opens like any link', async ({ page }) => {
    await open(page, '/admin/orgs');
    await expect(page.getByRole('heading', { name: 'Deleted, not purged yet' })).toBeVisible();
    const gone = row(page, /Old Labs/);
    await expect(gone.getByRole('button', { name: 'Restore' })).toBeVisible();
    await gone.getByRole('button', { name: 'Restore' }).click();
    await expect(notices(page).getByText('Old Labs is back, archived, with everything under it')).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Deleted, not purged yet' })).toHaveCount(0);
    await expect(row(page, /Old Labs/)).toContainText('Archived');

    await row(page, /Acme/).getByRole('link', { name: 'Open' }).click();
    await expect(page).toHaveURL(/\/org\/acme$/);
  });

  test("lists team orgs before people's own, and keeps one kind in the address", async ({ page }) => {
    await open(page, '/admin/orgs');
    const table = page.getByRole('table', { name: 'Organizations' });
    await expect(table.getByRole('row').nth(1)).not.toContainText('Personal');
    await page.getByRole('radio', { name: 'Personal' }).click();
    await expect(page).toHaveURL(/orgKind=personal/);
    await expect(table).not.toContainText('Acme');
    await page.reload();
    await expect(page.getByRole('radio', { name: 'Personal' })).toBeChecked();
    await page.getByRole('radio', { name: 'All' }).click();
    await expect(row(page, /Acme/)).toBeVisible();
  });
});

test.describe('Sign-in', () => {
  test('chooses who may register among three cards, and the choice is what the system then says of itself', async ({ page }) => {
    await open(page, '/admin/sign-in');
    const group = page.getByRole('radiogroup', { name: 'Who may register' });
    const invite = group.getByRole('radio', { name: 'Invite' });
    await expect(invite).toBeChecked();
    await expect(group.getByText('An invitation into any org here, a department included, or one of the system\'s. Each root org decides which forms it gives out.')).toBeVisible();
    await expect(page.getByText('Whoever joins gets an org of their own.')).toBeVisible();

    await group.getByRole('radio', { name: 'Open' }).click();
    await expect(group.getByRole('radio', { name: 'Open' })).toBeChecked();
    await page.reload();
    await expect(page.getByRole('radiogroup', { name: 'Who may register' }).getByRole('radio', { name: 'Open' })).toBeChecked();
    await page.getByRole('radiogroup', { name: 'Who may register' }).getByRole('radio', { name: 'Closed' }).click();
    await expect(page.getByRole('radiogroup', { name: 'Who may register' }).getByRole('radio', { name: 'Closed' })).toBeChecked();
  });

  test('chooses who may make a team org; an invitation to make one is given and taken back in Users, and somebody who may not make one is offered to join', async ({ page }) => {
    await open(page, '/admin/sign-in');
    const makers = page.getByRole('radiogroup', { name: 'Who may make an org' });
    await expect(makers.getByRole('radio', { name: 'Anyone' })).toBeChecked();
    await makers.getByRole('radio', { name: 'Invited' }).click();
    await expect(makers.getByRole('radio', { name: 'Invited' })).toBeChecked();
    await expect(page.getByText('A system admin gives such an invitation from Users; it is used once.')).toBeVisible();
    await expect(page.getByText("A person's own org is made with them whatever is chosen here.")).toBeVisible();

    await page.goto('/admin/users');
    await openMenu(page, 'Vera Rubin');
    await page.getByRole('menuitem', { name: /Invite to found an org/ }).click();
    await expect(notices(page).getByText('Vera Rubin may make one team org.')).toBeVisible();
    await expect(row(page, /Vera Rubin/)).toContainText('Invited to make an org');
    await openMenu(page, 'Vera Rubin');
    await page.getByRole('menuitem', { name: /Take back the invitation/ }).click();
    await expect(row(page, /Vera Rubin/)).not.toContainText('Invited to make an org');

    await page.getByRole('button', { name: 'Your account' }).click();
    await page.getByRole('menuitem', { name: 'Sign out' }).click();
    await page.waitForURL(/\/login/);
    await signIn(page, PEOPLE.vera);
    // Somebody who may not make a team org reads why, and is offered to join one.
    await page.getByRole('button', { name: 'Join an org' }).click();
    await expect(page.getByRole('menuitem', { name: /New org/ })).toBeDisabled();
    await expect(page.getByText('A system admin can invite you to make a team org here')).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Join with a link…' })).toBeEnabled();
  });

  test('keeps the other settings where they are shown: confirming addresses, passwords, and what a code by mail may be for with the lock', async ({ page }) => {
    await open(page, '/admin/sign-in');
    await page.getByRole('switch', { name: 'Ask new people to confirm their address' }).click();
    await expect(page.getByRole('switch', { name: 'Ask new people to confirm their address' })).toBeChecked();

    // A code by mail: the two ticks are the system's own value, and the lock holds every org to it.
    const code = page.getByRole('group', { name: 'A code by mail', exact: true });
    const signInByMail = code.getByRole('checkbox', { name: 'Sign in', exact: true });
    await expect(signInByMail).toHaveAccessibleDescription('A way to sign in without the password');
    await signInByMail.check();
    await expect(signInByMail).toBeChecked();
    await page.getByRole('switch', { name: 'Hold every org to it' }).click();
    await expect(page.getByRole('switch', { name: 'Hold every org to it' })).toBeChecked();
    await page.reload();
    await expect(signInByMail).toBeChecked();
    const secondStep = code.getByRole('checkbox', { name: 'Second step', exact: true });
    await expect(secondStep).toHaveAccessibleDescription('A second step after the password, for whoever switches it on');
    await expect(secondStep).not.toBeChecked();
    await expect(page.getByRole('switch', { name: 'Hold every org to it' })).toBeChecked();

    // The line that says how wrong tries are counted is there, since it is asked.
    await expect(page.getByText(/^Eight wrong tries at one name in three hours lock that proof for that name/)).toBeVisible();

    // A setting changed here can be put back to what Orch ships.
    const set = page.getByText('Set for this system');
    const before = await set.count();
    expect(before).toBeGreaterThan(0);
    await page.getByRole('button', { name: 'Put back what Orch ships' }).first().click();
    await expect(set).toHaveCount(before - 1);
  });

  test('says that nobody but a system admin could sign in when passwords are switched off and no provider is on', async ({ page }) => {
    await open(page, '/admin/sign-in');
    await expect(page.getByRole('table', { name: 'Identity providers' })).toBeVisible();
    await page.getByRole('switch', { name: 'Sign in with a password' }).click();
    await expect(page.getByRole('switch', { name: 'Sign in with a password' })).not.toBeChecked();
    // A provider is on, so passwords can go.
    await expect(page.getByText(/^No identity provider is switched on/)).toHaveCount(0);

    await row(page, /Acme single sign-on/).getByRole('button', { name: /^Actions for Acme single sign-on/ }).click();
    await page.getByRole('menuitem', { name: 'Change…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Change Acme single sign-on' });
    await dialog.getByRole('switch', { name: 'Switched on' }).click();
    await dialog.getByRole('button', { name: 'Save the changes' }).click();
    await expect(notices(page).getByText('Acme single sign-on is changed')).toBeVisible();
    await expect(page.getByText(/^No identity provider is switched on/)).toBeVisible();
  });

  test('adds an identity provider with its callback address to copy, and removes it after saying what happens', async ({ page }) => {
    await open(page, '/admin/sign-in');
    await expect(row(page, /Acme single sign-on/)).toContainText('/auth/oidc/acme-sso/callback');
    await row(page, /Acme single sign-on/).getByRole('button', { name: 'Actions for Acme single sign-on', exact: true }).click();
    const tested = page.waitForResponse((response) => response.request().method() === 'POST' && /\/api\/v1\/admin\/identity-providers\/[^/]+\/test$/u.test(new URL(response.url()).pathname));
    await page.getByRole('menuitem', { name: 'Test…', exact: true }).click();
    const result = await tested; expect(result.status()).toBe(200); expect(await result.json()).toMatchObject({ ok: true, error: null });
    const testDialog = page.getByRole('dialog', { name: 'Test Acme single sign-on', exact: true });
    await expect(testDialog.getByText(/Found it: issuer .*1 signing key\./u)).toBeVisible();
    await testDialog.getByRole('button', { name: 'Close', exact: true }).and(testDialog.locator('[data-action="dismiss"]')).click();

    await page.getByRole('button', { name: 'Add an identity provider' }).click();
    const dialog = page.getByRole('dialog', { name: 'Add an identity provider' });
    await dialog.getByRole('button', { name: 'Add the provider' }).click();
    await expect(dialog.getByText('Use 2 to 32 lower-case letters, digits or hyphens, beginning with a letter.')).toBeVisible();
    await expect(dialog.getByText('Give an https address without a query or a fragment (http only for this machine).')).toBeVisible();

    await dialog.getByRole('textbox', { name: 'Key' }).fill('corp-sso');
    await dialog.getByRole('textbox', { name: /^Name of the provider/ }).fill('Corporate sign-on');
    await dialog.getByRole('textbox', { name: 'Issuer' }).fill('http://insecure.example');
    await dialog.getByLabel('Client id').fill('orch');
    await dialog.getByLabel('Client secret').fill('s3cret');
    await dialog.getByRole('button', { name: 'Add the provider' }).click();
    await expect(dialog.getByText('Give an https address without a query or a fragment (http only for this machine).')).toBeVisible();
    await dialog.getByRole('textbox', { name: 'Issuer' }).fill('https://id.corp.example');
    await dialog.getByRole('button', { name: 'Add the provider' }).click();

    const added = row(page, /Corporate sign-on/);
    await expect(added).toContainText('/auth/oidc/corp-sso/callback');
    await expect(added.getByRole('button', { name: /^Copy the callback address of Corporate sign-on/ })).toBeVisible();

    await added.getByRole('button', { name: /^Actions for Corporate sign-on/ }).click();
    await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click();
    const remove = page.getByRole('alertdialog', { name: 'Delete Corporate sign-on?' });
    await expect(remove.getByText(/A provider cannot be deleted while it is someone's only sign-in method\./u)).toBeVisible();
    await remove.getByRole('button', { name: 'Delete Corporate sign-on' }).click();
    await expect(row(page, /Corporate sign-on/)).toHaveCount(0);
  });
});

test.describe('Hosts and runtimes', () => {
  test('marks a host that is full and says what being full does there, as warning and as refusal, from the settings shown above', async ({ page }) => {
    await open(page, '/admin/hosts');
    const main = row(page, /^main/);
    const box = row(page, /build-box/);
    const lab = row(page, /lab-gpu/);
    await expect(main).not.toContainText('Full');
    await expect(box).toContainText('Full');
    await expect(box).toContainText('Warns, and lets the host answer');
    // A host that was given its own word keeps it, and says so.
    await expect(lab).toContainText('Takes nothing more');
    await expect(lab).toContainText('(its own choice)');
    await expect(main.getByRole('meter', { name: 'Memory declared on main' })).toBeVisible();
    await expect(main).toContainText('24 GiB of 64 GiB');

    // The system's word, changed where it is shown: every host that says nothing of its own follows it.
    await page.getByRole('combobox', { name: 'Where a host says nothing of its own, being full' }).click();
    await page.getByRole('option', { name: 'Block', exact: true }).click();
    await expect(row(page, /build-box/)).toContainText('Takes nothing more');
    await expect(row(page, /build-box/)).not.toContainText('(its own choice)');

    // From what share a host counts as full, with its range said where it is wrong.
    const percent = page.getByRole('spinbutton', { name: 'A host counts as full from' });
    await percent.fill('0');
    await expect(page.getByText('A whole number from 1 to 100.')).toBeVisible();
    await percent.fill('30');
    await expect(row(page, /^main/)).toContainText('Full');
    await page.getByRole('button', { name: 'Put back what Orch ships' }).first().click();
    await expect(page.getByRole('spinbutton', { name: 'A host counts as full from' })).toHaveValue('90');
    await expect(row(page, /^main/)).not.toContainText('Full');
  });

  test('lists both pools of units, narrowed from the address, with what each is behind in', async ({ page }, info) => {
    await open(page, '/admin/runtimes');
    await expect(row(page, /Main/).first()).toContainText('Running');
    await expect(row(page, /GPU lab/).first()).toContainText('The image could not be pulled: unauthorized');
    await expect(row(page, /Claude Max runtime/)).toContainText('Behind in 2 things');
    await expect(page.getByText('2 units are behind. Open one by its name to update it.')).toBeVisible();
    const initialViewport = page.viewportSize();
    for (const width of [1440, 400]) await captureRuntimeWarnings(page, info, width);
    if (initialViewport !== null) await page.setViewportSize(initialViewport);

    await page.getByRole('radio', { name: 'Providers' }).click();
    await expect(page).toHaveURL(/kind=provider/);
    await expect(row(page, /Claude Max runtime/)).toBeVisible();
    await expect(row(page, /Scratch/)).toHaveCount(0);
  });
});

test.describe('Capabilities and harnesses', () => {
  test('adds a capability, changes it, and cannot remove one that ships with Orch', async ({ page }) => {
    await open(page, '/admin/capabilities');
    await expect(row(page, /Privileged container/)).toContainText('Critical');

    await page.getByRole('button', { name: 'Add a capability' }).click();
    const dialog = page.getByRole('dialog', { name: 'Add a capability' });
    await dialog.getByRole('textbox', { name: 'Key' }).fill('fuse');
    await dialog.getByRole('textbox', { name: 'Name', exact: true }).fill('FUSE mounts');
    await dialog.getByRole('textbox', { name: /^Patches, by driver/ }).fill('{ not json');
    await dialog.getByRole('button', { name: 'Add the capability' }).click();
    await expect(dialog.getByText('This is not a JSON object.')).toBeVisible();
    await dialog.getByRole('textbox', { name: /^Patches, by driver/ }).fill('{"docker":{"devices":[{"path":"/dev/fuse","permissions":"rwm"}]}}');
    await dialog.getByRole('button', { name: 'Add the capability' }).click();
    await expect(row(page, /FUSE mounts/)).toBeVisible();

    await page.getByRole('button', { name: 'Actions for FUSE mounts' }).click();
    await page.getByRole('menuitem', { name: 'Change…' }).click();
    const edit = page.getByRole('dialog', { name: 'Change FUSE mounts' });
    await expect(edit.getByRole('textbox', { name: 'Key' })).toHaveValue('fuse');
    await expect(edit.getByRole('textbox', { name: 'Key' })).toHaveAttribute('readonly', '');
    await edit.getByRole('combobox', { name: 'Risk' }).click();
    await page.getByRole('option', { name: 'High' }).click();
    await edit.getByRole('button', { name: 'Save the changes' }).click();
    await expect(row(page, /FUSE mounts/)).toContainText('High');

    // What ships cannot be removed; what was added can.
    await page.getByRole('button', { name: 'Actions for Privileged container' }).click();
    await expect(page.getByRole('menuitem', { name: 'Delete…', exact: true })).toHaveAttribute('aria-disabled', 'true');
    await expect(page.getByText('It ships with Orch, and would come back at the next start.')).toBeVisible();
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Actions for FUSE mounts' }).click();
    await page.getByRole('menuitem', { name: 'Delete…', exact: true }).click();
    await page.getByRole('alertdialog', { name: 'Delete FUSE mounts?' }).getByRole('button', { name: 'Delete FUSE mounts' }).click();
    await expect(row(page, /FUSE mounts/)).toHaveCount(0);
  });

  test('says what each adapter supports from its descriptor, and sets another build as the default of its kind', async ({ page }) => {
    await open(page, '/admin/harnesses');
    const table = page.getByRole('table', { name: 'What each adapter supports' });
    const claude = table.getByRole('row', { name: /Claude Code/ });
    await expect(claude).toContainText("Keeps its own notes in the thread's directory");
    await expect(claude).toContainText('Can be watched');
    const codex = table.getByRole('row', { name: /Codex/ });
    await expect(codex).toContainText('Has one memory for its whole home: kept off; the agent keeps notes with its tools');
    // A member the descriptor does not hold is said as not said, and a descriptor without phases is not given them.
    const acme = table.getByRole('row', { name: /Acme agent/ });
    await expect(acme).toContainText('None: the agent keeps notes with its tools');
    await expect(acme).toContainText('Not said');
    await expect(acme).toContainText('Can be typed into');

    const builds = page.getByRole('table', { name: 'Builds' });
    await expect(builds.getByRole('row', { name: /2\.1\.0/ })).toContainText('Default');
    await builds.getByRole('row', { name: /2\.0\.9/ }).getByRole('button', { name: 'Make default' }).click();
    await expect(builds.getByRole('row', { name: /2\.0\.9/ })).toContainText('Default');
    await expect(builds.getByRole('row', { name: /2\.1\.0/ }).getByRole('button', { name: 'Make default' })).toBeVisible();

    await page.getByRole('button', { name: 'Add a build' }).click();
    const dialog = page.getByRole('dialog', { name: 'Add a build' });
    await dialog.getByRole('button', { name: 'Add the build' }).click();
    await expect(dialog.getByText('Give the version.')).toBeVisible();
    await expect(dialog.getByText('Give an https address.')).toBeVisible();
    await dialog.getByRole('textbox', { name: 'Version' }).fill('2.2.0');
    await dialog.getByRole('textbox', { name: 'Address' }).fill('https://downloads.example/claude_code-2.2.0.tar.gz');
    await dialog.getByRole('textbox', { name: /^Checksum/ }).fill('ab'.repeat(32));
    await dialog.getByRole('button', { name: 'Add the build' }).click();
    await expect(builds.getByRole('row', { name: /2\.2\.0/ })).toContainText('Added here');
  });
});

test.describe('Audit', () => {
  test('is the system’s trail, under Management in the title bar', async ({ page }) => {
    await open(page, '/admin/audit');
    await expect(page.getByRole('heading', { level: 1, name: 'Audit', exact: true })).toBeVisible();
    await expect(page.getByRole('table', { name: 'Audit trail · System', exact: true })).toBeVisible();
    await expect(page.locator('[data-screen-titlebar]:visible')).toContainText('Management');
    await currentSection(page, 'audit', 'Audit');
  });
});

test.describe('the rest of Management', () => {
  test('lists storage, the authority and its download, the plugins and the system’s own agent profiles', async ({ page }) => {
    await open(page, '/admin/storage');
    await expect(page.getByText('Free on the disk').first()).toBeVisible();
    await expect(page.getByRole('table', { name: 'Environments that take the most' }).getByRole('row', { name: /Main/ })).toContainText('11 GiB');

    await page.goto('/admin/pki');
    await expect(page.getByText('A3:F1:C2:D4:E5:B6:07:18:29:3A:4B:5C:6D:7E:8F:90')).toBeVisible();
    await expect(page.getByRole('link', { name: 'Download the CA certificate' })).toHaveAttribute('href', /\/api\/v1\/pki\/ca\.pem$/);
    await expect(page.getByText('1 machine holds a certificate that ends within 30 days')).toBeVisible();
    const certificates = page.getByRole('table', { name: 'Machines with certificates ending soon' });
    await expect(certificates.getByRole('link', { name: 'build-box' })).toHaveAttribute('href', /\/resources\/machines\//);
    await expect(certificates.locator('time')).toHaveCount(1);
    const expiry = await certificates.locator('time').innerText();
    await certificates.getByRole('link', { name: 'build-box' }).click();
    await expect(page.getByRole('heading', { level: 1, name: 'build-box' })).toBeVisible();
    await expect(page).toHaveURL(/\/resources\/machines\/[^?]+\?tab=certificate$/u);
    await expect(page.getByRole('tab', { name: 'Software', exact: true })).toHaveAttribute('aria-selected', 'true');
    await expect(page.getByRole('navigation', { name: 'Software', exact: true })).toHaveAttribute('data-current-section', 'certificate');
    await expect(page.getByRole('region', { name: 'Its certificate', exact: true })).toBeVisible();
    await expect(page.getByText(expiry, { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Enroll again…' }).first()).toBeVisible();

    await page.goto('/admin/plugins');
    const table = page.getByRole('table', { name: 'Plugins' });
    await expect(table.getByRole('row', { name: /podman/ })).toContainText('Restarting');
    await expect(table.getByRole('row', { name: /sandbox-x/ })).toContainText('Protocol 3 is not supported by this release.');

    await page.goto('/admin/agents');
    await expect(page.getByRole('link', { name: 'Claude Code' })).toBeVisible();
    await page.getByRole('button', { name: 'New profile' }).click();
    const dialog = page.getByRole('dialog', { name: 'New profile' });
    await dialog.getByRole('textbox', { name: 'Name' }).fill('Codex for everybody');
    await dialog.getByRole('combobox', { name: 'Adapter' }).click();
    await page.getByRole('option', { name: 'Codex' }).click();
    await dialog.getByRole('button', { name: 'Make the profile' }).click();
    await expect(page.getByRole('link', { name: 'Codex for everybody' })).toBeVisible();
  });

  test('lists current Management destinations at its actual width and rejects an unknown section', async ({ page }) => {
    await open(page, '/admin/settings');
    await expect(page.getByLabel('Search the settings')).toBeVisible();
    const sections = await currentSection(page, 'settings', 'Settings');
    for (const [id, name] of [['email', 'Channels'], ['templates', 'Templates'], ['mcp', 'MCP presets'], ['defaults', 'Access defaults'], ['rates', 'Rates']] as const) {
      const link = sections.locator(`a[data-section-id="${id}"]`);
      await expect(link).toHaveText(name); await expect(link).toHaveAttribute('href', `/admin/${id}`);
    }
    const chooser = sections.getByRole('combobox', { name: 'Management sections', exact: true });
    if (await chooser.isVisible()) {
      await chooser.click(); await chooser.fill('Rates');
      await page.getByRole('listbox', { name: 'Management sections', exact: true }).getByRole('option', { name: 'Rates', exact: true }).click();
    } else await sections.getByRole('link', { name: 'Rates', exact: true }).click();
    await expect(page).toHaveURL(/\/admin\/rates$/u); await expect(page.getByRole('heading', { level: 1, name: 'Rates', exact: true })).toBeVisible();
    await currentSection(page, 'rates', 'Rates');
    await page.goto('/admin/nowhere');
    await expect(page.getByText('There is nothing at this address')).toBeVisible();
  });
});
