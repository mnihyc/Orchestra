/** What the specs of documents share: the ids of the fake API's documents, the requests under `/__double/documents` that play other people, and reading what the fake server holds. */
import { expect, type Locator, type Page } from '@playwright/test';

import { id } from '../doubles/fixtures.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

/** The documents the team starts with: a plan with a block of every kind, notes by a developer, and one that is private. */
export const DOCS = { plan: id(7, 1), notes: id(7, 2), private: id(7, 3) } as const;

/** A block of the plan, by the number it was made with. */
export const block = (n: number): string => id(8, n);

async function post(path: string, body: unknown): Promise<unknown> {
  const response = await fetch(`${DOUBLE}/__double/documents${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return response.status === 204 ? null : response.json();
}

export interface Held {
  id: string;
  parent_id: string | null;
  position: string;
  type: string;
  props: Record<string, unknown>;
  content: { type: string; text?: string }[];
  seq_updated: number;
}

/** What the fake server holds of a document: its live blocks in the order of the page, its title and how many versions it kept. */
export async function held(doc: string): Promise<{ blocks: Held[]; seq: number; title: string; versions: number }> {
  const reply = (await post('/blocks', { doc })) as { blocks: Held[]; seq: number; title: string; versions: number };
  const children = (parent: string | null): Held[] =>
    reply.blocks.filter((one) => one.parent_id === parent).sort((one, other) => Number(one.position) - Number(other.position));
  const ordered: Held[] = [];
  const walk = (parent: string | null) => {
    for (const one of children(parent)) {
      ordered.push(one);
      walk(one.id);
    }
  };
  walk(null);
  return { ...reply, blocks: ordered };
}

/** The words of a block, as a person reads them. */
export const wordsOf = (one: Held): string => one.content.map((run) => run.text ?? '').join('');

/** Sends a batch as another person would. */
export async function actAs(user: string, doc: string, ops: unknown[]): Promise<void> {
  await post('/as', { user, doc, ops });
}

/** Sends a batch as an agent that was let write, named by its thread. */
export async function agentWrites(doc: string, agent: string, thread: string, ops: unknown[]): Promise<void> {
  await post('/agent', { doc, agent, thread, ops });
}

/** Cuts a version of a document now, as the server does after ten minutes of writing or two of quiet. */
export async function cutVersion(doc: string): Promise<void> {
  await post('/cut', { doc });
}

/** Makes a document of many paragraphs beside a node. */
export async function makeLong(count: number, title = 'A long page'): Promise<string> {
  return ((await post('/make', { count, title })) as { id: string }).id;
}

/** Makes a private document of a person beside a node: it follows no page, and only its owner and a system administrator may read it. */
export async function makePrivateDocument(user: string, node: string, title: string): Promise<string> {
  return ((await post('/make', { user, node, title, count: 1, private: true })) as { id: string }).id;
}

/** Makes a lock above the document fix its Links tabs at the place's setting, or takes the lock away. */
export async function lockLinksTabs(doc: string, locked = true): Promise<void> {
  await post('/links-lock', { doc, locked });
}

/** Asks the fake API to withhold the next frame of the feed, to see a gap repaired. */
export async function loseNextFrame(): Promise<void> {
  await post('/lose', {});
}

/** The page of blocks. */
export const editorOf = (page: Page): Locator => page.getByRole('textbox', { name: 'Document text' });

/** The line of a block, by the words in it. */
export const lineOf = (page: Page, words: string | RegExp): Locator => editorOf(page).locator('[data-ot]').filter({ hasText: words }).first();

/** Opens a document by its address and waits until its page is there to write in. */
export async function openDocument(page: Page, doc: string): Promise<void> {
  await page.goto(`/doc/${doc}`);
  await expect(editorOf(page)).toBeVisible();
}

/** What the page says of what became of what was typed. */
export const saveState = (page: Page): Locator => page.locator('[data-save]');

/** Waits until everything typed has been answered. */
export async function waitUntilSaved(page: Page): Promise<void> {
  await expect(saveState(page)).toHaveAttribute('data-save', 'saved');
}

/** Puts the caret at the end of the line that has these words, by pressing in it, and waits until it is there: a page still settling can move it after the press. */
export async function caretAtEndOf(page: Page, words: string | RegExp): Promise<void> {
  const line = lineOf(page, words);
  await line.click();
  await page.keyboard.press('End');
  await expect
    .poll(() =>
      line.evaluate((element) => {
        const selection = element.ownerDocument.getSelection();
        if (selection === null || selection.rangeCount === 0 || !element.contains(selection.anchorNode)) return false;
        const range = selection.getRangeAt(0);
        const after = element.ownerDocument.createRange();
        after.selectNodeContents(element);
        after.setStart(range.endContainer, range.endOffset);
        return after.toString() === '';
      }),
    )
    .toBe(true);
}

/** Opens the block menu where the caret is and chooses an entry by what is typed to find it. */
export async function slash(page: Page, query: string, choose?: string | RegExp): Promise<void> {
  await page.keyboard.type(`/${query}`);
  const list = page.getByRole('listbox', { name: 'Blocks' });
  await expect(list).toBeVisible();
  if (choose !== undefined) await list.getByRole('option', { name: choose }).click();
  else await page.keyboard.press('Enter');
}
