/** What the end-to-end tests share: putting the fake API into a scenario, signing in as one of its people, and asking the page whether it is accessible. */
import type { Turn } from '../../src/generated/api-types';
import AxeBuilder from '@axe-core/playwright';
import { expect, type Page, type Request, type Response, test } from '@playwright/test';

import { ACME } from '../doubles/fixtures.ts';
import type { SceneName } from '../doubles/thread-scenes.ts';
import type { Rows } from '../doubles/threads.ts';
import type { Scenario } from '../doubles/world.ts';
import { expectUniqueGuideEntries } from '../accessibility/guide-entries.ts';
import { expectOneScrollerPerArea } from '../accessibility/scrolling.ts';
import { expectOneCurrentSection } from '../accessibility/navigation.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** The people of the fake API's team, with what each proves who they are by. */
export const PEOPLE = {
  ada: { login: 'ada', password: 'correct horse battery', name: 'Ada Lovelace' },
  grace: { login: 'grace', password: 'tabulating machine', totp: '246810' },
  linus: { login: 'linus', password: 'double helix model' },
  margaret: { login: 'margaret', password: 'apollo guidance computer' },
  bolted: { login: 'bolted', password: 'locked out of here' },
  /** Reads, and may change nothing. */
  vera: { login: 'vera', password: 'galaxy rotation curves', name: 'Vera Rubin' },
  /** A developer of Acme: opens topics and manages them. */
  dora: { login: 'dora', password: 'compilers and goto', name: 'Dora Developer' },
} as const;

export { ACME };

