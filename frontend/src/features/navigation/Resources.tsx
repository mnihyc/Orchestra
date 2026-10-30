/** The Resources view of the left bar: the kinds of resource as rows, each with how many its reader may see, opening to the things of that kind.
 *
 * Resources belong to whoever may use them and to no one org: the view's heading names no org, and
 * its lists are the ones the screens of resources show, read through the same requests. A kind's row
 * leads to the list of that kind as it was last left; opened, it holds the things of the kind by name,
 * each leading to its own screen in the main area. The kind in view, or the thing in view, is marked,
 * without opening that kind or reading its children merely because a page was visited.
 *
 * Which kinds are open is the reader's own on this device. The rail's Resources stop brings the view
 * to the front of the left bar.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link, useRouterState } from '@tanstack/react-router';
import { Boxes, LoaderCircle, RotateCw } from 'lucide-react';
import { type ReactElement, useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { everythingQuery } from '@/api/everything';
import { useResourceSummary } from '@/features/resources/summary';
import { useSettingsSidebarSlot } from '@/features/settings/sidebarPresence';
import { ResourceAddMenu } from '@/features/resources/ResourceCreation';
import { INVENTORY_KINDS } from '@/features/resources/inventory';
import type { ResourceCounts } from '@/generated/api-types';
import { KINDS } from '@/features/resources/ResourceNav';
import { listAsLeft, type ResourceKind } from '@/features/resources/trail';
import { registerView } from '@/features/workbench/views';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { cx } from '@/ui/cx';
import { Tree, type TreeRow } from '@/ui/Tree';
import { ScrollArea } from '@/ui/ScrollArea';
import { Tooltip } from '@/ui/Tooltip';

/** A thing of a kind, as the view names it. */
interface Listed {
  id: string;
  name: string;
}

/** What is known of the things of one kind: `null` while they have not been read. */
interface KindList {
  things: readonly Listed[] | null;
  failed: boolean;
  retry: () => void;
  more?: boolean;
  readMore?: () => void;
}

/** A row of the view: a kind, a thing of a kind, or the line that says the things of a kind are on their way or could not be read. */
export type ResourceItem =
  | { type: 'kind'; kind: ResourceKind; count: number | null }
  | { type: 'thing'; kind: ResourceKind; id: string; name: string }
  | { type: 'state'; kind: ResourceKind; state: 'loading' | 'failed' | 'more' };

const kindRowId = (kind: ResourceKind) => `kind:${kind}`;
const thingRowId = (kind: ResourceKind, id: string) => `${kind}:${id}`;

/**
 * The rows of the view, in order: every kind there is, in the order the screens of resources name them,
 * each with how many of its things the reader may see, and under a kind that is open its things by name.
 */
export function resourceRows(lists: Readonly<Record<ResourceKind, Pick<KindList, 'things' | 'failed' | 'more'>>>, open: ReadonlySet<ResourceKind>, locale: string, counts?: ResourceCounts): TreeRow<ResourceItem>[] {
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
  const rows: TreeRow<ResourceItem>[] = [];
  for (const { name: kind } of KINDS) {
    const { things, failed } = lists[kind];
    const id = kindRowId(kind);
    // A kind with nothing in it has nothing to open; one that is still unknown may have.
    const count = counts === undefined ? things?.length ?? null : counts[kind];
    const expandable = count === null || count > 0;
    const expanded = expandable && open.has(kind);
    rows.push({ id, depth: 0, parentId: null, expandable, expanded, item: { type: 'kind', kind, count } });
    if (!expanded) continue;
    if (things === null) {
      rows.push({ id: `state:${kind}`, depth: 1, parentId: id, expandable: false, expanded: false, item: { type: 'state', kind, state: failed ? 'failed' : 'loading' } });
      continue;
    }
    for (const thing of [...things].sort((one, other) => collator.compare(one.name, other.name) || (one.id < other.id ? -1 : 1))) {
      rows.push({ id: thingRowId(kind, thing.id), depth: 1, parentId: id, expandable: false, expanded: false, item: { type: 'thing', kind, id: thing.id, name: thing.name } });
    }
    if (failed || lists[kind].more === true) rows.push({ id: `state:${kind}`, depth: 1, parentId: id, expandable: false, expanded: false, item: { type: 'state', kind, state: failed ? 'failed' : 'more' } });
  }
  return rows;
}

