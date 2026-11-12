/** Every picker built on the shared Combobox keeps its room, in a dialog low on a tall screen and on a short one, wide and narrow:
 * the popup stays on the screen, off its field, with the rows it promises whole and the active row whole after each move. */
import { expect, test, type Page, type TestInfo } from '@playwright/test';
import type { AuthResult, IdentityProvider } from '../../src/generated/api-types.ts';
import words from '../../src/i18n/en.json' with { type: 'json' };
import { asAdmin } from './admin-mode-support.ts';
import { change, openBlocked, wizard } from './change-support.ts';
import { dropdownShown, expectRoomKept } from './dropdown-geometry.ts';
import { PEOPLE, scenario, signIn } from './support.ts';

const api = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/api/v1`;
const SCREENS = [[1440, 900], [1440, 600], [400, 900], [400, 600]] as const;

/** The screen as it stands, kept beside the case's output for a person to look at. */
async function look(page: Page, info: TestInfo, name: string) {
  const path = info.outputPath(`${name}.png`);
  await page.screenshot({ path });
  await info.attach(name, { path, contentType: 'image/png' });
}

/** Moves through the list by keyboard: each row reached is wholly inside the popup. */
async function walk(page: Page, field: ReturnType<Page['getByRole']>, label: string) {
  for (const key of ['ArrowDown', 'ArrowDown', 'ArrowDown', 'End', 'Home']) {
    await page.keyboard.press(key);
    expect((await dropdownShown(field)).active, `${label}: the active row is whole after ${key}`).toBe(true);
  }
}

for (const [width, height] of SCREENS) {
  const label = `${width}x${height}`;

  test(`the Change dialog's provider picker keeps its room (${label})`, async ({ page }, info) => {
    await scenario();
    await change.mode('manual');
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await openBlocked(page);
    await page.getByRole('button', { name: 'Switch provider…' }).click();
    const dialog = wizard(page, 'Switch “Main thread”');
    await expect(dialog).toBeVisible();
    const pair = dialog.getByRole('combobox', { name: 'Agent and provider', exact: true });
    await pair.click();
    await expect(page.getByRole('option').first()).toBeVisible();
    await expectRoomKept(pair, `${label} opened`);
    await look(page, info, `change-picker-opened-${label}`);
    await pair.fill('Codex');
    await expect(page.getByRole('option').first()).toBeVisible();
    await expectRoomKept(pair, `${label} narrowed`);
    await walk(page, pair, label);
    await look(page, info, `change-picker-walked-${label}`);
  });

  test(`the finder's Where field keeps its room (${label})`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await signIn(page);
    await page.keyboard.press('Control+k');
    const finder = page.getByRole('dialog', { name: words.finder.title, exact: true });
    await expect(finder).toBeVisible();
    await finder.getByRole('combobox').fill('> new thread');
    await finder.locator('[role="option"][data-value="commands:create.thread"]').click();
    const dialog = page.getByRole('dialog', { name: words.commands.create.thread, exact: true });
    const where = dialog.getByRole('combobox', { name: words.commands.create.where, exact: true });
    await where.fill('in');
    await expect(page.getByRole('option').first()).toBeVisible();
    await expectRoomKept(where, `${label} typed`);
    await look(page, info, `finder-where-${label}`);
    await walk(page, where, label);
  });

  test(`the admin default-org picker keeps its room (${label})`, async ({ page, request }, info) => {
    await scenario();
    const login = await request.post(`${api}/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
    expect(login.status()).toBe(200);
    const headers = { Authorization: `Bearer ${((await login.json()) as AuthResult).access_token}` };
    expect((await request.put(`${api}/me/admin-mode`, { headers, data: { as_admin: true } })).status()).toBe(200);
    // Enough orgs for a list far longer than any popup, as the real inventory is.
    for (let n = 1; n <= 30; n += 1) {
      const made = await request.post(`${api}/orgs`, { headers: { ...headers, 'Idempotency-Key': `room-${n}` }, data: { name: `Room ${String(n).padStart(2, '0')}`, slug: `room-${n}` } });
      expect(made.status()).toBe(200);
    }
    const providers = await request.get(`${api}/admin/identity-providers`, { headers });
    const provider = ((await providers.json()) as IdentityProvider[])[0];
    if (provider === undefined) throw new Error('The normal native fixture has no IDP to edit');
    await page.setViewportSize({ width, height });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await signIn(page); await asAdmin(page, true); await page.goto('/admin/sign-in');
    await page.getByRole('button', { name: words.admin.idp.menuOf.replace('{{name}}', provider.name), exact: true }).click();
    await page.getByRole('menuitem', { name: words.admin.idp.edit, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.admin.idp.editTitle.replace('{{name}}', provider.name), exact: true });
    const field = dialog.getByRole('combobox', { name: words.admin.idp.defaultOrg, exact: true });
    await field.scrollIntoViewIfNeeded();
    await field.click();
    await expect(page.getByRole('option', { name: /^Room 01/ })).toBeVisible();
    await expectRoomKept(field, `${label} opened`);
    await look(page, info, `admin-org-opened-${label}`);
    await field.fill('Room');
    await expect(page.getByRole('option', { name: /^Room 01/ })).toBeVisible();
    await expectRoomKept(field, `${label} narrowed`);
    await walk(page, field, label);
    await look(page, info, `admin-org-walked-${label}`);
  });
}
