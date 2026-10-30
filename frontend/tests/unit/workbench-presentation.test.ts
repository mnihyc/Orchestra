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

const state = () => useWorkbench.getState();
beforeEach(() => { state().leave(); localStorage.clear(); state().enter('reader-a', 'thread'); });
afterEach(() => { state().leave(); localStorage.clear(); });

it('recovers scroll/folds after close and reload without sharing another instance or another person’s state', () => {
  const owner = state().openTab('page', { href: '/thread/thread-a?continuation=history-a' });
  const duplicate = state().openTab('page', { href: '/thread/thread-a' }, 'new');
  const read = contentThreadRead('thread-a', owner), other = contentThreadRead('thread-a', duplicate);
  const position = { key: 'item-older', item: 'item-older', offset: 19, atEnd: false };
  timelineUi.setStep(read, 'tool', true); timelineUi.toggleFold(read, 'turn');
  keepTimelinePosition(read, 'history-a', position);
  state().close({ kind: 'tab', tab: owner }); timelineUi.forget(read);
  state().leave(); state().enter('reader-a', 'page'); state().reopen();
  expect(state().activeContent).toBe(owner);
  expect(allTabs(state().layout).find((tab) => tab.id === owner)?.of?.href).toBe('/thread/thread-a?continuation=history-a');
  expect(readTimelinePosition(read, 'history-a')).toEqual(position);
  expect(readTimelinePosition(other, 'history-a')).toBeNull();
  expect(readTimelinePosition(read, null)).toBeNull();
  const ui = renderHook(() => useThreadUi('thread-a'), { wrapper: ({ children }: { children: ReactNode }) => createElement(ThreadReadProvider, { read, children }) });
  expect(ui.result.current.steps.get('tool')).toBe(true); expect(ui.result.current.folds.has('turn')).toBe(true); ui.unmount();
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
