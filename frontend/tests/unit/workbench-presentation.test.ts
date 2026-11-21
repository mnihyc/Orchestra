/** Closing releases live readers; presentation snapshots remain scoped to their tab and signed-in workspace. */
import { beforeEach, afterEach, expect, it } from 'vitest';
import { renderHook } from '@testing-library/react';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { contentThreadRead } from '@/realtime/threadScope';
import { createElement, type ReactNode } from 'react';
import { timelineUi, useThreadUi } from '@/features/timeline/state';
import { keepTimelinePosition, readTimelinePosition } from '@/features/timeline/position';
import { useWorkbench } from '@/features/workbench/store';
import { capturePresentations, keepPresentation, readPresentation } from '@/features/workbench/presentation';
import { parsePageAddress, openPage } from '@/features/workbench/PageView';
import { allTabs } from '@/features/workbench/layout';
import { loadWorkspace, saveWorkspace } from '@/features/workbench/persistence';
import { DEFAULT_INTERFACE_READING } from '@/lib/interfaceReading';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { openThreadChanges } from '@/features/inspector/reviewTarget';

const state = () => useWorkbench.getState();
beforeEach(() => { state().leave(); localStorage.clear(); state().enter('reader-a', 'thread'); });
afterEach(() => { state().leave(); localStorage.clear(); });

it('recovers scroll and what was opened after close and reload without sharing another instance or another person’s state', () => {
  const owner = state().openTab('page', { href: '/thread/thread-a?continuation=history-a' });
  const duplicate = state().openTab('page', { href: '/thread/thread-a' }, 'new');
  const read = contentThreadRead('thread-a', owner), other = contentThreadRead('thread-a', duplicate);
  const position = { key: 'item-older', item: 'item-older', offset: 19, atEnd: false };
  timelineUi.setStep(read, 'tool', true); timelineUi.setStep(read, 'work:first-step', false);
  keepTimelinePosition(read, 'history-a', position);
  state().close({ kind: 'tab', tab: owner }); timelineUi.forget(read);
  state().leave(); state().enter('reader-a', 'page'); state().reopen();
  expect(state().activeContent).toBe(owner);
  expect(allTabs(state().layout).find((tab) => tab.id === owner)?.of?.href).toBe('/thread/thread-a?continuation=history-a');
  expect(readTimelinePosition(read, 'history-a')).toEqual(position);
  expect(readTimelinePosition(other, 'history-a')).toBeNull();
  expect(readTimelinePosition(read, null)).toBeNull();
  const ui = renderHook(() => useThreadUi('thread-a'), { wrapper: ({ children }: { children: ReactNode }) => createElement(ThreadReadProvider, { read, children }) });
  expect(ui.result.current.steps.get('tool')).toBe(true); expect(ui.result.current.steps.get('work:first-step')).toBe(false); ui.unmount();
  state().leave(); state().enter('reader-b', 'thread');
  expect(readTimelinePosition(read, 'history-a')).toBeNull();
  expect(capturePresentations(new Set([owner]))).toEqual({});
});

it('does not retain presentation records outside open or reopenable tabs', () => {
  const owner = state().openTab('page', { href: '/thread/a' });
  keepPresentation(owner, 'scroll', { top: 100 }); keepPresentation('discarded-owner', 'scroll', { top: 200 }); state().flush();
  state().leave(); state().enter('reader-a', 'thread');
  expect(readPresentation(owner, 'scroll')).toEqual({ top: 100 });
  expect(readPresentation('discarded-owner', 'scroll')).toBeUndefined();
});

