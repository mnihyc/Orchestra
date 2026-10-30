import { EmptyMain } from '@/features/workbench/EmptyMain';
import { useFrameReady } from '@/shell/FrameReadiness';
/** Phone navigation chooses the same containers in the same order, with single-view sheets for main and bottom tabs. */
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { LeaveDrawer } from '@/features/navigation/OrgSwitcher';
import { useVisibleLayout } from '@/features/workbench/availability';
import { ViewSlot } from '@/features/workbench/Board';
import { ViewActionSlot } from '@/features/workbench/actions';
import { containerOf, FALLBACK_CONTAINER_ICON } from '@/features/workbench/containers';
import { allTabs, findTab, spatialMainGroups, selectContainer, type SideBarId, type Tab } from '@/features/workbench/layout';
import { ContainerSheet } from '@/features/workbench/Narrow';
import { useWorkbench } from '@/features/workbench/store';
import { onViewRevealed, revealView } from '@/features/workbench/openView';
import { containerChoice, viewChoice } from '@/features/workbench/choiceWords';
import { viewOf } from '@/features/workbench/views';
import { useViewChoice } from '@/lib/viewChoices';
import { Drawer } from '@/ui/Drawer';
import { Button } from '@/ui/Button';
import type { NarrowChoice } from '@/features/workbench/NarrowChoices';
import { useChromeBottom } from '@/features/workbench/chrome';

