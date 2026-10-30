/** Ordinary file/diff openings retain previous views and use the last main group behind an inspector. */
import { afterEach, beforeEach, expect, it } from 'vitest';
import { openFile, openDifference, useJumps, usePreviews } from '@/features/files/FileTabs';
import { allTabs, findTab, makeGroup, makeTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';

const state = () => useWorkbench.getState();
beforeEach(() => { state().leave(); localStorage.clear(); state().enter('file-workspace-reader', 'thread'); usePreviews.setState({ ids: new Set() }); useJumps.setState({ jumps: {} }); });
afterEach(() => { state().leave(); localStorage.clear(); });

it('keeps both ordinary file views and selects an existing file for a requested line', () => {
  openFile({ environmentId: 'environment', threadId: 'thread', path: 'first.txt' });
  const first = allTabs(state().layout).find((tab) => tab.view === 'file');
  expect(first).toBeDefined();
  openFile({ environmentId: 'environment', threadId: 'thread', path: 'second.txt' });
  const files = allTabs(state().layout).filter((tab) => tab.view === 'file');
  expect(files.map((tab) => tab.of?.path)).toEqual(['first.txt', 'second.txt']);
  openFile({ environmentId: 'environment', threadId: 'thread', path: 'first.txt', line: 41 });
  expect(allTabs(state().layout).filter((tab) => tab.view === 'file')).toHaveLength(2);
  expect(state().focusedTab).toBe(first?.id);
  expect(useJumps.getState().jumps[first!.id]?.line).toBe(41);
  expect(useJumps.getState().jumps[files[1]!.id]).toBeUndefined();
});

it('keeps same-named direct-environment files and their line intent separate', () => {
  openFile({ environmentId: 'a', threadId: '', path: 'main.rs', line: 7 });
  const a = state().focusedTab!;
  openFile({ environmentId: 'b', threadId: '', path: 'main.rs', line: 23 });
  const b = state().focusedTab!;
  expect(a).not.toBe(b);
  expect(useJumps.getState().jumps[a]?.line).toBe(7);
  expect(useJumps.getState().jumps[b]?.line).toBe(23);
});

it('opens into the remembered main group from a bottom inspector and keeps staged diff targets distinct', () => {
  const a = state().openTab('page', { href: '/thread/a' });
  const b = state().openTab('page', { href: '/thread/b' }, 'beside');
  const group = findTab(state().layout, b)!.group.id;
  const terminal = makeTab('terminal');
  state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, group: makeGroup([terminal]) } }));
  state().focus(terminal.id);
  openFile({ environmentId: 'a', threadId: 'a', path: 'notes.md' });
  expect(findTab(state().layout, state().focusedTab!)?.group.id).toBe(group);
  expect(findTab(state().layout, a)).not.toBeNull();
  openDifference({ kind: 'git', environmentId: 'a', threadId: 'a', path: 'notes.md', staged: true });
  openDifference({ kind: 'git', environmentId: 'a', threadId: 'a', path: 'notes.md', staged: false });
  const diffs = allTabs(state().layout).filter((tab) => tab.view === 'difference');
  expect(diffs.map((tab) => tab.of?.staged)).toEqual(['1', '0']);
  expect(diffs.every((tab) => findTab(state().layout, tab.id)?.group.id === group)).toBe(true);
});
