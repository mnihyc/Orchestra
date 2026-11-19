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

export type PageReadLease = (() => void) & { shown?: (shown: boolean) => void; took?: () => boolean };

export interface PageReadStarters {
  /** Follows a thread for the tab that will show it. What is returned lets go of it. */
  thread?: (threadId: string, tabId: string) => PageReadLease;
  /** Starts independent reads from an admitted address, without mounting its screen. */
  page?: (target: PageTarget, tabId: string) => PageReadLease;
}

let starters: PageReadStarters = {};
/** A tab owns its initial reads and any dependent parts still admitting beside its body. */
const started = new Map<string, { address: string; release: PageReadLease; shown: boolean }>();
/** Only bodies still owned by the retained host may skip their first-read starter on Back. */
const mounted = new Map<string, string>();

function reading(tab: Tab) {
  if (tab.view !== 'page') return null;
  const location = contentLocation(tab);
  if (location?.view !== 'page') return null;
  const target = parsePageAddress(location.href);
  if (target === null) return null;
  const resource = pageResource(target);
  const address = resource?.kind === 'thread' ? `thread:${resource.id}` : pageHref(target);
  return { target, resource, address };
}

/** Says how the pages of this application start their reads. */
export function installPageReadStarters(next: PageReadStarters): void {
  for (const entry of started.values()) entry.release();
  started.clear();
  mounted.clear();
  starters = next;
}

/** Starts what the page of a tab needs at once from the address it was opened for. */
export function startPageReads(tab: Tab): void {
  const request = reading(tab);
  if (request === null) return;
  const { target, resource, address } = request;
  const held = started.get(tab.id);
  if (held?.address === address) return;
  started.delete(tab.id);
  held?.release();
  if (mounted.get(tab.id) === address) return;
  mounted.delete(tab.id);
  const leases = [resource?.kind === 'thread' ? starters.thread?.(resource.id, tab.id) : undefined,
    starters.page?.(target, tab.id)].filter((lease): lease is PageReadLease => lease !== undefined);
  if (leases.length === 0) return;
  let taken = false, retained = false;
  const release = Object.assign(() => { for (const lease of leases) lease(); }, {
    took: () => {
      if (!taken) {
        taken = true;
        for (const lease of leases) { const keep = lease.took?.() === true; retained = keep || retained; }
      }
      return retained;
    },
    shown: (shown: boolean) => { for (const lease of leases) lease.shown?.(shown); },
  });
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

/** The body takes over its readers; dependent parts keep their tab's visibility and disposal. */
export function pageTookReads(tab: Tab): void {
  const request = reading(tab);
  const pending = started.get(tab.id);
  // A delayed effect from the former address cannot retire its replacement's supplier.
  if (pending !== undefined && pending.address !== request?.address) return;
  if (pending?.release.took?.() !== true) { pending?.release(); started.delete(tab.id); }
  if (request !== null) mounted.set(tab.id, request.address);
}

/** Disposal of a body, rather than hiding its effects, releases its address ownership. */
export function forgetPageBody(tab: string): void {
  mounted.delete(tab);
  started.get(tab)?.release(); started.delete(tab);
}

export function clearPageBodies(): void {
  for (const entry of started.values()) entry.release();
  started.clear(); mounted.clear();
}