export function useNarrowWorkbench(pending = false, focusMode = false) {
  const ready = useFrameReady();
  const { t } = useTranslation();
  const top = useChromeBottom();
  const layout = useVisibleLayout();
  const change = useWorkbench((state) => state.change);
  const activeContent = useWorkbench((state) => state.activeContent);
  const focusedMainGroup = useWorkbench((state) => state.focusedMainGroup);
  const focusedMainTab = useWorkbench((state) => state.focusedMainTab);
  const route = activeContent ?? 'workbench';
  const [navigationAt, setNavigationAt] = useState<string | null>(null);
  useEffect(() => onViewRevealed(() => setNavigationAt(null)), []);
  const navigation = navigationAt === route;
  const setNavigation = (open: boolean) => setNavigationAt(open ? route : null);
  const [selectedView, setSelectedView] = useViewChoice<string>('global', 'mobileView', '');
  const [selectedTab, setSelectedTab] = useViewChoice<string>('global', 'mobileTab', '');
  const [selectedContainer, setSelectedContainer] = useViewChoice<string>('global', 'mobileContainer', '');
  const [selectedBar, setSelectedBar] = useViewChoice<SideBarId>('global', 'mobileBar', 'right');
  const tabs = allTabs(layout);
  const selected = tabs.find((tab) => tab.id === selectedTab && tab.view === selectedView) ?? tabs.findLast((tab) => tab.view === selectedView);
  const place = selected === undefined ? null : findTab(layout, selected.id);
  const bar = place?.container !== undefined && (place.area === 'left' || place.area === 'right') ? place.area : selectedBar;
  const wanted = selectedView !== '' ? place?.container?.name ?? '' : selectedContainer;
  const container = layout[bar].containers.find((candidate) => candidate.name === wanted);
  const viewer = selectedView !== '' && container === undefined && selected !== undefined && !['screen', 'page', 'navigation', 'resource-navigation'].includes(selected.view) ? selected : null;
  const groups = spatialMainGroups(layout.main);
  const group = groups.find((entry) => entry.id === focusedMainGroup) ?? groups[0];
  const primary = tabs.find((tab) => tab.id === focusedMainTab) ?? group?.tabs.find((tab) => tab.id === group.active) ?? null;
  const close = () => { setSelectedView(''); setSelectedContainer(''); };
  useEffect(() => useWorkbench.subscribe((next, before) => {
    if (next.activeContent === before.activeContent || next.userId !== before.userId) return;
    setNavigationAt(null); setSelectedView(''); setSelectedContainer('');
  }), [setSelectedView, setSelectedContainer]);
  const choose = (area: SideBarId, name: string) => {
    if (!navigation && container?.name === name && bar === area) { close(); return; }
    setNavigation(false);
    change((current) => selectContainer(current, area, name));
    setSelectedView(''); setSelectedContainer(name); setSelectedBar(area);
  };
  const openNavigation = () => { close(); change((current) => selectContainer(current, 'left', 'navigation')); setNavigation(!navigation); };
  const choices: NarrowChoice[] = (['left', 'right'] as const).flatMap((area) => layout[area].containers.map((item): NarrowChoice => {
    const definition = containerOf(item.name);
    const view = viewOf(item.sections[0]?.tab.view ?? '');
    const Icon = definition?.icon ?? view?.icon ?? FALLBACK_CONTAINER_ICON;
    const choice = definition === undefined && view !== undefined ? viewChoice(view, t) : containerChoice(item.name, t);
    const isNavigation = area === 'left' && item.name === 'navigation';
    return { id: `${area}:${item.name}`, name: choice.name, description: choice.description, icon: <Icon aria-hidden />,
      active: isNavigation ? navigation : !navigation && container?.name === item.name && bar === area,
      onSelect: isNavigation ? openNavigation : () => choose(area, item.name) };
  }));
  for (const tab of tabs.filter((tab) => findTab(layout, tab.id)?.area === 'bottom' || ['file', 'difference', 'browser', 'vnc'].includes(tab.view))) {
    const view = viewOf(tab.view);
    if (view === undefined) continue;
    const Icon = view.icon;
    const choice = viewChoice(view, t);
    const name = t('workbench.choices.tab.name', { name: tab.of?.path ?? choice.name });
    choices.push({ id: tab.id, name, description: t('workbench.choices.tab.description', { description: choice.description }), icon: <Icon aria-hidden />,
      active: !navigation && viewer?.id === tab.id, onSelect: () => { if (!navigation && viewer?.id === tab.id) { close(); return; } close(); useWorkbench.getState().focus(tab.id); setSelectedTab(tab.id); setSelectedView(tab.view); } });
  }
  const body = (tab: Tab) => <ViewSlot tab={tab} primary />;
  const title = viewer === null ? '' : viewer.of?.path ?? t(viewOf(viewer.view)?.title ?? 'views.screen.title');
  return {
    choices,
    primary,
    mainTabs: groups.flatMap((group) => group.tabs),
    primaryGroup: group?.id ?? null,
    selectMain: (id: string) => { close(); setNavigation(false); useWorkbench.getState().focus(id); },
    // A phone sheet covers the page. Removing its slot pauses the kept page's readers and effects.
    main: pending || !focusMode && (navigation || container !== undefined || viewer !== null) ? null
      : primary === null ? <EmptyMain groupId={group?.id ?? null} /> : <ViewSlot tab={primary} primary />,
    openNavigation,
    sheets: pending || focusMode ? null : <LeaveDrawer.Provider value={() => setNavigation(false)}>
      <div onClickCapture={(event) => { if (event.button === 0 && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey && (event.target as HTMLElement).closest('a[href]') !== null) setNavigation(false); }}>
        <ContainerSheet bar="left" layout={layout.left} open={navigation} onOpenChange={setNavigation} onSelect={(name) => choose('left', name)} renderView={body} side="left" restoreFocus={container === undefined && viewer === null} top={top} externalSwitcher />
      </div>
      <ContainerSheet bar={bar} layout={{ ...layout[bar], active: container?.name ?? layout[bar].active }} open={container !== undefined && !navigation} onOpenChange={(open) => { if (!open) close(); }} onSelect={(name) => choose(bar, name)} renderView={body} top={top} externalSwitcher />
      <Drawer prompt={false} side="bottom" title={title} actions={!ready || viewer === null ? undefined : <ViewActionSlot tab={viewer} />} open={viewer !== null && !navigation} onOpenChange={(open) => { if (!open) close(); }} persistent modal={false} scroll={false} className="h-[calc(100dvh-var(--control-md))] max-h-none" style={top === undefined ? undefined : { top, height: 'auto', maxHeight: 'none' }}>
        {viewer?.view === 'file' && <Button action="backFiles" size="sm" onClick={() => revealView('files')}>{t('craft.backToFiles')}</Button>}
        {viewer !== null && <div data-revealed-view data-area-stop tabIndex={-1} className="relative h-full min-h-0 overflow-hidden outline-none"><ViewSlot tab={viewer} primary /></div>}
      </Drawer>
    </LeaveDrawer.Provider>,
  };
}
