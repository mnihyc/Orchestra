/** What a tab's page starts reading at the moment the tab is opened for its address.
 *
 * A page used to begin its first read only when its screen had been drawn, several components after the tab was opened,
 * although the address already says what it is. The reads that need nothing but that address start here instead, and the
 * screen, when it comes, finds them under way (the feed registry shares one follower between whoever asks).
 *
 * The one who starts the application says how each kind of page reads (`installPageReadStarters`); a tab opened where
 * nobody did, as in a test of some other part, starts nothing.
 */
import { contentLocation, pageResource, parsePageAddress } from './contentAddress';
import type { Tab } from './layout';

export interface PageReadStarters {
  /** Follows a thread for the tab that will show it. What is returned lets go of it. */
  thread?: (threadId: string, tabId: string) => () => void;
}

let starters: PageReadStarters = {};
/** What each tab has started that its screen has not yet taken over. */
const started = new Map<string, { thread: string; release: () => void }>();

/** Says how the pages of this application start their reads. */
export function installPageReadStarters(next: PageReadStarters): void {
  starters = next;
}

/** Starts what the page of a tab needs at once from the address it was opened for. */
export function startPageReads(tab: Tab): void {
  const follow = starters.thread;
  if (tab.view !== 'page' || follow === undefined) return;
  const location = contentLocation(tab);
  if (location?.view !== 'page') return;
  const resource = pageResource(parsePageAddress(location.href));
  if (resource?.kind !== 'thread') return;
  const held = started.get(tab.id);
  if (held?.thread === resource.id) return;
  held?.release();
  started.set(tab.id, { thread: resource.id, release: follow(resource.id, tab.id) });
}

/** The screen of the tab has its own use of what was started: the hold of the tab is not needed any more. */
export function pageTookReads(tabId: string): void {
  started.get(tabId)?.release();
  started.delete(tabId);
}