/** Only an expanded kind reads its names, in bounded pages without resource-detail readers. */
function useBranch(kind: ResourceKind, open: ReadonlySet<ResourceKind>, reader: string | null): KindList {
  const shown = useViewShown();
  const options = everythingQuery(reader ?? '', 'resources', { scope: 'mine', kinds: [INVENTORY_KINDS[kind]],
    q: '', owner: null, createdFrom: '', createdTo: '', sort: 'name', direction: 'asc' }, 'resource-navigation');
  const query = useInfiniteQuery({ ...options, enabled: shown && reader !== null && open.has(kind) });
  return { things: query.data?.pages.flatMap((page) => page.items.flatMap((row) => row.name === null ? [] : [{ id: row.id, name: row.name }])) ?? null,
    failed: query.isError, retry: () => { if (query.isFetchNextPageError) void query.fetchNextPage(); else void query.refetch(); },
    more: query.hasNextPage, readMore: () => void query.fetchNextPage() };
}

function useKindLists(open: ReadonlySet<ResourceKind>, reader: string | null): Record<ResourceKind, KindList> {
  return { providers: useBranch('providers', open, reader), agents: useBranch('agents', open, reader),
    machines: useBranch('machines', open, reader), environments: useBranch('environments', open, reader),
    templates: useBranch('templates', open, reader), hosts: useBranch('hosts', open, reader),
    mcp: useBranch('mcp', open, reader), mounts: useBranch('mounts', open, reader), skills: useBranch('skills', open, reader) };
}

// The old key mixed deliberate choices with automatic reveals from visits. Keep only explicit expansions now.
const openKey = (userId: string) => `orch.${userId}.resources.expanded`;

function readOpen(userId: string | null): ReadonlySet<ResourceKind> {
  if (userId === null) return new Set();
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(openKey(userId)) ?? '[]');
    return new Set(Array.isArray(stored) ? KINDS.map((kind) => kind.name).filter((name) => stored.includes(name)) : []);
  } catch {
    return new Set();
  }
}

function writeOpen(userId: string | null, open: ReadonlySet<ResourceKind>): void {
  if (userId === null) return;
  try {
    localStorage.setItem(openKey(userId), JSON.stringify([...open]));
  } catch {
    // A device that keeps nothing shows every kind closed next time.
  }
}

/** The kind of a screen of resources and the thing it is of, from its address. */
const RESOURCE_ADDRESS = /^\/resources\/([a-z]+)(?:\/([^/]+))?\/?$/;

const ROW_LINK = 'flex min-w-0 flex-1 items-center gap-2 self-stretch outline-none';

/**
 * The link of a kind's row: to the list of that kind as it was last left, with what was typed and chosen there still in
 * it. It follows the address, since a list says how it was left whenever its own address changes.
 */
function KindLink({ kind }: { kind: (typeof KINDS)[number] }) {
  const { t } = useTranslation();
  useRouterState({ select: (state) => state.location.href });
  const search = listAsLeft(kind.name);
  const Icon = kind.icon;
  return (
    <Link to={kind.to} {...(search === undefined ? {} : { search })} tabIndex={-1} className={ROW_LINK}>
      <span className="flex size-4 shrink-0 items-center justify-center text-text-subtle">
        <Icon aria-hidden />
      </span>
      <span className="truncate">{t(`screens.${kind.name}.title`)}</span>
    </Link>
  );
}

