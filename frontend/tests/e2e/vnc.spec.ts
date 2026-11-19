/** The VNC view against the fake world, whose displays speak RFB 3.8 to the real viewer and note what reached them: a display Orch opened and one an agent announced with its sentence, the viewer connecting and drawing, watching that sends nothing, Work in it, two viewers, closing each kind in its own words, nothing found and off, keyboard and axe in both themes. */
import { expect, test, type Page } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { expectAccessibleInBothThemes, PEOPLE, scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41810'}`;

interface Received { by: string; control: boolean; dropped: boolean; kind: number }

async function control<T = unknown>(path: string, body: Record<string, unknown> = {}): Promise<T> {
  const reply = await fetch(`${DOUBLE}/__double/displays${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  expect(reply.ok).toBe(true);
  return (reply.status === 204 ? null : await reply.json()) as T;
}

async function open(page: Page, environment: string = RUNS_ON.workspace, person: Parameters<typeof signIn>[1] = PEOPLE.ada) {
  await signIn(page, person);
  await page.goto(`/resources/environments/${environment}?tab=vnc`);
  await expect(page.locator('[data-displays-view]')).toBeVisible();
}

/** The viewer's canvas, once the viewer connected. */
async function drawn(page: Page) {
  const canvas = page.locator('[data-vnc-viewer] canvas');
  await expect(canvas).toBeVisible();
  await expect(page.getByText('Loading the viewer…')).toHaveCount(0);
  await expect(page.getByText('Connecting…')).toHaveCount(0);
  return canvas;
}

test.beforeEach(async () => {
  await scenario();
});

test('a display Orch opened shows its line for programs, one an agent announced its sentence, and the viewer draws', async ({ page }) => {
  await open(page);
  const list = page.getByRole('list', { name: 'Displays' });
  await expect(list.locator('[data-display-row]')).toHaveCount(2);
  await drawn(page);
  await expect(page.locator('[data-display-front]')).toContainText('DISPLAY=:1');
  await expect(page.locator('[data-display-copied]')).toHaveCount(0);
  await list.locator('[data-display-row]').filter({ hasText: 'emulator' }).getByRole('button').first().click();
  await expect(page.locator('[data-display-copied]')).toHaveText('What is copied in this display may reach everybody who watches it.');
  await drawn(page);
  const listed = await control<{ name: string; clients: number }[]>('/list');
  expect(listed.find((display) => display.name === 'emulator')?.clients).toBeGreaterThan(0);
});

test('watching sends no key and no pointer; Work in it sends them', async ({ page }) => {
  await open(page);
  const canvas = await drawn(page);
  await canvas.click();
  await page.keyboard.type('abc');
  expect((await control<Received[]>('/received', { name: 'Desktop' })).filter((entry) => entry.kind === 4 || entry.kind === 5)).toEqual([]);
  await page.getByRole('button', { name: 'Work in it' }).click();
  await expect(page.locator('[data-vnc-viewer="working"]')).toBeVisible();
  const working = await drawn(page);
  await working.click();
  await page.keyboard.type('x');
  await expect.poll(async () => (await control<Received[]>('/received', { name: 'Desktop' })).filter((entry) => entry.control && !entry.dropped).map((entry) => entry.kind)).toEqual(expect.arrayContaining([4, 5]));
  await page.keyboard.press('Control+Alt+Escape');
  await expect(page.getByRole('button', { name: 'Work in it' })).toBeVisible();
});

test('two viewers are two clients of the display', async ({ page, browser }) => {
  await open(page);
  await drawn(page);
  const second = await browser.newContext();
  try {
    const other = await second.newPage();
    await open(other, RUNS_ON.workspace, PEOPLE.dora);
    await drawn(other);
    await expect.poll(async () => (await control<{ name: string; clients: number }[]>('/list')).find((display) => display.name === 'Desktop')?.clients).toBe(2);
  } finally {
    await second.close();
  }
});

test('closing each kind says what happens to it', async ({ page }) => {
  await open(page);
  const rows = page.locator('[data-display-row]');
  await rows.filter({ hasText: 'emulator' }).getByRole('button', { name: 'More for this display' }).click();
  await page.getByRole('menuitem', { name: 'Close…' }).click();
  await expect(page.locator('[data-close-words="announced"]')).toHaveText('It is no longer shown here. Its program runs on.');
  await page.getByRole('button', { name: 'Stop showing it' }).click();
  await expect(rows.filter({ hasText: 'emulator' })).toHaveCount(0);
  await rows.filter({ hasText: 'Desktop' }).getByRole('button', { name: 'More for this display' }).click();
  await page.getByRole('menuitem', { name: 'Close…' }).click();
  await expect(page.locator('[data-close-words="started"]')).toHaveText('What shows on it ends with it.');
  await page.getByRole('button', { name: 'Close the display' }).click();
  await expect(rows).toHaveCount(0);
});

test('nothing found and each way of being off', async ({ page }) => {
  await open(page, RUNS_ON.sandbox);
  await expect(page.locator('[data-displays-none]')).toContainText('Looked for:');
  for (const off of ['environment_switch_off', 'disabled_by_policy', 'machine_policy'] as const) {
    await control('/place', { environment: RUNS_ON.workspace, off });
    await page.goto(`/resources/environments/${RUNS_ON.workspace}?tab=vnc`);
    await expect(page.locator(`[data-displays-off="${off}"]`)).toBeVisible();
  }
});

test('the view by keyboard as far as the viewer allows, and with axe in both themes', async ({ page }) => {
  await open(page);
  await drawn(page);
  // The viewer's own canvas takes the keys while one works in it; the controls round it are buttons and a menu.
  await page.getByRole('button', { name: 'Work in it' }).focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('[data-vnc-viewer="working"]')).toBeVisible();
  await page.keyboard.press('Control+Alt+Escape');
  await expect(page.getByRole('button', { name: 'Work in it' })).toBeVisible();
  await expectAccessibleInBothThemes(page, '[data-displays-view]');
});
