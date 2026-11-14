/** What the browser keeps of the shell: what it holds, what it never holds, and how it is dropped. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { createStore } from 'zustand/vanilla';
import { act, cleanup, render } from '@testing-library/react';
import { createElement } from 'react';
import { client } from '@/api/client';
import { KeptShellCover } from '@/shell/KeptShellCover';

import { openContent } from '@/features/workbench/contentNavigation';
import { findTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import type { OrgTree } from '@/realtime/stores/tree';
import { buildKeptShell, kindOfTab, treeRows } from '@/shell/keepShell';
import { clearKeptShell, forgetWhenSignedOut, KEPT_SHELL_DAYS, readKeptShell, useCover, writeKeptShell } from '@/shell/keptShell';
import { treeNode } from '../doubles/tree-node';

const READER = '01950000-0000-7000-8000-000000000001';
const ORG = '01950000-0000-7000-8000-0000000000a0';
const PROJECT = '01950000-0000-7000-8000-0000000000a1';
const TOPIC = '01950000-0000-7000-8000-0000000000a2';
const THREAD = '01950000-0000-7000-8000-0000000000bb';

const tree = (): OrgTree => ({
  nodes: new Map([
    treeNode({ id: ORG, kind: 'org', parent_id: null, name: 'Acme' }),
    treeNode({ id: PROJECT, kind: 'project', parent_id: ORG, name: 'Website' }),
    treeNode({ id: TOPIC, kind: 'topic', parent_id: PROJECT, name: 'Launch' }),
    treeNode({ id: 'archived', kind: 'project', parent_id: ORG, name: 'Old', archived: true }),
    treeNode({ id: 'gone', kind: 'project', parent_id: ORG, name: 'Removed', deleted: true }),
  ].map((node) => [node.id, node])),
  lastSeq: 1, loaded: true, error: null,
});

beforeEach(() => { useWorkbench.getState().leave(); useWorkbench.getState().enter(READER, 'thread'); useCover.setState({ covering: true }); });
afterEach(() => { clearKeptShell(); useWorkbench.getState().leave(); });

it('paints no kept private name until the restored session confirms that record’s reader', () => {
  const session = client.session.getState();
  try {
    client.session.setState({ status: 'unknown', userId: null });
    const layout = useWorkbench.getState().layout;
    const kept = buildKeptShell({ user: READER, name: 'Private reader', layout: { ...layout, left: { ...layout.left, open: true } },
      focusedMainGroup: null, titles: new Map(), org: ORG, tree: tree(), open: new Set([PROJECT]), now: Date.now() });
    writeKeptShell(kept);
    const view = render(createElement(KeptShellCover));
    expect(view.container.textContent).not.toContain('Website');
    expect(view.container.textContent).not.toContain('Launch');
    act(() => { client.session.setState({ status: 'signed_in', userId: 'another-reader' }); });
    expect(view.container.textContent).not.toContain('Website');
    act(() => { client.session.setState({ status: 'signed_in', userId: READER }); });
    expect(view.container.textContent).toContain('Website');
  } finally { cleanup(); client.session.setState(session); }
});

it('holds the tabs, the bars and the rows of the tree as they stood, and nothing of what any of them holds', () => {
  const id = openContent(`/thread/${THREAD}`);
  if (id === null) throw new Error('The address opens no tab');
  const state = useWorkbench.getState();
  const kept = buildKeptShell({ user: READER, name: 'Ada Lovelace', layout: state.layout, focusedMainGroup: state.focusedMainGroup,
    titles: new Map([[id, { title: 'Fix the build' }]]), org: ORG, tree: tree(), open: new Set([PROJECT]), now: 1_000 });
  expect(kept.tabs.find((tab) => tab.id === id)).toEqual({ id, title: 'Fix the build', kind: 'thread', front: true });
  expect(kept.tree).toEqual({ org: ORG, rows: [{ depth: 0, kind: 'project', name: 'Website' }, { depth: 1, kind: 'topic', name: 'Launch' }] });
  expect(kept.left.size).toBeGreaterThan(0);
  // The record says who it is for and nothing else of anybody: its members are these, and no more.
  expect(Object.keys(kept).sort()).toEqual(['kept', 'left', 'name', 'right', 'tabs', 'tree', 'user', 'v']);
  expect(Object.keys(kept.tabs[0] ?? {}).sort()).toEqual(['front', 'id', 'kind', 'title']);
  // The thread's address is not in it: where a tab leads is not kept, only what it is called and what it shows.
  expect(JSON.stringify(kept)).not.toContain(THREAD);
});

it('names the kind of a tab by what it shows', () => {
  const id = openContent(`/thread/${THREAD}`);
  const tab = id === null ? null : findTab(useWorkbench.getState().layout, id)?.tab;
  expect(tab === null || tab === undefined ? null : kindOfTab(tab)).toBe('thread');
});

it('leaves a place closed unless it was open, and keeps a few levels of the rows that were', () => {
  expect(treeRows(tree(), ORG, new Set())).toEqual([{ depth: 0, kind: 'project', name: 'Website' }]);
});

it('is read as it was written, and is dropped when it is not a record of this shape or is too old', () => {
  const kept = buildKeptShell({ user: READER, name: 'Ada', layout: useWorkbench.getState().layout, focusedMainGroup: null, titles: new Map(), org: null, tree: undefined, open: null, now: 1_000 });
  writeKeptShell(kept);
  expect(readKeptShell(READER, 2_000)).toEqual(kept);
  expect(readKeptShell(READER, 1_000 + (KEPT_SHELL_DAYS + 1) * 86_400_000)).toBeNull();
  expect(localStorage.getItem(`orch.${READER}.shell`)).toBeNull();
  localStorage.setItem(`orch.${READER}.shell`, '{"v":2}');
  expect(readKeptShell(READER)).toBeNull();
  expect(localStorage.getItem(`orch.${READER}.shell`)).toBeNull();
});

it('is dropped, and no longer drawn, the moment the session says that nobody is signed in', () => {
  const session = createStore<{ status: string }>(() => ({ status: 'unknown' }));
  writeKeptShell(buildKeptShell({ user: READER, name: 'Ada', layout: useWorkbench.getState().layout, focusedMainGroup: null, titles: new Map(), org: null, tree: undefined, open: null, now: Date.now() }));
  const stop = forgetWhenSignedOut(session);
  expect(readKeptShell(READER)).not.toBeNull();
  expect(useCover.getState().covering).toBe(true);
  session.setState({ status: 'signed_out' });
  expect(localStorage.getItem(`orch.${READER}.shell`)).toBeNull();
  expect(useCover.getState().covering).toBe(false);
  stop();
});
