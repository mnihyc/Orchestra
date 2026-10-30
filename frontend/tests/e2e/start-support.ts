/** What the end-to-end tests of starting a thread share: steering the fake API's defaults, profiles and environments, reading what a page asked of the API, and the few places on the screen that every test looks at. */
import { expect, type Page } from '@playwright/test';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** A request only the fake API has, which steers what the screens that start a thread meet. */
async function steer(part: 'start' | 'resources', path: string, body: unknown = {}): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/${part}${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status} ${await response.text()}`);
}

/** Writes what the settings of a place say (a key with `null` is left unset), as the place's own and so for everything below it. */
export async function setDefaults(node: string, values: Record<string, unknown>, locked = false): Promise<void> {
  await steer('start', '/defaults', { node, values, locked });
}

/** Whether the fake API answers the request for what a thread could be bound to, or answers as a server that does not have it. */
export async function serveOptions(served: boolean): Promise<void> {
  await steer('start', '/serve', { options: served });
}

/** The harness of an agent profile says what it is, or is made to have said nothing: what a machine does once it has started the harness. */
export async function agentReports(agent: string, reported: boolean): Promise<void> {
  await steer('resources', '/report', { agent, reported });
}

/** Somebody who may make no environment, whatever their role says, or who may again. */
export async function mayNotMake(username: string, cannot = true): Promise<void> {
  await steer('resources', '/cannot-make', { username, cannot });
}

/** An environment that offers no executor to a harness that runs apart from its tools. */
export async function bareEnvironment(environment: string, bare = true): Promise<void> {
  await steer('resources', '/bare', { environment, bare });
}

/** A request that changed something: what it asked, where (below the API's own path), and with what body. */
export interface Asked {
  method: string;
  path: string;
  body: unknown;
}

/**
 * What the fake API was asked to change so far, in order, with the body of each; a request that only reads is not kept. The page is not
 * asked what it sent: it sends through a stream, which a browser does not show to a test.
 */
export async function requests(): Promise<Asked[]> {
  const response = await fetch(`${DOUBLE}/__double/start/sent`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' });
  if (!response.ok) throw new Error(`the fake API refused to say what it was asked: ${response.status}`);
  return (await response.json()) as Asked[];
}

/** Those of them that were of one method and whose path matches. */
export async function requestsOf(method: string, path: RegExp): Promise<Asked[]> {
  return (await requests()).filter((entry) => entry.method === method && path.test(entry.path));
}

/** The inline starter of a topic or a project. */
export const starter = (page: Page, kind: 'topic' | 'project' = 'topic') => page.getByRole('region', { name: kind === 'topic' ? 'Start a thread' : 'Start a topic' });

/** What is typed into it. */
export const starterBox = (page: Page, kind: 'topic' | 'project' = 'topic') => starter(page, kind).getByRole('textbox', { name: kind === 'topic' ? 'Start a thread' : 'Start a topic' });

/** The dialog that makes a thread in a topic. */
export const startDialog = (page: Page, place = 'Fix login') => page.getByRole('dialog', { name: `New thread in ${place}` });

/** Waits until the thread that was started is open, and gives its identifier. */
export async function openedThread(page: Page): Promise<string> {
  await expect(page).toHaveURL(/\/thread\//);
  return /\/thread\/([^/?]+)/.exec(page.url())?.[1] ?? '';
}
