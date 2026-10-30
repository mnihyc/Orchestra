/** A paged flat Navigation list using the tree's rows, menus, marks and keyboard. */
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { Ellipsis, ListTree, MessageSquare } from 'lucide-react';
import { Fragment, type ReactElement, useCallback, useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { everythingQuery, type EverythingFilter } from '@/api/everything';
import { useOpenPath } from '@/features/chat/open';
import { useViewShown } from '@/features/workbench/viewOpening';
import { DocContextMenu, DocMenuButton } from './DocMenu';
import { NodeContextMenu } from './NodeMenu';
import { NavigationDropProvider, NodeRow } from './NodeTree';
import { placeOfRow, type DocPlace } from './places';
import type { Needs, NavItem } from './treeRows';
import { groupedRecentsOnDay, recentAt, recentDay, recentGroupOnDay, type RecentGroup } from './recent';
import { useListingRetired, useRecentUpdates } from './useRecentUpdates';
import type { EverythingRow } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatExact, useNow } from '@/lib/time';
import type { OrgTree } from '@/realtime/stores/tree';
import { reloadOrgTree } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { NodeIcon } from '@/ui/NodeIcon';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { containedThreadStates } from '@/lib/threadDisplay';
import { RelativeTime } from '@/ui/RelativeTime';
import { nearestPlace, RowEnd, RowPlace } from '@/ui/RowDetails';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Tree, type TreeRow } from '@/ui/Tree';

interface RecentItem { row: EverythingRow; heading: RecentGroup | null }
export interface RecentListProps {
  reader: string;
  parent: string;
  orgId: string;
  filter: EverythingFilter;
  tree: OrgTree;
  current: string | null;
  needs: (ids: readonly string[]) => Needs;
  onShowTree: (row: EverythingRow) => void;
}

