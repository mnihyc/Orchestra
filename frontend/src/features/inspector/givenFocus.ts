/** Exact model-input selection belongs to one inspector tab and survives its normal presentation lifecycle. */
import { allTabs, findTab, findView, mapGroups, type Tab } from '@/features/workbench/layout';
import { revealTab, revealView } from '@/features/workbench/openView';
import { panelTarget } from '@/features/workbench/panelScope';
import { useWorkbench } from '@/features/workbench/store';
import { currentHeading } from '@/shell/screen';

export interface GivenSelection { turn?: string; record?: string }

/** Explicit message/record actions fix only a compatible Given inspector to their actual source. */
export function openGiven(thread: string, selection: GivenSelection = {}, owner?: string): void {
  const state = useWorkbench.getState();
  const compatible = (tab: Tab) => {
    if (tab.view !== 'given') return false;
    const target = panelTarget(tab, state.layout, state.activeContent, currentHeading());
    return target?.kind === 'thread' && target.id === thread;
  };
  const preferred = owner === undefined ? null : findTab(state.layout, owner)?.tab;
  const focused = state.focusedTab === null ? null : findTab(state.layout, state.focusedTab)?.tab;
  let target = preferred !== null && preferred !== undefined && compatible(preferred) ? preferred
    : focused !== null && focused !== undefined && compatible(focused) ? focused
      : allTabs(state.layout).find(compatible);
  let opened = false;
  if (target === undefined) {
    revealView('given', { thread });
    target = findView(useWorkbench.getState().layout, 'given', { thread })?.tab;
    opened = true;
  }
  if (target === undefined) return;
  const id = target.id;
  state.change((layout) => mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.map((tab) => tab.id === id ? {
    ...tab, scope: { mode: 'fixed' as const, target: { kind: 'thread' as const, id: thread } },
    of: { thread, givenThread: thread, ...(selection.turn === undefined ? {} : { givenTurn: selection.turn }), ...(selection.record === undefined ? {} : { givenRecord: selection.record }) },
  } : tab) })));
  if (!opened) revealTab(id);
}

/** Clearing a local filter preserves the inspector's explicit resource and other inspectors' selections. */
export function clearGiven(owner: string): void {
  useWorkbench.getState().change((layout) => mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.map((tab) => {
    if (tab.id !== owner || tab.view !== 'given') return tab;
    return { ...tab, of: Object.fromEntries(Object.entries(tab.of ?? {}).filter(([key]) => !['givenThread', 'givenTurn', 'givenRecord'].includes(key))) };
  }) })));
}
