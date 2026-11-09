/** An explicit Review act retargets the review layout's existing Changes panel. */
import { allTabs, findTab, mapGroups, setTabTarget } from '@/features/workbench/layout';
import { revealTab, revealView } from '@/features/workbench/openView';
import { useWorkbench } from '@/features/workbench/store';

export function openTurnReview(threadId: string, turnId: string): void {
  const state = useWorkbench.getState();
  state.choose('review');
  const layout = useWorkbench.getState().layout;
  const target = { thread: threadId, scope: 'turn', turn: turnId };
  const panel = allTabs(layout).find((tab) => tab.view === 'changes' && findTab(layout, tab.id)?.area === 'main');
  if (panel === undefined) { revealView('changes', target); return; }
  useWorkbench.getState().change((current) => mapGroups(setTabTarget(current, panel.id, target), (group) => ({ ...group,
    tabs: group.tabs.map((tab) => tab.id === panel.id ? { ...tab, scope: { mode: 'fixed' as const, target: { kind: 'thread' as const, id: threadId } } } : tab),
  })));
  revealTab(panel.id);
}

/** The header and shortcut reuse a Changes panel already bound to this thread. */
export function openThreadChanges(threadId: string, turnId?: string): void {
  const layout = useWorkbench.getState().layout;
  const panel = allTabs(layout).find((tab) => tab.view === 'changes' && (tab.of?.thread === threadId
    || tab.scope?.mode === 'fixed' && tab.scope.target.kind === 'thread' && tab.scope.target.id === threadId));
  const target = { thread: threadId, scope: 'thread', ...(turnId === undefined ? {} : { turn: turnId }) };
  if (panel === undefined) { revealView('changes', target); return; }
  useWorkbench.getState().change((current) => setTabTarget(current, panel.id, target));
  revealTab(panel.id);
}