export function ResourcesView() {
  const { t } = useTranslation();
  useSettingsSidebarSlot('resources');
  const userId = useStore(client.session, (state) => state.userId);
  const locale = useAppearance((state) => state.locale);
  const summary = useResourceSummary();
  const address = useRouterState({ select: (state) => state.location.pathname });
  const found = RESOURCE_ADDRESS.exec(address);
  const kindInView = KINDS.find((kind) => kind.name === found?.[1])?.name ?? null;
  const thingInView = kindInView === null ? null : (found?.[2] ?? null);

  const [opened, setOpened] = useState<{ userId: string | null; kinds: ReadonlySet<ResourceKind> } | null>(null);
  const kept = useMemo(() => readOpen(userId), [userId]);
  const open = opened !== null && opened.userId === userId ? opened.kinds : kept;
  const lists = useKindLists(open, userId);
  useEffect(() => {
    if (opened !== null) writeOpen(opened.userId, opened.kinds);
  }, [opened]);
  const rows = resourceRows(lists, open, locale, summary.data);
  const onExpandedChange = useCallback(
    (id: string, expanded: boolean) => {
      const kind = KINDS.find((one) => kindRowId(one.name) === id)?.name;
      if (kind === undefined) return;
      const next = new Set(open);
      if (expanded) next.add(kind);
      else next.delete(kind);
      setOpened({ userId, kinds: next });
    },
    [open, userId],
  );
  const renderRow = useCallback(
    (row: TreeRow<ResourceItem>) => {
      const item = row.item;
      if (item.type === 'kind') {
        const kind = KINDS.find((one) => one.name === item.kind);
        if (kind === undefined) return null;
        return (
          <>
            <KindLink kind={kind} />
            {item.count !== null && <span className="shrink-0 pr-2 text-sm text-text-muted tabular-nums">{formatNumber(item.count, locale)}</span>}
          </>
        );
      }
      if (item.type === 'thing') {
        return (
          // All eight resource kinds use this address shape.
          // Its name begins under the name of its kind.
          <Link to={`/resources/${item.kind}/$id` as '/resources/providers/$id'} params={{ id: item.id }} tabIndex={-1} className={cx(ROW_LINK, 'pl-2')}>
            <span className="truncate">{item.name}</span>
          </Link>
        );
      }
      if (item.state === 'loading') {
        return (
          <span className="flex min-w-0 flex-1 items-center gap-2 text-text-muted">
            <LoaderCircle className="animate-spin" aria-hidden />
            <span className="truncate">{t('common.loading')}</span>
          </span>
        );
      }
      if (item.state === 'more') return <button type="button" tabIndex={-1} className={cx(ROW_LINK, 'text-text-muted hover:text-text')} onClick={lists[item.kind].readMore}>{t('everything.more')}</button>;
      return (
        <button type="button" tabIndex={-1} className="flex min-w-0 flex-1 items-center gap-2 self-stretch text-left text-text-muted outline-none hover:text-text" onClick={lists[item.kind].retry}>
          <RotateCw aria-hidden className="shrink-0 text-text-subtle" />
          <span className="truncate underline underline-offset-2">{t('navigation.resources.retry')}</span>
        </button>
      );
    },
    [t, locale, lists],
  );
  // A typed letter finds a kind by its name as well as a thing by its own.
  const nameOf = useCallback((item: ResourceItem): string => (item.type === 'thing' ? item.name : item.type === 'kind' ? t(`screens.${item.kind}.title`) : ''), [t]);
  const wrapRow = useCallback((row: TreeRow<ResourceItem>, element: ReactElement) => row.item.type === 'state' ? element : <Tooltip content={nameOf(row.item)}>{element}</Tooltip>, [nameOf]);
  const current = kindInView === null ? null : thingInView === null ? kindRowId(kindInView) : rows.some((row) => row.id === thingRowId(kindInView, thingInView)) ? thingRowId(kindInView, thingInView) : kindRowId(kindInView);

  return (
    <nav aria-label={t('navigation.resources.title')} className="flex h-full min-h-0 flex-col bg-bg-subtle">
      <ScrollArea className="flex flex-1 flex-col py-1">
        {summary.isError && <button type="button" className="focus-inset min-h-9 px-3 text-left text-sm text-text-muted" onClick={() => void summary.refetch()}>{t('navigation.resources.retry')}</button>}
        <Tree label={t('navigation.resources.tree')} rows={rows} currentId={current} onExpandedChange={onExpandedChange} nameOf={nameOf} renderRow={renderRow} wrapRow={wrapRow} />
      </ScrollArea>
    </nav>
  );
}

/**
 * The name the view is registered under, and tabs hold. Its heading and its tab say Resources, and so does the view of a thread's right
 * bar that draws the load of its environment, which is registered as `resources`: the two are told apart by this name, or the one that
 * is registered later takes the place of the other.
 */
export const RESOURCES_VIEW = 'resource-navigation';

registerView({
  name: RESOURCES_VIEW,
  title: 'navigation.resources.title',
  about: 'navigation.resources.about',
  icon: Boxes,
  screens: 'all',
  area: 'left',
  add: ResourceAddMenu,
  body: ResourcesView,
});
