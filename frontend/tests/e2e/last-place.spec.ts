/** Where the application's own address comes back to: the place the person was last in on any device, while it can still be read. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, deny, PEOPLE, scenario } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41860'}`;
async function control(path: string, body: Record<string, unknown> = {}) {
  const response = await fetch(`${DOUBLE}/__double/first-steps${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  expect(response.ok).toBe(true);
  return response.status === 204 ? null : response.json() as Promise<Record<string, unknown>>;
}

/** What the server keeps as the place the person was last in. */
async function keptPlace(): Promise<unknown> {
  const held = await control('/state') as { state: { sidebar?: { last_place?: unknown } } };
  return held.state.sidebar?.last_place ?? null;
}

/** Signs Ada in without saying where she lands: that is what is tested. */
async function signInAnywhere(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

const TOPIC = `/topic/${ACME.fixLogin}`;

test.beforeEach(async () => { await scenario(); });

test('signing in comes back to the place last in, with the view it had, wherever it was written', async ({ page }) => {
  await control('/landing', { place: { node_id: ACME.fixLogin, route: `${TOPIC}?tab=threads` } });
  await signInAnywhere(page);
  await page.waitForURL(new RegExp(`${TOPIC}\\?tab=threads$`));
});

test('a place that is gone, or can no longer be read, falls back to the org choice', async ({ page }) => {
  await control('/landing', { place: { node_id: '00000000-0000-4000-8000-ffffffffffff', route: '/topic/00000000-0000-4000-8000-ffffffffffff' } });
  await signInAnywhere(page);
  await page.waitForURL(/\/org\/acme$/);

  // The place stays where it is, and the person may no longer read it.
  await control('/landing', { place: { node_id: ACME.fixLogin, route: TOPIC } });
  await deny(PEOPLE.ada.login, ['topic.view']);
  await page.goto('/');
  await page.waitForURL(/\/org\/acme$/);
});

test('an address outside the application is never followed', async ({ page }) => {
  await control('/landing', { place: { node_id: ACME.fixLogin, route: 'https://elsewhere.example/topic' } });
  await signInAnywhere(page);
  await page.waitForURL(/\/org\/acme$/);
});

test('where the person goes is kept by the server, and the application\'s own address opens it again', async ({ page }) => {
  await signInAnywhere(page);
  await page.waitForURL(/\/org\/acme$/);
  await page.goto(`${TOPIC}?tab=threads`);
  // A fresh page writes at once; after that, at most once in ten seconds.
  await expect.poll(keptPlace, { timeout: 15_000 }).toEqual({ node_id: ACME.fixLogin, route: `${TOPIC}?tab=threads` });
  await page.goto('/');
  await page.waitForURL(new RegExp(`${TOPIC}\\?tab=threads$`));
});

test('an explicit destination after sign-in wins over the place last in', async ({ page }) => {
  await control('/landing', { place: { node_id: ACME.fixLogin, route: TOPIC } });
  await page.goto('/inbox');
  await page.waitForURL(/\/login/);
  await page.getByLabel('Username or email address').fill(PEOPLE.ada.login);
  await page.getByLabel('Password', { exact: true }).fill(PEOPLE.ada.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL(/\/inbox$/);
});
