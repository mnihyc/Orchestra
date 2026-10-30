/** A name leads to a real, privacy-filtered page; the person edits it and private inspection is explicit. */
import { expect, test } from '@playwright/test';
import { asAdmin } from './admin-mode-support.ts';
import { bodySent, ACME, expectAccessibleSettled, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function configure(path: string, body: unknown) { const response = await fetch(`${DOUBLE}/__double/profile${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }); expect(response.ok).toBeTruthy(); }
test.beforeEach(async () => { await scenario(); });

test('the person edits their page, pins a visible place, and reads every day as a table', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto('/settings/your-page');
  await expect(page.getByRole('region', { name: 'Pinned', exact: true })).toHaveCount(1);
  await expect(page.getByRole('region', { name: 'Pin preview', exact: true })).toBeVisible();
  await expect(page.getByRole('complementary', { name: 'About you in the page preview', exact: true })).toBeVisible();
  await page.getByLabel('About you').fill('Builds **tools**.');
  await page.getByLabel('Pronouns').fill('she/her');
  await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
  await page.getByLabel('Find something to pin').fill('Web');
  const web = page.locator(`[data-pin-choice="node:${ACME.web}"]`).getByRole('button');
  await expect(web).toHaveAccessibleName(/^Web(?: · .+)?$/);
  const pinsAccepted = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname.endsWith('/me/profile/pins'));
  await web.click();
  const pinsReply = await pinsAccepted;
  expect(pinsReply.ok()).toBeTruthy();
  const pinsBody: unknown = (await bodySent(pinsReply.request()));
  expect(pinsBody).toEqual({ pins: [{ node_id: ACME.web }] });
  await page.getByRole('link', { name: 'Open page', exact: true }).click();
  await expect(page).toHaveURL(/\/u\/ada$/);
  await expect(page.getByText('Builds tools.').first()).toBeVisible();
  await expect(page.getByRole('region', { name: 'Pinned' }).getByRole('link', { name: 'Web' })).toBeVisible();
  await page.getByRole('button', { name: 'Show as a table' }).click();
  await expect(page.getByRole('table', { name: 'Work by day' }).getByRole('row')).toHaveCount(372);
  await expectAccessibleSettled(page);
});

test('unseen pins are omitted and closed, missing and service pages have the same outside state', async ({ page }) => {
  await configure('/page', { username: 'ada', profile: { pins: [{ node_id: ACME.web }] } });
  await signIn(page, PEOPLE.margaret);
  await page.goto('/u/ada');
  await expect(page.getByRole('region', { name: 'Pinned' }).getByRole('link', { name: 'Web' })).toHaveCount(0);
  await configure('/page', { username: 'ada', profile: { settings: { show_time: true, show_activity: true, audience: 'orgs' } } });
  for (const username of ['ada', 'nobody-has-this-name', 'release-bot']) {
    await page.goto(`/u/${username}`);
    await expect(page.getByRole('heading', { level: 1, name: 'There is no page at this address.' })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Home', exact: true })).toHaveAttribute('href', '/');
  }
});

test('private inspection has its band, and Only me explains the member managers exception', async ({ page }) => {
  await configure('/page', { username: 'dora', profile: { settings: { show_time: false, show_activity: true, audience: 'private' } } });
  await signIn(page, PEOPLE.ada);
  await asAdmin(page, true);
  await page.goto('/u/dora');
  await expect(page.getByRole('img', { name: 'Private · You see this as an admin of Acme' })).toBeVisible();
  await page.goto('/settings/your-page');
  await page.getByRole('combobox', { name: 'Who can open my page' }).click();
  const privateChoice = page.getByRole('option', { name: 'Only me', exact: true });
  await expect(privateChoice).toHaveAccessibleDescription(/Admins who manage members of one of your orgs/);
  const accepted = page.waitForResponse(async (response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname.endsWith('/me/profile') && ((await bodySent(response.request())) as { audience?: string }).audience === 'private');
  await privateChoice.click();
  await expect(page.getByText(/Admins who manage members of one of your orgs/)).toHaveCount(0);
  const activity = page.getByRole('checkbox', { name: 'Show activity to thread readers' });
  await expect(activity).not.toHaveAttribute('aria-describedby');
  const reply = await accepted;
  expect(reply.ok()).toBeTruthy();
  const saved: unknown = (await bodySent(reply.request()));
  expect(saved).toMatchObject({ audience: 'private' });
  const activityAccepted = page.waitForResponse(async (response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname.endsWith('/me/profile') && ((await bodySent(response.request())) as { show_activity?: boolean }).show_activity === false);
  await activity.uncheck();
  expect((await activityAccepted).ok()).toBeTruthy();
});

test('a name card opens with the keyboard, links to the page and returns focus on Escape', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto('/dev/ui');
  const name = page.getByRole('button', { name: 'Ada Lovelace', exact: true }).first();
  await name.focus();
  await page.keyboard.press('Enter');
  const card = page.getByRole('dialog', { name: 'Ada Lovelace' });
  await expect(card.getByRole('link', { name: 'Open page' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(name).toBeFocused();
});
