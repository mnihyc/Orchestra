/** What starts, and when, from the moment the page's script runs.
 *
 * The page used to ask in a chain, each link waiting for the one before although most needed only to know who the person is:
 * the words and the first draw, then the session, then (after the person's own live feed had joined) the shell's first read,
 * then the page's read, then the navigation tree. Here the session is asked for at script start, beside the code of the
 * screen the address names, and at its answer everything that needs only the person's id starts together: the shell's first
 * read (beside the join of the live feed, see `bootstrap.ts`), the navigation tree of the organization this device remembers,
 * and, as the tab of the address opens, the page's own first read (`features/workbench/pageReads.ts`).
 */
import type { AnyRouter } from '@tanstack/react-router';

import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { meKey } from '@/api/auth';
import { contentLocation, contentTab, pageResource, parseContentAddress, parsePageAddress, sameContentResource } from '@/features/workbench/contentAddress';
import { installPageReadStarters, startPageReads, updatePageReadVisibility, type PageReadLease } from '@/features/workbench/pageReads';
import { openContent } from '@/features/workbench/contentNavigation';
import { useWorkbench } from '@/features/workbench/store';
import { allTabs, findTab, mapGroups } from '@/features/workbench/layout';
import type { PageTarget } from '@/features/workbench/pageTargets';
import type { ScreenKind } from '@/routes/-access';
import { mark } from '@/lib/marks';
import { warmOrgTree, warmThread } from '@/realtime/live';
import { contentThreadRead } from '@/realtime/threadScope';
import { ensureShellBootstrap } from '@/shell/bootstrap';
import { forgetWhenSignedOut, uncover } from '@/shell/keptShell';
import { rememberedOrg, useSelectedOrg } from '@/shell/org';
import { prefetchScreenCode } from '@/shell/screenCode';

let begun = false;

/** Everything that waits for nothing but the session's answer starts together at that answer. */
function fanOut(userId: string, address: string | null): void {
  mark('start/session-answered');
  // The cover observes this confirmed reader and only then reads that person's own saved names.
  mark('start/shell-asked');
  // The first read of the shell lands in the cache by itself; whatever needs it waits for it alone. If it fails the screen
  // says so, and the kept shell does not stand over that.
  void ensureShellBootstrap(queryClient, userId).catch(() => { uncover(); });
  // The left bar begins in the organization this device last showed this person. If the page turns out to belong to another,
  // that one is followed when its screen says so, and this one is let go after a while.
  const org = rememberedOrg(userId);
  if (address !== null && org !== null) {
    if (useSelectedOrg.getState().orgId === null) useSelectedOrg.setState({ orgId: org });
    mark('start/tree-asked');
    warmOrgTree(org);
  }
  if (address !== null) warmAddress(userId, address);
}

function warmAddress(user: string, address: string): void {
  const location = parseContentAddress(address);
  if (location?.view !== 'page') return;
  const target = parsePageAddress(location.href);
  if (target === null || target.route === '/' || target.route === '/workbench/') return;
  const kind = pageResource(target)?.kind;
  const screen: ScreenKind = kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread' || kind === 'document' ? kind : 'page';
  useWorkbench.getState().enter(user, screen);
  const layout = useWorkbench.getState().layout;
  const existing = allTabs(layout).some((tab) => { const held = contentLocation(tab); return held !== null && sameContentResource(held, location); });
  const placeholder = existing ? undefined : allTabs(layout).find((tab) => tab.view === 'screen' && contentLocation(tab) === null);
  let id: string | null;
  if (placeholder === undefined) id = openContent(location);
  else {
    const content = contentTab(location);
    useWorkbench.getState().change((value) => mapGroups(value, (group) => ({ ...group, tabs: group.tabs.map((tab) => tab.id === placeholder.id ? { ...tab, ...content } : tab) })));
    useWorkbench.getState().focus(placeholder.id);
    id = placeholder.id;
  }
  const tab = id === null ? undefined : findTab(useWorkbench.getState().layout, id)?.tab;
  if (tab !== undefined) startPageReads(tab);
}

function startOtherPage(target: PageTarget, tab: string): PageReadLease {
  let disposed = false, shown = true;
  let held: PageReadLease | undefined;
  void import('@/shell/pageFirstReads').then(({ startAddressedPageReads }) => {
    if (!disposed) held = startAddressedPageReads(target, tab, shown);
  }).catch(() => undefined);
  return Object.assign(() => { disposed = true; held?.(); }, { shown: (next: boolean) => { shown = next; held?.shown?.(next); } });
}

/** The code of the screen the address names, fetched while the session is being asked for. */
function warmScreenCode(router: AnyRouter, href: string): void {
  prefetchScreenCode(router, href);
  const target = parsePageAddress(href);
  if (target === null) return;
  void import('@/features/workbench/pages')
    .then(({ pageDefinition }) => pageDefinition(target.route))
    .then((definition) => (definition.Body as unknown as { preload?: () => Promise<unknown> }).preload?.())
    .catch(() => undefined);
}

/**
 * Called once, as the entry's first act: asks for the session now (it needs no words and no first draw), and says what is
 * fanned out at its answer. A sign-in made on the page later fans out the same way.
 */
export function beginStartup(router: AnyRouter, address: string, signedInScreen: boolean): void {
  if (begun) return;
  begun = true;
  installPageReadStarters({ thread: (threadId, tabId) => { mark('start/page-read-asked', { tab: tabId }); return warmThread(contentThreadRead(threadId, tabId)); },
    page: (target, tab) => { mark('start/page-read-asked', { tab }); return startOtherPage(target, tab); } });
  useWorkbench.subscribe((next, previous) => { if (next.layout !== previous.layout) updatePageReadVisibility(next.layout); });
  let user = client.session.getState().userId;
  let first = true;
  client.onSession((event) => {
    if (event.type === 'signed_out') { user = null; return; }
    if (event.type !== 'signed_in') return;
    const changed = user !== event.result.user.id;
    if (changed) queryClient.clear();
    queryClient.setQueryData(meKey, event.result.user);
    user = event.result.user.id;
    fanOut(user, first || changed ? address : null);
    first = false;
  });
  if (signedInScreen) warmScreenCode(router, address);
  // A session that answers that there is none takes what was kept with it; one that cannot be asked leaves the failure to the screen.
  forgetWhenSignedOut(client.session);
  mark('start/session-asked');
  void client.restore().catch(() => { uncover(); });
}
