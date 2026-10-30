/** A result's requested area and exact anchor preserve the main conversation's tab and reactivate an existing result. */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { allTabs, close, findTab, mainGroups, open } from '@/features/workbench/layout';
import { shippedLayout } from '@/features/workbench/Arrangements';
import { contentAddress } from '@/features/workbench/contentAddress';
import { useWorkbench } from '@/features/workbench/store';
import { openResult, resultLayout, targetOfDestination, targetOfHit, threadResultHref } from '@/features/search/ResultView';
import type { SearchHit } from '@/generated/api-types';
import { agentMessage } from '../doubles/thread-rows';

const state = () => useWorkbench.getState();
beforeEach(() => { state().leave(); localStorage.clear(); state().enter('search-workspace-reader', 'thread'); });
afterEach(() => { state().leave(); localStorage.clear(); });

describe('opening a result beside the conversation', () => {
  const target = { kind: 'document', id: 'doc', anchor: 'block', href: '/doc/doc?block=block' };
  it('keeps the original full thread tab and opens the exact document as another full view beside it', () => {
    const thread = state().openTab('page', { href: '/thread/original' });
    const initial = state().layout;
    openResult({ kind: 'document', id: 'doc', anchor: 'block' }, null);
    const beside = state().layout;
    expect(mainGroups(beside.main)).toHaveLength(2);
    expect(findTab(beside, thread)?.group.id).toBe(findTab(initial, thread)?.group.id);
    const document = allTabs(beside).find((tab) => contentAddress(tab) === target.href);
    expect(document?.view).toBe('page');
    if (document === undefined) throw new Error('the full document is open');
    expect(allTabs(beside).some((tab) => tab.view === 'search-result')).toBe(false);
    openResult({ kind: 'document', id: 'doc', anchor: 'block' }, null);
    const again = state().layout;
    const duplicates = allTabs(again).filter((tab) => contentAddress(tab) === target.href);
    expect(duplicates).toHaveLength(2);
    expect(new Set(duplicates.map((tab) => tab.id)).size).toBe(2);
    expect(mainGroups(again.main)).toHaveLength(3);
    const closed = close(beside, document.id);
    expect(mainGroups(closed.main)).toHaveLength(1);
    expect(findTab(closed, thread)).not.toBeNull();
  });
  it('honours an explicitly requested area', () => {
    const layout = resultLayout(open(shippedLayout('thread'), 'screen', 'main'), target, 'bottom');
    const result = allTabs(layout).find((tab) => tab.view === 'search-result');
    expect(result === undefined ? null : findTab(layout, result.id)?.area).toBe('bottom');
  });
  it('keeps active, A and B bottom previews distinct without an href, while equal locators reuse their tab', () => {
    const initial = open(shippedLayout('thread'), 'screen', 'main');
    const active = { kind: 'thread', id: 'thread', anchor: 'inherited-item' };
    const a = { ...active, continuation: 'A' }, b = { ...active, continuation: 'B' };
    const three = resultLayout(resultLayout(resultLayout(initial, a, 'bottom'), b, 'bottom'), active, 'bottom');
    expect(allTabs(three).filter((tab) => tab.view === 'search-result')).toHaveLength(3);
    expect(allTabs(resultLayout(three, { ...active, continuation: '' }, 'bottom')).filter((tab) => tab.view === 'search-result')).toHaveLength(3);
    expect(allTabs(resultLayout(three, b, 'bottom')).filter((tab) => tab.view === 'search-result')).toHaveLength(3);
    expect(allTabs(resultLayout(three, { ...b, anchor: 'another-item' }, 'bottom')).filter((tab) => tab.view === 'search-result')).toHaveLength(4);
  });
  it('opens active and historical search hits as full thread panes with their own exact URLs', () => {
    for (const continuationId of ['A', 'B', null]) {
      openResult({ kind: 'thread', id: 'thread', anchor: 'inherited-item', continuationId }, null);
    }
    const tabs = allTabs(state().layout).filter((tab) => tab.view === 'page');
    expect(tabs.map(contentAddress)).toEqual([
      '/thread/thread?item=inherited-item&continuation=A',
      '/thread/thread?item=inherited-item&continuation=B',
      '/thread/thread?item=inherited-item',
    ]);
    expect(new Set(tabs.map((tab) => tab.id)).size).toBe(3);
    expect(new Set(tabs.map((tab) => findTab(state().layout, tab.id)?.group.id)).size).toBe(3);
  });
  it('uses a search hit’s frozen projection rather than its item’s owning continuation', () => {
    const hit: Extract<SearchHit, { kind: 'item' }> = { kind: 'item', snippet: 'Frozen answer', matches: [],
      item: { ...agentMessage('inherited-item', 'Frozen answer'), continuation_id: 'A' },
      thread: { kind: 'thread', id: 'thread', name: 'Thread', redacted: false },
      topic: { kind: 'topic', id: 'topic', name: 'Topic', redacted: false },
      project: { kind: 'project', id: 'project', name: 'Project', redacted: false },
      projection: { viewed_continuation_id: 'B', active_continuation_id: 'A', projection_revision: 1, execution_revision: 2, follows_active: false } };
    expect(targetOfHit(hit)).toEqual({ kind: 'thread', id: 'thread', anchor: 'inherited-item', continuationId: 'B' });
    expect(targetOfDestination({ to: '/thread/thread?item=inherited-item&continuation=B' })).toEqual(targetOfHit(hit));
    expect(threadResultHref({ kind: 'thread', id: 'thread', anchor: 'inherited-item', continuation: 'B', href: '/thread/thread?continuation=A' }))
      .toBe('/thread/thread?item=inherited-item&continuation=B');
  });
  it('keeps a chat message’s real chat identity instead of turning it into its surrounding place', () => {
    expect(targetOfDestination({ to: '/topic/$topicId', params: { topicId: 'topic' }, search: { panel: 'chat', chat: 'chat', message: 'message' } })).toEqual({ kind: 'chat', id: 'chat', anchor: 'message' });
    expect(targetOfDestination({ to: '/doc/$documentId', params: { documentId: 'doc' }, search: { block: 'block' } })).toEqual({ kind: 'document', id: 'doc', anchor: 'block' });
    expect(targetOfDestination({ to: '/org/own/messages?chat=direct&message=reply' })).toEqual({ kind: 'chat', id: 'direct', anchor: 'reply' });
    expect(targetOfDestination({ to: '/topic/topic?panel=chat' })).toEqual({ kind: 'place-chat', id: 'topic' });
    expect(targetOfDestination({ to: '/doc/doc?block=block' })).toEqual({ kind: 'document', id: 'doc', anchor: 'block' });
  });
});
