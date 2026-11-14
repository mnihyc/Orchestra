/** General access is chosen at creation; inherited holders are edited on each kind of page itself. */
import { expect, type Page, test } from '@playwright/test';
import type { PermissionPage } from '../../src/generated/api-types.ts';

import { ACME, RUNS_ON } from '../doubles/fixtures.ts';
import { DOCS } from './documents-support.ts';
import { bodySent, PEOPLE, scenario, signIn } from './support.ts';

test.beforeEach(async () => { await scenario(); });
const sharing = (page: Page) => page.getByRole('dialog', { name: /^Access to / });

async function selectGeneral(page: Page, text: string): Promise<void> {
  await sharing(page).getByRole('combobox', { name: 'General access' }).click();
  await page.getByRole('option', { name: text }).click();
  await page.getByRole('alertdialog').getByRole('button', { name: 'Change general access', exact: true }).click();
  await expect(page.getByRole('alertdialog')).toHaveCount(0);
}

/** Retain the real permission replies before dialog/route changes can retire their CDP handles. */
async function capturePermissions(page: Page, id: string) {
  let read!: (value: PermissionPage) => void, written!: (value: PermissionPage) => void;
  const before = new Promise<PermissionPage>((resolve) => { read = resolve; });
  const after = new Promise<PermissionPage>((resolve) => { written = resolve; });
  await page.route((url) => url.pathname.endsWith(`/${id}/permissions`), async (route) => {
    const method = route.request().method();
    if (method !== 'GET' && method !== 'PATCH') { await route.continue(); return; }
    const response = await route.fetch(); expect(response.status()).toBe(200);
    const body = await response.json() as PermissionPage;
    await route.fulfill({ response });
    if (method === 'GET') read(body); else written(body);
  });
  return { before, after };
}

