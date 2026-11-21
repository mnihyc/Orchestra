/** Anonymous shell geometry draws immediately; private names wait for the confirmed reader.
 *
 * Kept in this browser under the confirmed person's own key:
 *   - who it was kept for (the person's id, which is no secret) and the name they go by, for the account control;
 *   - the sizes of the left and right bars and whether they were open;
 *   - the tabs of the main area: for each its title, the kind of thing it shows and whether it is in front;
 *   - the navigation tree as it last stood in the organization in view: the rows that were showing, each with its kind, name and depth.
 * (How the page looks, its theme, density and language, is kept apart in `orch.appearance`, and drawn first of all.)
 *
 * Never kept: what is in a thread, a document or a file; what anything says or holds; a token or any secret; an address of
 * the application or a person's list of anything else. The record is written a short while after each change of what it holds;
 * it is read after the session confirms its owner and corrected by every answer; it is dropped when the session answers that there
 * is none or another person's, when somebody signs out, and when it is older than `KEPT_SHELL_DAYS`.
 */
import { create } from 'zustand';

const key = (user: string) => `orch.${user}.shell`;
const FRAME_KEY = 'orch.shell-frame';
/** How long a record is believed. A default of the build. */
export const KEPT_SHELL_DAYS = 30;
/** The most tabs, tree rows and characters of a name that a record holds: it draws a first look, nothing that is worked in. */
export const KEPT_TABS = 12;
export const KEPT_ROWS = 60;
export const KEPT_NAME_CHARS = 80;

export interface KeptTab { id: string; title: string; kind: string; front: boolean }
export interface KeptRow { depth: number; kind: string; name: string }
export interface KeptBar { open: boolean; size: number }
export interface KeptShell {
  v: 1;
  user: string;
  name: string;
  kept: number;
  left: KeptBar;
  right: KeptBar;
  tabs: KeptTab[];
  tree: { org: string; rows: KeptRow[] } | null;
}

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === 'string';
const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);
const bar = (value: unknown): KeptBar | null => record(value) && typeof value.open === 'boolean' && finite(value.size) ? { open: value.open, size: value.size } : null;

/** Reads a record as it was written, or nothing: a record of another shape or one that is too old is dropped. */
export function readKeptShell(user: string, now = Date.now()): KeptShell | null {
  let raw: string | null;
  try { raw = localStorage.getItem(key(user)); } catch { return null; }
  if (raw === null) return null;
  try {
    const value: unknown = JSON.parse(raw);
    if (record(value) && value.v === 1 && value.user === user && text(value.name) && finite(value.kept) && now - value.kept <= KEPT_SHELL_DAYS * 86_400_000
      && Array.isArray(value.tabs)) {
      const left = bar(value.left), right = bar(value.right);
      const tabs = value.tabs.flatMap((tab: unknown): KeptTab[] => record(tab) && text(tab.id) && text(tab.title) && text(tab.kind) && typeof tab.front === 'boolean'
        ? [{ id: tab.id, title: tab.title, kind: tab.kind, front: tab.front }] : []);
      let tree: KeptShell['tree'] = null;
      if (record(value.tree) && text(value.tree.org) && Array.isArray(value.tree.rows)) {
        tree = { org: value.tree.org, rows: value.tree.rows.flatMap((row: unknown): KeptRow[] => record(row) && finite(row.depth) && text(row.kind) && text(row.name)
          ? [{ depth: row.depth, kind: row.kind, name: row.name }] : []) };
      }
      if (left !== null && right !== null) return { v: 1, user: value.user, name: value.name, kept: value.kept, left, right, tabs, tree };
    }
  } catch {
    // Not a record at all.
  }
  clearKeptShell(user);
  return null;
}

export function writeKeptShell(shell: KeptShell): void {
  try {
    localStorage.setItem(key(shell.user), JSON.stringify(shell));
    localStorage.setItem(FRAME_KEY, JSON.stringify({ left: shell.left, right: shell.right }));
  } catch { /* A browser that keeps nothing draws the shell from nothing. */ }
}

/** Anonymous geometry carries no reader, resource, title or tree name. */
export function readShellGeometry(): { left: KeptBar; right: KeptBar } | null {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(FRAME_KEY) ?? 'null');
    if (!record(value)) return null;
    const left = bar(value.left), right = bar(value.right);
    return left === null || right === null ? null : { left, right };
  } catch { return null; }
}

export function clearKeptShell(user?: string): void {
  try {
    if (user !== undefined) { localStorage.removeItem(key(user)); return; }
    const keys = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index));
    for (const entry of keys) if (entry?.startsWith('orch.') === true && entry.endsWith('.shell')) localStorage.removeItem(entry);
    localStorage.removeItem(FRAME_KEY);
  } catch { /* Nothing was kept, then. */ }
}

/** Whether the record is drawn over the page still: until the frame stands admitted, or something shows the record is not to be believed. */
export const useCover = create<{ covering: boolean }>(() => ({ covering: true }));
export function uncover(): void {
  useCover.setState({ covering: false });
}

/** Drops the record the moment the session says that nobody is signed in: what a person may no longer see goes with their session. */
export function forgetWhenSignedOut(session: { subscribe: (listener: (state: { status: string }) => void) => () => void; getState: () => { status: string } }): () => void {
  const forget = (state: { status: string }) => {
    if (state.status !== 'signed_out') return;
    clearKeptShell();
    uncover();
  };
  forget(session.getState());
  return session.subscribe(forget);
}
