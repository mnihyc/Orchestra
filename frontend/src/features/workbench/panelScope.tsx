/** Contextual panels own explicit follow/fixed scope, including when moved away from a sidebar. */
import { retainDocumentMarks } from '@/features/documents/marks';
import { create } from 'zustand';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { NamePicker } from '@/features/search/NamePicker';
import type { NameKind } from '@/api/search';
import { Dialog } from '@/ui/Dialog';
import type { MenuEntry } from '@/ui/Menu';
import { currentHeading, type ScreenHeading } from '@/shell/screen';
import { findGroup, findTab, mapGroups, move, type InspectorScope, type InspectorTarget, type Layout, type Tab, type Target } from './layout';
import { contentLocation, contentTarget, parsePageAddress } from './contentAddress';
import { contentThreadRead, resultThreadRead, type ThreadReadRef } from '@/realtime/threadScope';
import { threadViewOf } from '@/realtime/stores/thread';
import { client } from '@/api/client';
import { useWorkbench } from './store';
import { captureViewAction } from './lossActions';

const contextual = new Set(['files', 'changes', 'git', 'environment', 'processes', 'resources', 'terminal', 'ports', 'browsers', 'displays',
  'outline', 'history', 'chat', 'documents', 'people', 'parameters', 'usage', 'knowledge', 'given', 'diagnostics', 'comments', 'links']);
const environmentPanels = new Set(['files', 'terminal']);
const useTargetChoice = create<{ tab: string | null }>(() => ({ tab: null }));