test('a topic member can view the new thread but cannot send, and a person added keeps their own level', async ({ page, browser }) => {
  // EN: Topic → Threads → New thread → View → create → Share thread → retain an explicit person's level.
  // 中文：主题 → 会话 → 新建线程 → 查看 → 创建 → 共享线程 → 保留单独授予人员的权限级别。
  await signIn(page, PEOPLE.ada);
  await page.goto(`/topic/${ACME.fixLogin}`);
  // The topic tab includes its current thread count in its accessible name.
  await page.getByRole('tab', { name: /^Threads(?:\s*\d+)?$/ }).click();
  await page.getByRole('button', { name: 'New thread', exact: true }).first().click();
  const creation = page.getByRole('dialog', { name: 'New thread in Fix login' });
  await creation.getByRole('combobox', { name: 'General access' }).click();
  const viewChoice = page.getByRole('option', { name: 'View', exact: true });
  await expect(viewChoice).toHaveAccessibleDescription('Everyone in Fix login can view');
  await viewChoice.click();
  await creation.getByRole('button', { name: 'Options', exact: true }).click();
  await creation.getByRole('textbox', { name: /^Name\s*\(optional\)$/ }).fill('Readers watch');
  await creation.getByRole('button', { name: 'Create thread' }).click();
  await expect(page).toHaveURL(/\/thread\//);
  const address = page.url();
  const context = await browser.newContext({ baseURL: new URL(address).origin });
  try {
    const member = await context.newPage();
    await signIn(member, PEOPLE.vera);
    await member.goto(address);
    await expect(member.getByRole('heading', { name: 'Readers watch' })).toBeVisible();
    // Viewers keep the common composer, with its input and send controls disabled.
    await expect(member.getByRole('textbox', { name: 'Message to the agent' })).toBeDisabled();
    await expect(member.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
    await page.locator('[data-thread-header] [data-thread-more]').click();
    await page.getByRole('menuitem', { name: 'Share thread', exact: true }).click();
    await sharing(page).getByRole('combobox', { name: 'Add people, groups or roles' }).fill('@vera');
    await page.getByRole('option', { name: /Vera Rubin/ }).click();
    await sharing(page).getByRole('combobox', { name: 'What they can do' }).click();
    await page.getByRole('option', { name: 'Can manage', exact: true }).click();
    await sharing(page).getByRole('button', { name: 'Add', exact: true }).click();
    await expect(sharing(page).getByRole('group', { name: 'Can manage' })).toContainText('Vera Rubin');
    await selectGeneral(page, 'Take part');
    await selectGeneral(page, 'View');
    await expect(sharing(page).getByRole('group', { name: 'Can manage' })).toContainText('Vera Rubin');
    await member.reload();
    await expect(member.getByRole('textbox', { name: 'Message to the agent' })).toBeEnabled();
  } finally { await context.close(); }
});

const KINDS = [
  { kind: 'thread', url: `/thread/${ACME.mainThread}`, name: 'Main thread', line: 'Can interact' },
  { kind: 'document', url: `/doc/${DOCS.plan}`, name: 'Rollout plan', line: 'Can edit' },
  { kind: 'topic', url: `/topic/${ACME.fixLogin}`, name: 'Fix login', line: 'Can manage' },
  { kind: 'environment', url: `/resources/environments/${RUNS_ON.workspace}`, name: 'Workspace', line: 'Can view' },
] as const;

for (const kind of KINDS) test(`${kind.kind}: a followed holder is removed here in one confirmed PATCH, and untouched rows still follow`, async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(kind.url);
  const id = kind.url.split('/').at(-1);
  if (id === undefined) throw new Error('The permission target needs its native identity');
  const opening = await capturePermissions(page, id);
  if (kind.kind === 'thread') {
    await page.locator('[data-thread-header] [data-thread-more]').click();
    await page.getByRole('menuitem', { name: 'Share thread', exact: true }).click();
  } else if (kind.kind === 'document') await page.getByRole('button', { name: 'Share', exact: true }).click();
  else {
    await page.getByRole('button', { name: `Actions for ${kind.name}` }).first().click();
    await page.getByRole('menuitem', { name: 'Share…' }).click();
  }
  const form = sharing(page);
  const before = await opening.before;
  await expect(form.getByRole('combobox', { name: 'General access' })).toBeVisible();
  const rows = form.getByRole('group', { name: kind.line });
  // The roster names the source directly and keeps holder changes in its level menu.
  await expect(rows).toContainText(/Inherited from /);
  await rows.getByRole('button', { name: /^Change .*’s access$/ }).first().click();
  await page.getByRole('menuitem', { name: /^Take .* out$/ }).click();
  const sent = page.waitForRequest((request) => request.method() === 'PATCH' && request.url().endsWith('/permissions'));
  await page.getByRole('alertdialog').getByRole('button', { name: /^Take out / }).click();
  const request = await sent;
  const change = (await bodySent(request)) as { whole?: string; rows: { permission: string; profile?: string; inherit?: boolean }[] };
  expect(change.whole).toBeUndefined();
  expect(change.rows.length).toBeGreaterThan(0);
  expect(change.rows.every((row) => row.inherit === false)).toBe(true);
  await expect(form.getByRole('combobox', { name: 'General access' })).toContainText('Custom');
  await expect(form.getByRole('button', { name: /^Back to .* rules$/ })).toBeVisible();
  // Empty inherited rows no longer appear in the compact roster. Check the
  // native returned rows so those untouched defaults remain part of the proof.
  const after = await opening.after;
  const key = (row: { permission: string; profile?: string }) => `${row.permission}:${row.profile ?? 'any'}`;
  const changed = new Set(change.rows.map(key));
  const untouched = after.rows.filter((row) => !changed.has(key(row)));
  expect(untouched.some((row) => row.inherit)).toBe(true);
  for (const row of untouched) expect(row.inherit).toBe(before.rows.find((old) => key(old) === key(row))?.inherit);
});