async function control(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** The JSON a streamed browser write delivered, read by the fake server and identified by its response. */
export async function bodySent(request: Request): Promise<unknown> {
  const response = await request.response();
  if (response === null) throw new Error(`No response to ${request.method()} ${request.url()}`);
  const id = response.headers()['x-request-id'];
  if (id === undefined) throw new Error(`The fake server did not identify ${request.method()} ${request.url()}`);
  const received = (await control('/requests')) as { id: string; body: unknown }[];
  const write = received.find((entry) => entry.id === id);
  if (write === undefined) throw new Error(`The fake server did not read the body of ${request.method()} ${request.url()}`);
  return write.body;
}

/** The server's exact JSON answer remains readable after the page follows its registration outcome. */
export async function replySent(response: Response): Promise<unknown> {
  const id = response.headers()['x-request-id'];
  if (id === undefined) throw new Error(`The fake server did not identify ${response.url()}`);
  const received = (await control('/requests')) as { id: string; reply?: { status: number; body: unknown } }[];
  const reply = received.find((entry) => entry.id === id)?.reply;
  if (reply?.status !== response.status()) throw new Error(`No matching JSON answer to ${response.url()}`);
  return reply.body;
}

/** Puts the fake API into a scenario: a team at work unless something else is asked for. */
export async function scenario(given: Scenario = {}): Promise<void> {
  await control('/reset', given);
}

/** What the fake API "mailed" last to somebody. */
export async function mailed(to: string, kind: 'code' | 'reset' | 'confirm'): Promise<string> {
  const outbox = (await control('/outbox')) as { to: string; kind: string; secret: string }[];
  const found = outbox.filter((mail) => mail.to === to && mail.kind === kind).at(-1);
  if (found === undefined) throw new Error(`nothing of kind ${kind} was mailed to ${to}`);
  return found.secret;
}

/** Takes permissions from somebody without telling their page: what a page that was loaded earlier still believes. */
export async function deny(username: string, permissions: string[]): Promise<void> {
  await control('/deny', { username, permissions });
}

/** Whether the fake system can send mail, which decides what becomes of a personal invitation. */
export async function mailWorks(works: boolean): Promise<void> {
  await control('/mail', { works });
}

/** Changes nodes of the team's tree as the real system would; `lose` withholds the frame that says so. */
export async function changeTree(nodes: Record<string, unknown>[], lose = false): Promise<void> {
  await control('/tree', { org: ACME.org, nodes, lose });
}

/** Ends every live connection, as a restart of the server does. */
export async function dropConnections(): Promise<void> {
  await control('/drop');
}

/** Makes every access token stop being good at once. */
export async function endTokens(): Promise<void> {
  await control('/end-sessions');
}

/** Rows appear in a thread as if the system made them; `lose` keeps them without telling whoever follows the thread. Gives the sequence they got. */
export async function emit(thread: string, rows: Rows, lose = false): Promise<number> {
  const reply = (await control('/thread/emit', { thread, rows, lose })) as { seq: number };
  return reply.seq;
}

/** A piece of growing text, sent as the system sends it. `offset` and `len` are in code points for text, in bytes for output. */
export async function delta(thread: string, piece: { item_id: string; field: 'text' | 'summary' | 'output'; offset: number; len: number; data: string }): Promise<void> {
  await control('/thread/delta', { thread, ...piece });
}

/** A whole conversation at once, from the scenes the doubles know by name. */
export async function playScene(thread: string, name: SceneName, count?: number): Promise<void> {
  await control('/thread/scene', { thread, name, ...(count === undefined ? {} : { count }) });
}

/** What the fake API says of how a thread behaves: that its agent cannot be steered, the limits in its way, how it sends by default, or that it is to forget everything. */
export async function configureThread(thread: string, settings: { cannot_steer?: boolean; limits?: unknown; default_delivery?: 'queue' | 'steer' | 'interrupt'; reset?: boolean }): Promise<void> {
  await control('/thread/set', { thread, ...settings });
}

/** Who is looking at a thread, as the presence frame says. */
export async function setPresence(thread: string, users: unknown[]): Promise<void> {
  await control('/thread/presence', { thread, users });
}

/** The system ends everybody's following of a thread: it was taken from them, or it was deleted. */
export async function closeThread(thread: string, reason: 'revoked' | 'deleted'): Promise<void> {
  await control('/thread/close', { thread, reason });
}

/** A request for approval that the fake API knows, which an approval item of a thread refers to. */
export async function addApproval(approval: unknown): Promise<void> {
  await control('/thread/approval', approval);
}

/** The whole output of a command, which the row holds only the tail of. */
export async function setOutput(item: string, text: string): Promise<void> {
  await control('/thread/output', { item, text });
}

/** What the fake API holds of a thread: its rows, the drafts kept, how far each person has read. */
export async function threadHeld(thread: string): Promise<{
  seq: number;
  items: { id: string }[];
  turns: Pick<Turn, 'id' | 'state' | 'usage'>[];
  inputs: { id: string; state: string; delivery: string; client_id: string | null; queue_held: boolean; content: { type: string; text?: string }[] }[];
  drafts: Record<string, { content: { type: string; text?: string }[] }>;
  read: Record<string, number>;
}> {
  return (await control('/thread/state', { thread })) as never;
}

/** Who the fake API was told is looking at each thread, by connection. */
export async function lookedAt(): Promise<Record<string, boolean[]>> {
  return (await control('/thread/looked-at')) as Record<string, boolean[]>;
}

/** Signs in with a name and a password, and waits for the workbench. */
export async function signIn(page: Page, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username or email address').fill(person.login);
  await page.getByLabel('Password', { exact: true }).fill(person.password);
  const accepted = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login'));
  await page.getByRole('button', { name: 'Sign in' }).click();
  expect((await accepted).status()).toBe(200);
  // The saved destination can be a document, thread, settings page or empty workbench.
  await expect(page.locator('[data-main-area]')).toBeVisible();
  // The frame is mounted while its landing route is still loading. Let that navigation finish
  // before a test chooses another address or starts counting its requests.
  await page.waitForLoadState('networkidle');
}

/**
 * What one audit of a page is allowed to take besides what the test itself needs. Axe walks every
 * element of the page, which for a page of a few thousand elements (the gallery of the controls, a list of
 * hundreds of rows) takes seconds on a quiet machine and a quarter of a minute on a busy one. That is the
 * audit's own cost, not the product's, so each audit adds this to the time of the test that asks for it.
 */
const AUDIT_ALLOWANCE_MS = 20_000;

/**
 * Asks the page, as it stands, whether anything on it is a serious barrier: nothing serious or
 * critical may be found, in the theme that is given. A list that has opened over the page and shut the
 * page off for a screen reader is asked on its own, with the selector `within`: the page behind it is
 * hidden by design, and what it holds that can take the keyboard is held away from it by the list.
 */
export async function expectAccessible(page: Page, theme: 'light' | 'dark' = 'light', within?: string): Promise<void> {
  test.info().setTimeout(test.info().timeout + AUDIT_ALLOWANCE_MS);
  await page.emulateMedia({ colorScheme: theme });
  // The theme follows the system on a page that was told nothing else; let it be drawn.
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  await expectUniqueGuideEntries(page, within);
  await expectOneScrollerPerArea(page, within);
  await expectOneCurrentSection(page, within);
  const audit = new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']);
  const results = await (within === undefined ? audit : audit.include(within)).analyze();
  const grave = results.violations.filter((violation) => violation.impact === 'serious' || violation.impact === 'critical');
  expect(
    grave.map((violation) => `${violation.id}: ${violation.nodes.map((node) => node.target.join(' ')).join(' | ')}`),
    `in the ${theme} theme`,
  ).toEqual([]);
}

/** The same in both themes. */
export async function expectAccessibleInBothThemes(page: Page, within?: string): Promise<void> {
  await expectAccessible(page, 'light', within);
  await expectAccessible(page, 'dark', within);
  await page.emulateMedia({ colorScheme: 'light' });
}

// ---------------------------------------------------------------- finding things

/** Fills the fake API with this many names (topics, threads, documents of Web), for a finder that has many to look through. */
export async function fillNames(count: number): Promise<void> {
  await control('/search/names', { count });
}

/** Holds every answer of the names and of the search back for this long, to look at what a slow answer shows. */
export async function holdSearch(ms: number): Promise<void> {
  await control('/search/latency', { ms });
}

/** Makes the lookup of names fail, or work again. */
export async function failNames(failing: boolean): Promise<void> {
  await control('/search/fail', { names: failing });
}

/** The values of the secrets that were written to the fake system, by place and name: no request of the API gives one back, and a test must see that it arrived. */
export async function secretsWritten(): Promise<Record<string, string>> {
  return (await control('/settings/secrets')) as Record<string, string>;
}

/** How long the colours of a page take to settle after the theme changes: they move over 200 ms, and a page asked in the middle of that reads as too pale. */
const THEME_SETTLES_MS = 350;

/** The same in both themes, with each given the time to settle first; for pages with long lists, where the change takes longest under load. */
export async function expectAccessibleSettled(page: Page, within?: string): Promise<void> {
  for (const theme of ['light', 'dark'] as const) {
    await page.emulateMedia({ colorScheme: theme });
    await page.waitForTimeout(THEME_SETTLES_MS);
    await expectAccessible(page, theme, within);
  }
  await page.emulateMedia({ colorScheme: 'light' });
}
