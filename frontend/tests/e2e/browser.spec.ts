/** The Browser view against the fake world, whose browsers serve real pictures and note what reached the page: the list, opening and joining, watching that reaches nothing, driving at the right place at two panel sizes, Ctrl+Alt+Esc, a tab the page opened, a dialog and a file asked for, the reduced picture, two people, nothing found and off, a profile renamed and deleted, and axe in both themes. */
import { expect, test, type Page } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41810'}`;

interface Received { by: string; control: boolean; dropped: boolean; frame: { op: string; kind?: string; x?: number; y?: number; key?: string; text?: string; accept?: boolean; paths?: string[] | null } }

async function control<T = unknown>(path: string, body: Record<string, unknown> = {}): Promise<T> {
  const reply = await fetch(`${DOUBLE}/__double/browsers${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  expect(reply.ok).toBe(true);
  return (reply.status === 204 ? null : await reply.json()) as T;
}

const received = (profile: string) => control<Received[]>('/received', { profile });

async function open(page: Page, environment: string = RUNS_ON.workspace, person: Parameters<typeof signIn>[1] = PEOPLE.ada) {
  await signIn(page, person);
  await page.goto(`/resources/environments/${environment}?tab=browsers`);
  await expect(page.locator('[data-browsers-view]')).toBeVisible();
}

/** The picture of the browser in front, once a first picture was drawn on it. */
async function picture(page: Page) {
  const canvas = page.locator('[data-browser-front] canvas');
  await expect(canvas).toBeVisible();
  await expect(page.getByText('Waiting for the first picture…')).toHaveCount(0);
  return canvas;
}

test.beforeEach(async () => {
  await scenario();
});

test('the list has the open browsers and the profiles none runs on; a new profile is opened and one in use is joined', async ({ page }) => {
  await open(page);
  const list = page.getByRole('list', { name: 'Browsers and profiles' });
  await expect(list.locator('[data-browser-row]')).toHaveCount(2);
  await expect(list).toContainText('opened by Ada Lovelace');
  await expect(list).toContainText('opened by the agent of Main thread');
  await expect(list.locator('[data-profile-row="twitter"]')).toContainText('a profile · not open');
  await page.getByRole('button', { name: 'Open a browser…' }).click();
  const dialog = page.getByRole('dialog', { name: /Open a browser in/ });
  await expect(dialog.locator('[data-signins-sentence]')).toContainText('A browser acts at every site as whoever signed in to it there.');
  await dialog.getByRole('radio', { name: /notion/ }).click();
  await expect(dialog.getByRole('button', { name: 'Join the browser on notion' })).toBeVisible();
  await dialog.getByRole('radio', { name: 'A new profile' }).click();
  await dialog.getByLabel('Name of the profile').fill('shop');
  await dialog.getByRole('button', { name: 'Open the browser' }).click();
  await expect(page.locator('[data-browser-front]')).toHaveAttribute('aria-label', 'The browser on shop');
  await picture(page);
});

test('watching reaches nothing; driving reaches the page at the right place at two sizes, and Ctrl+Alt+Esc gives the keys back', async ({ page }) => {
  await page.setViewportSize({ width: 1400, height: 900 });
  await open(page);
  await page.locator('[data-browser-row]').filter({ hasText: 'notion' }).getByRole('button').first().click();
  const canvas = await picture(page);
  await canvas.click();
  await page.keyboard.type('nothing');
  expect((await received('notion')).filter((entry) => !entry.dropped)).toEqual([]);
  await page.getByRole('button', { name: 'Drive' }).click();
  await expect(page.locator('[data-browser-picture="driving"]')).toBeVisible();
  for (const size of [{ width: 1400, height: 900 }, { width: 900, height: 900 }]) {
    await page.setViewportSize(size);
    await page.waitForTimeout(400);
    const box = await canvas.boundingBox();
    if (box === null) throw new Error('the picture has no box');
    await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
    await expect.poll(async () => (await received('notion')).filter((entry) => entry.frame.op === 'pointer' && entry.frame.kind === 'down').length).toBeGreaterThan(0);
    const down = (await received('notion')).filter((entry) => entry.frame.op === 'pointer' && entry.frame.kind === 'down').at(-1);
    expect(Math.abs((down?.frame.x ?? 0) - 640)).toBeLessThanOrEqual(3);
    expect(Math.abs((down?.frame.y ?? 0) - 400)).toBeLessThanOrEqual(3);
  }
  await page.keyboard.type('hi');
  await expect.poll(async () => (await received('notion')).filter((entry) => entry.frame.op === 'key' && entry.frame.text !== undefined).map((entry) => entry.frame.text).join('')).toContain('hi');
  await page.keyboard.press('Control+Alt+Escape');
  await expect(page.getByRole('button', { name: 'Drive' })).toBeVisible();
});

test('a tab the page opened, a dialog and a file asked for reach whoever drives', async ({ page }) => {
  await open(page);
  await page.locator('[data-browser-row]').filter({ hasText: 'notion' }).getByRole('button').first().click();
  await picture(page);
  await page.getByRole('button', { name: 'Drive' }).click();
  await control('/open-tab', { profile: 'notion', title: 'Sign in to Notion' });
  await expect(page.getByRole('group', { name: 'Tabs of the browser' }).getByRole('button', { name: 'Sign in to Notion' })).toHaveAttribute('aria-pressed', 'true');
  await control('/dialog', { profile: 'notion', kind: 'confirm', message: 'Leave this page?' });
  await expect(page.locator('[data-page-dialog="confirm"]')).toContainText('Leave this page?');
  await page.getByRole('button', { name: 'OK' }).click();
  await expect.poll(async () => (await received('notion')).some((entry) => entry.frame.op === 'dialog' && entry.frame.accept === true)).toBe(true);
  await control('/chooser', { profile: 'notion' });
  await page.getByLabel('The file to give, as a path of the workspace').fill('docs/plan.pdf');
  await page.getByRole('button', { name: 'Give these files' }).click();
  await expect.poll(async () => (await received('notion')).some((entry) => entry.frame.op === 'files' && entry.frame.paths?.[0] === 'docs/plan.pdf')).toBe(true);
});

test('the picture says when it is reduced, and two people are shown under it', async ({ page, browser }) => {
  await open(page);
  await page.locator('[data-browser-row]').filter({ hasText: 'notion' }).getByRole('button').first().click();
  await picture(page);
  await control('/quality', { profile: 'notion', level: 3 });
  await expect(page.locator('[data-browser-reduced]')).toHaveText('Reduced picture');
  const second = await browser.newContext();
  try {
    const other = await second.newPage();
    await open(other, RUNS_ON.workspace, PEOPLE.dora);
    await other.locator('[data-browser-row]').filter({ hasText: 'notion' }).getByRole('button').first().click();
    await expect(page.locator('[data-browser-people]')).toContainText('2 watching');
  } finally {
    await second.close();
  }
});

test('nothing found says what was looked for, and each way of being off says by whom', async ({ page }) => {
  await open(page, RUNS_ON.sandbox);
  await expect(page.locator('[data-browsers-none]')).toContainText('Looked for:');
  await expect(page.getByRole('button', { name: 'Look again' })).toBeVisible();
  for (const off of ['environment_switch_off', 'disabled_by_policy', 'machine_policy'] as const) {
    await control('/place', { environment: RUNS_ON.workspace, off });
    await page.goto(`/resources/environments/${RUNS_ON.workspace}?tab=browsers`);
    await expect(page.locator(`[data-browsers-off="${off}"]`)).toBeVisible();
  }
});

test('a profile is renamed and deleted, with its name typed', async ({ page }) => {
  await open(page);
  const row = page.locator('[data-profile-row="twitter"]');
  await row.getByRole('button', { name: 'Actions for the profile twitter' }).click();
  await page.getByRole('menuitem', { name: 'Rename…' }).click();
  await page.getByLabel('Name of the profile').fill('twitter-team');
  await page.getByRole('button', { name: 'Rename profile' }).click();
  const renamed = page.locator('[data-profile-row="twitter-team"]');
  await expect(renamed).toBeVisible();
  await renamed.getByRole('button', { name: 'Actions for the profile twitter-team' }).click();
  await page.getByRole('menuitem', { name: 'Delete…' }).click();
  await expect(page.getByRole('button', { name: 'Delete profile' })).toBeDisabled();
  await page.getByLabel('Type twitter-team to confirm').fill('twitter-team');
  await page.getByRole('button', { name: 'Delete profile' }).click();
  await expect(renamed).toHaveCount(0);
});

test('the view by keyboard, and with axe in both themes', async ({ page }) => {
  await open(page);
  await picture(page);
  await page.getByRole('button', { name: 'Drive' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('[data-browser-picture="driving"]')).toBeVisible();
  await page.keyboard.press('Control+Alt+Escape');
  await expect(page.getByRole('button', { name: 'Drive' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[data-browsers-view]');
});
