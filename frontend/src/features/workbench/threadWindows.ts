/** Tab lifetime stays observable after Activity pauses a view's effects. */
import { useEffect } from 'react';
import { forgetThreadRead } from '@/realtime/live';
import { useThreadStore } from '@/realtime/stores/thread';
import { timelineUi } from '@/features/timeline/state';
import { allTabs, type Layout, type Tab } from './layout';
import { useWorkbench } from './store';
import { contentThreadId } from './contentAddress';

function threadOf(tab: Tab): string | null {
  if (tab.scope?.mode === 'fixed' && tab.scope.target.kind === 'thread') return tab.scope.target.id;
  const contentThread = contentThreadId(tab);
  if (contentThread !== null) return contentThread;
  if (tab.view === 'subagent') return tab.of?.thread ?? null;
  if (tab.view === 'side-thread') return tab.of?.side ?? null;
  return null;
}

export function retireThreadWindows(before: Layout, after: Layout): void {
  const current = new Map(allTabs(after).map((tab) => [tab.id, tab]));
  const retired = new Set(allTabs(before).filter((tab) => {
    const next = current.get(tab.id);
    return next?.view !== tab.view || threadOf(next) !== threadOf(tab);
  }).map((tab) => tab.id));
  if (retired.size === 0) return;
  // Promotion keeps the same logical main timeline, but its old transport/receipts must retire
  // before the full controls owner is admitted. Derived child readers still retire normally.
  const promoted = new Map(allTabs(before).flatMap((tab) => {
    const next = current.get(tab.id), thread = contentThreadId(tab);
    return tab.view === 'search-result' && next?.view === 'page' && thread !== null && contentThreadId(next) === thread ? [[tab.id, thread] as const] : [];
  }));
  // Derived side-thread reads use the same owner, with their own actual native thread ID.
  for (const view of useThreadStore.getState().byThread.values()) {
    const read = view.ref;
    if (read?.key.kind === 'result' && retired.has(read.key.owner)) {
      forgetThreadRead(read);
      if (promoted.get(read.key.owner) !== read.threadId) timelineUi.forget(read);
    }
  }
}

export function useThreadWindowLifetime(): void {
  useEffect(() => useWorkbench.subscribe((next, before) => {
    if (next.layout !== before.layout) retireThreadWindows(before.layout, next.layout);
  }), []);
}
