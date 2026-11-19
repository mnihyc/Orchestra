/** A tab opened for an address starts the read of its page at that moment, and the screen that comes later takes it over. */
import { afterEach, beforeEach, expect, it } from 'vitest';

import { openContent } from '@/features/workbench/contentNavigation';
import { forgetPageBody, installPageReadStarters, pageReadShown, pageTookReads, startPageReads } from '@/features/workbench/pageReads';
import { ReadScheduler } from '@/api/readScheduler';
import { useWorkbench } from '@/features/workbench/store';
import { findTab, makeTab } from '@/features/workbench/layout';

const READER = '01950000-0000-7000-8000-000000000001';
const THREAD = '01950000-0000-7000-8000-0000000000bb';

beforeEach(() => { useWorkbench.getState().leave(); useWorkbench.getState().enter(READER, 'thread'); });
afterEach(() => { installPageReadStarters({}); useWorkbench.getState().leave(); });

it('starts thread and addressed parts together and transfers both visible leases to the mounted page', () => {
  const started: string[] = [], shown: boolean[] = [], stopped: string[] = [];
  const lease = (name: string) => Object.assign(() => { stopped.push(name); }, { shown: (next: boolean) => { shown.push(next); } });
  installPageReadStarters({
    thread: (id) => { started.push(`thread:${id}`); return lease('thread'); },
    page: (target) => { started.push(`${target.pathname}:${String(target.search.chat)}`); return lease('parts'); },
  });
  const view = openContent(`/thread/${THREAD}?chat=selected-chat`);
  if (view === null) throw new Error('The thread address must open');
  expect(started).toEqual([`thread:${THREAD}`, `/thread/${THREAD}:selected-chat`]);
  pageReadShown(view, false);
  expect(shown).toEqual([false, false]);
  pageTookReads(findTab(useWorkbench.getState().layout, view)!.tab);
  expect(stopped).toEqual(['thread', 'parts']);
});

it('starts the read of a thread when its tab is opened, once for each tab, and lets go of it when the screen has taken it over', () => {
  const started: [string, string][] = [];
  const released: string[] = [];
  installPageReadStarters({ thread: (thread, tab) => { started.push([thread, tab]); return () => { released.push(tab); }; } });
  const first = openContent(`/thread/${THREAD}`);
  if (first === null) throw new Error('The address opens no tab');
  expect(started).toEqual([[THREAD, first]]);
  // The same address again brings the open tab forward: nothing starts twice.
  expect(openContent(`/thread/${THREAD}`)).toBe(first);
  expect(started).toHaveLength(1);
  // A tab opened beside reads for itself.
  const beside = openContent(`/thread/${THREAD}`, 'new');
  if (beside === null) throw new Error('The address opens no second tab');
  expect(started).toEqual([[THREAD, first], [THREAD, beside]]);
  // The screen of the first tab follows what was started; the tab's own hold ends once.
  pageTookReads(findTab(useWorkbench.getState().layout, first)!.tab);
  pageTookReads(findTab(useWorkbench.getState().layout, first)!.tab);
  expect(released).toEqual([first]);
  // A page that reads no thread starts nothing.
  openContent('/inbox');
  expect(started).toHaveLength(2);
});

it('starts an addressed topic and its independent parts before any screen body mounts', () => {
  const started: string[] = [], released: string[] = [];
  installPageReadStarters({ page: (target, tab) => { started.push(`${target.route}:${target.params.topicId ?? ''}`); return () => { released.push(tab); }; } });
  const tab = openContent(`/topic/${THREAD}`);
  if (tab === null) throw new Error('The topic address must open a tab.');
  expect(started).toEqual([`/topic/$topicId/:${THREAD}`]);
  expect(openContent(`/topic/${THREAD}`)).toBe(tab);
  expect(started).toHaveLength(1);
  pageTookReads(findTab(useWorkbench.getState().layout, tab)!.tab);
  expect(released).toEqual([tab]);
});

it('lets Back paint a mounted retained page before its ordinary readers and warms a disposed body again', () => {
  const read: string[] = [], stopped: string[] = [];
  installPageReadStarters({ page: (target, tab) => { read.push(target.pathname); return () => { stopped.push(tab); }; } });
  const first = makeTab('page', { href: '/resources/providers/retained-provider' });
  startPageReads(first); pageTookReads(first);
  const other = makeTab('page', { href: '/resources/agents' });
  startPageReads(other); pageTookReads(other);
  startPageReads(first);
  expect(read).toEqual(['/resources/providers/retained-provider', '/resources/agents']);
  expect(stopped).toEqual([first.id, other.id]);
  forgetPageBody(first.id); startPageReads(first);
  expect(read).toEqual(['/resources/providers/retained-provider', '/resources/agents', '/resources/providers/retained-provider']);
});

it('keeps the new supplier when a link replaces the address of the same view', () => {
  const scheduler = new ReadScheduler();
  installPageReadStarters({ page: (target, tab) => {
    const owner = ['page-read', tab];
    scheduler.supply(owner, [['address', target.pathname]]);
    return () => scheduler.release(owner);
  } });
  const view = { id: 'replaced-view', view: 'page', of: { href: `/topic/${THREAD}` } };
  startPageReads(view);
  const next = `/project/${THREAD}`;
  startPageReads({ ...view, of: { href: next } });
  pageTookReads(view);
  expect(scheduler.owns(['address', next])).toBe(true);
  expect(scheduler.owns(['address', `/topic/${THREAD}`])).toBe(false);
});


it('keeps admitted dependent reads owned and visibility-aware after the page body takes over', () => {
  const stopped: string[] = [], handed: string[] = [], shown: boolean[] = [];
  const thread = Object.assign(() => { stopped.push('thread'); }, { shown: (next: boolean) => { shown.push(next); } });
  const parts = Object.assign(() => { stopped.push('parts'); }, { took: () => { handed.push('parts'); return true; }, shown: (next: boolean) => { shown.push(next); } });
  installPageReadStarters({ thread: () => thread, page: () => parts });
  const tab = makeTab('page', { href: `/thread/${THREAD}` });
  startPageReads(tab); pageTookReads(tab); pageTookReads(tab);
  expect(handed).toEqual(['parts']);
  expect(stopped).toEqual([]);
  pageReadShown(tab.id, false); pageReadShown(tab.id, true);
  expect(shown).toEqual([false, false, true, true]);
  forgetPageBody(tab.id);
  expect(stopped).toEqual(['thread', 'parts']);
});