it('bounds long close history and its presentation by the reading default while keeping open drafts', () => {
  const limit = DEFAULT_INTERFACE_READING.kept_closed_tabs;
  expect(SETTINGS_REGISTRY.find((entry) => entry.key === 'interface.kept_closed_tabs')?.default).toBe(limit);
  const draft = state().openTab('page', { href: '/thread/dirty-open' }); state().setUnsaved(draft, true);
  keepPresentation(draft, 'scroll', { top: 900 });
  const closed: string[] = [];
  for (let at = 0; at < limit * 3 + 1; at += 1) {
    const owner = state().openTab('page', { href: `/thread/closed-${at}` });
    keepPresentation(owner, 'scroll', { top: at });
    state().close({ kind: 'tab', tab: owner }); closed.push(owner);
  }
  state().flush();
  const expected = closed.slice(-limit), saved = loadWorkspace('reader-a').kept;
  expect(state().closedTabs.map((entry) => entry.tab.id)).toEqual(expected);
  expect(saved?.closedTabs.map((entry) => entry.tab.id)).toEqual(expected);
  expect(new Set(Object.keys(saved?.presentations ?? {}))).toEqual(new Set([draft, ...expected]));
  expect(readPresentation(closed[0]!, 'scroll')).toBeUndefined();
  expect(state().unsaved.has(draft)).toBe(true); expect(allTabs(state().layout).some((tab) => tab.id === draft)).toBe(true);
  state().leave(); state().enter('reader-a', 'thread');
  expect(state().closedTabs.map((entry) => entry.tab.id)).toEqual(expected);
  state().reopen(); expect(state().activeContent).toBe(expected.at(-1));
});

it.each([0, 2, 25])('uses the last confirmed closed limit %s before the shell returns while Reopen remains available', (limit) => {
  state().flush(); const seed = loadWorkspace('reader-a').kept;
  if (seed === null) throw new Error('The native workspace fixture must be saved');
  state().leave(); saveWorkspace('reader-a', Object.assign(seed, { closedLimit: limit }));
  state().enter('reader-a', 'thread');
  const closed: string[] = [];
  for (let at = 0; at < 30; at += 1) {
    const owner = state().openTab('page', { href: `/thread/bounded-${at}` });
    keepPresentation(owner, 'scroll', { top: at });
    state().close({ kind: 'tab', tab: owner }); closed.push(owner);
  }
  expect(state().closedTabs.map((entry) => entry.tab.id)).toEqual(closed.slice(-Math.max(1, limit)));
  expect(Object.keys(loadWorkspace('reader-a').kept?.presentations ?? {})).toHaveLength(limit);
  state().reopen(); expect(state().activeContent).toBe(closed.at(-1));
});

it('retains authorized tab names when focus mode reloads without the navigation tree mounted', () => {
  const a = state().openTab('page', { href: '/thread/a' }), b = state().openTab('page', { href: '/thread/b' });
  state().setNamed(a, { title: 'Build · Release', path: 'Team / Release / Build', source: 'view' });
  state().setNamed(b, { title: 'Review · Release', path: 'Team / Release / Review', source: 'cache' });
  state().setFocusMode(true); state().leave(); state().enter('reader-a', 'thread');
  expect(state().named.get(a)).toEqual({ title: 'Build · Release', path: 'Team / Release / Build', source: 'cache' });
  expect(state().named.get(b)?.title).toBe('Review · Release');
  state().leave(); state().enter('reader-b', 'thread'); expect(state().named.size).toBe(0);
});

it.each([['/settings', '/settings/profile'], ['/settings/usage', '/usage'], ['/admin', '/admin/overview']])('resolves %s in the same normal/new/beside lifecycle', (alias, actual) => {
  expect(parsePageAddress(alias)?.pathname).toBe(actual);
  const first = openPage(alias); expect(first).not.toBeNull();
  expect(openPage(actual)).toBe(first);
  const duplicate = openPage(alias, 'beside'); expect(duplicate).not.toBe(first);
  expect(allTabs(state().layout).filter((tab) => tab.of?.href === actual)).toHaveLength(2);
});

it('recognizes the root entrance as its existing full page adapter', () => {
  expect(parsePageAddress('/')?.route).toBe('/');
});

it('opens the header’s selected turn in turn scope and keeps the whole-thread act available', () => {
  const panel = state().openTab('changes', { thread: 'thread-a', scope: 'thread' });
  openThreadChanges('thread-a', 'turn-two');
  expect(allTabs(state().layout).find((tab) => tab.id === panel)?.of).toEqual({ thread: 'thread-a', scope: 'turn', turn: 'turn-two' });
  openThreadChanges('thread-a');
  expect(allTabs(state().layout).find((tab) => tab.id === panel)?.of).toEqual({ thread: 'thread-a', scope: 'thread' });
});
