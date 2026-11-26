/** Where the application's own address comes back to: the place the person was last in on any device, while it can still be read. */
import { expect, type Page, test } from '@playwright/test';
import { openingTimings } from './opening-timing.ts';

import { agentMessage } from '../doubles/thread-rows.ts';
import { ACME, deny, DOUBLE, emit, PEOPLE, scenario, screenAddress } from './support.ts';

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
const TEAM_HOME = screenAddress('/org/acme');
const THREADS_VIEW = screenAddress(TOPIC, { tab: 'threads' });
const KEPT_THREADS_ROUTE = new RegExp(`^${THREADS_VIEW.source}`);

test.beforeEach(async () => { await scenario(); });

test('signing in comes back to the place last in, with the view it had, wherever it was written', async ({ page }) => {
  await control('/landing', { place: { node_id: ACME.fixLogin, route: `${TOPIC}?tab=threads` } });
  await signInAnywhere(page);
  await page.waitForURL(THREADS_VIEW);
});

test('signing in opens the saved conversation within a second of the login answer at 200 ms', async ({ page }) => {
  const thread = ACME.mainThread;
  const messageText = 'This conversation was saved before signing out.';
  await emit(thread, { thread: { state: 'idle' }, items: [agentMessage('saved-answer', messageText, { turn: null })] });
  await control('/landing', { place: { node_id: thread, route: `/thread/${thread}` } });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Network.enable');
  await cdp.send('Network.emulateNetworkConditions', { offline: false, latency: 200, downloadThroughput: -1, uploadThroughput: -1 });
  const paths: string[] = [];
  page.on('request', (request) => { paths.push(new URL(request.url()).pathname); });
  await signInAnywhere(page);
  const draw = await page.waitForFunction((savedText) => {
    const header = document.querySelector('[data-thread-header]');
    const message = [...document.querySelectorAll('[data-screen="thread"] [role="log"] li[data-index]')]
      .some((row) => row.getBoundingClientRect().height > 0 && row.textContent.includes(savedText));
    const composer = [...document.querySelectorAll('textarea')].find((input) =>
      input.getAttribute('aria-label') === 'Message to the agent' || [...input.labels].some((label) => label.textContent.trim() === 'Message to the agent'));
    const login = performance.getEntriesByType('resource').find((entry) => new URL(entry.name).pathname === '/api/v1/auth/login');
    if (!header?.getBoundingClientRect().height || !header.textContent.trim() || !message || !composer?.getBoundingClientRect().height || composer.disabled || composer.readOnly || login === undefined) return false;
    return performance.now() - (login as PerformanceResourceTiming).responseEnd;
  }, messageText);
  const elapsed = await draw.jsonValue();
  process.stdout.write(`Saved conversation at 200 ms: ${String(elapsed)} ms after login\n`);
  expect(paths.filter((path) => path.endsWith('/me/landing'))).toHaveLength(1);
  expect(paths.filter((path) => path === '/api/v1/me/bootstrap' || path.endsWith('/overview') || path === `/api/v1/threads/${thread}/opening`)).toEqual([]);
  expect(elapsed, await openingTimings(page)).toBeLessThanOrEqual(1000);
});

test('a place that is gone, or can no longer be read, falls back to the org choice', async ({ page }) => {
  await control('/landing', { place: { node_id: '00000000-0000-4000-8000-ffffffffffff', route: '/topic/00000000-0000-4000-8000-ffffffffffff' } });
  await signInAnywhere(page);
  await page.waitForURL(TEAM_HOME);

  // The place stays where it is, and the person may no longer read it.
  await control('/landing', { place: { node_id: ACME.fixLogin, route: TOPIC } });
  await deny(PEOPLE.ada.login, ['topic.view']);
  await page.goto('/');
  await page.waitForURL(TEAM_HOME);
});

test('an address outside the application is never followed', async ({ page }) => {
  await control('/landing', { place: { node_id: ACME.fixLogin, route: 'https://elsewhere.example/topic' } });
  await signInAnywhere(page);
  await page.waitForURL(TEAM_HOME);
});

test('where the person goes is kept by the server, and the application\'s own address opens it again', async ({ page }) => {
  await signInAnywhere(page);
  await page.waitForURL(TEAM_HOME);
  await page.goto(`${TOPIC}?tab=threads`);
  // A fresh page writes at once; after that, at most once in ten seconds.
  await expect.poll(keptPlace, { timeout: 15_000 }).toEqual({ node_id: ACME.fixLogin, route: expect.stringMatching(KEPT_THREADS_ROUTE) });
  await page.goto('/');
  await page.waitForURL(THREADS_VIEW);
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
