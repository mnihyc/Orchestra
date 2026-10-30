/** One open-view act that reveals content in a desktop bar or the phone's current sheet. */
import { client } from '@/api/client';
import { activate, allTabs, findTab, findView, move, open, setTabTarget, type Place, type Tab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { viewContainer, viewOf, viewOrder } from '@/features/workbench/views';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { useHelpState } from '@/features/help/state';

const revealed = new Set<() => void>();
/** A sheet choosing an externally revealed view gives up its previous navigation surface. */
export function onViewRevealed(listener: () => void): () => void {
  revealed.add(listener);
  return () => { revealed.delete(listener); };
}

/** Opens one view visibly; the narrow layout reads the same named choice. */
export function revealView(name: string, of?: Tab['of'], options: { focus?: boolean } = {}): void {
  const definition = viewOf(name);
  if (definition === undefined) return;
  if (options.focus !== false && name !== 'help' && useHelpState.getState().overlay) useHelpState.setState({ overlay: false });
  const workbench = useWorkbench.getState();
  const single = definition.singleton === true ? allTabs(workbench.layout).find((tab) => tab.view === name) : undefined;
  const target = of ?? single?.of ?? (definition.available === undefined ? undefined : allTabs(workbench.layout).findLast((tab) => tab.view === name)?.of);
  workbench.change((layout) => {
    if (single !== undefined && of !== undefined) layout = setTabTarget(layout, single.id, of);
    const home = viewContainer(definition, workbench.screen);
    const existing = findView(layout, name, target);
    const opened = open(layout, name, definition.area, target, home);
    if (existing !== null || (definition.area !== 'left' && definition.area !== 'right')) return opened;
    const container = layout[definition.area].containers.find((candidate) => candidate.name === home);
    const index = container?.sections.findIndex((section) => {
      const other = viewOf(section.tab.view);
      return other !== undefined && viewOrder(other, workbench.screen) > viewOrder(definition, workbench.screen);
    });
    const added = findView(opened, name, target);
    return index === undefined || index < 0 || added === null ? opened : move(opened, added.tab.id, { to: 'container', bar: definition.area, container: home, index });
  });
  const place = findView(useWorkbench.getState().layout, name, target);
  if (place !== null) finishRevealing(place, options);
}

/** Reveal an addressed inspector instance without matching another panel of the same kind. */
export function revealTab(id: string, options: { focus?: boolean } = {}): void {
  if (findTab(useWorkbench.getState().layout, id) === null) return;
  useWorkbench.getState().change((layout) => activate(layout, id));
  const place = findTab(useWorkbench.getState().layout, id);
  if (place !== null) finishRevealing(place, options);
}

function finishRevealing(place: Place, options: { focus?: boolean }): void {
  const workbench = useWorkbench.getState(), name = place.tab.view;
  if (options.focus !== false && name !== 'help' && useHelpState.getState().overlay) useHelpState.setState({ overlay: false });
  for (const listener of revealed) listener();
  workbench.setOverlay(place.area === 'right' ? 'right' : null);
  writeViewChoice(client.session.getState().userId, 'global', 'mobileView', name);
  writeViewChoice(client.session.getState().userId, 'global', 'mobileTab', place.tab.id);
  writeViewChoice(client.session.getState().userId, 'global', 'mobileContainer', place.container?.name ?? '');
  if (place.area === 'left' || place.area === 'right') writeViewChoice(client.session.getState().userId, 'global', 'mobileBar', place.area);
  requestAnimationFrame(() => {
    if (findTab(useWorkbench.getState().layout, place.tab.id) === null) return;
    const element = document.getElementById(place.section === undefined ? `tab-${place.tab.id}` : `section-${place.tab.id}`)
      ?? document.querySelector<HTMLElement>('[data-revealed-view]');
    element?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    if (options.focus !== false) element?.focus({ preventScroll: true });
  });
}

/** The actual tab carries an opened file's target into its phone sheet. */
export function mobileViewTab(name: string): Tab | undefined {
  const tabs = allTabs(useWorkbench.getState().layout);
  const wanted = readViewChoice<string>(client.session.getState().userId, 'global', 'mobileTab', '');
  return tabs.find((tab) => tab.id === wanted && tab.view === name) ?? tabs.findLast((tab) => tab.view === name);
}
