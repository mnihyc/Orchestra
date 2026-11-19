/** The Layout menu and command palette invoke the same scoped workspace actions. */
import type { TFunction } from 'i18next';
import { Grid2X2, Grid3X3, Maximize, RotateCcw, X } from 'lucide-react';
import { useMemo } from 'react';
import { useRegisterCommands, type Command } from '@/features/finder/commands';
import { useWorkbench } from './store';
import { applyGrid, spatialMainGroups } from './layout';
import { viewOf } from './views';
import type { MenuEntry } from '@/ui/Menu';

export function workspaceEntries(t: TFunction): MenuEntry[] {
  const state = useWorkbench.getState();
  return [
    { id: 'grid-2x4', label: t('workbench.workspace.grid2x4'), icon: <Grid2X2 aria-hidden />, onSelect: () => state.change((layout) => applyGrid(layout, '2x4', useWorkbench.getState().focusedMainTab)) },
    { id: 'grid-3x3', label: t('workbench.workspace.grid3x3'), icon: <Grid3X3 aria-hidden />, onSelect: () => state.change((layout) => applyGrid(layout, '3x3', useWorkbench.getState().focusedMainTab)) },
    { type: 'check', id: 'focus', label: t('workbench.workspace.focus'), icon: <Maximize aria-hidden />, checked: state.focusMode, onCheckedChange: state.setFocusMode },
    { type: 'separator', id: 'workspace-close' },
    { id: 'close-other-main', label: t('workbench.workspace.closeOtherMain'), icon: <X aria-hidden />, onSelect: () => {
      const current = useWorkbench.getState();
      if (current.focusedMainTab !== null) current.close({ kind: 'main-others', tab: current.focusedMainTab });
    } },
    { id: 'close-all-main', label: t('workbench.workspace.closeAllMain'), icon: <X aria-hidden />, onSelect: () => state.close({ kind: 'main' }) },
    { id: 'reopen', label: t('workbench.workspace.reopen'), icon: <RotateCcw aria-hidden />,
      disabled: state.closedTabs.length === 0 ? t('workbench.workspace.noClosed') : false, onSelect: state.reopen },
  ];
}

export function WorkspaceCommands() {
  const focus = useWorkbench((state) => state.focusMode);
  const layout = useWorkbench((state) => state.layout);
  const named = useWorkbench((state) => state.named);
  const entries = useMemo<Command[]>(() => [
    ...spatialMainGroups(layout.main).flatMap((group) => group.tabs).map((tab): Command => ({ id: `workspace.focus.${tab.id}`, kind: 'view',
      title: (t) => named.get(tab.id)?.title ?? t(viewOf(tab.view)?.title ?? 'views.screen.title'),
      note: () => named.get(tab.id)?.path ?? '', icon: named.get(tab.id)?.icon ?? viewOf(tab.view)?.icon,
      run: () => useWorkbench.getState().focus(tab.id),
    })),
    ...(['2x4', '3x3'] as const).map((preset) => ({ id: `view.grid.${preset}`, kind: 'view' as const,
      title: (t: TFunction) => t(preset === '2x4' ? 'workbench.workspace.grid2x4' : 'workbench.workspace.grid3x3'), icon: Grid2X2,
      run: () => useWorkbench.getState().change((layout) => applyGrid(layout, preset, useWorkbench.getState().focusedMainTab)) })),
    { id: 'view.focus-mode', kind: 'view', title: (t) => t(focus ? 'workbench.workspace.exitFocus' : 'workbench.workspace.focus'), icon: Maximize,
      run: () => useWorkbench.getState().setFocusMode(!useWorkbench.getState().focusMode) },
    { id: 'view.close-all-main', kind: 'view', title: (t) => t('workbench.workspace.closeAllMain'), icon: X, run: () => useWorkbench.getState().close({ kind: 'main' }) },
    { id: 'view.close-other-main', kind: 'view', title: (t) => t('workbench.workspace.closeOtherMain'), icon: X, run: () => {
      const current = useWorkbench.getState();
      if (current.focusedMainTab !== null) current.close({ kind: 'main-others', tab: current.focusedMainTab });
    } },
    { id: 'view.reopen', kind: 'view', title: (t) => t('workbench.workspace.reopen'), icon: RotateCcw, run: () => useWorkbench.getState().reopen(),
      available: () => useWorkbench.getState().closedTabs.length > 0 },
  ], [focus, layout, named]);
  useRegisterCommands(entries);
  return null;
}
