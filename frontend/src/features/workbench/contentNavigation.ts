/** Resource opens use the one workbench store and the one application router. */
import { allTabs, findTab, mapGroups, setTabTarget } from './layout';
import { contentAddress, contentHref, contentLocation, contentTab, parseContentAddress, parsePageAddress, sameContentResource, type ContentLocation } from './contentAddress';
import { sidebarChoices } from '@/features/navigation/preferences';
import { captureViewAction } from './lossActions';
import { pageDestination, locatePage } from './pageTargets';
import { mark } from '@/lib/marks';
import { startPageReads } from './pageReads';
import { useWorkbench } from './store';

export type ContentHistory = 'push' | 'replace' | 'none';
type Navigator = (owner: string, href: string, replace: boolean) => void;
let navigator: Navigator | null = null;
/** Installed once by PageRouting, never by a pane or a nested router. */
export function bindContentNavigation(next: Navigator): () => void {
  navigator = next;
  return () => { if (navigator === next) navigator = null; };
}

/** An explicit content-tab choice supersedes an older pending browser navigation, even if already selected. */
export function selectContentTab(id: string): void {
  mark('switch/click', { tab: id });
  useWorkbench.getState().focus(id);
  const current = useWorkbench.getState();
  if (current.activeContent !== id) return;
  const tab = findTab(current.layout, id)?.tab;
  const href = tab === undefined ? null : contentAddress(tab);
  if (href !== null) navigator?.(id, href, true);
}

export function openContent(address: string | ContentLocation, mode: 'normal' | 'new' | 'beside' = 'normal', history: ContentHistory = 'none'): string | null {
  const parsed = typeof address === 'string' ? parseContentAddress(address) : parseContentAddress(contentHref(address));
  if (parsed === null) return null;
  const state = useWorkbench.getState();
  const matches = mode === 'normal' ? allTabs(state.layout).filter((tab) => {
    const location = contentLocation(tab);
    return location !== null && sameContentResource(location, parsed);
  }) : [];
  const existing = matches.find((tab) => tab.id === state.activeContent) ?? matches[0];
  const target = contentTab(parsed);
  const to = parsed.view === 'page' ? parsePageAddress(parsed.href) : null;
  const preference = sidebarChoices().openLinks ?? 'automatic';
  const focused = state.activeContent === null ? null : findTab(state.layout, state.activeContent)?.tab ?? null;
  const destination = mode === 'normal' && preference === 'automatic' && to !== null ? allTabs(state.layout).find((tab) => {
    const href = contentAddress(tab), page = href === null ? null : parsePageAddress(href);
    return tab.pinned !== true && page !== null && pageDestination(page) === pageDestination(to);
  }) : undefined;
  const active = destination ?? focused;
  const fromHref = active === null ? null : contentAddress(active);
  const from = fromHref === null ? null : parsePageAddress(fromHref);
  const reuse = mode === 'normal' && existing === undefined && active !== null && to !== null
    && (preference === 'this' || preference === 'automatic' && active.pinned !== true && from !== null && pageDestination(from) === pageDestination(to));
  if (reuse) {
    const current = () => findTab(useWorkbench.getState().layout, active.id)?.tab === active;
    captureViewAction([active.id], state.unsaved.has(active.id), () => {
      const fresh = useWorkbench.getState();
      fresh.change((layout) => mapGroups(layout, (group) => ({ ...group, tabs: group.tabs.map((tab) => tab.id === active.id ? { ...tab, ...target } : tab) })));
      fresh.setNamed(active.id, null);
      fresh.focus(active.id);
      const changed = findTab(useWorkbench.getState().layout, active.id)?.tab;
      if (changed !== undefined) { startPageReads(changed); mark('tab/opened', { tab: active.id }); }
      if (history !== 'none') navigator?.(active.id, contentHref(parsed), history === 'replace');
    }, current);
    const landed = findTab(useWorkbench.getState().layout, active.id)?.tab;
    return landed !== undefined && contentAddress(landed) === contentHref(parsed) ? active.id : null;
  }
  let id: string;
  if (existing !== undefined) {
    id = existing.id;
    if (contentAddress(existing) !== contentHref(parsed)) state.change((layout) => setTabTarget(layout, id, target.of));
    state.focus(id);
  } else id = state.openTab(target.view, target.of, mode);
  const current = findTab(useWorkbench.getState().layout, id);
  // A tab opened now reads what its address names at once, not when its screen has been drawn.
  if (existing === undefined && current !== null) startPageReads(current.tab);
  if (current !== null) mark('tab/opened', { tab: current.tab.id });
  if (history !== 'none' && current !== null) {
    const href = contentAddress(current.tab);
    if (href !== null) navigator?.(id, href, history === 'replace');
  }
  return id;
}


/** Plain internal anchors share the same act as buttons; browser modifiers, downloads and card actions retain their semantics. */
export function followContentLink(event: MouseEvent): void {
  if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
  const link = event.target instanceof Element ? event.target.closest<HTMLAnchorElement>('a[href]') : null;
  if (link === null || link.hasAttribute('download') || link.target !== '' && link.target !== '_self' || link.getAttribute('href')?.startsWith('#') === true) return;
  const target = parseContentAddress(link.href);
  if (target === null || target.view === 'page' && (parsePageAddress(target.href) === null || locatePage(new URL(link.href).pathname) === null)) return;
  event.preventDefault();
  openContent(target, 'normal', 'push');
}
