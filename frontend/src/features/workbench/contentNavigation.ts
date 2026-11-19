/** Resource opens use the one workbench store and the one application router. */
import { allTabs, findTab, setTabTarget } from './layout';
import { contentAddress, contentHref, contentLocation, contentTab, parseContentAddress, sameContentResource, type ContentLocation } from './contentAddress';
import { useWorkbench } from './store';

export type ContentHistory = 'push' | 'replace' | 'none';
type Navigator = (owner: string, href: string, replace: boolean) => void;
let navigator: Navigator | null = null;
/** Installed once by PageRouting, never by a pane or a nested router. */
export function bindContentNavigation(next: Navigator): () => void {
  navigator = next;
  return () => { if (navigator === next) navigator = null; };
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
  let id: string;
  if (existing !== undefined) {
    id = existing.id;
    if (contentAddress(existing) !== contentHref(parsed)) state.change((layout) => setTabTarget(layout, id, target.of));
    state.focus(id);
  } else id = state.openTab(target.view, target.of, mode);
  const current = findTab(useWorkbench.getState().layout, id);
  if (history !== 'none' && current !== null) {
    const href = contentAddress(current.tab);
    if (href !== null) navigator?.(id, href, history === 'replace');
  }
  return id;
}
