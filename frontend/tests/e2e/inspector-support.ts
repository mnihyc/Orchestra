/** What the end-to-end tests of the views beside a thread share: steering the fake workspace, and getting to a thread with its files in view. */
import { expect, type Page } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { ACME, emit, PEOPLE, scenario, signIn } from './support.ts';
import { showWorkbenchView } from './workbench-support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** The environment the main thread of the team works in. */
export const MAIN_ENVIRONMENT = RUNS_ON.workspace;
export const MAIN = ACME.mainThread;
/** The native root of the Workspace environment seeded by the resource double. */
const MAIN_WORKSPACE_ROOT = '/home/ada/work/main';

/** Steers the fake workspace, shells and processes (`/__double/workspace/…`). */
export async function workspace(path: string, body: unknown = {}): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/workspace${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

/** What the fake workspace holds of a file, as text; `null` where there is none. */
export async function onDisk(path: string): Promise<string | null> {
  return ((await workspace('/content', { path })) as { text: string | null }).text;
}

/** Puts the fake world into its beginning, with the thread working in `web/`. */
export async function begin(directory = 'web'): Promise<void> {
  await scenario();
  await workspace('/reset');
  await emit(MAIN, { thread: { state: 'idle', directory, cwd: directory === '' ? MAIN_WORKSPACE_ROOT : `${MAIN_WORKSPACE_ROOT}/${directory}` } });
}

/** Signs in and opens the thread; the Files view is in the right bar from the start. */
export async function openThread(page: Page, person: { login: string; password: string } = PEOPLE.ada, path = `/thread/${MAIN}`): Promise<void> {
  await signIn(page, person);
  await page.goto(path);
  await expect(page.getByRole('heading', { name: 'Main thread' })).toBeVisible();
}

/** Brings a view of the right bar to the front by its name. */
export async function showView(page: Page, name: string | RegExp): Promise<void> {
  await showWorkbenchView(page, name);
}

export const filesPanel = (page: Page) => page.getByTestId('files-panel');
export const fileTree = (page: Page) => page.getByRole('tree', { name: /^Files of/ });
export const row = (page: Page, name: string | RegExp) => fileTree(page).getByRole('treeitem', { name });

/** The editor of the file in front: its text, read from the page. */
export const editor = (page: Page) => page.locator('.cm-content');
