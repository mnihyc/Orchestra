/** What a tab's page starts reading at the moment the tab is opened for its address.
 *
 * A page used to begin its first read only when its screen had been drawn, several components after the tab was opened,
 * although the address already says what it is. The reads that need nothing but that address start here instead, and the
 * screen, when it comes, finds them under way (the feed registry shares one follower between whoever asks).
 *
 * The one who starts the application says how each kind of page reads (`installPageReadStarters`); a tab opened where
 * nobody did, as in a test of some other part, starts nothing.
 */
import { contentLocation, pageHref, pageResource, parsePageAddress } from './contentAddress';
import { findTab, type Layout, type Tab } from './layout';
import type { PageTarget } from './pageTargets';

export type PageReadLease = (() => void) & { shown?: (shown: boolean) => void };

export interface PageReadStarters {
  /** Follows a thread for the tab that will show it. What is returned lets go of it. */
  thread?: (threadId: string, tabId: string) => () => void;
  /** Starts independent reads from an admitted address, without mounting its screen. */
  page?: (target: PageTarget, tabId: string) => PageReadLease;
}

let starters: PageReadStarters = {};
/** What each tab has started that its screen has not yet taken over. */
const started = new Map<string, { address: string; release: PageReadLease; shown: boolean }>();

/** Says how the pages of this application start their reads. */
export function installPageReadStarters(next: PageReadStarters): void {
  for (const entry of started.values()) entry.release();
  started.clear();
  starters = next;
}

/** Starts what the page of a tab needs at once from the address it was opened for. */
export function startPageReads(tab: Tab): void {
  if (tab.view !== 'page') return;
  const location = contentLocation(tab);
  if (location?.view !== 'page') return;
  const target = parsePageAddress(location.href);
  if (target === null) return;
  const resource = pageResource(target);
  const address = resource?.kind === 'thread' ? `thread:${resource.id}` : pageHref(target);
  const held = started.get(tab.id);
  if (held?.address === address) return;
  started.delete(tab.id);
  held?.release();
  const release = resource?.kind === 'thread' ? starters.thread?.(resource.id, tab.id) : starters.page?.(target, tab.id);
  if (release === undefined) return;
  started.set(tab.id, { address, release, shown: true });
}

/** Actual displayed slots refine the initial foreground intent while code is still arriving. */
export function pageReadShown(tab: string, shown: boolean): void {
  const entry = started.get(tab);
  if (entry === undefined || entry.shown === shown) return;
  entry.shown = shown; entry.release.shown?.(shown);
}

/** Closing or explicitly hiding a tab retires its unfinished warm read before its body can mount. */
export function updatePageReadVisibility(layout: Layout): void {
  for (const [id, entry] of started) {
    const place = findTab(layout, id);
    if (place === null) { entry.release(); started.delete(id); continue; }
    const shown = place.area === 'main' ? place.group.active === id && (layout.maximized === null || layout.maximized === place.group.id)
      : place.area === 'bottom' ? layout.bottom.open && place.group.active === id
        : layout[place.area].open && place.container?.name === layout[place.area].active && place.section?.open === true && !place.section.hidden;
    pageReadShown(id, shown);
  }
}

/** Personal chrome and administrator pages need the shell's own state; native resource addresses do not. */
export function pageCanReadBeforeShell(tab: Tab): boolean {
  const location = contentLocation(tab);
  if (location?.view !== 'page') return false;
  const target = parsePageAddress(location.href);
  return target !== null && target.route !== '/' && target.route !== '/workbench/' && !target.pathname.startsWith('/admin')
    && !target.pathname.startsWith('/settings') && target.search.tab !== 'settings' && !target.pathname.includes('/settings');
}

/** The screen of the tab has its own use of what was started: the hold of the tab is not needed any more. */
export function pageTookReads(tabId: string): void {
  started.get(tabId)?.release();
  started.delete(tabId);
}
