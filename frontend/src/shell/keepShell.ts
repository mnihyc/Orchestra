/** Writes what `keptShell.ts` describes: a short while after each change of the tabs, the bars, the tree in view or the person's name,
 * and when the page is put away. It lives with the frame, which is the only part that knows those things; the page's entry only reads.
 */
import type { QueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';

import { meKey } from '@/api/auth';
import { client } from '@/api/client';
import { orgsKey } from '@/api/nodes';
import { queryClient } from '@/api/query';
import { contentLocation, pageResource, parsePageAddress } from '@/features/workbench/contentAddress';
import { type Layout, mainGroups, type Tab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { readOpen } from '@/features/navigation/beginning';
import { firstExpanded } from '@/features/navigation/treeRows';
import type { Me, Org, TreeNode } from '@/generated/api-types';
import { type OrgTree, useTreeStore } from '@/realtime/stores/tree';
import { KEPT_NAME_CHARS, KEPT_ROWS, KEPT_TABS, type KeptRow, type KeptShell, type KeptTab, writeKeptShell } from '@/shell/keptShell';
import { useSelectedOrg } from '@/shell/org';

/** How long after a change the record is written: changes that follow one another are written once. A default of the build. */
export const KEEP_SHELL_AFTER_MS = 500;
/** How deep into the tree the record goes. */
const KEPT_DEPTH = 3;

const shorten = (name: string): string => Array.from(name.trim()).slice(0, KEPT_NAME_CHARS).join('');

/** The kind of thing a tab shows, by one word: what its icon would say. */
export function kindOfTab(tab: Tab): string {
  const location = contentLocation(tab);
  if (location?.view === 'page') return pageResource(parsePageAddress(location.href))?.kind ?? 'page';
  return location?.view ?? tab.view;
}

const KIND_ORDER: Readonly<Record<string, number>> = { project: 0, topic: 1, thread: 2 };

/** The rows of a tree as they showed: the places that were open, in order, to a few levels. */
export function treeRows(tree: OrgTree, org: string, open: ReadonlySet<string>): KeptRow[] {
  const children = new Map<string, TreeNode[]>();
  for (const node of tree.nodes.values()) {
    if (node.deleted || node.archived || node.parent_id === null) continue;
    children.set(node.parent_id, [...(children.get(node.parent_id) ?? []), node]);
  }
  const rows: KeptRow[] = [];
  const visit = (parent: string, depth: number) => {
    const here = (children.get(parent) ?? []).sort((one, other) => (KIND_ORDER[one.kind] ?? 3) - (KIND_ORDER[other.kind] ?? 3) || one.name.localeCompare(other.name));
    for (const node of here) {
      if (rows.length >= KEPT_ROWS) return;
      rows.push({ depth, kind: node.kind, name: shorten(node.name) });
      if (depth + 1 < KEPT_DEPTH && open.has(node.id)) visit(node.id, depth + 1);
    }
  };
  visit(org, 0);
  return rows;
}

export interface KeptInputs {
  user: string;
  name: string;
  layout: Layout;
  focusedMainGroup: string | null;
  titles: ReadonlyMap<string, { title: string }>;
  org: string | null;
  tree: OrgTree | undefined;
  open: ReadonlySet<string> | null;
  now: number;
}

/** What the record says of what is on the screen now. */
export function buildKeptShell(input: KeptInputs): KeptShell {
  const groups = mainGroups(input.layout.main);
  const group = groups.find((each) => each.id === input.focusedMainGroup) ?? groups[0];
  const tabs: KeptTab[] = (group?.tabs ?? []).slice(0, KEPT_TABS).map((tab) => ({
    id: tab.id, title: shorten(input.titles.get(tab.id)?.title ?? ''), kind: kindOfTab(tab), front: group?.active === tab.id,
  }));
  const tree = input.org === null || input.tree?.loaded !== true ? null
    : { org: input.org, rows: treeRows(input.tree, input.org, input.open ?? new Set(firstExpanded(input.tree))) };
  return {
    v: 1, user: input.user, name: shorten(input.name), kept: input.now,
    left: { open: input.layout.left.open, size: input.layout.left.size }, right: { open: input.layout.right.open, size: input.layout.right.size },
    tabs, tree,
  };
}

function keepNow(user: string, cache: QueryClient): void {
  if (client.session.getState().userId !== user) return;
  const state = useWorkbench.getState();
  if (state.userId !== user) return;
  const org = useSelectedOrg.getState().orgId;
  const person = cache.getQueryData<Me>(meKey);
  const known = cache.getQueryData<Org[]>(orgsKey)?.some((each) => each.id === org) === true;
  writeKeptShell(buildKeptShell({
    user, name: person?.display_name ?? '', layout: state.layout, focusedMainGroup: state.focusedMainGroup, titles: state.named,
    org: known ? org : null, tree: org === null ? undefined : useTreeStore.getState().byOrg.get(org),
    open: org === null ? null : readOpen(user, org), now: Date.now(),
  }));
}

/** Keeps the shell of `user`, once the frame stands admitted and for as long as it does. */
export function useKeepShell(user: string, admitted: boolean): void {
  useEffect(() => {
    if (!admitted) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const keep = () => { timer = undefined; keepNow(user, queryClient); };
    const later = () => { timer ??= setTimeout(keep, KEEP_SHELL_AFTER_MS); };
    const away = () => {
      if (document.visibilityState !== 'hidden') return;
      clearTimeout(timer);
      keep();
    };
    later();
    const stops = [useWorkbench.subscribe(later), useTreeStore.subscribe(later), useSelectedOrg.subscribe(later)];
    document.addEventListener('visibilitychange', away);
    window.addEventListener('pagehide', away);
    return () => {
      clearTimeout(timer);
      for (const stop of stops) stop();
      document.removeEventListener('visibilitychange', away);
      window.removeEventListener('pagehide', away);
    };
  }, [user, admitted]);
}
