/**
 * The reads a page asks for before its first content, kept on this device by address, so that the next document of the same
 * address asks for all of them with its session in one request (`shell/startup.inline.js`, POST /auth/session/open) instead of
 * a chain of requests that each wait for the one before.
 *
 * What is kept is what the page itself asked the last time it opened, as it asked it: a read the page no longer asks for is
 * dropped by the next opening, and a read whose query changes from one opening to the next (a time in it, say) is left out of
 * that address for good, since its kept form would never be the one asked. Only reads are kept, never an answer, and the whole
 * list goes when the person signs out.
 *
 * The code and styles the page needed before that content are kept beside them, for the same release only, so that a browser
 * without them in its cache fetches them all with the document instead of in the waves its imports would find them.
 */
import { pageHtmlReads } from '@/api/htmlReads';

const KEY = 'orch.openings';
/** The shell's read is always part of the opening request, so it is never kept. */
const ALWAYS = new Set(['/me/bootstrap']);
/**
 * Addresses kept: a person opens a handful of pages first, and an address not opened for a while goes first. The figure only
 * bounds the device storage this takes (a few kilobytes); it changes no page's behaviour.
 */
const ADDRESSES = 40;
/**
 * The opening's request line carries every read, and the shipped edge refuses a request line over its 8 KiB header buffer
 * (`large_client_header_buffers`); this leaves room for the shell's read and the path itself.
 */
const QUERY_BYTES = 6_000;
/** The built files kept per address: the first screens of this build reach a few dozen. */
const FILES = 80;
/**
 * After its first content the opening still keeps the reads that content's own answers start (the left bar's tree, a panel
 * beside the page) until nothing new has been asked for two round trips of the 200 ms line the bar is measured at, and for
 * at most three seconds after the content; then the plan is kept and the answers nobody took are let go.
 */
const QUIET_MS = 400;
const WINDOW_MS = 3_000;

interface Plan { reads: string[]; unstable: string[]; at: number; files?: string[]; release?: string }
interface Kept { user: string; plans: Record<string, Plan> }

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const strings = (value: unknown): string[] => Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];

function readKept(): Kept | null {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(KEY) ?? 'null');
    if (!record(value) || typeof value.user !== 'string' || !record(value.plans)) return null;
    const plans: Record<string, Plan> = {};
    for (const [address, plan] of Object.entries(value.plans)) {
      if (record(plan)) plans[address] = { reads: strings(plan.reads), unstable: strings(plan.unstable), at: typeof plan.at === 'number' ? plan.at : 0,
        files: strings(plan.files), ...(typeof plan.release === 'string' ? { release: plan.release } : {}) };
    }
    return { user: value.user, plans };
  } catch { return null; }
}

const pathOf = (read: string) => read.split('?', 1)[0] ?? read;

/** The plan that the opening which asked `asked` leaves for its address, given what the address kept before. */
export function nextPlan(asked: readonly string[], before: Plan | undefined, now: number): Plan {
  const unique = [...new Set(asked)].filter((read) => !ALWAYS.has(pathOf(read)));
  const unstable = new Set(before?.unstable ?? []);
  // A kept read that was not asked again, while the same path was asked with another query, changes between openings.
  for (const read of before?.reads ?? []) {
    if (!unique.includes(read) && unique.some((other) => pathOf(other) === pathOf(read))) unstable.add(pathOf(read));
  }
  const reads: string[] = [];
  let bytes = 0;
  for (const read of unique) {
    if (unstable.has(pathOf(read))) continue;
    bytes += 'read='.length + encodeURIComponent(read).length + 1;
    if (bytes > QUERY_BYTES) break;
    reads.push(read);
  }
  return { reads, unstable: [...unstable], at: now };
}

/** The built scripts, styles and words this document fetched so far, as paths below its origin. */
export function fetchedFiles(entries: readonly PerformanceEntry[]): string[] {
  const files = new Set<string>();
  for (const entry of entries) {
    let url: URL;
    try { url = new URL(entry.name); } catch { continue; }
    if (url.origin === location.origin && /^\/assets\/(?:[\w./-]+\.(?:js|css)|words\/[\w.-]+\.json)$/.test(url.pathname)) files.add(url.pathname.slice(1));
  }
  return [...files].slice(0, FILES);
}

interface Opening { address: string; asked: string[]; user?: string; ends?: number; quiet?: ReturnType<typeof setTimeout> }

/** The opening of this document: from its first script until the reads its first content started are asked. */
let opening: Opening | null = typeof location === 'undefined' ? null
  : { address: pageHtmlReads()?.openingAddress ?? `${location.pathname}${location.search}`, asked: [] };

/** A read the page asked for, as its path and query below the API's base. */
export function askedRead(read: string): void {
  if (opening === null) return;
  opening.asked.push(read);
  if (opening.user !== undefined) settleLater(opening);
}

/** The content of a page is on the screen: the first one ends this document's opening once its own reads are asked. */
export function openingShown(user: string): void {
  const current = opening;
  if (current === null || current.user !== undefined) return;
  current.user = user;
  current.ends = Date.now() + WINDOW_MS;
  pageHtmlReads()?.firstContent?.();
  settleLater(current);
}

function settleLater(current: Opening): void {
  clearTimeout(current.quiet);
  current.quiet = setTimeout(() => { keepPlan(current); }, Math.min(QUIET_MS, Math.max(0, (current.ends ?? 0) - Date.now())));
}

function keepPlan(done: Opening): void {
  if (opening !== done || done.user === undefined) return;
  opening = null;
  pageHtmlReads()?.openingDone?.();
  const user = done.user;
  const held = readKept();
  const kept: Kept = held !== null && held.user === user ? held : { user, plans: {} };
  const release = pageHtmlReads()?.application?.file;
  const plan = nextPlan(done.asked, kept.plans[done.address], Date.now());
  kept.plans[done.address] = release === undefined ? plan
    : { ...plan, release, files: fetchedFiles(typeof performance === 'undefined' ? [] : performance.getEntriesByType('resource')) };
  const addresses = Object.entries(kept.plans).sort(([, a], [, b]) => b.at - a.at).slice(0, ADDRESSES);
  try { localStorage.setItem(KEY, JSON.stringify({ user, plans: Object.fromEntries(addresses) })); }
  catch { /* A device without storage opens each page with its shell alone. */ }
}

/** Nothing of a person's pages is kept after they sign out. */
export function forgetOpenings(): void {
  if (opening !== null) clearTimeout(opening.quiet);
  opening = null;
  try { localStorage.removeItem(KEY); } catch { /* Nothing was kept, then. */ }
}
