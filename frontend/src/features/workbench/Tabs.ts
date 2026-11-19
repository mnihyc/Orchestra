/** Addressed tab commands share explicit group/workspace scopes with their named menus. */
import { copyText } from '@/ui/clipboard';
import { moveScoped, panelScopeEntries } from '@/features/workbench/panelScope';
import { useWorkbench } from '@/features/workbench/store';
import { contentAddress } from './contentAddress';
import { openContent } from './contentNavigation';
import type { TFunction } from 'i18next';

import { activate, findTab, type Layout, type Target, toggleMaximized, mainGroups, setPinned } from '@/features/workbench/layout';
import { moveEntries } from '@/features/workbench/MoveView';
import { actionKeys } from '@/shell/keys';
import type { MenuEntry } from '@/ui/Menu';
import type { ChoiceHelp } from '@/ui/Choice';

/** Every presentation is closable, including the initially opened content page. */
export function isClosable(layout: Layout, tabId: string): boolean {
  const place = findTab(layout, tabId);
  return place !== null;
}

/** The entries of a tab's menu. `change` applies a change to the layout. */
export function tabEntries(
  layout: Layout,
  tabId: string,
  t: TFunction,
  change: (change: (layout: Layout) => Layout, focusedTab?: string) => void,
  helpFor?: (name: string) => ChoiceHelp | undefined,
): MenuEntry[] {
  const place = findTab(layout, tabId);
  if (place === null) return [];
  const closable = isClosable(layout, tabId);
  const apply = (target: Target) => change((current) => moveScoped(current, tabId, target), tabId);
  const severalGroups = place.area === 'main' && mainGroups(layout.main).length > 1;
  const shown = place.section === undefined ? place.group.active === tabId : place.section.open && !place.section.hidden;

  const path = useWorkbench.getState().named.get(tabId)?.path;
  const entries: MenuEntry[] = [
    { id: 'pin', label: t(place.tab.pinned === true ? 'workbench.tab.unpin' : 'workbench.tab.pin'),
      onSelect: () => change((current) => setPinned(current, tabId, place.tab.pinned !== true)) },
    ...(path === undefined || path === '' ? [] : [{ type: 'label' as const, id: 'target-path', label: path }]),
    ...panelScopeEntries(place.tab, t),
    ...(!shown ? [{
      id: 'show', action: 'show',
      label: t('workbench.tab.show'),
      onSelect: () => change((current) => activate(current, tabId)),
    } satisfies MenuEntry] : []),
    { type: 'separator', id: 'separator-move' },
    { type: 'sub', id: 'move', action: 'moveView', label: t('workbench.move.label'), entries: moveEntries(layout, tabId, t, apply, helpFor) },
  ];
  if (severalGroups) {
    entries.push({
      id: 'maximize', action: layout.maximized === place.group.id ? 'restoreGroup' : 'maximize',
      label: layout.maximized === place.group.id ? t('workbench.tab.restore') : t('workbench.tab.maximize'),
      onSelect: () => change((current) => toggleMaximized(current, place.group.id)),
    });
  }
  if (closable) entries.push(
    { type: 'separator', id: 'separator-close' },
    {
      id: 'close', action: 'dismiss',
      label: t('workbench.tab.close'),
      // The keys the person has for it now, or none shown when it has none.
      shortcut: actionKeys('close-tab'),
      onSelect: () => useWorkbench.getState().close({ kind: 'tab', tab: tabId, explicit: true }),
    },
  );
  if (place.area === 'main' || place.area === 'bottom') entries.push(
    { id: 'close-others-group', label: t('workbench.workspace.closeOtherGroup'), onSelect: () => useWorkbench.getState().close({ kind: 'group-others', tab: tabId }) },
    { id: 'close-all-group', label: t('workbench.workspace.closeAllGroup'), onSelect: () => useWorkbench.getState().close({ kind: 'group', group: place.group.id }) },
    { id: 'reopen', label: t('workbench.workspace.reopen'), disabled: useWorkbench.getState().closedTabs.length === 0 ? t('workbench.workspace.noClosed') : false, onSelect: () => useWorkbench.getState().reopen() },
  );
  const href = contentAddress(place.tab);
  if (href !== null) entries.unshift(
    { id: 'copy-link', action: 'copyLink', label: t('actions.copyLink.label'), onSelect: () => { void copyText(`${location.origin}${href}`); } },
    { id: 'new-tab', label: t('workbench.workspace.newTab'), onSelect: () => { openContent(href, 'new'); } },
    { id: 'beside', label: t('workbench.workspace.beside'), onSelect: () => { openContent(href, 'beside'); } },
  );
  if (place.area === 'main' && useWorkbench.getState().focusMode) entries.push({ id: 'exit-focus', label: t('workbench.workspace.exitFocus'), onSelect: () => useWorkbench.getState().setFocusMode(false) });
  return entries;
}