/** Rows have no detail reads; their ordinary menus still load permissions only when asked. */
export function RecentList({ reader, parent, orgId, filter, tree, current, needs, onShowTree }: RecentListProps) {
  const { t } = useTranslation();
  const open = useOpenPath();
  const shown = useViewShown();
  const q = useDebounced(filter.q.trim());
  const ask = { ...filter, q };
  const options = everythingQuery(reader, parent, ask);
  const query = useInfiniteQuery({ ...options, enabled: shown && tree.loaded });
  const retired = useListingRetired(options.queryKey, query.error);
  const client = useQueryClient();
  const seq = useRef(tree.lastSeq);
  useEffect(() => {
    if (seq.current === tree.lastSeq) return;
    seq.current = tree.lastSeq;
    if (shown) void client.invalidateQueries({ queryKey: options.queryKey, exact: true });
  }, [tree.lastSeq, shown, client, options.queryKey]);
  const now = useNow();
  const timeZone = useAppearance((state) => state.timeZone);
  const locale = useAppearance((state) => state.locale);
  const today = recentDay(now, timeZone);
  const order = filter.recent ?? 'opened';
  useRecentUpdates(options.queryKey, shown && tree.loaded, order === 'opened');
  const rows = useMemo<TreeRow<RecentItem>[]>(() => {
    const items = groupedRecentsOnDay(query.data?.pages.flatMap((page) => page.items) ?? [], order, today, timeZone);
    return items.map((row, index) => {
      const group = recentGroupOnDay(recentAt(row, order), today, timeZone);
      const previous = items[index - 1];
      const heading = previous !== undefined && group === recentGroupOnDay(recentAt(previous, order), today, timeZone) ? null : group;
      return { id: `${row.kind}:${row.id}`, parentId: null, depth: 0, expandable: false, expanded: false, item: { row, heading } };
    });
  }, [query.data, order, today, timeZone]);
  const docOf = useCallback((row: EverythingRow): DocPlace | null => {
    const node = row.path.at(-1);
    return node === undefined ? null : { id: row.id, title: row.name ?? '', node, archived: tree.nodes.get(node.id)?.archived ?? false, owner: row.owner };
  }, [tree.nodes]);
  const extra = useCallback((row: EverythingRow): MenuEntry[] => {
    const id = row.kind === 'document' || row.kind === 'chat' ? row.path.at(-1)?.id ?? row.parent_id : row.id;
    const target = id === null ? undefined : tree.nodes.get(id);
    return [{ id: 'show-in-tree', label: t('navigation.showInTree'), icon: <ListTree aria-hidden />, onSelect: () => onShowTree(row),
      ...(target === undefined || target.deleted ? { disabled: t('navigation.begins.notInTree') } : {}) }];
  }, [t, onShowTree, tree.nodes]);
  const chatEntries = useCallback((row: EverythingRow): MenuEntry[] => [
    ...(row.href === null ? [] : [{ id: 'open', label: t('places.menu.open'), href: row.href, onSelect: () => row.href !== null && open(row.href) }]), ...extra(row),
  ], [open, t, extra]);
  const render = useCallback(({ item: { row } }: TreeRow<RecentItem>) => {
    const node = tree.nodes.get(row.id);
    const at = recentAt(row, order);
    const where = nearestPlace(row);
    const time = at === null ? null : <RelativeTime compact at={at} />;
    const exact = at === null ? undefined : formatExact(at, { locale, timeZone });
    let body: ReactElement;
    if (node !== undefined && !node.deleted && row.kind !== 'chat' && row.kind !== 'document') {
      const held = needs([node.id]);
      const display = node.thread?.display;
      const item: Extract<NavItem, { type: 'node' }> = { type: 'node', node, folder: node.kind === 'project' && tree.nodes.get(node.parent_id ?? '')?.kind === 'project',
        urgent: node.kind === 'thread' ? display?.state ?? null : containedThreadStates(tree.nodes).get(node.id) ?? null,
        unread: held.unread, waiting: held.waiting, mentions: held.mentions, chat: 'none', chatMentions: 0 };
      body = <NodeRow item={item} where={where} end={time} endTitle={exact} extraEntries={extra(row)} />;
    } else {
      const doc = row.kind === 'document' ? docOf(row) : null;
      body = <><a href={row.href ?? undefined} tabIndex={-1} title={row.place_label === null ? row.name ?? '' : `${row.name ?? ''} · ${row.place_label}`}
        data-row-name-place={where !== null || undefined}
        aria-label={row.place_label === null ? row.name ?? '' : t('everything.placedName', { name: row.name, place: row.place_label })}
        className="flex min-w-0 flex-1 items-center gap-2 self-stretch outline-none" onClick={(event) => {
          if (row.href === null || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
          event.preventDefault(); open(row.href);
        }}>
        {row.kind === 'thread' ? <ThreadStateIcon display={row.my_access === 'path_only' ? undefined : row.thread_display} />
          : row.kind === 'chat' ? <MessageSquare className="size-4 shrink-0 text-text-muted" aria-hidden /> : <NodeIcon kind={row.kind === 'org' || row.kind === 'project' || row.kind === 'topic' ? row.kind : 'document'} />}
        <span className={cx('truncate', row.document_unread === true && 'font-semibold text-text')}>{row.name}</span>
        <RowPlace value={where} />
      </a><RowEnd time={time} title={exact}>{doc !== null ? <DocMenuButton doc={doc} skipTab extraEntries={extra(row)} />
        : <Menu label={t('places.menu.of', { name: row.name })} entries={chatEntries(row)} trigger={<IconButton size="sm" label={t('places.menu.more', { name: row.name })} icon={<Ellipsis aria-hidden />} tabIndex={-1} />} />}</RowEnd></>;
    }
    return body;
  }, [tree.nodes, needs, order, docOf, extra, chatEntries, open, t, locale, timeZone]);
  const wrap = useCallback(({ item: { row, heading } }: TreeRow<RecentItem>, element: ReactElement) => {
    const node = tree.nodes.get(row.id);
    const doc = row.kind === 'document' ? docOf(row) : null;
    const context = node !== undefined && row.kind !== 'chat' && row.kind !== 'document'
      ? <NodeContextMenu place={placeOfRow(node, node.kind === 'project' && tree.nodes.get(node.parent_id ?? '')?.kind === 'project')} extraEntries={extra(row)}>{element}</NodeContextMenu>
      : doc !== null ? <DocContextMenu doc={doc} extraEntries={extra(row)}>{element}</DocContextMenu>
        : <ContextMenu label={t('places.menu.of', { name: row.name })} entries={chatEntries(row)}>{element}</ContextMenu>;
    return <Fragment>{heading !== null && <h3 className="flex min-h-(--section-heading-height) items-end px-6 pt-3 pb-1 text-sm font-medium text-text-muted">{t(`navigation.recentGroups.${heading}`)}</h3>}{context}</Fragment>;
  }, [tree.nodes, docOf, extra, chatEntries, t]);
  if (!tree.loaded && tree.error !== null) return <ErrorState compact error={tree.error} onRetry={() => reloadOrgTree(orgId)} />;
  if (query.isPending) return <SkeletonRows rows={6} />;
  if (query.isError && (rows.length === 0 || retired)) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  if (rows.length === 0) return <StateLine compact title={t(q !== '' ? 'navigation.filter.none' : order === 'active' ? 'navigation.noActivity' : 'navigation.noRecent')} className="px-6" />;
  return <>
    {query.isError && <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />}
    <div className={query.isError ? 'opacity-60' : undefined}><NavigationDropProvider nodes={tree.nodes}><Tree flat label={t('navigation.arrangement.recent')} rows={rows} currentId={current} nameOf={(item) => item.row.name ?? ''} renderRow={render} wrapRow={wrap} onExpandedChange={() => undefined} /></NavigationDropProvider></div>
    {query.hasNextPage && <Button size="sm" variant="ghost" pending={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>{t('everything.more')}</Button>}
  </>;
}
