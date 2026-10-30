/** The order of an org's tree: one choice of each person in each org, kept by the server for every device (`sidebar.order`), and the org's own default (`navigation.order`), which an owner or admin of the org saves from the order they see.
 *
 * The choices are the org's default (as shipped, the most recently changed first), the person's own order (rows
 * arranged by hand within their parent), the newest first, the most recently changed first, and by name. A row
 * that an arrangement does not name stands after those it names, the newest first. Places of one kind stand
 * together (orgs inside, then projects, then topics, then threads) in every order but one's own.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo } from 'react';

import { meStateKey, meStateQuery, setMeState } from '@/api/personal';
import { putSetting, settingsQuery } from '@/api/settings';
import { record } from '@/features/onboarding/state';
import { useOrgTree } from '@/realtime/stores/tree';

export type OrderBy = 'custom' | 'newest' | 'changed' | 'name';

/** An order: how rows are ordered, and for one's own, the rows of each parent in the order they were put. */
export interface TreeOrder {
  by: OrderBy;
  custom?: Readonly<Record<string, readonly string[]>>;
}

/** What a person chose for an org: the org's default, or an order of their own. */
export type OrderChoice = 'default' | OrderBy;

/** The order the org has when nobody saved one: what was worked on lately comes first. */
export const SHIPPED_ORDER: TreeOrder = { by: 'changed' };

/** The setting that keeps an org's own order. */
export const ORDER_SETTING = 'navigation.order';

const BYS: readonly OrderBy[] = ['custom', 'newest', 'changed', 'name'];

const isBy = (value: unknown): value is OrderBy => typeof value === 'string' && (BYS as readonly string[]).includes(value);

/** An order as it is kept, or `null` where what is kept is none. */
export function orderOf(value: unknown): TreeOrder | null {
  const held = record(value);
  if (!isBy(held.by)) return null;
  const custom: Record<string, string[]> = {};
  for (const [parent, ids] of Object.entries(record(held.custom))) {
    if (Array.isArray(ids)) custom[parent] = (ids as unknown[]).filter((id): id is string => typeof id === 'string');
  }
  return Object.keys(custom).length === 0 ? { by: held.by } : { by: held.by, custom };
}

/** What the person chose for an org, from their interface state: the org's default where they chose nothing. */
export function choiceOf(state: Record<string, unknown> | undefined, orgId: string): { choice: OrderChoice; own: TreeOrder | null } {
  const held = record(record(record(state?.sidebar).order)[orgId]);
  const own = orderOf(held);
  return own === null ? { choice: 'default', own: null } : { choice: own.by, own };
}

/** The order the tree is shown in: the person's own choice, else the org's, else as shipped. */
export function effectiveOrder(own: TreeOrder | null, orgDefault: TreeOrder | null): TreeOrder {
  return own ?? orgDefault ?? SHIPPED_ORDER;
}

/** The kept `sidebar` with one org's choice changed, every other member left as the server holds it. */
export function withChoice(sidebar: unknown, orgId: string, order: TreeOrder | null): Record<string, unknown> {
  const held = record(sidebar);
  const orders = Object.fromEntries(Object.entries(record(held.order)).filter(([org]) => org !== orgId));
  return { ...held, order: order === null ? orders : { ...orders, [orgId]: order } };
}

/** The rows of one parent in the order they now stand, with one row moved to another place among them: what dragging it there makes one's own order. */
export function arranged(order: readonly string[], moved: string, before: string | null): string[] {
  const rest = order.filter((id) => id !== moved);
  const at = before === null ? rest.length : rest.indexOf(before);
  return [...rest.slice(0, at < 0 ? rest.length : at), moved, ...rest.slice(at < 0 ? rest.length : at)];
}

/** The order of an org's tree for its reader, and the ways to change it. */
export function useTreeOrder(orgId: string | null, shown = true) {
  const client = useQueryClient();
  const state = useQuery(meStateQuery);
  const tree = useOrgTree(orgId);
  // The visible tree's first page supplies the same native settings, before its rows are published.
  const settings = useQuery({ ...settingsQuery(orgId ?? ''), enabled: shown && orgId !== null && tree.loaded, retry: false });
  // One object while nothing it is made from changes: the tree is laid out again only when the order changes.
  const { choice, order, orgDefault } = useMemo(() => {
    const kept = orderOf(settings.data?.find((setting) => setting.key === ORDER_SETTING)?.value);
    const mine = orgId === null ? { choice: 'default' as const, own: null } : choiceOf(state.data, orgId);
    return { choice: mine.choice, order: effectiveOrder(mine.own, kept), orgDefault: kept };
  }, [orgId, state.data, settings.data]);

  const choose = useMutation({
    mutationFn: async (next: TreeOrder | null) => {
      if (orgId === null) return null;
      // Merge into the current shared state; bootstrap/live updates own this cache.
      const fresh = client.getQueryData<Record<string, unknown>>(meStateQuery.queryKey) ?? await client.query(meStateQuery);
      const sidebar = withChoice(fresh.sidebar, orgId, next);
      await setMeState('sidebar', sidebar);
      return sidebar;
    },
    onSuccess: (sidebar) => {
      if (sidebar !== null) client.setQueryData<Record<string, unknown>>(meStateKey, (held) => ({ ...held, sidebar }));
    },
  });

  const saveForEveryone = useMutation({
    mutationFn: async () => {
      if (orgId === null) return;
      await putSetting({ kind: 'node', id: orgId }, ORDER_SETTING, { value: order });
    },
  });

  return { choice, order, orgDefault: orgDefault ?? SHIPPED_ORDER, choose, saveForEveryone };
}