export function panelTarget(tab: Tab, layout: Layout, active: string | null, heading: ScreenHeading | null): InspectorTarget | null {
  if (tab.scope?.mode === 'fixed') return tab.scope.target;
  if (tab.scope === undefined) {
    if (tab.of?.thread !== undefined) return { kind: 'thread', id: tab.of.thread };
    if (tab.of?.environment !== undefined) return { kind: 'environment', id: tab.of.environment };
  }
  const page = active === null ? null : findTab(layout, active)?.tab;
  const resource = page === null || page === undefined ? null : contentTarget(page);
  if (resource !== null) return resource;
  const target = heading?.target;
  return target?.kind === 'org' ? { kind: 'org', id: target.id } : null;
}
/** Follow shares the exact content owner; only fixed panels acquire a separate retained reader. */
export function panelRead(tab: Tab, layout: Layout, active: string | null, target: InspectorTarget | null): ThreadReadRef | null {
  if (target?.kind !== 'thread') return null;
  if (tab.scope?.mode === 'fixed' || tab.scope === undefined && tab.of?.thread !== undefined) return resultThreadRead(target.id, tab.id);
  const page = active === null ? null : findTab(layout, active)?.tab;
  const source = page === null || page === undefined ? null : contentTarget(page);
  if (page === null || page === undefined || source?.kind !== 'thread' || source.id !== target.id) return null;
  return contentThreadRead(target.id, page.id);
}
export function panelContinuation(tab: Tab): string | null {
  if (tab.scope?.mode === 'fixed') return tab.scope.continuationId ?? null;
  return tab.of?.continuation === undefined || tab.of.continuation === '' ? null : tab.of.continuation;
}
function retainedScope(tab: Tab, layout: Layout, active: string | null, target: InspectorTarget): InspectorScope {
  if (target.kind !== 'thread') return { mode: 'fixed', target };
  const read = panelRead(tab, layout, active, target);
  const scope = read === null ? null : threadViewOf(read).read;
  const page = active === null ? null : findTab(layout, active)?.tab;
  const location = page === null || page === undefined ? null : contentLocation(page);
  const intended = location?.view === 'page' ? parsePageAddress(location.href)?.search.continuation : undefined;
  const continuationId = scope === null || scope.reader !== client.session.getState().readerVersion ? typeof intended === 'string' && intended !== '' ? intended : panelContinuation(tab) : scope.selection;
  return { mode: 'fixed', target, continuationId };
}
export function fixPanelScope(id: string): void {
  const state = useWorkbench.getState(), place = findTab(state.layout, id);
  if (place === null) return;
  const target = panelTarget(place.tab, state.layout, state.activeContent, currentHeading());
  if (target !== null) setPanelScope(id, retainedScope(place.tab, state.layout, state.activeContent, target));
}
export function setPanelScope(id: string, scope: InspectorScope): void {
  const state = useWorkbench.getState(), before = findTab(state.layout, id)?.tab;
  if (before === undefined || state.pendingClose !== null) return;
  const previous = panelTarget(before, state.layout, state.activeContent, currentHeading());
  const next = panelTarget({ ...before, scope }, state.layout, state.activeContent, currentHeading());
  const changedTarget = previous?.kind !== next?.kind || previous?.id !== next?.id;
  captureViewAction([id], changedTarget && state.unsaved.has(id), () => applyPanelScope(id, scope),
    () => findTab(useWorkbench.getState().layout, id)?.tab === before);
}
function applyPanelScope(id: string, scope: InspectorScope): void {
  useWorkbench.getState().change((layout) => mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.map((tab) => {
    if (tab.id !== id) return tab;
    // These old implicit context keys must not override the newly chosen scope inside a feature.
    const of = Object.fromEntries(Object.entries(tab.of ?? {}).filter(([key]) => !['thread', 'environment', 'document', 'node', 'continuation', 'prompt', 'anchor'].includes(key)));
    return { ...tab, scope, ...(Object.keys(of).length === 0 ? { of: undefined } : { of }) };
  }) })));
}
export function moveScoped(layout: Layout, id: string, destination: Target): Layout {
  const place = findTab(layout, id);
  if (place === null) return layout;
  const outside = destination.to === 'split' || destination.to === 'group'
    && !['left', 'right'].includes(findGroup(layout, destination.group)?.area ?? 'main')
    || destination.to === 'area' && destination.area !== 'left' && destination.area !== 'right';
  if (contextual.has(place.tab.view) && (place.area === 'left' || place.area === 'right') && outside && place.tab.scope?.mode !== 'fixed') {
    const target = panelTarget(place.tab, layout, useWorkbench.getState().activeContent, currentHeading());
    if (target?.kind === 'document' && useWorkbench.getState().activeContent !== null) retainDocumentMarks(useWorkbench.getState().activeContent ?? id, id, target.id);
    if (target !== null) layout = mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.map((tab) => tab.id === id ? { ...tab, scope: retainedScope(place.tab, layout, useWorkbench.getState().activeContent, target) } : tab) }));
  }
  return move(layout, id, destination);
}
export function panelScopeEntries(tab: Tab, t: TFunction): MenuEntry[] {
  if (!contextual.has(tab.view)) return [];
  const target = panelTarget(tab, useWorkbench.getState().layout, useWorkbench.getState().activeContent, currentHeading());
  return [
    { type: 'check', id: 'follow', label: t('workbench.workspace.follow'), checked: tab.scope?.mode !== 'fixed', onCheckedChange: () => setPanelScope(tab.id, { mode: 'follow' }) },
    { id: 'fix', label: t('workbench.workspace.fixed'), disabled: target === null ? t('workbench.workspace.noTargets') : false,
      onSelect: () => fixPanelScope(tab.id) },
    { id: 'change-target', label: t('workbench.workspace.changeTarget'), onSelect: () => useTargetChoice.setState({ tab: tab.id }) },
    { type: 'separator', id: 'target-separator' },
  ];
}
export function PanelTargetChoice() {
  const { t } = useTranslation();
  const id = useTargetChoice((state) => state.tab);
  const tab = useWorkbench((state) => id === null ? null : findTab(state.layout, id)?.tab ?? null);
  const kinds: readonly NameKind[] = tab?.view === 'comments' || tab?.view === 'links' ? ['document'] : tab !== null && environmentPanels.has(tab.view) ? ['thread', 'environment']
    : tab?.view === 'chat' || tab?.view === 'documents' ? ['org', 'project', 'topic', 'thread'] : tab?.view === 'outline' ? ['thread', 'document'] : ['thread'];
  return <Dialog open={id !== null && tab !== null} onOpenChange={(open) => { if (!open) useTargetChoice.setState({ tab: null }); }} title={t('workbench.workspace.target')}>
    <NamePicker label={t('workbench.workspace.target')} kinds={kinds} chosen={null} placeholder={t('workbench.workspace.selectTarget')} nothing={t('workbench.workspace.noTargets')}
      onChoose={(hit) => {
        if (hit === null || id === null || !['thread', 'document', 'environment', 'org', 'project', 'topic'].includes(hit.kind)) return;
        setPanelScope(id, { mode: 'fixed', target: { kind: hit.kind as InspectorTarget['kind'], id: hit.id } });
        useTargetChoice.setState({ tab: null });
      }} />
  </Dialog>;
}
